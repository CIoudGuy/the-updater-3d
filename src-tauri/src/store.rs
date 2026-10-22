//! Settings, activity log and small persistent state on disk.

use crate::model::{ActivityEntry, ActivityKind, JobError, Settings};
use crate::paths;
use serde::{Deserialize, Serialize};
use std::io::Write;

pub fn load_settings() -> Settings {
    migrate_from_old_id();
    std::fs::read(paths::settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// 1.0.0 used another app id and data folder. Bring its settings, state and activity across
/// once, so things like "Fewer admin prompts" stay on after updating.
fn migrate_from_old_id() {
    let flag = paths::data_dir().join("migrated-from-1.0.0");
    if flag.exists() {
        return;
    }
    let old = paths::local_app_data().join("cloud.devalt.theupdater3d").join("data");
    migrate_dirs(&old, &paths::data_dir());
    let _ = std::fs::write(flag, "");
}

fn migrate_dirs(old: &std::path::Path, new: &std::path::Path) {
    // Never overwrite what this version already saved (window size, history, settings changed
    // since): copy whole files only when they are missing here.
    for f in ["settings.json", "state.json", "activity.jsonl"] {
        if old.join(f).exists() && !new.join(f).exists() {
            let _ = std::fs::copy(old.join(f), new.join(f));
        }
    }
    // Settings that already exist here only take "Fewer admin prompts" from the old version,
    // because its Windows task is still registered.
    let read = |p: std::path::PathBuf| std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
    if let (Some(o), Some(mut n)) = (read(old.join("settings.json")), read(new.join("settings.json"))) {
        if o["silentElevation"] == true && n["silentElevation"] != true {
            n["silentElevation"] = serde_json::Value::Bool(true);
            let _ = std::fs::write(new.join("settings.json"), serde_json::to_vec_pretty(&n).unwrap_or_default());
        }
    }
}

pub fn save_settings(s: &Settings) {
    let tmp = paths::settings_file().with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, paths::settings_file());
    }
}

/// Apply a partial settings object from the UI onto the current settings.
pub fn merge(current: &Settings, patch: serde_json::Value) -> Settings {
    let mut v = serde_json::to_value(current).unwrap_or_default();
    if let (Some(obj), Some(p)) = (v.as_object_mut(), patch.as_object()) {
        for (k, val) in p {
            if k == "sources" {
                if let (Some(dst), Some(src)) = (obj.get_mut("sources").and_then(|s| s.as_object_mut()), val.as_object()) {
                    for (sk, sv) in src {
                        dst.insert(sk.clone(), sv.clone());
                    }
                    continue;
                }
            }
            obj.insert(k.clone(), val.clone());
        }
    }
    let mut s: Settings = serde_json::from_value(v).unwrap_or_else(|_| current.clone());
    s.parallel_downloads = s.parallel_downloads.clamp(1, 4);
    s
}

pub fn activity(kind: ActivityKind, title: impl Into<String>, meta: Option<String>, app_id: Option<String>, error: Option<JobError>) -> ActivityEntry {
    let e = ActivityEntry {
        id: uuid::Uuid::new_v4().simple().to_string(),
        time: chrono::Utc::now().to_rfc3339(),
        kind,
        title: title.into(),
        meta,
        app_id,
        error,
    };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(paths::activity_file()) {
        let _ = writeln!(f, "{}", serde_json::to_string(&e).unwrap_or_default());
    }
    e
}

