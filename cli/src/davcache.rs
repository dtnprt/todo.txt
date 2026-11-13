//! Offline cache for todo files on a WebDAV server (shared by the terminal app and the desktop app).
//!
//! Every remote file gets a folder `~/.todo.txt/cache/webdav/<hash of the URL>/`:
//! - `base.txt` + `base.etag`: the version last seen on the server,
//! - `local.txt`: only present while changes are *pending* (saved while the server was not reachable).
//!
//! * Reading while the server is unreachable returns the pending local version, else the cached base.
//! * Saving while it is unreachable writes `local.txt` (nothing is lost, the app keeps working).
//! * As soon as the server answers again (every read / ETag poll / save retries) the pending list is
//!   synced: unchanged server -> pushed as is; changed server -> a line-based three-way merge
//!   (base / local / server: lines removed locally are removed, lines added locally are added, everything
//!   else comes from the server) is pushed, so neither side's changes get lost.
//!
//! Only the main todo file is cached (not the notes).
//!
//! **Fast mode** (`davcache=on` in `todo.cfg`, default; only in the long-running clients TUI and desktop app,
//! see `set_interactive`): the cache is the primary copy. Reads come from the cache at once, saves only write
//! `local.txt` and return; a background worker per file pushes pending changes and polls the server
//! (`webdav::etag`, every `POLL`) and refreshes the cache when it changed. The "version" handed to the
//! clients is a local *token* (`token.txt`) that only changes when the content really changed from outside
//! (own pushes keep it), so their change check is a cheap file read and a change shows up within a second.
// included by both the CLI and the desktop app, each uses only part of it
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::webdav::{self, Error, Server};

static OFFLINE: AtomicBool = AtomicBool::new(false);
static INTERACTIVE: AtomicBool = AtomicBool::new(false);
static LOCK: Mutex<()> = Mutex::new(());
static WORKERS: Mutex<Vec<(String, Server)>> = Mutex::new(Vec::new());
static CFG: Mutex<Option<(Instant, bool)>> = Mutex::new(None);

/// How often the background worker asks the server whether the file changed.
const POLL: Duration = Duration::from_secs(3);

/// Called by long-running clients (TUI, desktop app): only they use the fast cache mode.
pub fn set_interactive() {
    INTERACTIVE.store(true, Ordering::Relaxed);
}

/// `davcache=` in `todo.cfg` (shared by all clients; default on).
pub fn config_enabled() -> bool {
    let mut g = CFG.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((t, v)) = *g {
        if t.elapsed() < Duration::from_secs(1) {
            return v;
        }
    }
    let v = fs::read_to_string(home().join(".todo.txt").join("todo.cfg"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.trim().strip_prefix("davcache=").map(|v| v.trim().to_lowercase())))
        .map_or(true, |v| !matches!(v.as_str(), "off" | "false" | "0" | "no"));
    *g = Some((Instant::now(), v));
    v
}

/// Fast cache mode in use?
pub fn active() -> bool {
    INTERACTIVE.load(Ordering::Relaxed) && config_enabled()
}

/// True when the last server access failed because the server could not be reached.
pub fn is_offline() -> bool {
    OFFLINE.load(Ordering::Relaxed)
}

fn set_offline(v: bool) {
    OFFLINE.store(v, Ordering::Relaxed);
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dir(url: &str) -> PathBuf {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    home().join(".todo.txt").join("cache").join("webdav").join(format!("{h:016x}"))
}

/// The text files of the cache hold the todo list itself: they are encrypted too when encryption is on.
fn is_content(name: &str) -> bool {
    matches!(name, "base.txt" | "local.txt")
}

fn read(d: &std::path::Path, name: &str) -> Option<String> {
    let t = fs::read_to_string(d.join(name)).ok()?;
    if is_content(name) { crate::crypt::open_session(&t) } else { Some(t) }
}

fn write(d: &std::path::Path, name: &str, text: &str) {
    let _ = fs::create_dir_all(d);
    if is_content(name) {
        // encryption on but locked: no plain-text copy is written (the older cache file stays)
        match crate::crypt::seal_session_if_enabled(text) {
            Some(t) => { let _ = fs::write(d.join(name), t); }
            None => {}
        }
    } else {
        let _ = fs::write(d.join(name), text);
    }
}

/// Is the cached copy of this remote list stored encrypted? None = nothing cached.
pub fn cached_encrypted(url: &str) -> Option<bool> {
    let t = fs::read_to_string(dir(url).join("base.txt")).ok()?;
    Some(crate::crypt::is_encrypted(&t))
}

fn store_base(d: &std::path::Path, url: &str, text: &str, etag: Option<&str>) {
    store_base_with(d, url, text, etag, true, true);
}

/// `bump`: the content changed from the clients' point of view (new token). `clear_local`: drop the pending version.
fn store_base_with(d: &std::path::Path, url: &str, text: &str, etag: Option<&str>, bump: bool, clear_local: bool) {
    let changed = read(d, "base.txt").as_deref() != Some(text);
    write(d, "base.txt", text);
    write(d, "base.etag", etag.unwrap_or(""));
    write(d, "url.txt", url);
    if clear_local {
        let _ = fs::remove_file(d.join("local.txt"));
    }
    if (bump && changed) || !d.join("token.txt").exists() {
        let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_nanos());
        write(d, "token.txt", &format!("t{n:x}"));
    }
}

