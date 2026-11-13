//! Reading and writing the todo.txt file. Lines are kept exactly as they are in the file
//! (blank lines included) so that line numbers match what an editor shows.

use std::cell::RefCell;
use std::fs;
use std::io;
use std::path::Path;

use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::webdav;

/// May a read or write ask for the password on the terminal? True for the one-shot CLI and for the start of the TUI
/// (before the screen is taken over), false while the TUI runs.
static CAN_PROMPT: AtomicBool = AtomicBool::new(false);

pub fn allow_prompt(on: bool) {
    CAN_PROMPT.store(on, Ordering::Relaxed);
}

/// Ask for the password (no echo). None when there is no terminal to ask on.
pub fn prompt_password(message: &str) -> Option<String> {
    if !io::stdin().is_terminal() {
        return None;
    }
    rpassword::prompt_password(message).ok().filter(|p| !p.is_empty())
}

/// Run `f`; if it fails because a password is needed, ask for it (up to three times) and try again.
/// A new file is being created: ask whether it is encrypted (default = what the folder policy / the global setting says) and
/// remember the answer in the `.todotxt.cfg` of its folder. No terminal / no prompting: nothing is asked.
fn ask_encrypt_new(default: bool) -> Option<bool> {
    if !CAN_PROMPT.load(Ordering::Relaxed) || !io::stdin().is_terminal() {
        return None;
    }
    eprint!("Encrypt this new file? (experimental; other todo.txt apps cannot read encrypted files) [{}] ", if default { "Y/n" } else { "y/N" });
    let mut line = String::new();
    io::stdin().read_line(&mut line).ok()?;
    Some(match line.trim().to_lowercase().as_str() {
        "y" | "yes" | "j" | "ja" => true,
        "n" | "no" | "nein" => false,
        _ => default,
    })
}

pub fn with_unlock<T>(f: impl Fn() -> io::Result<T>) -> io::Result<T> {
    let mut attempts = 0;
    loop {
        match f() {
            Err(e) if crate::crypt::is_lock_error(&e) && CAN_PROMPT.load(Ordering::Relaxed) && attempts < 3 => {
                attempts += 1;
                let msg = if attempts == 1 { "Password for the encrypted todo files: " } else { "Wrong password, try again: " };
                match prompt_password(msg) {
                    Some(p) => crate::crypt::set_session(Some(p)),
                    None => return Err(e),
                }
            }
            other => return other,
        }
    }
}

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
    with_unlock(|| read_text_once(path))
}

fn read_text_once(path: &Path) -> io::Result<(String, Option<String>)> {
    if webdav::is_remote(&path.to_string_lossy()) {
        let server = server_of(path)?;
        // through the offline cache: unreachable server -> cached / pending local version
        crate::davcache::get(&server, &path.to_string_lossy()).map(|g| (g.text, g.etag)).map_err(other)
    } else {
        let t = fs::read_to_string(path)?;
        crate::crypt::open(&t).map(|t| (t, None)).map_err(io::Error::from)
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
        with_unlock(|| self.save_once(path))
    }

    fn save_once(&self, path: &Path) -> io::Result<()> {
        if webdav::is_remote(&path.to_string_lossy()) {
            let server = server_of(path)?;
            if self.etag.borrow().is_none() {
                // no ETag = the file does not exist on the server yet
                let url = path.to_string_lossy().into_owned();
                let default = webdav::remote_want(&server, &url).unwrap_or_else(crate::crypt::enabled);
                if let Some(enc) = ask_encrypt_new(default) {
                    let _ = webdav::set_remote_file_policy(&server, &url, enc);
                }
            }
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
    if !path.exists() {
        let default = crate::crypt::policy_for_path(path).unwrap_or_else(crate::crypt::enabled);
        if let Some(enc) = ask_encrypt_new(default) {
            if let Some(dir) = path.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = crate::crypt::set_file_policy(path, enc);
        }
    }
    let out = crate::crypt::seal_for_path(path, &render(lines, crlf)).map_err(io::Error::from)?;
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    fs::write(path, out)
}
