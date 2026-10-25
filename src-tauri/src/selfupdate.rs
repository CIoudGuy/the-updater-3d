//! Updates for The Updater 3D itself, from a JSON release feed (format in the README).
//!
//! The feed address and its key are build settings: `build.rs` reads them from
//! `src-tauri/.update-feed` and `src-tauri/.update-key`, or from the
//! UPDATER3D_FEED_URL / UPDATER3D_UPDATE_KEY environment variables. A key in
//! `<data>\update-key.txt` overrides the built-in one. Without a feed the app never offers
//! self-updates. Installers are checked against the SHA-256 the feed lists.

use crate::download::{self, Control, Progress, Throttle};
use crate::sources::Download;
use crate::{http, paths, version};
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

/// The release to install: what the feed offers now (`fresh`), as the feed keeps only its
/// newest release and one found at an earlier check is gone (404) once a newer one is out; the
/// one found earlier (`saved`) when the feed can not be read; none when the feed has nothing
/// newer than this build any more.
pub fn current(saved: Option<Release>, fresh: Result<Option<Release>>) -> Option<Release> {
    match fresh {
        Ok(f) => f,
        Err(e) => {
            log::warn!("self-update: the feed could not be read again ({e:#}), using the release found before");
            saved
        }
    }
}

/// The app's own public project, where every release and its notes are.
pub const REPO: &str = "CIoudGuy/the-updater-3d";

/// A release's text without the checksum lines at its end (they are not changes).
fn changes_of(body: &str) -> &str {
    body.find("SHA-256 of").map(|i| &body[..i]).unwrap_or(body)
}

/// The version history of this app: its GitHub releases, newest first, with their notes, and
/// the release the update feed offers when GitHub does not have it yet. `installed` is this build.
pub fn history(rels: &[crate::sources::ghapps::Release], offered: Option<&Release>, installed: &str) -> Vec<crate::model::VersionInfo> {
    use crate::model::VersionInfo;
    let newest = rels.iter().filter(|r| !r.prerelease).map(|r| r.version.clone()).next();
    let mut out: Vec<VersionInfo> = rels
        .iter()
        .take(20)
        .map(|r| VersionInfo {
            version: r.version.clone(),
            date: r.date.clone(),
            notes: crate::notes::text_lines(changes_of(&r.body)),
            notes_url: Some(r.url.clone()).filter(|u| !u.is_empty()),
            size: r.assets.iter().find(|a| a.name.to_ascii_lowercase().ends_with("-setup.exe")).and_then(|a| a.size),
            installed: version::cmp(&r.version, installed).is_eq(),
            latest: false,
            prerelease: r.prerelease,
        })
        .collect();
    if let Some(o) = offered.filter(|o| !out.iter().any(|v| version::cmp(&v.version, &o.version).is_eq())) {
        out.insert(0, VersionInfo { version: o.version.clone(), date: None, notes: crate::notes::text_lines(changes_of(&o.notes)), notes_url: None, size: o.size, installed: false, latest: false, prerelease: false });
    }
    let top = offered.map(|o| o.version.clone()).or(newest);
    if let Some(v) = out.iter_mut().find(|v| Some(&v.version) == top.as_ref()) {
        v.latest = true;
    }
    out
}

/// This app's releases on GitHub (kept for an hour).
pub async fn releases() -> Vec<crate::sources::ghapps::Release> {
    let url = format!("https://api.github.com/repos/{REPO}/releases?per_page=20");
    let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(15), http::get_cached(&url, &[("Accept", "application/vnd.github+json")], Duration::from_secs(3600))).await else { return vec![] };
    crate::sources::ghapps::parse_releases(&serde_json::from_slice(&f.body).unwrap_or(Value::Null))
}

/// Where this app is, how big it is and when it was last installed.
pub fn about() -> crate::model::SelfInfo {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_ref().and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let size = dir.as_ref().map(|d| crate::winutil::dir_size(d)).filter(|s| *s > 0);
    let updated = exe.as_ref().and_then(|e| std::fs::metadata(e).ok()).and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Utc>::from(t).format("%Y-%m-%dT%H:%M:%SZ").to_string());
    crate::model::SelfInfo {
        version: env!("CARGO_PKG_VERSION").to_string(),
        install_dir: dir.map(|d| d.display().to_string()),
        size_on_disk: size,
        updated_at: updated,
        project_url: format!("https://github.com/{REPO}"),
    }
}

