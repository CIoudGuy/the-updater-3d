//! Places a leftover scan never deletes, whatever it finds: Windows, the Program Files roots and
//! shared vendor folders, the user's own content folders, browser profiles, other installed
//! programs' folders, and registry keys that belong to Windows or to many programs.

use super::{norm, strictly_under, under, Hive};
use std::path::{Path, PathBuf};

pub struct Guard {
    /// Never deleted, nor anything that holds them (drive roots, Windows, Program Files, the
    /// profile and AppData roots, ProgramData).
    pub roots: Vec<PathBuf>,
    /// Nothing inside these is deleted (Windows, user content folders).
    pub trees: Vec<PathBuf>,
    /// Folders of the other installed programs: not deleted, not inside, not what holds them.
    pub others: Vec<PathBuf>,
    /// Folders a shortcut may be deleted from (Start menu, desktop, Startup).
    pub shortcut_dirs: Vec<PathBuf>,
}

/// Vendor folders many programs share: never deleted whole.
const SHARED_NAMES: &[&str] = &["common files", "windowsapps", "microsoft", "windows", "windows nt", "windows defender", "windows mail", "windows media player", "windows photo viewer", "windows sidebar", "windowspowershell", "internet explorer", "microsoft shared", "reference assemblies", "msbuild", "dotnet", "package cache", "installer", "packages", "programs", "temp", "users", "public", "nvidia corporation", "intel", "amd", "realtek", "steam", "steamapps", "epic games", "google", "mozilla", "jetbrains", "adobe", "autodesk"];

/// Folders right under a drive that hold many programs or games, on any drive (a Steam library
/// on D:, the Xbox app's games, a "Games" folder): never deleted whole.
const TOP_SHARED: &[&str] = &["games", "steamlibrary", "xboxgames", "gog games", "gog galaxy", "ea games", "origin games", "ubisoft", "rockstar games", "amazon games", "itch"];

/// Folders under the profile that hold the user's own files or browser profiles.
const USER_CONTENT: &[&str] = &["Documents", "Desktop", "Pictures", "Videos", "Music", "Downloads", "Saved Games", "Favorites", "Contacts", "Links", "Searches", "3D Objects"];

/// Browser profiles (passwords, history): never deleted, nor anything in them.
const BROWSERS: &[&str] = &[
    "Local\\Google\\Chrome",
    "Local\\Microsoft\\Edge",
    "Local\\BraveSoftware",
    "Local\\Vivaldi",
    "Local\\Chromium",
    "Roaming\\Mozilla",
    "Local\\Mozilla",
    "Roaming\\Opera Software",
    "Roaming\\Waterfox",
    "Roaming\\librewolf",
    "Local\\Thorium",
];

impl Guard {
    /// The real places on this PC; `others` are the other installed programs' folders.
    pub fn system(others: Vec<PathBuf>) -> Guard {
        let env = |n: &str| std::env::var_os(n).map(PathBuf::from);
        let home = env("USERPROFILE").unwrap_or_default();
        let mut roots: Vec<PathBuf> = Vec::new();
        for n in ["SystemRoot", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "CommonProgramFiles", "CommonProgramFiles(x86)", "CommonProgramW6432", "ProgramData", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "PUBLIC", "ALLUSERSPROFILE", "SystemDrive", "TEMP", "TMP"] {
            roots.extend(env(n));
        }
        roots.push(home.join("AppData"));
        roots.push(home.join("AppData\\LocalLow"));
        roots.push(home.join("AppData\\Local\\Programs"));
        if let Some(h) = home.parent() {
            roots.push(h.to_path_buf());
        }
        // Every drive's root, and the folders that hold programs on any drive (programs are
        // installed to "D:\Program Files" too).
        for d in b'A'..=b'Z' {
            let drive = format!("{}:\\", d as char);
            for top in ["", "Program Files", "Program Files (x86)", "ProgramData"] {
                roots.push(PathBuf::from(format!("{drive}{top}")));
            }
        }
        roots.extend(crate::sources::locate::start_menu_dirs());
        let mut trees: Vec<PathBuf> = env("SystemRoot").into_iter().collect();
        // This app's own data, with the backups made here.
        trees.push(crate::paths::data_dir());
        trees.extend(USER_CONTENT.iter().map(|n| home.join(n)));
        for n in ["OneDrive", "OneDriveConsumer", "OneDriveCommercial"] {
            trees.extend(env(n));
        }
        if let Ok(rd) = std::fs::read_dir(&home) {
            trees.extend(rd.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase().starts_with("onedrive"))));
        }
        let ad = home.join("AppData");
        trees.extend(BROWSERS.iter().map(|b| ad.join(b)));
        let mut shortcut_dirs = crate::sources::locate::start_menu_dirs();
        shortcut_dirs.push(home.join("Desktop"));
        shortcut_dirs.extend(env("PUBLIC").map(|p| p.join("Desktop")));
        shortcut_dirs.extend(env("APPDATA").map(|p| p.join("Microsoft\\Internet Explorer\\Quick Launch")));
        Guard { roots, trees, others, shortcut_dirs }
    }

    /// Why a folder must not be deleted, or None when it may.
    pub fn dir_refused(&self, p: &Path) -> Option<&'static str> {
        if !p.is_absolute() || p.components().count() < 3 {
            return Some("a drive or top folder");
        }
        if self.roots.iter().any(|r| under(r, p)) {
            return Some("a Windows or system folder, or one that holds it");
        }
        if self.trees.iter().any(|t| under(p, t) || under(t, p)) {
            return Some("Windows, your own files or a browser profile");
        }
        if self.others.iter().any(|o| under(p, o) || under(o, p)) {
            return Some("another installed program's folder");
        }
        let name = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        if SHARED_NAMES.contains(&name.as_str()) || (p.components().count() == 3 && TOP_SHARED.contains(&name.as_str())) {
            return Some("a folder many programs share");
        }
        if p.join("User Data").is_dir() || p.join("profiles.ini").is_file() {
            return Some("a browser profile");
        }
        None
    }

    /// Why a file must not be deleted. Only shortcuts in the Start menu, on a desktop or in a
    /// Startup folder are ever deleted as files (settings files come from `prefs`).
    pub fn shortcut_refused(&self, p: &Path) -> Option<&'static str> {
        if !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
            return Some("not a shortcut");
        }
        if !self.shortcut_dirs.iter().any(|d| strictly_under(p, d)) {
            return Some("not in the Start menu, on a desktop or in Startup");
        }
        None
    }
}

