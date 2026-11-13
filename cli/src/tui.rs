//! Interactive terminal UI: the same layout and features as the web app.
//!
//!  - sidebar with tag / context / priority filters, task list, one bar at the bottom for
//!    adding (default), searching (`/`) and editing a task
//!  - the bar highlights priority, dates, +tags, @contexts and due:, completes tags / contexts /
//!    priorities, shows a calendar while a date is typed and converts `due:tomorrow`, `due:+3d`,
//!    `due:24.12.` ... to a real date
//!  - every change is written to the file immediately; Ctrl+Z undoes it

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use chrono::{Datelike, Duration as Days, NaiveDate};
use ratatui::{
    crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers},
    layout::{Constraint, Layout, Position, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph},
    DefaultTerminal, Frame,
};
use unicode_width::UnicodeWidthStr;

use crate::config;
use crate::folder;
use crate::git;
use crate::highlight::{highlight, Tok};
use crate::input::{Edited, LineInput};
use crate::list::{self, Filter, Sort};
use crate::store::Store;
use crate::todotxt::{self, DueStatus};

const MONTHS: [&str; 12] = [
    "January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November",
    "December",
];

fn pri_color(c: char) -> Color {
    match c {
        'A' => Color::Red,
        'B' => Color::Indexed(208),
        'C' => Color::Yellow,
        'D' => Color::Green,
        _ => Color::Gray,
    }
}

fn tok_style(tok: Tok) -> Style {
    match tok {
        Tok::Plain => Style::default(),
        Tok::Priority('\0') => Style::default().fg(Color::Gray),
        Tok::Priority(c) => Style::default().fg(pri_color(c)).add_modifier(Modifier::BOLD),
        Tok::Done => Style::default().fg(Color::Green),
        Tok::Date | Tok::Meta => Style::default().fg(Color::Gray),
        Tok::Code => Style::default().fg(Color::Cyan),
        Tok::Project => Style::default().fg(Color::LightBlue),
        Tok::Context => Style::default().fg(Color::Green),
        Tok::Due => Style::default().fg(Color::Yellow),
    }
}

fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Focus {
    List,
    Sidebar,
    Input,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Add,
    Search,
    /// Editing the line with this index.
    Edit(usize),
}

const GIT_ROWS: [(&str, &str); 7] = [
    ("enabled", "Git support"),
    ("auto", "Git auto (pull on start, commit + push changes)"),
    ("pull", "  Auto pull"),
    ("commit", "  Auto commit"),
    ("push", "  Auto push"),
    ("sync", "Git sync now (F9)"),
    ("init", "Initialize git here..."),
];

struct Item {
    value: String,
    tok: Tok,
    count: usize,
}

enum Assist {
    /// `exact`: the typed word is itself an existing value (then Space / Enter must not complete it).
    List { start: usize, end: usize, items: Vec<Item>, idx: usize, exact: bool },
    Cal { start: usize, end: usize, month: NaiveDate, cursor: NaiveDate, selected: Option<NaiveDate> },
}

#[derive(Clone)]
enum Side {
    Project(String),
    Context(String),
    Priority(char),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PKind {
    Up,
    Dir,
    File,
}

struct PEntry {
    name: String,
    kind: PKind,
}

/// The file selector (F8): browse folders, open a todo.txt, or use a whole folder.
struct Picker {
    dir: PathBuf,
    entries: Vec<PEntry>,
    state: ListState,
}

impl Picker {
    fn new(dir: PathBuf, select: Option<&str>) -> Picker {
        let mut entries = Vec::new();
        if folder::parent_of(&dir).is_some() {
            entries.push(PEntry { name: "..".into(), kind: PKind::Up });
        }
        let mut dirs: Vec<String> = Vec::new();
        if crate::webdav::is_remote(&dir.to_string_lossy()) {
            // a folder on a WebDAV server
            let url = dir.to_string_lossy().into_owned();
            if let Ok(server) = crate::store::server_of(&dir) {
                if let Ok(list) = crate::webdav::list(&server, &url) {
                    let same = |a: &str| a.trim_end_matches('/') == url.trim_end_matches('/');
                    dirs = list
                        .iter()
                        .filter(|e| e.is_dir && !same(&e.url))
                        .map(|e| crate::webdav::file_name(&e.url))
                        .filter(|n| !n.starts_with('.'))
                        .collect();
                }
            }
        } else if let Ok(rd) = fs::read_dir(&dir) {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if let Ok(meta) = e.metadata() {
                    if meta.is_dir() && !folder::is_hidden(&name, &meta) {
                        dirs.push(name);
                    }
                }
            }
        }
        dirs.sort_by_key(|n| n.to_lowercase());
        entries.extend(dirs.into_iter().map(|name| PEntry { name, kind: PKind::Dir }));
        for p in folder::list_txt_files(&dir) {
            let n = if crate::webdav::is_remote(&p.to_string_lossy()) {
                Some(crate::webdav::file_name(&p.to_string_lossy()))
            } else {
                p.file_name().map(|n| n.to_string_lossy().into_owned())
            };
            if let Some(n) = n {
                entries.push(PEntry { name: n, kind: PKind::File });
            }
        }
        let idx = select.and_then(|n| entries.iter().position(|e| e.name == n)).unwrap_or(0);
        let mut state = ListState::default();
        state.select(Some(idx));
        Picker { dir, entries, state }
    }

    fn idx(&self) -> usize {
        self.state.selected().unwrap_or(0)
    }

    fn move_by(&mut self, d: isize) {
        let n = self.entries.len() as isize;
        if n > 0 {
            self.state.select(Some((self.idx() as isize + d).clamp(0, n - 1) as usize));
        }
    }
}

struct App {
    path: PathBuf,
    /// Folder mode: the folder whose *.txt files can be switched between (F8).
    folder: Option<PathBuf>,
    /// Git status of the folder for the status bar (refreshed every 2 s).
    git_status: Option<git::Status>,
    git_checked: Instant,
    /// File content as last read or written, to notice changes made by other programs.
    disk_text: String,
    last_check: Instant,
    /// When `todo.cfg` was last looked at for a file switch made by the desktop app / CLI.
    cfg_checked: Instant,
    picker: Option<Picker>,
    /// F10 git settings popup: selected row.
    git_menu: Option<usize>,
    /// "Changed on the server" popup (WebDAV): removed lines, added lines, scroll.
    server_diff: Option<(Vec<String>, Vec<String>, usize, bool)>,
    /// Note being edited in the popup (F7, or a `note:` typed into the bar).
    note_edit: Option<NoteEdit>,
    /// Selected parent entry (Ctrl+P): line index and its text (indices move when the file changes).
    parent: Option<(usize, String)>,
    /// Ctrl+O: destination prompt for moving the filtered tasks to another file.
    move_dlg: Option<LineInput>,
    /// Name of the note being written for a `note:` typed into the bar (finishes the word afterwards).
    store: Store,
    undo: Vec<Vec<String>>,
    sort: Sort,
    show_done: bool,
    show_sidebar: bool,
    calendar: bool,
    /// Status line at the bottom (Ctrl+L).
    show_status: bool,
    filter: Filter,
    view: Vec<usize>,
    sel_pos: usize,
    sel_line: Option<usize>,
    list_state: ListState,
    side_idx: usize,
    focus: Focus,
    mode: Mode,
    input: LineInput,
    add_draft: String,
    assist: Option<Assist>,
    dismissed: Option<(usize, String)>,
    msg: Option<String>,
    help: bool,
    quit: bool,
}

pub fn run(path: PathBuf, folder: Option<PathBuf>, sort: Sort) -> io::Result<()> {
    crate::davcache::set_interactive(); // fast WebDAV cache mode (davcache= in todo.cfg)
    let store = Store::load(&path)?;
    let cfg = config::Config::load();
    let disk_text = crate::store::read_text(&path).map(|r| r.0).unwrap_or_default();
    let mut app = App {
        path,
        folder,
        git_status: None,
        git_checked: Instant::now() - Duration::from_secs(60),
        disk_text,
        last_check: Instant::now(),
        cfg_checked: Instant::now(),
        picker: None,
        git_menu: None,
        server_diff: None,
        note_edit: None,
        parent: None,
        move_dlg: None,
        store,
        undo: Vec::new(),
        sort,
        show_done: cfg.show_done.unwrap_or(true),
        show_sidebar: cfg.sidebar.unwrap_or(true),
        calendar: cfg.calendar.unwrap_or(true),
        show_status: cfg.statusbar.unwrap_or(true),
        filter: Filter::default(),
        view: Vec::new(),
        sel_pos: 0,
        sel_line: None,
        list_state: ListState::default(),
        side_idx: 0,
        focus: Focus::Input,
        mode: Mode::Add,
        input: LineInput::new(""),
        add_draft: String::new(),
        assist: None,
        dismissed: None,
        msg: None,
        help: false,
        quit: false,
    };
    app.reload_view();
    crate::store::allow_prompt(false);   // the terminal belongs to the UI now
    let mut terminal = ratatui::init();
    let res = app.event_loop(&mut terminal);
    ratatui::restore();
    crate::davcache::flush(); // push what the background worker has not sent yet
    res
}