/// Download the installer, verify it, run it silently and start the new version afterwards.
/// The caller exits the app right after this returns.
pub async fn install(r: &Release) -> Result<()> {
    let k = key().ok_or_else(|| anyhow!("No update key"))?;
    let dest = paths::downloads().join("self").join(&r.name);
    let d = Download { url: r.url.clone(), dest: dest.clone(), size: r.size, sha256: r.sha256.clone(), md5: None, signer: None, sha512: None, headers: vec![("X-Update-Key".into(), k)] };
    if r.sha256.is_none() {
        bail!("The release has no checksum, so it was not installed");
    }
    let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
    let progress = Arc::new(Progress::new());
    download::fetch(&d, progress, Arc::new(Throttle::new()), rx).await.context("Downloading the update")?;
    // The installer does the rest itself, like Tauri's own updater: /S runs silently, it closes
    // this app through the Windows Restart Manager, /UPDATE keeps shortcuts and settings, and /R
    // starts the new version when done. No script or hidden shell (antivirus tools rightly stop those).
    log::info!("self-update: starting the {} installer", r.version);
    std::process::Command::new(&dest).args(["/S", "/UPDATE", "/R"]).spawn().context("Starting the installer")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn rels() -> Vec<crate::sources::ghapps::Release> {
        let body = |v: &str| format!("- **Programs from GitHub update here.** Version {v}.\n\nSHA-256 of `TheUpdater3D_{v}_x64-setup.exe`:\n`09fad6c41659644d6b2d4bc54800c93384667d7c0b717044baf57ccbea79c28d`\n");
        let rel = |v: &str| json!({ "tag_name": format!("v{v}"), "draft": false, "prerelease": false, "html_url": format!("https://github.com/{REPO}/releases/tag/v{v}"), "published_at": "2026-10-24T09:30:00Z", "body": body(v), "assets": [{ "name": format!("TheUpdater3D_{v}_x64-setup.exe"), "size": 7685729, "browser_download_url": format!("https://github.com/{REPO}/releases/download/v{v}/TheUpdater3D_{v}_x64-setup.exe") }] });
        crate::sources::ghapps::parse_releases(&json!([rel("1.1.8"), rel("1.1.7")]))
    }

    #[test]
    fn the_update_installed_is_the_one_the_feed_offers_now() {
        let rel = |v: &str| Release { version: v.into(), notes: String::new(), url: format!("https://files.example/{v}.exe"), sha256: Some("ab".repeat(32)), size: Some(1), name: format!("TheUpdater3D_{v}_x64-setup.exe") };
        // 1.2.8 was found at the last check; the feed has replaced it with 1.2.9 since (its 1.2.8
        // file is gone): 1.2.9 is installed.
        assert_eq!(current(Some(rel("1.2.8")), Ok(Some(rel("1.2.9")))).map(|r| r.version).as_deref(), Some("1.2.9"));
        // The feed can not be read: the one found before.
        assert_eq!(current(Some(rel("1.2.8")), Err(anyhow!("offline"))).map(|r| r.version).as_deref(), Some("1.2.8"));
        // Nothing newer than this build any more: nothing.
        assert!(current(Some(rel("1.2.8")), Ok(None)).is_none());
    }

    #[test]
    fn the_apps_own_history_has_each_releases_changes_without_its_checksum() {
        let list = history(&rels(), None, "1.1.7");
        assert_eq!(list.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["1.1.8", "1.1.7"]);
        assert!(list[0].latest && !list[0].installed);
        assert!(list[1].installed && !list[1].latest);
        assert_eq!(list[0].notes, ["Programs from GitHub update here. Version 1.1.8"]);
        assert_eq!(list[0].size, Some(7685729));
        assert_eq!(list[0].notes_url.as_deref(), Some("https://github.com/CIoudGuy/the-updater-3d/releases/tag/v1.1.8"));
    }

    #[test]
    fn a_version_the_feed_offers_before_github_has_it_comes_first() {
        let offered = Release { version: "1.1.9".into(), notes: "- **Faster checks.**\n\nSHA-256 of `x`:\n`abc`".into(), url: String::new(), sha256: None, size: Some(1), name: "setup.exe".into() };
        let list = history(&rels(), Some(&offered), "1.1.8");
        assert_eq!(list.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["1.1.9", "1.1.8", "1.1.7"]);
        assert!(list[0].latest && list[1].installed);
        assert_eq!(list[0].notes, ["Faster checks"]);
        // Once GitHub has it, it is listed once.
        let offered = Release { version: "1.1.8".into(), ..offered };
        assert_eq!(history(&rels(), Some(&offered), "1.1.7").len(), 2);
    }
}
