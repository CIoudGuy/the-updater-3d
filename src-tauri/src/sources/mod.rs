//! The sources of apps. Each one can scan what is installed, look up the latest
//! versions, list what can be installed, and turn a request into a Plan the job runner executes.

pub mod about;
pub mod adobe;
pub mod amd;
pub mod blender;
pub mod board;
pub mod browse;
pub mod chipset;
pub mod craft;
pub mod drivers;
pub mod nvidia;
pub mod icons;
pub mod intel;
pub mod msix;
pub mod msstore;
pub mod locate;
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
    /// Managed elsewhere (Steam, the Microsoft Store): shown, never updated or removed by us.
    pub read_only: bool,
    pub icon_remote: Option<String>,
    pub data: SourceData,
    /// A Microsoft Store copy: its package, to open it and its Store page.
    pub store: Option<StoreRef>,
}

/// A Microsoft Store (MSIX) package an installed copy came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoreRef {
    /// Package family name: "BlenderFoundation.Blender_ppwjx1n5r4v9t"
    pub family: String,
    /// The application id inside the package (from its manifest), to start it.
    pub app: Option<String>,
}

impl StoreRef {
    /// The app's page in the Microsoft Store app.
    pub fn page_url(&self) -> String {
        format!("ms-windows-store://pdp/?PFN={}", self.family)
    }
    /// What starts the app through the shell, when its application id is known.
    pub fn launch_target(&self) -> Option<String> {
        self.app.as_ref().map(|a| format!("shell:AppsFolder\\{}!{a}", self.family))
    }
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
    /// From the Microsoft Store, which updates it.
    Store,
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
    /// Delete an app folder after its uninstaller ran, only when nothing but the uninstaller and
    /// its logs is left in it (`winutil::only_uninstaller_left`); otherwise leave it as it is.
    RemoveIfOnlyUninstaller { path: PathBuf, elevated: bool, label: String },
    /// Stop unless Windows trusts the file's Authenticode signature by exactly `signer`: for a
    /// program unpacked from a download that has no published checksum (Adobe's RUM).
    CheckSigner { path: PathBuf, signer: String, label: String },
    /// Install an MSIX or APPX package for this user with Windows' package manager, after
    /// checking that it is package family `family` with a trusted signature (and the signature
    /// file `signature_sha256`, when the catalog gives one). `update`: Windows closes the app if
    /// it still runs; `any_version`: an older version may replace a newer one (a roll back).
    /// `elevated`: in the admin helper (same user), for a package with a Windows service in it,
    /// which Windows only installs with admin rights (WSL); see `admin_retry`.
    AddPackage {
        path: PathBuf,
        family: String,
        signature_sha256: Option<String>,
        update: bool,
        any_version: bool,
        #[serde(default)]
        elevated: bool,
        label: String,
    },
    /// Remove this user's packages of the family with Windows' package manager.
    RemovePackage {
        family: String,
        #[serde(default)]
        elevated: bool,
        label: String,
    },
    /// Delete one thing an uninstalled program left behind (`uninstall::remove::apply`, which
    /// checks again that it is safe to delete).
    Leftover { target: crate::uninstall::Target, elevated: bool, label: String },
}

