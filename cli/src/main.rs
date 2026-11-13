//! `todo` - a todo.txt command line tool.
//!
//!   todo (A) call mom +Family @phone   add a task and exit
//!   todo -a                            add a task in an interactive line editor
//!   todo                               interactive terminal UI on the last used file
//!   todo -l | -t | -P                  list / numbered text / plain text
//!   todo -p 3 A                        set priority of line 3 (todo -p 3 - removes it)
//!   todo -e 3 | -c 3 TEXT | -d 3       edit / change / delete line 3

mod backup;
mod config;
mod crypt;
mod davcache;
mod editor;
mod folder;
mod textarea;
mod git;
mod highlight;
mod input;
mod list;
mod ops;
mod store;
mod switcher;
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
        (remembered in ~/.todo.txt/todo.cfg). Text without a switch is added as a new task.",
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

    /// Add the new task as a sub-entry of line NUMBER (one level deeper: `- `, `-- ` ... up to `----`), below that entry
    #[arg(short = 'u', long, value_name = "NUMBER")]
    parent: Option<usize>,

    /// Move all tasks matching FILTER (such as "+Project @ctx") with their sub-entries from SOURCE to DEST (created if missing)
    #[arg(long = "move", num_args = 3, value_names = ["FILTER", "SOURCE", "DEST"])]
    move_tasks: Option<Vec<String>>,

    /// With --move: only show what would be moved
    #[arg(long)]
    dry_run: bool,

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

    /// Edit the note of line NUMBER in a multi-line input (Enter = new line, Ctrl+Enter saves); adds `note:NAME` if missing
    #[arg(short = 'D', long, value_name = "NUMBER")]
    note: Option<usize>,

    /// WebDAV: add the server USER@URL and store its password in the OS keyring (asks for it; or set TODO_WEBDAV_PASSWORD)
    #[arg(long, value_name = "USER@URL")]
    webdav_login: Option<String>,

    /// WebDAV: forget the server USER@URL (removes it and its password)
    #[arg(long, value_name = "USER@URL")]
    webdav_logout: Option<String>,

    /// WebDAV: fast local cache (`~/.todo.txt/cache/webdav`, synced in the background) on|off; used by the TUI and the desktop app
    #[arg(long, value_name = "on|off")]
    webdav_cache: Option<String>,

    /// WebDAV: list the files and folders at URL (without URL: the folder of the current todo.txt, or the first server)
    #[arg(long, value_name = "URL", num_args = 0..=1, default_missing_value = "")]
    webdav_ls: Option<String>,

    /// EXPERIMENTAL encryption (AES-256-GCM): on|off. Converts the open todo file / folder (lists, notes, done.txt) and encrypts every later save; asks for a password
    #[arg(long, value_name = "on|off")]
    encrypt: Option<String>,

    /// Encryption: set the password (asks twice) and keep it in the OS keyring
    #[arg(long)]
    crypt_login: bool,

    /// Encryption: forget the password stored in the OS keyring
    #[arg(long)]
    crypt_logout: bool,

    /// Encryption: change the password and re-encrypt the files
    #[arg(long)]
    crypt_passwd: bool,

    /// Encryption: ask for Windows Hello (biometrics / PIN) before the stored password is used on|off
    #[arg(long, value_name = "on|off")]
    crypt_hello: Option<String>,

    /// Local backup as a .zip of the open file / folder (lists, notes, images, done.txt; encrypted files stay encrypted). Without FILE: `~/.todo.txt/backups/todo-backup-DATE.zip`
    #[arg(long, value_name = "FILE.zip", num_args = 0..=1, default_missing_value = "")]
    backup: Option<String>,

    /// Encryption of THIS list only: on|off (an exception in the `.todotxt.cfg` of its folder, e.g. to keep one list readable by other apps); converts the file
    #[arg(long, value_name = "on|off")]
    encrypt_file: Option<String>,

    /// List the known folders (folders with a hidden `.todo.txt`) with their encryption settings
    #[arg(long)]
    folders: bool,

    /// List the recent files (the last 20 opened by any client, known or not)
    #[arg(long)]
    recent: bool,

    /// List the known files (`files=`) with their kind (cloud / git / local) and whether they are encrypted
    #[arg(long)]
    files: bool,

    /// With -f: remember the folder of the file (creates a hidden `.todo.txt` folder there and adds the file to the known files)
    #[arg(short = 'K', long)]
    keep: bool,

    /// Switch the todo file: without VALUE a menu of known files / folder / WebDAV servers; VALUE = list number, path, URL or part of a known path. Remembered.
    #[arg(short = 'F', long, value_name = "NUMBER|PATH|URL", num_args = 0..=1, default_missing_value = "")]
    switch: Option<String>,

    /// Use this todo.txt (a path, or a WebDAV URL https://...), or a whole FOLDER of *.txt files (and remember it)
    #[arg(short = 'f', long, value_name = "FILE|FOLDER")]
    file: Option<PathBuf>,

    /// Text of the task to add (or the new text for --change)
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, value_name = "TEXT")]
    words: Vec<String>,
}