impl App {
    fn event_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            if event::poll(Duration::from_millis(500))? {
                if let Event::Key(k) = event::read()? {
                    if k.kind != KeyEventKind::Release {
                        self.on_key(k);
                    }
                }
            }
            if let Some(m) = git::take_result() {
                self.msg = Some(m); // outcome of a background push
            }
            if self.git_checked.elapsed() >= Duration::from_secs(2) {
                self.refresh_git_status();
            }
            if self.cfg_checked.elapsed() >= Duration::from_secs(1) {
                self.follow_lastfile();
            }
            if self.last_check.elapsed() >= Duration::from_secs(if crate::webdav::is_remote(&self.path.to_string_lossy()) && !crate::davcache::active() { 8 } else { 1 }) {
                self.check_external();
            }
        }
        Ok(())
    }

    fn refresh_git_status(&mut self) {
        self.git_checked = Instant::now();
        self.git_status = self.folder.as_deref().and_then(git::status);
    }

    /// Reload the file when another program changed it (the TUI saves every change at once, so
    /// there is never unsaved work). The reload is an undoable step.
    fn check_external(&mut self) {
        self.last_check = Instant::now();
        if matches!(self.mode, Mode::Edit(_)) {
            return;
        }
        let Ok((text, _)) = crate::store::read_text(&self.path) else { return };
        if text == self.disk_text {
            return;
        }
        self.disk_text = text;
        if let Ok(new) = Store::load(&self.path) {
            if new.lines != self.store.lines {
                let old = std::mem::replace(&mut self.store, new);
                // Another app's change is applied silently (no diff popup); the status message tells it.
                self.undo.push(old.lines);
                if self.undo.len() > 200 {
                    self.undo.remove(0);
                }
                self.reload_view();
                self.msg = Some("File changed on disk: reloaded (Ctrl+Z undoes)".into());
            }
        }
    }

    /// Remember the view settings (sort, completed, sidebar, calendar) in the config file.
    fn save_view(&self) {
        let (sort, done, side, cal, status) = (self.sort, self.show_done, self.show_sidebar, self.calendar, self.show_status);
        config::update(|c| {
            c.sort = Some(sort.name().to_string());
            c.show_done = Some(done);
            c.sidebar = Some(side);
            c.calendar = Some(cal);
            c.statusbar = Some(status);
        });
    }

    /// Open another todo.txt (`folder` = Some(dir) keeps / enters folder mode).
    /// The desktop app (or `todo -F`) switched to another *known* file: follow it. A file from a folder unknown
    /// to the config (opened one-off) is never written to `lastfile=`, so it cannot pull this app away.
    fn follow_lastfile(&mut self) {
        self.cfg_checked = Instant::now();
        if matches!(self.mode, Mode::Edit(_)) || self.picker.is_some() || self.note_edit.is_some() || self.git_menu.is_some() {
            return;
        }
        let cfg = config::Config::load();
        let Some(last) = cfg.lastfile.clone() else { return };
        let norm = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
        if norm(&last) == norm(&self.path) {
            return;
        }
        let in_folder = cfg.folder.as_ref().is_some_and(|d| folder::is_inside(d, &last));
        let known = in_folder
            || cfg.file.as_ref().is_some_and(|f| norm(f) == norm(&last))
            || cfg.files.iter().any(|f| norm(f) == norm(&last));
        if !known {
            return;
        }
        let folder = if in_folder { cfg.folder.clone() } else { self.folder.clone().filter(|d| folder::is_inside(d, &last)) };
        self.switch_file(last, folder, false);
    }

    fn switch_file(&mut self, path: PathBuf, folder: Option<PathBuf>, remember: bool) {
        // entering another folder: pull first (same as the desktop app), before the data is read
        if let Some(d) = folder.as_ref().filter(|d| self.folder.as_ref() != Some(*d)) {
            match git::auto_start(d, false) {
                Ok(m) if !m.is_empty() => git::post(&m),
                Err(e) => git::post(&format!("git: {e}")),
                _ => {}
            }
        }
        match Store::load(&path) {
            Ok(store) => {
                self.disk_text = crate::store::read_text(&path).map(|r| r.0).unwrap_or_default();
                self.store = store;
                self.undo.clear();
                self.filter.clear();
                self.sel_line = None;
                self.sel_pos = 0;
                self.mode = Mode::Add;
                self.reload_view();
                let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
                if remember {
                    let (p2, f2) = (path.clone(), folder.clone());
                    config::update(|c| {
                        c.lastfile = Some(p2.clone());
                        c.file = Some(p2);
                        c.folder = f2;
                    });
                }
                self.path = path;
                self.folder = folder;
                self.msg = Some(format!("Opened {name}"));
            }
            Err(e) => self.msg = Some(format!("Could not open {}: {e}", path.display())),
        }
    }

    /// F7: edit the note of the selected task in the editor (adds `note:NAME` if missing).
    fn edit_note(&mut self) {
        if matches!(self.mode, Mode::Edit(_)) {
            return;
        }
        let Some(i) = self.sel_line else { return };
        let t = todotxt::parse(&self.store.lines[i]);
        let name = match folder::note_name(&t) {
            Some(n) => n,
            None => {
                let n = folder::next_note_name(&self.store.lines, self.folder.as_deref());
                let raw = format!("{} note:{n}", self.store.lines[i]);
                self.replace_line(i, raw);
                n
            }
        };
        self.open_note_edit(&name, false);
    }

    /// Open the note popup for `name`; `word` = it was started by typing `note:` into the bar.
    fn open_note_edit(&mut self, name: &str, word: bool) {
        match folder::prepare_note(&self.path, self.folder.as_deref(), name) {
            Ok(n) => {
                let text = n.read();
                self.assist = None;
                self.note_edit = Some(NoteEdit { n, name: name.to_string(), word, ta: crate::textarea::TextArea::new(&text) });
            }
            Err(e) => self.msg = Some(format!("Could not open note: {e}")),
        }
    }

    fn note_key(&mut self, k: KeyEvent) {
        use crate::textarea::Outcome;
        let Some(ne) = &mut self.note_edit else { return };
        let outcome = ne.ta.handle_key(k);
        if outcome == Outcome::Continue {
            return;
        }
        let Some(ne) = self.note_edit.take() else { return };
        let save = outcome == Outcome::Save;
        if save {
            if let Err(e) = ne.n.write(&ne.ta.text()) {
                self.msg = Some(format!("Could not write note: {e}"));
                return;
            }
        }
        let has = folder::note_nonempty(&ne.n);
        let res = folder::finish_note(&ne.n);
        if ne.word {
            // `note:` typed in the bar: it becomes note:NAME, or disappears if the note is empty / cancelled
            self.input.set_note_word(if save && has { Some(&ne.name) } else { None });
            self.on_input_changed();
        }
        self.msg = Some(match res {
            Ok(()) if save => {
                self.git_commit();
                format!("Note saved: {}", ne.name)
            }
            Ok(()) => "Note cancelled".to_string(),
            Err(e) => format!("Note failed: {e}"),
        });
    }

    fn draw_note_edit(&self, f: &mut Frame, area: Rect) {
        let Some(ne) = &self.note_edit else { return };
        let w = area.width.saturating_sub(6).clamp(20, 100).min(area.width);
        let h = area.height.saturating_sub(4).clamp(6, 30).min(area.height);
        let rect = Rect::new(area.x + area.width.saturating_sub(w) / 2, area.y + area.height.saturating_sub(h) / 2, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" note:{} ", ne.name))
            .title_bottom(" Enter new line · Ctrl+Enter / Ctrl+S save · Esc cancel ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let rows = inner.height as usize;
        let first = (ne.ta.row + 1).saturating_sub(rows);
        let lines: Vec<Line> = ne.ta.lines.iter().skip(first).take(rows).map(|l| Line::from(l.as_str())).collect();
        f.render_widget(Paragraph::new(lines), inner);
        let before: String = ne.ta.lines[ne.ta.row].chars().take(ne.ta.col).collect();
        let x = (inner.x + before.width() as u16).min(inner.x + inner.width.saturating_sub(1));
        f.set_cursor_position(Position::new(x, inner.y + (ne.ta.row - first) as u16));
    }

    // ---------------------------------------------------------------- file selector

    fn open_picker(&mut self) {
        if matches!(self.mode, Mode::Edit(_)) {
            return;
        }
        if self.mode == Mode::Search {
            self.leave_search();
        }
        self.assist = None;
        let dir = self
            .folder
            .clone()
            .or_else(|| folder::parent_of(&self.path))
            .unwrap_or_else(|| PathBuf::from("."));
        let cur = Some(if crate::webdav::is_remote(&self.path.to_string_lossy()) {
            crate::webdav::file_name(&self.path.to_string_lossy())
        } else {
            self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
        });
        self.picker = Some(Picker::new(dir, cur.as_deref()));
    }

    fn picker_key(&mut self, k: KeyEvent) {
        let Some(pk) = &mut self.picker else { return };
        match k.code {
            KeyCode::Esc => self.picker = None,
            KeyCode::Up => pk.move_by(-1),
            KeyCode::Down => pk.move_by(1),
            KeyCode::PageUp => pk.move_by(-10),
            KeyCode::PageDown => pk.move_by(10),
            KeyCode::Home => pk.move_by(isize::MIN / 2),
            KeyCode::End => pk.move_by(isize::MAX / 2),
            KeyCode::Backspace | KeyCode::Left => {
                if let Some(parent) = pk.dir.parent().map(|p| p.to_path_buf()) {
                    let from = pk.dir.file_name().map(|n| n.to_string_lossy().into_owned());
                    *pk = Picker::new(parent, from.as_deref());
                }
            }
            KeyCode::Char('s') | KeyCode::Char('S') => {
                // use the shown directory as the folder (all its *.txt files)
                let dir = pk.dir.clone();
                let remembered = self.path.clone();
                self.picker = None;
                let file = folder::pick_file(&dir, Some(&remembered));
                self.switch_file(file, Some(dir), true);
            }
            KeyCode::Enter | KeyCode::Right => {
                let Some(e) = pk.entries.get(pk.idx()) else { return };
                match e.kind {
                    PKind::Up => {
                        if let Some(parent) = folder::parent_of(&pk.dir) {
                            let from = Some(if crate::webdav::is_remote(&pk.dir.to_string_lossy()) {
                                crate::webdav::file_name(&pk.dir.to_string_lossy())
                            } else {
                                pk.dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
                            });
                            *pk = Picker::new(parent, from.as_deref());
                        }
                    }
                    PKind::Dir => {
                        let dir = if crate::webdav::is_remote(&pk.dir.to_string_lossy()) {
                            PathBuf::from(format!("{}/", crate::webdav::join(&pk.dir.to_string_lossy(), &e.name).trim_end_matches('/')))
                        } else {
                            pk.dir.join(&e.name)
                        };
                        *pk = Picker::new(dir, None);
                    }
                    PKind::File if k.code == KeyCode::Enter => {
                        let path = folder::join(&pk.dir, &e.name);
                        // staying in the folder keeps folder mode; a file elsewhere leaves it
                        let folder = if self.folder.as_deref() == Some(pk.dir.as_path()) { self.folder.clone() } else { None };
                        self.picker = None;
                        self.switch_file(path, folder, true);
                    }
                    PKind::File => {}
                }
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- model

    fn today(&self) -> NaiveDate {
        todotxt::today()
    }

    /// Current line index of the selected parent (follows it when lines moved; None when it is gone).
    fn parent_line(&self) -> Option<usize> {
        let (i, raw) = self.parent.as_ref()?;
        if self.store.lines.get(*i) == Some(raw) {
            Some(*i)
        } else {
            self.store.lines.iter().position(|l| l == raw)
        }
    }

    /// Ctrl+P: make the selected entry the parent of new entries (they are added below it, one level deeper); again = off.
    fn toggle_parent(&mut self) {
        let cur = self.parent_line();
        match (self.sel_line, cur) {
            (Some(i), c) if c != Some(i) => {
                self.parent = Some((i, self.store.lines[i].clone()));
                self.msg = Some(format!("New entries are added under line {}  (Ctrl+P again: off)", i + 1));
            }
            _ => {
                self.parent = None;
                self.msg = Some("New entries are added at the end".into());
            }
        }
    }

    /// Ctrl+O: ask for a file and move the tasks matching the active filter (and their sub-entries) there.
    fn open_move(&mut self) {
        if matches!(self.mode, Mode::Edit(_)) {
            return;
        }
        if !self.filter.is_active() {
            self.msg = Some("Ctrl+O moves the filtered tasks: select a project/context in the sidebar or search (/) first".into());
            return;
        }
        let n = list::select_matching(&self.store.lines, |l| self.filter.matches_line(l)).len();
        if n == 0 {
            self.msg = Some("No task matches the filter".into());
            return;
        }
        let dir = self.path.parent().map(|p| format!("{}{}", p.display(), std::path::MAIN_SEPARATOR)).unwrap_or_default();
        self.assist = None;
        self.move_dlg = Some(LineInput::new(if crate::webdav::is_remote(&self.path.to_string_lossy()) { "" } else { &dir }));
    }

    fn move_key(&mut self, k: KeyEvent) {
        match k.code {
            KeyCode::Esc => self.move_dlg = None,
            KeyCode::Enter => self.do_move(),
            _ => {
                let today = self.today();
                if let Some(inp) = &mut self.move_dlg {
                    inp.handle_key(k, today);
                }
            }
        }
    }

    fn do_move(&mut self) {
        let Some(inp) = self.move_dlg.take() else { return };
        let text = inp.text.trim().to_string();
        if text.is_empty() {
            self.msg = Some("No destination given".into());
            return;
        }
        let mut dest = PathBuf::from(&text);
        if !crate::webdav::is_remote(&text) && dest.is_relative() {
            dest = self.path.parent().map_or(dest.clone(), |p| p.join(&dest));
        }
        if dest == self.path {
            self.msg = Some("The destination is the open file".into());
            return;
        }
        let idx = list::select_matching(&self.store.lines, |l| self.filter.matches_line(l));
        let moved: Vec<String> = idx.iter().map(|&i| self.store.lines[i].clone()).collect();
        if moved.is_empty() {
            return;
        }
        if let Err(e) = crate::ops::append_lines(&dest, &moved) {
            self.msg = Some(format!("Could not write {}: {e}", dest.display()));
            return;
        }
        let gone: std::collections::HashSet<usize> = idx.into_iter().collect();
        let lines: Vec<String> = self.store.lines.iter().enumerate().filter(|(i, _)| !gone.contains(i)).map(|(_, l)| l.clone()).collect();
        self.commit(lines);
        self.msg = Some(format!("Moved {} line(s) to {}  (Ctrl+Z restores them here, not in the destination)", moved.len(), dest.display()));
    }

    fn reload_view(&mut self) {
        let keep = self.sel_line;
        self.view = list::view(&self.store.lines, self.sort, self.show_done, &self.filter);
        let pos = keep.and_then(|l| self.view.iter().position(|&i| i == l)).unwrap_or(self.sel_pos);
        self.select_pos(pos);
    }

    fn select_pos(&mut self, p: usize) {
        if self.view.is_empty() {
            self.sel_pos = 0;
            self.sel_line = None;
            self.list_state.select(None);
        } else {
            let p = p.min(self.view.len() - 1);
            self.sel_pos = p;
            self.sel_line = Some(self.view[p]);
            self.list_state.select(Some(p));
        }
    }

    fn move_sel(&mut self, delta: isize) {
        let p = (self.sel_pos as isize + delta).clamp(0, self.view.len().saturating_sub(1) as isize);
        self.select_pos(p as usize);
    }

    fn save(&mut self) {
        match self.store.save(&self.path) {
            Ok(()) => {
                self.disk_text = self.store.text();
                if crate::davcache::is_offline() && crate::webdav::is_remote(&self.path.to_string_lossy()) {
                    self.msg = Some("Server not reachable: saved in the offline cache, synced when it is back".into());
                }
                self.git_commit();
                self.refresh_git_status(); // status line shows the new git state at once
            }
            Err(e) => self.msg = Some(format!("Could not save: {e}")),
        }
    }

    /// The git repository of the todo folder, if folder mode is on and the folder is one.
    fn git_dir(&self) -> Option<PathBuf> {
        self.folder.clone().filter(|d| d.join(".git").exists())
    }

    /// After a change: commit and (in the background) push, if `.todo-git.cfg` has git + auto on.
    fn git_commit(&mut self) {
        if let Some(dir) = self.git_dir() {
            let name = self.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            match git::auto_change(&dir, &format!("todo: update {name}"), true, false) {
                Ok(m) if !m.is_empty() => self.msg = Some(m),
                Err(e) => self.msg = Some(format!("git: {e}")),
                _ => {}
            }
        }
    }

    /// F9: commit, pull, push.
    fn git_sync(&mut self) {
        let Some(dir) = self.git_dir() else {
            self.msg = Some("F9 git sync needs folder mode in a git repository (todo -f FOLDER, todo --git-init)".into());
            return;
        };
        self.msg = Some(match git::sync(&dir, false) {
            Ok(s) => format!("git: {s}"),
            Err(e) => format!("git: {e}"),
        });
        self.check_external(); // a pull may have changed the open file
    }

    /// Replace the whole list (one undo step) and write the file.
    fn commit(&mut self, lines: Vec<String>) {
        self.undo.push(std::mem::replace(&mut self.store.lines, lines));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.save();
        self.reload_view();
    }

    fn replace_line(&mut self, i: usize, raw: String) {
        let mut lines = self.store.lines.clone();
        lines[i] = raw;
        self.commit(lines);
    }

    fn undo_list(&mut self) {
        match self.undo.pop() {
            Some(prev) => {
                self.store.lines = prev;
                self.save();
                self.reload_view();
                self.msg = Some("Undone".into());
            }
            None => self.msg = Some("Nothing to undo".into()),
        }
    }

    fn toggle_done(&mut self) {
        let Some(i) = self.sel_line else { return };
        let raw = self.store.lines[i].clone();
        let new = if todotxt::parse(&raw).done {
            todotxt::reopen(&raw)
        } else {
            todotxt::complete(&raw, &todotxt::iso(self.today()))
        };
        self.replace_line(i, new);
    }

    fn delete_selected(&mut self) {
        let Some(i) = self.sel_line else { return };
        let mut lines = self.store.lines.clone();
        let gone = lines.remove(i);
        self.commit(lines);
        folder::remove_note(&self.path, self.folder.as_deref(), &gone, &self.store.lines);
        self.msg = Some(format!("Deleted: {gone}  (note file removed)"));
    }

    /// Alt+Up / Alt+Down: raise or lower the priority of the selected open task.
    fn shift_priority(&mut self, raise: bool) {
        let Some(i) = self.sel_line else { return };
        let raw = self.store.lines[i].clone();
        let t = todotxt::parse(&raw);
        if t.done {
            return;
        }
        let new = if raise {
            match t.priority {
                None => Some('C'),
                Some('A') => Some('A'),
                Some(c) => Some((c as u8 - 1) as char),
            }
        } else {
            match t.priority {
                Some(c) if c < 'C' => Some((c as u8 + 1) as char),
                _ => None,
            }
        };
        if new != t.priority {
            self.replace_line(i, todotxt::set_priority(&raw, new));
        }
    }

    fn archive_done(&mut self) {
        let (done, open): (Vec<String>, Vec<String>) =
            self.store.lines.iter().cloned().partition(|l| todotxt::parse(l).done);
        if done.is_empty() {
            self.msg = Some("No completed tasks to archive".into());
            return;
        }
        let target = self.path.with_file_name("done.txt");
        let crlf = self.store.crlf();
        let res = OpenOptions::new().create(true).append(true).open(&target).and_then(|mut f| {
            for l in &done {
                write!(f, "{l}{}", if crlf { "\r\n" } else { "\n" })?;
            }
            Ok(())
        });
        match res {
            Ok(()) => {
                let n = done.len();
                self.commit(open);
                self.msg = Some(format!("Archived {n} task(s) to {}", target.display()));
            }
            Err(e) => self.msg = Some(format!("Could not write {}: {e}", target.display())),
        }
    }

    /// Sidebar entries. Each section only counts tasks that match the *other* active sidebar filters,
    /// so with a project selected only the contexts and priorities that exist in it are listed.
    fn side_entries(&self) -> Vec<(Side, usize)> {
        let f = &self.filter;
        let subset = |skip: u8| -> Vec<String> {
            self.store
                .lines
                .iter()
                .filter(|l| !l.trim().is_empty())
                .filter(|l| {
                    let t = todotxt::parse(l);
                    (skip == 0 || f.project.is_empty() || f.project.iter().any(|p| t.projects.contains(p)))
                        && (skip == 1 || f.context.is_empty() || f.context.iter().any(|c| t.contexts.contains(c)))
                        && (skip == 2 || f.priority.is_empty() || t.priority.map_or(false, |p| f.priority.contains(&p)))
                })
                .cloned()
                .collect()
        };
        let mut v = Vec::new();
        for (n, c) in list::counts(&subset(0), true, |t| t.projects.clone()) {
            v.push((Side::Project(n), c));
        }
        for (n, c) in list::counts(&subset(1), true, |t| t.contexts.clone()) {
            v.push((Side::Context(n), c));
        }
        for (n, c) in list::counts(&subset(2), false, |t| t.priority.map(|p| p.to_string()).into_iter().collect()) {
            v.push((Side::Priority(n.chars().next().unwrap_or('A')), c));
        }
        v
    }

    /// `multi` (Space / Ctrl+Enter): add or remove the entry, keeping the others of its section
    /// (like Ctrl+click in the GUI). Otherwise (Enter): only this entry, or none if it is the only one.
    fn toggle_side(&mut self, e: &Side, multi: bool) {
        fn toggle<T: PartialEq + Clone>(slot: &mut Vec<T>, v: T, multi: bool) {
            if multi {
                if let Some(i) = slot.iter().position(|x| *x == v) {
                    slot.remove(i);
                } else {
                    slot.push(v);
                }
            } else if slot.len() == 1 && slot[0] == v {
                slot.clear();
            } else {
                *slot = vec![v];
            }
        }
        match e {
            Side::Project(p) => toggle(&mut self.filter.project, p.clone(), multi),
            Side::Context(c) => toggle(&mut self.filter.context, c.clone(), multi),
            Side::Priority(p) => toggle(&mut self.filter.priority, *p, multi),
        }
        self.reload_view();
    }

    /// Clear the filter of the section the sidebar cursor is in (the `[x]` shown at its title).
    fn clear_side_section(&mut self, e: &Side) {
        match e {
            Side::Project(_) => self.filter.project.clear(),
            Side::Context(_) => self.filter.context.clear(),
            Side::Priority(_) => self.filter.priority.clear(),
        }
        self.reload_view();
    }

    // ---------------------------------------------------------------- modes

    fn enter_search(&mut self) {
        if self.mode == Mode::Add {
            self.add_draft = self.input.text.clone();
        }
        self.mode = Mode::Search;
        let q = self.filter.q.clone();
        self.input.set(&q);
        self.focus = Focus::Input;
        self.assist = None;
    }

    /// Back to adding; the search filter stays active (shown in the status line).
    fn leave_search(&mut self) {
        if self.mode == Mode::Search {
            self.filter.q = self.input.text.clone();
            self.mode = Mode::Add;
            let d = self.add_draft.clone();
            self.input.set(&d);
            self.reload_view();
        }
        self.assist = None;
    }

    fn start_edit(&mut self) {
        let Some(i) = self.sel_line else { return };
        if self.mode == Mode::Add {
            self.add_draft = self.input.text.clone();
        }
        self.mode = Mode::Edit(i);
        let raw = self.store.lines[i].clone();
        self.input.set(&raw);
        self.focus = Focus::Input;
        self.refresh_assist();
    }

    fn finish_edit(&mut self, save: bool) {
        if let Mode::Edit(i) = self.mode {
            if save {
                let v = todotxt::normalize_due(&todotxt::fix_priority_case(self.input.text.trim()), self.today());
                if v.is_empty() {
                    let mut lines = self.store.lines.clone();
                    lines.remove(i);
                    self.commit(lines);
                } else if v != self.store.lines[i] {
                    self.replace_line(i, v);
                }
            }
            self.mode = Mode::Add;
            let d = self.add_draft.clone();
            self.input.set(&d);
        }
        self.assist = None;
        self.focus = Focus::List;
    }

    fn focus_list(&mut self) {
        if self.mode == Mode::Search {
            self.leave_search();
        }
        self.assist = None;
        self.focus = Focus::List;
    }

    fn submit_add(&mut self) {
        let text = self.input.text.trim().to_string();
        if text.is_empty() {
            return;
        }
        let today = self.today();
        let line = todotxt::with_creation_date(
            &todotxt::normalize_due(&todotxt::fix_priority_case(&text), today),
            &todotxt::iso(today),
        );
        let mut lines = self.store.lines.clone();
        let parent = self.parent_line().map(|i| i + 1);
        let new_idx = match crate::ops::insert_task(&mut lines, line.clone(), parent) {
            Ok(n) => n - 1,
            Err(e) => {
                self.msg = Some(e);
                return;
            }
        };
        let line = lines[new_idx].clone();
        self.input.set("");
        self.assist = None;
        self.commit(lines);
        // jump to the new task if it is visible
        if let Some(p) = self.view.iter().position(|&i| i == new_idx) {
            self.select_pos(p);
        }
        self.msg = Some(format!("Added: {line}"));
    }

    // ---------------------------------------------------------------- assist

    fn on_input_changed(&mut self) {
        if self.mode == Mode::Search {
            self.filter.q = self.input.text.clone();
            self.reload_view();
        }
        self.refresh_assist();
    }

    fn refresh_assist(&mut self) {
        if self.focus != Focus::Input {
            self.assist = None;
            return;
        }
        let (start, end, word) = self.input.word_at_cursor();
        if let Some((s, w)) = &self.dismissed {
            if *s == start && *w == word {
                self.assist = None;
                return;
            }
            self.dismissed = None;
        }

        // priorities (first word), tags, contexts
        let first = word.chars().next();
        if matches!(first, Some('+') | Some('@')) || (first == Some('(') && start == 0) {
            let rest: String = word.chars().skip(1).collect();
            let all: Vec<Item> = match first {
                Some('+') => self.words_matching(&rest, |t| t.projects.clone(), true, '+', Tok::Project),
                Some('@') => self.words_matching(&rest, |t| t.contexts.clone(), true, '@', Tok::Context),
                _ => {
                    let prefix = rest.trim_end_matches(')').to_uppercase();
                    list::counts(&self.store.lines, false, |t| t.priority.map(|p| p.to_string()).into_iter().collect())
                        .into_iter()
                        .filter(|(n, _)| n.starts_with(&prefix))
                        .map(|(n, c)| Item {
                            tok: Tok::Priority(n.chars().next().unwrap_or('A')),
                            value: format!("({n})"),
                            count: c,
                        })
                        .collect()
                }
            };
            let exact = all.iter().any(|it| it.value == word);
            let items: Vec<Item> = all.into_iter().filter(|it| it.value != word).take(8).collect();
            if items.is_empty() {
                self.assist = None;
                return;
            }
            let idx = match &self.assist {
                Some(Assist::List { idx, .. }) => (*idx).min(items.len() - 1),
                _ => 0,
            };
            self.assist = Some(Assist::List { start, end, items, idx, exact });
            return;
        }

        // dates: `due`, `due:...` and bare dates outside the leading prefix
        if self.calendar {
            if let Some((selected, year, month)) = self.date_hit(start, &word) {
                let today = self.today();
                let (mut cmonth, mut ccursor) = match &self.assist {
                    Some(Assist::Cal { month, cursor, .. }) => (*month, *cursor),
                    _ => (today.with_day(1).unwrap_or(today), today),
                };
                if let Some(sel) = selected {
                    cmonth = sel.with_day(1).unwrap_or(sel);
                    ccursor = sel;
                } else if year.is_some() || month.is_some() {
                    let y = year.unwrap_or(cmonth.year());
                    let m = month.unwrap_or(cmonth.month());
                    if let Some(d) = NaiveDate::from_ymd_opt(y, m, 1) {
                        cmonth = d;
                        ccursor = d;
                    }
                }
                self.assist = Some(Assist::Cal { start, end, month: cmonth, cursor: ccursor, selected });
                return;
            }
        }
        self.assist = None;
    }

    fn words_matching<F>(&self, prefix: &str, pick: F, include_done: bool, marker: char, tok: Tok) -> Vec<Item>
    where
        F: Fn(&todotxt::Task) -> Vec<String>,
    {
        let prefix = prefix.to_lowercase();
        list::counts(&self.store.lines, include_done, pick)
            .into_iter()
            .filter(|(n, _)| n.to_lowercase().starts_with(&prefix))
            .map(|(n, c)| Item { value: format!("{marker}{n}"), tok, count: c })
            .collect()
    }

    /// (selected date, year, month) when the calendar should show for this word.
    fn date_hit(&self, start: usize, word: &str) -> Option<(Option<NaiveDate>, Option<i32>, Option<u32>)> {
        let today = self.today();
        if word == "due" || word.starts_with("due:") {
            let expr = &word[3.min(word.len())..].trim_start_matches(':');
            if let Some(d) = todotxt::parse_date_expr(expr, today, false) {
                return Some((Some(d), None, None));
            }
            // partially typed ISO date: 2026 / 2026-1 / 2026-12-
            let mut year = None;
            let mut month = None;
            let mut parts = expr.split('-');
            if let Some(y) = parts.next() {
                if y.len() == 4 {
                    year = y.parse().ok().filter(|y| *y >= 1000);
                }
            }
            if let Some(m) = parts.next() {
                month = m.parse::<u32>().ok().filter(|m| (1..=12).contains(m));
            }
            return Some((None, year, month));
        }
        // bare dates, but not the leading "x" / priority / creation-date prefix
        let text = &self.input.text;
        let mut prefix_len = 0;
        let mut rest = text.as_str();
        if let Some(r) = rest.strip_prefix("x ") {
            prefix_len += 2;
            rest = r;
        } else if rest.len() >= 4 && rest.starts_with('(') && rest.as_bytes()[2] == b')' && rest.as_bytes()[3] == b' ' {
            prefix_len += 4;
            rest = &rest[4..];
        }
        for _ in 0..2 {
            let w: String = rest.chars().take(11).collect();
            if w.chars().count() == 11 && w.ends_with(' ') && todotxt::parse_date_expr(w.trim_end(), today, true).is_some()
                && w.trim_end().len() == 10
            {
                prefix_len += 11;
                rest = &rest[11..];
            }
        }
        if start >= prefix_len {
            if let Some(d) = todotxt::parse_date_expr(word, today, true) {
                return Some((Some(d), None, None));
            }
        }
        None
    }

    fn accept_assist(&mut self) -> bool {
        match self.assist.take() {
            Some(Assist::List { start, end, items, idx, .. }) => {
                self.input.replace_range(start, end, &items[idx].value);
                self.on_input_changed();
                true
            }
            Some(Assist::Cal { start, end, cursor, .. }) => {
                self.input.replace_range(start, end, &format!("due:{}", todotxt::iso(cursor)));
                self.on_input_changed();
                true
            }
            None => false,
        }
    }

    fn cal_move(&mut self, days: i64, months: i32) -> bool {
        if let Some(Assist::Cal { month, cursor, selected, .. }) = &mut self.assist {
            let mut c = *cursor;
            if months != 0 {
                let total = c.year() * 12 + c.month0() as i32 + months;
                c = NaiveDate::from_ymd_opt(total.div_euclid(12), total.rem_euclid(12) as u32 + 1, 1).unwrap_or(c);
            } else if let Some(n) = c.checked_add_signed(Days::days(days)) {
                c = n;
            }
            *cursor = c;
            *month = c.with_day(1).unwrap_or(c);
            *selected = None;
            return true;
        }
        false
    }

    // ---------------------------------------------------------------- keys

    fn on_key(&mut self, k: KeyEvent) {
        self.msg = None;
        if let Some((rem, add, scroll, _)) = self.server_diff.as_mut() {
            let max = (rem.len() + add.len()).saturating_sub(1);
            match k.code {
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::Down => *scroll = (*scroll + 1).min(max),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::PageDown => *scroll = (*scroll + 10).min(max),
                KeyCode::Home => *scroll = 0,
                KeyCode::End => *scroll = max,
                _ => self.server_diff = None,
            }
            return;
        }
        if self.note_edit.is_some() {
            self.note_key(k);
            return;
        }
        if self.move_dlg.is_some() {
            self.move_key(k);
            return;
        }
        if self.help {
            self.help = false;
            return;
        }
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Char('q') | KeyCode::Char('c') if ctrl => {
                self.quit = true;
                return;
            }
            KeyCode::F(1) => {
                self.help = true;
                return;
            }
            KeyCode::F(2) => {
                self.sort = self.sort.next();
                self.reload_view();
                self.save_view();
                self.msg = Some(format!("Sort: {}", self.sort.name()));
                return;
            }
            KeyCode::F(3) => {
                self.show_done = !self.show_done;
                self.reload_view();
                self.save_view();
                return;
            }
            KeyCode::F(4) => {
                self.show_sidebar = !self.show_sidebar;
                if !self.show_sidebar && self.focus == Focus::Sidebar {
                    self.focus = Focus::List;
                }
                self.save_view();
                return;
            }
            KeyCode::F(5) => {
                self.archive_done();
                return;
            }
            KeyCode::F(6) => {
                self.calendar = !self.calendar;
                self.refresh_assist();
                self.save_view();
                self.msg = Some(format!("Calendar when typing dates: {}", if self.calendar { "on" } else { "off" }));
                return;
            }
            KeyCode::Char('p') if ctrl => {
                self.toggle_parent();
                return;
            }
            KeyCode::Char('o') if ctrl => {
                self.open_move();
                return;
            }
            KeyCode::Char('l') if ctrl => {
                self.show_status = !self.show_status;
                self.save_view();
                return;
            }
            KeyCode::F(7) => {
                self.edit_note();
                return;
            }
            KeyCode::F(9) => {
                self.git_sync();
                return;
            }
            KeyCode::F(10) => {
                self.git_menu = if self.git_menu.is_some() { None } else { Some(0) };
                return;
            }
            KeyCode::Char('k') if ctrl => {
                // add the open file to the known files and make it the default (a file opened with -f from
                // another folder is otherwise only used for this run)
                let (p, f) = (self.path.clone(), self.folder.clone());
                config::update(|c| {
                    c.file = Some(p.clone());
                    c.folder = f.clone();
                    if f.is_none() && !c.files.contains(&p) {
                        c.files.push(p);
                    }
                });
                self.msg = Some(format!("Remembered: {}", self.path.display()));
                return;
            }
            KeyCode::F(8) => {
                if self.picker.is_some() {
                    self.picker = None;
                } else {
                    self.open_picker();
                }
                return;
            }
            _ => {}
        }
        if self.git_menu.is_some() {
            self.git_menu_key(k);
            return;
        }
        if self.picker.is_some() {
            self.picker_key(k);
            return;
        }
        match self.focus {
            Focus::List => self.list_key(k),
            Focus::Sidebar => self.side_key(k),
            Focus::Input => self.input_key(k),
        }
    }

    fn list_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let alt = k.modifiers.contains(KeyModifiers::ALT);
        match k.code {
            KeyCode::Up if alt => self.shift_priority(true),
            KeyCode::Down if alt => self.shift_priority(false),
            KeyCode::Up => self.move_sel(-1),
            KeyCode::Down => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-10),
            KeyCode::PageDown => self.move_sel(10),
            KeyCode::Home => self.select_pos(0),
            KeyCode::End => self.select_pos(self.view.len().saturating_sub(1)),
            KeyCode::Char(' ') if !ctrl && !alt => self.toggle_done(),
            KeyCode::Enter => self.start_edit(),
            KeyCode::Delete => self.delete_selected(),
            KeyCode::Tab if self.show_sidebar => self.focus = Focus::Sidebar,
            KeyCode::Tab => self.focus = Focus::Input,
            KeyCode::Esc => {
                if self.filter.is_active() {
                    self.filter.clear();
                    self.reload_view();
                } else {
                    self.quit = true;
                }
            }
            KeyCode::Char('z') if ctrl && !alt => self.undo_list(),
            KeyCode::Char('/') if !ctrl => self.enter_search(),
            // any other printable key starts typing in the bar (like the web app)
            KeyCode::Char(_) if (!ctrl && !alt) || (ctrl && alt) => {
                self.focus = Focus::Input;
                self.input_key(k);
            }
            _ => {}
        }
    }

    fn side_key(&mut self, k: KeyEvent) {
        let entries = self.side_entries();
        let n = entries.len();
        match k.code {
            KeyCode::Up => self.side_idx = self.side_idx.saturating_sub(1),
            KeyCode::Down => self.side_idx = (self.side_idx + 1).min(n.saturating_sub(1)),
            KeyCode::Enter | KeyCode::Char(' ') => {
                // Space (or Ctrl+Enter) = add/remove, Enter = only this one
                let multi = k.code == KeyCode::Char(' ') || k.modifiers.contains(KeyModifiers::CONTROL);
                if let Some((e, _)) = entries.get(self.side_idx) {
                    let e = e.clone();
                    self.toggle_side(&e, multi);
                }
            }
            KeyCode::Delete | KeyCode::Backspace => {
                if let Some((e, _)) = entries.get(self.side_idx) {
                    let e = e.clone();
                    self.clear_side_section(&e);
                }
            }
            KeyCode::Tab | KeyCode::Esc | KeyCode::Right => self.focus = Focus::List,
            KeyCode::Char(_) => {
                self.focus = Focus::Input;
                self.input_key(k);
            }
            _ => {}
        }
    }

    fn input_key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL) && !k.modifiers.contains(KeyModifiers::ALT);
        let alt = k.modifiers.contains(KeyModifiers::ALT) && !k.modifiers.contains(KeyModifiers::CONTROL);
        let in_cal = matches!(self.assist, Some(Assist::Cal { .. }));

        // calendar navigation
        if in_cal {
            match k.code {
                KeyCode::Left if alt => { self.cal_move(-1, 0); return; }
                KeyCode::Right if alt => { self.cal_move(1, 0); return; }
                KeyCode::Up if alt => { self.cal_move(-7, 0); return; }
                KeyCode::Down if alt => { self.cal_move(7, 0); return; }
                KeyCode::PageUp => { self.cal_move(0, -1); return; }
                KeyCode::PageDown => { self.cal_move(0, 1); return; }
                _ => {}
            }
        }

        match k.code {
            KeyCode::Esc => {
                if self.assist.is_some() {
                    let (start, _, word) = self.input.word_at_cursor();
                    self.dismissed = Some((start, word));
                    self.assist = None;
                } else {
                    match self.mode {
                        Mode::Edit(_) => self.finish_edit(false),
                        Mode::Search => self.leave_search(),
                        Mode::Add => self.focus_list(),
                    }
                }
            }
            KeyCode::Enter | KeyCode::Char(' ')
                if !ctrl && !alt && matches!(self.assist, Some(Assist::List { exact: false, .. })) =>
            {
                // Enter / Space take the highlighted suggestion (like Tab); a word that already
                // exists exactly is left alone
                self.accept_assist();
            }
            KeyCode::Enter => match self.mode {
                Mode::Add => self.submit_add(),
                Mode::Search => self.focus_list(),
                Mode::Edit(_) => self.finish_edit(true),
            },
            KeyCode::Tab => {
                if !self.accept_assist() {
                    self.focus_list();
                }
            }
            KeyCode::Up | KeyCode::Down if self.assist.is_some() && !in_cal => {
                if let Some(Assist::List { idx, items, .. }) = &mut self.assist {
                    let n = items.len();
                    *idx = if k.code == KeyCode::Down { (*idx + 1) % n } else { (*idx + n - 1) % n };
                }
            }
            KeyCode::Up | KeyCode::Down if !alt => {
                if self.mode == Mode::Search {
                    self.leave_search();
                }
                self.assist = None;
                self.focus = Focus::List;
                self.move_sel(if k.code == KeyCode::Up { -1 } else { 1 });
            }
            KeyCode::Char('/') if !ctrl && !alt && self.mode == Mode::Add && self.input.is_empty() => self.enter_search(),
            _ => {
                let today = self.today();
                match self.input.handle_key(k, today) {
                    Edited::No => {}
                    Edited::Moved => self.refresh_assist(),
                    Edited::Yes => {
                        self.on_input_changed();
                        if k.code == KeyCode::Char(':') && self.mode != Mode::Search && self.input.note_trigger() {
                            let name = folder::next_note_name_with(&self.store.lines, &self.input.text, self.folder.as_deref());
                            self.open_note_edit(&name, true);
                        }
                    }
                    Edited::Word(_) => {
                        self.dismissed = None; // let the tag / context menu open right away
                        self.on_input_changed();
                    }
                }
            }
        }
    }

    // ---------------------------------------------------------------- drawing

    fn draw(&mut self, f: &mut Frame) {
        let area = f.area();
        let rows = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(if self.show_status { 1 } else { 0 }),
        ])
        .split(area);
        self.draw_header(f, rows[0]);

        let list_area = if self.show_sidebar && rows[1].width > 50 {
            let cols = Layout::horizontal([Constraint::Length(26), Constraint::Min(20)]).split(rows[1]);
            self.draw_sidebar(f, cols[0]);
            cols[1]
        } else {
            rows[1]
        };
        self.draw_tasks(f, list_area);
        self.draw_input(f, rows[2]);
        if self.show_status {
            self.draw_status(f, rows[3]);
        }
        self.draw_assist(f, list_area, rows[2]);
        if self.picker.is_some() {
            self.draw_picker(f, area);
        }
        if self.git_menu.is_some() {
            self.draw_git_menu(f, area);
        }
        if self.note_edit.is_some() {
            self.draw_note_edit(f, area);
        }
        if self.move_dlg.is_some() {
            self.draw_move(f, area);
        }
        if self.server_diff.is_some() {
            self.draw_server_diff(f, area);
        }
        if self.help {
            self.draw_help(f, area);
        }
    }

    /// Popup listing what changed on the WebDAV server (the new version was already loaded).
    fn draw_server_diff(&self, f: &mut Frame, area: Rect) {
        let Some((rem, add, scroll, sorted)) = &self.server_diff else { return };
        let w = 90.min(area.width.saturating_sub(2));
        let want = (rem.len() + add.len() + 4).max(6);
        let h = want.min(area.height.saturating_sub(2).max(6) as usize).min(area.height as usize) as u16;
        let rect = Rect::new(area.x + area.width.saturating_sub(w) / 2, area.y + area.height.saturating_sub(h) / 2, w, h);
        f.render_widget(Clear, rect);
        let mut lines: Vec<Line> = Vec::new();
        for l in rem {
            lines.push(Line::from(Span::styled(format!("- {l}"), Style::default().fg(Color::Red))));
        }
        for l in add {
            lines.push(Line::from(Span::styled(format!("+ {l}"), Style::default().fg(Color::Green))));
        }
        if lines.is_empty() {
            lines.push(Line::from(Span::styled("No lines were added or removed.", dim())));
        }
        if *sorted {
            let t = if rem.is_empty() && add.is_empty() { "The lines are only sorted differently on the server." } else { "The remaining lines are also sorted differently on the server." };
            lines.insert(0, Line::from(Span::styled(t, Style::default().fg(Color::Yellow))));
        }
        let title = format!(" Changed on the server: {} removed, {} added ", rem.len(), add.len());
        let block = Block::default()
            .borders(Borders::ALL)
            .title(title)
            .title_bottom(" Ctrl+Z undoes · ↑↓ scroll · any other key closes ");
        f.render_widget(Paragraph::new(lines).block(block).scroll((*scroll as u16, 0)), rect);
    }

    fn draw_git_menu(&self, f: &mut Frame, area: Rect) {
        let Some(sel) = self.git_menu else { return };
        let mut lines: Vec<Line> = Vec::new();
        match &self.folder {
            None => lines.push(Line::from("Git works on a todo folder (F8, then S to use a folder).")),
            Some(d) => {
                let cfg = git::GitCfg::load(d);
                let repo = d.join(".git").exists();
                for (i, (key, label)) in GIT_ROWS.iter().enumerate() {
                    let text = match (*key, &cfg) {
                        ("sync", _) | ("init", _) => format!("    {label}"),
                        (k, Some(c)) => format!("[{}] {label}", if c.get(k).unwrap_or(false) { "x" } else { " " }),
                        (_, None) => format!("[ ] {label}"),
                    };
                    let dim = (*key == "init" && cfg.is_some() && repo) || (!["sync", "init"].contains(key) && cfg.is_none());
                    let mut st = if dim { Style::default().fg(Color::DarkGray) } else { Style::default() };
                    if i == sel {
                        st = st.bg(Color::Indexed(238));
                    }
                    lines.push(Line::from(Span::styled(text, st)));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("{} - {}", d.display(), if repo { "git repository" } else { "no git repository" }),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        }
        let w = 62.min(area.width.saturating_sub(2));
        let h = (lines.len() as u16 + 2).min(area.height);
        let rect = Rect::new(area.x + area.width.saturating_sub(w) / 2, area.y + area.height.saturating_sub(h) / 2, w, h);
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightBlue))
            .title(" Git (.todo-git.cfg) ")
            .title_bottom(" Up/Down - Enter toggle / run - Esc close ");
        f.render_widget(Paragraph::new(lines).block(block), rect);
    }

    fn git_menu_key(&mut self, k: KeyEvent) {
        let Some(sel) = self.git_menu else { return };
        match k.code {
            KeyCode::Esc => self.git_menu = None,
            KeyCode::Up => self.git_menu = Some(sel.saturating_sub(1)),
            KeyCode::Down => self.git_menu = Some((sel + 1).min(GIT_ROWS.len() - 1)),
            KeyCode::Enter | KeyCode::Char(' ') => {
                let Some(dir) = self.folder.clone() else {
                    self.msg = Some("git needs folder mode (F8, then S)".into());
                    return;
                };
                let key = GIT_ROWS[sel].0;
                match key {
                    "sync" => {
                        self.git_menu = None;
                        self.git_sync();
                    }
                    "init" => {
                        self.msg = Some(match git::init(&dir) {
                            Ok(m) => format!("git: {m}"),
                            Err(e) => format!("git: {e}"),
                        });
                    }
                    _ => match git::GitCfg::load(&dir) {
                        None => self.msg = Some("no .todo-git.cfg: initialize git first (last row)".into()),
                        Some(mut c) => {
                            let on = !c.get(key).unwrap_or(false);
                            let r = c.set(key, on).and_then(|_| c.save(&dir).map_err(|e| e.to_string()));
                            if let Err(e) = r {
                                self.msg = Some(format!("git: {e}"));
                            }
                        }
                    },
                }
            }
            _ => {}
        }
    }

    fn draw_header(&self, f: &mut Frame, area: Rect) {
        let name = self.path.file_name().map_or_else(|| "todo.txt".into(), |n| n.to_string_lossy().into_owned());
        let line = Line::from(vec![
            Span::styled(format!(" {name} "), Style::default().add_modifier(Modifier::BOLD).fg(Color::Black).bg(Color::LightBlue)),
            Span::styled(
                format!("  sort: {}{}  ", self.sort.name(), if self.show_done { "" } else { "  (completed hidden)" }),
                dim(),
            ),
            Span::styled(self.path.display().to_string(), dim()),
            Span::styled(if self.folder.is_some() { "   [folder mode - F8 switches files]" } else { "" }, dim()),
        ]);
        // version and commit hash, top right
        let version = format!(" v{} ({}) ", env!("TODO_VERSION"), env!("TODO_HASH"));
        let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(version.width() as u16)]).split(area);
        f.render_widget(Paragraph::new(line), cols[0]);
        f.render_widget(Paragraph::new(Span::styled(version, dim())), cols[1]);
    }

    fn draw_sidebar(&self, f: &mut Frame, area: Rect) {
        let entries = self.side_entries();
        let border = if self.focus == Focus::Sidebar { Color::LightBlue } else { Color::DarkGray };
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(border)).title(" Filters ");
        let inner = block.inner(area);
        f.render_widget(block, area);

        let mut lines: Vec<Line> = Vec::new();
        let mut section = "";
        for (i, (e, count)) in entries.iter().enumerate() {
            let (title, label, active, tok) = match e {
                Side::Project(p) => ("Projects", format!("+{p}"), self.filter.project.contains(p), Tok::Project),
                Side::Context(c) => ("Contexts", format!("@{c}"), self.filter.context.contains(c), Tok::Context),
                Side::Priority(p) => ("Priority", format!("({p})"), self.filter.priority.contains(p), Tok::Priority(*p)),
            };
            if title != section {
                section = title;
                let on = (title == "Projects" && !self.filter.project.is_empty())
                    || (title == "Contexts" && !self.filter.context.is_empty())
                    || (title == "Priority" && !self.filter.priority.is_empty());
                let mut spans = vec![Span::styled(title.to_string(), dim().add_modifier(Modifier::BOLD))];
                if on {
                    // clear button at the right (Del / Backspace on an entry of the section)
                    let x = "[x]";
                    let pad = (inner.width as usize).saturating_sub(title.width() + x.width());
                    spans.push(Span::raw(" ".repeat(pad)));
                    spans.push(Span::styled(x, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
                }
                lines.push(Line::from(spans));
            }
            let mut st = tok_style(tok);
            if active {
                st = st.add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
            }
            if self.focus == Focus::Sidebar && i == self.side_idx {
                st = st.add_modifier(Modifier::REVERSED);
            }
            let width = inner.width as usize;
            let text = format!("{}{}", if active { "✓ " } else { "  " }, label);
            let count = count.to_string();
            let pad = width.saturating_sub(text.width() + count.width());
            lines.push(Line::from(vec![Span::styled(text, st), Span::raw(" ".repeat(pad)), Span::styled(count, dim())]));
        }
        if entries.is_empty() {
            lines.push(Line::from(Span::styled("no projects, contexts", dim())));
        }
        f.render_widget(Paragraph::new(lines), inner);
    }

    fn draw_tasks(&mut self, f: &mut Frame, area: Rect) {
        let today = todotxt::iso(self.today());
        let items: Vec<ListItem> = self.view.iter().map(|&i| self.task_item(i, &today)).collect();
        let border = if self.focus == Focus::List { Color::LightBlue } else { Color::DarkGray };
        let title = format!(" Tasks ({}) ", self.view.len());
        let block = Block::default().borders(Borders::ALL).border_style(Style::default().fg(border)).title(title);
        if items.is_empty() {
            let text = if self.store.lines.iter().all(|l| l.trim().is_empty()) {
                "No tasks yet. Type below and press Enter to add one."
            } else {
                "No tasks match."
            };
            f.render_widget(Paragraph::new(Span::styled(text, dim())).block(block), area);
            return;
        }
        let list = List::new(items).block(block).highlight_style(Style::default().bg(Color::Indexed(238)));
        f.render_stateful_widget(list, area, &mut self.list_state);
    }

    fn task_item(&self, line: usize, today: &str) -> ListItem<'static> {
        let t = todotxt::parse(&self.store.lines[line]);
        // soon = yellow, today = orange, overdue = red: on the due: word and the line number
        let due_style = |st: DueStatus| match st {
            DueStatus::Overdue => Some(Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
            DueStatus::Today => Some(Style::default().fg(Color::Indexed(208)).add_modifier(Modifier::BOLD)),
            DueStatus::Soon => Some(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            DueStatus::Upcoming => None,
        };
        let row_due = todotxt::due_status(&t, today).filter(|_| !t.done).and_then(due_style);
        let mut spans = vec![
            Span::styled(format!("{:>3} ", line + 1), row_due.unwrap_or_else(dim)),
            Span::raw("  ".repeat(t.level)), // sub-entries are indented
            Span::raw(if t.done { "[x] " } else { "[ ] " }),
        ];
        match t.priority {
            Some(p) if !t.done => spans.push(Span::styled(format!("({p}) "), tok_style(Tok::Priority(p)))),
            Some(p) => spans.push(Span::styled(format!("({p}) "), dim())),
            None => spans.push(Span::raw("    ")),
        }
        for (piece, tok) in highlight(&t.text, false) {
            let style = if t.done {
                dim().add_modifier(Modifier::CROSSED_OUT)
            } else if tok == Tok::Due {
                let mut tt = todotxt::Task::default();
                if let Some(d) = piece.strip_prefix("due:") {
                    tt.tags.insert("due".into(), d.to_string());
                }
                match todotxt::due_status(&tt, today).and_then(due_style) {
                    Some(st) => st,
                    None => Style::default().fg(Color::Gray),
                }
            } else {
                tok_style(tok)
            };
            spans.push(Span::styled(piece, style));
        }
        let item = ListItem::new(Line::from(spans));
        if self.parent_line() == Some(line) {
            item.style(Style::default().bg(Color::Indexed(236)).add_modifier(Modifier::UNDERLINED)) // the selected parent
        } else {
            item
        }
    }

    fn draw_move(&self, f: &mut Frame, area: Rect) {
        let Some(inp) = &self.move_dlg else { return };
        let n = list::select_matching(&self.store.lines, |l| self.filter.matches_line(l)).len();
        let w = 76.min(area.width.saturating_sub(2));
        let rect = Rect::new(area.x + area.width.saturating_sub(w) / 2, area.y + area.height.saturating_sub(3) / 2, w, 3.min(area.height));
        f.render_widget(Clear, rect);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow))
            .title(format!(" Move {n} line(s) (filtered tasks + sub-entries) to file "))
            .title_bottom(" Enter move · Esc cancel ");
        let inner = block.inner(rect);
        f.render_widget(block, rect);
        let col = inp.text.chars().take(inp.cursor).collect::<String>().width() as u16;
        let offset = col.saturating_sub(inner.width.saturating_sub(1));
        f.render_widget(Paragraph::new(inp.text.clone()).scroll((0, offset)), inner);
        f.set_cursor_position(Position::new(inner.x + col - offset, inner.y));
    }

    fn draw_input(&self, f: &mut Frame, area: Rect) {
        let (title, color) = match self.mode {
            Mode::Add => match self.parent_line() {
                Some(i) => (format!(" Add sub-entry under line {}  (Ctrl+P: off) ", i + 1), Color::LightBlue),
                None => (" Add task ".to_string(), Color::LightBlue),
            },
            Mode::Search => (" Search  (Esc: back to adding) ".to_string(), Color::Yellow),
            Mode::Edit(i) => (format!(" Edit line {}  (Enter: save, Esc: cancel) ", i + 1), Color::Green),
        };
        let focused = self.focus == Focus::Input;
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(if focused { color } else { Color::DarkGray }))
            .title(title);
        let inner = block.inner(area);
        f.render_widget(block, area);

        let line = if self.input.is_empty() {
            let hint = match self.mode {
                Mode::Add => "(A) Call Mom +Family @phone due:tomorrow      type to add, / to search",
                Mode::Search => "type to filter the list",
                Mode::Edit(_) => "",
            };
            Line::from(Span::styled(hint, dim()))
        } else {
            Line::from(highlight(&self.input.text, true).into_iter().map(|(s, t)| Span::styled(s, tok_style(t))).collect::<Vec<_>>())
        };
        let col = self.input.text.chars().take(self.input.cursor).collect::<String>().width() as u16;
        let offset = col.saturating_sub(inner.width.saturating_sub(1));
        f.render_widget(Paragraph::new(line).scroll((0, offset)), inner);
        if focused && !self.help {
            f.set_cursor_position(Position::new(inner.x + col - offset, inner.y));
        }
    }

    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let open = self.store.lines.iter().filter(|l| !l.trim().is_empty() && !todotxt::parse(l).done).count();
        let total = self.store.lines.iter().filter(|l| !l.trim().is_empty()).count();
        let mut right = format!(" {open} open · {total} total · showing {} ", self.view.len());
        if self.filter.is_active() {
            let mut parts: Vec<String> = Vec::new();
            if !self.filter.q.trim().is_empty() {
                parts.push(format!("\"{}\"", self.filter.q.trim()));
            }
            parts.extend(self.filter.project.iter().map(|p| format!("+{p}")));
            parts.extend(self.filter.context.iter().map(|c| format!("@{c}")));
            parts.extend(self.filter.priority.iter().map(|p| format!("({p})")));
            right = format!(" filter: {} (Esc clears) ·{right}", parts.join(" "));
        }
        let left = match &self.msg {
            Some(m) => Span::styled(format!(" {m}"), Style::default().fg(Color::Yellow)),
            None => Span::styled(
                match self.focus {
                    Focus::List => " ↑↓ select · Space done · Enter edit · Del delete · Alt+↑↓ priority · / search · F7 note · F8 files · F1 help",
                    Focus::Sidebar => " ↑↓ select · Enter only this · Space add/remove · Del clear section · Tab back · F1 help",
                    Focus::Input => " Enter ok · Tab complete/list · Esc back · F1 help",
                },
                dim(),
            ),
        };
        let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(right.width() as u16)]).split(area);
        let mut spans: Vec<Span> = Vec::new();
        let mut used = left.width();
        if let Some(st) = &self.git_status {
            used += st.label().width() + 8;
        }
        let remote_path = self.path.to_string_lossy().into_owned();
        if crate::webdav::is_remote(&remote_path) && crate::davcache::is_offline() {
            let pend = crate::davcache::pending_changes(&remote_path).map_or(String::new(), |(r, a)| format!(" · {} pending", r + a));
            spans.push(Span::styled(format!(" offline{pend} "), Style::default().fg(Color::Yellow)));
            spans.push(Span::styled("│", dim()));
            used += 12 + pend.width();
        }
        if crate::webdav::is_remote(&remote_path) {
            // WebDAV: show the full address on the left (shortened from the front when there is no room)
            let room = (cols[0].width as usize).saturating_sub(used + 4).min(cols[0].width as usize * 55 / 100);
            let label = dav_label(&remote_path);
            let shown: String = if label.width() > room {
                let keep = room.saturating_sub(1);
                let tail: String = label.chars().rev().take(keep).collect::<Vec<_>>().into_iter().rev().collect();
                format!("…{tail}")
            } else {
                label
            };
            if room > 8 {
                spans.push(Span::styled(format!(" {shown} "), dim()));
                spans.push(Span::styled("│", dim()));
            }
        } else if let Some(dir) = &self.folder {
            let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.display().to_string());
            if used + name.width() + 4 <= cols[0].width as usize {
                spans.push(Span::styled(format!(" {name} "), dim()));
                spans.push(Span::styled("│", dim()));
            }
        }
        if let Some(st) = &self.git_status {
            let style = if st.changed > 0 { Style::default().fg(Color::Yellow) } else { dim() };
            spans.push(Span::styled(format!(" git: {} ", st.label()), style));
            spans.push(Span::styled("│", dim()));
        }
        spans.push(left);
        f.render_widget(Paragraph::new(Line::from(spans)), cols[0]);
        f.render_widget(Paragraph::new(Span::styled(right, dim())), cols[1]);
    }

    fn draw_assist(&self, f: &mut Frame, list_area: Rect, input_area: Rect) {
        let Some(assist) = &self.assist else { return };
        match assist {
            Assist::List { items, idx, .. } => {
                let w = items.iter().map(|i| i.value.width() + 6).max().unwrap_or(10).max(16) as u16 + 2;
                let h = items.len() as u16 + 2;
                let rect = popup_rect(list_area, input_area, w, h);
                let lines: Vec<Line> = items
                    .iter()
                    .enumerate()
                    .map(|(i, it)| {
                        let mut st = tok_style(it.tok);
                        if i == *idx {
                            st = st.add_modifier(Modifier::REVERSED);
                        }
                        let pad = (rect.width as usize).saturating_sub(2 + it.value.width() + it.count.to_string().len() + 1);
                        Line::from(vec![
                            Span::styled(format!(" {}", it.value), st),
                            Span::styled(" ".repeat(pad), st),
                            Span::styled(format!("{} ", it.count), if i == *idx { st } else { dim() }),
                        ])
                    })
                    .collect();
                f.render_widget(Clear, rect);
                f.render_widget(
                    Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Tab: insert ")),
                    rect,
                );
            }
            Assist::Cal { month, cursor, selected, .. } => {
                let rect = popup_rect(list_area, input_area, 26, 11);
                let today = self.today();
                let mut lines: Vec<Line> = vec![
                    Line::from(Span::styled(
                        format!("{:^24}", format!("{} {}", MONTHS[month.month0() as usize], month.year())),
                        Style::default().add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(" Mo Tu We Th Fr Sa Su", dim())),
                ];
                let offset = month.weekday().num_days_from_monday() as usize;
                let days_in_month = days_in(month.year(), month.month());
                let mut cells: Vec<Span> = vec![Span::raw("   "); offset];
                for d in 1..=days_in_month {
                    let date = NaiveDate::from_ymd_opt(month.year(), month.month(), d).unwrap_or(*month);
                    let mut st = Style::default();
                    if date == today {
                        st = st.add_modifier(Modifier::UNDERLINED).fg(Color::LightBlue);
                    }
                    if Some(date) == *selected {
                        st = st.fg(Color::Green).add_modifier(Modifier::BOLD);
                    }
                    if date == *cursor {
                        st = st.add_modifier(Modifier::REVERSED);
                    }
                    cells.push(Span::styled(format!("{d:>3}"), st));
                }
                for week in cells.chunks(7) {
                    lines.push(Line::from(week.to_vec()));
                }
                lines.truncate(8);
                lines.push(Line::from(Span::styled("Tab insert · Alt+←↑↓→ · PgUp/Dn", dim())));
                f.render_widget(Clear, rect);
                f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" due ")), rect);
            }
        }
    }

    fn draw_picker(&mut self, f: &mut Frame, area: Rect) {
        let current = self.path.file_name().map(|n| n.to_string_lossy().into_owned());
        let in_dir = self.folder.clone();
        let Some(pk) = &mut self.picker else { return };
        let w = 70.min(area.width.saturating_sub(2));
        let h = (pk.entries.len() as u16 + 4).clamp(8, area.height.saturating_sub(2).max(8)).min(area.height);
        let rect = Rect::new(
            area.x + area.width.saturating_sub(w) / 2,
            area.y + area.height.saturating_sub(h) / 2,
            w,
            h,
        );
        f.render_widget(Clear, rect);
        let items: Vec<ListItem> = pk
            .entries
            .iter()
            .map(|e| match e.kind {
                PKind::Up => ListItem::new(Span::styled("..  (up)", Style::default().fg(Color::LightBlue))),
                PKind::Dir => ListItem::new(Span::styled(format!("{}/", e.name), Style::default().fg(Color::LightBlue))),
                PKind::File => {
                    let cur = pk.dir == self.path.parent().map(|p| p.to_path_buf()).unwrap_or_default()
                        && current.as_deref() == Some(e.name.as_str());
                    ListItem::new(Span::styled(
                        format!("{}{}", if cur { "✓ " } else { "  " }, e.name),
                        if cur { Style::default().add_modifier(Modifier::BOLD) } else { Style::default() },
                    ))
                }
            })
            .collect();
        let mode = if in_dir.as_deref() == Some(pk.dir.as_path()) { " (folder mode)" } else { "" };
        let title = format!(" {}{} ", pk.dir.display(), mode);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::LightBlue))
            .title(title)
            .title_bottom(" Enter open · S use this folder · Backspace up · Esc close ");
        let list = List::new(items).block(block).highlight_style(Style::default().bg(Color::Indexed(238)));
        f.render_stateful_widget(list, rect, &mut pk.state);
    }

    fn draw_help(&self, f: &mut Frame, area: Rect) {
        let text = [
            "todo.txt - keys",
            "",
            "List:      ↑ ↓ PgUp PgDn Home End   select",
            "           Space        mark done / open again",
            "           Enter        edit the selected line in the bar",
            "           Delete       delete (Ctrl+Z undoes)",
            "           Alt+↑ / Alt+↓  raise / lower priority",
            "           Tab          sidebar filters   Esc  clear filter / quit",
            "           Ctrl+P       select as parent: new entries go below it, indented (again: off)",
            "           Ctrl+O       move the filtered tasks (+ sub-entries) to another file",
            "Sub-entries: start a line with - / -- / --- / ---- (sub, sub-sub ...)",
            "Sidebar:   Enter only this entry   Space add/remove (several = OR in a section)",
            "           Del / Backspace clear that section's filter ([x] at its title)",
            "           any other key starts typing a new task",
            "Bar:       Enter add / save   Esc back   /  search (first char)",
            "           Tab accept suggestion (+tag @context (P) due:date)",
            "           Alt+arrows / PgUp PgDn move in the calendar, Tab inserts",
            "           Ctrl+Backspace / Ctrl+Delete  remove the whole word",
            "           Ctrl+Z / Ctrl+Y  undo / redo while typing",
            "Dates:     due:tomorrow  due:+3d  due:next week  due:24.12.  due:24.12.2026",
            "Global:    F2 sort   F3 show/hide done   F4 sidebar   F5 archive done",
            "           F6 calendar on/off   F7 note of the task   F8 file / folder selector",
            "           Ctrl+K remember this file (add to known files, make it the default)",
            "           Ctrl+L status line on/off",
            "           F9 git sync now   F10 git settings (support, auto, pull, commit, push, init;",
            "           stored in .todo-git.cfg of the folder, same as the desktop app)",
            "           Ctrl+Q quit   every change is saved at once; view settings are remembered",
            "Files:     F8: Enter opens a file / enters a folder, S uses the shown folder for all",
            "           its *.txt files, Backspace goes up. note:NAME text lives in NAME.txt",
            "           (folder mode: next to the lists, else ~/.todo.txt/notes/)",
            "",
            "press any key",
        ];
        let w = 78.min(area.width.saturating_sub(2));
        let h = (text.len() as u16 + 2).min(area.height);
        let rect = Rect::new(area.x + (area.width.saturating_sub(w)) / 2, area.y + (area.height.saturating_sub(h)) / 2, w, h);
        f.render_widget(Clear, rect);
        let lines: Vec<Line> = text.iter().map(|l| Line::from(*l)).collect();
        f.render_widget(Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" Help ")), rect);
    }
}

