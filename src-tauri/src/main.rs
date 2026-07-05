// Deliberately thin. The mobile entry point and all real setup live in
// lib.rs — see https://v2.tauri.app/start/project-structure/ for why:
// mobile builds compile the app as a library and load it through the
// platform framework, so main.rs can't hold anything load-bearing.
fn main() {
    openmind_desktop_lib::run();
}
