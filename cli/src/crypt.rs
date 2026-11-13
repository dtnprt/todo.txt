//! Optional encryption of todo files, notes and the WebDAV offline cache (AES-256-GCM).
//! Shared by the terminal app and the desktop app (included there via `#[path]`); `web/crypt.js` implements the same format.
//!
//! File format (text, so git and WebDAV treat it like any other text file):
//! ```text
//! todotxt-enc1 pbkdf2-sha256 600000 <salt, base64> <nonce, base64>
//! <ciphertext + 16 byte GCM tag, base64, wrapped at 76 columns>
//! ```
//! The key is PBKDF2-HMAC-SHA256(password, salt, iterations); the header line is the authenticated data (AAD), so the
//! parameters cannot be changed without detection. A file is recognised by its first word, so encrypted and plain files
//! can be mixed; whether *new writes* are encrypted is the `encrypt=` setting of `todo.cfg`.
//!
//! The password lives in memory for the session, can be kept in the OS keyring (service `todo.txt`, account
//! `encryption`) and optionally be released only after a Windows Hello check (`encrypt_hello=true`).
//! `TODO_PASSWORD` in the environment is a fallback for scripts.

#![allow(dead_code)] // each client uses a different part

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Mutex;

pub const MAGIC: &str = "todotxt-enc1";
const KDF: &str = "pbkdf2-sha256";
/// OWASP 2023 recommendation for PBKDF2-HMAC-SHA256.
pub const ITERATIONS: u32 = 600_000;
const MAX_ITERATIONS: u32 = 20_000_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The file is encrypted and no password is known (yet).
    NeedPassword,
    /// The password does not fit (or the file was modified).
    WrongPassword,
    /// The user cancelled the biometric / Windows Hello check.
    Cancelled,
    Malformed(String),
    Other(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NeedPassword => write!(f, "password required: the file is encrypted"),
            Error::WrongPassword => write!(f, "wrong password (or the encrypted file was modified)"),
            Error::Cancelled => write!(f, "password required: unlocking was cancelled"),
            Error::Malformed(m) => write!(f, "damaged encrypted file: {m}"),
            Error::Other(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<Error> for std::io::Error {
    fn from(e: Error) -> Self {
        std::io::Error::new(std::io::ErrorKind::PermissionDenied, e.to_string())
    }
}

/// Is this message (of any error type) a "need the password" error that a UI can answer with a prompt?
pub fn is_lock_message(m: &str) -> bool {
    m.contains("password required") || m.contains("wrong password")
}

pub fn is_lock_error(e: &std::io::Error) -> bool {
    is_lock_message(&e.to_string())
}

/// Does the text start with the encrypted-file header?
pub fn is_encrypted(text: &str) -> bool {
    text.strip_prefix(MAGIC).is_some_and(|r| r.starts_with(' '))
}

// ---------- the cipher (pure) ----------

fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut b = [0u8; N];
    getrandom::getrandom(&mut b).map_err(|e| Error::Other(format!("random numbers: {e}")))?;
    Ok(b)
}

type CachedKey = (String, Vec<u8>, u32, [u8; 32]);
static KEYS: Mutex<Vec<CachedKey>> = Mutex::new(Vec::new());
static SEAL_SALT: Mutex<Option<[u8; 16]>> = Mutex::new(None);

/// PBKDF2 is slow on purpose: keep the derived keys of this process (at most a few).
fn key_for(password: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut keys = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((_, _, _, k)) = keys.iter().find(|(p, s, i, _)| p == password && s == salt && *i == iterations) {
        return *k;
    }
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha256>(password.as_bytes(), salt, iterations, &mut key);
    if keys.len() >= 8 {
        keys.remove(0);
    }
    keys.push((password.to_string(), salt.to_vec(), iterations, key));
    key
}

/// Encrypt `plain` with `password` (fresh nonce each time; the salt, and with it the key, is reused for the process).
pub fn seal(plain: &str, password: &str) -> Result<String, Error> {
    seal_bytes(plain.as_bytes(), password)
}

/// Encrypt arbitrary bytes (pasted images); the result is the same text format.
pub fn seal_bytes(plain: &[u8], password: &str) -> Result<String, Error> {
    let salt = {
        let mut s = SEAL_SALT.lock().unwrap_or_else(|e| e.into_inner());
        match *s {
            Some(v) => v,
            None => {
                let v = random::<16>()?;
                *s = Some(v);
                v
            }
        }
    };
    seal_with_salt(plain, password, &salt, ITERATIONS)
}

fn seal_with_salt(plain: &[u8], password: &str, salt: &[u8; 16], iterations: u32) -> Result<String, Error> {
    let nonce = random::<12>()?;
    let key = key_for(password, salt, iterations);
    let header = format!("{MAGIC} {KDF} {iterations} {} {}", B64.encode(salt), B64.encode(nonce));
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| Error::Other(e.to_string()))?;
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plain, aad: header.as_bytes() })
        .map_err(|_| Error::Other("encryption failed".into()))?;
    let body = B64.encode(ct);
    let mut out = String::with_capacity(header.len() + body.len() + body.len() / 76 + 4);
    out.push_str(&header);
    out.push('\n');
    for chunk in body.as_bytes().chunks(76) {
        out.push_str(std::str::from_utf8(chunk).unwrap_or(""));
        out.push('\n');
    }
    Ok(out)
}

