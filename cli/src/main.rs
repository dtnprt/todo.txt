//! `todo` - a todo.txt command line tool.
//!
//!   todo (A) call mom +Family @phone   add a task and exit
//!   todo -a                            add a task in an interactive line editor
//!   todo                               interactive terminal UI on the last used file
//!   todo -l | -t | -P                  list / numbered text / plain text
//!   todo -p 3 A                        set priority of line 3 (todo -p 3 - removes it)
//!   todo -e 3 | -c 3 TEXT | -d 3       edit / change / delete line 3

mod config;
mod editor;
mod folder;
mod git;
mod highlight;
mod input;
mod list;
mod ops;
mod store;
mod webdav;
mod todotxt;
mod tui;

use clap::Parser;
use config::Config;
use list::Sort;
use ops::Style;
use std::error::Error;
use std::path::PathBuf;
use store::Store;

#[derive(Parser, Debug)]
#[command(
    name = "todo",
    version = env!("TODO_VERSION_FULL"),
    about = "todo.txt on the command line",
    long_about = "todo.txt on the command line.\n\n\
        Without arguments the interactive terminal UI opens on the last used file\n\
        (remembered in ~/.config/todo.cfg). Text without a switch is added as a new task.",
    after_help = "EXAMPLES:\n  \
        todo \"(A) call mom +Family @phone due:tomorrow\"   add a task\n  \
        todo -l -s due                                    list, sorted by due date\n  \
        todo -l \"+Home @phone\"                            list only tasks with +Home and @phone\n  \
        todo -c 3 pay rent due:+3d                        replace line 3\n  \
        todo -e 3                                         edit line 3 interactively\n\n\
        Put switches before the text. Quote the text if your shell treats ( ) or + specially\n\
        (PowerShell and bash do).\n\
        Dates after due: understand 2026-12-01, 24.12.2026, 24.12., today, tomorrow, next week, +3d, +2 weeks."
)]
struct Cli {
    /// Show the todo list (coloured, with line numbers); optional FILTER such as "+tag @context (A) word"
    /// shows only matching tasks (all terms must match)
    #[arg(short = 'l', long, value_name = "FILTER", num_args = 0..=1, default_missing_value = "")]
    list: Option<String>,

    /// Sort order: priority, due (date), created, alpha, file
    #[arg(short = 's', long, value_name = "ORDER")]
    sort: Option<String>,

    /// With --list: also show completed tasks (hidden by default)
    #[arg(short = 'A', long)]
    all: bool,

    /// With --list: also show the summary line (open / total tasks, file name)
    #[arg(short = 'v', long)]
    verbose: bool,

    /// Git (folder mode): commit, pull and push the folder now
    #[arg(short = 'G', long)]
    sync: bool,

    /// Git (folder mode): make the folder a git repository and create .todo-git.cfg
    #[arg(long)]
    git_init: bool,

    /// Git support on or off (`enabled` in .todo-git.cfg); off = no git action at all
    #[arg(long, value_name = "on|off")]
    git: Option<String>,

    /// Automatic git on or off (`auto` in .todo-git.cfg): pull on start, commit + push every change
    #[arg(long, value_name = "on|off")]
    git_auto: Option<String>,

    /// Single automatic actions in .todo-git.cfg, e.g. --git-set pull=off --git-set push=on (pull, commit, push)
    #[arg(long, value_name = "NAME=on|off")]
    git_set: Vec<String>,

    /// Show the text with line numbers
    #[arg(short = 't', long)]
    text: bool,

    /// Show only the text (no line numbers, no colours)
    #[arg(short = 'P', long)]
    plain: bool,

    /// Set the priority of line NUMBER; the priority (A-Z, or - / none to remove it) is the TEXT: todo -p 3 A
    #[arg(short = 'p', long, value_name = "NUMBER")]
    priority: Option<usize>,

