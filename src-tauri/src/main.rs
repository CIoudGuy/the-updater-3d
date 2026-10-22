// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().collect();
    // The same exe doubles as the elevated install helper.
    if args.iter().any(|a| a == "--worker" || a == "--worker-task") {
        std::process::exit(updater_lib::elevate::worker_main(&args));
    }
    updater_lib::run();
}
