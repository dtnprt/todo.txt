//! Git integration (libgit2 via `git2`, credentials via `auth-git2`: ssh-agent, ~/.ssh keys,
//! git credential helpers, optional password prompt).
//!
//! This file is shared: the CLI/TUI use it as `mod git`, the desktop app includes it with `#[path]`.
//! It must not use anything from the rest of the crate.
//!
//! Works on a dedicated todo **folder** that is a git repository (only that exact directory is
//! opened, never a parent repository). Automatic actions are controlled by `.todo-git.cfg` in the
//! folder (created by `--git-init`); **without that file nothing happens automatically**:
//!
//! ```text
//! enabled=true   # git support on/off (off: no git action at all, not even a manual sync)
//! auto=false     # automatic actions on/off (master switch for the three below)
//! pull=true      # pull when todo starts / the folder is opened
//! commit=true    # commit after every change
//! push=true      # push after every change (in the background in TUI / desktop app)
//! ```
//!
//! The settings file is added to `.git/info/exclude`, so every machine opts in on its own.

#![allow(dead_code)]

use auth_git2::GitAuthenticator;
use git2::{build::CheckoutBuilder, Commit, IndexAddOption, MergeOptions, Repository, Signature, StatusOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

type Res<T> = Result<T, String>;

pub const CFG_NAME: &str = ".todo-git.cfg";
/// Names accepted by [`GitCfg::set`].
pub const KEYS: [&str; 5] = ["enabled", "auto", "pull", "commit", "push"];

fn e(err: git2::Error) -> String {
    err.message().to_string()
}

// ------------------------------------------------------------------ settings file

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitCfg {
    pub enabled: bool,
    pub auto: bool,
    pub pull: bool,
    pub commit: bool,
    pub push: bool,
}

impl Default for GitCfg {
    fn default() -> Self {
        GitCfg { enabled: true, auto: false, pull: true, commit: true, push: true }
    }
}

impl GitCfg {
    /// `None` if the folder has no `.todo-git.cfg` (then no automatic git action is allowed).
    pub fn load(dir: &Path) -> Option<GitCfg> {
        let text = std::fs::read_to_string(dir.join(CFG_NAME)).ok()?;
        let mut c = GitCfg::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let on = matches!(v.trim().to_lowercase().as_str(), "true" | "1" | "yes" | "on");
            let _ = c.set(k.trim(), on);
        }
        Some(c)
    }

    pub fn save(&self, dir: &Path) -> std::io::Result<()> {
        let text = format!(
            "# todo git settings (this machine only; listed in .git/info/exclude)\n\
             # without this file todo never runs git on its own\n\
             enabled={}\nauto={}\npull={}\ncommit={}\npush={}\n",
            self.enabled, self.auto, self.pull, self.commit, self.push
        );
        std::fs::write(dir.join(CFG_NAME), text)
    }

    pub fn get(&self, key: &str) -> Option<bool> {
        Some(match key {
            "enabled" => self.enabled,
            "auto" => self.auto,
            "pull" => self.pull,
            "commit" => self.commit,
            "push" => self.push,
            _ => return None,
        })
    }

    pub fn set(&mut self, key: &str, on: bool) -> Res<()> {
        match key {
            "enabled" => self.enabled = on,
            "auto" => self.auto = on,
            "pull" => self.pull = on,
            "commit" => self.commit = on,
            "push" => self.push = on,
            _ => return Err(format!("unknown git setting '{key}' (use {})", KEYS.join(", "))),
        }
        Ok(())
    }

    /// Automatic actions are allowed: git support and auto are both on.
    pub fn active(&self) -> bool {
        self.enabled && self.auto
    }

    pub fn describe(&self) -> String {
        let f = |b: bool| if b { "on" } else { "off" };
        format!(
            "git {} · auto {} (pull {}, commit {}, push {})",
            f(self.enabled),
            f(self.auto),
            f(self.pull),
            f(self.commit),
            f(self.push)
        )
    }
}

/// The settings if automatic actions are allowed here: repository, settings file, enabled and auto.
pub fn active_cfg(dir: &Path) -> Option<GitCfg> {
    if !dir.join(".git").exists() {
        return None;
    }
    GitCfg::load(dir).filter(GitCfg::active)
}

// ------------------------------------------------------------------ status line

