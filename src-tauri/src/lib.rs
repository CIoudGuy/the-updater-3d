pub mod commands;
pub mod core;
pub mod download;
pub mod elevate;
pub mod http;
pub mod install_detail;
pub mod machine;
pub mod model;
pub mod notes;
pub mod notice;
pub mod paths;
pub mod prefs;
pub mod procs;
pub mod render;
pub mod selfupdate;
pub mod services;
pub mod sources;
pub mod store;
pub mod tray;
pub mod uninstall;
pub mod version;
pub mod winutil;

use std::sync::Arc;
use tauri::Manager;

static LOGGER: store::FileLog = store::FileLog;

/// One log line for a crash, so a user can send the log and the cause is in it.
fn crash_line(msg: &str, at: Option<String>) -> String {
    format!("CRASH in {} {}: {msg}", env!("CARGO_PKG_VERSION"), at.unwrap_or_else(|| "unknown place".into()))
}

/// Write every panic (with a backtrace) to the app log before the app goes down: release builds
/// have no console, so without this a crash leaves nothing behind.
fn log_crashes() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let msg = info.payload().downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| info.payload().downcast_ref::<String>().cloned()).unwrap_or_default();
        let at = info.location().map(|l| format!("at {}:{}", l.file(), l.line()));
        log::error!("{}
{}", crash_line(&msg, at), std::backtrace::Backtrace::force_capture());
        log::logger().flush();
        default(info);
    }));
}

/// The app's config and assets (one copy, for the app and the `--render-page` check).
pub fn context() -> tauri::Context<tauri::Wry> {
    tauri::generate_context!()
}

pub fn run() {
    let _ = log::set_logger(&LOGGER).map(|_| log::set_max_level(log::LevelFilter::Info));
    log_crashes();
    log::info!("The Updater 3D {} starting", env!("CARGO_PKG_VERSION"));
    let minimized = std::env::args().any(|a| a == "--minimized");
    // Started by a click on a notification while no copy was running: open on its page.
    let launch_route = notice::route_in_args(&std::env::args().collect::<Vec<_>>());

    tauri::Builder::default()
        // A second start (the app again, or a click on a notification) hands its arguments over
        // and ends: show the window, on the page a notification link asks for.
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            tray::open_route(app, notice::route_in_args(&args));
        }))
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--minimized"])))
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            render::set_app(app.handle().clone());
            let core = core::Core::new(app.handle().clone());
            app.manage(core.clone());
            tray::create(app.handle())?;
            // Before the one-time default changes below mark this copy as started once.
            let first_start = !core.persisted.lock().autostart_applied;
            // Checking in the background at sign-in is on by default: set it up once.
            {
                let mut p = core.persisted.lock();
                let pending = !(p.silent_elevation_default_on && p.other_apps_default_on && p.prereleases_default_on && p.keep_previous_default_off && p.autostart_applied);
                if pending {
                    let mut s = core.settings.write();
                    let autostart = store::apply_default_changes(&mut p, &mut s);
                    store::save_settings(&s);
                    store::save_state(&p);
                    if autostart {
                        use tauri_plugin_autostart::ManagerExt;
                        let _ = app.autolaunch().enable();
                    }
                }
            }
            // Adobe on or off from the installer's question (once; Settings can change it later).
            {
                let answer = store::installer_adobe_choice();
                let mut p = core.persisted.lock();
                let mut s = core.settings.write();
                if store::apply_adobe_choice(&mut p, &mut s, answer.as_deref(), first_start, sources::adobe::cc_installed) {
                    log::info!("Adobe apps {} (installer answer: {})", if s.sources.adobe { "on" } else { "off" }, answer.as_deref().unwrap_or("none"));
                    store::save_settings(&s);
                    store::save_state(&p);
                }
            }
            // Keep the Windows startup entry in step with the setting (an uninstall or a cleanup
            // tool can remove it while the setting stays on).
            {
                use tauri_plugin_autostart::ManagerExt;
                let want = core.settings.read().launch_at_startup;
                let m = app.autolaunch();
                if want && !m.is_enabled().unwrap_or(false) {
                    let _ = m.enable();
                }
            }
            // Notifications open a link this exe handles.
            notice::register_link_handler();
            // Started by Windows at sign-in: stay a tiny background checker, no window.
            if launch_route.is_some() {
                tray::open_route(app.handle(), launch_route);
            } else if !minimized {
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
            // The window is gone: free what only it used.
            if let tauri::WindowEvent::Destroyed = event {
                let app = window.app_handle().clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                    if app.get_webview_window("main").is_none() {
                        app.state::<Arc<core::Core>>().trim_memory();
                    }
                });
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
            commands::get_catalog_entry,
            commands::prefs_preview,
            commands::window_ready,
            commands::allow_admin,
            commands::open_logs_folder,
            commands::take_route,
            commands::close_and_continue,
            commands::get_activity,
            commands::export_activity,
            commands::open_log,
            commands::get_services,
            commands::stop_service,
            commands::start_service,
            commands::stop_safe_services,
            commands::open_creative_cloud,
            commands::open_store_page,
            commands::set_settings,
            commands::pick_folder,
            commands::clear_cache,
            commands::show_main,
            commands::install_self_update,
            commands::test_notification,
            commands::catalog_versions,
            commands::browse_other_apps,
            commands::quit_app,
            commands::leftover_scan,
            commands::leftover_delete,
            commands::leftover_restore,
            commands::leftover_backups,
            commands::open_driver_page,
            commands::check_self_update,
            commands::restart_pc,
        ])
        .build(context())
        .expect("error while building The Updater 3D")
        .run(|app, event| {
            // Closing the last window keeps the background checker alive; Quit exits for real.
            match &event {
                tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
                tauri::RunEvent::Exit => {
                    if let Some(core) = app.try_state::<Arc<core::Core>>() {
                        core.save_queue_now();
                        store::save_state(&core.persisted.lock());
                    }
                }
                _ => {}
            }
        });
}

#[cfg(test)]
mod crash_tests {
    #[test]
    fn a_crash_line_names_the_version_place_and_reason() {
        let l = super::crash_line("there is no reactor running", Some("at src\\core.rs:1043".into()));
        assert!(l.starts_with(&format!("CRASH in {} at src", env!("CARGO_PKG_VERSION"))), "{l}");
        assert!(l.ends_with(": there is no reactor running"));
    }
}
