//! `todo -F`: switch the todo file in use (local file, folder, WebDAV URL) and remember it in `todo.cfg`.
//!
//!   todo -F            menu of the known files (arrows, Enter = use, Del = forget, Esc = cancel)
//!   todo -F 2          the second entry of that list
//!   todo -F work       the only entry whose path contains "work"
//!   todo -F PATH|URL   same as -f, but also added to the list
//!
//! The menu also lists the files of the current folder, the configured WebDAV servers (browse them,
//! pick or create a .txt) and "other…" to type a path, folder or URL.

use crate::config::{self, Config};
use crate::{editor, folder, ops, webdav};
use ratatui::crossterm::{
    cursor::{Hide, MoveTo, Show},
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, queue,
    style::{Attribute, Print, SetAttribute},
    terminal::{self, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::error::Error;
use std::io::{self, IsTerminal, Write};
use std::path::{Path, PathBuf};

#[derive(Clone)]
enum Target {
    /// A todo file; the folder is set when it lies in the current folder (folder mode stays on).
    File(PathBuf, Option<PathBuf>),
    /// Browse this WebDAV server (`USER@URL`).
    Server(String),
    /// Type a path, folder or URL.
    Other,
}

struct Item {
    label: String,
    target: Target,
    current: bool,
}

pub enum Pick {
    Item(usize),
    Remove(usize),
    Cancel,
}

fn build(cfg: &Config, current: &Path) -> Vec<Item> {
    let mut items: Vec<Item> = Vec::new();
    let folder = cfg.folder.clone().filter(|d| d.is_dir());
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut push_file = |items: &mut Vec<Item>, p: PathBuf, label: String| {
        if seen.contains(&p) {
            return;
        }
        let fold = folder.clone().filter(|d| p.parent() == Some(d.as_path()));
        items.push(Item { label, current: p == current, target: Target::File(p.clone(), fold) });
        seen.push(p);
    };
    push_file(&mut items, current.to_path_buf(), current.display().to_string());
    if let Some(d) = &folder {
        for p in folder::list_txt_files(d) {
            let l = p.display().to_string();
            push_file(&mut items, p, l);
        }
    }
    for p in &cfg.files {
        push_file(&mut items, p.clone(), p.display().to_string());
    }
    for s in &cfg.webdav {
        items.push(Item { label: format!("WebDAV: {s}  (browse…)"), target: Target::Server(s.clone()), current: false });
    }
    items.push(Item { label: "other path, folder or URL…".into(), target: Target::Other, current: false });
    items
}

/// Full-screen list. Up/Down/PgUp/PgDn/Home/End move, Enter picks, Del/x forgets, Esc/q cancels.
pub fn menu(title: &str, hint: &str, items: &[String], marks: &[bool], start: usize) -> io::Result<Pick> {
    terminal::enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, Hide)?;
    let res = menu_loop(&mut out, title, hint, items, marks, start);
    let _ = execute!(out, Show, LeaveAlternateScreen);
    terminal::disable_raw_mode()?;
    res
}

fn menu_loop(out: &mut io::Stdout, title: &str, hint: &str, items: &[String], marks: &[bool], start: usize) -> io::Result<Pick> {
    let mut sel = start.min(items.len().saturating_sub(1));
    let mut top = 0usize;
    loop {
        let (cols, rows) = terminal::size().unwrap_or((80, 24));
        let room = (rows as usize).saturating_sub(3).max(1);
        if sel < top {
            top = sel;
        } else if sel >= top + room {
            top = sel + 1 - room;
        }
        queue!(out, Clear(ClearType::All), MoveTo(0, 0), SetAttribute(Attribute::Bold), Print(cut(title, cols)), SetAttribute(Attribute::Reset))?;
        for (r, i) in (top..items.len().min(top + room)).enumerate() {
            let mark = if marks.get(i).copied().unwrap_or(false) { '✓' } else { ' ' };
            let line = cut(&format!("{} {mark} {}", if i == sel { '>' } else { ' ' }, items[i]), cols);
            queue!(out, MoveTo(0, r as u16 + 1))?;
            if i == sel {
                queue!(out, SetAttribute(Attribute::Reverse), Print(line), SetAttribute(Attribute::Reset))?;
            } else {
                queue!(out, Print(line))?;
            }
        }
        queue!(out, MoveTo(0, rows.saturating_sub(1)), SetAttribute(Attribute::Dim), Print(cut(hint, cols)), SetAttribute(Attribute::Reset))?;
        out.flush()?;
        let Event::Key(k) = event::read()? else { continue };
        if k.kind == KeyEventKind::Release {
            continue;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        match k.code {
            KeyCode::Enter => return Ok(Pick::Item(sel)),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(Pick::Cancel),
            KeyCode::Char('c') if ctrl => return Ok(Pick::Cancel),
            KeyCode::Delete | KeyCode::Char('x') if !items.is_empty() => return Ok(Pick::Remove(sel)),
            KeyCode::Up | KeyCode::Char('k') => sel = sel.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => sel = (sel + 1).min(items.len().saturating_sub(1)),
            KeyCode::PageUp => sel = sel.saturating_sub(room),
            KeyCode::PageDown => sel = (sel + room).min(items.len().saturating_sub(1)),
            KeyCode::Home => sel = 0,
            KeyCode::End => sel = items.len().saturating_sub(1),
            _ => {}
        }
    }
}

/// Parent directory / URL folder of a path, for comparing where files live (case-insensitive, `/` and `\` alike).
pub fn dir_of(p: &str) -> String {
    let p = p.replace('\\', "/");
    p.rsplit_once('/').map_or(String::new(), |(d, _)| d.to_lowercase())
}

fn cut(s: &str, cols: u16) -> String {
    s.chars().take((cols as usize).saturating_sub(1)).collect()
}

fn prompt(p: &str) -> Result<Option<String>, Box<dyn Error>> {
    Ok(editor::edit_line(p, "", ops::use_color(), None)?.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Browse a WebDAV server: folders to enter, `.txt` files to pick, a new file to create. Returns the file URL.
fn browse(cfg: &Config, entry: &str) -> Result<Option<String>, Box<dyn Error>> {
    let server = webdav::Server::parse(entry).ok_or("bad WebDAV entry in todo.cfg")?;
    let mut url = server.url.clone();
    let same = |a: &str, b: &str| a.trim_end_matches('/') == b.trim_end_matches('/');
    let _ = cfg;
    loop {
        let list = webdav::list(&server, &url).map_err(|e| e.to_string())?;
        let mut labels: Vec<String> = Vec::new();
        let mut acts: Vec<Option<(String, bool)>> = Vec::new(); // (url, is_dir); None = new file
        if !same(&url, &server.url) {
            labels.push("../".into());
            acts.push(Some((webdav::parent(&url), true)));
        }
        let mut dirs: Vec<&webdav::Entry> = list.iter().filter(|e| e.is_dir && !same(&e.url, &url)).collect();
        dirs.sort_by_key(|e| webdav::file_name(&e.url).to_lowercase());
        for e in dirs {
            labels.push(format!("{}/", webdav::file_name(&e.url)));
            acts.push(Some((e.url.clone(), true)));
        }
        let mut files: Vec<&webdav::Entry> = list.iter().filter(|e| !e.is_dir && folder::is_txt(&webdav::file_name(&e.url))).collect();
        files.sort_by_key(|e| webdav::file_name(&e.url).to_lowercase());
        for e in files {
            labels.push(webdav::file_name(&e.url));
            acts.push(Some((e.url.clone(), false)));
        }
        labels.push("new file here…".into());
        acts.push(None);
        let marks = vec![false; labels.len()];
        match menu(&format!("{}  {}", server.entry(), url), "Enter open / use · Esc back", &labels, &marks, 0)? {
            Pick::Item(i) => match &acts[i] {
                Some((u, true)) => url = u.clone(),
                Some((u, false)) => return Ok(Some(u.clone())),
                None => {
                    if let Some(mut n) = prompt("file name> ")? {
                        if !folder::is_txt(&n) {
                            n.push_str(".txt");
                        }
                        return Ok(Some(webdav::join(&url, &n)));
                    }
                }
            },
            Pick::Remove(_) => {}
            Pick::Cancel => {
                if same(&url, &server.url) {
                    return Ok(None);
                }
                url = webdav::parent(&url);
            }
        }
    }
}

/// A typed path, folder or URL -> (file, folder).
fn resolve_path(text: &str, cfg: &Config) -> Result<(PathBuf, Option<PathBuf>), Box<dyn Error>> {
    if webdav::is_remote(text) {
        return Ok((PathBuf::from(text), None));
    }
    let p = std::path::absolute(text)?;
    if p.is_dir() {
        Ok((folder::pick_file(&p, cfg.file.as_deref()), Some(p)))
    } else {
        Ok((p, None))
    }
}

fn apply(path: PathBuf, fold: Option<PathBuf>) {
    let p = path.clone();
    config::update(|c| {
        c.file = Some(p.clone());
        c.folder = fold;
        if c.folder.is_none() && !c.files.contains(&p) {
            c.files.push(p);
        }
    });
    println!("Now using: {}", path.display());
}

/// Entry point. `arg` is the value of `-F` (empty = menu).
pub fn run(arg: &str, current: &Path) -> Result<(), Box<dyn Error>> {
    let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
    let mut start = 0;
    loop {
        let cfg = Config::load();
        let items = build(&cfg, current);

        if !arg.is_empty() {
            // direct selection: number, path / URL, or a unique part of a listed path
            if let Ok(n) = arg.parse::<usize>() {
                return match items.get(n.wrapping_sub(1)).map(|i| i.target.clone()) {
                    Some(Target::File(p, f)) => Ok(apply(p, f)),
                    _ => Err(format!("no file number {n} (todo -F shows the list)").into()),
                };
            }
            if webdav::is_remote(arg) || Path::new(arg).exists() {
                let (p, f) = resolve_path(arg, &cfg)?;
                return Ok(apply(p, f));
            }
            let low = arg.to_lowercase();
            let hits: Vec<&Item> = items.iter().filter(|i| matches!(i.target, Target::File(..)) && i.label.to_lowercase().contains(&low)).collect();
            return match hits.as_slice() {
                [one] => match &one.target {
                    Target::File(p, f) => Ok(apply(p.clone(), f.clone())),
                    _ => unreachable!(),
                },
                [] => Err(format!("no known file matches '{arg}'").into()),
                _ => Err(format!("'{arg}' matches {} files, be more specific", hits.len()).into()),
            };
        }

        if !interactive {
            for (n, i) in items.iter().enumerate() {
                if matches!(i.target, Target::File(..)) {
                    println!("{:>2}{} {}", n + 1, if i.current { '*' } else { ' ' }, i.label);
                }
            }
            return Ok(());
        }

        let labels: Vec<String> = items.iter().map(|i| i.label.clone()).collect();
        let marks: Vec<bool> = items.iter().map(|i| i.current).collect();
        let cur = items.iter().position(|i| i.current).unwrap_or(0);
        match menu("Use which todo file?", "↑↓ select · Enter use · Del forget · Esc cancel", &labels, &marks, start.max(cur))? {
            Pick::Cancel => {
                println!("Unchanged.");
                return Ok(());
            }
            Pick::Remove(i) => {
                if let Target::File(p, _) = &items[i].target {
                    if items[i].current {
                        println!("The file in use cannot be forgotten.");
                    }
                    let p = p.clone();
                    config::update(|c| c.files.retain(|f| *f != p));
                }
                start = i.saturating_sub(1);
            }
            Pick::Item(i) => match items[i].target.clone() {
                Target::File(p, f) => return Ok(apply(p, f)),
                Target::Server(s) => match browse(&cfg, &s) {
                    Ok(Some(url)) => return Ok(apply(PathBuf::from(url), None)),
                    Ok(None) => start = i,
                    Err(e) => {
                        eprintln!("todo: {e}");
                        return Err(e);
                    }
                },
                Target::Other => {
                    if let Some(t) = prompt("path, folder or URL> ")? {
                        let (p, f) = resolve_path(&t, &cfg)?;
                        return Ok(apply(p, f));
                    }
                }
            },
        }
    }
}