/// What the status bar shows for a todo folder that is a git repository.
#[derive(Clone, Debug)]
pub struct Status {
    /// Current branch ("(detached)" when HEAD is detached, "(no commits)" in a fresh repository).
    pub branch: String,
    /// Short hash of the current commit ("" without commits).
    pub commit: String,
    /// First line of the commit message.
    pub summary: String,
    /// Number of changed / untracked files in the working tree.
    pub changed: usize,
    pub ahead: usize,
    pub behind: usize,
}

impl Status {
    /// "main a1b2c3d · 2 changed · ↑1 ↓0" / "main a1b2c3d · clean".
    pub fn label(&self) -> String {
        let mut s = if self.commit.is_empty() { self.branch.clone() } else { format!("{} {}", self.branch, self.commit) };
        s.push_str(&if self.changed == 0 { " · clean".to_string() } else { format!(" · {} changed", self.changed) });
        if self.ahead > 0 || self.behind > 0 {
            s.push_str(&format!(" · ↑{} ↓{}", self.ahead, self.behind));
        }
        s
    }
}

/// Status of the repository at exactly `dir` (None if it is not one).
pub fn status(dir: &Path) -> Option<Status> {
    if !dir.join(".git").exists() {
        return None;
    }
    let repo = Repository::open(dir).ok()?;
    let mut st = Status { branch: "(no commits)".into(), commit: String::new(), summary: String::new(), changed: 0, ahead: 0, behind: 0 };
    if let Ok(head) = repo.head() {
        st.branch = if head.is_branch() { head.shorthand().unwrap_or("?").to_string() } else { "(detached)".into() };
        if let Ok(c) = head.peel_to_commit() {
            st.commit = c.id().to_string().chars().take(7).collect();
            st.summary = c.message().unwrap_or("").lines().next().unwrap_or("").to_string();
            if let Some(up) = head.is_branch().then(|| git2::Branch::wrap(head)).and_then(|b| b.upstream().ok()) {
                if let Some(uid) = up.get().target() {
                    if let Ok((a, b)) = repo.graph_ahead_behind(c.id(), uid) {
                        st.ahead = a;
                        st.behind = b;
                    }
                }
            }
        }
    }
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    st.changed = repo.statuses(Some(&mut opts)).map(|s| s.len()).unwrap_or(0);
    Some(st)
}

// ------------------------------------------------------------------ repository basics

fn open(dir: &Path) -> Res<Repository> {
    Repository::open(dir).map_err(|_| format!("{} is not a git repository (todo --git-init)", dir.display()))
}

fn signature(repo: &Repository) -> Res<Signature<'static>> {
    repo.signature().or_else(|_| Signature::now("todo", "todo@localhost")).map_err(e)
}

fn is_dirty(repo: &Repository) -> Res<bool> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true);
    Ok(!repo.statuses(Some(&mut opts)).map_err(e)?.is_empty())
}

/// Create the repository (if needed) in `dir`, write `.todo-git.cfg` (if missing), keep that file out
/// of the repository and commit what is there.
pub fn init(dir: &Path) -> Res<String> {
    let existed = Repository::open(dir).is_ok();
    let repo = match Repository::open(dir) {
        Ok(r) => r,
        Err(_) => Repository::init(dir).map_err(e)?,
    };
    let mut notes: Vec<&str> = Vec::new();
    if GitCfg::load(dir).is_none() {
        GitCfg::default().save(dir).map_err(|x| x.to_string())?;
        notes.push("settings file created (automatic actions are off: todo --git-auto on)");
    }
    let exclude = repo.path().join("info").join("exclude");
    let current = std::fs::read_to_string(&exclude).unwrap_or_default();
    if !current.lines().any(|l| l.trim() == CFG_NAME) {
        if let Some(parent) = exclude.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&exclude, format!("{current}{}{CFG_NAME}\n", if current.is_empty() || current.ends_with('\n') { "" } else { "\n" }));
    }
    if !existed && commit_all_repo(&repo, "todo: initial commit")? {
        notes.push("initial commit made");
    }
    let head = if existed { "already a git repository" } else { "created a git repository" };
    Ok(if notes.is_empty() { head.to_string() } else { format!("{head}, {}", notes.join(", ")) })
}

/// `git add -A && git commit`; false if there was nothing to commit.
pub fn commit_all(dir: &Path, msg: &str) -> Res<bool> {
    commit_all_repo(&open(dir)?, msg)
}