/// Is this file one the CLI may switch to on its own (see `lastfile=`)?
fn is_known(cfg: &Config, p: &std::path::Path) -> bool {
    let norm = |p: &std::path::Path| p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase();
    webdav::is_remote(&p.to_string_lossy())
        || cfg.files.iter().any(|f| norm(f) == norm(p))
        || cfg.file.as_ref().is_some_and(|f| norm(f) == norm(p))
        || cfg.folder.as_ref().is_some_and(|d| folder::is_inside(d, p))
        || p.parent().is_some_and(crypt::has_todo_dir)   // a local file is known once its folder has a `.todo.txt`
}

/// ("cloud" | "git" | "local", encrypted?) of a known file.
fn file_kind(p: &std::path::Path) -> (&'static str, Option<bool>) {
    let t = p.to_string_lossy();
    if webdav::is_remote(&t) {
        return ("cloud", davcache::cached_encrypted(&t));
    }
    let git = p.parent().is_some_and(|d| d.join(".git").exists());
    (if git { "git" } else { "local" }, crypt::file_state(p))
}

fn crypt_switches(cli: &Cli) -> bool {
    cli.encrypt.is_some() || cli.encrypt_file.is_some() || cli.files || cli.recent || cli.folders || cli.crypt_login || cli.crypt_logout || cli.crypt_passwd || cli.crypt_hello.is_some()
}

fn parse_on_off(name: &str, v: &str) -> Result<bool, String> {
    match v.to_lowercase().as_str() {
        "on" | "true" | "1" | "yes" => Ok(true),
        "off" | "false" | "0" | "no" => Ok(false),
        _ => Err(format!("{name} takes on or off")),
    }
}

/// A new password, asked twice.
fn new_password(what: &str) -> Result<String, Box<dyn Error>> {
    let a = store::prompt_password(&format!("New {what} password: ")).ok_or("no password given (or no terminal to ask on)")?;
    let b = store::prompt_password("Repeat the password: ").ok_or("no password given")?;
    if a != b {
        return Err("the passwords differ".into());
    }
    Ok(a)
}

/// The password of the encrypted files: stored / session one, else asked for.
fn current_password() -> Result<String, Box<dyn Error>> {
    match crypt::resolve_password() {
        Ok(p) => Ok(p),
        Err(_) => store::prompt_password("Current password: ").ok_or_else(|| "no password given (or no terminal to ask on)".into()),
    }
}

