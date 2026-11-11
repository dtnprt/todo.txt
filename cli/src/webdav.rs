//! WebDAV as storage backend (shared by the terminal app and the desktop app via `#[path]`).
//!
//! A *server* is configured as `USER@URL` (config key `webdav=["alice@https://host/dav/"]`); the password is
//! never written to a file: it lives in the OS keyring (service `todo.txt`, account = the entry), with the
//! environment variable `TODO_WEBDAV_PASSWORD` as fallback for machines without a keyring.
//! Any file path that starts with `http://` or `https://` is a remote file on such a server (the server whose
//! URL is a prefix of it). Reading is `GET`, writing `PUT` guarded by `If-Match: <etag>` (a change made
//! meanwhile by someone else gives an error instead of being overwritten), listing is `PROPFIND`.

use std::io::Read;
use std::time::Duration;

pub const KEYRING_SERVICE: &str = "todo.txt";

/// One configured server.
#[derive(Debug, Clone, PartialEq)]
pub struct Server {
    pub user: String,
    /// Base URL, always with a trailing `/`.
    pub url: String,
}

impl Server {
    /// `USER@URL` (the form kept in the config file and used as keyring account).
    pub fn entry(&self) -> String {
        format!("{}@{}", self.user, self.url)
    }

    pub fn parse(entry: &str) -> Option<Server> {
        let entry = entry.trim();
        let at = entry.find("@http://").or_else(|| entry.find("@https://"))?;
        let user = entry[..at].to_string();
        let mut url = entry[at + 1..].to_string();
        if !url.ends_with('/') {
            url.push('/');
        }
        Some(Server { user, url })
    }
}

pub fn is_remote(path: &str) -> bool {
    let p = path.trim_start().to_ascii_lowercase();
    p.starts_with("http://") || p.starts_with("https://")
}

/// The configured server (entries as in the config file) whose base URL is a prefix of `url`.
pub fn server_for(entries: &[String], url: &str) -> Option<Server> {
    let url = ensure_slash_if_needed(url);
    entries
        .iter()
        .filter_map(|e| Server::parse(e))
        .filter(|s| url.starts_with(&s.url))
        .max_by_key(|s| s.url.len())
}

fn ensure_slash_if_needed(url: &str) -> String {
    // `https://host/dav` (the server itself, no trailing slash) also belongs to `https://host/dav/`
    if url.ends_with('/') { url.to_string() } else { format!("{url}/") }
}

// ---------------------------------------------------------------- credentials

pub mod cred {
    use super::*;

    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    fn entry(server: &Server) -> Result<keyring::Entry, String> {
        keyring::Entry::new(KEYRING_SERVICE, &server.entry()).map_err(|e| e.to_string())
    }

    /// The stored password (keyring first, then `TODO_WEBDAV_PASSWORD`).
    pub fn get(server: &Server) -> Result<String, String> {
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        {
            if let Ok(p) = entry(server).and_then(|e| e.get_password().map_err(|e| e.to_string())) {
                return Ok(p);
            }
        }
        std::env::var("TODO_WEBDAV_PASSWORD")
            .map_err(|_| format!("no password stored for {} (log in first)", server.entry()))
    }

    pub fn set(server: &Server, password: &str) -> Result<(), String> {
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        {
            entry(server)?.set_password(password).map_err(|e| format!("keyring: {e}"))
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            let _ = (server, password);
            Err("no keyring on this platform".into())
        }
    }

    pub fn delete(server: &Server) -> Result<(), String> {
        #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
        {
            match entry(server)?.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(format!("keyring: {e}")),
            }
        }
        #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
        {
            let _ = server;
            Ok(())
        }
    }

    pub fn exists(server: &Server) -> bool {
        get(server).is_ok()
    }
}

// ---------------------------------------------------------------- HTTP

#[derive(Debug)]
pub enum Error {
    NotFound,
    /// The file changed on the server since it was read (HTTP 412).
    Conflict,
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotFound => write!(f, "not found on the server"),
            Error::Conflict => write!(f, "the file was changed on the server meanwhile (reload it first)"),
            Error::Other(s) => write!(f, "{s}"),
        }
    }
}