fn commit_all_repo(repo: &Repository, msg: &str) -> Res<bool> {
    if !is_dirty(repo)? {
        return Ok(false);
    }
    let mut index = repo.index().map_err(e)?;
    index.add_all(["*"].iter(), IndexAddOption::DEFAULT, None).map_err(e)?;
    index.update_all(["*"].iter(), None).map_err(e)?; // also record deletions
    index.write().map_err(e)?;
    let tree = repo.find_tree(index.write_tree().map_err(e)?).map_err(e)?;
    let sig = signature(repo)?;
    let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
    if let Some(p) = &parent {
        if p.tree_id() == tree.id() {
            return Ok(false);
        }
    }
    let parents: Vec<&Commit> = parent.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parents).map_err(e)?;
    Ok(true)
}

fn authenticator(interactive: bool) -> GitAuthenticator {
    if interactive {
        GitAuthenticator::default()
    } else {
        // inside the TUI / desktop app nothing may read from the terminal
        GitAuthenticator::default().try_password_prompt(0).prompt_ssh_key_password(false)
    }
}

fn branch_name(repo: &Repository) -> Res<String> {
    let head = repo.head().map_err(|_| "no commits yet".to_string())?;
    Ok(head.shorthand().map_err(|_| "detached HEAD".to_string())?.to_string())
}

/// Fetch `origin` and fast-forward / merge it into the current branch. The work tree must be clean.
/// `Ok(None)`: no remote (or nothing on the remote yet); otherwise what happened.
fn fetch_merge(repo: &Repository, branch: &str, auth: &GitAuthenticator) -> Res<Option<&'static str>> {
    let Ok(mut remote) = repo.find_remote("origin") else { return Ok(None) };
    auth.fetch(repo, &mut remote, &[], None).map_err(|x| format!("fetch failed: {}", e(x)))?;
    let Ok(up) = repo.find_reference(&format!("refs/remotes/origin/{branch}")) else { return Ok(None) };
    let theirs = repo.reference_to_annotated_commit(&up).map_err(e)?;
    let (analysis, _) = repo.merge_analysis(&[&theirs]).map_err(e)?;
    if analysis.is_fast_forward() {
        let mut r = repo.find_reference(&format!("refs/heads/{branch}")).map_err(e)?;
        r.set_target(theirs.id(), "todo: fast-forward").map_err(e)?;
        repo.set_head(&format!("refs/heads/{branch}")).map_err(e)?;
        repo.checkout_head(Some(CheckoutBuilder::new().force())).map_err(e)?;
        Ok(Some("pulled"))
    } else if analysis.is_normal() {
        repo.merge(&[&theirs], Some(&mut MergeOptions::new()), Some(CheckoutBuilder::new().safe())).map_err(e)?;
        let mut index = repo.index().map_err(e)?;
        if index.has_conflicts() {
            repo.cleanup_state().map_err(e)?;
            repo.checkout_head(Some(CheckoutBuilder::new().force())).map_err(e)?;
            return Err("local and remote changes conflict: merge them with git, nothing was changed".into());
        }
        let tree = repo.find_tree(index.write_tree_to(repo).map_err(e)?).map_err(e)?;
        let sig = signature(repo)?;
        let mine = repo.head().and_then(|h| h.peel_to_commit()).map_err(e)?;
        let other = repo.find_commit(theirs.id()).map_err(e)?;
        repo.commit(Some("HEAD"), &sig, &sig, "todo: merge", &tree, &[&mine, &other]).map_err(e)?;
        repo.cleanup_state().map_err(e)?;
        repo.checkout_head(Some(CheckoutBuilder::new().force())).map_err(e)?;
        Ok(Some("merged"))
    } else {
        Ok(None)
    }
}

fn push_repo(repo: &Repository, branch: &str, auth: &GitAuthenticator) -> Res<bool> {
    let Ok(mut remote) = repo.find_remote("origin") else { return Ok(false) };
    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    auth.push(repo, &mut remote, &[&refspec]).map_err(|x| format!("push failed: {}", e(x)))?;
    Ok(true)
}

fn require_enabled(dir: &Path) -> Res<()> {
    match GitCfg::load(dir) {
        Some(c) if !c.enabled => Err("git support is off (todo --git on)".into()),
        _ => Ok(()),
    }
}

