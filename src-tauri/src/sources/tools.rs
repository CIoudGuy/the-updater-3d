//! More apps: OBS Studio, Krita, FreeCAD, Godot, Unity Editor, Unity Hub and the Epic Games Launcher (Preview).
//!
//! One small table entry per app (`APPS`), and a few lookups per app:
//!
//! * OBS Studio, FreeCAD and Godot: GitHub Releases (ETag cached). GitHub publishes a SHA-256
//!   `digest` for each asset, which the download is checked against.
//! * Krita: the KDE download index (`download.kde.org`), checksum from the `.sha256` file next to
//!   the installer, change list from the release post on krita.org.
//! * Unity Editor: Unity's public release API; the installer is checked by exact size and MD5.
//! * Epic Games Launcher (Preview): the version is read from where Epic's download link leads;
//!   no checksum is published, so the installer is checked by exact size and its Authenticode signer.
//!
//! Installed copies come from the Windows uninstall list, plus folders for the portable Godot
//! copies and for editors installed by the Unity Hub.

use super::{Download, Installed, Latest, Plan, SourceData, Step, Verify};
use crate::model::{CatalogEntry, JobKind, Source, VersionInfo};
use crate::{download, http, notes, paths, version, winutil};
use anyhow::{anyhow, bail, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Written next to portable copies so they can be recognised later.
pub const MARKER: &str = ".the-updater-3d.json";

const KRITA_STABLE: &str = "https://download.kde.org/stable/krita/";
const KRITA_ATTIC: &str = "https://download.kde.org/Attic/krita/";
const UNITY_API: &str = "https://services.api.unity.com/unity/editor/release/v1/releases";
/// Unity Hub's update feed (electron-builder): newest version, installer, size and SHA-512.
const HUB_FEED: &str = "https://public-cdn.cloud.unity3d.com/hub/prod/latest.yml";
const HUB_FILES: &str = "https://public-cdn.cloud.unity3d.com/hub/prod/";
const HUB_NOTES: &str = "https://unity.com/unity-hub/release-notes";
/// Epic's download link for the new launcher (it answers with a redirect to the versioned file).
const EPIC_DOWNLOAD: &str = "https://launcher-public-service-prod06.ol.epicgames.com/launcher/api/installer/download/EpicGamesLauncherSetup-x86_64.exe";
const EPIC_PAGE: &str = "https://launcher.epicgames.com/";
/// The name on Epic's code signing certificate, as `winutil::authenticode_signer` returns it.
const EPIC_SIGNER: &str = "Epic Games Inc.";

pub struct AppDef {
    pub key: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    pub code: &'static str,
    pub fg: &'static str,
    pub bg: &'static str,
    /// Official icon for the Get apps list (installed apps use the icon of their program file).
    pub icon: &'static str,
    /// Several copies can be installed next to each other.
    pub side_by_side: bool,
}

pub const APPS: &[AppDef] = &[
    AppDef { key: "obs", name: "OBS Studio", tagline: "Recording and live streaming", code: "Ob", fg: "#E4E4EC", bg: "#1E1E27", icon: "https://obsproject.com/assets/images/new_icon_small-r.png", side_by_side: false },
    AppDef { key: "krita", name: "Krita", tagline: "Digital painting and illustration", code: "Kr", fg: "#E38BC4", bg: "#2A1222", icon: "https://raw.githubusercontent.com/KDE/krita/master/krita/pics/branding/default/256-apps-krita.png", side_by_side: false },
    AppDef { key: "freecad", name: "FreeCAD", tagline: "Parametric 3D CAD modeler", code: "Fc", fg: "#E8736A", bg: "#2C1311", icon: "https://raw.githubusercontent.com/FreeCAD/FreeCAD/main/src/Gui/Icons/freecad.svg", side_by_side: true },
    AppDef { key: "godot", name: "Godot", tagline: "Game engine and editor", code: "Gd", fg: "#6FB1E3", bg: "#0F2131", icon: "https://godotengine.org/assets/press/icon_color.png", side_by_side: true },
    AppDef { key: "unity", name: "Unity Editor", tagline: "Game engine and real-time 3D", code: "Un", fg: "#F0F0F2", bg: "#2A2F36", icon: "https://unity.com/icon-dark.svg", side_by_side: true },
    // Unity editors only start with a license, which Unity Hub signs in for and keeps.
    AppDef { key: "unityhub", name: "Unity Hub", tagline: "Signs in and licenses Unity editors", code: "Uh", fg: "#F0F0F2", bg: "#2A2F36", icon: "https://unity.com/icon-dark.svg", side_by_side: false },
    // Epic's new launcher is a beta. The old one (MSI, version 1.x) is a different app in Other apps.
    AppDef { key: "epic", name: "Epic Games Launcher (Preview)", tagline: "Epic's new games launcher, in preview", code: "Ep", fg: "#FFFFFF", bg: "#121214", icon: "https://launcher.epicgames.com/apple-touch-icon.png", side_by_side: false },
];

pub fn def(key: &str) -> Option<&'static AppDef> {
    APPS.iter().find(|a| a.key == key)
}

// ---------------------------------------------------------------- releases

/// One installable release of an app.
#[derive(Clone, Debug, Default)]
pub struct Rel {
    /// What people call it: "32.2.2", "26.3rc2", "4.8-beta1", "6000.3.27f1"
    pub version: String,
    pub date: Option<String>,
    pub prerelease: bool,
    pub notes: Vec<String>,
    pub notes_url: Option<String>,
    /// Address of release notes that are read only when asked for (Unity's markdown)
    pub notes_src: Option<String>,
    pub size: Option<u64>,
    pub url: String,
    pub file: String,
    pub sha256: Option<String>,
    /// Checksum file to read at install time when GitHub has no digest (FreeCAD)
    pub sums_url: Option<String>,
    pub md5: Option<String>,
    /// Lowercase hex SHA-512 (Unity Hub)
    pub sha512: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    /// app key -> releases, newest first (pre-releases flagged)
    pub apps: HashMap<String, Vec<Rel>>,
    /// Unity release series ("6000.3", "2022.3") of installed editors -> releases, newest first
    pub unity_series: HashMap<String, Vec<Rel>>,
}

impl Feed {
    pub fn list(&self, key: &str) -> &[Rel] {
        self.apps.get(key).map(|v| v.as_slice()).unwrap_or(&[])
    }
}

// ---------------------------------------------------------------- version order

static STAGE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)(dev|alpha|beta|rc)[.\-]?(\d*)").unwrap());
static UNITY_PRE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d+\.\d+\.\d+[ab]\d+$").unwrap());
static UNITY_VER: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d{4}\.\d+\.\d+[abfp]\d+$").unwrap());

/// Pre-release stage: a final release is later than its release candidates, which are later
/// than its betas, and so on.
fn stage(v: &str) -> (u8, u64) {
    match STAGE.captures(v) {
        Some(c) => {
            let rank = match c[1].to_ascii_lowercase().as_str() {
                "dev" => 1,
                "alpha" => 2,
                "beta" => 3,
                _ => 4,
            };
            (rank, c[2].parse().unwrap_or(0))
        }
        None => (9, 0),
    }
}

/// Order of two versions of any of these apps ("26.3rc2" < "26.3", "4.8-beta1" < "4.8-rc1").
pub fn cmp_ver(a: &str, b: &str) -> Ordering {
    version::cmp(a, b).then_with(|| stage(a).cmp(&stage(b)))
}

pub fn is_pre(v: &str) -> bool {
    stage(v).0 != 9 || UNITY_PRE.is_match(v)
}

/// Pre-releases of versions that are already out are noise in a version list: only the ones
/// newer than the newest stable release stay.
fn trim_pre(list: &mut Vec<Rel>) {
    let Some(stable) = list.iter().find(|r| !r.prerelease).map(|r| r.version.clone()) else { return };
    list.retain(|r| !r.prerelease || cmp_ver(&r.version, &stable) == Ordering::Greater);
}

fn sort_newest_first(list: &mut [Rel]) {
    list.sort_by(|a, b| cmp_ver(&b.version, &a.version));
}

// ---------------------------------------------------------------- notes

/// Short plain bullets from release notes in markdown: handles and links removed, no long dashes.
fn bullets(md: &str) -> Vec<String> {
    static HANDLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*\[[\w ,.\-]{2,60}\]$").unwrap());
    super::craft::note_lines(md)
        .into_iter()
        .map(|l| HANDLE.replace(&l, "").trim().to_string())
        .filter(|l| !l.is_empty() && !l.contains("http://") && !l.contains("https://") && !l.eq_ignore_ascii_case("full changelog"))
        .collect()
}

/// A release text that is a short announcement and links, not a list: its first sentence is
/// the note and the "Release notes" link is the full notes.
fn intro_notes(body: &str) -> (Vec<String>, Option<String>) {
    let link = Regex::new(r"\[Release notes\]\((https?://[^)]+)\)").unwrap();
    let url = link.captures(body).map(|c| c[1].to_string());
    let md_link = Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap();
    let para = body
        .replace("\r\n", "\n")
        .split("\n\n")
        .map(|p| p.trim().to_string())
        .find(|p| !p.is_empty() && !p.starts_with('-') && !p.starts_with('>') && !p.starts_with("http"))
        .unwrap_or_default();
    let mut t = md_link.replace_all(&para, "$1").replace("**", "").replace('\n', " ");
    t = t.replace(" \u{2014} ", ", ").replace('\u{2014}', ", ").replace('\u{2013}', "-");
    let sentence = match t.find(". ") {
        Some(i) => t[..i].to_string(),
        None => t.trim_end_matches('.').to_string(),
    };
    let sentence = sentence.trim().to_string();
    (if sentence.is_empty() { vec![] } else { vec![sentence] }, url)
}