impl From<Error> for String {
    fn from(e: Error) -> String {
        e.to_string()
    }
}

fn base64(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for c in input.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        out.push(T[(n >> 18) as usize & 63] as char);
        out.push(T[(n >> 12) as usize & 63] as char);
        out.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    out
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(20)).build()
}

fn call(server: &Server, method: &str, url: &str, headers: &[(&str, &str)], body: Option<&str>) -> Result<ureq::Response, Error> {
    let pass = cred::get(server).map_err(Error::Other)?;
    let auth = format!("Basic {}", base64(format!("{}:{}", server.user, pass).as_bytes()));
    let mut req = agent().request(method, url).set("Authorization", &auth);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    let res = match body {
        Some(b) => req.send_string(b),
        None => req.call(),
    };
    match res {
        Ok(r) => Ok(r),
        Err(ureq::Error::Status(404, _)) => Err(Error::NotFound),
        Err(ureq::Error::Status(412, _)) => Err(Error::Conflict),
        Err(ureq::Error::Status(401, _)) => Err(Error::Other("login failed (401): check user name and password".into())),
        Err(ureq::Error::Status(403, _)) => Err(Error::Other("access denied (403)".into())),
        Err(ureq::Error::Status(c, r)) => Err(Error::Other(format!("server answered {c} {}", r.status_text()))),
        Err(e) => Err(Error::Other(e.to_string())),
    }
}

/// Text of a remote file and its ETag.
pub fn get(server: &Server, url: &str) -> Result<(String, Option<String>), Error> {
    let res = call(server, "GET", url, &[], None)?;
    let etag = res.header("ETag").map(str::to_string);
    let mut text = String::new();
    res.into_reader().take(32 << 20).read_to_string(&mut text).map_err(|e| Error::Other(e.to_string()))?;
    Ok((text, etag))
}

/// Write a remote file. With `etag` the write only succeeds if the file is still the version that was read
/// (`Error::Conflict` otherwise); without it the file is overwritten or created. Returns the new ETag.
/// Missing parent folders are created.
pub fn put(server: &Server, url: &str, text: &str, etag: Option<&str>) -> Result<Option<String>, Error> {
    let mut headers: Vec<(&str, &str)> = vec![("Content-Type", "text/plain; charset=utf-8")];
    if let Some(e) = etag {
        headers.push(("If-Match", e));
    }
    let res = match call(server, "PUT", url, &headers, Some(text)) {
        Err(Error::Other(m)) if m.contains("409") => {
            make_parents(server, url)?;
            call(server, "PUT", url, &headers, Some(text))?
        }
        other => other?,
    };
    let new = res.header("ETag").map(str::to_string);
    if new.is_some() {
        return Ok(new);
    }
    // some servers send no ETag for PUT: ask for it
    Ok(head_etag(server, url))
}

fn head_etag(server: &Server, url: &str) -> Option<String> {
    call(server, "HEAD", url, &[], None).ok().and_then(|r| r.header("ETag").map(str::to_string))
}

/// ETag of the file now (cheap change check; None when the server does not give one).
pub fn etag(server: &Server, url: &str) -> Result<Option<String>, Error> {
    let res = call(server, "HEAD", url, &[], None)?;
    Ok(res.header("ETag").map(str::to_string))
}

/// Create the folders between the server base and the file.
fn make_parents(server: &Server, url: &str) -> Result<(), Error> {
    let rest = url.strip_prefix(&server.url).unwrap_or("");
    let mut dir = server.url.clone();
    let mut parts: Vec<&str> = rest.split('/').collect();
    parts.pop(); // the file name
    for p in parts.into_iter().filter(|p| !p.is_empty()) {
        dir.push_str(p);
        dir.push('/');
        mkcol(server, &dir)?;
    }
    Ok(())
}