fn token(d: &std::path::Path) -> Option<String> {
    read(d, "token.txt").filter(|t| !t.is_empty())
}

/// Start the background worker for this file (once per process).
fn register(server: &Server, url: &str) {
    {
        let mut w = WORKERS.lock().unwrap_or_else(|e| e.into_inner());
        if w.iter().any(|(u, _)| u == url) {
            return;
        }
        w.push((url.to_string(), server.clone()));
    }
    let (server, url) = (server.clone(), url.to_string());
    std::thread::spawn(move || {
        let d = dir(&url);
        let mut next_poll = Instant::now() + POLL;
        let mut next_try = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(200));
            if !active() {
                WORKERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|(u, _)| *u != url);
                return;
            }
            let now = Instant::now();
            if d.join("local.txt").exists() {
                if now >= next_try && !matches!(sync(&server, &url, None), Ok(Some(_))) {
                    next_try = Instant::now() + POLL;
                }
            } else if now >= next_poll {
                match webdav::etag(&server, &url) {
                    Ok(e) => {
                        set_offline(false);
                        if e.is_some() && e != base_etag(&d) {
                            if let Ok((text, etag)) = webdav::get(&server, &url) {
                                let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
                                store_base_with(&d, &url, &text, etag.as_deref(), true, false);
                            }
                        }
                    }
                    Err(Error::Offline(_)) => set_offline(true),
                    Err(_) => {}
                }
                next_poll = Instant::now() + POLL;
            }
        }
    });
}

/// Push every pending cached file now (at exit of a long-running client). Unreachable server: stays pending.
pub fn flush() {
    let list: Vec<(String, Server)> = WORKERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    for (url, server) in list {
        if pending(&url) {
            let _ = sync(&server, &url, None);
        }
    }
}

fn base_etag(d: &std::path::Path) -> Option<String> {
    read(d, "base.etag").filter(|e| !e.is_empty())
}

/// Changes saved offline and not yet synced?
pub fn pending(url: &str) -> bool {
    dir(url).join("local.txt").exists()
}

/// Pending offline changes as (lines removed, lines added) relative to the last server version.
pub fn pending_changes(url: &str) -> Option<(usize, usize)> {
    let d = dir(url);
    let local = read(&d, "local.txt")?;
    let base = read(&d, "base.txt").unwrap_or_default();
    let (r, a) = line_diff(&lines(&base), &lines(&local));
    Some((r.len(), a.len()))
}

fn lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

/// Lines removed from / added to a list between two versions (order-independent; blank lines ignored).
pub fn line_diff(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    fn count(v: &[String]) -> HashMap<&str, usize> {
        let mut m = HashMap::new();
        for l in v.iter().filter(|l| !l.trim().is_empty()) {
            *m.entry(l.as_str()).or_insert(0) += 1;
        }
        m
    }
    let (mut o, mut n) = (count(old), count(new));
    let mut removed = Vec::new();
    let mut added = Vec::new();
    for l in old.iter().filter(|l| !l.trim().is_empty()) {
        match n.get_mut(l.as_str()) {
            Some(k) if *k > 0 => *k -= 1,
            _ => removed.push(l.clone()),
        }
    }
    for l in new.iter().filter(|l| !l.trim().is_empty()) {
        match o.get_mut(l.as_str()) {
            Some(k) if *k > 0 => *k -= 1,
            _ => added.push(l.clone()),
        }
    }
    (removed, added)
}

