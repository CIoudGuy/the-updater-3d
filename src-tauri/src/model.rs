//! Types shared with the UI. Mirrors src/lib/types.ts field for field.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Adobe,
    Craft,
    Blender,
    /// OBS Studio, Krita, FreeCAD, Godot and Unity Editor.
    Tools,
    /// Any other app, matched from the public winget catalog.
    Other,
}

impl Source {
    pub fn group(self) -> &'static str {
        match self {
            Source::Adobe => "Adobe",
            Source::Craft => "Craft apps",
            Source::Blender => "Blender",
            Source::Tools => "More apps",
            Source::Other => "Other apps",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppStatus {
    Current,
    Update,
    Queued,
    Updating,
    Blocked,
    Failed,
    Missing,
    Unknown,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppEntry {
    pub id: String,
    pub source: Source,
    pub name: String,
    pub code: String,
    pub fg: String,
    pub bg: String,
    pub icon_url: Option<String>,
    pub version: String,
    pub latest: Option<String>,
    pub status: AppStatus,
    pub update_size: Option<u64>,
    pub size_on_disk: Option<u64>,
    pub install_dir: Option<String>,
    pub updated_at: Option<String>,
    pub tagline: Option<String>,
    pub group: String,
    pub running: bool,
    pub can_uninstall: bool,
    pub can_rollback: bool,
    pub channel: Option<String>,
    pub note: Option<String>,
    pub job_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogEntry {
    pub id: String,
    pub source: Source,
    pub name: String,
    pub code: String,
    pub fg: String,
    pub bg: String,
    pub icon_url: Option<String>,
    pub version: String,
    pub size: Option<u64>,
    pub tagline: Option<String>,
    pub group: String,
    pub prerelease: bool,
    pub installed: bool,
    pub pick_version: bool,
}

/// One thing "Also delete settings and add-ons" would remove, for the uninstall dialog.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrefsItem {
    /// Shown path: "AppData\Roaming\obs-studio", or a registry key "HKCU\Software\..."
    pub path: String,
    /// Bytes on disk (none for a registry key)
    pub size: Option<u64>,
    pub registry: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionInfo {
    pub version: String,
    pub date: Option<String>,
    pub notes: Vec<String>,
    pub notes_url: Option<String>,
    pub size: Option<u64>,
    pub installed: bool,
    pub latest: bool,
    pub prerelease: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobKind {
    Update,
    Install,
    Uninstall,
    Rollback,
    Repair,
}

impl JobKind {
    pub fn verb(self) -> &'static str {
        match self {
            JobKind::Update => "Update",
            JobKind::Install => "Install",
            JobKind::Uninstall => "Uninstall",
            JobKind::Rollback => "Roll back",
            JobKind::Repair => "Repair",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum JobState {
    Waiting,
    Downloading,
    Paused,
    Installing,
    Verifying,
    Blocked,
    Done,
    Failed,
    Canceled,
}

impl JobState {
    pub fn is_finished(self) -> bool {
        matches!(self, JobState::Done | JobState::Failed | JobState::Canceled)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobError {
    pub code: String,
    pub message: String,
    pub detail: String,
    pub log_path: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: String,
    pub app_id: String,
    pub name: String,
    pub code: String,
    pub fg: String,
    pub bg: String,
    pub icon_url: Option<String>,
    pub kind: JobKind,
    pub from_version: Option<String>,
    pub to_version: String,
    pub state: JobState,
    pub downloaded: u64,
    pub total: u64,
    pub speed: f64,
    pub eta: Option<f64>,
    pub progress: f64,
    pub step: String,
    pub blocked_by: Vec<String>,
    pub error: Option<JobError>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    /// An installing job that is only waiting (for its turn, or for Creative Cloud), so Cancel is safe
    #[serde(default)]
    pub cancelable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivityKind {
    Update,
    Install,
    Uninstall,
    Rollback,
    Check,
    Failure,
    Service,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub id: String,
    pub time: String,
    pub kind: ActivityKind,
    pub title: String,
    pub meta: Option<String>,
    pub app_id: Option<String>,
    pub error: Option<JobError>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub kind: String,
    pub running: bool,
    pub cpu: f32,
    pub memory: u64,
    pub required: bool,
    pub safe_to_stop: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AutoMode {
    Off,
    Notify,
    Install,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sources {
    pub adobe: bool,
    pub craft: bool,
    pub blender: bool,
    /// OBS Studio, Krita, FreeCAD, Godot, Unity. Settings saved by older versions have no
    /// value for it, and must keep loading: it defaults to on.
    #[serde(default = "yes")]
    pub tools: bool,
    /// Every other app, matched from the winget catalog. On by default.
    #[serde(default = "yes")]
    pub other: bool,
}

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub check_interval: u32,
    pub auto_mode: AutoMode,
    pub window_start: String,
    pub window_end: String,
    pub exceptions: BTreeMap<String, AutoMode>,
    pub keep_previous_versions: bool,
    pub bandwidth_limit: u64,
    pub bandwidth_limit_enabled: bool,
    pub launch_at_startup: bool,
    pub start_minimized: bool,
    pub close_to_tray: bool,
    pub notify_updates: bool,
    pub notify_finished: bool,
    pub notify_failed: bool,
    pub language: String,
    pub adobe_install_dir: String,
    pub blender_install_dir: String,
    /// Root that new Godot versions go into (one folder per version)
    pub godot_install_dir: String,
    pub blender_channel: String,
    pub blender_keep_patch_versions: bool,
    pub craft_prereleases: bool,
    /// Get apps also lists apps that are already installed (off: only what you do not have)
    pub show_installed_in_get_apps: bool,
    pub sources: Sources,
    pub parallel_downloads: u32,
    pub silent_elevation: bool,
    pub ui_zoom: f64,
}

impl Default for Settings {
    fn default() -> Self {
        let pf = std::env::var("ProgramFiles").unwrap_or_else(|_| "C:\\Program Files".into());
        Settings {
            check_interval: 4,
            auto_mode: AutoMode::Notify,
            window_start: "02:00".into(),
            window_end: "06:00".into(),
            exceptions: BTreeMap::new(),
            keep_previous_versions: false,
            bandwidth_limit: 40 * 1024 * 1024,
            bandwidth_limit_enabled: false,
            launch_at_startup: true,
            start_minimized: false,
            close_to_tray: true,
            notify_updates: true,
            notify_finished: true,
            notify_failed: true,
            language: "en_US".into(),
            adobe_install_dir: format!("{pf}\\Adobe"),
            blender_install_dir: format!("{pf}\\Blender Foundation"),
            godot_install_dir: format!("{}\\Programs\\Godot", std::env::var("LOCALAPPDATA").unwrap_or_else(|_| "C:\\Users\\Public\\AppData\\Local".into())),
            blender_channel: "beta".into(),
            blender_keep_patch_versions: false,
            craft_prereleases: false,
            show_installed_in_get_apps: false,
            sources: Sources { adobe: true, craft: true, blender: true, tools: true, other: true },
            parallel_downloads: 2,
            silent_elevation: false,
            ui_zoom: 1.0,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskInfo {
    pub path: String,
    pub free: u64,
    pub total: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub online: bool,
    pub checking: bool,
    pub last_check: Option<String>,
    pub cc_running: bool,
    pub cc_installed: bool,
    pub signed_in: bool,
    pub disks: Vec<DiskInfo>,
    pub cache_size: u64,
    pub scan_step: Option<String>,
    pub first_scan: bool,
    pub app_version: String,
    pub self_update: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub apps: Vec<AppEntry>,
    pub jobs: Vec<Job>,
    pub settings: Settings,
    pub status: Status,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Toast {
    pub kind: String,
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_saved_before_the_tools_source_still_load() {
        // A settings.json from before "More apps" has sources without a "tools" value and no
        // godotInstallDir. It must keep its values instead of falling back to defaults.
        let old = r#"{"checkInterval":9,"sources":{"adobe":true,"craft":false,"blender":true}}"#;
        let s: Settings = serde_json::from_str(old).expect("old settings load");
        assert_eq!(s.check_interval, 9);
        assert!(!s.sources.craft);
        assert!(s.sources.tools);
        assert!(s.godot_install_dir.ends_with("Programs\\Godot"));
        // Get apps hides installed apps unless switched on.
        assert!(!s.show_installed_in_get_apps);
        assert_eq!(Source::Tools.group(), "More apps");
        assert_eq!(serde_json::to_string(&Source::Tools).unwrap(), "\"tools\"");
        // Other apps came later still: off for old settings and for new ones.
        // Other apps is on by default.
        assert!(s.sources.other);
        assert!(Settings::default().sources.other);
        assert_eq!(Source::Other.group(), "Other apps");
        assert_eq!(serde_json::to_string(&Source::Other).unwrap(), "\"other\"");
    }
}
