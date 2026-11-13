//! Folder mode (a whole folder of todo.txt files) and `note:NAME` files.
//!
//! Note live in `<folder>/notes/NAME.txt` in folder mode, otherwise in `~/.todo.txt/notes/NAME.txt`.
//! Keep in sync with the Rust commands in `src-tauri/src/lib.rs` and with `web/app.js`.

use crate::config::home_dir;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

/// Dot files and (on Windows) files with the "hidden" attribute.
pub fn is_hidden(name: &str, meta: &fs::Metadata) -> bool {
    if name.starts_with('.') {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if meta.file_attributes() & 0x2 != 0 {
            return true;
        }
    }
    let _ = meta;
    false
}

pub fn is_txt(name: &str) -> bool {
    name.to_lowercase().ends_with(".txt")
}

/// Names (lower case) referenced as `note:NAME` in the given text.
fn note_refs(text: &str, out: &mut HashSet<String>) {
    for word in text.split_whitespace() {
        if let Some(n) = word.strip_prefix("note:") {
            out.insert(n.to_lowercase());
        }
    }
}

/// A folder on a WebDAV server: an `http(s)://` URL ending with `/`.
pub fn is_remote_dir(p: &Path) -> bool {
    let s = p.to_string_lossy();
    crate::webdav::is_remote(&s) && s.ends_with('/')
}

/// A local directory or a remote folder URL.
pub fn is_folder(p: &Path) -> bool {
    is_remote_dir(p) || p.is_dir()
}

/// `dir/name` (URL-aware).
pub fn join(dir: &Path, name: &str) -> PathBuf {
    if is_remote_dir(dir) {
        PathBuf::from(crate::webdav::join(&dir.to_string_lossy(), name))
    } else {
        dir.join(name)
    }
}

/// Directory of a file or folder (URL-aware; a remote folder keeps its trailing `/`).
pub fn parent_of(p: &Path) -> Option<PathBuf> {
    if crate::webdav::is_remote(&p.to_string_lossy()) {
        let s = p.to_string_lossy();
        let up = crate::webdav::parent(&s);
        return (up.trim_end_matches('/') != s.trim_end_matches('/')).then(|| PathBuf::from(up));
    }
    p.parent().map(|x| x.to_path_buf())
}

/// Remote folder: the `*.txt` files on the server (notes live in `notes/`, so no note-file filtering is needed).
fn list_remote_txt(dir: &Path) -> Vec<PathBuf> {
    let Ok(server) = crate::store::server_of(dir) else { return Vec::new() };
    let url = dir.to_string_lossy();
    let Ok(list) = crate::webdav::list(&server, &url) else { return Vec::new() };
    let mut out: Vec<PathBuf> = list
        .iter()
        .filter(|e| !e.is_dir)
        .map(|e| crate::webdav::file_name(&e.url))
        .filter(|n| is_txt(n) && !n.starts_with('.'))
        .map(|n| PathBuf::from(crate::webdav::join(&url, &n)))
        .collect();
    out.sort_by_key(|p| p.to_string_lossy().to_lowercase());
    out
}

/// The visible `*.txt` files of `dir`, sorted by name; files that are the note file of a task
/// (`note:NAME` in one of the others) are left out.
pub fn list_txt_files(dir: &Path) -> Vec<PathBuf> {
    if is_remote_dir(dir) {
        return list_remote_txt(dir);
    }
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || is_hidden(&name, &meta) || !is_txt(&name) {
                continue;
            }
            let text = crate::crypt::open_quiet(&fs::read_to_string(entry.path()).unwrap_or_default());
            files.push((entry.path(), text));
        }
    }
    let mut refs = HashSet::new();
    for (_, text) in &files {
        note_refs(text, &mut refs);
    }
    let mut out: Vec<PathBuf> = files
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| {
            let stem = p.file_stem().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            !refs.contains(&stem)
        })
        .collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    out
}

/// The file to open when a folder is chosen: the remembered one if it is inside, else `todo.txt`, else the first.
pub fn pick_file(dir: &Path, remembered: Option<&Path>) -> PathBuf {
    if let Some(r) = remembered {
        if r.parent() == Some(dir) && (is_remote_dir(dir) || r.exists()) {
            return r.to_path_buf();
        }
    }
    let files = list_txt_files(dir);
    files
        .iter()
        .find(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("todo.txt")))
        .or_else(|| files.first())
        .cloned()
        .unwrap_or_else(|| join(dir, "todo.txt"))
}

/// Where the note files live.
pub fn note_dir(folder: Option<&Path>) -> PathBuf {
    match folder {
        Some(f) => f.join("notes"),
        None => home_dir().join(".todo.txt").join("notes"),
    }
}

pub fn note_file(folder: Option<&Path>, name: &str) -> PathBuf {
    note_dir(folder).join(format!("{name}.txt"))
}