/// The bullet lines of one Unity release notes markdown file, starting at its release notes
/// section (the known issues that come before it are skipped).
pub fn unity_bullets(md: &str) -> Vec<String> {
    let link = Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap();
    let md = md.replace("\r\n", "\n");
    let start = md.lines().position(|l| l.starts_with('#') && l.contains("Release Notes")).unwrap_or(0);
    let mut out = Vec::new();
    // The "6000.3.27f1 Release Notes" heading itself is the title, not a section.
    for (i, l) in md.lines().skip(start).enumerate() {
        if i > 0 {
            if let Some(h) = super::craft::markdown_heading(l.trim()) {
                out.push(h);
                continue;
            }
        }
        let Some(item) = l.strip_prefix("- ") else { continue };
        let t = link.replace_all(item, "$1").replace(" \u{2014} ", ", ").replace('\u{2014}', ", ").replace('\u{2013}', "-");
        let t = t.trim().trim_end_matches('.').to_string();
        if !t.is_empty() {
            out.push(t);
        }
    }
    notes::tidy_headings(out)
}

// ---------------------------------------------------------------- GitHub

struct Asset {
    name: String,
    url: String,
    size: Option<u64>,
    sha256: Option<String>,
}

fn assets(r: &Value) -> Vec<Asset> {
    r["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| Asset {
                    name: x["name"].as_str().unwrap_or("").to_string(),
                    url: x["browser_download_url"].as_str().unwrap_or("").to_string(),
                    size: x["size"].as_u64(),
                    sha256: x["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|s| s.to_ascii_lowercase()),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn gh_rel(r: &Value, version: String, a: &Asset, prerelease: bool, notes: Vec<String>) -> Rel {
    Rel {
        version,
        date: r["published_at"].as_str().map(String::from),
        prerelease,
        notes,
        notes_url: r["html_url"].as_str().map(String::from),
        size: a.size,
        url: a.url.clone(),
        file: a.name.clone(),
        sha256: a.sha256.clone(),
        ..Default::default()
    }
}

fn releases_of(v: &Value) -> impl Iterator<Item = &Value> {
    v.as_array().into_iter().flatten().filter(|r| r["draft"].as_bool() != Some(true))
}

/// OBS Studio releases that have a Windows x64 installer, release candidates and betas flagged.
pub fn parse_obs(v: &Value) -> Vec<Rel> {
    let re = Regex::new(r"^OBS-Studio-(.+)-Windows-x64-Installer\.exe$").unwrap();
    let mut out = Vec::new();
    for r in releases_of(v) {
        let list = assets(r);
        let Some((a, ver)) = list.iter().find_map(|a| re.captures(&a.name).map(|c| (a, c[1].to_string()))) else { continue };
        let pre = r["prerelease"].as_bool().unwrap_or(false);
        out.push(gh_rel(r, ver, a, pre, bullets(r["body"].as_str().unwrap_or(""))));
    }
    sort_newest_first(&mut out);
    out
}

/// FreeCAD releases with a Windows x64 installer. The weekly development builds are left out;
/// release candidates are flagged.
pub fn parse_freecad(v: &Value) -> Vec<Rel> {
    let re = Regex::new(r"^FreeCAD_(.+?)-Windows-x86_64(?:-py\d+)?-installer\.exe$").unwrap();
    let mut out = Vec::new();
    for r in releases_of(v) {
        if r["tag_name"].as_str().unwrap_or("").starts_with("weekly") {
            continue;
        }
        let list = assets(r);
        let Some((a, ver)) = list.iter().find_map(|a| re.captures(&a.name).map(|c| (a, c[1].to_string()))) else { continue };
        if ver.starts_with("weekly") {
            continue;
        }
        let pre = r["prerelease"].as_bool().unwrap_or(false) || is_pre(&ver);
        let sums = list.iter().find(|s| s.name == format!("{}-SHA256.txt", a.name)).map(|s| s.url.clone());
        // A release candidate's text is a call for testing, not a change list.
        let body = r["body"].as_str().unwrap_or("");
        let mut notes = if pre { vec![] } else { bullets(body) };
        if notes.is_empty() {
            notes = intro_notes(body).0;
        }
        let mut rel = gh_rel(r, ver, a, pre, notes);
        rel.sums_url = sums;
        out.push(rel);
    }
    sort_newest_first(&mut out);
    out
}

/// Godot releases. The main repository has the stable releases; `builds` is the godot-builds
/// repository, from which only the betas and release candidates are taken.
pub fn parse_godot(v: &Value, builds: bool) -> Vec<Rel> {
    parse_godot_for(v, builds, winutil::native_arch())
}

/// `parse_godot` for a PC with the `native` processor: the ARM64 build when there is one on
/// ARM64, else the x64 one (it runs emulated).
fn parse_godot_for(v: &Value, builds: bool, native: &str) -> Vec<Rel> {
    let mut out = Vec::new();
    for r in releases_of(v) {
        let tag = r["tag_name"].as_str().unwrap_or("");
        let (ver, pre) = match tag.strip_suffix("-stable") {
            Some(s) if !builds => (s.to_string(), false),
            Some(_) => continue,
            None if builds && (tag.contains("-beta") || tag.contains("-rc")) => (tag.to_string(), true),
            None => continue,
        };
        let mut files = vec![format!("Godot_v{tag}_win64.exe.zip")];
        if native == "arm64" {
            files.insert(0, format!("Godot_v{tag}_windows_arm64.exe.zip"));
        }
        let list = assets(r);
        let Some(a) = files.iter().find_map(|f| list.iter().find(|a| &a.name == f)) else { continue };
        let (notes, link) = intro_notes(r["body"].as_str().unwrap_or(""));
        let mut rel = gh_rel(r, ver, a, pre, notes);
        if link.is_some() {
            rel.notes_url = link;
        }
        // Older releases have no GitHub digest, but a SHA512-SUMS.txt.
        if rel.sha256.is_none() {
            rel.sums_url = list.iter().find(|s| s.name == "SHA512-SUMS.txt").map(|s| s.url.clone());
        }
        out.push(rel);
    }
    sort_newest_first(&mut out);
    out
}

/// Release lists parsed before, by address and the parse used: an unchanged answer is not parsed again.
static PARSED: http::Parsed<Vec<Rel>> = http::Parsed::new();

/// A GitHub release list, read with `parse` (named by `how`).
async fn github(repo: &str, per_page: u32, how: &str, parse: impl FnOnce(&Value) -> Vec<Rel>) -> Result<Vec<Rel>> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page={per_page}");
    let f = http::get_cached(&url, &[("Accept", "application/vnd.github+json")], Duration::from_secs(10 * 60)).await?;
    PARSED.get_or(&format!("{url}#{how}"), &f.body, |b| {
        let v: Value = serde_json::from_slice(b).map_err(|e| anyhow!("GitHub answer: {e}"))?;
        if !v.is_array() {
            bail!("GitHub: {}", v["message"].as_str().unwrap_or("unexpected answer"));
        }
        Ok(parse(&v))
    })
}

// ---------------------------------------------------------------- Krita

/// Rows of an Apache style directory index: (name, date "2026-10-01", size such as "167M").
fn index_rows(html: &str) -> Vec<(String, Option<String>, Option<String>)> {
    let re = Regex::new(r#"<a href="([^"]+)">[^<]+</a>\s*</td><td align="right">(\d{4}-\d{2}-\d{2})[^<]*</td><td align="right">\s*([^<]*?)\s*</td>"#).unwrap();
    re.captures_iter(html)
        .map(|c| {
            let size = c[3].trim().to_string();
            (c[1].to_string(), Some(c[2].to_string()), Some(size).filter(|s| s != "-" && !s.is_empty()))
        })
        .collect()
}

/// "167M" -> bytes, roughly (index sizes are rounded).
fn index_size(s: &str) -> Option<u64> {
    let (num, mul) = match s.chars().last()? {
        'K' => (&s[..s.len() - 1], 1024f64),
        'M' => (&s[..s.len() - 1], 1024f64 * 1024.0),
        'G' => (&s[..s.len() - 1], 1024f64 * 1024.0 * 1024.0),
        _ => (s, 1.0),
    };
    num.parse::<f64>().ok().map(|n| (n * mul) as u64)
}

/// Release folders of the Krita index ("6.0.4.1/"), pre-releases and Android builds left out.
pub fn krita_versions(html: &str) -> Vec<String> {
    let re = Regex::new(r"^(\d+(?:\.\d+){1,3})/$").unwrap();
    let mut v: Vec<String> = index_rows(html).into_iter().filter_map(|(n, _, _)| re.captures(&n).map(|c| c[1].to_string())).collect();
    v.sort_by(|a, b| version::cmp(b, a));
    v.dedup();
    v
}

/// The Windows installer of one Krita release folder: file name, date and rounded size.
pub fn krita_installer(html: &str, ver: &str) -> Option<(String, Option<String>, Option<u64>)> {
    let name = format!("krita-x64-{ver}-setup.exe");
    index_rows(html).into_iter().find(|(n, _, _)| *n == name).map(|(n, d, s)| (n, d.map(|d| format!("{d}T00:00:00Z")), s.and_then(|s| index_size(&s))))
}

async fn krita_folder(ver: &str, base: &str) -> Option<Rel> {
    let dir = format!("{base}{ver}/");
    let f = http::get_cached(&dir, &[], Duration::from_secs(30 * 60)).await.ok()?;
    let (file, date, size) = krita_installer(&String::from_utf8_lossy(&f.body), ver)?;
    Some(Rel { version: ver.to_string(), date, size, url: format!("{dir}{file}"), file, notes_url: Some(dir), ..Default::default() })
}

/// Krita releases with a Windows installer, newest first. The old ones in the Attic are only
/// read when `attic` is set (version lists); the update check needs just the newest.
pub async fn krita_releases(attic: bool, want: usize) -> Result<Vec<Rel>> {
    let mut cands: Vec<(String, &'static str)> = Vec::new();
    let stable = http::get_cached(KRITA_STABLE, &[], Duration::from_secs(30 * 60)).await?;
    cands.extend(krita_versions(&String::from_utf8_lossy(&stable.body)).into_iter().map(|v| (v, KRITA_STABLE)));
    if attic {
        if let Ok(a) = http::get_cached(KRITA_ATTIC, &[], Duration::from_secs(6 * 3600)).await {
            cands.extend(krita_versions(&String::from_utf8_lossy(&a.body)).into_iter().filter(|v| !version::newer("4.0.0", v)).map(|v| (v, KRITA_ATTIC)));
        }
    }
    cands.sort_by(|a, b| version::cmp(&b.0, &a.0));
    cands.dedup_by(|a, b| a.0 == b.0);
    let mut out: Vec<Rel> = Vec::new();
    for chunk in cands.chunks(want + 3) {
        let found = futures_util::future::join_all(chunk.iter().map(|(v, base)| krita_folder(v, base))).await;
        out.extend(found.into_iter().flatten());
        if out.len() >= want {
            break;
        }
    }
    out.truncate(want);
    if out.is_empty() {
        bail!("No Krita release found on download.kde.org");
    }
    Ok(out)
}

// ---------------------------------------------------------------- Unity

/// Releases from Unity's release API, in the order the API gives them. Only the BETA and ALPHA
/// streams (and "a"/"b" builds) are pre-releases; LTS, SUPPORTED and TECH builds are final.
pub fn parse_unity(v: &Value) -> Vec<Rel> {
    let mut out = Vec::new();
    for r in v["results"].as_array().into_iter().flatten() {
        let ver = r["version"].as_str().unwrap_or("");
        let Some(d) = r["downloads"].as_array().and_then(|a| a.iter().find(|d| d["url"].as_str().is_some_and(|u| u.ends_with(".exe")))) else { continue };
        let url = d["url"].as_str().unwrap_or("").to_string();
        if ver.is_empty() || url.is_empty() {
            continue;
        }
        let stream = r["stream"].as_str().unwrap_or("");
        out.push(Rel {
            version: ver.to_string(),
            date: r["releaseDate"].as_str().map(String::from),
            prerelease: stream.eq_ignore_ascii_case("BETA") || stream.eq_ignore_ascii_case("ALPHA") || UNITY_PRE.is_match(ver),
            notes: vec![],
            notes_url: Some(format!("https://unity.com/releases/editor/whats-new/{ver}")),
            notes_src: r["releaseNotes"]["url"].as_str().map(String::from),
            size: d["downloadSize"]["value"].as_u64(),
            file: url.rsplit('/').next().unwrap_or("UnitySetup64.exe").to_string(),
            url,
            sha256: None,
            sums_url: None,
            md5: d["integrity"].as_str().and_then(download::md5_hex_from_integrity),
            sha512: None,
        });
    }
    out
}

/// Unity editor builds to ask for, best first: native ARM64 on ARM64, else x64.
fn unity_archs(native: &str) -> &'static [&'static str] {
    if native == "arm64" {
        &["ARM64", "X86_64"]
    } else {
        &["X86_64"]
    }
}

async fn unity_page(query: &str) -> Result<Vec<Rel>> {
    let mut last = Ok(vec![]);
    for arch in unity_archs(winutil::native_arch()) {
        let url = format!("{UNITY_API}?limit=25&offset=0&platform=WINDOWS&architecture={arch}&{query}");
        last = async {
            let f = http::get_cached(&url, &[], Duration::from_secs(30 * 60)).await?;
            PARSED.get_or(&url, &f.body, |b| {
                let v: Value = serde_json::from_slice(b).map_err(|e| anyhow!("Unity release list: {e}"))?;
                Ok(parse_unity(&v))
            })
        }
        .await;
        if last.as_ref().is_ok_and(|l: &Vec<Rel>| !l.is_empty()) {
            break;
        }
    }
    last
}

async fn unity_notes(src: &str) -> Vec<String> {
    match tokio::time::timeout(Duration::from_secs(12), http::get_cached(src, &[], Duration::from_secs(24 * 3600))).await {
        Ok(Ok(f)) => unity_bullets(&String::from_utf8_lossy(&f.body)),
        _ => vec![],
    }
}

// ---------------------------------------------------------------- feed

pub async fn fetch(installed: &[Installed]) -> Result<Feed> {
    let mut feed = Feed::default();
    let godot_pre = installed.iter().any(|i| matches!(&i.data, SourceData::Tools { key, .. } if key == "godot") && is_pre(&i.version));
    let unity_series: Vec<String> = {
        let mut s: Vec<String> = installed.iter().filter(|i| matches!(&i.data, SourceData::Tools { key, .. } if key == "unity")).map(|i| version::series(&i.version)).collect();
        s.sort();
        s.dedup();
        s
    };
    let (obs, fc, gd, gdb, kr, un, hub, epic) = tokio::join!(
        github("obsproject/obs-studio", 40, "obs", parse_obs),
        github("FreeCAD/FreeCAD", 100, "freecad", parse_freecad),
        github("godotengine/godot", 60, "godot", |v| parse_godot(v, false)),
        async { if godot_pre { github("godotengine/godot-builds", 60, "godot-builds", |v| parse_godot(v, true)).await.ok() } else { None } },
        krita_releases(false, 1),
        unity_page("stream=LTS"),
        unity_hub_releases(),
        epic_release(),
    );
    let mut put = |key: &str, r: Result<Vec<Rel>>| match r {
        Ok(l) if !l.is_empty() => {
            feed.apps.insert(key.to_string(), l);
        }
        Ok(_) => log::warn!("{key}: no releases"),
        Err(e) => log::warn!("{key}: {e:#}"),
    };
    put("obs", obs);
    put("freecad", fc);
    put("godot", gd.map(|mut l| {
        if let Some(b) = gdb {
            l.extend(b);
            sort_newest_first(&mut l);
        }
        l
    }));
    put("krita", kr);
    put("unityhub", hub);
    put("epic", epic.map(|r| vec![r]));
    // Newest version first, so the first stable one is what is offered when no version is picked.
    put("unity", un.map(|mut l| {
        sort_newest_first(&mut l);
        l
    }));
    let per_series = futures_util::future::join_all(unity_series.iter().map(|s| async move { (s.clone(), unity_page(&format!("version={s}")).await) })).await;
    for (s, r) in per_series {
        match r {
            Ok(mut l) if !l.is_empty() => {
                sort_newest_first(&mut l);
                feed.unity_series.insert(s, l);
            }
            Ok(_) => {}
            Err(e) => log::warn!("unity {s}: {e:#}"),
        }
    }
    if feed.apps.is_empty() {
        bail!("Could not reach GitHub, download.kde.org or Unity");
    }
    Ok(feed)
}

/// The version in the address Epic's download link leads to:
/// `.../Builds/EGLv2/0.100.0%2Bg9dfc3c3/Windows/EpicGamesLauncherSetup.exe?...` gives "0.100.0".
pub fn epic_version(url: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"/EGLv2/(\d+(?:\.\d+){1,3})(?:%2[Bb]|\+|/)").unwrap());
    RE.captures(url).map(|c| c[1].to_string())
}

