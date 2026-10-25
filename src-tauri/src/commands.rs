//! Commands the UI calls. Names and arguments match src/lib/commands.ts.

use crate::core::{Core, JobRequest};
use crate::model::*;
use crate::services::{self, Action};
use crate::{paths, store, winutil};
use std::sync::Arc;
use tauri::{AppHandle, Manager, State};

type Res<T> = Result<T, String>;
type C<'a> = State<'a, Arc<Core>>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub fn get_snapshot(core: C<'_>) -> Snapshot {
    core.snapshot()
}

#[tauri::command]
pub async fn check_now(core: C<'_>) -> Res<()> {
    let c = core.inner().clone();
    c.check_now(true).await.map_err(err)
}

#[tauri::command]
pub fn update_apps(core: C<'_>, app_ids: Vec<String>) -> Res<()> {
    let c = core.inner();
    for id in app_ids {
        c.enqueue(JobRequest::Update { app_id: id, version: None }).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub fn update_all(core: C<'_>) -> Res<()> {
    let c = core.inner();
    // A BIOS is updated from its maker's page, not here.
    let ids: Vec<String> = c.apps().into_iter().filter(|a| a.status == AppStatus::Update && a.manual_url.is_none()).map(|a| a.id).collect();
    if ids.is_empty() {
        c.toast("info", "Everything is current", None);
    }
    for id in ids {
        c.enqueue(JobRequest::Update { app_id: id, version: None }).map_err(err)?;
    }
    Ok(())
}

#[tauri::command]
pub fn install_app(core: C<'_>, id: String, version: Option<String>) -> Res<()> {
    core.inner().enqueue(JobRequest::Install { id, version }).map(|_| ()).map_err(err)
}

#[tauri::command]
pub fn install_version(core: C<'_>, app_id: String, version: String) -> Res<()> {
    core.inner().enqueue(JobRequest::Update { app_id, version: Some(version) }).map(|_| ()).map_err(err)
}

/// The screen the tray or a clicked notification asked for, once (the window asks on start and on
/// each "navigate").
#[tauri::command]
pub fn take_route(core: C<'_>) -> Option<String> {
    core.inner().pending_route.lock().take()
}

/// "Allow admin rights" in the Queue: ask once more and continue the jobs waiting for it.
#[tauri::command]
pub fn allow_admin(core: C<'_>) {
    core.inner().allow_admin();
}

/// The window opened: set up "Fewer admin prompts" if it is on but not ready yet.
#[tauri::command]
pub fn window_ready(core: C<'_>) {
    core.inner().ensure_silent_task();
}

#[tauri::command]
pub async fn prefs_preview(core: C<'_>, app_id: String) -> Res<Vec<crate::model::PrefsItem>> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || c.prefs_preview(&app_id)).await.map_err(err)?.map_err(err)
}

#[tauri::command]
pub fn uninstall_app(core: C<'_>, app_id: String, remove_prefs: Option<bool>) -> Res<()> {
    core.inner().enqueue(JobRequest::Uninstall { app_id, remove_prefs: remove_prefs.unwrap_or(false) }).map(|_| ()).map_err(err)
}

#[tauri::command]
pub fn repair_app(core: C<'_>, app_id: String) -> Res<()> {
    core.inner().enqueue(JobRequest::Repair { app_id }).map(|_| ()).map_err(err)
}

#[tauri::command]
pub fn open_app(core: C<'_>, app_id: String) -> Res<()> {
    let i = core.installed(&app_id).ok_or("That app is no longer installed")?;
    // A Store app starts through the shell, which gives it its package identity.
    if let Some(t) = i.store.as_ref().and_then(|s| s.launch_target()) {
        return winutil::shell_open(&t).map_err(err);
    }
    let exe = i.exe.ok_or("Could not find the program file")?;
    winutil::launch(&exe, &[]).map_err(err)
}

/// Open a Microsoft Store copy's page in the Store app, where it is updated or removed; or the
/// Store page of a Get apps entry that only the Store installs ("other:<package id>").
#[tauri::command]
pub fn open_store_page(core: C<'_>, app_id: String) -> Res<()> {
    let s = match core.installed(&app_id) {
        Some(i) => i.store.ok_or("This copy is not from the Microsoft Store")?,
        None => crate::sources::other::store_ref_of(&app_id).ok_or("This app is not in the Microsoft Store")?,
    };
    winutil::shell_open(&s.page_url()).map_err(err)
}

#[tauri::command]
pub fn show_in_folder(core: C<'_>, app_id: String) -> Res<()> {
    let i = core.installed(&app_id).ok_or("That app is no longer installed")?;
    match (i.exe.filter(|e| e.exists()), i.install_dir.filter(|d| d.exists())) {
        (Some(e), _) => winutil::show_in_folder(&e).map_err(err),
        (None, Some(d)) => winutil::show_in_folder(&d).map_err(err),
        _ => Err("The install folder is gone".into()),
    }
}

#[tauri::command]
pub async fn get_versions(core: C<'_>, app_id: String) -> Res<Vec<VersionInfo>> {
    let c = core.inner().clone();
    c.versions(&app_id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub fn get_catalog(core: C<'_>) -> Vec<CatalogEntry> {
    core.get_apps()
}

#[tauri::command]
pub async fn get_catalog_entry(core: C<'_>, id: String) -> Result<Option<CatalogEntry>, String> {
    // Off the window's thread: an Other apps entry may read the catalog index from disk.
    let core = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || core.catalog_entry(&id).or_else(|| core.other_entry(&id))).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub fn pause_job(core: C<'_>, job_id: String) {
    core.pause(&job_id)
}

#[tauri::command]
pub fn resume_job(core: C<'_>, job_id: String) {
    core.resume(&job_id)
}

#[tauri::command]
pub fn cancel_job(core: C<'_>, job_id: String) -> Res<()> {
    core.cancel(&job_id).map_err(err)
}

#[tauri::command]
pub fn retry_job(core: C<'_>, job_id: String) -> Res<()> {
    core.inner().retry(&job_id).map_err(err)
}

#[tauri::command]
pub fn reorder_jobs(core: C<'_>, job_ids: Vec<String>) {
    core.reorder(&job_ids)
}

#[tauri::command]
pub fn pause_all(core: C<'_>) {
    core.pause_all()
}

#[tauri::command]
pub fn resume_all(core: C<'_>) {
    core.resume_all()
}

#[tauri::command]
pub fn clear_finished_jobs(core: C<'_>) {
    core.clear_finished()
}

#[tauri::command]
pub fn close_and_continue(core: C<'_>, job_id: String) {
    core.inner().close_blockers(&job_id)
}

#[tauri::command]
pub fn get_activity() -> Vec<ActivityEntry> {
    store::read_activity()
}

#[tauri::command]
pub async fn export_activity(app: AppHandle) -> Res<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let name = format!("The Updater 3D activity {}.txt", chrono::Local::now().format("%Y-%m-%d"));
    let picked = tauri::async_runtime::spawn_blocking(move || app.dialog().file().set_file_name(&name).add_filter("Text", &["txt"]).blocking_save_file())
        .await
        .map_err(err)?;
    let Some(path) = picked.and_then(|p| p.into_path().ok()) else { return Ok(None) };
    let mut text = String::new();
    for e in store::read_activity().into_iter().rev() {
        let t = chrono::DateTime::parse_from_rfc3339(&e.time).map(|t| t.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()).unwrap_or(e.time.clone());
        text += &format!("{t}  {}", e.title);
        if let Some(m) = &e.meta {
            text += &format!("  ({m})");
        }
        text += "\n";
        if let Some(er) = &e.error {
            for l in er.detail.lines() {
                text += &format!("        {l}\n");
            }
        }
    }
    std::fs::write(&path, text).map_err(err)?;
    Ok(Some(path.display().to_string()))
}

#[tauri::command]
pub fn open_log(path: String) -> Res<()> {
    let p = std::path::PathBuf::from(&path);
    if !p.exists() {
        return Err("That log file is gone".into());
    }
    winutil::shell_open(&path).map_err(err)
}

/// Opens the folder with app.log, for bug reports.
#[tauri::command]
pub fn open_logs_folder() -> Res<()> {
    winutil::shell_open(&crate::paths::logs().to_string_lossy()).map_err(err)
}

#[tauri::command]
pub async fn get_services() -> Vec<ServiceInfo> {
    tauri::async_runtime::spawn_blocking(services::list).await.unwrap_or_default()
}

async fn run_action(core: &Arc<Core>, a: Action) -> Res<()> {
    match a {
        Action::Close(pids) => {
            tauri::async_runtime::spawn_blocking(move || crate::procs::close(&pids, std::time::Duration::from_secs(6))).await.map_err(err)?;
            Ok(())
        }
        Action::Elevated(step) => {
            let silent = core.settings.read().silent_elevation;
            let r = core.elevator.run(step, silent).await.map_err(|e| format!("{e:#}"))?;
            if r.ok { Ok(()) } else { Err(format!("{} {}", r.message, r.output).trim().to_string()) }
        }
        Action::OpenCc => crate::sources::adobe::open_creative_cloud().map_err(err),
    }
}

#[tauri::command]
pub async fn stop_service(core: C<'_>, name: String) -> Res<()> {
    let c = core.inner().clone();
    let a = services::stop(&name).map_err(err)?;
    run_action(&c, a).await?;
    store::activity(ActivityKind::Service, format!("Stopped {}", crate::procs::friendly(&name)), None, None, None);
    Ok(())
}

#[tauri::command]
pub async fn start_service(core: C<'_>, name: String) -> Res<()> {
    let c = core.inner().clone();
    let a = services::start(&name).map_err(err)?;
    run_action(&c, a).await?;
    store::activity(ActivityKind::Service, format!("Started {}", crate::procs::friendly(&name)), None, None, None);
    Ok(())
}

#[tauri::command]
pub async fn stop_safe_services(core: C<'_>) -> Res<()> {
    let c = core.inner().clone();
    let names = tauri::async_runtime::spawn_blocking(services::safe_names).await.map_err(err)?;
    let mut failed = Vec::new();
    for n in &names {
        match services::stop(n) {
            Ok(a) => {
                if let Err(e) = run_action(&c, a).await {
                    failed.push(format!("{n}: {e}"));
                }
            }
            Err(e) => failed.push(format!("{n}: {e}")),
        }
    }
    store::activity(ActivityKind::Service, format!("Stopped {} Adobe background programs", names.len() - failed.len()), None, None, None);
    if failed.is_empty() { Ok(()) } else { Err(failed.join("\n")) }
}

#[tauri::command]
pub fn open_creative_cloud() -> Res<()> {
    crate::sources::adobe::open_creative_cloud().map_err(err)
}

#[tauri::command]
pub async fn set_settings(core: C<'_>, patch: serde_json::Value) -> Res<Settings> {
    let c = core.inner().clone();
    Ok(c.apply_settings(patch).await)
}

#[tauri::command]
pub async fn pick_folder(app: AppHandle, current: String) -> Res<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let picked = tauri::async_runtime::spawn_blocking(move || {
        let mut d = app.dialog().file();
        if std::path::Path::new(&current).exists() {
            d = d.set_directory(&current);
        }
        d.blocking_pick_folder()
    })
    .await
    .map_err(err)?;
    Ok(picked.and_then(|p| p.into_path().ok()).map(|p| p.display().to_string()))
}

#[tauri::command]
pub async fn clear_cache(core: C<'_>) -> Res<u64> {
    let c = core.inner().clone();
    if c.jobs().iter().any(|j| !j.state.is_finished()) {
        return Err("Wait until the queue is empty".into());
    }
    let freed = tauri::async_runtime::spawn_blocking(|| {
        let dir = paths::downloads();
        let size = winutil::dir_size(&dir);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
        }
        size.saturating_sub(winutil::dir_size(&dir))
    })
    .await
    .map_err(err)?;
    let me = c.clone();
    tauri::async_runtime::spawn_blocking(move || me.update_disk_status()).await.ok();
    c.emit_status();
    Ok(freed)
}

/// Async on purpose: a synchronous command that creates a window deadlocks on Windows, so the
/// tray's "Open app" hung whenever the main window had been closed.
#[tauri::command]
pub async fn show_main(app: AppHandle, route: Option<String>) -> Result<(), String> {
    crate::tray::open_route(&app, route);
    if let Some(w) = app.get_webview_window("tray") {
        let _ = w.hide();
    }
    Ok(())
}

/// Look for a newer version of this app only (Settings > About), not for app updates.
#[tauri::command]
pub async fn check_self_update(core: C<'_>) -> Res<Option<String>> {
    core.inner().check_self(false).await.map_err(err)
}

// ------------------------------------------------------------ drivers

/// Restart the PC to finish installing drivers (in a minute, so work can be saved).
#[tauri::command]
pub fn restart_pc(core: C<'_>) -> Res<()> {
    core.inner().restart_pc().map_err(err)
}

/// Open a driver's maker page: where a BIOS is updated from, or the maker's own updater for a
/// card (only the makers' addresses).
#[tauri::command]
pub fn open_driver_page(core: C<'_>, id: String) -> Res<()> {
    let url = core.inner().driver_page(&id).ok_or_else(|| "No maker page for that driver".to_string())?;
    winutil::shell_open(&url).map_err(err)
}

#[tauri::command]
pub async fn install_self_update(app: AppHandle, core: C<'_>) -> Res<()> {
    let c = core.inner().clone();
    if c.jobs().iter().any(|j| !j.state.is_finished()) {
        return Err("Finish or cancel the queue first".into());
    }
    // A second click while the first one is still downloading does nothing.
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
    // What the feed offers now: the release found at the last check may have been replaced.
    let saved = c.self_release.lock().clone();
    let Some(r) = crate::selfupdate::current(saved, crate::selfupdate::check().await) else {
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        c.forget_self_update();
        return Err("The Updater 3D is up to date".into());
    };
    c.offer_self_update(r.clone());
    c.toast("info", format!("Downloading The Updater 3D {}", r.version), None);
    if let Err(e) = crate::selfupdate::install(&r).await {
        BUSY.store(false, std::sync::atomic::Ordering::SeqCst);
        return Err(format!("{e:#}"));
    }
    // The admin helper is another copy of this exe; close it so the installer can replace it.
    c.elevator.disconnect().await;
    if let Some(t) = app.tray_by_id("main") {
        let _ = t.set_visible(false);
    }
    app.exit(0);
    Ok(())
}

#[tauri::command]
pub fn test_notification(app: AppHandle) {
    crate::notice::show(&app, "The Updater 3D", "Notifications are working. You will get one when an update is ready.", "Default", &crate::notice::Target::Home);
}

#[tauri::command]
pub async fn other_builds(core: C<'_>, id: String) -> Res<Vec<OtherBuild>> {
    Ok(core.inner().clone().other_builds(&id).await)
}

#[tauri::command]
pub async fn catalog_versions(core: C<'_>, id: String) -> Res<Vec<VersionInfo>> {
    let c = core.inner().clone();
    c.catalog_versions(&id).await.map_err(|e| format!("{e:#}"))
}

/// One page of the whole winget catalog (Get apps, Other apps): a search or none, a category,
/// a first letter and an order.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn browse_other_apps(core: C<'_>, query: Option<String>, category: Option<String>, letter: Option<String>, sort: Option<String>, offset: usize, limit: usize) -> Res<crate::sources::browse::BrowsePage> {
    let c = core.inner().clone();
    c.browse_other(crate::sources::browse::Request { query, category, letter, sort, offset, limit }).await.map_err(|e| format!("{e:#}"))
}

