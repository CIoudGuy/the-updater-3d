//! Blender. Every installed release series (4.2, 4.5, 5.2...) is its own app, because Blender
//! installs each series side by side. New series always install next to the old ones.
//! Bug-fix releases update their series in place (the MSI does that), or, with
//! "Keep bug-fix versions" on, install as a separate portable copy so the old one stays.
//!
//! Stable releases come from download.blender.org (with SHA-256 files), betas from
//! builder.blender.org.

use super::{BlenderKind, Download, Installed, Latest, Plan, SourceData, Step, Verify};
use crate::model::{CatalogEntry, JobKind, Source, VersionInfo};
use crate::{http, paths, version, winutil};
use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::time::Duration;

const RELEASES: &str = "https://download.blender.org/release/";
const BUILDER: &str = "https://builder.blender.org/download/daily/?format=json&v=1";
/// Known long-term support series; the LTS page on blender.org adds newer ones (`lts_series`).
const LTS: &[&str] = &["2.83", "2.93", "3.3", "3.6", "4.2", "4.5", "5.2"];
const LTS_PAGE: &str = "https://www.blender.org/download/lts/";
/// LTS series read from blender.org in this session.
static LTS_SEEN: std::sync::RwLock<Vec<String>> = std::sync::RwLock::new(Vec::new());
const FG: &str = "#F5792A";
const BG: &str = "#2B1607";
pub const MARKER: &str = ".the-updater-3d.json";

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Marker {
    pub version: String,
    pub kind: BlenderKind,
    #[serde(default)]
    pub build: Option<String>,
    /// "Alpha", "Beta", "Release candidate" for pre-release builds
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub date: Option<String>,
    pub msi: Option<String>,
    pub zip: Option<String>,
    pub msi_size: Option<u64>,
    pub zip_size: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Beta {
    pub version: String,
    pub series: String,
    pub build: String,
    pub url: String,
    pub size: Option<u64>,
    pub date: Option<String>,
    pub label: String,
}

impl Beta {
    /// "5.4.0 Alpha fbe82a4": what the UI shows and passes back to install this build.
    pub fn display(&self) -> String {
        build_display(&self.version, &self.label, &self.build)
    }
}

pub fn build_display(ver: &str, label: &str, build: &str) -> String {
    format!("{} {} {}", version::trim_like(ver, "1.2.3"), label, &build[..build.len().min(7)]).split_whitespace().collect::<Vec<_>>().join(" ")
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    /// series -> releases newest first
    pub series: BTreeMap<String, Vec<Release>>,
    pub newest_series: Option<String>,
    pub beta: Option<Beta>,
    /// Every release series on download.blender.org, newest first (2.80 and later).
    pub all_series: Vec<String>,
    /// Every pre-release build for Windows (alpha, beta, release candidate), newest first.
    pub builds: Vec<Beta>,
}

impl Feed {
    /// The build a "-beta" app id or a picked version means: an exact build when the version
    /// string ends with its hash, else the newest build of that series, else the featured beta.
    pub fn build_for(&self, id: &str, ver: Option<&str>) -> Option<&Beta> {
        if let Some(v) = ver {
            let hash = v.split_whitespace().last().unwrap_or("");
            if let Some(b) = self.builds.iter().find(|b| !hash.is_empty() && b.build.starts_with(hash)) {
                return Some(b);
            }
        }
        let series = id.trim_start_matches("blender:").trim_end_matches("-beta");
        self.builds.iter().find(|b| b.series == series).or(self.beta.as_ref())
    }
}

impl Feed {
    pub fn newest_in(&self, series: &str) -> Option<&Release> {
        self.series.get(series).and_then(|v| v.first())
    }
    pub fn find(&self, ver: &str) -> Option<&Release> {
        self.series.get(&version::series(ver))?.iter().find(|r| version::cmp(&r.version, ver).is_eq())
    }
}

fn is_lts(series: &str) -> bool {
    LTS.contains(&series) || LTS_SEEN.read().is_ok_and(|l| l.iter().any(|s| s == series))
}

/// The LTS series named on blender.org's LTS page ("Blender 4.5 LTS").
fn parse_lts(html: &str) -> Vec<String> {
    use once_cell::sync::Lazy;
    static RE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"Blender (\d+\.\d+) LTS").unwrap());
    let mut v: Vec<String> = RE.captures_iter(html).map(|c| c[1].to_string()).filter(|s| !version::newer("2.80", s)).collect();
    v.sort_by(|a, b| version::cmp(a, b));
    v.dedup();
    v
}

/// Every LTS series known, oldest first: the built-in list and what blender.org lists.
fn lts_series() -> Vec<String> {
    let mut v: Vec<String> = LTS.iter().map(|s| s.to_string()).collect();
    if let Ok(l) = LTS_SEEN.read() {
        v.extend(l.iter().cloned());
    }
    v.sort_by(|a, b| version::cmp(a, b));
    v.dedup();
    v
}

// ---------------------------------------------------------------- feeds

/// Parse an nginx/Apache index: (name, date, size) per link.
fn parse_index(html: &str) -> Vec<(String, Option<String>, Option<u64>)> {
    let re = regex::Regex::new(r#"<a href="([^"]+)">[^<]*</a>\s+(\d{2}-\w{3}-\d{4} \d{2}:\d{2})?\s*([\d.]+[KMG]?|-)?"#).unwrap();
    re.captures_iter(html)
        .map(|c| {
            let name = c.get(1).map(|m| m.as_str().to_string()).unwrap_or_default();
            let date = c.get(2).and_then(|m| chrono::NaiveDateTime::parse_from_str(m.as_str(), "%d-%b-%Y %H:%M").ok()).map(|d| d.and_utc().to_rfc3339());
            let size = c.get(3).and_then(|m| {
                let s = m.as_str();
                let (num, mul) = match s.chars().last() {
                    Some('K') => (&s[..s.len() - 1], 1024f64),
                    Some('M') => (&s[..s.len() - 1], 1024f64 * 1024.0),
                    Some('G') => (&s[..s.len() - 1], 1024f64 * 1024.0 * 1024.0),
                    _ => (s, 1.0),
                };
                num.parse::<f64>().ok().map(|n| (n * mul) as u64)
            });
            (name, date, size)
        })
        .collect()
}

/// The version and kind ("msi" or "zip") of a 64-bit Windows release file:
/// "blender-5.2.2-windows-x64.msi", and "blender-2.81-windows64.zip" for 2.80 to 2.92.
#[cfg(test)]
fn windows_file(name: &str) -> Option<(String, &'static str)> {
    windows_file_for(name, "x64").map(|(v, k, _)| (v, k))
}

/// How well a build for `arch` suits a PC whose processor is `native`: a native ARM64 build is
/// best on ARM64, x64 runs everywhere (emulated on ARM64), an ARM64 build never runs on x64.
fn arch_rank(arch: &str, native: &str) -> Option<u8> {
    match arch.to_ascii_lowercase().as_str() {
        "arm64" if native == "arm64" => Some(2),
        "arm64" => None,
        "x64" | "amd64" | "x86_64" | "64" => Some(1),
        _ => None,
    }
}

/// `windows_file` for a PC with the `native` processor, with the file's `arch_rank`.
fn windows_file_for(name: &str, native: &str) -> Option<(String, &'static str, u8)> {
    use once_cell::sync::Lazy;
    static FILE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"^blender-(\d+\.\d+(?:\.\d+)?)-windows-?(x64|64|arm64)\.(msi|zip)$").unwrap());
    let c = FILE.captures(name)?;
    let rank = arch_rank(&c[2], native)?;
    Some((c[1].to_string(), if &c[3] == "msi" { "msi" } else { "zip" }, rank))
}