/// The newest Epic Games Launcher (Preview): a HEAD request follows the redirect, so the file is
/// not downloaded; the final address names the version and the answer gives the exact size.
async fn epic_release() -> Result<Rel> {
    let r = http::CLIENT.head(EPIC_DOWNLOAD).timeout(Duration::from_secs(20)).send().await?;
    if !r.status().is_success() {
        bail!("Epic's download link answered {}", r.status());
    }
    let url = r.url().to_string();
    let version = epic_version(&url).ok_or_else(|| anyhow!("no version in Epic's download address"))?;
    let size = r.headers().get(reqwest::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0);
    Ok(Rel { version, size, url, file: "EpicGamesLauncherSetup.exe".into(), notes_url: Some(EPIC_PAGE.into()), ..Default::default() })
}

/// The newest Unity Hub from its update feed (`version`, x64 `url`, `sha512` in base64, `size`).
pub fn parse_hub_feed(yml: &str) -> Option<Rel> {
    parse_hub_feed_for(yml, winutil::native_arch())
}

/// `parse_hub_feed` for a PC with the `native` processor (the arm64 installer on ARM64).
fn parse_hub_feed_for(yml: &str, native: &str) -> Option<Rel> {
    let field = |line: &str, k: &str| line.trim().trim_start_matches("- ").strip_prefix(k).map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string());
    let version = yml.lines().find_map(|l| if l.starts_with("version:") { field(l, "version:") } else { None })?;
    let date = yml.lines().find_map(|l| if l.starts_with("releaseDate:") { field(l, "releaseDate:") } else { None });
    // Each file is "- url:" followed by its sha512 and size; take the x64 one.
    let lines: Vec<&str> = yml.lines().collect();
    let x64 = |u: &str| u.contains("-x64") || !u.contains("arm64");
    let arm = lines.iter().position(|l| field(l, "url:").is_some_and(|u| u.contains("-arm64"))).filter(|_| native == "arm64");
    let i = arm.or_else(|| lines.iter().position(|l| field(l, "url:").is_some_and(|u| x64(&u))))?;
    let url = field(lines[i], "url:")?;
    let mut sha512 = None;
    let mut size = None;
    for l in lines.iter().skip(i + 1).take(4) {
        if l.trim_start().starts_with("- ") {
            break;
        }
        if let Some(v) = field(l, "sha512:") {
            use base64::Engine;
            sha512 = base64::engine::general_purpose::STANDARD.decode(v).ok().map(|b| b.iter().map(|x| format!("{x:02x}")).collect::<String>());
        }
        if let Some(v) = field(l, "size:") {
            size = v.parse().ok();
        }
    }
    let file = url.rsplit('/').next().unwrap_or(&url).to_string();
    Some(Rel { version, date, url: format!("{HUB_FILES}{url}"), file, sha512: Some(sha512?), size, notes_url: Some(HUB_NOTES.into()), ..Default::default() })
}

