//! The window's commands: uninstall plans for Other programs, leftover scans, deleting what was
//! ticked (after saving it) and restoring a backup.

use super::backup;
use super::leftovers::{self, Item, Kind, Subject};
use super::{entry_key_path, programs, Hive, Target};
use crate::core::Core;
use crate::model::{ActivityKind, JobKind};
use crate::sources::{Installed, Plan, SourceData, Step, Verify};
use crate::winutil;
use anyhow::{anyhow, bail, Result};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Apps as they were when their uninstall was queued: after it they are no longer listed, and
/// the leftover scan needs to know where they were.
static SUBJECTS: Lazy<Mutex<HashMap<String, Installed>>> = Lazy::new(Default::default);

/// Scans shown in the window, by id: what "Delete" may delete is only what was shown.
static SCANS: Lazy<Mutex<HashMap<String, (String, Vec<Item>, std::time::Instant)>>> = Lazy::new(Default::default);

/// How long a leftover list stays valid: after that the PC may have changed (the app installed
/// again into the same folder), so it is scanned again before anything is deleted.
pub const SCAN_VALID: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Is a list made at `made` still safe to delete from?
pub fn scan_fresh(made: std::time::Instant, now: std::time::Instant) -> bool {
    now.saturating_duration_since(made) < SCAN_VALID
}

pub fn remember(inst: &Installed) {
    SUBJECTS.lock().insert(inst.id.clone(), inst.clone());
}

fn starts_dir(p: &str, root: &Path) -> bool {
    super::under(Path::new(p.trim_matches('"')), root) && !super::safety::same_dir(Path::new(p.trim_matches('"')), root)
}

/// The link an uninstall entry hands its uninstall to, and the launcher that opens it: a game
/// launcher's entry ("steam.exe steam://uninstall/400040") asks the launcher, which runs for the
/// user, to uninstall, and the launcher asks the user to confirm. None for an uninstaller.
pub fn launcher_link(args: &[String]) -> Option<(String, String)> {
    let uri = args.iter().skip(1).find(|a| {
        let Some((scheme, rest)) = a.split_once("://") else { return false };
        scheme.len() >= 2 && !rest.is_empty() && scheme.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-')) && !matches!(scheme.to_ascii_lowercase().as_str(), "http" | "https" | "file" | "ftp")
    })?;
    let scheme = uri.split_once("://").map(|(s, _)| s.to_ascii_lowercase()).unwrap_or_default();
    let by = match scheme.as_str() {
        "steam" => "Steam".to_string(),
        "uplay" => "Ubisoft Connect".to_string(),
        "goggalaxy" => "GOG Galaxy".to_string(),
        "com.epicgames.launcher" => "the Epic Games Launcher".to_string(),
        "origin" | "origin2" | "link2ea" => "the EA app".to_string(),
        "battlenet" => "Battle.net".to_string(),
        _ => args.first().and_then(|a| Path::new(a).file_stem()).map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "its launcher".into()),
    };
    Some((uri.clone(), by))
}

/// The launcher a plan hands an uninstall to ("Steam"), from its step's label.
pub fn handed_to(plan: &Plan) -> Option<String> {
    plan.steps.iter().find_map(|s| match s {
        Step::OpenUri { label, .. } if plan.kind == JobKind::Uninstall => label.split_once(": confirm it in ").map(|(_, by)| by.to_string()),
        _ => None,
    })
}

