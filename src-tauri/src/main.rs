#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // The containment launcher shares this binary. This check must run before
    // Tauri starts so a launcher never opens a window.
    if let Some(exit_code) = jevcode_lib::run_containment_launcher_if_requested() {
        std::process::exit(exit_code);
    }
    jevcode_lib::run();
}