/// Unity Hub releases: the newest one installable (it is the only one with a published checksum),
/// every version with its change list from the release notes page.
async fn unity_hub_releases() -> Result<Vec<Rel>> {
    let (feed, page) = tokio::join!(text_of_fresh(HUB_FEED, 30 * 60), text_of_fresh(HUB_NOTES, 6 * 3600));
    let mut newest = parse_hub_feed(&feed.ok_or_else(|| anyhow!("Unity Hub's update feed did not answer"))?).ok_or_else(|| anyhow!("Unity Hub's update feed changed its format"))?;
    let mut out: Vec<Rel> = Vec::new();
    for (ver, date, lines) in page.as_deref().map(notes::unity_hub).unwrap_or_default() {
        if ver == newest.version {
            newest.notes = lines;
            if newest.date.is_none() {
                newest.date = date;
            }
            continue;
        }
        out.push(Rel { version: ver, date, notes: lines, notes_url: Some(HUB_NOTES.into()), ..Default::default() });
    }
    out.insert(0, newest);
    sort_newest_first(&mut out);
    Ok(out)
}

async fn text_of_fresh(url: &str, secs: u64) -> Option<String> {
    let f = http::get_cached(url, &[], Duration::from_secs(secs)).await.ok()?;
    Some(String::from_utf8_lossy(&f.body).into_owned())
}

// ---------------------------------------------------------------- installed apps

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Marker {
    pub version: String,
    /// Program file inside the folder
    pub exe: String,
}

fn tools_key(i: &Installed) -> &str {
    match &i.data {
        SourceData::Tools { key, .. } => key,
        _ => "",
    }
}

fn series_of(i: &Installed) -> &str {
    match &i.data {
        SourceData::Tools { series, .. } => series,
        _ => "",
    }
}

fn mtime_iso(p: &Path) -> Option<String> {
    p.metadata().ok()?.modified().ok().map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
}

fn needs_admin(p: &Path) -> bool {
    let s = p.to_string_lossy().to_ascii_lowercase();
    s.starts_with(&paths::program_files().to_string_lossy().to_ascii_lowercase()) || s.starts_with(&paths::program_files_x86().to_string_lossy().to_ascii_lowercase())
}

/// The folder of the program named first in an uninstall command or icon path.
fn exe_dir_of(cmd: &str) -> Option<PathBuf> {
    let first = winutil::split_program_line(cmd).into_iter().next()?;
    let first = first.split(',').next().unwrap_or("").trim_matches('"').to_string();
    if !first.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    PathBuf::from(first).parent().map(|p| p.to_path_buf())
}

/// The quiet uninstall command of an uninstall list entry (NSIS style: `/S` is silent).
fn uninstall_cmd(e: &winutil::UninstallEntry) -> Option<Vec<String>> {
    let v = if !e.quiet_uninstall_string.is_empty() {
        winutil::split_program_line(&e.quiet_uninstall_string)
    } else if !e.uninstall_string.is_empty() {
        let mut v = winutil::split_program_line(&e.uninstall_string);
        if !v.iter().any(|a| a.eq_ignore_ascii_case("/S")) {
            v.push("/S".into());
        }
        v
    } else {
        vec![]
    };
    (!v.is_empty()).then_some(v)
}

fn make(d: &AppDef, id: String, name: String, ver: String, series: String, dir: Option<PathBuf>, exe: Option<PathBuf>) -> Installed {
    let missing = exe.as_ref().is_none_or(|e| !e.exists());
    Installed {
        id,
        source: Source::Tools,
        name,
        code: d.code.into(),
        fg: d.fg.into(),
        bg: d.bg.into(),
        channel: is_pre(&ver).then(|| "beta".to_string()),
        version: ver,
        install_dir: dir,
        updated_at: exe.as_deref().and_then(mtime_iso),
        exe,
        uninstall: None,
        size_hint: None,
        tagline: Some(d.tagline.into()),
        missing,
        read_only: false,
        store: None,
        icon_remote: None,
        data: SourceData::Tools { key: d.key.into(), series },
    }
}

fn from_entry(d: &AppDef, id: String, name: String, e: &winutil::UninstallEntry, series: String, dir: PathBuf, exe: PathBuf) -> Installed {
    let mut i = make(d, id, name, e.display_version.clone(), series, Some(dir), Some(exe));
    i.uninstall = uninstall_cmd(e);
    i.size_hint = Some(e.estimated_size_kb * 1024).filter(|s| *s > 0);
    i.updated_at = winutil::install_date_iso(&e.install_date).or(i.updated_at);
    i
}

fn obs_dir(e: &winutil::UninstallEntry) -> PathBuf {
    if !e.install_location.is_empty() {
        return winutil::pathbuf(&e.install_location);
    }
    if let Some(d) = exe_dir_of(&e.uninstall_string).or_else(|| exe_dir_of(&e.quiet_uninstall_string)) {
        return d;
    }
    use winreg::enums::*;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("SOFTWARE\\OBS Studio", KEY_READ | KEY_WOW64_64KEY)
        .and_then(|k| k.get_value::<String, _>(""))
        .map(|s| winutil::pathbuf(&s))
        .unwrap_or_else(|_| paths::program_files().join("obs-studio"))
}

fn scan_obs(un: &[winutil::UninstallEntry], out: &mut Vec<Installed>) {
    let d = def("obs").unwrap();
    if let Some(e) = un.iter().find(|e| e.display_name.eq_ignore_ascii_case("OBS Studio")) {
        let dir = obs_dir(e);
        let exe = dir.join("bin").join("64bit").join("obs64.exe");
        out.push(from_entry(d, "tools:obs".into(), d.name.into(), e, String::new(), dir, exe));
    }
}

fn scan_krita(un: &[winutil::UninstallEntry], out: &mut Vec<Installed>) {
    let d = def("krita").unwrap();
    let hit = un.iter().find(|e| e.key.eq_ignore_ascii_case("Krita_x64") || e.display_name.starts_with("Krita (x64)"));
    if let Some(e) = hit {
        let dir = if !e.install_location.is_empty() {
            winutil::pathbuf(&e.install_location)
        } else {
            exe_dir_of(&e.uninstall_string).or_else(|| exe_dir_of(&e.quiet_uninstall_string)).unwrap_or_else(|| paths::program_files().join("Krita (x64)"))
        };
        let exe = dir.join("bin").join("krita.exe");
        let mut e2 = e.clone();
        if e2.display_version.is_empty() {
            e2.display_version = e.display_name.rsplit(' ').next().unwrap_or("").to_string();
        }
        out.push(from_entry(d, "tools:krita".into(), d.name.into(), &e2, String::new(), dir, exe));
    }
}

fn scan_freecad(un: &[winutil::UninstallEntry], out: &mut Vec<Installed>) {
    let d = def("freecad").unwrap();
    let mut found: Vec<Installed> = Vec::new();
    for e in un.iter().filter(|e| e.display_name.to_ascii_lowercase().starts_with("freecad") && !e.display_version.is_empty() && !e.display_name.to_ascii_lowercase().contains("addon")) {
        let mut cands: Vec<PathBuf> = Vec::new();
        if !e.install_location.is_empty() {
            cands.push(winutil::pathbuf(&e.install_location));
        }
        for c in [&e.display_icon, &e.uninstall_string, &e.quiet_uninstall_string] {
            if let Some(p) = exe_dir_of(c) {
                // The icon sits in <folder>\bin, the uninstaller in <folder>.
                cands.push(if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("bin")) { p.parent().map(|x| x.to_path_buf()).unwrap_or(p) } else { p });
            }
        }
        let Some(dir) = cands.iter().find(|c| c.join("bin").join("FreeCAD.exe").exists()).or(cands.first()).cloned() else { continue };
        let series = version::series(&e.display_version);
        let exe = dir.join("bin").join("FreeCAD.exe");
        found.push(from_entry(d, format!("tools:freecad:{series}"), format!("FreeCAD {series}"), e, series, dir, exe));
    }
    // One entry per release series: the newest copy of each.
    found.sort_by(|a, b| cmp_ver(&b.version, &a.version));
    for f in found {
        if !out.iter().any(|o| o.id == f.id) {
            out.push(f);
        }
    }
}