/// `--encrypt`, `--crypt-login`, `--crypt-logout`, `--crypt-passwd`, `--crypt-hello`.
fn crypt_cmds(cli: &Cli, cfg: &mut Config, path: &std::path::Path, folder: Option<&std::path::Path>) -> Result<(), Box<dyn Error>> {
    let remote = webdav::is_remote(&path.to_string_lossy());
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).unwrap_or_default();
    // what is converted: the local lists + notes (folder mode: the folder; else the file, its done.txt and the single-file notes)
    let convert = |old: Option<&str>, new: Option<&str>| -> Result<usize, Box<dyn Error>> {
        let mut n = 0;
        match folder {
            Some(d) if !folder::is_remote_dir(d) => n += crypt::convert_dir(d, old, new)?,
            _ => {
                if !remote {
                    if path.exists() && crypt::convert_file(path, old, new)? {
                        n += 1;
                    }
                    let done = path.with_file_name("done.txt");
                    if done.exists() && crypt::convert_file(&done, old, new)? {
                        n += 1;
                    }
                }
                if let Some(dir) = path.parent().filter(|d| crypt::has_todo_dir(d)) {
                    let t = crypt::todo_dir(dir);
                    n += crypt::convert_flat(&t.join("notes"), old, new)? + crypt::convert_images(&t.join("images"), old, new)?;
                }
                n += crypt::convert_flat(&home.join(".todo.txt").join("notes"), old, new)?;
                n += crypt::convert_images(&home.join(".todo.txt").join("images"), old, new)?;
            }
        }
        Ok(n)
    };
    if let Some(v) = &cli.crypt_hello {
        let on = parse_on_off("--crypt-hello", v)?;
        cfg.set_other("encrypt_hello", if on { "true" } else { "false" });
        cfg.save()?;
        println!("Windows Hello before using the stored password: {}", if on { "on" } else { "off" });
        if on && !crypt::hello_available() {
            eprintln!("note: Windows Hello is not available here (Windows only, needs a set-up PIN/fingerprint/face); the setting has no effect until it is");
        }
    }
    if cli.crypt_logout {
        crypt::forget_stored_password()?;
        crypt::set_session(None);
        println!("password removed from the keyring");
    }
    if cli.crypt_login {
        let pw = new_password("encryption")?;
        crypt::store_password(&pw)?;
        crypt::set_session(Some(pw));
        println!("password stored in the keyring");
    }
    if cli.crypt_passwd {
        let old = current_password()?;
        let new = new_password("encryption")?;
        let n = if crypt::enabled() { convert(Some(&old), Some(&new))? } else { 0 };
        if remote && crypt::enabled() {
            let store = store::Store::load(path)?;
            crypt::set_session(Some(new.clone()));
            store.save(path)?;
        }
        if crypt::has_stored_password() {
            crypt::store_password(&new)?;
        }
        crypt::set_session(Some(new));
        println!("password changed ({n} file(s) re-encrypted)");
    }
    if cli.folders {
        let mut dirs: Vec<PathBuf> = Vec::new();
        let mut add = |d: PathBuf| {
            if !dirs.contains(&d) {
                dirs.push(d);
            }
        };
        for p in cfg.files.iter().chain(cfg.recent.iter()).chain(cfg.file.iter()) {
            if !webdav::is_remote(&p.to_string_lossy()) {
                if let Some(d) = p.parent() {
                    add(d.to_path_buf());
                }
            }
        }
        if let Some(f) = &cfg.folder {
            if !folder::is_remote_dir(f) {
                add(f.clone());
            }
        }
        let mut n = 0;
        for d in dirs.into_iter().filter(|d| crypt::has_todo_dir(d)) {
            let p = crypt::policy_of_dir(&d);
            let git = if d.join(".git").exists() { "git" } else { "local" };
            let default = match p.default { Some(true) => "encrypt all", Some(false) => "plain", None => "no default" };
            let mut extra = String::new();
            if !p.plain.is_empty() { extra.push_str(&format!(" · not encrypted: {}", p.plain.join(", "))); }
            if !p.encrypted.is_empty() { extra.push_str(&format!(" · encrypted: {}", p.encrypted.join(", "))); }
            println!("{git:<6} {} ({default}{extra})", d.display());
            n += 1;
        }
        if n == 0 {
            println!("no known folders yet (a folder becomes known when it has a hidden .todo.txt folder: todo -f FILE --keep)");
        }
    }
    if cli.recent {
        for (i, p) in cfg.recent.iter().enumerate() {
            let (kind, _) = file_kind(p);
            println!("{:>2}  {kind:<6} {}", i + 1, p.display());
        }
        if cfg.recent.is_empty() {
            println!("no recent files yet");
        }
    }
    if cli.files {
        let mut all: Vec<PathBuf> = cfg.files.clone();
        if let Some(f) = &cfg.file {
            if !all.contains(f) {
                all.push(f.clone());
            }
        }
        for p in all {
            let (kind, enc) = file_kind(&p);
            let state = match enc {
                Some(true) => "encrypted",
                Some(false) => "plain",
                None => "?",
            };
            println!("{kind:<6} {state:<9} {}{}", p.display(), if p == path { "   <- current" } else { "" });
        }
    }
    // the folder (or, for a single file, its directory / the server folder) that holds the policy file `.todotxt.cfg`
    let set_dir_policy = |on: bool| -> Result<(), Box<dyn Error>> {
        let target: Option<String> = match folder {
            Some(d) if folder::is_remote_dir(d) => Some(d.to_string_lossy().into_owned()),
            Some(d) => {
                crypt::set_dir_policy(d, on)?;
                None
            }
            None if remote => Some(webdav::parent(&path.to_string_lossy())),
            None => {
                if let Some(d) = path.parent() {
                    crypt::set_dir_policy(d, on)?;
                }
                None
            }
        };
        if let Some(url) = target {
            let server = store::server_of(std::path::Path::new(&url))?;
            let mut p = webdav::remote_policy(&server, &url);
            p.set_default(on);
            webdav::write_remote_policy(&server, &url, &p).map_err(|e| e.to_string())?;
        }
        Ok(())
    };
    if let Some(v) = &cli.encrypt_file {
        let on = parse_on_off("--encrypt-file", v)?;
        let pw = match crypt::resolve_password() {
            Ok(p) => p,
            Err(_) if on => new_password("encryption")?,
            Err(_) => current_password()?,
        };
        crypt::set_session(Some(pw.clone()));
        if remote {
            let server = store::server_of(path)?;
            let text = store::Store::load(path)?;
            webdav::set_remote_file_policy(&server, &path.to_string_lossy(), on).map_err(|e| e.to_string())?;
            text.save(path)?;
        } else {
            crypt::set_file_policy(path, on)?;
            if on { crypt::convert_file(path, Some(&pw), Some(&pw))?; } else { crypt::convert_file(path, Some(&pw), None)?; }
        }
        eprintln!("note: encryption is experimental - keep a backup (todo --backup)");
        println!("{}: {}", path.display(), if on { "encrypted" } else { "stored as plain text (readable by other apps)" });
    }
    if let Some(v) = &cli.encrypt {
        let on = parse_on_off("--encrypt", v)?;
        if on {
            let pw = match crypt::resolve_password() {
                Ok(p) => p,
                Err(_) => new_password("encryption")?,
            };
            crypt::set_session(Some(pw.clone()));
            crypt::set_enabled_override(Some(true));
            set_dir_policy(true)?;
            let n = convert(Some(&pw), Some(&pw))?;
            if remote {
                store::Store::load(path)?.save(path)?;
            }
            cfg.set_other("encrypt", "true");
            cfg.save()?;
            eprintln!("note: encryption is experimental - keep a backup (todo --backup) of your files. Other todo.txt apps, editors and scripts can no longer read encrypted files");
            println!("encryption on: {n} file(s) encrypted{}", if remote { " (the remote list is encrypted on its next save; its notes stay plain)" } else { "" });
            if !crypt::has_stored_password() {
                println!("tip: todo --crypt-login keeps the password in the keyring so you are not asked every time");
            }
        } else {
            let pw = current_password()?;
            crypt::set_session(Some(pw.clone()));
            crypt::set_enabled_override(Some(false));
            set_dir_policy(false)?;
            let n = convert(Some(&pw), None)?;
            if remote {
                store::Store::load(path)?.save(path)?;
            }
            cfg.set_other("encrypt", "false");
            cfg.save()?;
            println!("encryption off: {n} file(s) decrypted");
        }
    }
    Ok(())
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
            // the folder the current todo file lies in (if it is on a WebDAV server), else the first server
            let cur = cfg.lastfile.clone().or_else(|| cfg.file.clone()).map(|p| p.to_string_lossy().into_owned());
            match cur.filter(|c| webdav::is_remote(c)) {
                Some(c) => webdav::parent(&c),
                None => cfg.webdav.first().and_then(|e| webdav::Server::parse(e)).map(|s| s.url).ok_or("no WebDAV server configured (todo --webdav-login USER@URL)")?,
            }
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
    store::allow_prompt(true);
    let mut cfg = Config::load();

    // WebDAV server management (no todo file involved)
    if let Some(v) = &cli.webdav_cache {
        let on = match v.to_lowercase().as_str() {
            "on" | "true" | "1" | "yes" => true,
            "off" | "false" | "0" | "no" => false,
            _ => return Err("--webdav-cache takes on or off".into()),
        };
        cfg.set_other("davcache", if on { "true" } else { "false" });
        cfg.save()?;
        println!("WebDAV cache: {}", if on { "on" } else { "off" });
        if cli.webdav_login.is_none() && cli.webdav_logout.is_none() && cli.webdav_ls.is_none() {
            return Ok(());
        }
    }
    if cli.webdav_login.is_some() || cli.webdav_logout.is_some() || cli.webdav_ls.is_some() {
        return webdav_cmds(&cli, &mut cfg);
    }

    // move tasks between two files, then exit
    if let Some(v) = &cli.move_tasks {
        return move_tasks(&v[0], &v[1], &v[2], cli.dry_run);
    }
    if cli.dry_run {
        return Err("--dry-run only works with --move".into());
    }

    // file switcher (menu / number / path), then exit
    if let Some(arg) = &cli.switch {
        let current = cfg.file.clone().unwrap_or_else(config::default_todo_file);
        return switcher::run(arg, &current);
    }

    // Git: pull before anything is read (only with .todo-git.cfg: enabled, auto and pull on)
    let git_switches = cli.git_init || cli.sync || cli.git.is_some() || cli.git_auto.is_some() || !cli.git_set.is_empty();
    let early_dir = match &cli.file {
        Some(f) if webdav::is_remote(&f.to_string_lossy()) => None,
        Some(f) => std::path::absolute(f).ok().filter(|p| p.is_dir()),
        None => cfg.folder.clone().filter(|d| d.is_dir()), // (a WebDAV folder has no git)
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
        && cli.priority.is_none()
        && cli.parent.is_none()
        && cli.backup.is_none()
        && !crypt_switches(&cli);
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
    // the file used last (any client) wins over `file=` at start; `file=` stays the remembered default
    // `lastfile=` is the file the other clients switched to: a WebDAV file is always followed, a local / git file only when it is
    // in the list of known files (Settings -> Known files, `todo --files`), in the folder, or the remembered `file=`
    let last = cfg.lastfile.clone().filter(|l| is_known(&cfg, l)).or_else(|| cfg.file.clone());
    let last_is_remote = last.as_ref().is_some_and(|f| webdav::is_remote(&f.to_string_lossy()));
    let last_outside_folder = match (&cfg.folder, &last) {
        (Some(d), Some(l)) => folder::is_folder(d) && !folder::is_inside(d, l) && (last_is_remote || l.exists()),
        _ => false,
    };
    let path = match &cli.file {
        Some(f) if folder::is_remote_dir(f) => {
            folder = Some(f.clone());
            folder::pick_file(f, last.as_deref())
        }
        Some(f) if webdav::is_remote(&f.to_string_lossy()) => f.clone(),
        Some(f) => {
            let f = std::path::absolute(f)?;
            if f.is_dir() {
                folder = Some(f.clone());
                folder::pick_file(&f, last.as_deref())
            } else {
                f
            }
        }
        // the last file was on a WebDAV server: use it (also when a folder= is configured from earlier)
        None if last_outside_folder || (last_is_remote && cfg.folder.is_none()) => last.clone().unwrap(),
        None => match cfg.folder.clone().filter(|d| folder::is_folder(d)) {
            Some(d) => {
                folder = Some(d.clone());
                folder::pick_file(&d, last.as_deref())
            }
            None => last.clone().unwrap_or_else(config::default_todo_file),
        },
    };
    // `-f FILE` outside the folder of the remembered file (e.g. a todo.txt in another git repository) is used for
    // this run only: not stored as `file=`, not added to the known files. Use `todo -F PATH` or `--keep` to remember it.
    let path_is_remote = webdav::is_remote(&path.to_string_lossy());
    if cli.keep && !path_is_remote && folder.is_none() {
        // "remember this folder": a hidden `.todo.txt` in the folder of the list makes it a known folder
        if let Some(dir) = path.parent() {
            crypt::ensure_todo_dir(dir)?;
        }
    }
    let one_off = cli.file.is_some()
        && folder.is_none()
        && !path_is_remote            // a WebDAV file is always remembered
        && !cli.keep
        && !path.parent().is_some_and(crypt::has_todo_dir)   // local files are remembered once their folder has a `.todo.txt`
        && match &cfg.file {
            Some(last) => switcher::dir_of(&last.to_string_lossy()) != switcher::dir_of(&path.to_string_lossy()) && path.exists(),
            None => false,
        };
    // started from `lastfile=` without -f: nothing was chosen, so `file=` / `folder=` stay as they are
    let remote_last = cli.file.is_none() && cfg.lastfile.is_some();
    if !one_off && !remote_last && (cfg.file.as_ref() != Some(&path) || cfg.folder != folder || (folder.is_none() && !cfg.files.contains(&path))) {
        cfg.file = Some(path.clone());
        cfg.folder = folder.clone();
        if folder.is_none() && !cfg.files.contains(&path) {
            cfg.files.push(path.clone()); // known files, for the switcher (todo -F)
        }
        let _ = cfg.save(); // remembering the file is a convenience, never fatal
    }
    // the recent files list gets every file that is opened, known or not
    if !info_only {
        cfg.push_recent(&path);
        let _ = cfg.save();
    }
    // `lastfile=` is what the other clients follow: a one-off file (another folder, unknown to them) stays out of it
    if !one_off && cfg.lastfile.as_ref() != Some(&path) {
        cfg.lastfile = Some(path.clone());
        let _ = cfg.save();
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
    let _dav = if tui_mode { None } else { Some(DavNote(path.clone())) };
    let _auto = if tui_mode { None } else { Some(AutoCommit::new(folder.as_deref(), &path, cli.note.is_some())) };

    // ---- backup ----
    if let Some(dest) = &cli.backup {
        let dest = if dest.is_empty() { None } else { Some(PathBuf::from(dest)) };
        let (zip, n) = backup::create(&path, folder.as_deref(), dest.as_deref())?;
        println!("backup: {} ({n} file(s))", zip.display());
        return Ok(());
    }

    // ---- encryption switches ----
    if crypt_switches(&cli) {
        return crypt_cmds(&cli, &mut cfg, &path, folder.as_deref());
    }

    // ---- note ----
    if let Some(n) = cli.note {
        let mut store = Store::load(&path)?;
        let i = store.index_of(n).ok_or_else(|| format!("no task at line {n}"))?;
        let t = todotxt::parse(&store.lines[i]);
        let name = match folder::note_name(&t) {
            Some(name) => name,
            None => {
                let name = folder::next_note_name(&store.lines, folder::notes_root(&path, folder.as_deref()).as_deref());
                store.lines[i] = format!("{} note:{name}", store.lines[i]);
                store.save(&path)?;
                println!("{n}: {}", store.lines[i]);
                name
            }
        };
        let file = folder::prepare_note(&path, folder.as_deref(), &name)?;
        folder::edit_note_inline(&file, &name)?;
        folder::finish_note(&file)?;
        return Ok(());
    }

    // ---- interactive add ----
    if cli.add {
        let nroot = folder::notes_root(&path, folder.as_deref());
        let ctx = folder::NoteCtx { todo: &path, folder: nroot.as_deref(), lines: Store::load(&path)?.lines };
        match editor::edit_line("add> ", &text, ops::use_color(), Some(&ctx))? {
            None => println!("Cancelled."),
            Some(new) => {
                let new = new.trim();
                if new.is_empty() {
                    println!("Empty text: nothing added.");
                } else {
                    let mut store = Store::load(&path)?;
                    let line = todotxt::with_creation_date(&norm(new, today), &today_s);
                    let n = ops::insert_task(&mut store.lines, line, cli.parent)?;
                    store.save(&path)?;
                    println!("Added as line {n}: {}", store.lines[n - 1]);
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
        let nroot = folder::notes_root(&path, folder.as_deref());
        let ctx = folder::NoteCtx { todo: &path, folder: nroot.as_deref(), lines: store.lines.clone() };
        match editor::edit_line(&format!("{n}> "), &initial, ops::use_color(), Some(&ctx))? {
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
        let p_str = path.to_string_lossy().into_owned();
        let header = if p_str.starts_with("http://") || p_str.starts_with("https://") { tui::dav_label(&p_str) } else { file_name.clone() };
        ops::print_list(&store.lines, sort, style, &file_name, &header, folder.as_deref(), &filter, cli.all, cli.verbose);
        return Ok(());
    }

    // ---- add and close ----
    if !text.trim().is_empty() {
        let mut store = Store::load(&path)?;
        let line = todotxt::with_creation_date(&norm(text.trim(), today), &today_s);
        let n = ops::insert_task(&mut store.lines, line, cli.parent)?;
        store.save(&path)?;
        println!("Added as line {n}: {}", store.lines[n - 1]);
        return Ok(());
    }
    if cli.parent.is_some() {
        return Err("--parent needs a task text (todo -u 5 TEXT) or -a (todo -a -u 5)".into());
    }

    // ---- no arguments: interactive mode ----
    tui::run(path, folder, default_sort)?;
    Ok(())
}

/// `todo --move FILTER SOURCE DEST [--dry-run]`: the tasks matching FILTER (all terms: `+x`, `@x`, `(A)`, words)
/// plus their sub-entries are removed from SOURCE and appended to DEST. DEST is written first.
fn move_tasks(filter: &str, source: &str, dest: &str, dry: bool) -> Result<(), Box<dyn Error>> {
    let terms: Vec<String> = filter.split_whitespace().map(str::to_string).collect();
    if terms.is_empty() {
        return Err("--move needs a filter, e.g. todo --move \"+Project\" source.txt destination.txt".into());
    }
    let to_path = |s: &str| -> Result<PathBuf, Box<dyn Error>> {
        if webdav::is_remote(s) { Ok(PathBuf::from(s)) } else { Ok(std::path::absolute(s)?) }
    };
    let (src, dst) = (to_path(source)?, to_path(dest)?);
    if src == dst {
        return Err("source and destination are the same file".into());
    }
    let remote = |p: &std::path::Path| webdav::is_remote(&p.to_string_lossy());
    if !remote(&src) && !src.exists() {
        return Err(format!("source file not found: {}", src.display()).into());
    }
    let _commit_src = AutoCommit::new(src.parent().filter(|_| !remote(&src)), &src, false);
    let _commit_dst = AutoCommit::new(dst.parent().filter(|_| !remote(&dst)), &dst, false);
    let mut store = Store::load(&src)?;
    let idx = list::select_for_move(&store.lines, &terms);
    let moved: Vec<String> = idx.iter().map(|&i| store.lines[i].clone()).collect();
    println!("{} {} line(s) from {} to {}", if dry { "Would move" } else { "Moving" }, moved.len(), src.display(), dst.display());
    for &i in &idx {
        println!("{:>4}  {}", i + 1, store.lines[i]);
    }
    if moved.is_empty() || dry {
        return Ok(());
    }
    if ops::has_attachments(&moved) && src.parent() != dst.parent() {
        eprintln!("todo: some of these tasks have note: or image: files; those files are not moved");
    }
    ops::append_lines(&dst, &moved)?; // destination first: nothing is lost if it fails
    let gone: std::collections::HashSet<usize> = idx.into_iter().collect();
    let mut i = 0;
    store.lines.retain(|_| { i += 1; !gone.contains(&(i - 1)) });
    store.save(&src)?;
    println!("Done.");
    Ok(())
}

/// Typed text -> task text: `(a)` -> `(A)`, `due:tomorrow` -> `due:YYYY-MM-DD`.
fn norm(text: &str, today: chrono::NaiveDate) -> String {
    todotxt::normalize_due(&todotxt::fix_priority_case(text), today)
}

/// Commits (and pushes) the folder when it is dropped at the end of the run, if the todo file changed
/// meanwhile and `.todo-git.cfg` has git support and auto on.
/// At the end of a CLI run: say so when the WebDAV server was not reachable (offline cache in use).
struct DavNote(PathBuf);

impl Drop for DavNote {
    fn drop(&mut self) {
        if !davcache::is_offline() {
            return;
        }
        let url = self.0.to_string_lossy();
        match davcache::pending_changes(&url) {
            Some((r, a)) => eprintln!("todo: WebDAV not reachable: changes kept in the offline cache ({r} removed, {a} added), synced on the next run when the server is back"),
            None => eprintln!("todo: WebDAV not reachable: showing the cached copy"),
        }
    }
}

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
    println!("notes:      {}", folder::note_dir(folder::notes_root(path, folder).as_deref()).display());
    println!("backups:    {} (before archiving: {})", backup::backup_dir().display(), onoff(Some(backup::archive_backup_enabled()), true));
    println!(
        "encryption: {} · password {} · Windows Hello {}",
        if crypt::enabled() { "on (AES-256-GCM, experimental)" } else { "off (experimental feature)" },
        if crypt::has_stored_password() { "in the keyring" } else { "not stored (todo --crypt-login)" },
        if crypt::hello_enabled() { if crypt::hello_available() { "on" } else { "on, but not available" } } else { "off" }
    );
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
