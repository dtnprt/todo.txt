include!("../tools/buildinfo.rs");

fn main() {
    emit_buildinfo();
    tauri_build::build()
}