/// Registry keys that belong to Windows or to many programs: never deleted.
pub fn key_refused(hive: Hive, path: &str) -> Option<&'static str> {
    let low = path.trim_matches('\\').to_ascii_lowercase();
    let parts: Vec<&str> = low.split('\\').filter(|s| !s.is_empty()).collect();
    if parts.is_empty() {
        return Some("a hive root");
    }
    // Under SOFTWARE: skip "WOW6432Node" to see what the key really is.
    let rest: Vec<&str> = if parts[0] == "software" {
        let r = &parts[1..];
        if r.first() == Some(&"wow6432node") {
            r[1..].to_vec()
        } else {
            r.to_vec()
        }
    } else if hive == Hive::Hklm && parts.len() == 4 && parts[0] == "system" && parts[1] == "currentcontrolset" && parts[2] == "services" {
        // One service's own key.
        return None;
    } else {
        return Some("outside Software");
    };
    let Some(first) = rest.first() else { return Some("the Software key") };
    let s = |a: &str| rest.len() > 1 && rest.join("\\").starts_with(a);
    match *first {
        "microsoft" => {
            // Only one program's own entry in these lists, never the list.
            let ok = (s("microsoft\\windows\\currentversion\\uninstall\\") && rest.len() == 5) || (s("microsoft\\windows\\currentversion\\app paths\\") && rest.len() == 5);
            if ok {
                None
            } else {
                Some("a Windows key")
            }
        }
        "classes" => {
            // One file type or program key ("Vendor.Document", "Applications\x.exe"), never an
            // extension, CLSID or interface list.
            match rest.len() {
                2 if !rest[1].starts_with('.') && !["clsid", "interface", "typelib", "appid", "applications", "directory", "folder", "drive", "*", "allfilesystemobjects", "wow6432node", "local settings", "installer", "systemfileassociations", "mime"].contains(&rest[1]) => None,
                3 if rest[1] == "applications" && rest[2].ends_with(".exe") => None,
                _ => Some("a shared file type key"),
            }
        }
        "policies" | "clients" | "registeredapplications" | "odbc" | "intel" | "nvidia corporation" | "khronos" | "wow6432node" | "windows" => Some("a key many programs share"),
        _ => None,
    }
}

/// Values are only ever deleted from the startup lists (Run, RunOnce).
pub fn value_refused(hive: Hive, path: &str) -> Option<&'static str> {
    let _ = hive;
    let low = path.trim_matches('\\').to_ascii_lowercase();
    let ok = ["software\\microsoft\\windows\\currentversion\\run", "software\\microsoft\\windows\\currentversion\\runonce", "software\\wow6432node\\microsoft\\windows\\currentversion\\run", "software\\wow6432node\\microsoft\\windows\\currentversion\\runonce"];
    if ok.contains(&low.as_str()) {
        None
    } else {
        Some("not a startup list")
    }
}

/// `path` with its folders normalized, for comparing.
pub fn same_dir(a: &Path, b: &Path) -> bool {
    norm(a) == norm(b)
}