/// Decrypt a file text with `password`.
pub fn open_with(text: &str, password: &str) -> Result<String, Error> {
    String::from_utf8(open_bytes_with(text, password)?).map_err(|_| Error::Malformed("not UTF-8 text".into()))
}

/// Decrypt to raw bytes.
pub fn open_bytes_with(text: &str, password: &str) -> Result<Vec<u8>, Error> {
    let (header, body) = text.split_once('\n').unwrap_or((text, ""));
    let header = header.trim_end_matches('\r');
    let parts: Vec<&str> = header.split_whitespace().collect();
    if parts.len() != 5 || parts[0] != MAGIC {
        return Err(Error::Malformed("bad header".into()));
    }
    if parts[1] != KDF {
        return Err(Error::Malformed(format!("unknown key derivation '{}'", parts[1])));
    }
    let iterations: u32 = parts[2].parse().map_err(|_| Error::Malformed("bad iteration count".into()))?;
    if iterations == 0 || iterations > MAX_ITERATIONS {
        return Err(Error::Malformed("unreasonable iteration count".into()));
    }
    let salt = B64.decode(parts[3]).map_err(|_| Error::Malformed("bad salt".into()))?;
    let nonce = B64.decode(parts[4]).map_err(|_| Error::Malformed("bad nonce".into()))?;
    if nonce.len() != 12 || salt.len() < 8 {
        return Err(Error::Malformed("bad salt or nonce length".into()));
    }
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let ct = B64.decode(compact).map_err(|_| Error::Malformed("bad body".into()))?;
    let key = key_for(password, &salt, iterations);
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| Error::Other(e.to_string()))?;
    cipher
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ct, aad: header.as_bytes() })
        .map_err(|_| Error::WrongPassword)
}

// ---------- session password, keyring, settings ----------

static SESSION: Mutex<Option<String>> = Mutex::new(None);
/// 0 = follow `encrypt=` in todo.cfg, 1 = forced off, 2 = forced on (set by the desktop app so a change is seen at once).
static HELLO_DENIED: AtomicBool = AtomicBool::new(false);
static OVERRIDE: AtomicU8 = AtomicU8::new(0);

pub fn set_session(password: Option<String>) {
    *SESSION.lock().unwrap_or_else(|e| e.into_inner()) = password;
}

