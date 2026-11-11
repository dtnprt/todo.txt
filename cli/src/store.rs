//! Reading and writing the todo.txt file. Lines are kept exactly as they are in the file
//! (blank lines included) so that line numbers match what an editor shows.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::Path;

use crate::webdav;

pub struct Store {
    pub lines: Vec<String>,
    crlf: bool,
    /// WebDAV: ETag of the version that was read (a save only succeeds if it is still current).
    etag: RefCell<Option<String>>,
}

/// The server a remote path (`https://...`) belongs to.
pub fn server_of(path: &Path) -> io::Result<webdav::Server> {
    let url = path.to_string_lossy();
    webdav::server_for(&crate::config::Config::load().webdav, &url).ok_or_else(|| {
        io::Error::new(io::ErrorKind::Other, format!("no WebDAV server configured for {url} (todo --webdav-login USER@URL)"))
    })
}

fn other(e: webdav::Error) -> io::Error {
    match e {
        webdav::Error::NotFound => io::Error::new(io::ErrorKind::NotFound, e.to_string()),
        e => io::Error::new(io::ErrorKind::Other, e.to_string()),
    }
}

/// Text of a local or remote file (plus the ETag for remote ones). A missing file is `ErrorKind::NotFound`.
pub fn read_text(path: &Path) -> io::Result<(String, Option<String>)> {
    if webdav::is_remote(&path.to_string_lossy()) {
        let server = server_of(path)?;
        // through the offline cache: unreachable server -> cached / pending local version
        crate::davcache::get(&server, &path.to_string_lossy()).map(|g| (g.text, g.etag)).map_err(other)
    } else {
        fs::read_to_string(path).map(|t| (t, None))
    }
}

impl Store {
    /// A missing file is an empty list (it is created on the first save).
    pub fn load(path: &Path) -> io::Result<Store> {
        match read_text(path) {
            Ok((text, etag)) => {
                let crlf = text.contains("\r\n");
                let lines = text.lines().map(str::to_string).collect();
                Ok(Store { lines, crlf, etag: RefCell::new(etag) })
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Store { lines: Vec::new(), crlf: cfg!(windows) && !webdav::is_remote(&path.to_string_lossy()), etag: RefCell::new(None) }),
            Err(e) => Err(e),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        if webdav::is_remote(&path.to_string_lossy()) {
            let server = server_of(path)?;
            let r = crate::davcache::put(&server, &path.to_string_lossy(), &self.text(), self.etag.borrow().as_deref()).map_err(other)?;
            *self.etag.borrow_mut() = r.etag;
            return Ok(());
        }
        save_lines(path, &self.lines, self.crlf)
    }

    /// The file content as it would be written.
    pub fn text(&self) -> String {
        render(&self.lines, self.crlf)
    }

    pub fn crlf(&self) -> bool {
        self.crlf
    }

    /// 1-based task number -> index, only for non-blank lines.
    pub fn index_of(&self, number: usize) -> Option<usize> {
        let i = number.checked_sub(1)?;
        self.lines.get(i).filter(|l| !l.trim().is_empty()).map(|_| i)
    }
}

pub fn render(lines: &[String], crlf: bool) -> String {
    let nl = if crlf { "\r\n" } else { "\n" };
    let mut out = lines.join(nl);
    if !lines.is_empty() {
        out.push_str(nl);
    }
    out
}

pub fn save_lines(path: &Path, lines: &[String], crlf: bool) -> io::Result<()> {
    let out = render(lines, crlf);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    fs::write(path, out)
}
