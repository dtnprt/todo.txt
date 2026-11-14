include!("../tools/buildinfo.rs");

/// Android cannot read bundled resource files, so the themes (`../themes/*.css`) are compiled into the library there:
/// writes `$OUT_DIR/embedded_themes.rs` with `EMBEDDED_THEMES: &[(name, css)]` (empty on the other platforms).
fn embed_themes() {
    println!("cargo:rerun-if-changed=../themes");
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("embedded_themes.rs");
    let android = std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|t| t == "android");
    let mut src = String::from("pub const EMBEDDED_THEMES: &[(&str, &str)] = &[\n");
    if android {
        let dir = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir")).join("../themes");
        let mut files: Vec<_> = std::fs::read_dir(&dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
        files.sort();
        for p in files {
            if p.extension().is_some_and(|x| x == "css") {
                let name = p.file_stem().unwrap().to_string_lossy().into_owned();
                let abs = std::fs::canonicalize(&p).unwrap_or(p).to_string_lossy().replace('\\', "/").trim_start_matches("//?/").to_string();
                src.push_str(&format!("    ({name:?}, include_str!({abs:?})),\n"));
            }
        }
    }
    src.push_str("];\n");
    std::fs::write(out, src).expect("write embedded_themes.rs");
}

fn main() {
    emit_buildinfo();
    embed_themes();
    // Android 15+ devices with 16 KB pages: align the ELF LOAD segments accordingly
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|t| t == "android") {
        println!("cargo:rustc-link-arg=-Wl,-z,max-page-size=16384");
    }
    tauri_build::build()
}