pub fn session_password() -> Option<String> {
    SESSION.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub fn has_session() -> bool {
    session_password().is_some()
}

pub fn home() -> PathBuf {
    #[cfg(windows)]
    let order = ["USERPROFILE", "HOME"];
    #[cfg(not(windows))]
    let order = ["HOME", "USERPROFILE"];
    std::env::var_os(order[0])
        .or_else(|| std::env::var_os(order[1]))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// A `key=value` setting of `~/.todo.txt/todo.cfg`.
pub fn cfg_get(key: &str) -> Option<String> {
    let text = std::fs::read_to_string(home().join(".todo.txt").join("todo.cfg")).ok()?;
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())
}

fn cfg_flag(key: &str) -> bool {
    matches!(cfg_get(key).as_deref(), Some("true" | "on" | "1" | "yes"))
}

/// Are new writes encrypted?
pub fn enabled() -> bool {
    match OVERRIDE.load(Ordering::Relaxed) {
        1 => false,
        2 => true,
        _ => cfg_flag("encrypt"),
    }
}

/// Make `enabled()` answer `on` right now, independent of the config file (None = follow the file again).
pub fn set_enabled_override(on: Option<bool>) {
    OVERRIDE.store(match on { None => 0, Some(false) => 1, Some(true) => 2 }, Ordering::Relaxed);
}

/// Ask for Windows Hello (or another biometric / device check) before the stored password is used?
pub fn hello_enabled() -> bool {
    cfg_flag("encrypt_hello")
}

#[cfg(not(target_os = "android"))]
fn entry() -> Result<keyring::Entry, String> {
    keyring::Entry::new("todo.txt", "encryption").map_err(|e| e.to_string())
}

/// Keep the password in the OS keyring (Windows Credential Manager, macOS Keychain, Secret Service).
pub fn store_password(password: &str) -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        entry()?.set_password(password).map_err(|e| e.to_string())
    }
    #[cfg(target_os = "android")]
    {
        let _ = password;
        Err("no system keyring on Android".into())
    }
}

pub fn stored_password() -> Option<String> {
    #[cfg(not(target_os = "android"))]
    {
        entry().ok()?.get_password().ok()
    }
    #[cfg(target_os = "android")]
    {
        None
    }
}

pub fn has_stored_password() -> bool {
    #[cfg(not(target_os = "android"))]
    {
        entry().ok().is_some_and(|e| e.get_password().is_ok())
    }
    #[cfg(target_os = "android")]
    {
        false
    }
}

pub fn forget_stored_password() -> Result<(), String> {
    #[cfg(not(target_os = "android"))]
    {
        match entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }
    #[cfg(target_os = "android")]
    {
        Ok(())
    }
}

/// Is a biometric / Windows Hello check available on this machine?
pub fn hello_available() -> bool {
    #[cfg(windows)]
    {
        use windows::Security::Credentials::UI::{UserConsentVerifier, UserConsentVerifierAvailability};
        UserConsentVerifier::CheckAvailabilityAsync()
            .and_then(|op| op.join())
            .is_ok_and(|a| a == UserConsentVerifierAvailability::Available)
    }
    #[cfg(not(windows))]
    {
        false
    }
}

/// Ask the user to verify with Windows Hello (fingerprint, face, PIN). `Ok(false)` = refused / cancelled.
pub fn verify_user(reason: &str) -> Result<bool, String> {
    #[cfg(windows)]
    {
        use windows::core::HSTRING;
        use windows::Security::Credentials::UI::{UserConsentVerificationResult, UserConsentVerifier};
        let r = UserConsentVerifier::RequestVerificationAsync(&HSTRING::from(reason))
            .and_then(|op| op.join())
            .map_err(|e| e.to_string())?;
        Ok(r == UserConsentVerificationResult::Verified)
    }
    #[cfg(not(windows))]
    {
        let _ = reason;
        Err("biometric unlock is only available on Windows (Windows Hello)".into())
    }
}

