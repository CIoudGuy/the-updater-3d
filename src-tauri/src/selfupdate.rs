//! Updates for The Updater 3D itself, from a JSON release feed (format in the README).
//!
//! The feed address and its key are build settings: `build.rs` reads them from
//! `src-tauri/.update-feed` and `src-tauri/.update-key`, or from the
//! UPDATER3D_FEED_URL / UPDATER3D_UPDATE_KEY environment variables. A key in
//! `<data>\update-key.txt` overrides the built-in one. Without a feed the app never offers
//! self-updates. Installers are checked against the SHA-256 the feed lists.

use crate::download::{self, Control, Progress, Throttle};
use crate::sources::Download;
use crate::{http, paths, version, winutil};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

const FEED: Option<&str> = option_env!("UPDATER3D_FEED_URL");
const BUILD_KEY: Option<&str> = option_env!("UPDATER3D_UPDATE_KEY");

fn key() -> Option<String> {
    std::fs::read_to_string(paths::data_dir().join("update-key.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| BUILD_KEY.map(String::from))
}

/// A random id saved once, so the creator panel can count installs.
fn install_id() -> String {
    let p = paths::data_dir().join("install-id.txt");
    if let Ok(s) = std::fs::read_to_string(&p) {
        let s = s.trim().to_string();
        if s.len() >= 16 {
            return s;
        }
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let _ = std::fs::write(&p, &id);
    id
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub url: String,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub name: String,
}

pub fn enabled() -> bool {
    FEED.is_some() && key().is_some()
}

/// The newest release on the stable channel when it is newer than this build.
pub async fn check() -> Result<Option<Release>> {
    let (Some(feed), Some(k)) = (FEED, key()) else { return Ok(None) };
    let url = format!("{feed}?current={}", env!("CARGO_PKG_VERSION"));
    let resp = http::CLIENT
        .get(&url)
        .header("X-Update-Key", &k)
        .header("X-Install-Id", install_id())
        .header("X-Client", "windows")
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    if !resp.status().is_success() {
        bail!("Update feed answered HTTP {}", resp.status());
    }
    let v: Value = resp.json().await?;
    let ch = &v["app"]["channels"]["stable"];
    let ver = ch["version"].as_str().unwrap_or("");
    if ver.is_empty() || !version::newer(ver, env!("CARGO_PKG_VERSION")) {
        return Ok(None);
    }
    let files = ch["files"].as_array().cloned().unwrap_or_default();
    let f = files
        .iter()
        .find(|f| f["platform"].as_str().is_some_and(|p| p.starts_with("windows")) && f["name"].as_str().is_some_and(|n| n.to_ascii_lowercase().ends_with(".exe")))
        .or_else(|| files.iter().find(|f| f["name"].as_str().is_some_and(|n| n.to_ascii_lowercase().ends_with("-setup.exe"))))
        .ok_or_else(|| anyhow!("Release {ver} has no Windows installer"))?;
    Ok(Some(Release {
        version: ver.to_string(),
        notes: ch["notes"].as_str().unwrap_or("").to_string(),
        url: f["url"].as_str().unwrap_or("").to_string(),
        sha256: f["sha256"].as_str().map(|s| s.to_ascii_lowercase()),
        size: f["size"].as_u64(),
        name: f["name"].as_str().unwrap_or("setup.exe").to_string(),
    }))
}

/// Download the installer, verify it, run it silently and start the new version afterwards.
/// The caller exits the app right after this returns.
pub async fn install(r: &Release) -> Result<()> {
    let k = key().ok_or_else(|| anyhow!("No update key"))?;
    let dest = paths::downloads().join("self").join(&r.name);
    let d = Download { url: r.url.clone(), dest: dest.clone(), size: r.size, sha256: r.sha256.clone(), headers: vec![("X-Update-Key".into(), k)] };
    if r.sha256.is_none() {
        bail!("The release has no checksum, so it was not installed");
    }
    let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
    let progress = Arc::new(Progress::new());
    download::fetch(&d, progress, Arc::new(Throttle::new()), rx).await.context("Downloading the update")?;
    let exe = std::env::current_exe()?;
    let log = paths::logs().join("self-update.log");
    let q = |p: &std::path::Path| p.display().to_string().replace('\'', "''");
    // Runs after we exit: wait until this process is really gone (the installer refuses to
    // replace a running app), close any other copy of this exe, install, then start the new one.
    let script = [
        format!("$log = '{}'", q(&log)),
        "function L($m) { (Get-Date -Format s) + ' ' + $m | Out-File -FilePath $log -Append -Encoding utf8 }".to_string(),
        format!("L 'waiting for process {} to exit'", std::process::id()),
        format!("Wait-Process -Id {} -Timeout 60 -ErrorAction SilentlyContinue", std::process::id()),
        format!(
            "Get-Process -Name 'the-updater-3d' -ErrorAction SilentlyContinue | Where-Object {{ $_.Path -eq '{}' }} | Stop-Process -Force -ErrorAction SilentlyContinue",
            q(&exe)
        ),
        "Start-Sleep -Milliseconds 400".to_string(),
        format!("L 'installing {}'", r.version),
        format!("$p = Start-Process -FilePath '{}' -ArgumentList '/S' -Wait -PassThru", q(&dest)),
        "L ('installer exit code ' + $p.ExitCode)".to_string(),
        format!("Start-Process -FilePath '{}'", q(&exe)),
        "L 'started the new version'".to_string(),
    ]
    .join("; ");
    winutil::cmd("powershell.exe").args(["-NoProfile", "-NonInteractive", "-WindowStyle", "Hidden", "-Command", &script]).spawn()?;
    Ok(())
}