/// A valid note name: letters, digits, `_ - .`, not starting with a dot.
pub fn valid_note_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// All `note:NAME` names of a task, in order (a task can have several notes).
pub fn note_names(t: &crate::todotxt::Task) -> Vec<String> {
    t.text
        .split_whitespace()
        .filter_map(|w| w.strip_prefix("note:"))
        .filter(|n| valid_note_name(n))
        .map(str::to_string)
        .collect()
}

/// The first `note:NAME` of a task, if any.
pub fn note_name(t: &crate::todotxt::Task) -> Option<String> {
    note_names(t).into_iter().next()
}

/// File-name-safe short name from a task description (projects, contexts and key:value words dropped).
#[allow(dead_code)]
pub fn slugify(text: &str) -> String {
    let mut out = String::new();
    for w in text.split_whitespace() {
        if w.starts_with('+') || w.starts_with('@') || w.contains(':') {
            continue;
        }
        for c in w.chars() {
            let mapped: String = match c {
                'ä' | 'Ä' => "a".into(),
                'ö' | 'Ö' => "o".into(),
                'ü' | 'Ü' => "u".into(),
                'ß' => "ss".into(),
                c => c.to_ascii_lowercase().to_string(),
            };
            for c in mapped.chars() {
                if c.is_ascii_alphanumeric() {
                    out.push(c);
                } else if !out.ends_with('-') && !out.is_empty() {
                    out.push('-');
                }
            }
        }
        if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
        if out.len() >= 32 {
            break;
        }
    }
    let s: String = out.trim_matches('-').chars().take(32).collect();
    let s = s.trim_end_matches('-').to_string();
    if s.is_empty() { "note".into() } else { s }
}

/// `Note-1`, `Note-2`, ...: the first name that no line uses and that has no note file yet.
pub fn next_note_name(lines: &[String], folder: Option<&Path>) -> String {
    let used: HashSet<String> = lines
        .iter()
        .flat_map(|l| note_names(&crate::todotxt::parse(l)))
        .map(|n| n.to_lowercase())
        .collect();
    for n in 1.. {
        let cand = format!("Note-{n}");
        if !used.contains(&cand.to_lowercase()) && !note_file(folder, &cand).exists() {
            return cand;
        }
    }
    unreachable!()
}

/// Create the (empty) note file and its directory if needed.
pub fn ensure_note_file(folder: Option<&Path>, name: &str) -> std::io::Result<PathBuf> {
    let p = note_file(folder, name);
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir)?;
    }
    if !p.exists() {
        fs::write(&p, "")?;
    }
    Ok(p)
}

/// A note file ready for the editor. For a todo file on a WebDAV server the note lives in `notes/NAME.txt` next to
/// it on the server: it is downloaded to a temporary local file and uploaded again by `finish_note`.
pub struct NoteFile {
    pub local: PathBuf,
    remote: Option<(crate::webdav::Server, String)>,
}

impl NoteFile {
    /// The note text (a local note may be encrypted; the temporary copy of a remote note is plain).
    pub fn read(&self) -> String {
        crate::crypt::open(&fs::read_to_string(&self.local).unwrap_or_default()).unwrap_or_default()
    }

    /// Write the note text: encrypted when encryption is on (not the temporary copy of a remote note: the upload encrypts).
    pub fn write(&self, text: &str) -> std::io::Result<()> {
        if self.remote.is_some() {
            return fs::write(&self.local, text);
        }
        fs::write(&self.local, crate::crypt::seal_for_path(&self.local, text).map_err(std::io::Error::from)?)
    }
}

pub fn prepare_note(todo: &Path, folder: Option<&Path>, name: &str) -> std::io::Result<NoteFile> {
    let t = todo.to_string_lossy().into_owned();
    if !crate::webdav::is_remote(&t) {
        let nf = NoteFile { local: ensure_note_file(folder, name)?, remote: None };
        // an encrypted note that cannot be decrypted must not be opened (saving would overwrite it)
        let raw = fs::read_to_string(&nf.local).unwrap_or_default();
        if crate::crypt::is_encrypted(&raw) {
            crate::crypt::open(&raw)?;
        }
        return Ok(nf);
    }
    let server = crate::store::server_of(todo)?;
    let url = crate::webdav::join(&format!("{}notes/", crate::webdav::parent(&t)), &format!("{name}.txt"));
    let text = match crate::webdav::get(&server, &url) {
        Ok((text, _)) => text,
        Err(crate::webdav::Error::NotFound) => String::new(),
        Err(e) => return Err(std::io::Error::new(std::io::ErrorKind::Other, e.to_string())),
    };
    let dir = std::env::temp_dir().join("todo-notes");
    fs::create_dir_all(&dir)?;
    let local = dir.join(format!("{name}.txt"));
    fs::write(&local, text)?;
    Ok(NoteFile { local, remote: Some((server, url)) })
}