async fn series_releases(series: &str) -> Result<Vec<Release>> {
    let url = format!("{RELEASES}Blender{series}/");
    let f = http::get_cached(&url, &[], Duration::from_secs(30 * 60)).await?;
    let html = String::from_utf8_lossy(&f.body);
    Ok(releases_from_index(&url, &html, winutil::native_arch()))
}

/// The releases in a series folder's listing, with the files that suit the `native` processor.
fn releases_from_index(url: &str, html: &str, native: &str) -> Vec<Release> {
    let mut by_ver: HashMap<String, Release> = HashMap::new();
    let mut ranks: HashMap<(String, &'static str), u8> = HashMap::new();
    for (name, date, size) in parse_index(html) {
        let Some((ver, kind, rank)) = windows_file_for(&name, native) else { continue };
        let best = ranks.entry((ver.clone(), kind)).or_insert(0);
        if rank <= *best {
            continue;
        }
        *best = rank;
        let r = by_ver.entry(ver.clone()).or_insert(Release { version: ver, date: None, msi: None, zip: None, msi_size: None, zip_size: None });
        let full = format!("{url}{name}");
        if kind == "msi" {
            r.msi = Some(full);
            r.msi_size = size;
            r.date = date.or(r.date.take());
        } else {
            r.zip = Some(full);
            r.zip_size = size;
            if r.date.is_none() {
                r.date = date;
            }
        }
    }
    let mut v: Vec<Release> = by_ver.into_values().collect();
    v.sort_by(|a, b| version::cmp(&b.version, &a.version));
    v
}

pub async fn fetch(installed: &[Installed], beta_channel: bool) -> Result<Feed> {
    let f = http::get_cached(RELEASES, &[], Duration::from_secs(30 * 60)).await?;
    let html = String::from_utf8_lossy(&f.body);
    let re = regex::Regex::new(r"^Blender(\d+\.\d+)/$").unwrap();
    let mut all_series: Vec<String> = parse_index(&html).into_iter().filter_map(|(n, _, _)| re.captures(&n).map(|c| c[1].to_string())).collect();
    all_series.sort_by(|a, b| version::cmp(b, a));
    all_series.dedup();
    let mut wanted: Vec<String> = installed.iter().filter_map(|i| match &i.data {
        SourceData::Blender { series, kind, .. } if *kind != BlenderKind::Beta => Some(series.clone()),
        _ => None,
    }).collect();
    // The newest series that actually has a Windows build yet.
    let mut newest = None;
    for s in all_series.iter().take(3) {
        if let Ok(list) = series_releases(s).await {
            if list.iter().any(|r| r.msi.is_some() || r.zip.is_some()) {
                newest = Some(s.clone());
                break;
            }
        }
    }
    if let Some(n) = &newest {
        wanted.push(n.clone());
    }
    if let Ok(f) = http::get_cached(LTS_PAGE, &[], Duration::from_secs(24 * 3600)).await {
        let found = parse_lts(&String::from_utf8_lossy(&f.body));
        if !found.is_empty() {
            if let Ok(mut l) = LTS_SEEN.write() {
                *l = found;
            }
        }
    }
    if let Some(lts) = lts_series().last() {
        wanted.push(lts.clone());
    }
    wanted.sort();
    wanted.dedup();
    let mut feed = Feed { newest_series: newest, ..Default::default() };
    feed.all_series = all_series.iter().filter(|s| !version::newer("2.80", s)).cloned().collect();
    for s in wanted {
        match series_releases(&s).await {
            Ok(list) if !list.is_empty() => {
                feed.series.insert(s, list);
            }
            Ok(_) => {}
            Err(e) => log::warn!("blender {s}: {e}"),
        }
    }
    // Betas are always listed in Get apps (marked Beta); the channel setting decides updates.
    let _ = beta_channel;
    {
        feed.builds = fetch_builds().await.unwrap_or_else(|e| {
            log::warn!("blender pre-releases: {e}");
            vec![]
        });
        // The featured pre-release: the newest beta or release candidate of a new series.
        feed.beta = feed
            .builds
            .iter()
            .find(|b| b.label != "Alpha" && version::parts(&b.version).get(2).copied().unwrap_or(0) == 0)
            .or(feed.builds.first())
            .cloned();
    }
    Ok(feed)
}

async fn fetch_builds() -> Result<Vec<Beta>> {
    let f = http::get_cached(BUILDER, &[], Duration::from_secs(30 * 60)).await?;
    let v: serde_json::Value = serde_json::from_slice(&f.body)?;
    Ok(builds_from(&v, winutil::native_arch()))
}

/// Pre-release builds for Windows on the `native` processor, one per version and kind.
fn builds_from(v: &serde_json::Value, native: &str) -> Vec<Beta> {
    let mut builds: Vec<(Beta, (u8, i64))> = Vec::new();
    for b in v.as_array().into_iter().flatten() {
        let rank = b["architecture"].as_str().and_then(|a| arch_rank(a, native));
        let platform_ok = b["platform"] == "windows" && rank.is_some();
        let rank = rank.unwrap_or(0);
        let risk = b["risk_id"].as_str().unwrap_or("");
        let label = match risk {
            "alpha" => "Alpha",
            "beta" => "Beta",
            "rc" | "candidate" => "Release candidate",
            _ => continue,
        };
        if !platform_ok || b["file_extension"] != "zip" {
            continue;
        }
        let ver = b["version"].as_str().unwrap_or("").to_string();
        let mtime = b["file_mtime"].as_i64().unwrap_or(0);
        let cand = Beta {
            series: version::series(&ver),
            build: b["hash"].as_str().unwrap_or("").to_string(),
            url: b["url"].as_str().unwrap_or("").to_string(),
            size: b["file_size"].as_u64(),
            date: chrono::DateTime::from_timestamp(mtime, 0).map(|d| d.to_rfc3339()),
            label: label.into(),
            version: ver,
        };
        // One build per version and kind: for the native processor if there is one, the newest.
        match builds.iter_mut().find(|(o, _)| o.version == cand.version && o.label == cand.label) {
            Some(slot) if (rank, mtime) > slot.1 => *slot = (cand, (rank, mtime)),
            Some(_) => {}
            None => builds.push((cand, (rank, mtime))),
        }
    }
    builds.sort_by(|a, b| version::cmp(&b.0.version, &a.0.version).then(b.1 .1.cmp(&a.1 .1)));
    builds.into_iter().map(|(b, _)| b).collect()
}

// ---------------------------------------------------------------- installed copies

#[derive(Serialize, Deserialize, Default)]
struct VersionCache {
    /// exe path -> (modified unix time, version)
    entries: HashMap<String, (i64, String)>,
}

/// The version of a portable copy we did not install (cached by exe mtime). Read from the exe's
/// version resource: running `blender.exe --version` in the background upset antivirus.
fn probe_version(exe: &Path) -> Option<String> {
    let cache_path = paths::data_dir().join("blender-versions.json");
    let mut cache: VersionCache = std::fs::read(&cache_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let mtime = exe.metadata().ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_secs() as i64;
    let key = exe.to_string_lossy().to_string();
    if let Some((t, v)) = cache.entries.get(&key) {
        if *t == mtime {
            return Some(v.clone());
        }
    }
    let v = winutil::exe_version(exe)?;
    cache.entries.insert(key, (mtime, v.clone()));
    let _ = std::fs::write(&cache_path, serde_json::to_vec(&cache).unwrap_or_default());
    Some(v)
}

fn steam_libraries() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let steam = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Valve\\Steam")
        .and_then(|k| k.get_value::<String, _>("SteamPath"))
        .map(|s| PathBuf::from(s.replace('/', "\\")))
        .unwrap_or_else(|_| paths::program_files_x86().join("Steam"));
    out.push(steam.clone());
    if let Ok(vdf) = std::fs::read_to_string(steam.join("steamapps\\libraryfolders.vdf")) {
        let re = regex::Regex::new(r#""path"\s+"([^"]+)""#).unwrap();
        for c in re.captures_iter(&vdf) {
            out.push(PathBuf::from(c[1].replace("\\\\", "\\")));
        }
    }
    out.sort();
    out.dedup();
    out
}

fn launcher(dir: &Path) -> Option<PathBuf> {
    let l = dir.join("blender-launcher.exe");
    if l.exists() {
        return Some(l);
    }
    let b = dir.join("blender.exe");
    b.exists().then_some(b)
}

fn mtime_iso(p: &Path) -> Option<String> {
    p.metadata().ok()?.modified().ok().map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())
}

pub fn scan(uninstall: &[winutil::UninstallEntry], install_root: &Path) -> Vec<Installed> {
    let mut out: Vec<Installed> = Vec::new();
    let mut seen_dirs: Vec<String> = Vec::new();
    let norm = |p: &Path| p.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase();

    for e in uninstall.iter().filter(|e| e.display_name.eq_ignore_ascii_case("blender") && e.publisher.to_ascii_lowercase().contains("blender")) {
        let dir = winutil::pathbuf(&e.install_location);
        let exe = launcher(&dir);
        let series = version::series(&e.display_version);
        seen_dirs.push(norm(&dir));
        let code = if e.key.starts_with('{') { Some(e.key.clone()) } else { None };
        out.push(Installed {
            id: format!("blender:{series}"),
            source: Source::Blender,
            name: format!("Blender {series}"),
            code: "Bl".into(),
            fg: FG.into(),
            bg: BG.into(),
            version: e.display_version.clone(),
            missing: exe.is_none(),
            install_dir: Some(dir.clone()),
            uninstall: code.as_ref().map(|c| vec!["msiexec.exe".into(), "/x".into(), c.clone(), "/qn".into(), "/norestart".into()]),
            updated_at: winutil::install_date_iso(&e.install_date).or_else(|| exe.as_deref().and_then(mtime_iso)),
            exe,
            size_hint: Some(e.estimated_size_kb * 1024).filter(|s| *s > 0),
            tagline: Some(if is_lts(&series) { "Long-term support release".into() } else { "3D creation suite".into() }),
            channel: Some(if is_lts(&series) { "lts".into() } else { "stable".into() }),
            read_only: false,
            store: None,
            icon_remote: None,
            data: SourceData::Blender { series, kind: BlenderKind::Msi, msi_product_code: code },
        });
    }

    // Portable copies in the Blender folders (ours carry a marker with the exact version).
    let mut roots = vec![install_root.to_path_buf(), paths::program_files().join("Blender Foundation")];
    roots.sort();
    roots.dedup();
    for root in roots {
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        for d in rd.flatten() {
            let dir = d.path();
            if !dir.is_dir() || seen_dirs.contains(&norm(&dir)) {
                continue;
            }
            let Some(exe) = launcher(&dir) else { continue };
            let marker: Option<Marker> = std::fs::read(dir.join(MARKER)).ok().and_then(|b| serde_json::from_slice(&b).ok());
            let (ver, kind, build) = match &marker {
                Some(m) => (m.version.clone(), m.kind, m.build.clone()),
                None => match probe_version(&dir.join("blender.exe")) {
                    Some(v) => (v, BlenderKind::Zip, None),
                    None => continue,
                },
            };
            seen_dirs.push(norm(&dir));
            let series = version::series(&ver);
            let (id, name, channel) = if kind == BlenderKind::Beta {
                (format!("blender:{series}-beta"), dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| format!("Blender {series} Beta")), "beta".to_string())
            } else {
                (format!("blender:{ver}"), format!("Blender {}", version::trim_like(&ver, "1.2.3")), if is_lts(&series) { "lts".into() } else { "stable".into() })
            };
            let label = marker.as_ref().and_then(|m| m.label.clone()).unwrap_or_else(|| "Beta".into());
            let display_ver = if kind == BlenderKind::Beta { build_display(&ver, &label, build.as_deref().unwrap_or("")) } else { ver.clone() };
            out.push(Installed {
                id,
                source: Source::Blender,
                name,
                code: "Bl".into(),
                fg: FG.into(),
                bg: BG.into(),
                version: display_ver,
                install_dir: Some(dir.clone()),
                updated_at: mtime_iso(&exe),
                exe: Some(exe),
                uninstall: None,
                size_hint: None,
                tagline: Some(if kind == BlenderKind::Beta { "Pre-release build, for testing".into() } else { "Portable copy".into() }),
                channel: Some(channel),
                missing: false,
                read_only: false,
                store: None,
                icon_remote: None,
                data: SourceData::Blender { series, kind, msi_product_code: None },
            });
        }
    }

    for lib in steam_libraries() {
        let dir = lib.join("steamapps\\common\\Blender");
        let Some(exe) = launcher(&dir) else { continue };
        let Some(ver) = probe_version(&dir.join("blender.exe")) else { continue };
        out.push(Installed {
            id: "blender:steam".into(),
            source: Source::Blender,
            name: "Blender (Steam)".into(),
            code: "Bl".into(),
            fg: FG.into(),
            bg: BG.into(),
            version: ver.clone(),
            install_dir: Some(dir.clone()),
            updated_at: mtime_iso(&exe),
            exe: Some(exe),
            uninstall: None,
            size_hint: None,
            tagline: Some("Updated by Steam".into()),
            channel: Some("stable".into()),
            missing: false,
            read_only: true,
            store: None,
            icon_remote: None,
            data: SourceData::Blender { series: version::series(&ver), kind: BlenderKind::Steam, msi_product_code: None },
        });
        break;
    }
    out.extend(store_copies(&super::msstore::packages()));
    out.sort_by(|a, b| version::cmp(&b.version, &a.version));
    out
}