/// The password to use: session -> keyring (after Windows Hello, if switched on) -> `TODO_PASSWORD`.
/// A password found this way is kept for the session.
pub fn resolve_password() -> Result<String, Error> {
    if let Some(p) = session_password() {
        return Ok(p);
    }
    if let Some(p) = stored_password() {
        if hello_enabled() && hello_available() {
            // after a refusal nobody is asked again until `unlock()` is called on purpose (no prompt storms from polling)
            if HELLO_DENIED.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            match verify_user("Unlock your encrypted todo.txt files") {
                Ok(true) => {}
                Ok(false) => {
                    HELLO_DENIED.store(true, Ordering::Relaxed);
                    return Err(Error::Cancelled);
                }
                Err(e) => return Err(Error::Other(e)),
            }
        }
        set_session(Some(p.clone()));
        return Ok(p);
    }
    if let Ok(p) = std::env::var("TODO_PASSWORD") {
        if !p.is_empty() {
            set_session(Some(p.clone()));
            return Ok(p);
        }
    }
    Err(Error::NeedPassword)
}

/// Make the stored/environment password the session password (asks Windows Hello if switched on).
pub fn unlock() -> Result<(), Error> {
    HELLO_DENIED.store(false, Ordering::Relaxed);
    resolve_password().map(|_| ())
}

/// Plain text of a file's content: decrypts when needed (password from `resolve_password`). A wrong session password
/// is forgotten so that the next attempt asks again.
pub fn open(text: &str) -> Result<String, Error> {
    if !is_encrypted(text) {
        return Ok(text.to_string());
    }
    let pw = resolve_password()?;
    match open_with(text, &pw) {
        Err(Error::WrongPassword) => {
            set_session(None);
            Err(Error::WrongPassword)
        }
        r => r,
    }
}

/// Like `open`, but never asks anyone: only the session password is used; an encrypted text without it is returned as it is.
pub fn open_quiet(text: &str) -> String {
    if !is_encrypted(text) {
        return text.to_string();
    }
    session_password().and_then(|p| open_with(text, &p).ok()).unwrap_or_else(|| text.to_string())
}

/// Cache files: decrypt with the session password only (never prompts); None = encrypted and locked.
pub fn open_session(text: &str) -> Option<String> {
    if !is_encrypted(text) {
        return Some(text.to_string());
    }
    session_password().and_then(|p| open_with(text, &p).ok())
}

/// Cache files: encrypt with the session password when encryption is on; None = on, but locked (do not write plain text).
pub fn seal_session_if_enabled(text: &str) -> Option<String> {
    if !enabled() {
        return Some(text.to_string());
    }
    session_password().and_then(|p| seal(text, &p).ok())
}

/// The text to write: encrypted when `enabled()`, else as it is.
pub fn seal_if_enabled(text: &str) -> Result<String, Error> {
    if !enabled() {
        return Ok(text.to_string());
    }
    let pw = resolve_password()?;
    seal(text, &pw)
}

/// Like `seal_if_enabled` for a local file: a file that is encrypted on disk stays encrypted when it is overwritten, even if
/// `encrypt=` is off (turning encryption off is done explicitly, which decrypts the files: `todo --encrypt off`).
pub fn seal_for_path(path: &Path, text: &str) -> Result<String, Error> {
    let keep = !enabled()
        && std::fs::File::open(path).ok().is_some_and(|mut f| {
            use std::io::Read;
            let mut head = [0u8; 13];
            f.read_exact(&mut head).is_ok() && head.starts_with(MAGIC.as_bytes()) && head[12] == b' '
        });
    if enabled() || keep {
        let pw = resolve_password()?;
        seal(text, &pw)
    } else {
        Ok(text.to_string())
    }
}

// ---------- converting files ----------

fn is_encrypted_bytes(b: &[u8]) -> bool {
    b.len() > MAGIC.len() && b.starts_with(MAGIC.as_bytes()) && b[MAGIC.len()] == b' '
}