/// Do the lines both versions have appear in a different order (the list is just sorted differently)?
pub fn reordered(old: &[String], new: &[String]) -> bool {
    fn common(a: &[String], b: &[String]) -> Vec<String> {
        let mut m: HashMap<&str, usize> = HashMap::new();
        for l in b.iter().filter(|l| !l.trim().is_empty()) {
            *m.entry(l.as_str()).or_insert(0) += 1;
        }
        let mut out = Vec::new();
        for l in a.iter().filter(|l| !l.trim().is_empty()) {
            if let Some(k) = m.get_mut(l.as_str()) {
                if *k > 0 {
                    *k -= 1;
                    out.push(l.clone());
                }
            }
        }
        out
    }
    common(old, new) != common(new, old)
}

/// Line-based three-way merge (see the module comment). `server` order is kept, added lines go last.
pub fn merge3(base: &str, local: &str, server: &str) -> String {
    let (removed, added) = line_diff(&lines(base), &lines(local));
    let mut rm: HashMap<String, usize> = HashMap::new();
    for l in removed {
        *rm.entry(l).or_insert(0) += 1;
    }
    let mut out: Vec<String> = Vec::new();
    let mut have: HashMap<String, usize> = HashMap::new();
    for l in server.lines() {
        if let Some(k) = rm.get_mut(l) {
            if *k > 0 {
                *k -= 1;
                continue;
            }
        }
        if !l.trim().is_empty() {
            *have.entry(l.to_string()).or_insert(0) += 1;
        }
        out.push(l.to_string());
    }
    for l in added {
        // the server already got the same line (added on both sides): do not duplicate it
        if let Some(k) = have.get_mut(&l) {
            if *k > 0 {
                *k -= 1;
                continue;
            }
        }
        out.push(l);
    }
    let nl = if server.contains("\r\n") || (server.is_empty() && local.contains("\r\n")) { "\r\n" } else { "\n" };
    let mut s = out.join(nl);
    if !out.is_empty() {
        s.push_str(nl);
    }
    s
}

pub struct Got {
    pub text: String,
    pub etag: Option<String>,
    /// The text comes from the cache because the server is not reachable.
    pub offline: bool,
}

/// Read a remote file; falls back to the cache while the server is unreachable.
/// Pending offline changes are synced first when the server is back.
pub fn get(server: &Server, url: &str) -> Result<Got, Error> {
    if active() {
        let d = dir(url);
        register(server, url);
        if let Some(text) = read(&d, "local.txt").or_else(|| read(&d, "base.txt")) {
            return Ok(Got { text, etag: token(&d), offline: is_offline() });
        }
        // nothing cached yet: fetch once, then the cache takes over
        let mut g = get_remote(server, url)?;
        g.etag = token(&d).or(g.etag);
        return Ok(g);
    }
    get_remote(server, url)
}

fn get_remote(server: &Server, url: &str) -> Result<Got, Error> {
    let d = dir(url);
    if d.join("local.txt").exists() {
        return match sync(server, url, None)? {
            Some(g) => Ok(g),
            None => Ok(Got { text: read(&d, "local.txt").unwrap_or_default(), etag: base_etag(&d), offline: true }),
        };
    }
    match webdav::get(server, url) {
        Ok((text, etag)) => {
            store_base(&d, url, &text, etag.as_deref());
            set_offline(false);
            Ok(Got { text, etag, offline: false })
        }
        Err(Error::Offline(m)) => {
            set_offline(true);
            match read(&d, "base.txt") {
                Some(text) => Ok(Got { text, etag: base_etag(&d), offline: true }),
                None => Err(Error::Offline(m)),
            }
        }
        Err(e) => Err(e),
    }
}

pub struct Put {
    pub etag: Option<String>,
    /// Not written to the server (unreachable): kept locally and synced later.
    pub offline: bool,
    /// The server had changed meanwhile: the merged text that is now on the server.
    pub merged: Option<String>,
}