/// Publisher id of the Blender Foundation's Microsoft Store packages.
const STORE_PUBLISHER: &str = "ppwjx1n5r4v9t";

/// Blender from the Microsoft Store: "Blender" and one package per LTS series
/// ("BlenderFoundation.Blender4.5LTS"). Listed only; the Store updates and removes them.
pub fn store_copies(pkgs: &[super::msstore::Package]) -> Vec<Installed> {
    let mut out = Vec::new();
    for p in pkgs {
        let Some(rest) = p.name.strip_prefix("BlenderFoundation.Blender") else { continue };
        if p.publisher_id() != STORE_PUBLISHER || !rest.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') {
            continue;
        }
        let c = super::msstore::copy_of(p);
        let series = version::series(&c.version);
        let id = if rest.is_empty() { "blender:store".to_string() } else { format!("blender:store:{}", rest.to_ascii_lowercase()) };
        out.push(Installed {
            id,
            source: Source::Blender,
            name: format!("Blender {series} (Microsoft Store)"),
            code: "Bl".into(),
            fg: FG.into(),
            bg: BG.into(),
            version: c.version,
            install_dir: p.install_dir.clone(),
            updated_at: p.installed_at.clone(),
            exe: c.exe,
            uninstall: None,
            size_hint: None,
            tagline: Some("Updated by the Microsoft Store".into()),
            channel: Some(if is_lts(&series) { "lts".into() } else { "stable".into() }),
            missing: false,
            read_only: true,
            store: Some(c.store),
            icon_remote: None,
            data: SourceData::Blender { series, kind: BlenderKind::Store, msi_product_code: None },
        });
    }
    out
}