/// The Other apps cards on screen (`visible`, in order) and the ones kept around them (`near`):
/// their icons are searched for first, searches for places that scrolled away are dropped, and
/// the icons known now come back at once.
#[tauri::command]
pub async fn icons_in_view(core: C<'_>, visible: Vec<String>, near: Vec<String>) -> Res<Vec<crate::core::IconFound>> {
    Ok(core.inner().icons_in_view(visible, near).await)
}

/// The Updater 3D itself, for its own page: where it is, its size and its project.
#[tauri::command]
pub async fn self_info() -> Res<SelfInfo> {
    tauri::async_runtime::spawn_blocking(crate::selfupdate::about).await.map_err(err)
}

/// The Updater 3D's version history with each release's notes.
#[tauri::command]
pub async fn self_versions(core: C<'_>) -> Res<Vec<VersionInfo>> {
    // The feed's offer as it is now, so the newest version is the one an update installs.
    let c = core.inner().clone();
    let (_, rels) = tokio::join!(c.check_self(false), crate::selfupdate::releases());
    let offered = c.self_release.lock().clone();
    Ok(crate::selfupdate::history(&rels, offered.as_ref(), env!("CARGO_PKG_VERSION")))
}

/// A picture in release notes (a GitHub screenshot): read and kept on this PC, as an address the
/// window can load. None when it is not one GitHub serves or can not be read.
#[tauri::command]
pub async fn note_image(url: String) -> Res<Option<String>> {
    Ok(crate::noteimg::get(&url).await.map(|p| paths::asset_url(&p)))
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}