/// Rewrite one file (a list, a note or an image): decrypt with `old` if it is encrypted, then encrypt with `new`
/// (None = leave it plain). Returns whether the file changed.
pub fn convert_file(path: &Path, old: Option<&str>, new: Option<&str>) -> Result<bool, Error> {
    let raw = std::fs::read(path).map_err(|e| Error::Other(format!("{}: {e}", path.display())))?;
    let encrypted = is_encrypted_bytes(&raw);
    let plain: Vec<u8> = if encrypted {
        let text = String::from_utf8(raw.clone()).map_err(|_| Error::Malformed("not text".into()))?;
        open_bytes_with(&text, old.ok_or(Error::NeedPassword)?)?
    } else {
        raw
    };
    let out: Vec<u8> = match new {
        Some(p) => {
            if encrypted && old == Some(p) {
                return Ok(false);
            }
            seal_bytes(&plain, p)?.into_bytes()
        }
        None => {
            if !encrypted {
                return Ok(false);
            }
            plain
        }
    };
    std::fs::write(path, out).map_err(|e| Error::Other(format!("{}: {e}", path.display())))?;
    Ok(true)
}

/// Read a file that may be encrypted (images): the plain bytes.
pub fn read_file_auto(path: &Path) -> Result<Vec<u8>, Error> {
    let raw = std::fs::read(path).map_err(|e| Error::Other(format!("{}: {e}", path.display())))?;
    if !is_encrypted_bytes(&raw) {
        return Ok(raw);
    }
    let text = String::from_utf8(raw).map_err(|_| Error::Malformed("not text".into()))?;
    let pw = resolve_password()?;
    match open_bytes_with(&text, &pw) {
        Err(Error::WrongPassword) => {
            set_session(None);
            Err(Error::WrongPassword)
        }
        r => r,
    }
}

/// Bytes to write to `path` (images): encrypted when encryption is on or the file is already encrypted.
pub fn seal_bytes_for_path(path: &Path, data: &[u8]) -> Result<Vec<u8>, Error> {
    let keep = !enabled() && std::fs::read(path).ok().is_some_and(|b| is_encrypted_bytes(&b[..b.len().min(16)]));
    if enabled() || keep {
        let pw = resolve_password()?;
        Ok(seal_bytes(data, &pw)?.into_bytes())
    } else {
        Ok(data.to_vec())
    }
}

/// Convert the visible `*.txt` files directly in `dir` (see `convert_file`). Returns the number of changed files.
pub fn convert_flat(dir: &Path, old: Option<&str>, new: Option<&str>) -> Result<usize, Error> {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(0) };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let is_file = e.metadata().map(|m| m.is_file()).unwrap_or(false);
        if is_file && !name.starts_with('.') && name.to_lowercase().ends_with(".txt") && convert_file(&e.path(), old, new)? {
            n += 1;
        }
    }
    Ok(n)
}

/// Convert the pasted images (`png jpg jpeg gif webp bmp`) directly in `dir`.
pub fn convert_images(dir: &Path, old: Option<&str>, new: Option<&str>) -> Result<usize, Error> {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(dir) else { return Ok(0) };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        let ext_ok = name.rsplit_once('.').is_some_and(|(_, x)| matches!(x, "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"));
        if e.metadata().map(|m| m.is_file()).unwrap_or(false) && !name.starts_with('.') && ext_ok && convert_file(&e.path(), old, new)? {
            n += 1;
        }
    }
    Ok(n)
}