    /// Add a task in an interactive line editor (optional TEXT is pre-filled)
    #[arg(short = 'a', long)]
    add: bool,

    /// Edit line NUMBER in an interactive line editor
    #[arg(short = 'e', long, value_name = "NUMBER")]
    edit: Option<usize>,

    /// Change line NUMBER to TEXT
    #[arg(short = 'c', long, value_name = "NUMBER")]
    change: Option<usize>,

    /// Delete line NUMBER
    #[arg(short = 'd', long, value_name = "NUMBER")]
    delete: Option<usize>,

    /// Mark line NUMBER as done (or open again if it is done)
    #[arg(short = 'x', long, value_name = "NUMBER")]
    done: Option<usize>,

    /// Edit the note of line NUMBER in your editor ($VISUAL / $EDITOR); adds `note:NAME` if missing
    #[arg(short = 'D', long, value_name = "NUMBER")]
    note: Option<usize>,

    /// WebDAV: add the server USER@URL and store its password in the OS keyring (asks for it; or set TODO_WEBDAV_PASSWORD)
    #[arg(long, value_name = "USER@URL")]
    webdav_login: Option<String>,

    /// WebDAV: forget the server USER@URL (removes it and its password)
    #[arg(long, value_name = "USER@URL")]
    webdav_logout: Option<String>,

    /// WebDAV: list the files and folders at URL (without URL: the first server)
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "")]
    webdav_ls: Option<String>,

    /// Use this todo.txt (a path, or a WebDAV URL https://...), or a whole FOLDER of *.txt files (and remember it)
    #[arg(short = 'f', long, value_name = "FILE|FOLDER")]
    file: Option<PathBuf>,

    /// Text of the task to add (or the new text for --change)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "TEXT")]
    words: Vec<String>,
}

fn main() {
    if let Err(e) = run(Cli::parse()) {
        eprintln!("todo: {e}");
        std::process::exit(1);
    }
}