/// Create a folder (already existing is fine).
pub fn mkcol(server: &Server, url: &str) -> Result<(), Error> {
    match call(server, "MKCOL", url, &[], None) {
        Ok(_) => Ok(()),
        Err(Error::Other(m)) if m.contains("405") => Ok(()), // exists
        Err(e) => Err(e),
    }
}

pub fn delete(server: &Server, url: &str) -> Result<(), Error> {
    match call(server, "DELETE", url, &[], None) {
        Ok(_) | Err(Error::NotFound) => Ok(()),
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub name: String,
    /// Absolute URL (folders end with `/`).
    pub url: String,
    pub is_dir: bool,
    pub size: u64,
}

/// The direct children of a folder (folders first, then files, both sorted).
pub fn list(server: &Server, url: &str) -> Result<Vec<Entry>, Error> {
    let url = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
    let body = r#"<?xml version="1.0"?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:getcontentlength/></d:prop></d:propfind>"#;
    let res = call(server, "PROPFIND", &url, &[("Depth", "1"), ("Content-Type", "application/xml")], Some(body))?;
    let mut xml = String::new();
    res.into_reader().take(8 << 20).read_to_string(&mut xml).map_err(|e| Error::Other(e.to_string()))?;
    Ok(parse_multistatus(&xml, &url))
}

fn unescape_xml(s: &str) -> String {
    s.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() && s.is_char_boundary(i + 1) && s.is_char_boundary(i + 3) {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn percent_encode_segment(s: &str) -> String {
    let mut out = String::new();
    for &c in s.as_bytes() {
        if c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'~') {
            out.push(c as char);
        } else {
            out.push_str(&format!("%{c:02X}"));
        }
    }
    out
}

/// Tag-name matching ignores the namespace prefix (`d:`, `D:`, none).
fn tag_text<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let lower = block.to_ascii_lowercase();
    let mut from = 0;
    while let Some(p) = lower[from..].find('<') {
        let start = from + p;
        let end = lower[start..].find('>')? + start;
        let tag = lower[start + 1..end].trim_end_matches('/');
        let local = tag.split_whitespace().next().unwrap_or("").rsplit(':').next().unwrap_or("");
        if local == name && !lower[start + 1..].starts_with('/') {
            let close = lower[end..].find("</")? + end;
            return Some(&block[end + 1..close]);
        }
        from = end;
    }
    None
}

fn parse_multistatus(xml: &str, request_url: &str) -> Vec<Entry> {
    let lower = xml.to_ascii_lowercase();
    // split into <response> blocks
    let mut blocks = Vec::new();
    let mut pos = 0;
    while let Some(s) = find_tag_open(&lower, "response", pos) {
        let e = find_tag_close(&lower, "response", s).unwrap_or(lower.len());
        blocks.push(&xml[s..e]);
        pos = e.max(s + 1);
    }
    let origin = request_url.find("://").and_then(|i| request_url[i + 3..].find('/').map(|j| i + 3 + j)).map(|i| &request_url[..i]).unwrap_or("");
    let req_path = percent_decode(&request_url[origin.len()..]);
    let mut dirs = Vec::new();
    let mut files = Vec::new();
    for b in blocks {
        let Some(href) = tag_text(b, "href") else { continue };
        let href = unescape_xml(href.trim());
        let path = if is_remote(&href) { href[href.find("://").map(|i| i + 3).unwrap_or(0)..].split_once('/').map(|x| format!("/{}", x.1)).unwrap_or_default() } else { href.clone() };
        let path_dec = percent_decode(&path);
        if path_dec.trim_end_matches('/') == req_path.trim_end_matches('/') {
            continue; // the folder itself
        }
        let is_dir = path_dec.ends_with('/') || tag_text(b, "resourcetype").is_some_and(|r| r.to_ascii_lowercase().contains("collection"));
        let name = path_dec.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string();
        if name.is_empty() {
            continue;
        }
        let size = tag_text(b, "getcontentlength").and_then(|s| s.trim().parse().ok()).unwrap_or(0);
        let mut url = format!("{origin}{}", if path.starts_with('/') { path } else { format!("/{path}") });
        if is_dir && !url.ends_with('/') {
            url.push('/');
        }
        let e = Entry { name, url, is_dir, size };
        if is_dir { dirs.push(e) } else { files.push(e) }
    }
    dirs.sort_by_key(|e| e.name.to_lowercase());
    files.sort_by_key(|e| e.name.to_lowercase());
    dirs.extend(files);
    dirs
}

fn find_tag_open(lower: &str, name: &str, from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(p) = lower[i..].find('<') {
        let s = i + p;
        let e = lower[s..].find('>')? + s;
        let tag = &lower[s + 1..e];
        if !tag.starts_with('/') && tag.split_whitespace().next().unwrap_or("").rsplit(':').next() == Some(name) {
            return Some(s);
        }
        i = e;
    }
    None
}

fn find_tag_close(lower: &str, name: &str, from: usize) -> Option<usize> {
    let mut i = from;
    while let Some(p) = lower[i..].find("</") {
        let s = i + p;
        let e = lower[s..].find('>')? + s;
        if lower[s + 2..e].trim().rsplit(':').next() == Some(name) {
            return Some(e + 1);
        }
        i = e;
    }
    None
}

/// Join a folder URL and a child name (the name is percent-encoded).
pub fn join(base: &str, name: &str) -> String {
    let mut b = base.to_string();
    if !b.ends_with('/') {
        b.push('/');
    }
    b + &percent_encode_segment(name)
}

/// Last path segment, decoded (`https://h/dav/todo.txt` -> `todo.txt`).
pub fn file_name(url: &str) -> String {
    percent_decode(url.trim_end_matches('/').rsplit('/').next().unwrap_or(""))
}

/// Parent folder URL with trailing slash.
pub fn parent(url: &str) -> String {
    let t = url.trim_end_matches('/');
    match t.rfind('/') {
        Some(i) if i > t.find("://").map_or(0, |j| j + 2) => format!("{}/", &t[..i]),
        _ => format!("{t}/"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_entry_roundtrip() {
        let s = Server::parse("jo.hn@example.com@https://cloud.example.com/remote.php/dav/files/jo").unwrap();
        assert_eq!(s.user, "jo.hn@example.com");
        assert_eq!(s.url, "https://cloud.example.com/remote.php/dav/files/jo/");
        assert_eq!(Server::parse(&s.entry()), Some(s));
        assert!(Server::parse("https://x/").is_none());
    }

    #[test]
    fn finds_server_by_prefix() {
        let e = vec!["a@https://h/dav/".to_string(), "b@https://h/dav/team/".to_string()];
        assert_eq!(server_for(&e, "https://h/dav/team/todo.txt").unwrap().user, "b");
        assert_eq!(server_for(&e, "https://h/dav/me/todo.txt").unwrap().user, "a");
        assert!(server_for(&e, "https://other/todo.txt").is_none());
    }

    #[test]
    fn parses_propfind() {
        let xml = r#"<?xml version="1.0"?><d:multistatus xmlns:d="DAV:">
<d:response><d:href>/dav/todo/</d:href><d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>
<d:response><d:href>/dav/todo/a%20list.txt</d:href><d:propstat><d:prop><d:resourcetype/><d:getcontentlength>12</d:getcontentlength></d:prop></d:propstat></d:response>
<D:response><D:href>/dav/todo/notes/</D:href><D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat></D:response>
</d:multistatus>"#;
        let v = parse_multistatus(xml, "https://h/dav/todo/");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "notes");
        assert!(v[0].is_dir);
        assert_eq!(v[1].name, "a list.txt");
        assert_eq!(v[1].url, "https://h/dav/todo/a%20list.txt");
        assert_eq!(v[1].size, 12);
    }

    #[test]
    fn url_helpers() {
        assert_eq!(join("https://h/d", "a b.txt"), "https://h/d/a%20b.txt");
        assert_eq!(file_name("https://h/d/a%20b.txt"), "a b.txt");
        assert_eq!(parent("https://h/d/x/"), "https://h/d/");
        assert_eq!(base64(b"user:pass"), "dXNlcjpwYXNz");
    }
}