fn kind_of(i: &Installed) -> (String, BlenderKind) {
    match &i.data {
        SourceData::Blender { series, kind, .. } => (series.clone(), *kind),
        _ => (String::new(), BlenderKind::Zip),
    }
}

pub fn latest(inst: &Installed, all: &[Installed], feed: &Feed) -> Option<Latest> {
    latest_kept(inst, all, feed, false)
}

/// Is this a portable copy we unpacked (it has our marker)? Only those are ever removed.
fn ours(dir: Option<&PathBuf>) -> bool {
    dir.is_some_and(|d| d.join(MARKER).is_file())
}

/// `keep_patch_versions` is the setting that keeps old bug-fix copies of a series.
pub fn latest_kept(inst: &Installed, all: &[Installed], feed: &Feed, keep_patch_versions: bool) -> Option<Latest> {
    let (series, kind) = kind_of(inst);
    match kind {
        BlenderKind::Steam | BlenderKind::Store => None,
        BlenderKind::Beta => {
            let b = feed.builds.iter().find(|b| b.series == series)?;
            let installed_hash = inst.version.split_whitespace().last().unwrap_or("");
            let same = !installed_hash.is_empty() && b.build.starts_with(installed_hash);
            Some(Latest { version: if same { inst.version.clone() } else { b.display() }, size: b.size, note: None })
        }
        BlenderKind::Msi | BlenderKind::Zip => {
            let newest_patch = feed.newest_in(&series)?;
            if version::newer(&newest_patch.version, &inst.version) {
                let note = match kind {
                    BlenderKind::Zip if !keep_patch_versions && ours(inst.install_dir.as_ref()) => Some(format!("{} replaces this copy", newest_patch.version)),
                    BlenderKind::Zip => Some(format!("{} installs as a new copy, this one stays", newest_patch.version)),
                    _ => None,
                };
                return Some(Latest { version: newest_patch.version.clone(), size: newest_patch.msi_size.or(newest_patch.zip_size), note });
            }
            // Up to date in its series: the newest installed stable series is offered the next one.
            let newest_installed = all
                .iter()
                .filter(|o| matches!(kind_of(o).1, BlenderKind::Msi | BlenderKind::Zip))
                .map(|o| kind_of(o).0)
                .max_by(|a, b| version::cmp(a, b))?;
            if series == newest_installed {
                if let Some(ns) = &feed.newest_series {
                    if version::newer(ns, &series) {
                        if let Some(r) = feed.newest_in(ns) {
                            return Some(Latest {
                                version: r.version.clone(),
                                size: r.msi_size.or(r.zip_size),
                                note: Some(format!("{} installs next to {series}, your {series} stays", version::series(&r.version))),
                            });
                        }
                    }
                }
            }
            Some(Latest { version: inst.version.clone(), size: None, note: None })
        }
    }
}

pub fn catalog_entries(feed: &Feed, installed: &[Installed], _beta_channel: bool) -> Vec<CatalogEntry> {
    let have: Vec<String> = installed.iter().filter(|i| matches!(kind_of(i).1, BlenderKind::Msi | BlenderKind::Zip)).map(|i| kind_of(i).0).collect();
    let mut out = Vec::new();
    let mut offer = |id: String, name: String, version: String, size: Option<u64>, kind: &str, pre: bool, installed: bool, pick: bool| {
        let (tagline, description) = super::about::blender(kind);
        out.push(CatalogEntry {
            id,
            source: Source::Blender,
            name,
            code: "Bl".into(),
            fg: FG.into(),
            bg: BG.into(),
            icon_url: None,
            icon_miss: false,
            version,
            size,
            tagline: Some(tagline.into()),
            description: Some(description),
            blocked: None,
            group: Source::Blender.group().into(),
            prerelease: pre,
            installed,
            pick_version: pick,
        });
    };
    // Newest stable first, then the long-term support release.
    let mut series: Vec<&String> = feed.series.keys().collect();
    series.sort_by(|a, b| version::cmp(b, a));
    let newest = feed.newest_series.clone();
    for s in series {
        let is_newest = newest.as_deref() == Some(s.as_str());
        if !is_newest && !is_lts(s) {
            continue;
        }
        if let Some(r) = feed.series[s].first() {
            let kind = if is_lts(s) { "lts" } else { "stable" };
            offer(format!("blender:{s}"), format!("Blender {s}"), r.version.clone(), r.msi_size.or(r.zip_size), kind, false, have.contains(s), true);
        }
    }
    if let Some(b) = &feed.beta {
        // The card offers every pre-release build: it counts as installed only when all of them are.
        let inst = all_builds_installed(feed, installed);
        offer(format!("blender:{}-beta", b.series), format!("Blender {} {}", b.series, b.label), b.display(), b.size, "beta", true, inst, true);
    }
    if !feed.all_series.is_empty() {
        offer("blender:older".into(), "Older Blender versions".into(), format!("{} and earlier", feed.all_series.get(1).cloned().unwrap_or_default()), None, "older", false, false, true);
    }
    out
}

fn prerelease_info(b: &Beta, latest: bool, installed: bool) -> VersionInfo {
    VersionInfo {
        version: b.display(),
        date: b.date.clone(),
        notes: vec![format!("{} build of Blender {}", b.label, version::trim_like(&b.version, "1.2.3"))],
        notes_url: Some(format!("https://developer.blender.org/docs/release_notes/{}/", b.series)),
        size: b.size,
        installed,
        latest,
        prerelease: true,
    }
}

/// Every pre-release build on offer is installed (each copy's version ends with its build hash).
fn all_builds_installed(feed: &Feed, installed: &[Installed]) -> bool {
    !feed.builds.is_empty()
        && feed.builds.iter().all(|b| {
            installed.iter().any(|i| {
                matches!(&i.data, SourceData::Blender { kind: BlenderKind::Beta, .. })
                    && i.version.split_whitespace().last().is_some_and(|h| !h.is_empty() && b.build.starts_with(h))
            })
        })
}

/// Pre-release builds carry the release notes of their version as written so far (Blender keeps
/// them for versions still in development), after the line that says which build it is.
async fn with_notes_so_far(mut list: Vec<VersionInfo>) -> Vec<VersionInfo> {
    let mut series: Vec<String> = list.iter().map(|v| version::series(v.version.split_whitespace().next().unwrap_or(""))).collect();
    series.sort();
    series.dedup();
    let found = futures_util::future::join_all(series.iter().map(|s| async move { (s.clone(), crate::notes::blender_major(s).await) })).await;
    for v in &mut list {
        let s = version::series(v.version.split_whitespace().next().unwrap_or(""));
        if let Some((_, n)) = found.iter().find(|(x, n)| *x == s && !n.lines.is_empty()) {
            let head = v.notes.first().cloned().unwrap_or_default();
            v.notes = vec![format!("{head}, with the release notes written so far")];
            v.notes.extend(n.lines.iter().cloned());
        }
    }
    list
}