#[cfg(test)]
mod window_tests {
    /// Tauri: a synchronous command that creates a window deadlocks on Windows. Every command that
    /// can open the main window must be async.
    #[test]
    fn commands_that_open_windows_are_async() {
        let src = include_str!("commands.rs");
        let mut cmd = false;
        let mut sync_fn: Option<String> = None;
        for line in src.lines() {
            let t = line.trim_start();
            if t.starts_with("#[tauri::command]") {
                cmd = true;
                continue;
            }
            if t.starts_with("pub fn ") || t.starts_with("pub async fn ") {
                sync_fn = (cmd && t.starts_with("pub fn ")).then(|| t.to_string());
                cmd = false;
                continue;
            }
            if let Some(f) = &sync_fn {
                let opens = [concat!("tray::", "show_main("), concat!("tray::", "open_route("), concat!("WebviewWindow", "Builder::new(")];
                assert!(!opens.iter().any(|o| t.contains(o)), "sync command opens a window: {f}");
            }
        }
    }
}

/// What an uninstalled (or broken) program left behind, shown before anything is deleted.
#[tauri::command]
pub async fn leftover_scan(core: C<'_>, app_id: String) -> Res<crate::uninstall::run::LeftoverScan> {
    let c = core.inner().clone();
    tauri::async_runtime::spawn_blocking(move || crate::uninstall::run::scan(&c, &app_id)).await.map_err(err)?.map_err(err)
}

