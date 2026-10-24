// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // The same exe doubles as the elevated install helper.
    if args.iter().any(|a| a == "--worker" || a == "--worker-task") {
        std::process::exit(updater_lib::elevate::worker_main(&args));
    }
    // ...and as the elevated driver installer (Windows Update installs only for admins).
    if args.iter().any(|a| a == updater_lib::sources::drivers::INSTALL_FLAG) {
        std::process::exit(updater_lib::sources::drivers::install_main(&args));
    }
    // ...and as a one-page render check (`--render-page <url> <out file>`).
    if args.iter().any(|a| a == "--render-page") {
        std::process::exit(updater_lib::render::render_main(&args));
    }
    updater_lib::run();
}