/// How an Other program is uninstalled: its own uninstaller (silent when it is known how), or
/// the launcher its entry hands the uninstall to.
pub fn plan(inst: &Installed) -> Result<Plan> {
    let SourceData::Other { entry, .. } = &inst.data else { bail!("{} is not a program from the uninstall list", inst.name) };
    if inst.missing {
        bail!("The uninstaller of {} is missing. Use Remove from list to clean up what is left.", inst.name);
    }
    let c = programs::choice_for(inst).ok_or_else(|| anyhow!("{} names no uninstaller", inst.name))?;
    // Opened as the user: an elevated copy of the launcher would not reach the one that runs.
    if let Some((uri, by)) = launcher_link(&c.args) {
        return Ok(Plan {
            app_id: inst.id.clone(),
            kind: JobKind::Uninstall,
            name: inst.name.clone(),
            to_version: inst.version.clone(),
            downloads: vec![],
            steps: vec![Step::OpenUri { uri, label: format!("Uninstalling {}: confirm it in {by}", inst.name) }],
            block_dirs: vec![],
            block_names: vec![],
            verify: Verify::Removed { app_id: inst.id.clone() },
            fallback: None,
            cleanup: vec![],
        });
    }
    let in_pf = starts_dir(&c.args[0], &crate::paths::program_files()) || starts_dir(&c.args[0], &crate::paths::program_files_x86());
    let elevated = entry.hive != "HKCU" || in_pf;
    // An uninstaller with its own window: the user answers it, and the uninstall list tells
    // whether it removed the program, whatever it returns.
    let ok_codes: Vec<u32> = if c.silent { vec![0, 3010, 1641, 1605] } else { (0..=255).chain([3010, 1641, 1605, 1614]).collect() };
    let label = if c.silent { format!("Uninstalling {}", inst.name) } else { format!("Uninstalling {}: answer its uninstaller's window", inst.name) };
    Ok(Plan {
        app_id: inst.id.clone(),
        kind: JobKind::Uninstall,
        name: inst.name.clone(),
        to_version: inst.version.clone(),
        downloads: vec![],
        steps: vec![Step::Run { exe: PathBuf::from(&c.args[0]), args: c.args[1..].to_vec(), cwd: None, ok_codes, elevated, label, log: None }],
        block_dirs: inst.install_dir.iter().filter(|d| crate::sources::other::own_dir(d)).cloned().collect(),
        block_names: vec![],
        verify: Verify::Removed { app_id: inst.id.clone() },
        fallback: None,
        cleanup: vec![],
    })
}

/// A leftover scan, for the window.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LeftoverScan {
    pub scan_id: String,
    pub app_id: String,
    pub name: String,
    /// The program's uninstaller is missing: this scan is for "Remove from list".
    pub broken: bool,
    pub items: Vec<Item>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    pub removed: usize,
    pub failed: Vec<String>,
    pub freed: u64,
    pub backup_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupInfo {
    pub id: String,
    pub created: String,
    pub app: String,
    pub items: usize,
}

/// The entry an app came from, when it is from the uninstall list.
fn entry_of(i: &Installed) -> Option<&winutil::UninstallEntry> {
    match &i.data {
        SourceData::Other { entry, .. } if !entry.key.is_empty() => Some(entry),
        _ => None,
    }
}

/// What the scan needs to know about the app and the rest of the PC.
pub fn subject(inst: &Installed) -> Subject {
    Subject {
        name: inst.name.clone(),
        publisher: entry_of(inst).map(|e| e.publisher.clone()).unwrap_or_default(),
        install_dir: inst.install_dir.clone(),
        entry: entry_of(inst).map(|e| (Hive::of_entry(e.hive), entry_key_path(e.hive, &e.key))),
        exes: inst.exe.iter().cloned().collect(),
        runner: entry_of(inst).and_then(|e| winutil::split_program_line(&e.uninstall_string).into_iter().next()).map(|p| winutil::pathbuf(&p)),
    }
}

/// The other programs' folders and names, from the uninstall list (the app's own entries, and
/// those named like it, left out).
pub fn others(inst: &Installed, entries: &[winutil::UninstallEntry]) -> (Vec<PathBuf>, Vec<String>) {
    let me = entry_of(inst);
    let mine = |e: &winutil::UninstallEntry| {
        me.is_some_and(|m| (m.key.eq_ignore_ascii_case(&e.key) && m.hive == e.hive) || m.msi_parts.iter().any(|k| k.eq_ignore_ascii_case(&e.key)))
            || e.display_name.trim().eq_ignore_ascii_case(inst.name.trim())
            || me.is_some_and(|m| e.display_name.trim().eq_ignore_ascii_case(m.display_name.trim()))
    };
    let mut dirs = Vec::new();
    let mut names = Vec::new();
    for e in entries.iter().filter(|e| !mine(e)) {
        if !e.install_location.trim().is_empty() {
            dirs.push(winutil::pathbuf(e.install_location.trim().trim_matches('"')));
        }
        let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"').to_string();
        let un = winutil::split_program_line(&e.uninstall_string).into_iter().next().unwrap_or_default();
        for prog in [icon, un] {
            let p = PathBuf::from(prog);
            if p.is_absolute() && !crate::sources::locate::bad_place(&p.to_string_lossy()) {
                dirs.extend(p.parent().map(Path::to_path_buf));
            }
        }
        if crate::sources::other::is_app(e) {
            names.push(e.display_name.trim().to_string());
        }
    }
    (dirs, names)
}