/// Delete the ticked leftovers of a scan (registry keys, services and tasks are saved first).
#[tauri::command]
pub async fn leftover_delete(core: C<'_>, scan_id: String, ids: Vec<String>) -> Res<crate::uninstall::run::DeleteResult> {
    crate::uninstall::run::delete(core.inner().clone(), &scan_id, &ids).await.map_err(err)
}

/// Put back what a cleanup saved ("Restore" in Activity).
#[tauri::command]
pub async fn leftover_restore(core: C<'_>, backup_id: String) -> Res<()> {
    // An app's settings saved before an update ("settings:<id>").
    if let Some(id) = backup_id.strip_prefix("settings:") {
        return core.inner().clone().restore_settings(id).await.map_err(err);
    }
    crate::uninstall::run::restore(core.inner().clone(), &backup_id).await.map_err(err)
}

/// Cleanups, and apps' saved settings, that can still be restored.
#[tauri::command]
pub async fn leftover_backups() -> Res<Vec<crate::uninstall::run::BackupInfo>> {
    tauri::async_runtime::spawn_blocking(|| {
        let mut list = crate::uninstall::run::backups();
        list.extend(crate::snapshots::list(&crate::snapshots::root()).into_iter().filter(|s| !s.restored).map(|s| crate::uninstall::run::BackupInfo { id: format!("settings:{}", s.id), created: s.created, app: s.app, items: s.places.len() + s.keys.len() }));
        list
    })
    .await
    .map_err(err)
}