fn godot_shortcut(name: &str) -> PathBuf {
    paths::app_data().join("Microsoft\\Windows\\Start Menu\\Programs\\Godot").join(format!("{name}.lnk"))
}

fn scan_godot(root: &Path, out: &mut Vec<Installed>) {
    let d = def("godot").unwrap();
    let Ok(rd) = std::fs::read_dir(root) else { return };
    let loose = Regex::new(r"^Godot_v(.+)_(?:win64|windows_arm64)\.exe$").unwrap();
    let mut found: Vec<Installed> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let fname = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if p.is_dir() {
            let Some(m) = std::fs::read(p.join(MARKER)).ok().and_then(|b| serde_json::from_slice::<Marker>(&b).ok()) else { continue };
            let exe = p.join(&m.exe);
            let mut i = make(d, format!("tools:godot:{}", m.version), format!("Godot {}", m.version), m.version.clone(), m.version.clone(), Some(p.clone()), Some(exe));
            i.updated_at = mtime_iso(&p.join(MARKER)).or(i.updated_at);
            found.push(i);
        } else if let Some(c) = loose.captures(&fname) {
            // A copy the user unpacked there: listed, never touched except by an explicit uninstall.
            let ver = c[1].trim_end_matches("-stable").to_string();
            let mut i = make(d, format!("tools:godot:{ver}"), format!("Godot {ver}"), ver.clone(), ver, None, Some(p.clone()));
            i.size_hint = p.metadata().ok().map(|m| m.len());
            i.tagline = Some("Copy found in the Godot folder".into());
            found.push(i);
        }
    }
    for f in found {
        if !out.iter().any(|o| o.id == f.id) {
            out.push(f);
        }
    }
}

fn unity_hub_editors() -> PathBuf {
    paths::program_files().join("Unity").join("Hub").join("Editor")
}

/// Editors the Unity Hub installed or knows of, as (version, folder with Editor\Unity.exe):
/// the default folder, the Hub's own install folder setting (secondaryInstallPath.json) and the
/// editors added by hand (editors.json, editors-v2.json), all in the UnityHub folder of `appdata`.
fn hub_editors(appdata: &Path) -> Vec<(String, PathBuf)> {
    let hub = appdata.join("UnityHub");
    let mut roots = vec![unity_hub_editors()];
    if let Some(p) = std::fs::read(hub.join("secondaryInstallPath.json")).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.as_str().map(String::from)) {
        if !p.trim().is_empty() {
            roots.push(winutil::pathbuf(&p));
        }
    }
    fn add(ver: String, dir: PathBuf, out: &mut Vec<(String, PathBuf)>) {
        if UNITY_VER.is_match(&ver) && dir.join("Editor").join("Unity.exe").is_file() && !out.iter().any(|(v, _)| *v == ver) {
            out.push((ver, dir));
        }
    }
    let mut out: Vec<(String, PathBuf)> = Vec::new();
    for root in roots {
        for e in std::fs::read_dir(&root).into_iter().flatten().flatten() {
            add(e.file_name().to_string_lossy().to_string(), e.path(), &mut out);
        }
    }
    // Each listed editor has a "location": the path of its Unity.exe (a string or a list).
    fn walk(v: &Value, found: &mut Vec<(Option<String>, String)>) {
        match v {
            Value::Object(m) => {
                let locs: Vec<String> = match m.get("location") {
                    Some(Value::String(s)) => vec![s.clone()],
                    Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(String::from)).collect(),
                    _ => vec![],
                };
                for l in locs {
                    found.push((m.get("version").and_then(|x| x.as_str()).map(String::from), l));
                }
                m.values().for_each(|x| walk(x, found));
            }
            Value::Array(a) => a.iter().for_each(|x| walk(x, found)),
            _ => {}
        }
    }
    for f in ["editors-v2.json", "editors.json"] {
        let Some(v) = std::fs::read(hub.join(f)).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()) else { continue };
        let mut found = Vec::new();
        walk(&v, &mut found);
        for (ver, loc) in found {
            let exe = winutil::pathbuf(&loc);
            let Some(dir) = exe.parent().filter(|e| e.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Editor"))).and_then(|e| e.parent()) else { continue };
            let ver = ver.unwrap_or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
            add(ver, dir.to_path_buf(), &mut out);
        }
    }
    out
}

fn scan_unity(un: &[winutil::UninstallEntry], out: &mut Vec<Installed>) {
    let d = def("unity").unwrap();
    let name_re = Regex::new(r"^Unity\s+(\d{4}\.\d+\.\d+[abfp]\d+)").unwrap();
    let mut found: Vec<Installed> = Vec::new();
    for e in un {
        let Some(c) = name_re.captures(&e.display_name) else { continue };
        let ver = c[1].to_string();
        // The uninstaller sits in <folder>\Editor, next to Unity.exe.
        let editor = exe_dir_of(&e.uninstall_string).or_else(|| exe_dir_of(&e.quiet_uninstall_string)).or_else(|| exe_dir_of(&e.display_icon));
        let dir = if !e.install_location.is_empty() {
            winutil::pathbuf(&e.install_location)
        } else {
            match editor {
                Some(p) if p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Editor")) => p.parent().map(|x| x.to_path_buf()).unwrap_or(p),
                Some(p) => p,
                None => unity_hub_editors().join(&ver),
            }
        };
        let exe = dir.join("Editor").join("Unity.exe");
        let mut e2 = e.clone();
        e2.display_version = ver.clone();
        found.push(from_entry(d, format!("tools:unity:{ver}"), format!("Unity {ver}"), &e2, ver.clone(), dir, exe));
    }
    // Editors installed by the Unity Hub have no entry in the uninstall list.
    for (ver, dir) in hub_editors(&paths::app_data()) {
        let exe = dir.join("Editor").join("Unity.exe");
        if found.iter().any(|f| f.version == ver) {
            continue;
        }
        let mut i = make(d, format!("tools:unity:{ver}"), format!("Unity {ver}"), ver.clone(), ver, Some(dir.clone()), Some(exe));
        let un_exe = dir.join("Editor").join("Uninstall.exe");
        if un_exe.exists() {
            i.uninstall = Some(vec![un_exe.to_string_lossy().to_string(), "/S".into()]);
        }
        found.push(i);
    }
    found.sort_by(|a, b| cmp_ver(&b.version, &a.version));
    for f in found {
        if !out.iter().any(|o| o.id == f.id) {
            out.push(f);
        }
    }
}

/// The new launcher's uninstall entry. The old launcher is an MSI package (and 1.x), this one
/// is installed by Epic's own setup program, which is also its uninstaller.
pub fn is_epic_preview(e: &winutil::UninstallEntry) -> bool {
    let setup = |s: &str| s.to_ascii_lowercase().contains("epicgameslaunchersetup.exe");
    !e.windows_installer && e.display_name.trim().eq_ignore_ascii_case("Epic Games Launcher") && (setup(&e.uninstall_string) || setup(&e.quiet_uninstall_string))
}

fn scan_epic(un: &[winutil::UninstallEntry], out: &mut Vec<Installed>) {
    let d = def("epic").unwrap();
    if let Some(e) = un.iter().find(|e| is_epic_preview(e) && !e.display_version.is_empty()) {
        let dir = if !e.install_location.is_empty() {
            winutil::pathbuf(&e.install_location)
        } else {
            exe_dir_of(&e.uninstall_string).or_else(|| exe_dir_of(&e.quiet_uninstall_string)).unwrap_or_else(|| paths::local_app_data().join("Epic Games").join("Launcher"))
        };
        let exe = dir.join("EpicGamesLauncher.exe");
        out.push(from_entry(d, "tools:epic".into(), d.name.into(), e, String::new(), dir, exe));
    }
}

fn scan_unity_hub(un: &[winutil::UninstallEntry], out: &mut Vec<Installed>) {
    let d = def("unityhub").unwrap();
    if let Some(e) = un.iter().find(|e| e.display_name.starts_with("Unity Hub") && !e.display_version.is_empty()) {
        let dir = if !e.install_location.is_empty() {
            winutil::pathbuf(&e.install_location)
        } else {
            exe_dir_of(&e.uninstall_string).unwrap_or_else(|| paths::program_files().join("Unity Hub"))
        };
        let exe = dir.join("Unity Hub.exe");
        out.push(from_entry(d, "tools:unityhub".into(), d.name.into(), e, String::new(), dir, exe));
    }
}