/// Versions offered by the Get apps version picker for a Blender entry.
pub async fn catalog_versions(id: &str, feed: &Feed) -> Result<Vec<VersionInfo>> {
    let key = id.trim_start_matches("blender:");
    if key.ends_with("-beta") {
        if feed.builds.is_empty() {
            return Err(anyhow!("No pre-release builds right now"));
        }
        return Ok(with_notes_so_far(feed.builds.iter().enumerate().map(|(i, b)| prerelease_info(b, i == 0, false)).collect()).await);
    }
    if key == "older" {
        // Newest bug-fix release of every series, with the notes of its series.
        let futs = feed.all_series.iter().map(|s| async move { (s.clone(), series_releases(s).await) });
        let results = futures_util::future::join_all(futs).await;
        let newest: Vec<(String, Release)> = results
            .into_iter()
            .filter_map(|(s, r)| r.ok().and_then(|list| list.into_iter().find(|r| r.msi.is_some() || r.zip.is_some())).map(|r| (s, r)))
            .collect();
        // A few series at a time: each reads one to three pages, a first release many more.
        use futures_util::StreamExt;
        let infos: Vec<VersionInfo> = futures_util::stream::iter(newest.into_iter().map(|(s, r)| older_info(s, r))).buffered(4).collect().await;
        return Ok(infos
            .into_iter()
            .enumerate()
            .map(|(i, mut v)| {
                v.latest = i == 0;
                v
            })
            .collect());
    }
    let list = match feed.series.get(key) {
        Some(l) => l.clone(),
        None => series_releases(key).await?,
    };
    let (notes, src) = crate::notes::blender_with_source(key, true).await;
    Ok(list
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let mut v = release_info(key, r, &notes, src.as_deref());
            v.latest = i == 0;
            v
        })
        .collect())
}

pub async fn versions(inst: &Installed, feed: &Feed) -> Result<Vec<VersionInfo>> {
    let (series, kind) = kind_of(inst);
    if kind == BlenderKind::Beta {
        // Every pre-release build: one of another series installs next to this copy.
        let hash = inst.version.split_whitespace().last().unwrap_or("").to_string();
        let list: Vec<VersionInfo> = feed
            .builds
            .iter()
            .enumerate()
            .map(|(i, b)| prerelease_info(b, i == 0, !hash.is_empty() && b.build.starts_with(&hash)))
            .collect();
        if list.is_empty() {
            return Err(anyhow!("No pre-release builds of Blender {series} right now"));
        }
        return Ok(with_notes_so_far(list).await);
    }
    let list = match feed.series.get(&series) {
        Some(l) => l.clone(),
        None => series_releases(&series).await?,
    };
    let (notes, src) = crate::notes::blender_with_source(&series, true).await;
    let newest = list.first().map(|r| r.version.clone());
    Ok(list
        .into_iter()
        .map(|r| {
            let mut v = release_info(&series, &r, &notes, src.as_deref());
            v.installed = version::cmp(&r.version, &inst.version).is_eq();
            v.latest = newest.as_deref() == Some(r.version.as_str());
            v
        })
        .collect())
}

/// The newest release of an older series for "Older Blender versions", with its notes: its fix
/// list, the release notes for a first release, and for a series that published no fix lists
/// (2.83, 2.91, 2.93) the series' release notes, said so on top.
async fn older_info(series: String, r: Release) -> VersionInfo {
    let series = series.as_str();
    let patch = version::parts(&r.version).get(2).copied().unwrap_or(0);
    let (mut notes, src) = crate::notes::blender_with_source(series, patch == 0).await;
    if patch > 0 && notes.is_empty() {
        let m = crate::notes::blender_major(series).await;
        if !m.lines.is_empty() {
            let mut lines = vec![format!("No separate fix list is published for {}; these are the release notes of Blender {series}", r.version)];
            lines.extend(m.lines);
            notes.insert(r.version.clone(), crate::notes::Notes { date: None, lines, url: None });
        }
    }
    release_info(series, &r, &notes, src.as_deref())
}

/// One Blender release with its fix list and date from blender.org when available. `fixes_url`
/// is the page the fix lists came from.
fn release_info(series: &str, r: &Release, notes: &HashMap<String, crate::notes::Notes>, fixes_url: Option<&str>) -> VersionInfo {
    let patch = version::parts(&r.version).get(2).copied().unwrap_or(0);
    // The first release is "2.81" in the file names of old series and "2.81.0" in the notes.
    let n = notes.get(&r.version).or_else(|| if patch == 0 { notes.get(&format!("{series}.0")) } else { None });
    let mut lines = n.map(|n| n.lines.clone()).unwrap_or_default();
    if lines.is_empty() {
        lines.push(if patch == 0 { format!("First release of Blender {series}. See the full notes for everything new.") } else { format!("Bug-fix release for Blender {series}. Blender has not published its list of fixes yet; it shows here once it does.") });
    }
    let notes_url = if patch == 0 || notes.is_empty() {
        format!("https://developer.blender.org/docs/release_notes/{series}/")
    } else if let Some(u) = fixes_url {
        u.to_string()
    } else if is_lts(series) {
        format!("https://www.blender.org/download/lts/{}/", series.replace('.', "-"))
    } else {
        format!("https://developer.blender.org/docs/release_notes/{series}/corrective_releases/")
    };
    VersionInfo {
        version: r.version.clone(),
        date: n.and_then(|n| n.date.clone()).or_else(|| r.date.clone()),
        notes: lines,
        notes_url: Some(notes_url),
        size: r.msi_size.or(r.zip_size),
        installed: false,
        latest: false,
        prerelease: false,
    }
}

/// The checksum files a release may have: "blender-4.2.3.sha256", and for 2.80 to 2.82 one per
/// release named "release281a.sha256".
fn sha_urls(ver: &str) -> Vec<String> {
    let series = version::series(ver);
    let dir = format!("{RELEASES}Blender{series}/");
    vec![format!("{dir}blender-{ver}.sha256"), format!("{dir}release{}.sha256", ver.replace('.', ""))]
}

fn sha_in(text: &str, name: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let h = it.next()?;
        (it.next()?.trim_start_matches('*') == name && h.len() == 64).then(|| h.to_ascii_lowercase())
    })
}

async fn sha_for(ver: &str, file_url: &str) -> Option<String> {
    let name = file_url.rsplit('/').next()?;
    for url in sha_urls(ver) {
        if let Ok(f) = http::get_cached(&url, &[], Duration::from_secs(24 * 3600)).await {
            if let Some(h) = sha_in(&String::from_utf8_lossy(&f.body), name) {
                return Some(h);
            }
        }
    }
    None
}

fn needs_admin(p: &Path) -> bool {
    let s = p.to_string_lossy().to_ascii_lowercase();
    s.starts_with(&paths::program_files().to_string_lossy().to_ascii_lowercase()) || s.starts_with(&paths::program_files_x86().to_string_lossy().to_ascii_lowercase())
}

fn shortcut_path(name: &str) -> PathBuf {
    paths::app_data().join("Microsoft\\Windows\\Start Menu\\Programs\\Blender").join(format!("{name}.lnk"))
}

/// Steps that unpack a portable Blender zip into `dir`, write our marker and a Start menu entry.
fn zip_steps(zip: &Path, dir: &Path, marker: &Marker, name: &str, replace: bool) -> Vec<Step> {
    let admin = needs_admin(dir);
    let mut steps = Vec::new();
    if replace {
        steps.push(Step::RemoveDir { path: dir.to_path_buf(), elevated: admin, label: format!("Removing the old {name}") });
    }
    steps.push(Step::ExtractZip { zip: zip.to_path_buf(), dest: dir.to_path_buf(), strip_root: true, elevated: admin, label: format!("Unpacking {name}") });
    steps.push(Step::WriteFile { path: dir.join(MARKER), contents: serde_json::to_string(marker).unwrap_or_default(), elevated: admin });
    steps.push(Step::Shortcut { lnk: shortcut_path(name), target: dir.join("blender-launcher.exe"), label: "Adding it to the Start menu".into() });
    steps
}