/// `--webdav-login`, `--webdav-logout`, `--webdav-ls`.
fn webdav_cmds(cli: &Cli, cfg: &mut Config) -> Result<(), Box<dyn Error>> {
    if let Some(entry) = &cli.webdav_login {
        let server = webdav::Server::parse(entry).ok_or("use USER@URL, e.g. alice@https://cloud.example.com/remote.php/dav/files/alice/")?;
        let pass = match std::env::var("TODO_WEBDAV_PASSWORD") {
            Ok(p) if !p.is_empty() => p,
            _ => rpassword::prompt_password(format!("Password for {} : ", server.entry()))?,
        };
        webdav::cred::set(&server, &pass)?;
        if let Err(e) = webdav::list(&server, &server.url) {
            let _ = webdav::cred::delete(&server);
            return Err(format!("login failed: {e}").into());
        }
        if !cfg.webdav.contains(&server.entry()) {
            cfg.webdav.push(server.entry());
            cfg.save()?;
        }
        println!("ok: {} (password stored in the keyring)", server.entry());
    }
    if let Some(entry) = &cli.webdav_logout {
        let server = webdav::Server::parse(entry).ok_or("use USER@URL")?;
        webdav::cred::delete(&server)?;
        cfg.webdav.retain(|e| *e != server.entry());
        cfg.save()?;
        println!("removed {}", server.entry());
    }
    if let Some(url) = &cli.webdav_ls {
        let url = if url.is_empty() {
            cfg.webdav.first().and_then(|e| webdav::Server::parse(e)).map(|s| s.url).ok_or("no WebDAV server configured (todo --webdav-login USER@URL)")?
        } else {
            url.clone()
        };
        let server = webdav::server_for(&cfg.webdav, &url).ok_or("no WebDAV server configured for that URL")?;
        for e in webdav::list(&server, &url).map_err(|e| e.to_string())? {
            println!("{}{}", if e.is_dir { "[dir]  " } else { "       " }, e.url);
        }
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), Box<dyn Error>> {
    ops::enable_ansi();
    let mut cfg = Config::load();

    // WebDAV server management (no todo file involved)
    if cli.webdav_login.is_some() || cli.webdav_logout.is_some() || cli.webdav_ls.is_some() {
        return webdav_cmds(&cli, &mut cfg);
    }

    // Git: pull before anything is read (only with .todo-git.cfg: enabled, auto and pull on)
    let git_switches = cli.git_init || cli.sync || cli.git.is_some() || cli.git_auto.is_some() || !cli.git_set.is_empty();
    let early_dir = match &cli.file {
        Some(f) if webdav::is_remote(&f.to_string_lossy()) => None,
        Some(f) => std::path::absolute(f).ok().filter(|p| p.is_dir()),
        None => cfg.folder.clone().filter(|d| d.is_dir()),
    };
    // the interactive UI does its own pull / commit / push
    let idle = cli.words.join(" ").trim().is_empty()
        && cli.list.is_none()
        && !cli.text
        && !cli.plain
        && !cli.add
        && cli.sort.is_none()
        && cli.edit.is_none()
        && cli.change.is_none()
        && cli.delete.is_none()
        && cli.done.is_none()
        && cli.note.is_none()
        && cli.priority.is_none();
    let info_only = idle && cli.verbose && !git_switches;
    let tui_mode = idle && !cli.verbose;
    if let (false, false, Some(d)) = (git_switches, tui_mode, &early_dir) {
        match git::auto_start(d, true) {
            Ok(m) if !m.is_empty() => {
                eprintln!("todo: {m}");
                git::post(&m);
            }
            Err(e) => {
                eprintln!("todo: git: {e}");
                git::post(&format!("git: {e}"));
            }
            _ => {}
        }
    }

    // folder mode: `-f FOLDER`, or `folder=` in the config; otherwise a single file
    let mut folder: Option<PathBuf> = None;
    let path = match &cli.file {
        Some(f) if webdav::is_remote(&f.to_string_lossy()) => f.clone(),
        Some(f) => {
            let f = std::path::absolute(f)?;
            if f.is_dir() {
                folder = Some(f.clone());
                folder::pick_file(&f, cfg.file.as_deref())
            } else {
                f
            }
        }
        None => match cfg.folder.clone().filter(|d| d.is_dir()) {
            Some(d) => {
                folder = Some(d.clone());
                folder::pick_file(&d, cfg.file.as_deref())
            }
            None => cfg.file.clone().unwrap_or_else(config::default_todo_file),
        },
    };
    if cfg.file.as_ref() != Some(&path) || cfg.folder != folder {
        cfg.file = Some(path.clone());
        cfg.folder = folder.clone();
        let _ = cfg.save(); // remembering the file is a convenience, never fatal
    }

    let sort_arg = match &cli.sort {
        Some(s) => Some(Sort::parse(s).ok_or_else(|| {
            format!("unknown sort order '{s}' (use priority, due, created, alpha or file)")
        })?),
        None => None,
    };
    let default_sort = sort_arg.or_else(|| cfg.sort.as_deref().and_then(Sort::parse)).unwrap_or(Sort::Priority);
    let today = todotxt::today();
    let today_s = todotxt::iso(today);
    let file_name = path.file_name().map_or_else(|| "todo.txt".to_string(), |n| n.to_string_lossy().into_owned());
    let text = cli.words.join(" ");

    // ---- plain -v: everything about folder, file, git, version and options ----
    if info_only {
        print_info(&cfg, &path, folder.as_deref(), default_sort);
        return Ok(());
    }

    // ---- git ----
    if git_switches {
        let dir = folder.as_deref().ok_or("git works on a todo folder: use todo -f FOLDER first")?;
        let on_off = |name: &str, v: &str| -> Result<bool, String> {
            match v.to_lowercase().as_str() {
                "on" | "true" | "1" | "yes" => Ok(true),
                "off" | "false" | "0" | "no" => Ok(false),
                _ => Err(format!("{name} needs on or off")),
            }
        };
        if cli.git_init {
            println!("{}", git::init(dir)?);
        }
        if cli.git.is_some() || cli.git_auto.is_some() || !cli.git_set.is_empty() {
            let mut gc = git::GitCfg::load(dir).ok_or("no .todo-git.cfg here: run todo --git-init first")?;
            if let Some(v) = &cli.git {
                gc.set("enabled", on_off("--git", v)?)?;
            }
            if let Some(v) = &cli.git_auto {
                gc.set("auto", on_off("--git-auto", v)?)?;
            }
            for kv in &cli.git_set {
                let (k, v) = kv.split_once('=').ok_or("--git-set needs NAME=on|off")?;
                gc.set(k.trim(), on_off(k, v.trim())?)?;
            }
            gc.save(dir)?;
        }
        if let Some(gc) = git::GitCfg::load(dir) {
            println!("{}", gc.describe());
        } else {
            println!("no {} here: no automatic git actions", git::CFG_NAME);
        }
        if cli.sync {
            println!("{}", git::sync(dir, true)?);
        }
        return Ok(());
    }
    // commit (+ push) when this run changed the todo file or the note, if .todo-git.cfg allows it
    let _auto = if tui_mode { None } else { Some(AutoCommit::new(folder.as_deref(), &path, cli.note.is_some())) };

    // ---- note ----
    if let Some(n) = cli.note {
        let mut store = Store::load(&path)?;
        let i = store.index_of(n).ok_or_else(|| format!("no task at line {n}"))?;
        let t = todotxt::parse(&store.lines[i]);
        let name = match folder::note_name(&t) {
            Some(name) => name,
            None => {
                let name = folder::next_note_name(&store.lines, folder.as_deref());
                store.lines[i] = format!("{} note:{name}", store.lines[i]);
                store.save(&path)?;
                println!("{n}: {}", store.lines[i]);
                name
            }
        };
        let file = folder::prepare_note(&path, folder.as_deref(), &name)?;
        folder::edit_in_editor(&file.local)?;
        folder::finish_note(&file)?;
        return Ok(());
    }

    // ---- interactive add ----
    if cli.add {
        match editor::edit_line("add> ", &text, ops::use_color())? {
            None => println!("Cancelled."),
            Some(new) => {
                let new = new.trim();
                if new.is_empty() {
                    println!("Empty text: nothing added.");
                } else {
                    let mut store = Store::load(&path)?;
                    let line = todotxt::with_creation_date(&norm(new, today), &today_s);
                    store.lines.push(line.clone());
                    store.save(&path)?;
                    println!("Added as line {}: {line}", store.lines.len());
                }
            }
        }
        return Ok(());
    }

    // ---- changes to a numbered line ----
    if let Some(n) = cli.edit {
        let mut store = Store::load(&path)?;
        let i = store.index_of(n).ok_or_else(|| format!("no task at line {n}"))?;
        let old = store.lines[i].clone();
        let initial = if text.is_empty() { old.clone() } else { text.clone() };
        match editor::edit_line(&format!("{n}> "), &initial, ops::use_color())? {
            None => println!("Cancelled."),
            Some(new) => {
                let new = norm(new.trim(), today);
                if new.is_empty() {
                    println!("Empty text: nothing changed (use --delete {n} to remove the task).");
                } else if new == old {
                    println!("Unchanged.");
                } else {
                    store.lines[i] = new.clone();
                    store.save(&path)?;
                    println!("{n}: {new}");
                }
            }
        }
        return Ok(());
    }

    if let Some(n) = cli.change {
        if text.trim().is_empty() {
            return Err("--change needs the new text: todo -c NUMBER TEXT".into());
        }
        let mut store = Store::load(&path)?;
        let i = store.index_of(n).ok_or_else(|| format!("no task at line {n}"))?;
        let new = norm(text.trim(), today);
        store.lines[i] = new.clone();
        store.save(&path)?;
        println!("{n}: {new}");
        return Ok(());
    }

    if let Some(n) = cli.priority {
        let arg = text.trim().trim_matches(|c| c == '(' || c == ')');
        let prio = match arg {
            "-" | "" | "none" | "off" => None,
            a if a.chars().count() == 1 && a.chars().all(|c| c.is_ascii_alphabetic()) => {
                Some(a.chars().next().unwrap().to_ascii_uppercase())
            }
            _ => return Err("--priority needs a letter A-Z (or - to remove): todo -p NUMBER A".into()),
        };
        if text.trim().is_empty() {
            return Err("--priority needs a letter A-Z (or - to remove): todo -p NUMBER A".into());
        }
        let mut store = Store::load(&path)?;
        let i = store.index_of(n).ok_or_else(|| format!("no task at line {n}"))?;
        if todotxt::parse(&store.lines[i]).done {
            return Err(format!("line {n} is done; reopen it first (todo -x {n})").into());
        }
        let new = todotxt::set_priority(&store.lines[i], prio);
        store.lines[i] = new.clone();
        store.save(&path)?;
        println!("{n}: {new}");
        return Ok(());
    }

    if let Some(n) = cli.delete {
        let mut store = Store::load(&path)?;
        let i = store.index_of(n).ok_or_else(|| format!("no task at line {n}"))?;
        let gone = store.lines.remove(i);
        store.save(&path)?;
        folder::remove_note(&path, folder.as_deref(), &gone, &store.lines);
        println!("Deleted {n}: {gone}");
        return Ok(());
    }

    if let Some(n) = cli.done {
        let mut store = Store::load(&path)?;
        let i = store.index_of(n).ok_or_else(|| format!("no task at line {n}"))?;
        let old = &store.lines[i];
        let new = if todotxt::parse(old).done { todotxt::reopen(old) } else { todotxt::complete(old, &today_s) };
        store.lines[i] = new.clone();
        store.save(&path)?;
        println!("{n}: {new}");
        return Ok(());
    }

    // ---- listing ----
    let listing = cli.list.is_some() || cli.text || cli.plain || (cli.sort.is_some() && text.is_empty());
    if listing {
        // filter = --list VALUE plus any trailing words (todo -l +Home @phone)
        let expr = format!("{} {}", cli.list.as_deref().unwrap_or(""), text);
        let filter = list::Filter::from_expr(&expr);
        let store = Store::load(&path)?;
        let style = if cli.plain {
            Style::Plain
        } else if cli.text {
            Style::Numbered
        } else {
            Style::Pretty
        };
        // --text / --plain show the file as it is unless a sort order is requested
        let sort = if style == Style::Pretty || sort_arg.is_some() { default_sort } else { Sort::File };
        ops::print_list(&store.lines, sort, style, &file_name, folder.as_deref(), &filter, cli.all, cli.verbose);
        return Ok(());
    }

    // ---- add and close ----
    if !text.trim().is_empty() {
        let mut store = Store::load(&path)?;
        let line = todotxt::with_creation_date(&norm(text.trim(), today), &today_s);
        store.lines.push(line.clone());
        store.save(&path)?;
        println!("Added as line {}: {line}", store.lines.len());
        return Ok(());
    }

    // ---- no arguments: interactive mode ----
    tui::run(path, folder, default_sort)?;
    Ok(())
}

/// Typed text -> task text: `(a)` -> `(A)`, `due:tomorrow` -> `due:YYYY-MM-DD`.
fn norm(text: &str, today: chrono::NaiveDate) -> String {
    todotxt::normalize_due(&todotxt::fix_priority_case(text), today)
}

/// Commits (and pushes) the folder when it is dropped at the end of the run, if the todo file changed
/// meanwhile and `.todo-git.cfg` has git support and auto on.
struct AutoCommit {
    folder: Option<PathBuf>,
    auto: bool,
    path: PathBuf,
    before: Option<String>,
    always: bool,
}

impl AutoCommit {
    fn new(folder: Option<&std::path::Path>, path: &std::path::Path, always: bool) -> AutoCommit {
        AutoCommit {
            folder: folder.filter(|d| d.join(".git").exists()).map(|d| d.to_path_buf()),
            auto: folder.is_some_and(|d| git::active_cfg(d).is_some()),
            path: path.to_path_buf(),
            before: std::fs::read_to_string(path).ok(),
            always,
        }
    }
}

impl Drop for AutoCommit {
    fn drop(&mut self) {
        let Some(dir) = &self.folder else { return };
        if !self.always && std::fs::read_to_string(&self.path).ok() == self.before {
            return;
        }
        if self.auto {
            let name = self.path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            match git::auto_change(dir, &format!("todo: update {name}"), false, true) {
                Ok(m) if !m.is_empty() => eprintln!("todo: {m}"),
                Err(e) => eprintln!("todo: git: {e}"),
                _ => {}
            }
        }
        // after adding / deleting / changing: show where the repository stands
        if let Some(st) = git::status(dir) {
            eprintln!("todo: git: {}", st.label());
        }
    }
}

/// `todo -v` alone: folder, file, git state, program version and the enabled options.
fn print_info(cfg: &Config, path: &std::path::Path, folder: Option<&std::path::Path>, sort: Sort) {
    let onoff = |b: Option<bool>, default: bool| if b.unwrap_or(default) { "on" } else { "off" };
    println!("todo {}", env!("TODO_VERSION_FULL"));
    println!("config:     {}", config::config_path().display());
    match folder {
        Some(d) => println!("folder:     {}", d.display()),
        None => println!("folder:     - (single file mode)"),
    }
    println!("file:       {}", path.display());
    if cfg.webdav.is_empty() {
        println!("webdav:     - (none; todo --webdav-login USER@URL)");
    }
    for e in &cfg.webdav {
        let ok = webdav::Server::parse(e).is_some_and(|s| webdav::cred::exists(&s));
        println!("webdav:     {e} ({})", if ok { "password stored" } else { "NO password" });
    }
    match Store::load(path) {
        Ok(store) => {
            let tasks: Vec<_> = store.lines.iter().filter(|l| !l.trim().is_empty()).map(|l| todotxt::parse(l)).collect();
            let done = tasks.iter().filter(|t| t.done).count();
            println!("tasks:      {} open, {done} done, {} total", tasks.len() - done, tasks.len());
        }
        Err(e) => println!("tasks:      (cannot read: {e})"),
    }
    if let Some(d) = folder {
        let files = folder::list_txt_files(d);
        println!("lists:      {} *.txt file(s) in the folder", files.len());
    }
    println!("notes:      {}", folder::note_dir(folder).display());
    println!("sort:       {sort:?}");
    println!(
        "options:    showdone {} · sidebar {} · calendar {} · statusbar {}",
        onoff(cfg.show_done, false),
        onoff(cfg.sidebar, true),
        onoff(cfg.calendar, true),
        onoff(cfg.statusbar, true)
    );
    if !cfg.files.is_empty() {
        println!("known files: {}", cfg.files.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "));
    }
    match folder {
        None => println!("git:        - (needs a todo folder)"),
        Some(d) => match git::status(d) {
            None => println!("git:        - (folder is not a git repository)"),
            Some(st) => {
                println!("git:        {}", st.label());
                if !st.summary.is_empty() {
                    println!("  commit:   {}", st.summary);
                }
                match git::GitCfg::load(d) {
                    Some(gc) => println!("  settings: {}", gc.describe()),
                    None => println!("  settings: no {} (no automatic git actions; todo --git-init)", git::CFG_NAME),
                }
            }
        },
    }
}