/// Leftovers are only for programs that are gone, or broken ones ("Remove from list"): a scan
/// of an installed app would offer its live files, keys and services for deletion.
pub fn may_scan(still_installed: bool, broken: bool) -> bool {
    !still_installed || broken
}

/// Not an app: the PC-wide scan for shortcuts whose program is gone.
pub const DEAD_SHORTCUTS: &str = "system:dead-shortcuts";

/// Scan what the app left (blocking). Its own entry is ticked and required when its uninstaller
/// is missing ("Remove from list").
pub fn scan(core: &Arc<Core>, app_id: &str) -> Result<LeftoverScan> {
    if app_id == DEAD_SHORTCUTS {
        let entries = winutil::uninstall_entries();
        let dirs: Vec<PathBuf> = core.installed_all().iter().filter_map(|i| i.install_dir.clone()).collect();
        let names: Vec<String> = entries.iter().filter(|e| crate::sources::other::is_app(e)).map(|e| e.display_name.trim().to_string()).collect();
        let env = leftovers::Env::system(dirs, names);
        let items = leftovers::dead_shortcuts(&env);
        let scan_id = uuid::Uuid::new_v4().simple().to_string();
        let name = "Dead shortcuts".to_string();
        SCANS.lock().insert(scan_id.clone(), (name.clone(), items.clone(), std::time::Instant::now()));
        return Ok(LeftoverScan { scan_id, app_id: DEAD_SHORTCUTS.into(), name, broken: true, items });
    }
    let now = core.installed(app_id);
    let inst = now.clone().or_else(|| SUBJECTS.lock().get(app_id).cloned()).ok_or_else(|| anyhow!("Nothing is known about that app any more"))?;
    let entries = winutil::uninstall_entries();
    let (dirs, names) = others(&inst, &entries);
    let rest: Vec<Installed> = core.installed_all().into_iter().filter(|i| i.id != inst.id).collect();
    let prefs = crate::prefs::steps(&inst, &rest, &crate::prefs::Roots::system());
    let env = leftovers::Env::system(dirs, names);
    let broken = now.as_ref().is_some_and(|i| i.missing && i.source == crate::model::Source::Programs);
    if !may_scan(now.is_some(), broken) {
        bail!("{} is still installed. Its leftovers are shown after it is uninstalled.", inst.name);
    }
    let mut items = leftovers::scan(&subject(&inst), &env, &prefs);
    if broken {
        for i in items.iter_mut().filter(|i| i.kind == Kind::Entry) {
            i.required = true;
            i.checked = true;
        }
    }
    let scan_id = uuid::Uuid::new_v4().simple().to_string();
    SCANS.lock().insert(scan_id.clone(), (inst.name.clone(), items.clone(), std::time::Instant::now()));
    Ok(LeftoverScan { scan_id, app_id: inst.id.clone(), name: inst.name.clone(), broken, items })
}

fn label_of(i: &Item) -> String {
    match i.kind {
        Kind::Folder => "Deleting what is left of its folder".into(),
        Kind::Entry => "Removing its entry from the installed programs".into(),
        Kind::Shortcut | Kind::Startup => "Removing a shortcut or startup entry".into(),
        Kind::Service => format!("Removing the service {}", i.label),
        Kind::Task => format!("Removing the task {}", i.label),
        _ => format!("Removing {}", i.label),
    }
}