pub struct Ctx<'a> {
    pub root: &'a Path,
    pub keep_patch_versions: bool,
    pub keep_previous: bool,
}

/// Install `ver` (stable) or the beta (`ver` = "beta"). `current` is the copy being updated.
pub async fn plan_install(id: &str, ver: Option<&str>, kind: JobKind, current: Option<&Installed>, feed: &Feed, ctx: Ctx<'_>) -> Result<Plan> {
    let default_root = paths::program_files().join("Blender Foundation");
    let use_msi_dir = ctx.root.to_string_lossy().eq_ignore_ascii_case(&default_root.to_string_lossy());

    // Beta builds (always portable, replaced in place on update).
    if id.ends_with("-beta") {
        let b = feed.build_for(id, ver).cloned().ok_or_else(|| anyhow!("There is no Blender pre-release build right now"))?;
        let name = format!("Blender {} {}", b.series, b.label);
        let dir = ctx.root.join(&name);
        let zip = paths::downloads().join("blender").join(b.url.rsplit('/').next().unwrap_or("blender-beta.zip"));
        let sha = http::get_cached(&format!("{}.sha256", b.url), &[], Duration::from_secs(3600)).await.ok().and_then(|f| String::from_utf8_lossy(&f.body).split_whitespace().next().map(|s| s.to_ascii_lowercase()));
        let marker = Marker { version: b.version.clone(), kind: BlenderKind::Beta, build: Some(b.build.clone()), label: Some(b.label.clone()) };
        let dl = Download { url: b.url.clone(), dest: zip.clone(), size: b.size, sha256: sha, md5: None, signer: None, sha512: None, headers: vec![] };
        super::require_checked(&dl, &name)?;
        return Ok(Plan {
            app_id: format!("blender:{}-beta", b.series),
            kind,
            name: name.clone(),
            to_version: b.display(),
            downloads: vec![dl],
            steps: zip_steps(&zip, &dir, &marker, &name, dir.exists()),
            block_dirs: vec![dir],
            block_names: vec![],
            verify: Verify::None,
            fallback: None,
            cleanup: vec![zip],
        });
    }

    let target = match ver {
        Some(v) => feed.find(v).cloned().or(match series_releases(&version::series(v)).await {
            Ok(list) => list.into_iter().find(|r| version::cmp(&r.version, v).is_eq()),
            Err(_) => None,
        }),
        None => {
            if id == "blender:older" {
                bail!("Pick a version to install");
            }
            let series = id.trim_start_matches("blender:");
            let series = version::series(series);
            feed.newest_in(&series).cloned()
        }
    }
    .ok_or_else(|| anyhow!("Blender {} is not on download.blender.org", ver.unwrap_or(id)))?;
    let t_series = version::series(&target.version);
    let cur_kind = current.map(|c| kind_of(c).1);
    let same_series = current.is_some_and(|c| kind_of(c).0 == t_series);
    let downgrade = current.is_some_and(|c| version::newer(&c.version, &target.version));

    // MSI when updating an MSI series in place, or a new series into the default folder.
    // Otherwise a portable copy, which always keeps every other copy.
    let msi_in_place = cur_kind == Some(BlenderKind::Msi) && same_series && !downgrade && !ctx.keep_patch_versions && kind != JobKind::Rollback;
    let msi_new_series = !same_series && use_msi_dir && target.msi.is_some() && !downgrade && kind != JobKind::Rollback;
    if (msi_in_place || msi_new_series) && target.msi.is_some() {
        let url = target.msi.clone().unwrap();
        let file = paths::downloads().join("blender").join(url.rsplit('/').next().unwrap_or("blender.msi"));
        let log = paths::logs().join(format!("blender-{}.log", target.version));
        let app_id = format!("blender:{t_series}");
        let sha = sha_for(&target.version, &url).await;
        let dl = Download { url, dest: file.clone(), size: target.msi_size, sha256: sha, md5: None, signer: None, sha512: None, headers: vec![] };
        super::require_checked(&dl, &format!("Blender {}", target.version))?;
        return Ok(Plan {
            app_id: app_id.clone(),
            kind,
            name: format!("Blender {t_series}"),
            to_version: target.version.clone(),
            downloads: vec![dl],
            steps: vec![Step::Run {
                exe: PathBuf::from("msiexec.exe"),
                args: vec!["/i".into(), file.to_string_lossy().into(), "/qn".into(), "/norestart".into(), "MSIFASTINSTALL=7".into(), "/l*".into(), log.to_string_lossy().into()],
                cwd: None,
                ok_codes: vec![0, 3010, 1641],
                elevated: true,
                label: format!("Installing Blender {}", target.version),
                log: Some(log),
            }],
            block_dirs: if msi_in_place { current.and_then(|c| c.install_dir.clone()).into_iter().collect() } else { vec![] },
            block_names: vec![],
            verify: Verify::Version { app_id, version: target.version.clone() },
            fallback: None,
            cleanup: vec![file],
        });
    }

    let url = target.zip.clone().ok_or_else(|| anyhow!("Blender {} has no portable download", target.version))?;
    let name = format!("Blender {}", target.version);
    let dir = ctx.root.join(&name);
    if dir.exists() && !downgrade {
        bail!("{} is already installed in {}", name, dir.display());
    }
    let zip = paths::downloads().join("blender").join(url.rsplit('/').next().unwrap_or("blender.zip"));
    let sha = sha_for(&target.version, &url).await;
    let dl = Download { url, dest: zip.clone(), size: target.zip_size, sha256: sha, md5: None, signer: None, sha512: None, headers: vec![] };
    super::require_checked(&dl, &name)?;
    let marker = Marker { version: target.version.clone(), kind: BlenderKind::Zip, build: None, label: None };
    let mut steps = zip_steps(&zip, &dir, &marker, &name, false);
    steps.extend(replaced_copy_steps(current, same_series && !downgrade, ctx.keep_patch_versions));
    Ok(Plan {
        app_id: format!("blender:{}", target.version),
        kind,
        name: name.clone(),
        to_version: target.version.clone(),
        downloads: vec![dl],
        steps,
        block_dirs: vec![],
        block_names: vec![],
        verify: Verify::Version { app_id: format!("blender:{}", target.version), version: target.version.clone() },
        fallback: None,
        cleanup: vec![zip],
    })
}

/// A portable copy replaced by a newer bug-fix copy of its series goes away when versions are
/// not kept, but only one we unpacked: a copy the user unpacked themselves always stays.
fn replaced_copy_steps(current: Option<&Installed>, same_series_upgrade: bool, keep_patch_versions: bool) -> Vec<Step> {
    let Some(c) = current else { return vec![] };
    let Some(d) = &c.install_dir else { return vec![] };
    if kind_of(c).1 != BlenderKind::Zip || !same_series_upgrade || keep_patch_versions || !ours(Some(d)) {
        return vec![];
    }
    vec![Step::RemoveDir { path: d.clone(), elevated: needs_admin(d), label: format!("Removing {}", c.name) }, Step::RemoveFile { path: shortcut_path(&c.name) }]
}