/// Manual sync: commit, fetch, fast-forward or merge, push. Returns a one-line summary.
pub fn sync(dir: &Path, interactive: bool) -> Res<String> {
    require_enabled(dir)?;
    let repo = open(dir)?;
    let mut parts: Vec<String> = Vec::new();
    if commit_all_repo(&repo, "todo: update")? {
        parts.push("committed".into());
    }
    let branch = branch_name(&repo)?;
    if repo.find_remote("origin").is_err() {
        parts.push("no remote 'origin' configured (git remote add origin URL)".into());
        return Ok(parts.join(", "));
    }
    let auth = authenticator(interactive);
    if let Some(what) = fetch_merge(&repo, &branch, &auth)? {
        parts.push(what.into());
    }
    if push_repo(&repo, &branch, &auth)? {
        parts.push("pushed".into());
    }
    Ok(parts.join(", "))
}

// ------------------------------------------------------------------ automatic actions

/// On start / folder open, if `pull` is configured: pull before the data is read. A dirty work tree
/// is committed first (if `commit` is on), otherwise the pull is skipped. Empty string = nothing to say.
pub fn auto_start(dir: &Path, interactive: bool) -> Res<String> {
    let Some(cfg) = active_cfg(dir) else { return Ok(String::new()) };
    if !cfg.pull {
        return Ok(String::new());
    }
    let repo = open(dir)?;
    if is_dirty(&repo)? {
        if cfg.commit {
            commit_all_repo(&repo, "todo: update")?;
        } else {
            return Err("uncommitted changes: pull skipped".into());
        }
    }
    let branch = branch_name(&repo)?;
    let auth = authenticator(interactive);
    Ok(fetch_merge(&repo, &branch, &auth)?.map(|w| format!("git: {w}")).unwrap_or_default())
}

/// After a change: commit and push as configured. With `background` the push runs in a thread and
/// its outcome is available from [`take_result`]. Empty string = nothing to say.
pub fn auto_change(dir: &Path, msg: &str, background: bool, interactive: bool) -> Res<String> {
    let Some(cfg) = active_cfg(dir) else { return Ok(String::new()) };
    if cfg.commit {
        commit_all(dir, msg)?;
    }
    if cfg.push {
        if background {
            push_background(dir.to_path_buf());
        } else {
            let repo = open(dir)?;
            let branch = branch_name(&repo)?;
            if push_repo(&repo, &branch, &authenticator(interactive))? {
                return Ok("git: pushed".into());
            }
        }
    }
    Ok(String::new())
}

static PUSH_BUSY: AtomicBool = AtomicBool::new(false);
static PUSH_PENDING: AtomicBool = AtomicBool::new(false);
static RESULT: Mutex<Option<String>> = Mutex::new(None);

/// Push in a worker thread; changes made meanwhile are covered by one more push afterwards.
fn push_background(dir: PathBuf) {
    PUSH_PENDING.store(true, Ordering::SeqCst);
    if PUSH_BUSY.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::spawn(move || loop {
        while PUSH_PENDING.swap(false, Ordering::SeqCst) {
            let r = open(&dir).and_then(|repo| {
                let branch = branch_name(&repo)?;
                push_repo(&repo, &branch, &authenticator(false))
            });
            let msg = match r {
                Ok(true) => Some("git: pushed".to_string()),
                Ok(false) => None,
                Err(x) => Some(format!("git: {x}")),
            };
            if let (Some(m), Ok(mut slot)) = (msg, RESULT.lock()) {
                *slot = Some(m);
            }
        }
        PUSH_BUSY.store(false, Ordering::SeqCst);
        if !(PUSH_PENDING.load(Ordering::SeqCst) && !PUSH_BUSY.swap(true, Ordering::SeqCst)) {
            break;
        }
    });
}

/// Leave a message for the UI (shown by the TUI, e.g. the outcome of the pull at start).
pub fn post(msg: &str) {
    if let Ok(mut slot) = RESULT.lock() {
        *slot = Some(msg.to_string());
    }
}