/// After editing: upload a remote note (an empty one is deleted), remove the temporary file or an empty local note.
pub fn finish_note(n: &NoteFile) -> std::io::Result<()> {
    let text = n.read();
    let err = |e: crate::webdav::Error| std::io::Error::new(std::io::ErrorKind::Other, e.to_string());
    match &n.remote {
        Some((server, url)) => {
            if text.trim().is_empty() {
                crate::webdav::delete(server, url).map_err(err)?;
            } else {
                crate::webdav::put(server, url, &text, None).map_err(err)?;
            }
            let _ = fs::remove_file(&n.local);
        }
        None => {
            if text.is_empty() {
                let _ = fs::remove_file(&n.local); // an empty note file is not worth keeping
            }
        }
    }
    Ok(())
}

/// True if the note file (after editing) has any text.
pub fn note_nonempty(n: &NoteFile) -> bool {
    !n.read().trim().is_empty()
}

/// Default name for a note typed into a text that is not in `lines` yet.
pub fn next_note_name_with(lines: &[String], extra: &str, folder: Option<&Path>) -> String {
    let mut v = lines.to_vec();
    v.push(extra.to_string());
    next_note_name(&v, folder)
}

/// Where the interactive add/edit prompt (`todo -a`, `-e`) finds its notes.
pub struct NoteCtx<'a> {
    pub todo: &'a Path,
    pub folder: Option<&'a Path>,
    pub lines: Vec<String>,
}

/// Edit the note `name` in the built-in multi-line input (CLI; Enter = new line, Ctrl+Enter saves). Returns false if
/// the note is empty afterwards.
pub fn note_via_editor(todo: &Path, folder: Option<&Path>, name: &str) -> std::io::Result<bool> {
    let n = prepare_note(todo, folder, name)?;
    edit_note_inline(&n, name)?;
    let has = note_nonempty(&n);
    finish_note(&n)?;
    Ok(has)
}

/// Run the inline multi-line input on the note file (cancel keeps the old content).
pub fn edit_note_inline(n: &NoteFile, name: &str) -> std::io::Result<()> {
    let cur = n.read();
    if let Some(t) = crate::textarea::run_inline(&format!("note:{name}"), &cur)? {
        n.write(&t)?;
    }
    Ok(())
}

/// Delete the note files of a removed task, unless another line still uses the name.
pub fn remove_note(todo: &Path, folder: Option<&Path>, removed: &str, remaining: &[String]) {
    let remote = crate::webdav::is_remote(&todo.to_string_lossy());
    let still: HashSet<String> = remaining.iter().flat_map(|l| note_names(&crate::todotxt::parse(l))).collect();
    for name in note_names(&crate::todotxt::parse(removed)) {
        if still.contains(&name) {
            continue;
        }
        if remote {
            if let Ok(server) = crate::store::server_of(todo) {
                let t = todo.to_string_lossy();
                let _ = crate::webdav::delete(&server, &crate::webdav::join(&format!("{}notes/", crate::webdav::parent(&t)), &format!("{name}.txt")));
            }
            continue;
        }
        let p = note_file(folder, &name);
        if fs::remove_file(&p).is_ok() {
            if let Some(dir) = p.parent() {
                let _ = fs::remove_dir(dir); // only if empty
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug() {
        assert_eq!(slugify("Call Mom +Family @phone due:2024-01-01"), "call-mom");
        assert_eq!(slugify("Größe prüfen!"), "grosse-prufen");
        assert_eq!(slugify("+only @tags"), "note");
    }

    #[test]
    fn folder_listing_skips_hidden_and_note_files() {
        let dir = std::env::temp_dir().join(format!("todo-folder-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("b.txt"), "task note:note
").unwrap();
        fs::write(dir.join("A.txt"), "x
").unwrap();
        fs::write(dir.join(".hidden.txt"), "x
").unwrap();
        fs::write(dir.join("note.txt"), "long text
").unwrap();
        fs::write(dir.join("readme.md"), "x
").unwrap();
        let names: Vec<String> =
            list_txt_files(&dir).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(names, vec!["A.txt", "b.txt"]);
    }

    #[test]
    fn names() {
        assert!(valid_note_name("my-note.v2"));
        assert!(!valid_note_name("../x"));
        assert!(!valid_note_name(".hidden"));
    }

    #[test]
    fn several_notes_per_task() {
        let t = crate::todotxt::parse("(A) call mom note:Note-1 note:b.c note:../x");
        assert_eq!(note_names(&t), vec!["Note-1", "b.c"]);
        assert_eq!(note_name(&t).as_deref(), Some("Note-1"));
        let lines = vec!["x note:Note-1".to_string(), "y note:note-2".to_string()];
        assert_eq!(next_note_name(&lines, Some(Path::new("."))), "Note-3");
    }
}

/// Is `file` located in `dir` (directly or below)? Case-insensitive, `/` and `\` alike.
pub fn is_inside(dir: &Path, file: &Path) -> bool {
    let norm = |p: &Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    norm(file).starts_with(&(norm(dir) + "/"))
}