pub fn scan(uninstall: &[winutil::UninstallEntry], godot_root: &Path) -> Vec<Installed> {
    let mut out = Vec::new();
    scan_unity_hub(uninstall, &mut out);
    scan_epic(uninstall, &mut out);
    scan_obs(uninstall, &mut out);
    scan_krita(uninstall, &mut out);
    scan_freecad(uninstall, &mut out);
    scan_godot(godot_root, &mut out);
    scan_unity(uninstall, &mut out);
    out.extend(store_copies(&super::msstore::packages()));
    out
}

/// Apps of this source that the Microsoft Store has as MSIX packages, by publisher id and the
/// end of the package name. OBS Studio's Store listing runs its normal installer, so that copy is
/// found in the uninstall list like any other.
const STORE_APPS: &[(&str, &str, &str)] = &[("krita", "n3kgb906j1zjg", ".Krita")];

/// Copies from the Microsoft Store: listed only, the Store updates and removes them.
pub fn store_copies(pkgs: &[super::msstore::Package]) -> Vec<Installed> {
    let mut out: Vec<Installed> = Vec::new();
    for p in pkgs {
        let Some(&(key, _, _)) = STORE_APPS.iter().find(|(_, publisher, suffix)| p.publisher_id() == *publisher && p.name.ends_with(suffix)) else { continue };
        let Some(d) = def(key) else { continue };
        let id = format!("tools:{key}:store");
        if out.iter().any(|o| o.id == id) {
            continue;
        }
        let c = super::msstore::copy_of(p);
        let mut i = make(d, id, format!("{} (Microsoft Store)", d.name), c.version, "store".into(), p.install_dir.clone(), c.exe);
        // The Store keeps the program file out of reach of a plain check: the package is there.
        i.missing = false;
        i.updated_at = p.installed_at.clone().or(i.updated_at);
        i.tagline = Some("Updated by the Microsoft Store".into());
        i.read_only = true;
        i.store = Some(c.store);
        out.push(i);
    }
    out
}

// ---------------------------------------------------------------- latest versions

/// The release an installed app is offered. Apps that install one copy per series or version
/// (`inst` has siblings in `all`) are only offered newer builds of their own series, and nothing
/// when a sibling already has it; the newest installed series is offered the next series.
pub fn latest(inst: &Installed, all: &[Installed], feed: &Feed) -> Option<Latest> {
    // A Store copy is the Store's to update.
    if inst.read_only {
        return None;
    }
    let key = tools_key(inst);
    let d = def(key)?;
    let list: &[Rel] = if key == "unity" { feed.unity_series.get(&version::series(&inst.version)).map(|v| v.as_slice())? } else { feed.list(key) };
    let allow_pre = is_pre(&inst.version);
    // A release without a download address (a Unity Hub version the notes page lists before
    // the update feed has it) cannot be installed, so it is never offered.
    let usable = |r: &&Rel| !r.url.is_empty() && (allow_pre || !r.prerelease);
    let same = |r: &&Rel| !d.side_by_side || version::series(&r.version) == version::series(&inst.version);
    let up_to_date = || Some(Latest { version: inst.version.clone(), size: None, note: None });

    if let Some(n) = list.iter().filter(usable).find(same) {
        if cmp_ver(&n.version, &inst.version) == Ordering::Greater {
            if key == "godot" || key == "unity" {
                let siblings = all.iter().any(|o| o.id != inst.id && tools_key(o) == key && version::series(&o.version) == version::series(&inst.version) && cmp_ver(&o.version, &n.version) != Ordering::Less);
                if siblings {
                    return up_to_date();
                }
                return Some(Latest { version: n.version.clone(), size: n.size, note: Some(format!("{} installs next to {}, your {} stays", n.version, inst.version, inst.version)) });
            }
            return Some(Latest { version: n.version.clone(), size: n.size, note: None });
        }
    }
    // Up to date in its series: the newest installed series is offered the next one.
    if d.side_by_side && key != "unity" {
        let newest_installed = all.iter().filter(|o| tools_key(o) == key).map(|o| version::series(&o.version)).max_by(|a, b| version::cmp(a, b))?;
        if version::series(&inst.version) == newest_installed {
            if let Some(m) = list.iter().find(|r| !r.prerelease && !r.url.is_empty()) {
                if version::newer(&version::series(&m.version), &newest_installed) {
                    return Some(Latest { version: m.version.clone(), size: m.size, note: Some(format!("{} installs next to {}, your {} stays", m.version, inst.version, inst.version)) });
                }
            }
        }
    }
    up_to_date()
}

// ---------------------------------------------------------------- Get apps

pub fn catalog_entries(feed: &Feed, installed: &[Installed]) -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    for d in APPS {
        let list = feed.list(d.key);
        let Some(r) = list.iter().find(|r| !r.prerelease && !r.url.is_empty()).or(list.iter().find(|r| !r.url.is_empty())) else { continue };
        // A Store copy does not count: the app can still be installed from its own site.
        let have = installed.iter().filter(|i| tools_key(i) == d.key && !i.read_only);
        let is_installed = match d.key {
            "freecad" => have.into_iter().any(|i| series_of(i) == version::series(&r.version)),
            "godot" | "unity" => have.into_iter().any(|i| cmp_ver(&i.version, &r.version) == Ordering::Equal),
            _ => have.into_iter().next().is_some(),
        };
        out.push(CatalogEntry {
            id: format!("tools:{}", d.key),
            source: Source::Tools,
            name: d.name.into(),
            code: d.code.into(),
            fg: d.fg.into(),
            bg: d.bg.into(),
            icon_url: None,
            version: r.version.clone(),
            size: r.size,
            tagline: Some(d.tagline.into()),
            description: super::about::tools(d.key).map(String::from),
            blocked: None,
            group: Source::Tools.group().into(),
            prerelease: r.prerelease,
            installed: is_installed,
            pick_version: true,
        });
    }
    out
}

fn info(r: &Rel, installed: bool, latest: bool) -> VersionInfo {
    VersionInfo {
        version: r.version.clone(),
        date: r.date.clone(),
        notes: r.notes.clone(),
        notes_url: r.notes_url.clone(),
        size: r.size,
        installed,
        latest,
        prerelease: r.prerelease,
    }
}

/// Fill in the change lists that are read from separate pages, for the newest `n` versions.
async fn fill_notes(key: &str, list: &mut [Rel], n: usize) {
    let work = list.iter().take(n).map(|r| {
        let (ver, date, src) = (r.version.clone(), r.date.clone(), r.notes_src.clone());
        async move {
            match key {
                "unity" => (match src {
                    Some(s) => unity_notes(&s).await,
                    None => vec![],
                }, None),
                "krita" => {
                    let year = date.as_deref().and_then(|d| d.get(..4)).and_then(|y| y.parse().ok()).unwrap_or(2026);
                    match tokio::time::timeout(Duration::from_secs(15), notes::krita(&ver, year)).await {
                        Ok(Some((url, lines))) => (lines, Some(url)),
                        _ => (vec![], None),
                    }
                }
                _ => (vec![], None),
            }
        }
    });
    let got = futures_util::future::join_all(work).await;
    for (r, (lines, url)) in list.iter_mut().zip(got) {
        if !lines.is_empty() {
            r.notes = lines;
        }
        if url.is_some() {
            r.notes_url = url;
        }
    }
}

fn with_fallback_notes(key: &str, list: &mut [Rel]) {
    for r in list.iter_mut() {
        if r.notes.is_empty() {
            r.notes.push(match key {
                "krita" => "No change list is published for this release".to_string(),
                "epic" => "Epic does not publish a change list for the launcher preview".to_string(),
                _ => format!("{} release. See the full notes for the changes", if r.prerelease { "Pre-release" } else { "Maintenance" }),
            });
        }
    }
}

/// Versions an installed app can be set to (newest first, at most 25).
pub async fn versions(inst: &Installed, feed: &Feed) -> Result<Vec<VersionInfo>> {
    let key = tools_key(inst).to_string();
    let d = def(&key).ok_or_else(|| anyhow!("unknown app"))?;
    let mut list: Vec<Rel> = match key.as_str() {
        "krita" => krita_releases(true, 25).await?,
        "unity" => {
            let s = version::series(&inst.version);
            match feed.unity_series.get(&s) {
                Some(l) => l.clone(),
                None => {
                    let mut l = unity_page(&format!("version={s}")).await?;
                    sort_newest_first(&mut l);
                    l
                }
            }
        }
        _ => feed.list(&key).to_vec(),
    };
    if d.side_by_side && key != "unity" {
        list.retain(|r| version::series(&r.version) == version::series(&inst.version));
    }
    if list.is_empty() {
        bail!("{} has no other versions to choose from", d.name);
    }
    let allow_pre = is_pre(&inst.version);
    if !allow_pre {
        trim_pre(&mut list);
    }
    let newest = list.iter().find(|r| allow_pre || !r.prerelease).map(|r| r.version.clone());
    list.truncate(25);
    fill_notes(&key, &mut list, 25).await;
    with_fallback_notes(&key, &mut list);
    Ok(list.iter().map(|r| info(r, cmp_ver(&r.version, &inst.version).is_eq(), newest.as_deref() == Some(r.version.as_str()))).collect())
}

/// The newest `each` releases of every release series (newest series first), at most `max` in all.
fn per_series(list: &[Rel], each: usize, max: usize) -> Vec<Rel> {
    let mut out: Vec<Rel> = Vec::new();
    for r in list {
        if out.iter().filter(|o| version::series(&o.version) == version::series(&r.version)).count() < each {
            out.push(r.clone());
        }
    }
    out.truncate(max);
    out
}