/// Delete the ticked items of a scan (and the required ones), after saving the registry keys,
/// services and tasks among them.
pub async fn delete(core: Arc<Core>, scan_id: &str, ids: &[String]) -> Result<DeleteResult> {
    let (name, items, made) = SCANS.lock().get(scan_id).cloned().ok_or_else(|| anyhow!("That list is out of date. Open it again."))?;
    if !scan_fresh(made, std::time::Instant::now()) {
        SCANS.lock().remove(scan_id);
        bail!("That list is out of date. Open it again.");
    }
    let picked: Vec<Item> = items.into_iter().filter(|i| i.required || ids.contains(&i.id)).collect();
    if picked.is_empty() {
        return Ok(DeleteResult { removed: 0, failed: vec![], freed: 0, backup_id: None });
    }
    let targets: Vec<Target> = picked.iter().map(|i| i.target.clone()).collect();
    let n2 = name.clone();
    let saved = tauri::async_runtime::spawn_blocking(move || {
        let root = backup::root();
        backup::prune(&root, chrono::Utc::now());
        backup::save(&root, &n2, &targets)
    })
    .await
    .map_err(|e| anyhow!("{e}"))?
    .map_err(|e| anyhow!("Nothing was deleted: the registry backup failed ({e:#})"))?;
    let silent = core.settings.read().silent_elevation;
    let (mut removed, mut freed, mut failed) = (0, 0u64, Vec::new());
    for i in &picked {
        let step = Step::Leftover { target: i.target.clone(), elevated: i.elevated, label: label_of(i) };
        match core.elevator.run(step, silent).await {
            Ok(r) if r.ok => {
                removed += 1;
                if matches!(i.target, Target::Dir { .. } | Target::File { .. }) {
                    freed += i.size.unwrap_or(0);
                }
            }
            Ok(r) => failed.push(format!("{}: {}", i.label, r.message)),
            Err(e) => failed.push(format!("{}: {e:#}", i.label)),
        }
    }
    SCANS.lock().remove(scan_id);
    let mut meta = vec![format!("{removed} removed")];
    if freed > 0 {
        meta.push(format!("{} freed", crate::core::fmt_bytes(freed)));
    }
    if !failed.is_empty() {
        meta.push(format!("{} could not be removed", failed.len()));
    }
    let backup_id = saved.as_ref().map(|s| s.id.clone());
    core.log_activity(ActivityKind::Uninstall, format!("Cleaned up after {name}"), Some(meta.join(", ")), backup_id.clone());
    core.rescan().await;
    Ok(DeleteResult { removed, failed, freed, backup_id })
}

/// Put back what a cleanup saved.
pub async fn restore(core: Arc<Core>, id: &str) -> Result<()> {
    let root = backup::root();
    let mut saved = backup::load(&root, id).filter(|s| !s.restored).ok_or_else(|| anyhow!("That backup is gone"))?;
    let silent = core.settings.read().silent_elevation;
    let mut failed = Vec::new();
    for step in backup::restore_steps(&saved) {
        let label = step.label().to_string();
        match core.elevator.run(step, silent).await {
            Ok(r) if r.ok => {}
            Ok(r) => failed.push(format!("{label}: {}", r.message)),
            Err(e) => failed.push(format!("{label}: {e:#}")),
        }
    }
    // Done only when everything came back: a part that failed (a declined admin prompt for the
    // machine-wide keys) stays in Activity so it can be tried again. Importing a key again that
    // already came back is harmless.
    if failed.is_empty() {
        saved.restored = true;
        let _ = backup::write_manifest(&root, &saved);
    }
    let meta = if failed.is_empty() { "Registry entries and tasks are back. Services return after a restart.".to_string() } else { failed.join("; ") };
    core.log_activity(ActivityKind::Uninstall, format!("Restored what was cleaned up after {}", saved.app), Some(meta), None);
    core.rescan().await;
    if failed.is_empty() {
        Ok(())
    } else {
        Err(anyhow!("Some of it could not be restored: {}", failed.join("; ")))
    }
}

/// Backups that can still be restored.
pub fn backups() -> Vec<BackupInfo> {
    backup::list(&backup::root()).into_iter().map(|s| BackupInfo { items: s.items.len(), id: s.id, created: s.created, app: s.app }).collect()
}