/// "server - folder/file" for a WebDAV address: the host of the configured server and the path below its base URL.
pub(crate) fn dav_label(url: &str) -> String {
    let after_scheme = url.split_once("://").map_or(url, |x| x.1);
    let (host, path) = after_scheme.split_once('/').unwrap_or((after_scheme, ""));
    let base_path = crate::store::server_of(std::path::Path::new(url))
        .ok()
        .map(|s| s.url.split_once("://").map_or(s.url.clone(), |x| x.1.to_string()))
        .and_then(|b| b.split_once('/').map(|x| x.1.to_string()))
        .unwrap_or_default();
    let rel = path.strip_prefix(&base_path).unwrap_or(path);
    let rel = percent_decode(rel);
    format!("{} - {}", host.split('@').last().unwrap_or(host).split(':').next().unwrap_or(host), rel)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn days_in(year: i32, month: u32) -> u32 {
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map_or(30, |d| d.day())
}

/// A popup above the input bar, aligned with the task list.
fn popup_rect(list_area: Rect, input_area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(list_area.width);
    let h = h.min(input_area.y.saturating_sub(list_area.y).max(3));
    let y = input_area.y.saturating_sub(h);
    Rect::new(list_area.x + 1, y, w, h)
}

/// The note popup (a multi-line input, no external editor).
struct NoteEdit {
    n: folder::NoteFile,
    name: String,
    word: bool,
    ta: crate::textarea::TextArea,
}