/// Versions offered by the Get apps version picker.
pub async fn catalog_versions(key: &str, feed: &Feed, installed: &[Installed]) -> Result<Vec<VersionInfo>> {
    let d = def(key).ok_or_else(|| anyhow!("unknown app {key}"))?;
    let mut list: Vec<Rel> = match key {
        "krita" => krita_releases(true, 25).await?,
        "unity" => {
            // Long-term support first, then the newest beta and tech stream builds.
            let mut l: Vec<Rel> = per_series(feed.list("unity"), 5, 18);
            let (beta, tech) = tokio::join!(unity_page("stream=BETA"), unity_page("stream=TECH"));
            let mut beta = beta.unwrap_or_default();
            sort_newest_first(&mut beta);
            let mut tech = tech.unwrap_or_default();
            sort_newest_first(&mut tech);
            l.extend(beta.into_iter().take(4));
            l.extend(tech.into_iter().take(3));
            l
        }
        "godot" => {
            let mut l = feed.list("godot").to_vec();
            if let Ok(b) = github("godotengine/godot-builds", 60, "godot-builds", |v| parse_godot(v, true)).await {
                l.extend(b);
                sort_newest_first(&mut l);
            }
            l
        }
        _ => feed.list(key).to_vec(),
    };
    if list.is_empty() {
        bail!("{} releases are not loaded yet", d.name);
    }
    trim_pre(&mut list);
    list.truncate(25);
    let newest = list.iter().find(|r| !r.prerelease).map(|r| r.version.clone());
    fill_notes(key, &mut list, 25).await;
    with_fallback_notes(key, &mut list);
    let have: Vec<&Installed> = installed.iter().filter(|i| tools_key(i) == key).collect();
    Ok(list.iter().map(|r| info(r, have.iter().any(|i| cmp_ver(&i.version, &r.version).is_eq()), newest.as_deref() == Some(r.version.as_str()))).collect())
}

// ---------------------------------------------------------------- plans

pub struct Ctx<'a> {
    /// Folder that new Godot copies are unpacked into
    pub godot_root: &'a Path,
}

/// Size of a file at a redirecting address (Krita's installer comes from a mirror).
async fn remote_size(url: &str) -> Option<u64> {
    let r = http::CLIENT.head(url).timeout(Duration::from_secs(20)).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    // A HEAD answer has no body, so reqwest's own length is 0: read the header.
    r.headers().get(reqwest::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0)
}

/// First checksum in a `sha256sum` style file (optionally the line for `name`).
fn sum_from(text: &str, name: &str) -> Option<String> {
    let mut first = None;
    for l in text.lines() {
        let mut it = l.split_whitespace();
        let Some(h) = it.next() else { continue };
        if h.len() != 64 || !h.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        first.get_or_insert_with(|| h.to_ascii_lowercase());
        if it.next().is_some_and(|n| n.trim_start_matches('*') == name) {
            return Some(h.to_ascii_lowercase());
        }
    }
    first
}

/// The SHA-512 for `name` in a sums file ("hash  name" lines); only an exact name counts.
fn sha512_from(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let h = it.next()?;
        (h.len() == 128 && h.chars().all(|c| c.is_ascii_hexdigit()) && it.next()?.trim_start_matches('*') == name).then(|| h.to_ascii_lowercase())
    })
}

async fn text_of(url: &str) -> Option<String> {
    let f = http::get_cached(url, &[], Duration::from_secs(24 * 3600)).await.ok()?;
    Some(String::from_utf8_lossy(&f.body).into_owned())
}

/// The release for an install request: the exact version asked for, or the newest stable one.
async fn find_release(key: &str, ver: Option<&str>, series: Option<&str>, feed: &Feed) -> Result<Rel> {
    let d = def(key).ok_or_else(|| anyhow!("unknown app {key}"))?;
    let list: Vec<Rel> = if key == "unity" {
        match ver {
            Some(v) => {
                let mut l = feed.unity_series.get(&version::series(v)).cloned().unwrap_or_default();
                if !l.iter().any(|r| r.version == v) {
                    l = unity_page(&format!("version={v}")).await?;
                }
                l
            }
            None => feed.list("unity").to_vec(),
        }
    } else {
        feed.list(key).to_vec()
    };
    let mut list = list;
    if key == "godot" && ver.is_some_and(is_pre) && !list.iter().any(|r| Some(r.version.as_str()) == ver) {
        if let Ok(b) = github("godotengine/godot-builds", 60, "godot-builds", |v| parse_godot(v, true)).await {
            list.extend(b);
        }
    }
    if key == "krita" && ver.is_some_and(|v| !list.iter().any(|r| r.version == v)) {
        list = krita_releases(true, 25).await?;
    }
    match ver {
        Some(v) => list.into_iter().find(|r| r.version == v || cmp_ver(&r.version, v).is_eq()).ok_or_else(|| anyhow!("{} {} is not available from its download page", d.name, v)),
        None => list
            .into_iter()
            .find(|r| !r.prerelease && !r.url.is_empty() && series.is_none_or(|s| version::series(&r.version) == s))
            .ok_or_else(|| anyhow!("{} has no release for Windows right now", d.name)),
    }
}

fn run_installer(dest: &Path, args: Vec<String>, elevated: bool, label: String) -> Step {
    Step::Run { exe: dest.to_path_buf(), args, cwd: None, ok_codes: vec![0], elevated, label, log: None }
}

/// OBS Studio's files that other programs load: its game capture hook (in games and apps it
/// captured) and its virtual camera (in apps that list cameras, like Discord or a browser). OBS's
/// installer stops with exit code 6 while one is in use (the list its installer checks).
pub fn obs_in_use_files(install_dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = [
        "bin/64bit/avutil-57.dll",
        "bin/64bit/swscale-6.dll",
        "core/win-capture/data/graphics-hook32.dll",
        "core/win-capture/data/graphics-hook64.dll",
        "core/win-dshow/data/obs-virtualcam-module32.dll",
        "core/win-dshow/data/obs-virtualcam-module64.dll",
        "data/obs-plugins/win-capture/graphics-hook32.dll",
        "data/obs-plugins/win-capture/graphics-hook64.dll",
        "data/obs-plugins/win-dshow/obs-virtualcam-module32.dll",
        "data/obs-plugins/win-dshow/obs-virtualcam-module64.dll",
    ]
    .iter()
    .map(|f| install_dir.join(f.replace('/', "\\")))
    .collect();
    if let Ok(appdata) = std::env::var("APPDATA") {
        let hook = PathBuf::from(appdata).join("obs-studio-hook");
        v.push(hook.join("graphics-hook32.dll"));
        v.push(hook.join("graphics-hook64.dll"));
    }
    v
}

/// Everything an install needs once the release is known. No network.
pub fn build_plan(key: &str, rel: &Rel, kind: JobKind, current: Option<&Installed>, ctx: &Ctx<'_>) -> Result<Plan> {
    let d = def(key).ok_or_else(|| anyhow!("unknown app {key}"))?;
    let dest = paths::downloads().join("tools").join(&rel.file);
    let dl = Download { url: rel.url.clone(), dest: dest.clone(), size: rel.size, sha256: rel.sha256.clone(), md5: rel.md5.clone(), signer: (key == "epic").then(|| EPIC_SIGNER.to_string()), sha512: rel.sha512.clone(), headers: vec![] };
    super::require_checked(&dl, &format!("{} {}", d.name, rel.version))?;
    let label = format!("Installing {} {}", d.name, rel.version);
    let current_dir = current.and_then(|c| c.install_dir.clone());
    let repair = kind == JobKind::Repair;
    let (app_id, name, steps, block_dirs, block_names) = match key {
        "obs" => ("tools:obs".to_string(), d.name.to_string(), vec![run_installer(&dest, vec!["/S".into()], true, label)], current_dir.into_iter().collect::<Vec<_>>(), vec!["obs64.exe".to_string()]),
        "krita" => ("tools:krita".to_string(), d.name.to_string(), vec![run_installer(&dest, vec!["/S".into()], true, label)], current_dir.into_iter().collect(), vec!["krita.exe".to_string()]),
        "unityhub" => {
            if rel.url.is_empty() || rel.sha512.is_none() {
                bail!("Only the newest Unity Hub can be downloaded with a checksum, so other versions are not installed");
            }
            ("tools:unityhub".to_string(), d.name.to_string(), vec![run_installer(&dest, vec!["/S".into()], true, label)], current_dir.into_iter().collect(), vec!["Unity Hub.exe".to_string()])
        }
        // Epic's setup installs for the current user (no admin) and the launcher must be closed.
        "epic" => ("tools:epic".to_string(), d.name.to_string(), vec![run_installer(&dest, vec!["--silent".into()], false, label)], current_dir.into_iter().collect(), vec!["EpicGamesLauncher.exe".to_string(), "EpicGamesLauncherUpdate.exe".to_string()]),
        "freecad" => {
            let series = version::series(&rel.version);
            let same = current.is_some_and(|c| series_of(c) == series);
            // A series is updated where it is; a new series gets its own per-user folder.
            let dir = if same { current_dir.clone().unwrap_or_else(|| paths::local_app_data().join("Programs").join(format!("FreeCAD {series}"))) } else { paths::local_app_data().join("Programs").join(format!("FreeCAD {series}")) };
            let step = run_installer(&dest, vec!["/S".into(), format!("/D={}", dir.display())], needs_admin(&dir), label);
            (format!("tools:freecad:{series}"), format!("FreeCAD {series}"), vec![step], if same { vec![dir] } else { vec![] }, vec![])
        }
        "unity" => {
            let dir = unity_hub_editors().join(&rel.version);
            if dir.join("Editor").join("Unity.exe").exists() && !repair {
                bail!("Unity {} is already installed in {}", rel.version, dir.display());
            }
            let step = run_installer(&dest, vec!["/S".into(), format!("/D={}", dir.display())], true, label);
            (format!("tools:unity:{}", rel.version), format!("Unity {}", rel.version), vec![step], if repair { vec![dir] } else { vec![] }, vec![])
        }
        "godot" => {
            let name = format!("Godot {}", rel.version);
            let dir = ctx.godot_root.join(&name);
            if dir.exists() && !repair {
                bail!("{} is already installed in {}", name, dir.display());
            }
            let admin = needs_admin(&dir);
            let exe_name = rel.file.trim_end_matches(".zip").to_string();
            let marker = Marker { version: rel.version.clone(), exe: exe_name.clone() };
            let mut steps = Vec::new();
            if dir.exists() {
                steps.push(Step::RemoveDir { path: dir.clone(), elevated: admin, label: format!("Removing the old {name}") });
            }
            steps.push(Step::ExtractZip { zip: dest.clone(), dest: dir.clone(), strip_root: false, elevated: admin, label: format!("Unpacking {name}") });
            steps.push(Step::WriteFile { path: dir.join(MARKER), contents: serde_json::to_string(&marker).unwrap_or_default(), elevated: admin });
            steps.push(Step::Shortcut { lnk: godot_shortcut(&name), target: dir.join(&exe_name), label: "Adding it to the Start menu".into() });
            (format!("tools:godot:{}", rel.version), name, steps, if dir.exists() { vec![dir] } else { vec![] }, vec![])
        }
        _ => bail!("unknown app {key}"),
    };
    Ok(Plan {
        app_id: app_id.clone(),
        kind,
        name,
        to_version: rel.version.clone(),
        downloads: vec![dl],
        steps,
        block_dirs,
        block_names,
        verify: Verify::Version { app_id, version: rel.version.clone() },
        fallback: None,
        cleanup: vec![dest],
    })
}

