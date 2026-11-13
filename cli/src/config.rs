//! Settings kept in `~/.todo.txt/todo.cfg` (in the user's profile; `%USERPROFILE%` on Windows).
//! Plain `key=value` lines; unknown keys are preserved so future versions can add more.

use std::fs;
use std::path::PathBuf;

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `~/.todo.txt/todo.cfg`. An old `~/.config/todo.cfg` is copied there once.
pub fn config_path() -> PathBuf {
    let new = home_dir().join(".todo.txt").join("todo.cfg");
    if !new.exists() {
        let old = home_dir().join(".config").join("todo.cfg");
        if old.exists() {
            if let Some(dir) = new.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = fs::copy(&old, &new);
        }
    }
    new
}

#[derive(Debug, Default, Clone)]
pub struct Config {
    /// The todo.txt that was used last.
    pub file: Option<PathBuf>,
    /// The file used last by any client (`lastfile=`, written on every use, also for one-off files); opened at start.
    pub lastfile: Option<PathBuf>,
    /// Default sort order name (priority, due, created, alpha, file).
    pub sort: Option<String>,
    /// Folder mode: a folder whose `*.txt` files are all todo lists (`folder=`, shared with the desktop app).
    pub folder: Option<PathBuf>,
    /// View settings of the terminal UI (`showdone=`, `sidebar=`, `calendar=`).
    pub show_done: Option<bool>,
    pub sidebar: Option<bool>,
    pub calendar: Option<bool>,
    /// Status bar visible (`statusbar=`, shared with the desktop app).
    pub statusbar: Option<bool>,
    /// All known todo.txt files (`files=["a","b"]`, JSON array, shared with the desktop app).
    pub files: Vec<PathBuf>,
    /// WebDAV servers as `USER@URL` (`webdav=["a@https://host/dav/"]`, shared with the desktop app); passwords are in the keyring.
    pub webdav: Vec<String>,
    /// Keys this version does not know about.
    other: Vec<(String, String)>,
}

impl Config {
    pub fn load() -> Config {
        let mut cfg = Config::default();
        let Ok(text) = fs::read_to_string(config_path()) else {
            return cfg;
        };
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "file" if !v.is_empty() => cfg.file = Some(PathBuf::from(v)),
                "lastfile" if !v.is_empty() => cfg.lastfile = Some(PathBuf::from(v)),
                "sort" if !v.is_empty() => cfg.sort = Some(v.to_string()),
                "folder" => cfg.folder = Some(PathBuf::from(v)).filter(|_| !v.is_empty()),
                "showdone" => cfg.show_done = parse_bool(v),
                "sidebar" => cfg.sidebar = parse_bool(v),
                "calendar" => cfg.calendar = parse_bool(v),
                "statusbar" => cfg.statusbar = parse_bool(v),
                "webdav" => cfg.webdav = parse_json_strings(v),
                "files" => cfg.files = parse_json_strings(v).into_iter().map(PathBuf::from).collect(),
                _ => cfg.other.push((k.to_string(), v.to_string())),
            }
        }
        cfg
    }

    /// Set a key this module does not know about (kept in the file as is).
    pub fn set_other(&mut self, key: &str, value: &str) {
        self.other.retain(|(k, _)| k != key);
        self.other.push((key.to_string(), value.to_string()));
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut out = String::from("# todo.txt command line tool\n");
        if let Some(f) = &self.file {
            out.push_str(&format!("file={}\n", f.display()));
        }
        if let Some(f) = &self.lastfile {
            out.push_str(&format!("lastfile={}
", f.display()));
        }
        if let Some(s) = &self.sort {
            out.push_str(&format!("sort={s}\n"));
        }
        if let Some(f) = &self.folder {
            out.push_str(&format!("folder={}\n", f.display()));
        }
        for (k, v) in [("showdone", self.show_done), ("sidebar", self.sidebar), ("calendar", self.calendar), ("statusbar", self.statusbar)] {
            if let Some(v) = v {
                out.push_str(&format!("{k}={v}\n"));
            }
        }
        if !self.files.is_empty() {
            let items: Vec<String> = self.files.iter().map(|f| json_string(&f.display().to_string())).collect();
            out.push_str(&format!("files=[{}]\n", items.join(",")));
        }
        if !self.webdav.is_empty() {
            let items: Vec<String> = self.webdav.iter().map(|f| json_string(f)).collect();
            out.push_str(&format!("webdav=[{}]
", items.join(",")));
        }
        for (k, v) in &self.other {
            out.push_str(&format!("{k}={v}\n"));
        }
        fs::write(path, out)
    }

    /// Remember `path` in the list of known files (and as the current one).
#[allow(dead_code)]
    pub fn remember(&mut self, path: &std::path::Path) {
        self.file = Some(path.to_path_buf());
        if !self.files.iter().any(|f| f == path) {
            self.files.push(path.to_path_buf());
        }
    }
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.to_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Re-read the config file, change it and write it back, so settings changed meanwhile by the
/// desktop app (autosave, line numbers, file list) are not lost.
pub fn update(f: impl FnOnce(&mut Config)) {
    let mut cfg = Config::load();
    f(&mut cfg);
    let _ = cfg.save(); // settings are a convenience, never fatal
}

fn json_string(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// Minimal parser for a JSON array of strings (`["a","b\\c"]`); anything else gives an empty list.
fn parse_json_strings(v: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = v.trim().chars();
    if chars.next() != Some('[') {
        return out;
    }
    let mut cur: Option<String> = None;
    while let Some(c) = chars.next() {
        match (&mut cur, c) {
            (None, '"') => cur = Some(String::new()),
            (None, ']') => break,
            (None, _) => {}
            (Some(s), '\\') => match chars.next() {
                Some('n') => s.push('\n'),
                Some('t') => s.push('\t'),
                Some(o) => s.push(o),
                None => break,
            },
            (Some(_), '"') => out.push(cur.take().unwrap()),
            (Some(s), c) => s.push(c),
        }
    }
    out.retain(|s| !s.is_empty());
    out
}

/// Create the config file if it does not exist yet, so that it can be shown or opened.
#[allow(dead_code)]
pub fn ensure_exists() -> std::io::Result<PathBuf> {
    let path = config_path();
    if !path.exists() {
        Config::load().save()?;
    }
    Ok(path)
}

/// Open `path` in the default editor / associated program of the platform.
#[allow(dead_code)]
pub fn open_in_editor(path: &std::path::Path) -> std::io::Result<()> {
    use std::process::Command;
    let status = if cfg!(windows) {
        // `start` is a cmd built-in; the empty string is the window title argument
        Command::new("cmd").args(["/C", "start", ""]).arg(path).status()?
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(path).status()?
    } else {
        Command::new("xdg-open").arg(path).status()?
    };
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::new(std::io::ErrorKind::Other, format!("could not open {}", path.display())))
    }
}

/// The file to use when nothing was configured yet.
pub fn default_todo_file() -> PathBuf {
    home_dir().join("todo.txt")
}
