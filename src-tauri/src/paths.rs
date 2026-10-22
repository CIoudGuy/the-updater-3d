//! Where the app keeps its files. Everything lives under %LOCALAPPDATA%\app.theupdater3d\data.

use std::path::PathBuf;

/// Same folder Tauri uses for this app (its identifier), so the installer's own folder
/// (%LOCALAPPDATA%\The Updater 3D) never holds user data.
pub const APP_DIR: &str = "app.theupdater3d";

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let p = base.join(APP_DIR).join("data");
    let _ = std::fs::create_dir_all(&p);
    p
}

fn sub(name: &str) -> PathBuf {
    let p = data_dir().join(name);
    let _ = std::fs::create_dir_all(&p);
    p
}

/// Downloaded installers. Cleared by Settings > Storage > Clear cache.
pub fn downloads() -> PathBuf {
    sub("downloads")
}

/// HTTP response cache (catalogs, feeds) with their ETags.
pub fn http_cache() -> PathBuf {
    sub("http-cache")
}

/// App icons, served to the webview through the asset protocol.
pub fn icons() -> PathBuf {
    sub("icons")
}

pub fn logs() -> PathBuf {
    sub("logs")
}

pub fn settings_file() -> PathBuf {
    data_dir().join("settings.json")
}

pub fn activity_file() -> PathBuf {
    data_dir().join("activity.jsonl")
}

pub fn state_file() -> PathBuf {
    data_dir().join("state.json")
}

pub fn worker_file() -> PathBuf {
    data_dir().join("worker.json")
}

pub fn program_files() -> PathBuf {
    std::env::var_os("ProgramFiles")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Program Files"))
}

pub fn program_files_x86() -> PathBuf {
    std::env::var_os("ProgramFiles(x86)")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("C:\\Program Files (x86)"))
}

pub fn local_app_data() -> PathBuf {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_default()
}

pub fn app_data() -> PathBuf {
    std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_default()
}

/// URL the webview can load for a local file (Tauri asset protocol on Windows).
pub fn asset_url(p: &std::path::Path) -> String {
    let s = p.to_string_lossy();
    let mut out = String::from("http://asset.localhost/");
    for b in s.as_bytes() {
        match *b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}
