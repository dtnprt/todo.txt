// Shared by cli/build.rs and src-tauri/build.rs (`include!`): exports the version for `env!()`.
//   TODO_VERSION       0.1.12            (from ../VERSION, counted up by build.ps1 for every build)
//   TODO_HASH          a1b2c3d           (current commit; a trailing * = uncommitted changes)
//   TODO_VERSION_FULL  0.1.12 (a1b2c3d)
fn emit_buildinfo() {
    use std::path::PathBuf;
    use std::process::Command;

    let root = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into())).join("..");
    let version = std::fs::read_to_string(root.join("VERSION"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "0.0.0".into());
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(&root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    let mut hash = git(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "nogit".into());
    if git(&["status", "--porcelain"]).is_some_and(|s| !s.is_empty()) {
        hash.push('*');
    }
    println!("cargo:rustc-env=TODO_VERSION={version}");
    println!("cargo:rustc-env=TODO_HASH={hash}");
    println!("cargo:rustc-env=TODO_VERSION_FULL={version} ({hash})");
    println!("cargo:rerun-if-changed=../VERSION");
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/logs/HEAD");
}