/// `id` is the catalog or installed id ("tools:krita", "tools:freecad:1.1"); `ver` the exact
/// version to install, or None for the newest stable one.
pub async fn plan_install(id: &str, ver: Option<&str>, kind: JobKind, current: Option<&Installed>, installed: &[Installed], feed: &Feed, ctx: Ctx<'_>) -> Result<Plan> {
    let mut parts = id.splitn(3, ':');
    let _ = parts.next();
    let key = parts.next().unwrap_or("");
    let series = parts.next().filter(|_| key == "freecad");
    let mut rel = find_release(key, ver, series, feed).await?;
    match key {
        "krita" => {
            // Mirrors serve the file: its exact size comes from the mirror, its checksum from KDE.
            rel.size = remote_size(&rel.url).await;
            rel.sha256 = text_of(&format!("{}.sha256", rel.url)).await.and_then(|t| sum_from(&t, &rel.file));
        }
        "freecad" if rel.sha256.is_none() => {
            if let Some(u) = &rel.sums_url {
                rel.sha256 = text_of(u).await.and_then(|t| sum_from(&t, &rel.file));
            }
        }
        "godot" if rel.sha256.is_none() => {
            if let Some(u) = &rel.sums_url {
                rel.sha512 = text_of(u).await.and_then(|t| sha512_from(&t, &rel.file));
            }
        }
        _ => {}
    }
    // Installing a version of an app that is already here (OBS, Krita, a FreeCAD series) goes
    // over that copy.
    let here = current.or_else(|| {
        installed.iter().find(|i| tools_key(i) == key && !i.read_only && match key {
            "obs" | "krita" | "unityhub" | "epic" => true,
            "freecad" => series_of(i) == version::series(&rel.version),
            _ => false,
        })
    });
    let mut plan = build_plan(key, &rel, kind, here, &ctx)?;
    // A Unity editor only starts with a license from Unity Hub: install Hub first when it is missing.
    if key == "unity" && !installed.iter().any(|i| tools_key(i) == "unityhub") {
        if let Some(hub) = feed.list("unityhub").first().filter(|r| !r.url.is_empty() && r.sha512.is_some()) {
            let h = build_plan("unityhub", hub, JobKind::Install, None, &ctx)?;
            plan.downloads.splice(0..0, h.downloads);
            plan.steps.splice(0..0, h.steps);
            plan.cleanup.extend(h.cleanup);
        }
    }
    Ok(plan)
}

/// Uninstall steps for an app with an NSIS uninstaller. The uninstaller is run in place (`_?=`),
/// because otherwise it copies itself to a temporary folder and returns before the work is done;
/// the install folder is then removed here.
fn nsis_uninstall(inst: &Installed) -> Result<Vec<Step>> {
    let u = inst.uninstall.clone().filter(|u| !u.is_empty()).ok_or_else(|| anyhow!("{} has no uninstaller registered", inst.name))?;
    let exe = PathBuf::from(&u[0]);
    let mut args: Vec<String> = u[1..].iter().filter(|a| !a.starts_with("_?=")).cloned().collect();
    if !args.iter().any(|a| a.eq_ignore_ascii_case("/S")) {
        args.push("/S".into());
    }
    let admin = needs_admin(&exe);
    if let Some(p) = exe.parent() {
        args.push(format!("_?={}", p.display()));
    }
    let mut steps = vec![Step::Run { exe, args, cwd: None, ok_codes: vec![0], elevated: admin, label: format!("Uninstalling {}", inst.name), log: None }];
    if let Some(dir) = &inst.install_dir {
        // Only a real application folder is removed, never a drive, Program Files or similar.
        if safe_app_dir(dir) {
            steps.push(Step::RemoveDir { path: dir.clone(), elevated: needs_admin(dir), label: format!("Removing {}", dir.display()) });
        }
    }
    Ok(steps)
}

/// Epic's own setup program is the uninstaller: `--silent --uninstall`, no NSIS switches.
fn epic_uninstall(inst: &Installed) -> Result<Vec<Step>> {
    let u = inst.uninstall.clone().filter(|u| !u.is_empty()).ok_or_else(|| anyhow!("{} has no uninstaller registered", inst.name))?;
    let mut args: Vec<String> = u[1..].iter().filter(|a| !a.eq_ignore_ascii_case("/S")).cloned().collect();
    for a in ["--silent", "--uninstall"] {
        if !args.iter().any(|x| x.eq_ignore_ascii_case(a)) {
            args.push(a.into());
        }
    }
    Ok(vec![Step::Run { exe: PathBuf::from(&u[0]), args, cwd: None, ok_codes: vec![0], elevated: needs_admin(Path::new(&u[0])), label: format!("Uninstalling {}", inst.name), log: None }])
}

fn safe_app_dir(p: &Path) -> bool {
    let low = |x: &Path| x.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase();
    let me = low(p);
    p.components().count() >= 4
        && ![paths::program_files(), paths::program_files_x86(), paths::local_app_data(), paths::local_app_data().join("Programs"), paths::app_data(), unity_hub_editors()].iter().any(|x| low(x) == me)
}

pub fn plan_uninstall(inst: &Installed) -> Result<Plan> {
    let key = tools_key(inst);
    let steps = if key == "epic" {
        epic_uninstall(inst)?
    } else if key == "godot" {
        match &inst.install_dir {
            Some(d) => vec![Step::RemoveDir { path: d.clone(), elevated: needs_admin(d), label: format!("Removing {}", inst.name) }, Step::RemoveFile { path: godot_shortcut(&inst.name) }],
            None => {
                // A loose copy: just its program files.
                let exe = inst.exe.clone().ok_or_else(|| anyhow!("unknown file"))?;
                let console = exe.with_file_name(exe.file_name().map(|n| n.to_string_lossy().replace("_win64.exe", "_win64_console.exe")).unwrap_or_default());
                vec![Step::RemoveFile { path: exe }, Step::RemoveFile { path: console }, Step::RemoveFile { path: godot_shortcut(&inst.name) }]
            }
        }
    } else {
        nsis_uninstall(inst)?
    };
    Ok(Plan {
        app_id: inst.id.clone(),
        kind: JobKind::Uninstall,
        name: inst.name.clone(),
        to_version: inst.version.clone(),
        downloads: vec![],
        steps,
        block_dirs: inst.install_dir.iter().cloned().collect(),
        block_names: if inst.install_dir.is_none() { inst.exe.iter().filter_map(|e| e.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase())).collect() } else { vec![] },
        verify: Verify::Removed { app_id: inst.id.clone() },
        fallback: None,
        cleanup: vec![],
    })
}

/// Guess of how long an install takes, in seconds, for the progress bar before it has been learned.
pub fn install_seconds(key: &str, mb: f64) -> f64 {
    match key {
        "unity" => 120.0 + mb / 15.0,
        "godot" => 10.0 + mb / 60.0,
        _ => 20.0 + mb / 25.0,
    }
}

#[cfg(test)]
mod tests;
