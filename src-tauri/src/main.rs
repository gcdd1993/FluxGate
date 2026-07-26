// Keep release builds free of the Windows console window; debug builds keep
// it so `cargo tauri dev` still shows logs.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    fluxgate_lib::run();
}