pub fn plan_uninstall(inst: &Installed) -> Result<Plan> {
    let (_, kind) = kind_of(inst);
    let steps = match kind {
        BlenderKind::Steam => bail!("Blender from Steam is removed in Steam"),
        BlenderKind::Store => bail!("Blender from the Microsoft Store is removed in Windows Settings"),
        BlenderKind::Msi => {
            let u = inst.uninstall.clone().ok_or_else(|| anyhow!("{} has no uninstaller registered", inst.name))?;
            vec![Step::Run { exe: PathBuf::from(&u[0]), args: u[1..].to_vec(), cwd: None, ok_codes: vec![0, 1605, 3010], elevated: true, label: format!("Uninstalling {}", inst.name), log: None }]
        }
        BlenderKind::Zip | BlenderKind::Beta => {
            let d = inst.install_dir.clone().ok_or_else(|| anyhow!("unknown folder"))?;
            vec![
                Step::RemoveDir { path: d.clone(), elevated: needs_admin(&d), label: format!("Removing {}", inst.name) },
                Step::RemoveFile { path: shortcut_path(&inst.name) },
            ]
        }
    };
    Ok(Plan {
        app_id: inst.id.clone(),
        kind: JobKind::Uninstall,
        name: inst.name.clone(),
        to_version: inst.version.clone(),
        downloads: vec![],
        steps,
        block_dirs: inst.install_dir.iter().cloned().collect(),
        block_names: vec![],
        verify: Verify::Removed { app_id: inst.id.clone() },
        fallback: None,
        cleanup: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_release_index() {
        let html = r#"<a href="blender-5.2.2-windows-x64.msi">blender-5.2.2-windows-x64.msi</a>                      21-Oct-2026 23:27           380M
<a href="blender-5.2.2-windows-x64.zip">blender-5.2.2-windows-x64.zip</a>                      21-Oct-2026 23:27      410123456
<a href="Blender5.2/">Blender5.2/</a>                                         21-Oct-2026 23:27                   -"#;
        let v = parse_index(html);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].0, "blender-5.2.2-windows-x64.msi");
        assert_eq!(v[0].2, Some(380 * 1024 * 1024));
        assert_eq!(v[1].2, Some(410123456));
        assert!(v[0].1.as_deref().unwrap().starts_with("2026-10-21T23:27"));
    }

    #[test]
    fn old_series_file_names_and_first_release_notes() {
        assert_eq!(windows_file("blender-5.2.2-windows-x64.msi"), Some(("5.2.2".into(), "msi")));
        // 2.80 to 2.92 are named "windows64".
        assert_eq!(windows_file("blender-2.81-windows64.zip"), Some(("2.81".into(), "zip")));
        assert_eq!(windows_file("blender-2.81-windows32.zip"), None);
        assert_eq!(windows_file("blender-5.2.2-windows-arm64.msi"), None);
        // "2.81" is the first release, read from the notes of "2.81.0".
        let mut notes = HashMap::new();
        notes.insert("2.81.0".to_string(), crate::notes::Notes { date: Some("2019-11-21T00:00:00Z".into()), lines: vec!["# Cycles".into(), "OptiX support".into()], url: None });
        let v = release_info("2.81", &rel("2.81"), &notes, None);
        assert_eq!(v.notes, vec!["# Cycles", "OptiX support"]);
        assert_eq!(v.date.as_deref(), Some("2019-11-21T00:00:00Z"));
        // Fix lists link to the page they came from.
        notes.insert("5.2.2".to_string(), crate::notes::Notes { lines: vec!["A fix".into()], ..Default::default() });
        let v = release_info("5.2", &rel("5.2.2"), &notes, Some("https://www.blender.org/releases/5-2/"));
        assert_eq!(v.notes_url.as_deref(), Some("https://www.blender.org/releases/5-2/"));
        assert!(is_lts("5.2"));
    }

    fn inst(series: &str, ver: &str, kind: BlenderKind) -> Installed {
        Installed {
            id: format!("blender:{series}"),
            source: Source::Blender,
            name: String::new(),
            code: String::new(),
            fg: String::new(),
            bg: String::new(),
            version: ver.into(),
            install_dir: None,
            exe: None,
            uninstall: None,
            updated_at: None,
            size_hint: None,
            tagline: None,
            channel: None,
            missing: false,
            read_only: false,
            store: None,
            icon_remote: None,
            data: SourceData::Blender { series: series.into(), kind, msi_product_code: None },
        }
    }

    fn rel(v: &str) -> Release {
        Release { version: v.into(), date: None, msi: Some(format!("m{v}")), zip: Some(format!("z{v}")), msi_size: Some(1), zip_size: Some(2) }
    }

    fn build(ver: &str, label: &str, hash: &str) -> Beta {
        Beta { version: ver.into(), series: version::series(ver), build: hash.into(), url: String::new(), size: None, date: None, label: label.into() }
    }

    #[test]
    fn microsoft_store_copies_are_listed_and_left_to_the_store() {
        use crate::sources::msstore::Package;
        let pkg = |name: &str, family: &str, ver: &str| Package { name: name.into(), family: family.into(), version: ver.into(), install_dir: None, installed_at: Some("2026-10-01T00:00:00+00:00".into()) };
        let pkgs = vec![
            pkg("BlenderFoundation.Blender", "BlenderFoundation.Blender_ppwjx1n5r4v9t", "5.0.1.0"),
            pkg("BlenderFoundation.Blender4.5LTS", "BlenderFoundation.Blender4.5LTS_ppwjx1n5r4v9t", "4.5.3.0"),
            // Another publisher's package with the same name, and the Foundation's other apps.
            pkg("BlenderFoundation.Blender", "BlenderFoundation.Blender_abcdefghijklm", "9.9.9.0"),
            pkg("BlenderFoundation.Blender-Tools", "BlenderFoundation.Blender-Tools_ppwjx1n5r4v9t", "1.0.0.0"),
            pkg("Microsoft.WindowsCalculator", "Microsoft.WindowsCalculator_8wekyb3d8bbwe", "11.2.0.0"),
        ];
        let v = store_copies(&pkgs);
        let ids: Vec<&str> = v.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, vec!["blender:store", "blender:store:4.5lts"]);
        let lts = &v[1];
        assert_eq!(lts.name, "Blender 4.5 (Microsoft Store)");
        assert_eq!(lts.version, "4.5.3");
        assert_eq!(lts.channel.as_deref(), Some("lts"));
        assert_eq!(lts.tagline.as_deref(), Some("Updated by the Microsoft Store"));
        assert!(lts.read_only && lts.uninstall.is_none() && !lts.missing);
        assert_eq!(lts.store.as_ref().unwrap().page_url(), "ms-windows-store://pdp/?PFN=BlenderFoundation.Blender4.5LTS_ppwjx1n5r4v9t");
        assert_eq!(lts.updated_at.as_deref(), Some("2026-10-01T00:00:00+00:00"));

        // Never offered an update, never removed by us, and they change nothing for the other copies.
        let mut feed = Feed::default();
        feed.series.insert("4.5".into(), vec![rel("4.5.5"), rel("4.5.3")]);
        feed.series.insert("5.0".into(), vec![rel("5.0.2"), rel("5.0.1")]);
        feed.newest_series = Some("5.0".into());
        let msi = inst("4.2", "4.2.9", BlenderKind::Msi);
        let mut all = v.clone();
        all.push(msi.clone());
        assert!(v.iter().all(|i| latest(i, &all, &feed).is_none()));
        assert_eq!(latest(&msi, &all, &feed).map(|l| l.version), latest(&msi, std::slice::from_ref(&msi), &feed).map(|l| l.version));
        assert!(plan_uninstall(lts).is_err());
    }

    #[test]
    fn prerelease_builds_are_picked_by_hash_or_series() {
        let mut feed = Feed::default();
        feed.builds = vec![build("5.4.0", "Alpha", "fbe82a461016"), build("5.3.0", "Beta", "873fa9640966"), build("4.5.15", "Release candidate", "ef05e168f531")];
        feed.beta = Some(feed.builds[1].clone());
        assert_eq!(feed.builds[0].display(), "5.4.0 Alpha fbe82a4");
        // An exact build from the picker.
        assert_eq!(feed.build_for("blender:5.3-beta", Some("5.4.0 Alpha fbe82a4")).unwrap().build, "fbe82a461016");
        // No pick: the newest build of that series.
        assert_eq!(feed.build_for("blender:4.5-beta", None).unwrap().label, "Release candidate");
        // An installed 5.3 beta is offered the newer 5.3 build only, never the 5.4 alpha.
        let mut i = inst("5.3", "5.3.0 Beta 1111111", BlenderKind::Beta);
        i.id = "blender:5.3-beta".into();
        let l = latest(&i, std::slice::from_ref(&i), &feed).unwrap();
        assert_eq!(l.version, "5.3.0 Beta 873fa96");
        i.version = "5.3.0 Beta 873fa96".into();
        assert_eq!(latest(&i, std::slice::from_ref(&i), &feed).unwrap().version, i.version);
    }

    #[test]
    fn the_prerelease_card_stays_while_a_build_is_left_to_install() {
        let mut feed = Feed::default();
        feed.builds = vec![build("5.4.0", "Alpha", "9547657aaaaa"), build("5.3.0", "Beta", "1b53729bbbbb")];
        feed.beta = Some(feed.builds[1].clone());
        let mut beta53 = inst("5.3", "5.3.0 Beta 1b53729", BlenderKind::Beta);
        beta53.id = "blender:5.3-beta".into();
        // 5.3 Beta installed, 5.4 Alpha not: the card is still something to install.
        let card = |inst: &[Installed]| catalog_entries(&feed, inst, true).into_iter().find(|e| e.id.ends_with("-beta")).unwrap();
        assert!(!card(std::slice::from_ref(&beta53)).installed);
        let mut alpha54 = inst("5.4", "5.4.0 Alpha 9547657", BlenderKind::Beta);
        alpha54.id = "blender:5.4-beta".into();
        assert!(card(&[beta53.clone(), alpha54]).installed);
    }

    #[test]
    fn newest_series_is_offered_next_series_side_by_side() {
        let mut feed = Feed::default();
        feed.series.insert("5.2".into(), vec![rel("5.2.2")]);
        feed.series.insert("5.3".into(), vec![rel("5.3.0")]);
        feed.series.insert("4.5".into(), vec![rel("4.5.9"), rel("4.5.5")]);
        feed.newest_series = Some("5.3".into());
        let all = vec![inst("5.2", "5.2.2", BlenderKind::Msi), inst("4.5", "4.5.5", BlenderKind::Msi)];
        let l52 = latest(&all[0], &all, &feed).unwrap();
        assert_eq!(l52.version, "5.3.0");
        assert!(l52.note.unwrap().contains("your 5.2 stays"));
        let l45 = latest(&all[1], &all, &feed).unwrap();
        assert_eq!(l45.version, "4.5.9");
        assert!(l45.note.is_none());
    }

    #[test]
    fn only_our_portable_copies_are_removed_on_update() {
        let root = std::env::temp_dir().join(format!("tu3d-blender-test-{}", std::process::id()));
        let own = root.join("Blender 4.5.1");
        std::fs::create_dir_all(&own).unwrap();
        std::fs::write(own.join("blender.exe"), b"x").unwrap();
        let mut c = inst("4.5", "4.5.1", BlenderKind::Zip);
        c.name = "Blender 4.5.1".into();
        c.install_dir = Some(own.clone());
        let mut feed = Feed::default();
        feed.series.insert("4.5".into(), vec![rel("4.5.2")]);
        // Unpacked by the user (no marker): never removed, and the note says it stays.
        assert!(replaced_copy_steps(Some(&c), true, false).is_empty());
        assert!(latest_kept(&c, std::slice::from_ref(&c), &feed, false).unwrap().note.unwrap().contains("this one stays"));
        // Ours: replaced unless bug-fix versions are kept.
        std::fs::write(own.join(MARKER), b"{}").unwrap();
        let steps = replaced_copy_steps(Some(&c), true, false);
        assert!(matches!(&steps[0], Step::RemoveDir { path, .. } if path == &own));
        assert!(replaced_copy_steps(Some(&c), true, true).is_empty());
        assert!(latest_kept(&c, std::slice::from_ref(&c), &feed, false).unwrap().note.unwrap().contains("replaces this copy"));
        assert!(latest_kept(&c, std::slice::from_ref(&c), &feed, true).unwrap().note.unwrap().contains("this one stays"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn old_releases_have_their_checksums_in_one_file_per_release() {
        assert_eq!(sha_urls("4.2.3"), vec![format!("{RELEASES}Blender4.2/blender-4.2.3.sha256"), format!("{RELEASES}Blender4.2/release423.sha256")]);
        assert_eq!(sha_urls("2.81a")[1], format!("{RELEASES}Blender2.81/release281a.sha256"));
        let h = "e7".repeat(32);
        let text = format!("{}  blender-2.80-windows32.msi
{h}  blender-2.80-windows64.msi
", "6e".repeat(32));
        assert_eq!(sha_in(&text, "blender-2.80-windows64.msi"), Some(h));
        assert_eq!(sha_in(&text, "blender-2.80-windows64.zip"), None);
    }

    #[test]
    fn arm64_pcs_get_native_builds_and_x64_pcs_never_do() {
        let html = r#"<a href="blender-4.5.3-windows-arm64.msi">x</a> 21-Oct-2026 23:27 300M
<a href="blender-4.5.3-windows-x64.msi">x</a> 21-Oct-2026 23:27 380M
<a href="blender-4.5.3-windows-x64.zip">x</a> 21-Oct-2026 23:27 410M
<a href="blender-4.5.3-windows-arm64.msix">x</a> 21-Oct-2026 23:27 300M"#;
        let arm = releases_from_index("u/", html, "arm64");
        assert_eq!(arm[0].msi.as_deref(), Some("u/blender-4.5.3-windows-arm64.msi"));
        // No ARM64 zip: the x64 one runs emulated.
        assert_eq!(arm[0].zip.as_deref(), Some("u/blender-4.5.3-windows-x64.zip"));
        let x64 = releases_from_index("u/", html, "x64");
        assert_eq!(x64[0].msi.as_deref(), Some("u/blender-4.5.3-windows-x64.msi"));
        let b = |arch: &str, t: i64| serde_json::json!({ "platform": "windows", "architecture": arch, "risk_id": "beta", "file_extension": "zip", "version": "5.3.0", "hash": arch, "url": arch, "file_mtime": t });
        let v = serde_json::json!([b("amd64", 20), b("arm64", 10)]);
        assert_eq!(builds_from(&v, "arm64")[0].url, "arm64");
        assert_eq!(builds_from(&v, "x64")[0].url, "amd64");
        assert_eq!(builds_from(&v, "x64").len(), 1);
    }

    #[test]
    fn lts_series_come_from_the_lts_page_too() {
        let html = "<h2>Blender 5.8 LTS</h2><a>Blender 4.5 LTS</a> Blender 5.8 LTS <p>Blender 1.0 LTS</p>";
        assert_eq!(parse_lts(html), vec!["4.5", "5.8"]);
        assert!(!is_lts("5.8"));
        *LTS_SEEN.write().unwrap() = parse_lts(html);
        assert!(is_lts("5.8") && is_lts("2.83"));
        assert_eq!(lts_series().last().map(String::as_str), Some("5.8"));
        LTS_SEEN.write().unwrap().clear();
    }
}
