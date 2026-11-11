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

/// The `note:NAME` of a task line, if valid.
pub fn note_name(tags: &std::collections::BTreeMap<String, String>) -> Option<String> {
    tags.get("note").filter(|n| valid_note_name(n)).cloned()
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

/// `base`, `base-2`, ... : the first name that no line uses and that has no note file yet.
pub fn unique_note_name(base: &str, lines: &[String], folder: Option<&Path>) -> String {
    let used: HashSet<String> = lines
        .iter()
        .filter_map(|l| note_name(&crate::todotxt::parse(l).tags))
        .collect();
    let mut n = 1;
    loop {
        let cand = if n == 1 { base.to_string() } else { format!("{base}-{n}") };
        if !used.contains(&cand) && !note_file(folder, &cand).exists() {
            return cand;
        }
        n += 1;
    }
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

/// Delete the note file of a removed task, unless another line still uses the name.
pub fn remove_note(folder: Option<&Path>, removed: &str, remaining: &[String]) {
    let Some(name) = note_name(&crate::todotxt::parse(removed).tags) else { return };
    if remaining.iter().any(|l| note_name(&crate::todotxt::parse(l).tags).as_deref() == Some(name.as_str())) {
        return;
    }
    let p = note_file(folder, &name);
    if fs::remove_file(&p).is_ok() {
        if let Some(dir) = p.parent() {
            let _ = fs::remove_dir(dir); // only if empty
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
}
