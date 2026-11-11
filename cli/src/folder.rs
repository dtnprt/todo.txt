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

/// The visible `*.txt` files of `dir`, sorted by name; files that are the note file of a task
/// (`note:NAME` in one of the others) are left out.
pub fn list_txt_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<(PathBuf, String)> = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for entry in rd.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() || is_hidden(&name, &meta) || !is_txt(&name) {
                continue;
            }
            let text = fs::read_to_string(entry.path()).unwrap_or_default();
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
        if r.parent() == Some(dir) && r.exists() {
            return r.to_path_buf();
        }
    }
    let files = list_txt_files(dir);
    files
        .iter()
        .find(|p| p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("todo.txt")))
        .or_else(|| files.first())
        .cloned()
        .unwrap_or_else(|| dir.join("todo.txt"))
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

/// `Note-A`, `Note-B`, ...: the first name that no line uses and that has no note file yet.
pub fn next_note_name(lines: &[String], folder: Option<&Path>) -> String {
    let used: HashSet<String> = lines
        .iter()
        .flat_map(|l| note_names(&crate::todotxt::parse(l)))
        .map(|n| n.to_lowercase())
        .collect();
    for n in 0.. {
        let cand = match n {
            0..=25 => format!("Note-{}", (b'A' + n as u8) as char),
            _ => format!("Note-{}", n + 1),
        };
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

pub fn prepare_note(todo: &Path, folder: Option<&Path>, name: &str) -> std::io::Result<NoteFile> {
    let t = todo.to_string_lossy().into_owned();
    if !crate::webdav::is_remote(&t) {
        return Ok(NoteFile { local: ensure_note_file(folder, name)?, remote: None });
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
    let text = fs::read_to_string(&n.local).unwrap_or_default();
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
    fs::read_to_string(&n.local).map(|t| !t.trim().is_empty()).unwrap_or(false)
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

/// Open the editor for a new note `name` (typed as `note:` in an input). Returns false if it stayed empty.
pub fn note_via_editor(todo: &Path, folder: Option<&Path>, name: &str) -> std::io::Result<bool> {
    let n = prepare_note(todo, folder, name)?;
    edit_in_editor(&n.local)?;
    let has = note_nonempty(&n);
    finish_note(&n)?;
    Ok(has)
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

/// Open `path` in the user's editor ($VISUAL / $EDITOR, else notepad on Windows, vi elsewhere) and wait.
pub fn edit_in_editor(path: &Path) -> std::io::Result<()> {
    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| if cfg!(windows) { "notepad".into() } else { "vi".into() });
    let mut parts = editor.split_whitespace();
    let prog = parts.next().unwrap_or("vi");
    let status = std::process::Command::new(prog).args(parts).arg(path).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, format!("{editor} exited with {status}")))
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
        let t = crate::todotxt::parse("(A) call mom note:Note-A note:b.c note:../x");
        assert_eq!(note_names(&t), vec!["Note-A", "b.c"]);
        assert_eq!(note_name(&t).as_deref(), Some("Note-A"));
        let lines = vec!["x note:Note-A".to_string(), "y note:note-b".to_string()];
        assert_eq!(next_note_name(&lines, Some(Path::new("."))), "Note-C");
    }
}
