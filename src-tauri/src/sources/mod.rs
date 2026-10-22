//! The sources of apps. Each one can scan what is installed, look up the latest
//! versions, list what can be installed, and turn a request into a Plan the job runner executes.

pub mod adobe;
pub mod blender;
pub mod craft;
pub mod other;
pub mod tools;
pub mod winget;

use crate::model::{JobKind, Source};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// An installed app as a source sees it.
#[derive(Clone, Debug)]
pub struct Installed {
    pub id: String,
    pub source: Source,
    pub name: String,
    pub code: String,
    pub fg: String,
    pub bg: String,
    pub version: String,
    pub install_dir: Option<PathBuf>,
    pub exe: Option<PathBuf>,
    /// Program and arguments that uninstall it.
    pub uninstall: Option<Vec<String>>,
    pub updated_at: Option<String>,
    pub size_hint: Option<u64>,
    pub tagline: Option<String>,
    pub channel: Option<String>,
    pub missing: bool,
    /// Managed elsewhere (Steam): shown, never updated by us.
    pub read_only: bool,
    pub icon_remote: Option<String>,
    pub data: SourceData,
}

#[derive(Clone, Debug)]
pub enum SourceData {
    Adobe { sap: String, codex: String, platform: String, product_name: String },
    Craft { key: String, msi_product_code: Option<String> },
    Blender { series: String, kind: BlenderKind, msi_product_code: Option<String> },
    /// OBS Studio, Krita, FreeCAD, Godot, Unity. `series` is what makes an installed copy unique
    /// for apps that install side by side (FreeCAD "1.1", Godot "4.7.2", Unity "6000.3.27f1").
    Tools { key: String, series: String },
    /// Matched from the winget catalog: the package, how it was matched and the uninstall list
    /// entry it was matched from.
    Other { pkg: String, how: other::How, entry: Box<crate::winutil::UninstallEntry>, upgrade_code: Option<String> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlenderKind {
    Msi,
    Zip,
    Beta,
    Steam,
}

/// What a source knows about the newest version for an installed app.
#[derive(Clone, Debug, Default)]
pub struct Latest {
    pub version: String,
    pub size: Option<u64>,
    /// "Installs next to 5.2, your 5.2 stays"
    pub note: Option<String>,
}

/// One file to fetch before installing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Download {
    pub url: String,
    pub dest: PathBuf,
    pub size: Option<u64>,
    pub sha256: Option<String>,
    /// Lowercase hex MD5, for vendors that only publish that (Unity).
    #[serde(default)]
    pub md5: Option<String>,
    /// Required Authenticode signer ("Adobe Inc."), for vendors that publish no checksum.
    #[serde(default)]
    pub signer: Option<String>,
    /// Lowercase hex SHA-512, for vendors that publish that (Unity Hub)
    #[serde(default)]
    pub sha512: Option<String>,
    pub headers: Vec<(String, String)>,
}

/// One install action. Elevated steps run in the admin helper.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "t")]
pub enum Step {
    Run {
        exe: PathBuf,
        args: Vec<String>,
        cwd: Option<PathBuf>,
        ok_codes: Vec<u32>,
        elevated: bool,
        label: String,
        log: Option<PathBuf>,
    },
    ExtractZip { zip: PathBuf, dest: PathBuf, strip_root: bool, elevated: bool, label: String },
    RemoveDir { path: PathBuf, elevated: bool, label: String },
    WriteFile { path: PathBuf, contents: String, elevated: bool },
    Shortcut { lnk: PathBuf, target: PathBuf, label: String },
    RemoveFile { path: PathBuf },
    OpenUri { uri: String, label: String },
    /// Delete a key (and everything under it) in the user's own registry hive, HKCU.
    RemoveUserRegKey { path: String, label: String },
}

impl Step {
    pub fn label(&self) -> &str {
        match self {
            Step::Run { label, .. }
            | Step::ExtractZip { label, .. }
            | Step::RemoveDir { label, .. }
            | Step::Shortcut { label, .. }
            | Step::OpenUri { label, .. }
            | Step::RemoveUserRegKey { label, .. } => label,
            Step::WriteFile { .. } => "Writing settings",
            Step::RemoveFile { .. } => "Cleaning up",
        }
    }
    pub fn elevated(&self) -> bool {
        match self {
            Step::Run { elevated, .. } | Step::ExtractZip { elevated, .. } | Step::RemoveDir { elevated, .. } | Step::WriteFile { elevated, .. } => *elevated,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
pub enum Verify {
    /// After the steps the app with this id reports this version.
    Version { app_id: String, version: String },
    /// The app with this id is gone.
    Removed { app_id: String },
    /// Hand-off to another program, nothing to check.
    None,
    /// Hand-off to another program (Creative Cloud): wait up to `minutes` for this version.
    Wait { app_id: String, version: String, minutes: u64 },
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub app_id: String,
    pub kind: JobKind,
    pub name: String,
    pub to_version: String,
    pub downloads: Vec<Download>,
    pub steps: Vec<Step>,
    /// Running programs inside these folders block the install.
    pub block_dirs: Vec<PathBuf>,
    /// Extra process names (lowercase exe names) that block the install.
    pub block_names: Vec<String>,
    pub verify: Verify,
    /// Tried when the main plan fails (Adobe: Remote Update Manager).
    pub fallback: Option<Box<Plan>>,
    /// Files and folders to delete once the job succeeds.
    pub cleanup: Vec<PathBuf>,
}

/// Brand marks for apps that have no icon yet.
pub fn mark(code: &str, fg: &str, bg: &str) -> (String, String, String) {
    (code.to_string(), fg.to_string(), bg.to_string())
}
