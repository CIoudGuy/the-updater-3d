//! Commands the UI calls. Names and arguments match src/lib/commands.ts.

use crate::core::{Core, JobRequest};
use crate::model::*;
use crate::services::{self, Action};
use crate::{paths, store, winutil};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};

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
    let ids: Vec<String> = c.apps().into_iter().filter(|a| a.status == AppStatus::Update).map(|a| a.id).collect();
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

#[tauri::command]
pub async fn prefs_preview(core: C<'_>, app_id: String) -> Res<Vec<crate::model::PrefsItem>> {
    let c = core.inner().clone();
    tokio::task::spawn_blocking(move || c.prefs_preview(&app_id)).await.map_err(err)?.map_err(err)
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
    let exe = i.exe.ok_or("Could not find the program file")?;
    winutil::launch(&exe, &[]).map_err(err)
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
pub fn get_catalog_entry(core: C<'_>, id: String) -> Option<CatalogEntry> {
    core.catalog_entry(&id)
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
    let picked = tokio::task::spawn_blocking(move || app.dialog().file().set_file_name(&name).add_filter("Text", &["txt"]).blocking_save_file())
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

#[tauri::command]
pub async fn get_services() -> Vec<ServiceInfo> {
    tokio::task::spawn_blocking(services::list).await.unwrap_or_default()
}

async fn run_action(core: &Arc<Core>, a: Action) -> Res<()> {
    match a {
        Action::Close(pids) => {
            tokio::task::spawn_blocking(move || crate::procs::close(&pids, std::time::Duration::from_secs(6))).await.map_err(err)?;
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
    let names = tokio::task::spawn_blocking(services::safe_names).await.map_err(err)?;
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
    let picked = tokio::task::spawn_blocking(move || {
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
    let freed = tokio::task::spawn_blocking(|| {
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
    tokio::task::spawn_blocking(move || me.update_disk_status()).await.ok();
    c.emit_status();
    Ok(freed)
}

#[tauri::command]
pub fn show_main(app: AppHandle, route: Option<String>) {
    crate::tray::show_main(&app);
    if let Some(r) = route {
        let _ = app.emit("navigate", r);
    }
    if let Some(w) = app.get_webview_window("tray") {
        let _ = w.hide();
    }
}

#[tauri::command]
pub async fn install_self_update(app: AppHandle, core: C<'_>) -> Res<()> {
    let c = core.inner().clone();
    if c.jobs().iter().any(|j| !j.state.is_finished()) {
        return Err("Finish or cancel the queue first".into());
    }
    let r = c.self_release.lock().clone().ok_or("No update for The Updater 3D right now")?;
    // A second click while the first one is still downloading does nothing.
    static BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if BUSY.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return Ok(());
    }
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
pub fn test_notification(app: AppHandle) -> Res<()> {
    use tauri_plugin_notification::NotificationExt;
    app.notification().builder().title("The Updater 3D").body("Notifications are working. You will get one when an update is ready.").show().map_err(err)
}

#[tauri::command]
pub async fn catalog_versions(core: C<'_>, id: String) -> Res<Vec<VersionInfo>> {
    let c = core.inner().clone();
    c.catalog_versions(&id).await.map_err(|e| format!("{e:#}"))
}

#[tauri::command]
pub async fn search_other_apps(core: C<'_>, query: String) -> Res<Vec<CatalogEntry>> {
    let c = core.inner().clone();
    Ok(c.search_other(query).await)
}

#[tauri::command]
pub fn quit_app(app: AppHandle) {
    app.exit(0);
}
