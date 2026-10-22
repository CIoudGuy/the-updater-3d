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
}

pub fn load_state() -> Persisted {
    std::fs::read(paths::state_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save_state(s: &Persisted) {
    let _ = std::fs::write(paths::state_file(), serde_json::to_vec_pretty(s).unwrap_or_default());
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
