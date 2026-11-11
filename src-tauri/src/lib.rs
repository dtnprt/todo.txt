// The UI lives in ../web (plain HTML/JS). Rust provides the window, the native dialog / file-system
// plugins that web/app.js uses through window.__TAURI__, and a few small commands for folders and
// note files (hidden-file filtering needs the Windows file attributes, which JS cannot see).

// git support (pull / commit / push, .todo-git.cfg): the same code as the terminal app
#[path = "../../cli/src/git.rs"]
mod git;

// WebDAV storage + keyring credentials: the same code as the terminal app
#[path = "../../cli/src/webdav.rs"]
mod webdav;

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.todo.txt/todo.cfg` (shared with the CLI); an old `~/.config/todo.cfg` is copied there once.
#[tauri::command]
fn cfg_path() -> String {
    let new = home_dir().join(".todo.txt").join("todo.cfg");
    if !new.exists() {
        let old = home_dir().join(".config").join("todo.cfg");
        let _ = fs::create_dir_all(home_dir().join(".todo.txt"));
        if old.exists() {
            let _ = fs::copy(&old, &new);
        }
    }
    new.to_string_lossy().replace('\\', "/")
}

/// Dot files and (on Windows) files with the "hidden" attribute.
fn is_hidden(name: &str, meta: &fs::Metadata) -> bool {
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

/// Names referenced as `note:NAME` in the given text.
fn note_refs(text: &str, out: &mut HashSet<String>) {
    for word in text.split_whitespace() {
        if let Some(n) = word.strip_prefix("note:") {
            out.insert(n.to_lowercase());
        }
    }
}

/// The visible `*.txt` files of a folder, sorted; files that are the note file of a task are left out.
#[tauri::command]
fn list_txt_files(dir: String) -> Result<Vec<String>, String> {
    let mut files: Vec<(String, String)> = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() || is_hidden(&name, &meta) || !name.to_lowercase().ends_with(".txt") {
            continue;
        }
        let text = fs::read_to_string(entry.path()).unwrap_or_default();
        files.push((name, text));
    }
    let mut refs = HashSet::new();
    for (_, text) in &files {
        note_refs(text, &mut refs);
    }
    let mut names: Vec<String> = files
        .into_iter()
        .map(|(n, _)| n)
        .filter(|n| !refs.contains(&n[..n.len() - 4].to_lowercase()))
        .collect();
    names.sort_by_key(|n| n.to_lowercase());
    Ok(names)
}

/// Folder of the note files: `<folder>/note` in folder mode, else `~/.todo.txt/note`.
fn note_path(folder: &Option<String>, name: &str) -> Result<PathBuf, String> {
    let ok = !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
    if !ok {
        return Err(format!("invalid note name '{name}'"));
    }
    let dir = match folder.as_deref().filter(|f| !f.is_empty()) {
        Some(f) => PathBuf::from(f).join("notes"),
        None => home_dir().join(".todo.txt").join("notes"),
    };
    Ok(dir.join(format!("{name}.txt")))
}

#[tauri::command]
fn read_note(folder: Option<String>, name: String) -> Result<String, String> {
    let p = note_path(&folder, &name)?;
    match fs::read_to_string(&p) {
        Ok(t) => Ok(t),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
fn write_note(folder: Option<String>, name: String, text: String) -> Result<(), String> {
    let p = note_path(&folder, &name)?;
    if text.is_empty() && !Path::new(&p).exists() {
        return Ok(()); // nothing to store
    }
    if let Some(dir) = p.parent() {
        fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    fs::write(&p, text).map_err(|e| e.to_string())
}

/// Own themes: `~/.todo.txt/themes/NAME.css` -> [(NAME, css)].
#[tauri::command]
fn list_user_themes() -> Vec<(String, String)> {
    let dir = home_dir().join(".todo.txt").join("themes");
    let mut out = Vec::new();
    if let Ok(rd) = fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("css")) {
                if let (Some(stem), Ok(css)) = (p.file_stem(), fs::read_to_string(&p)) {
                    out.push((stem.to_string_lossy().into_owned(), css));
                }
            }
        }
    }
    out.sort();
    out
}

/// Write built-in themes (`[(NAME, css)]`) to `~/.todo.txt/themes/NAME.css`; existing files are kept. Returns (written, kept).
#[tauri::command]
fn export_themes(files: Vec<(String, String)>) -> Result<(usize, usize), String> {
    let dir = home_dir().join(".todo.txt").join("themes");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let (mut written, mut kept) = (0, 0);
    for (name, css) in files {
        if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '-' || c == '_') {
            continue;
        }
        let p = dir.join(format!("{name}.css"));
        if p.exists() {
            kept += 1;
        } else {
            fs::write(&p, css).map_err(|e| e.to_string())?;
            written += 1;
        }
    }
    Ok((written, kept))
}

#[tauri::command]
fn delete_note(folder: Option<String>, name: String) -> Result<(), String> {
    let p = note_path(&folder, &name)?;
    match fs::remove_file(&p) {
        Ok(()) => {
            if let Some(dir) = p.parent() {
                let _ = fs::remove_dir(dir); // only if empty
            }
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

// ---------------------------------------------------------------- version + git

/// "0.1.12 (a1b2c3d)": VERSION file + commit hash at build time (see build.ps1).
#[tauri::command]
fn app_version() -> String {
    env!("TODO_VERSION_FULL").to_string()
}

#[derive(serde::Serialize)]
struct GitInfo {
    /// The folder is a git repository.
    repo: bool,
    /// `.todo-git.cfg` exists (without it no automatic git action happens).
    configured: bool,
    enabled: bool,
    auto: bool,
    pull: bool,
    commit: bool,
    push: bool,
}

fn git_info_of(dir: &str) -> GitInfo {
    let d = Path::new(dir);
    let repo = d.join(".git").exists();
    match git::GitCfg::load(d) {
        Some(c) => GitInfo { repo, configured: true, enabled: c.enabled, auto: c.auto, pull: c.pull, commit: c.commit, push: c.push },
        None => GitInfo { repo, configured: false, enabled: false, auto: false, pull: false, commit: false, push: false },
    }
}

/// git2 calls block, so they run on a worker thread.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn git_info(dir: String) -> Result<GitInfo, String> {
    blocking(move || git_info_of(&dir)).await
}

/// Change one setting of `.todo-git.cfg` (enabled, auto, pull, commit, push).
#[tauri::command]
async fn git_set(dir: String, key: String, on: bool) -> Result<GitInfo, String> {
    blocking(move || {
        let d = Path::new(&dir);
        let mut c = git::GitCfg::load(d).ok_or("no .todo-git.cfg: initialize git first")?;
        c.set(&key, on)?;
        c.save(d).map_err(|e| e.to_string())?;
        Ok::<GitInfo, String>(git_info_of(&dir))
    })
    .await?
}

/// Branch, current commit and working-tree state for the status bar (None: not a repository).
#[tauri::command]
async fn git_status(dir: String) -> Result<Option<GitStatus>, String> {
    blocking(move || {
        git::status(Path::new(&dir)).map(|s| GitStatus {
            label: s.label(),
            summary: s.summary,
            branch: s.branch,
            commit: s.commit,
            changed: s.changed,
            ahead: s.ahead,
            behind: s.behind,
        })
    })
    .await
}

#[derive(serde::Serialize)]
struct GitStatus {
    /// Ready-made text for the status bar (same as the terminal app).
    label: String,
    summary: String,
    branch: String,
    commit: String,
    changed: usize,
    ahead: usize,
    behind: usize,
}

#[tauri::command]
async fn git_init(dir: String) -> Result<String, String> {
    blocking(move || git::init(Path::new(&dir))).await?
}

/// Folder opened / app started: pull first (only with .todo-git.cfg: enabled, auto, pull).
#[tauri::command]
async fn git_start(dir: String) -> Result<String, String> {
    blocking(move || git::auto_start(Path::new(&dir), false)).await?
}

/// A file was saved: commit and push (in the background) as configured.
#[tauri::command]
async fn git_changed(dir: String, name: String) -> Result<String, String> {
    blocking(move || git::auto_change(Path::new(&dir), &format!("todo: update {name}"), true, false)).await?
}

#[tauri::command]
async fn git_sync(dir: String) -> Result<String, String> {
    blocking(move || git::sync(Path::new(&dir), false)).await?
}

/// Outcome of the last background push, once.
#[tauri::command]
fn git_result() -> Option<String> {
    git::take_result()
}

// ---------------------------------------------------------------- WebDAV
// `entry` is the server as configured (`USER@URL`, config key `webdav=`); the password only ever lives in the keyring.

fn server(entry: &str) -> Result<webdav::Server, String> {
    webdav::Server::parse(entry).ok_or_else(|| format!("invalid server '{entry}' (USER@URL)"))
}

/// Store the password in the keyring and check that the server accepts the login.
#[tauri::command]
async fn webdav_login(entry: String, password: String) -> Result<(), String> {
    blocking(move || {
        let s = server(&entry)?;
        webdav::cred::set(&s, &password)?;
        if let Err(e) = webdav::list(&s, &s.url) {
            let _ = webdav::cred::delete(&s);
            return Err(format!("login failed: {e}"));
        }
        Ok(())
    })
    .await?
}

#[tauri::command]
async fn webdav_logout(entry: String) -> Result<(), String> {
    blocking(move || webdav::cred::delete(&server(&entry)?)).await?
}

#[tauri::command]
async fn webdav_has_password(entry: String) -> Result<bool, String> {
    blocking(move || server(&entry).map(|s| webdav::cred::exists(&s))).await?
}

#[derive(serde::Serialize)]
struct DavEntry {
    name: String,
    url: String,
    is_dir: bool,
    size: u64,
}

#[tauri::command]
async fn webdav_list(entry: String, url: String) -> Result<Vec<DavEntry>, String> {
    blocking(move || {
        let s = server(&entry)?;
        Ok::<_, String>(
            webdav::list(&s, &url)?
                .into_iter()
                .map(|e| DavEntry { name: e.name, url: e.url, is_dir: e.is_dir, size: e.size })
                .collect(),
        )
    })
    .await?
}

#[derive(serde::Serialize)]
struct DavFile {
    text: String,
    etag: Option<String>,
    /// The file does not exist yet (it is created on the first save).
    missing: bool,
}

#[tauri::command]
async fn webdav_get(entry: String, url: String) -> Result<DavFile, String> {
    blocking(move || {
        let s = server(&entry)?;
        match webdav::get(&s, &url) {
            Ok((text, etag)) => Ok(DavFile { text, etag, missing: false }),
            Err(webdav::Error::NotFound) => Ok(DavFile { text: String::new(), etag: None, missing: true }),
            Err(e) => Err(e.to_string()),
        }
    })
    .await?
}

/// Write with `If-Match: etag`; the error text starts with `conflict:` when the file changed meanwhile.
#[tauri::command]
async fn webdav_put(entry: String, url: String, text: String, etag: Option<String>) -> Result<Option<String>, String> {
    blocking(move || {
        let s = server(&entry)?;
        webdav::put(&s, &url, &text, etag.as_deref()).map_err(|e| match e {
            webdav::Error::Conflict => format!("conflict: {e}"),
            e => e.to_string(),
        })
    })
    .await?
}

#[tauri::command]
async fn webdav_etag(entry: String, url: String) -> Result<Option<String>, String> {
    blocking(move || {
        let s = server(&entry)?;
        match webdav::etag(&s, &url) {
            Err(webdav::Error::NotFound) => Ok(None),
            r => r.map_err(|e| e.to_string()),
        }
    })
    .await?
}

#[tauri::command]
async fn webdav_mkcol(entry: String, url: String) -> Result<(), String> {
    blocking(move || webdav::mkcol(&server(&entry)?, &url).map_err(|e| e.to_string())).await?
}

#[tauri::command]
async fn webdav_delete(entry: String, url: String) -> Result<(), String> {
    blocking(move || webdav::delete(&server(&entry)?, &url).map_err(|e| e.to_string())).await?
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            cfg_path,
            list_txt_files,
            read_note,
            write_note,
            delete_note,
            list_user_themes,
            export_themes,
            app_version,
            git_info,
            git_status,
            git_set,
            git_init,
            git_start,
            git_changed,
            git_sync,
            git_result,
            webdav_login,
            webdav_logout,
            webdav_has_password,
            webdav_list,
            webdav_get,
            webdav_put,
            webdav_etag,
            webdav_mkcol,
            webdav_delete
        ])
        .run(tauri::generate_context!())
        .expect("error while running the todo.txt application");
}
