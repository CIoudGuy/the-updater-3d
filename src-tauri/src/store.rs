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
    if old.join("settings.json").exists() {
        for f in ["settings.json", "state.json", "activity.jsonl"] {
            let _ = std::fs::copy(old.join(f), paths::data_dir().join(f));
        }
    }
    let _ = std::fs::write(flag, "");
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