/// The outcome of the last background push, once.
pub fn take_result() -> Option<String> {
    RESULT.lock().ok().and_then(|mut s| s.take())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("todo-git-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn lf(p: &Path) -> String {
        fs::read_to_string(p).unwrap().replace("\r\n", "\n")
    }

    #[test]
    fn settings_file() {
        let d = tmp("cfg");
        assert!(GitCfg::load(&d).is_none());
        assert!(active_cfg(&d).is_none());
        let mut c = GitCfg::default();
        c.set("auto", true).unwrap();
        c.set("push", false).unwrap();
        assert!(c.set("nope", true).is_err());
        c.save(&d).unwrap();
        assert_eq!(GitCfg::load(&d), Some(c));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn no_settings_file_means_no_automatic_action() {
        let d = tmp("noauto");
        fs::write(d.join("todo.txt"), "one\n").unwrap();
        Repository::init(&d).unwrap();
        assert_eq!(auto_change(&d, "x", false, false).unwrap(), "");
        assert_eq!(auto_start(&d, false).unwrap(), "");
        assert!(Repository::open(&d).unwrap().head().is_err()); // still no commit
        // with the file but git support off: still nothing
        let mut c = GitCfg::default();
        c.auto = true;
        c.enabled = false;
        c.save(&d).unwrap();
        assert_eq!(auto_change(&d, "x", false, false).unwrap(), "");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn init_commit_sync_between_two_clones() {
        let base = tmp("sync");
        let remote = base.join("remote.git");
        Repository::init_bare(&remote).unwrap();
        let url = remote.to_string_lossy().into_owned();

        let a = base.join("a");
        fs::create_dir_all(&a).unwrap();
        fs::write(a.join("todo.txt"), "one\n").unwrap();
        init(&a).unwrap();
        assert!(a.join(CFG_NAME).exists());
        // the settings file is not part of the repository
        let r = Repository::open(&a).unwrap();
        assert!(r.head().unwrap().peel_to_tree().unwrap().get_name(CFG_NAME).is_none());
        r.remote("origin", &url).unwrap();
        assert!(sync(&a, false).unwrap().contains("pushed"));

        // a second working copy: clone, change, sync
        let b = base.join("b");
        Repository::clone(&url, &b).unwrap();
        fs::write(b.join("todo.txt"), "one\ntwo\n").unwrap();
        assert!(sync(&b, false).unwrap().contains("committed"));

        // the first one pulls it (fast-forward)
        assert!(sync(&a, false).unwrap().contains("pulled"));
        assert_eq!(lf(&a.join("todo.txt")), "one\ntwo\n");

        // diverging, non-conflicting changes in different files are merged
        fs::write(a.join("x.txt"), "x\n").unwrap();
        fs::write(b.join("y.txt"), "y\n").unwrap();
        sync(&b, false).unwrap();
        let out = sync(&a, false).unwrap();
        assert!(out.contains("merged"), "{out}");
        assert!(a.join("y.txt").exists());

        // conflicting edits of the same line are refused and leave the file alone
        fs::write(a.join("todo.txt"), "one\nA\n").unwrap();
        sync(&a, false).unwrap();
        fs::write(b.join("todo.txt"), "one\nB\n").unwrap();
        commit_all(&b, "b").unwrap();
        assert!(sync(&b, false).is_err());
        assert_eq!(lf(&b.join("todo.txt")), "one\nB\n");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn auto_pull_on_start_and_auto_commit_push() {
        let base = tmp("auto");
        let remote = base.join("remote.git");
        Repository::init_bare(&remote).unwrap();
        let url = remote.to_string_lossy().into_owned();
        let a = base.join("a");
        fs::create_dir_all(&a).unwrap();
        fs::write(a.join("todo.txt"), "one\n").unwrap();
        init(&a).unwrap();
        Repository::open(&a).unwrap().remote("origin", &url).unwrap();
        let mut c = GitCfg::load(&a).unwrap();
        c.auto = true;
        c.save(&a).unwrap();

        // a change: committed and pushed (synchronously here)
        fs::write(a.join("todo.txt"), "one\ntwo\n").unwrap();
        assert_eq!(auto_change(&a, "todo: update todo.txt", false, false).unwrap(), "git: pushed");

        // another machine pushes; starting todo on `a` pulls first
        let b = base.join("b");
        Repository::clone(&url, &b).unwrap();
        fs::write(b.join("todo.txt"), "one\ntwo\nthree\n").unwrap();
        commit_all(&b, "b").unwrap();
        let rb = Repository::open(&b).unwrap();
        let br = branch_name(&rb).unwrap();
        push_repo(&rb, &br, &authenticator(false)).unwrap();
        assert_eq!(auto_start(&a, false).unwrap(), "git: pulled");
        assert_eq!(lf(&a.join("todo.txt")), "one\ntwo\nthree\n");

        // pull switched off: nothing happens
        let mut c = GitCfg::load(&a).unwrap();
        c.pull = false;
        c.save(&a).unwrap();
        assert_eq!(auto_start(&a, false).unwrap(), "");
        let _ = fs::remove_dir_all(&base);
    }
}