/// Newest first, at most 1000 entries.
pub fn read_activity() -> Vec<ActivityEntry> {
    let text = std::fs::read_to_string(paths::activity_file()).unwrap_or_default();
    let mut v: Vec<ActivityEntry> = text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect();
    if v.len() > 3000 {
        // Keep the file from growing forever.
        let keep = v.split_off(v.len() - 2000);
        let body: String = keep.iter().map(|e| serde_json::to_string(e).unwrap_or_default() + "\n").collect();
        let _ = std::fs::write(paths::activity_file(), body);
        v = keep;
    }
    v.reverse();
    v.truncate(1000);
    v
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Persisted {
    pub last_check: Option<String>,
    /// app id -> latest version we already told the user about
    pub notified: std::collections::BTreeMap<String, String>,
    pub last_auto_install_day: Option<String>,
    /// "id@version" -> bytes on disk, so sizes show at once on the next start
    #[serde(default)]
    pub sizes: std::collections::BTreeMap<String, u64>,
    /// Main window placement: x, y, width, height (physical pixels), maximized
    #[serde(default)]
    pub window: Option<(i32, i32, u32, u32, bool)>,
    /// Startup at sign-in was set up once (it is on by default)
    #[serde(default)]
    pub autostart_applied: bool,
    /// How long installs of a kind took (seconds, smoothed), for a progress bar that fits
    #[serde(default)]
    pub install_times: std::collections::BTreeMap<String, f64>,
    /// "Keep previous version" became off by default in 1.0.5; switched off once for copies
    /// that only had the old default.
    #[serde(default)]
    pub keep_previous_default_off: bool,
    /// Blender pre-releases became shown by default in 1.0.5; switched on once
    #[serde(default)]
    pub prereleases_default_on: bool,
    /// Other apps became on by default in 1.1.0; switched on once
    #[serde(default)]
    pub other_apps_default_on: bool,
    /// Fewer admin prompts became on by default in 1.1.2; switched on once
    #[serde(default)]
    pub silent_elevation_default_on: bool,
    /// Other apps package id -> day (YYYY-MM-DD) no icon was found for it, so it is not searched
    /// for again at every start
    #[serde(default)]
    pub icon_misses: std::collections::BTreeMap<String, String>,
}

pub fn load_state() -> Persisted {
    load_state_from(&paths::state_file())
}

fn backup_of(file: &std::path::Path) -> std::path::PathBuf {
    file.with_extension("json.bak")
}

/// The state file, or its backup when it is damaged. When neither can be read but one exists,
/// this is not a first start: the one-time default changes count as done, so a damaged file
/// never switches settings the user chose back to old defaults.
pub fn load_state_from(file: &std::path::Path) -> Persisted {
    let bak = backup_of(file);
    let read = |p: &std::path::Path| std::fs::read(p).ok().and_then(|b| serde_json::from_slice::<Persisted>(&b).ok());
    if let Some(p) = read(file).or_else(|| read(&bak)) {
        return p;
    }
    let mut p = Persisted::default();
    if file.exists() || bak.exists() {
        p.autostart_applied = true;
        p.keep_previous_default_off = true;
        p.prereleases_default_on = true;
        p.other_apps_default_on = true;
        p.silent_elevation_default_on = true;
    }
    p
}

pub fn save_state(s: &Persisted) {
    save_state_to(&paths::state_file(), s);
}

/// Written to a temporary file and renamed over the old one, so a crash mid-write never
/// leaves a damaged state file; the previous good one is kept as a backup.
pub fn save_state_to(file: &std::path::Path, s: &Persisted) {
    let tmp = file.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(s).unwrap_or_default()).is_err() {
        return;
    }
    if std::fs::read(file).ok().is_some_and(|b| serde_json::from_slice::<Persisted>(&b).is_ok()) {
        let _ = std::fs::copy(file, backup_of(file));
    }
    let _ = std::fs::rename(&tmp, file);
}

/// One-time changes of defaults (each runs once per PC, recorded in the state). Returns true
/// when start at sign-in should be turned on in Windows now.
pub fn apply_default_changes(p: &mut Persisted, s: &mut Settings) -> bool {
    let mut autostart = false;
    // Fewer admin prompts became on by default in 1.1.2; the task behind it is set up (one
    // admin prompt) when the window opens.
    if !p.silent_elevation_default_on {
        s.silent_elevation = true;
        p.silent_elevation_default_on = true;
    }
    if !p.other_apps_default_on {
        s.sources.other = true;
        p.other_apps_default_on = true;
    }
    if !p.prereleases_default_on {
        s.blender_channel = "beta".into();
        p.prereleases_default_on = true;
    }
    if !p.keep_previous_default_off {
        s.keep_previous_versions = false;
        p.keep_previous_default_off = true;
    }
    if !p.autostart_applied {
        // Also turns it on for copies that saved the old default (off).
        s.launch_at_startup = true;
        autostart = true;
        p.autostart_applied = true;
    }
    autostart
}

/// Simple file logger for the app's own log (logs\app.log), rotated at 2 MB.
pub struct FileLog;