/// Write a remote file (`etag` = version that was read). While the server is unreachable the text is kept
/// as pending local version; a conflict with pending offline work is merged instead of refused.
pub fn put(server: &Server, url: &str, text: &str, etag: Option<&str>) -> Result<Put, Error> {
    let d = dir(url);
    if active() {
        register(server, url);
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if read(&d, "base.txt").as_deref() == Some(text) {
            let _ = fs::remove_file(d.join("local.txt")); // back to the server version: nothing to push
        } else {
            write(&d, "local.txt", text);
            write(&d, "url.txt", url);
        }
        return Ok(Put { etag: token(&d).or_else(|| etag.map(str::to_string)), offline: false, merged: None });
    }
    if d.join("local.txt").exists() {
        return match sync(server, url, Some(text))? {
            Some(g) => Ok(Put { etag: g.etag, offline: false, merged: if g.text != text { Some(g.text) } else { None } }),
            None => Ok(Put { etag: base_etag(&d), offline: true, merged: None }),
        };
    }
    match webdav::put(server, url, text, etag) {
        Ok(e) => {
            store_base(&d, url, text, e.as_deref());
            set_offline(false);
            Ok(Put { etag: e, offline: false, merged: None })
        }
        Err(Error::Offline(_)) => {
            write(&d, "local.txt", text);
            write(&d, "url.txt", url);
            set_offline(true);
            Ok(Put { etag: etag.map(str::to_string), offline: true, merged: None })
        }
        Err(Error::Conflict) => {
            // 412: really changed on the server - or the ETag only *looks* different (weak `W/` tags, `-gzip`
            // suffixes, PUT and GET answering in another format). Compare the content: if the server still has
            // exactly what we last saw, it is no conflict and the save goes through.
            let same = match webdav::get(server, url) {
                Ok((stext, _)) => read(&d, "base.txt").map_or(false, |b| b == stext),
                Err(Error::NotFound) => false,
                Err(e) => return Err(e),
            };
            if !same {
                return Err(Error::Conflict);
            }
            let e = webdav::put(server, url, text, None)?;
            store_base(&d, url, text, e.as_deref());
            set_offline(false);
            Ok(Put { etag: e, offline: false, merged: None })
        }
        Err(e) => Err(e),
    }
}

/// Cheap change check. Also retries a pending sync. `Ok(None)` = unknown / not reachable (= no change).
pub fn etag(server: &Server, url: &str) -> Result<Option<String>, Error> {
    if active() {
        register(server, url);
        if let Some(t) = token(&dir(url)) {
            return Ok(Some(t));
        }
    }
    if pending(url) {
        return Ok(sync(server, url, None)?.and_then(|g| g.etag));
    }
    match webdav::etag(server, url) {
        Err(Error::Offline(_)) => {
            set_offline(true);
            Ok(None)
        }
        r => {
            if r.is_ok() {
                set_offline(false);
            }
            r
        }
    }
}

/// Push the pending local version (merging when the server changed). `Ok(None)` = still unreachable.
fn sync(server: &Server, url: &str, local_override: Option<&str>) -> Result<Option<Got>, Error> {
    let d = dir(url);
    if let Some(t) = local_override {
        write(&d, "local.txt", t);
    }
    let local = read(&d, "local.txt").unwrap_or_default();
    let (stext, setag) = match webdav::get(server, url) {
        Ok(x) => x,
        Err(Error::NotFound) => (String::new(), None),
        Err(Error::Offline(_)) => {
            set_offline(true);
            return Ok(None);
        }
        Err(e) => return Err(e),
    };
    let base = read(&d, "base.txt").unwrap_or_default();
    let merged = if stext == base || stext == local { local.clone() } else { merge3(&base, &local, &stext) };
    let etag = if merged != stext {
        // a 412 right after the GET is an ETag format problem, not a concurrent change: retry unconditionally
        let r = match webdav::put(server, url, &merged, setag.as_deref()) {
            Err(Error::Conflict) => webdav::put(server, url, &merged, None),
            other => other,
        };
        match r {
            Ok(e) => e,
            Err(Error::Offline(_)) => {
                set_offline(true);
                return Ok(None);
            }
            Err(e) => return Err(e),
        }
    } else {
        setag
    };
    {
        let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // a newer save may have arrived while pushing: then it stays pending
        let still = read(&d, "local.txt").as_deref() == Some(local.as_str());
        store_base_with(&d, url, &merged, etag.as_deref(), merged != local, still);
    }
    set_offline(false);
    Ok(Some(Got { text: merged, etag, offline: false }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_keeps_both_sides() {
        let base = "a\nb\nc\n";
        let local = "a\nc\nlocal new\n"; // removed b, added "local new"
        let server = "a\nb\nc\nserver new\n"; // added "server new"
        assert_eq!(merge3(base, local, server), "a\nc\nserver new\nlocal new\n");
    }

    #[test]
    fn merge_same_line_added_on_both_sides_once() {
        assert_eq!(merge3("a\n", "a\nx\n", "a\nx\n"), "a\nx\n");
    }

    #[test]
    fn diff_ignores_order() {
        let (r, a) = line_diff(&lines("a\nb\n"), &lines("b\nc\na\n"));
        assert!(r.is_empty());
        assert_eq!(a, vec!["c".to_string()]);
    }
}
