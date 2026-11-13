// No console window on Windows, in debug builds too (the debug binary is started by double click;
// run it from a terminal with `cargo run` / see stderr by temporarily removing this line).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    todotxt_lib::run()
}