impl log::Log for FileLog {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Info || m.target().starts_with("updater")
    }
    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) || r.target().starts_with("tao") || r.target().starts_with("wry") {
            return;
        }
        let p = paths::logs().join("app.log");
        if p.metadata().map(|m| m.len() > 2 * 1024 * 1024).unwrap_or(false) {
            let _ = std::fs::rename(&p, paths::logs().join("app.old.log"));
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(p) {
            let _ = writeln!(f, "{} {:5} {}: {}", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"), r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}

#[cfg(test)]
mod tests {
    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("u3d-state-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn damaged_state_does_not_bring_back_old_defaults() {
        let dir = scratch("damaged");
        let file = dir.join("state.json");
        // A crash in the middle of writing left half a file.
        std::fs::write(&file, br#"{"last_check":"2026-10-01","notif"#).unwrap();
        let mut p = super::load_state_from(&file);
        let mut s: crate::model::Settings = serde_json::from_str(r#"{"blenderChannel":"stable","keepPreviousVersions":true}"#).unwrap();
        let autostart = super::apply_default_changes(&mut p, &mut s);
        assert_eq!(s.blender_channel, "stable");
        assert!(s.keep_previous_versions);
        assert!(!autostart);
        // A first start (no state file at all) still gets the new defaults.
        let mut p = super::load_state_from(&dir.join("none.json"));
        let mut s: crate::model::Settings = serde_json::from_str(r#"{"blenderChannel":"stable"}"#).unwrap();
        super::apply_default_changes(&mut p, &mut s);
        assert_eq!(s.blender_channel, "beta");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_is_saved_whole_and_the_backup_is_used() {
        let dir = scratch("atomic");
        let file = dir.join("state.json");
        let mut p = super::Persisted::default();
        p.last_check = Some("one".into());
        super::save_state_to(&file, &p);
        p.last_check = Some("two".into());
        super::save_state_to(&file, &p);
        assert_eq!(super::load_state_from(&file).last_check.as_deref(), Some("two"));
        assert!(!file.with_extension("json.tmp").exists());
        // The main file is damaged: the backup (the save before) is used.
        std::fs::write(&file, b"{").unwrap();
        assert_eq!(super::load_state_from(&file).last_check.as_deref(), Some("one"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn persisted_keeps_window_placement() {
        let mut p = super::Persisted::default();
        p.window = Some((120, 80, 1400, 900, false));
        let back: super::Persisted = serde_json::from_slice(&serde_json::to_vec(&p).unwrap()).unwrap();
        assert_eq!(back.window, Some((120, 80, 1400, 900, false)));
        // State files written before the window field existed still load.
        let old: super::Persisted = serde_json::from_str(r#"{"last_check":null,"notified":{},"last_auto_install_day":null}"#).unwrap();
        assert_eq!(old.window, None);
    }

    #[test]
    fn carry_over_keeps_what_the_new_version_saved() {
        let root = std::env::temp_dir().join(format!("u3d-migrate-{}", std::process::id()));
        let (old, new) = (root.join("old"), root.join("new"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("state.json"), r#"{"notified":{}}"#).unwrap();
        std::fs::write(old.join("settings.json"), r#"{"silentElevation":true,"uiZoom":1.0}"#).unwrap();
        std::fs::write(new.join("state.json"), r#"{"notified":{},"window":[10,20,1500,950,false]}"#).unwrap();
        std::fs::write(new.join("settings.json"), r#"{"silentElevation":false,"uiZoom":1.25}"#).unwrap();
        super::migrate_dirs(&old, &new);
        let state: super::Persisted = serde_json::from_slice(&std::fs::read(new.join("state.json")).unwrap()).unwrap();
        assert_eq!(state.window, Some((10, 20, 1500, 950, false)), "window placement must survive");
        let settings: serde_json::Value = serde_json::from_slice(&std::fs::read(new.join("settings.json")).unwrap()).unwrap();
        assert_eq!(settings["silentElevation"], true);
        assert_eq!(settings["uiZoom"], 1.25);
        // Missing files are still copied across.
        std::fs::remove_file(new.join("state.json")).unwrap();
        super::migrate_dirs(&old, &new);
        assert!(new.join("state.json").exists());
        let _ = std::fs::remove_dir_all(&root);
    }
}