impl Step {
    pub fn label(&self) -> &str {
        match self {
            Step::Run { label, .. }
            | Step::ExtractZip { label, .. }
            | Step::RemoveDir { label, .. }
            | Step::Shortcut { label, .. }
            | Step::OpenUri { label, .. }
            | Step::RemoveUserRegKey { label, .. }
            | Step::RemoveIfOnlyUninstaller { label, .. }
            | Step::CheckSigner { label, .. }
            | Step::AddPackage { label, .. }
            | Step::RemovePackage { label, .. }
            | Step::Leftover { label, .. } => label,
            Step::WriteFile { .. } => "Writing settings",
            Step::RemoveFile { .. } => "Cleaning up",
        }
    }
    pub fn elevated(&self) -> bool {
        match self {
            Step::Run { elevated, .. } | Step::ExtractZip { elevated, .. } | Step::RemoveDir { elevated, .. } | Step::WriteFile { elevated, .. } | Step::RemoveIfOnlyUninstaller { elevated, .. } | Step::Leftover { elevated, .. } | Step::AddPackage { elevated, .. } | Step::RemovePackage { elevated, .. } => *elevated,
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

impl Plan {
    /// Does this plan run an installer, uninstaller or other program (MSI, setup.exe, a hand-off
    /// to another app)? Those clash with each other, so they run one at a time. Plans that only
    /// unpack, copy, move, delete or write files, shortcuts and our own registry entries do not.
    pub fn runs_installer(&self) -> bool {
        self.steps.iter().any(|s| matches!(s, Step::Run { .. } | Step::OpenUri { .. } | Step::AddPackage { .. } | Step::RemovePackage { .. }))
    }

    /// Files other programs may have loaded that the installer will not replace while they do:
    /// the programs using them must close first, like the app itself.
    pub fn block_files(&self) -> Vec<PathBuf> {
        match self.app_id.as_str() {
            "tools:obs" => self.block_dirs.first().map(|d| tools::obs_in_use_files(d)).unwrap_or_default(),
            _ => vec![],
        }
    }

    /// What the plan writes to: the app and the folders it unpacks into or deletes, lowercase.
    /// Two jobs that share one of these never run their steps at the same time.
    pub fn write_keys(&self) -> Vec<String> {
        let norm = |p: &std::path::Path| format!("dir:{}", p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase());
        let mut v = vec![format!("app:{}", self.app_id.to_ascii_lowercase())];
        for s in &self.steps {
            match s {
                Step::ExtractZip { dest, .. } => v.push(norm(dest)),
                Step::RemoveDir { path, .. } | Step::RemoveIfOnlyUninstaller { path, .. } => v.push(norm(path)),
                _ => {}
            }
        }
        v.sort();
        v.dedup();
        v
    }
}

/// A step whose label starts with this opens an installer the user must click through.
pub const WAITING_FOR_YOU: &str = "Waiting for you";

/// Installer exit codes that mean "installed, restart to finish": Windows Installer's (also ours
/// for Windows Update drivers) and, for NVIDIA's package, 1.
pub fn restart_code(app_id: &str, code: u32) -> bool {
    code == 3010 || code == 1641 || (code == 1 && app_id.starts_with("driver:nvidia:"))
}

/// Brand marks for apps that have no icon yet.
pub fn mark(code: &str, fg: &str, bg: &str) -> (String, String, String) {
    (code.to_string(), fg.to_string(), bg.to_string())
}

/// Is `latest` an update for the installed `installed`? A final release is newer than its own
/// pre-releases ("2.0.0" over "2.0.0-beta.3"); a beta channel copy takes any other build.
pub fn update_offered(latest: &str, installed: &str, beta_channel: bool) -> bool {
    tools::cmp_ver(latest, installed) == std::cmp::Ordering::Greater || (beta_channel && latest != installed)
}

#[cfg(test)]
mod update_tests {
    use super::update_offered;

    #[test]
    fn a_final_release_updates_its_pre_release() {
        assert!(update_offered("2.0.0", "2.0.0-beta.3", false));
        assert!(!update_offered("2.0.0-beta.3", "2.0.0", false));
        // Other formats keep their order.
        assert!(!update_offered("4.2.3", "4.2.3 LTS", false));
        assert!(!update_offered("4.2.3 LTS", "4.2.3", false));
        assert!(update_offered("6000.3.27f1", "6000.3.26f1", false));
        assert!(!update_offered("6000.3.27f1", "6000.3.27f1", false));
        assert!(update_offered("27.11", "27.2", false));
        assert!(!update_offered("27.11", "27.11.0", false));
        assert!(update_offered("5.4.0 Alpha fbe82a4", "5.3.0 Beta 873fa96", true));
    }
}

impl Download {
    /// Is there anything to check the file against (a checksum or a required signer)?
    pub fn is_checked(&self) -> bool {
        self.sha256.as_deref().is_some_and(|s| !s.is_empty())
            || self.md5.as_deref().is_some_and(|s| !s.is_empty())
            || self.sha512.as_deref().is_some_and(|s| !s.is_empty())
            || self.signer.as_deref().is_some_and(|s| !s.is_empty())
    }
}

/// Plans never download a program that cannot be checked: `what` is the app and version.
pub fn require_checked(d: &Download, what: &str) -> anyhow::Result<()> {
    if d.is_checked() {
        Ok(())
    } else {
        anyhow::bail!("Could not get the checksum for {what}. Try again later.")
    }
}

/// The result of `Step::CheckSigner`: `got` is the trusted signer Windows reports, if any.
pub fn check_signer(path: &std::path::Path, want: &str, got: Option<String>) -> anyhow::Result<()> {
    match got {
        Some(g) if g == want => Ok(()),
        Some(g) => anyhow::bail!("{} is signed by {g}, not {want}, so it was not run", path.display()),
        None => anyhow::bail!("{} is not validly signed by {want}, so it was not run", path.display()),
    }
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    fn plan(app_id: &str, steps: Vec<Step>) -> Plan {
        Plan { app_id: app_id.into(), kind: JobKind::Update, name: "X".into(), to_version: "1".into(), downloads: vec![], steps, block_dirs: vec![], block_names: vec![], verify: Verify::None, fallback: None, cleanup: vec![] }
    }

    #[test]
    fn obs_waits_for_the_apps_that_have_its_capture_hook_or_virtual_camera_loaded() {
        // OBS 32.2.2 to 33.0.0 stopped with exit code 6: its files were in use by other apps.
        let mut p = plan("tools:obs", vec![run(r"C:\dl\OBS-Studio-33.0.0-Windows-x64-Installer.exe")]);
        p.block_dirs = vec![r"C:\Program Files\obs-studio".into()];
        let files = p.block_files();
        assert!(files.contains(&PathBuf::from(r"C:\Program Files\obs-studio\data\obs-plugins\win-capture\graphics-hook64.dll")), "{files:?}");
        assert!(files.contains(&PathBuf::from(r"C:\Program Files\obs-studio\data\obs-plugins\win-dshow\obs-virtualcam-module64.dll")));
        assert!(files.iter().any(|f| f.ends_with(r"obs-studio-hook\graphics-hook64.dll")));
        assert!(plan("tools:krita", vec![]).block_files().is_empty());
    }

    fn run(exe: &str) -> Step {
        Step::Run { exe: exe.into(), args: vec![], cwd: None, ok_codes: vec![0], elevated: true, label: "Installing".into(), log: None }
    }

    #[test]
    fn only_plans_that_run_a_program_wait_for_the_install_queue() {
        // A Blender zip: unpack, swap folders, shortcut, settings. Nothing that clashes with an MSI.
        let unzip = plan(
            "blender:5.2",
            vec![
                Step::ExtractZip { zip: "C:/dl/b.zip".into(), dest: "D:/Blender/5.2.new".into(), strip_root: true, elevated: false, label: "Unpacking".into() },
                Step::RemoveDir { path: "D:/Blender/5.2".into(), elevated: false, label: "Removing the old version".into() },
                Step::Shortcut { lnk: "C:/Start/Blender.lnk".into(), target: "D:/Blender/5.2/blender.exe".into(), label: "Shortcut".into() },
                Step::WriteFile { path: "D:/Blender/5.2/portable.txt".into(), contents: String::new(), elevated: false },
                Step::RemoveUserRegKey { path: "Software\\X".into(), label: "Cleaning".into() },
                Step::CheckSigner { path: "C:/x.exe".into(), signer: "Adobe Inc.".into(), label: "Checking".into() },
            ],
        );
        assert!(!unzip.runs_installer());
        assert!(plan("x", vec![run("C:/Windows/System32/msiexec.exe")]).runs_installer());
        assert!(plan("x", vec![run("C:/dl/setup.exe")]).runs_installer());
        assert!(plan("x", vec![Step::OpenUri { uri: "unityhub://6000.3.27f1".into(), label: "Hub".into() }]).runs_installer());
        // Windows' package manager installs one package at a time too.
        assert!(plan("x", vec![Step::AddPackage { path: "C:/dl/a.msix".into(), family: "A_x".into(), signature_sha256: None, update: false, any_version: false, elevated: false, label: "Installing".into() }]).runs_installer());
        assert!(plan("x", vec![Step::RemovePackage { family: "A_x".into(), elevated: false, label: "Uninstalling".into() }]).runs_installer());
        // An uninstall runs the vendor's uninstaller.
        assert!(plan("x", vec![run("C:/Program Files/App/uninstall.exe"), Step::RemoveDir { path: "C:/Program Files/App".into(), elevated: true, label: "Rest".into() }]).runs_installer());
        assert!(!plan("x", vec![]).runs_installer());
    }

    #[test]
    fn jobs_writing_the_same_folder_share_a_key() {
        let a = plan("blender:5.2", vec![Step::ExtractZip { zip: "a.zip".into(), dest: "D:/Blender/5.2".into(), strip_root: true, elevated: false, label: String::new() }]);
        let b = plan("blender:5.2-beta", vec![Step::RemoveDir { path: "d:\\blender\\5.2\\".into(), elevated: false, label: String::new() }]);
        let ka = a.write_keys();
        let kb = b.write_keys();
        assert!(ka.iter().any(|k| kb.contains(k)), "{ka:?} {kb:?}");
        assert!(ka.contains(&"app:blender:5.2".to_string()));
        let c = plan("blender:4.5", vec![Step::ExtractZip { zip: "c.zip".into(), dest: "D:/Blender/4.5".into(), strip_root: true, elevated: false, label: String::new() }]);
        assert!(!c.write_keys().iter().any(|k| ka.contains(k)));
    }
}
