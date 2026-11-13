//! Local backup of the todo data as a `.zip` (shared by the terminal app and the desktop app via `#[path]`).
//!
//! The files are copied exactly as they are stored: encrypted lists, notes and images stay encrypted in the archive.
//! Folder mode: everything in the folder (lists, `notes/`, `images/`, `done.txt` ...) except hidden files and folders
//! (`.git`, `.todo-git.cfg`) and the backup folder itself. Single file: the list, the `done.txt` next to it and the
//! shared `~/.todo.txt/notes` and `~/.todo.txt/images`. A WebDAV list is not local and cannot be backed up this way.
//!
//! Settings in `todo.cfg`: `backupdir=` (default `~/.todo.txt/backups`), `backupkeep=` (automatic backups to keep, default 20),
//! `backuparchive=true|false` (default true: archiving done tasks makes a backup first).

#![allow(dead_code)] // each client uses a different part

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

const PREFIX: &str = "todo-backup-";

fn other(m: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::Other, m.to_string())
}

/// Where automatic backups go.
pub fn backup_dir() -> PathBuf {
    match crate::crypt::cfg_get("backupdir").filter(|d| !d.is_empty()) {
        Some(d) => PathBuf::from(d),
        None => crate::crypt::home().join(".todo.txt").join("backups"),
    }
}

/// Does archiving done tasks make a backup first? (default: yes)
pub fn archive_backup_enabled() -> bool {
    !matches!(crate::crypt::cfg_get("backuparchive").as_deref(), Some("false" | "off" | "0" | "no"))
}

fn keep_count() -> usize {
    crate::crypt::cfg_get("backupkeep").and_then(|v| v.parse().ok()).unwrap_or(20)
}

fn visible(name: &str) -> bool {
    !name.starts_with('.')
}

/// All files below `dir` (not hidden, not `skip`) as (path, name inside the zip, using `/`).
fn walk(dir: &Path, rel: &str, skip: &Path, out: &mut Vec<(PathBuf, String)>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let path = e.path();
        if !visible(&name) || path == skip {
            continue;
        }
        let zip_name = if rel.is_empty() { name.clone() } else { format!("{rel}/{name}") };
        match e.metadata() {
            Ok(m) if m.is_dir() => walk(&path, &zip_name, skip, out),
            Ok(m) if m.is_file() => out.push((path, zip_name)),
            _ => {}
        }
    }
}

/// The files a backup contains.
pub fn files_for(todo: &Path, folder: Option<&Path>) -> io::Result<Vec<(PathBuf, String)>> {
    let mut out = Vec::new();
    let skip = backup_dir();
    match folder {
        Some(d) if !crate::webdav::is_remote(&d.to_string_lossy()) => walk(d, "", &skip, &mut out),
        Some(_) => return Err(other("a WebDAV folder cannot be backed up (only local files)")),
        None => {
            if crate::webdav::is_remote(&todo.to_string_lossy()) {
                return Err(other("a WebDAV list cannot be backed up (only local files)"));
            }
            if todo.exists() {
                out.push((todo.to_path_buf(), todo.file_name().map_or_else(|| "todo.txt".into(), |n| n.to_string_lossy().into_owned())));
            }
            let done = todo.with_file_name("done.txt");
            if done.exists() && done != todo {
                out.push((done, "done.txt".into()));
            }
            let base = crate::crypt::home().join(".todo.txt");
            walk(&base.join("notes"), "notes", &skip, &mut out);
            walk(&base.join("images"), "images", &skip, &mut out);
        }
    }
    Ok(out)
}

/// Write the backup. `dest` = the zip file to create (None: a new `todo-backup-DATE.zip` in the backup folder, old
/// automatic backups beyond `backupkeep` are deleted). Returns the zip path and the number of files in it.
pub fn create(todo: &Path, folder: Option<&Path>, dest: Option<&Path>) -> io::Result<(PathBuf, usize)> {
    let files = files_for(todo, folder)?;
    if files.is_empty() {
        return Err(other("nothing to back up (no todo file yet)"));
    }
    let (path, auto) = match dest {
        Some(d) => (d.to_path_buf(), false),
        None => {
            let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
            (backup_dir().join(format!("{PREFIX}{stamp}.zip")), true)
        }
    };
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            fs::create_dir_all(dir)?;
        }
    }
    // write to a temporary name first so a failed backup never leaves a broken zip behind
    let tmp = path.with_extension("zip.part");
    let res = (|| -> io::Result<()> {
        let mut zip = zip::ZipWriter::new(fs::File::create(&tmp)?);
        let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (src, name) in &files {
            zip.start_file(name.as_str(), opts).map_err(other)?;
            zip.write_all(&fs::read(src)?)?;
        }
        zip.finish().map_err(other)?;
        Ok(())
    })();
    if let Err(e) = res {
        let _ = fs::remove_file(&tmp);
        return Err(e);
    }
    fs::rename(&tmp, &path)?;
    if auto {
        prune(&path);
    }
    Ok((path, files.len()))
}

/// Delete the oldest automatic backups beyond `backupkeep` (file names `todo-backup-*.zip` in the same folder).
fn prune(newest: &Path) {
    let Some(dir) = newest.parent() else { return };
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut names: Vec<String> = rd
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with(PREFIX) && n.ends_with(".zip"))
        .collect();
    names.sort(); // the time stamp sorts chronologically
    let keep = keep_count().max(1);
    if names.len() > keep {
        for n in &names[..names.len() - keep] {
            let _ = fs::remove_file(dir.join(n));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn folder_backup_contains_lists_notes_images_but_no_hidden_files() {
        let root = std::env::temp_dir().join(format!("todo-backup-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("notes")).unwrap();
        fs::create_dir_all(root.join("images")).unwrap();
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::write(root.join("todo.txt"), "(A) one\n").unwrap();
        fs::write(root.join("notes").join("n.txt"), "note\n").unwrap();
        fs::write(root.join("images").join("i.png"), [1u8, 2, 3]).unwrap();
        fs::write(root.join(".git").join("config"), "x").unwrap();
        fs::write(root.join(".todo-git.cfg"), "x").unwrap();
        let zip_path = root.join("out.zip");
        let (p, n) = create(&root.join("todo.txt"), Some(&root), Some(&zip_path)).unwrap();
        assert_eq!(p, zip_path);
        assert_eq!(n, 3);
        let mut z = zip::ZipArchive::new(fs::File::open(&zip_path).unwrap()).unwrap();
        let mut names: Vec<String> = (0..z.len()).map(|i| z.by_index(i).unwrap().name().to_string()).collect();
        names.sort();
        assert_eq!(names, ["images/i.png", "notes/n.txt", "todo.txt"]);
        let mut s = String::new();
        z.by_name("todo.txt").unwrap().read_to_string(&mut s).unwrap();
        assert_eq!(s, "(A) one\n");
        let _ = fs::remove_dir_all(&root);
    }
}