/// Convert everything a todo folder holds: the lists, `notes/` and `images/`.
pub fn convert_dir(dir: &Path, old: Option<&str>, new: Option<&str>) -> Result<usize, Error> {
    Ok(convert_flat(dir, old, new)? + convert_flat(&dir.join("notes"), old, new)? + convert_images(&dir.join("images"), old, new)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_header() {
        let s = seal("(A) buy milk +home\nx 2026-01-01 done\n", "correct horse").unwrap();
        assert!(is_encrypted(&s));
        assert!(s.starts_with("todotxt-enc1 pbkdf2-sha256 600000 "));
        assert!(!s.contains("milk"));
        assert_eq!(open_with(&s, "correct horse").unwrap(), "(A) buy milk +home\nx 2026-01-01 done\n");
        assert_eq!(open_with(&s, "wrong").unwrap_err(), Error::WrongPassword);
    }

    #[test]
    fn nonce_is_fresh_and_tampering_is_detected() {
        let a = seal("same", "pw").unwrap();
        let b = seal("same", "pw").unwrap();
        assert_ne!(a, b);
        // change the iteration count in the header: authenticated data -> refused
        let t = a.replacen(" 600000 ", " 600001 ", 1);
        assert!(open_with(&t, "pw").is_err());
        // flip a character of the body
        let mut lines: Vec<String> = a.lines().map(String::from).collect();
        let l = &mut lines[1];
        let c = if l.starts_with('A') { "B" } else { "A" };
        l.replace_range(0..1, c);
        assert!(open_with(&lines.join("\n"), "pw").is_err());
    }

    #[test]
    fn empty_and_unicode_text() {
        let s = seal("", "pw").unwrap();
        assert_eq!(open_with(&s, "pw").unwrap(), "");
        let u = "Käse kaufen ☕ +einkauf\r\n";
        assert_eq!(open_with(&seal(u, "pässwörd").unwrap(), "pässwörd").unwrap(), u);
    }

    #[test]
    fn detection() {
        assert!(!is_encrypted("(A) todotxt-enc1 something"));
        assert!(!is_encrypted("todotxt-enc10 x"));
        assert!(is_encrypted("todotxt-enc1 pbkdf2-sha256 1 AA AA\nxx"));
    }

    #[test]
    fn known_vector_for_the_javascript_implementation() {
        // fixed salt/nonce so web/crypt.js can be checked against the very same text
        let s = seal_with_salt(b"hello todo.txt\n", "pw", &[7u8; 16], 1000).unwrap();
        assert_eq!(open_with(&s, "pw").unwrap(), "hello todo.txt\n");
        assert!(s.starts_with(&format!("{MAGIC} {KDF} 1000 {} ", B64.encode([7u8; 16]))));
    }

    #[test]
    fn convert_files() {
        let dir = std::env::temp_dir().join(format!("todo-crypt-test-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("notes")).unwrap();
        std::fs::write(dir.join("a.txt"), "(A) one\n").unwrap();
        std::fs::write(dir.join("notes").join("n.txt"), "long text\n").unwrap();
        std::fs::write(dir.join(".hidden.txt"), "x\n").unwrap();
        std::fs::create_dir_all(dir.join("images")).unwrap();
        let png = [0x89u8, b'P', b'N', b'G', 0, 1, 2, 255, 254];
        std::fs::write(dir.join("images").join("img-1.png"), png).unwrap();
        assert_eq!(convert_dir(&dir, None, Some("pw")).unwrap(), 3);
        assert!(is_encrypted_bytes(&std::fs::read(dir.join("images").join("img-1.png")).unwrap()));
        set_session(Some("pw".into()));
        assert_eq!(read_file_auto(&dir.join("images").join("img-1.png")).unwrap(), png);
        set_session(None);
        assert!(is_encrypted(&std::fs::read_to_string(dir.join("a.txt")).unwrap()));
        assert_eq!(std::fs::read_to_string(dir.join(".hidden.txt")).unwrap(), "x\n");
        assert_eq!(convert_dir(&dir, Some("pw"), Some("pw")).unwrap(), 0);
        assert_eq!(convert_dir(&dir, Some("pw"), Some("new")).unwrap(), 3);
        assert_eq!(convert_dir(&dir, Some("wrong"), None).unwrap_err(), Error::WrongPassword);
        assert_eq!(convert_dir(&dir, Some("new"), None).unwrap(), 3);
        assert_eq!(std::fs::read_to_string(dir.join("a.txt")).unwrap(), "(A) one\n");
        assert_eq!(std::fs::read(dir.join("images").join("img-1.png")).unwrap(), png);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
