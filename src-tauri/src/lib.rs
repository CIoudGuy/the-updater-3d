pub mod commands;
pub mod core;
pub mod download;
pub mod elevate;
pub mod http;
pub mod model;
pub mod notes;
pub mod paths;
pub mod procs;
pub mod selfupdate;
pub mod services;
pub mod sources;
pub mod store;
pub mod tray;
pub mod version;
pub mod winutil;

use std::sync::Arc;
use tauri::Manager;

static LOGGER: store::FileLog = store::FileLog;

pub fn run() {
    let _ = log::set_logger(&LOGGER).map(|_| log::set_max_level(log::LevelFilter::Info));
    let minimized = std::env::args().any(|a| a == "--minimized");

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            tray::show_main(app);
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            let core = core::Core::new(app.handle().clone());
            app.manage(core.clone());
            tray::create(app.handle())?;
            // Checking in the background at sign-in is on by default: set it up once.
            {
                let mut p = core.persisted.lock();
                if !p.autostart_applied {
                    // Also turns it on for copies that saved the old default (off).
                    use tauri_plugin_autostart::ManagerExt;
                    let mut s = core.settings.write();
                    s.launch_at_startup = true;
                    store::save_settings(&s);
                    let _ = app.autolaunch().enable();
                    p.autostart_applied = true;
                    store::save_state(&p);
                }
            }
            // Started by Windows at sign-in: stay a tiny background checker, no window.
            if !minimized {
                tray::show_main(app.handle());
            }
            tauri::async_runtime::spawn(core.clone().jobs_ticker());
            tauri::async_runtime::spawn(core.clone().queue_loop());
            tauri::async_runtime::spawn(core.clone().scheduler());
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let core = app.state::<Arc<core::Core>>();
                store::save_state(&core.persisted.lock());
                let busy = core.jobs().iter().any(|j| !j.state.is_finished());
                if core.settings.read().close_to_tray || busy {
                    // Close the window for real so its web view frees its memory; the checker keeps running.
                    let _ = api;
                } else {
                    app.exit(0);
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_snapshot,
            commands::check_now,
            commands::update_apps,
            commands::update_all,
            commands::install_app,
            commands::install_version,
            commands::uninstall_app,
            commands::repair_app,
            commands::open_app,
            commands::show_in_folder,
            commands::get_versions,
            commands::get_catalog,
            commands::pause_job,
            commands::resume_job,
            commands::cancel_job,
            commands::retry_job,
            commands::reorder_jobs,
            commands::pause_all,
            commands::resume_all,
            commands::clear_finished_jobs,
            commands::close_and_continue,
            commands::get_activity,
            commands::export_activity,
            commands::open_log,
            commands::get_services,
            commands::stop_service,
            commands::start_service,
            commands::stop_safe_services,
            commands::open_creative_cloud,
            commands::set_settings,
            commands::pick_folder,
            commands::clear_cache,
            commands::show_main,
            commands::install_self_update,
            commands::test_notification,
            commands::catalog_versions,
            commands::quit_app,
        ])
        .build(tauri::generate_context!())
        .expect("error while building The Updater 3D")
        .run(|app, event| {
            // Closing the last window keeps the background checker alive; Quit exits for real.
            match &event {
                tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
                tauri::RunEvent::Exit => {
                    if let Some(core) = app.try_state::<Arc<core::Core>>() {
                        store::save_state(&core.persisted.lock());
                    }
                }
                _ => {}
            }
        });
}
