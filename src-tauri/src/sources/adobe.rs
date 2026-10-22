//! Adobe Creative Cloud apps.
//!
//! Installed apps come from Adobe's own install database (caps\hdpim.db) checked against the
//! files on disk, with the Add/Remove Programs entries for the uninstall commands.
//! Latest versions come from the public Creative Cloud product catalog. Installs download the
//! packages from Adobe's CDN and hand them to Adobe's installer (HDBox Setup.exe) with a driver
//! file, the same way Adobe's admin packages install. If that fails, updates fall back to
//! Adobe's Remote Update Manager.

use super::{Download, Installed, Latest, Plan, SourceData, Step, Verify};
use crate::model::{CatalogEntry, JobKind, Source, VersionInfo};
use crate::{http, paths, version, winutil};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

const CATALOG_URL: &str = "https://cdn-ffc.oobesaas.adobe.com/core/v6/products/all?_type=xml&channel=ccm&channel=sti&platform=win32,win64&productType=Desktop";
const APP_JSON_URL: &str = "https://cdn-ffc.oobesaas.adobe.com/core/v3/applications";
const CDN: &str = "https://ccmdls.adobe.com";
const RUM_URL: &str = "https://deploymenttools.acp.adobeoobe.com/RUM/Win/RemoteUpdateManager.zip";
const APP_ID_HEADER: (&str, &str) = ("x-adobe-app-id", "accc-apps-panel-desktop");
const API_KEY_HEADER: (&str, &str) = ("x-api-key", "CC_HD_ESD_1_0");

#[derive(Clone, Debug)]
pub struct Product {
    pub sap: String,
    pub version: String,
    pub name: String,
    pub channel: String,
    pub platform: String,
    pub build_guid: String,
    pub product_version: String,
    pub base_version: String,
    pub size: Option<u64>,
    pub icon: Option<String>,
    pub tagline: Option<String>,
    pub whats_new: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Catalog {
    pub products: Vec<Product>,
}

impl Catalog {
    pub fn for_sap<'a>(&'a self, sap: &str) -> impl Iterator<Item = &'a Product> + 'a {
        let sap = sap.to_string();
        self.products.iter().filter(move |p| p.sap == sap)
    }
    pub fn is_app(&self, sap: &str) -> bool {
        self.for_sap(sap).any(|p| p.channel == "ccm") || known(sap).is_some()
    }
    pub fn find(&self, sap: &str, ver: &str, platform: &str) -> Option<&Product> {
        self.for_sap(sap)
            .filter(|p| version::cmp(&p.version, ver).is_eq())
            .find(|p| p.platform == platform)
            .or_else(|| self.for_sap(sap).find(|p| version::cmp(&p.version, ver).is_eq()))
    }
    pub fn newest(&self, sap: &str, platform: &str, same_major: Option<u64>) -> Option<&Product> {
        self.for_sap(sap)
            .filter(|p| p.platform == platform || platform.is_empty())
            .filter(|p| same_major.map_or(true, |m| version::major(&p.version) == m))
            .max_by(|a, b| version::cmp(&a.version, &b.version))
    }
    pub fn icon(&self, sap: &str) -> Option<&str> {
        self.for_sap(sap)
            .filter(|p| p.icon.is_some())
            .max_by(|a, b| version::cmp(&a.version, &b.version))
            .and_then(|p| p.icon.as_deref())
    }
}

/// Brand marks and names for Adobe apps, also the offline fallback list of what counts as an app.
pub fn known(sap: &str) -> Option<(&'static str, &'static str, &'static str, &'static str)> {
    // (code, fg, bg, name)
    Some(match sap {
        "PHSP" => ("Ps", "#31A8FF", "#001E36", "Photoshop"),
        "ILST" => ("Ai", "#FF9A00", "#330000", "Illustrator"),
        "AEFT" => ("Ae", "#D291FF", "#1F0040", "After Effects"),
        "PPRO" => ("Pr", "#9999FF", "#00005B", "Premiere Pro"),
        "AME" => ("Me", "#9999FF", "#00005B", "Media Encoder"),
        "AUDT" => ("Au", "#00E4BB", "#00005B", "Audition"),
        "IDSN" => ("Id", "#FF3366", "#49021F", "InDesign"),
        "AICY" => ("Ic", "#FF3366", "#49021F", "InCopy"),
        "KBRG" => ("Br", "#E3B23C", "#2B1A05", "Bridge"),
        "LTRM" => ("Lrc", "#31A8FF", "#001E36", "Lightroom Classic"),
        "LRCC" => ("Lr", "#31A8FF", "#001E36", "Lightroom"),
        "APRO" => ("Ac", "#FF5C5C", "#2A0606", "Acrobat"),
        "FLPR" => ("An", "#FF61F6", "#2A0028", "Animate"),
        "DRWV" => ("Dw", "#FF61F6", "#2A0028", "Dreamweaver"),
        "CHAR" => ("Ch", "#9999FF", "#00005B", "Character Animator"),
        "SBSTP" => ("Pt", "#FF5C6C", "#2A0A10", "Substance 3D Painter"),
        "SBSTD" => ("Ds", "#F0A33A", "#2B1A05", "Substance 3D Designer"),
        "SBSTA" => ("Sa", "#4FD8C4", "#062A26", "Substance 3D Sampler"),
        "STGR" => ("St", "#C08BFF", "#22093D", "Substance 3D Stager"),
        "SHPR" => ("Mo", "#7FB4FF", "#0B1F3A", "Substance 3D Modeler"),
        "ESHR" => ("Dn", "#5EE0C8", "#062A26", "Dimension"),
        "FRSC" => ("Fr", "#FFB45C", "#2B1503", "Fresco"),
        "UXPD" => ("Ux", "#C8C8C8", "#262626", "UXP Developer Tools"),
        _ => return None,
    })
}

/// Catalog entries that are tools, legacy or store apps, never offered.
fn skip_in_catalog(sap: &str) -> bool {
    matches!(
        sap,
        "AAM" | "KCCC" | "FLBR" | "FWKS" | "GSDK" | "KEMN" | "KETK" | "MUSE" | "ASCT" | "TAPI" | "PSXUWP" | "SPRKBE" | "ANML"
    )
}

fn display_name(sap: &str, catalog_name: &str) -> String {
    if let Some((_, _, _, n)) = known(sap) {
        return n.to_string();
    }
    let n = catalog_name.trim_start_matches("Adobe ").to_string();
    if n.is_empty() { sap.to_string() } else { n }
}

fn mark_for(sap: &str, name: &str) -> (String, String, String) {
    if let Some((c, fg, bg, _)) = known(sap) {
        return (c.into(), fg.into(), bg.into());
    }
    let letters: String = name.split_whitespace().filter_map(|w| w.chars().next()).take(2).collect();
    (if letters.is_empty() { sap[..2.min(sap.len())].to_string() } else { letters }, "#D0D0D0".into(), "#2A2A2A".into())
}

// ---------------------------------------------------------------- catalog

pub async fn fetch_catalog() -> Result<Catalog> {
    let f = http::get_cached(CATALOG_URL, &[APP_ID_HEADER], Duration::from_secs(20 * 60)).await?;
    tokio::task::spawn_blocking(move || parse_catalog(&f.body)).await?
}

fn parse_catalog(body: &[u8]) -> Result<Catalog> {
    let text = std::str::from_utf8(body).context("catalog is not UTF-8")?;
    let doc = roxmltree::Document::parse_with_options(text, roxmltree::ParsingOptions { allow_dtd: true, ..Default::default() })?;
    let mut products = Vec::new();
    for ch in doc.descendants().filter(|n| n.has_tag_name("channel")) {
        let channel = ch.attribute("name").unwrap_or("").to_string();
        for p in ch.descendants().filter(|n| n.has_tag_name("product")) {
            let sap = p.attribute("id").unwrap_or("").to_string();
            let ver = p.attribute("version").unwrap_or("").to_string();
            if sap.is_empty() || ver.is_empty() {
                continue;
            }
            let child_text = |tag: &str| p.children().find(|c| c.has_tag_name(tag)).and_then(|c| c.text()).map(|s| s.to_string());
            let name = child_text("displayName").unwrap_or_else(|| sap.clone());
            let mut icon: Option<(u32, String)> = None;
            if let Some(icons) = p.children().find(|c| c.has_tag_name("productIcons")) {
                for i in icons.children().filter(|c| c.has_tag_name("icon")) {
                    let size: u32 = i.attribute("size").and_then(|s| s.split('x').next()).and_then(|s| s.parse().ok()).unwrap_or(0);
                    if let Some(url) = i.text() {
                        // Prefer 192, else the largest.
                        let better = match &icon {
                            None => true,
                            Some((s, _)) => (size == 192) || (*s != 192 && size > *s),
                        };
                        if better {
                            icon = Some((size, url.to_string()));
                        }
                    }
                }
            }
            let mut tagline = None;
            let mut whats_new = None;
            if let Some(cd) = p.children().find(|c| c.has_tag_name("custom-data")) {
                for e in cd.children().filter(|c| c.has_tag_name("custom-entry")) {
                    let key = e.attribute("key").unwrap_or("");
                    let val = e.descendants().find(|c| c.has_tag_name("value")).and_then(|c| c.text()).unwrap_or("");
                    if key == "productDescriptionTagline" {
                        tagline = serde_json::from_str::<Value>(val).ok().and_then(|v| {
                            v["Language"].as_array()?.iter().find(|l| l["locale"] == "en_US").and_then(|l| l["value"].as_str()).map(String::from)
                        });
                    } else if key == "updateDescriptionURL" {
                        // Release notes page, preferred over the "what's new" page.
                        let url = serde_json::from_str::<Value>(val).ok().and_then(|v| {
                            v["Language"].as_array()?.iter().find(|l| l["locale"] == "en_US").and_then(|l| l["value"].as_str()).map(String::from)
                        });
                        if let Some(u) = url.filter(|u| u.starts_with("http")) {
                            whats_new = Some(u);
                        }
                    } else if key == "prodWhatsNewPage" && val.starts_with("http") && whats_new.is_none() {
                        whats_new = Some(val.to_string());
                    }
                }
            }
            for pl in p.descendants().filter(|n| n.has_tag_name("platform")) {
                let platform = pl.attribute("id").unwrap_or("").to_string();
                if platform != "win64" && platform != "win32" {
                    continue;
                }
                let Some(ls) = pl.descendants().find(|n| n.has_tag_name("languageSet")) else { continue };
                let size = ls
                    .descendants()
                    .find(|n| n.has_tag_name("esdData"))
                    .and_then(|e| e.children().find(|c| c.has_tag_name("size")))
                    .and_then(|s| s.text())
                    .and_then(|s| s.parse().ok());
                products.push(Product {
                    sap: sap.clone(),
                    version: ver.clone(),
                    name: name.clone(),
                    channel: channel.clone(),
                    platform,
                    build_guid: ls.attribute("buildGuid").unwrap_or("").to_string(),
                    product_version: ls.attribute("productVersion").unwrap_or("").to_string(),
                    base_version: ls.attribute("baseVersion").unwrap_or("").to_string(),
                    size,
                    icon: icon.as_ref().map(|(_, u)| u.clone()),
                    tagline: tagline.clone(),
                    whats_new: whats_new.clone(),
                });
            }
        }
    }
    if products.is_empty() {
        bail!("The Adobe catalog was empty");
    }
    Ok(Catalog { products })
}

// ---------------------------------------------------------------- installed apps

pub fn hdbox_dir() -> PathBuf {
    paths::program_files_x86().join("Common Files\\Adobe\\Adobe Desktop Common\\HDBox")
}

fn caps_db() -> PathBuf {
    paths::program_files_x86().join("Common Files\\Adobe\\caps\\hdpim.db")
}

#[derive(Clone, Debug, Default)]
pub struct DbProduct {
    pub sap: String,
    pub version: String,
    pub platform: String,
    pub meta: HashMap<String, String>,
}

/// Products in Adobe's install database plus `LatestInstalledVersion` by (sap, base version).
pub fn read_db() -> (Vec<DbProduct>, HashMap<(String, String), String>) {
    let src = caps_db();
    if !src.exists() {
        return (vec![], HashMap::new());
    }
    // Read a private copy so we never hold a lock on Adobe's database.
    let copy = paths::data_dir().join("hdpim-copy.db");
    for (s, d) in [("", ""), ("-wal", "-wal"), ("-shm", "-shm")] {
        let from = PathBuf::from(format!("{}{s}", src.display()));
        let to = PathBuf::from(format!("{}{d}", copy.display()));
        if from.exists() {
            let _ = std::fs::copy(&from, &to);
        } else {
            let _ = std::fs::remove_file(&to);
        }
    }
    let mut products = Vec::new();
    let mut latest = HashMap::new();
    let Ok(conn) = rusqlite::Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY) else {
        return (products, latest);
    };
    if let Ok(mut st) = conn.prepare("SELECT SAPCode, ProductVersion, ProcessorFamily, Status FROM product_installation_info") {
        let rows = st.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2).unwrap_or_default(), r.get::<_, i64>(3).unwrap_or(1)))
        });
        if let Ok(rows) = rows {
            for (sap, ver, fam, status) in rows.flatten() {
                if status != 1 {
                    continue;
                }
                products.push(DbProduct { sap, version: ver, platform: fam, meta: HashMap::new() });
            }
        }
    }
    if let Ok(mut st) = conn.prepare("SELECT SAPCode, ProductVersion, Key, Value FROM product_installation_meta_info") {
        if let Ok(rows) = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?, r.get::<_, String>(3).unwrap_or_default()))) {
            for (sap, ver, k, v) in rows.flatten() {
                if k == "LatestInstalledVersion" {
                    latest.insert((sap.clone(), ver.clone()), v.clone());
                }
                if let Some(p) = products.iter_mut().find(|p| p.sap == sap && p.version == ver) {
                    p.meta.insert(k, v);
                }
            }
        }
    }
    (products, latest)
}

pub fn scan(catalog: Option<&Catalog>, uninstall: &[winutil::UninstallEntry]) -> Vec<Installed> {
    let (db, _) = read_db();
    let mut out: Vec<Installed> = Vec::new();
    // Uninstall entries keyed by (sap, productVersion).
    let mut un: HashMap<(String, String), &winutil::UninstallEntry> = HashMap::new();
    for e in uninstall {
        if !e.uninstall_string.contains("--sapCode=") {
            continue;
        }
        let args = winutil::split_command_line(&e.uninstall_string);
        let get = |k: &str| args.iter().find_map(|a| a.strip_prefix(&format!("--{k}="))).map(String::from).unwrap_or_default();
        un.insert((get("sapCode"), get("productVersion")), e);
    }

    for p in &db {
        let is_app = match catalog {
            Some(c) => c.is_app(&p.sap),
            None => known(&p.sap).is_some(),
        };
        if !is_app {
            continue;
        }
        let install_dir = p.meta.get("InstallDir").map(|s| winutil::pathbuf(s));
        let exe = match (p.meta.get("AppLaunch"), &install_dir) {
            (Some(l), Some(dir)) if !l.is_empty() => {
                Some(PathBuf::from(l.replace("[INSTALLDIR]", &dir.to_string_lossy()).replace("[installdir]", &dir.to_string_lossy())))
            }
            _ => None,
        };
        let missing = match (&exe, &install_dir) {
            (Some(e), _) => !e.exists(),
            (None, Some(d)) => !d.exists(),
            _ => false,
        };
        let entry = un.get(&(p.sap.clone(), p.version.clone())).copied();
        let raw_name = p.meta.get("Name").cloned().or_else(|| entry.map(|e| e.display_name.clone())).unwrap_or_default();
        let mut name = display_name(&p.sap, &raw_name);
        // Year releases (Photoshop 2026) show the year from the install folder.
        if let Some(dir) = &install_dir {
            let folder = dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
            if let Some(year) = folder.split_whitespace().last().filter(|y| y.len() == 4 && y.starts_with("20") && y.chars().all(|c| c.is_ascii_digit())) {
                if !name.ends_with(year) {
                    name = format!("{name} {year}");
                }
            }
        }
        let (code, fg, bg) = mark_for(&p.sap, &name);
        let platform = match p.meta.get("Platform").map(String::as_str) {
            Some(x) if !x.is_empty() => x.to_string(),
            _ => if p.platform.contains("64") { "win64".into() } else { "win32".into() },
        };
        let updated_at = entry
            .and_then(|e| winutil::install_date_iso(&e.install_date))
            .or_else(|| exe.as_ref().and_then(|e| e.metadata().ok()).and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()));
        let tagline = catalog.and_then(|c| c.for_sap(&p.sap).find_map(|x| x.tagline.clone()));
        out.push(Installed {
            id: format!("adobe:{}:{}", p.sap, version::major(&p.version)),
            source: Source::Adobe,
            name,
            code,
            fg,
            bg,
            version: p.version.clone(),
            install_dir: install_dir.clone(),
            exe,
            uninstall: entry.map(|e| winutil::split_command_line(&e.uninstall_string)),
            updated_at,
            size_hint: entry.map(|e| e.estimated_size_kb * 1024).filter(|s| *s > 0),
            tagline,
            channel: None,
            missing,
            read_only: false,
            icon_remote: catalog.and_then(|c| c.icon(&p.sap)).map(String::from),
            data: SourceData::Adobe {
                sap: p.sap.clone(),
                codex: p.version.clone(),
                platform,
                product_name: raw_name,
            },
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn sap_of(i: &Installed) -> (&str, &str) {
    match &i.data {
        SourceData::Adobe { sap, platform, .. } => (sap, platform),
        _ => ("", ""),
    }
}

/// Newest version for an installed app. The newest installed release line of a product is
/// offered the newest version overall; older lines only get updates within their own major.
pub fn latest(inst: &Installed, all: &[Installed], cat: &Catalog) -> Option<Latest> {
    let (sap, platform) = sap_of(inst);
    let my_major = version::major(&inst.version);
    let newest_installed_major = all
        .iter()
        .filter(|o| sap_of(o).0 == sap)
        .map(|o| version::major(&o.version))
        .max()
        .unwrap_or(my_major);
    let pick = if my_major >= newest_installed_major { cat.newest(sap, platform, None) } else { cat.newest(sap, platform, Some(my_major)) }?;
    let mut note = None;
    if version::major(&pick.version) != my_major {
        note = Some(format!("{} installs as a new app next to this one", pick.version));
    }
    Some(Latest { version: pick.version.clone(), size: pick.size, note })
}

pub fn catalog_entries(cat: &Catalog, installed: &[Installed]) -> Vec<CatalogEntry> {
    let mut by_sap: HashMap<&str, &Product> = HashMap::new();
    for p in cat.products.iter().filter(|p| p.channel == "ccm" && p.platform == "win64") {
        if skip_in_catalog(&p.sap) {
            continue;
        }
        let e = by_sap.entry(&p.sap).or_insert(p);
        if version::newer(&p.version, &e.version) {
            *e = p;
        }
    }
    let mut out: Vec<CatalogEntry> = by_sap
        .into_values()
        .filter(|p| !installed.iter().any(|i| sap_of(i).0 == p.sap))
        .map(|p| {
            let name = display_name(&p.sap, &p.name);
            let (code, fg, bg) = mark_for(&p.sap, &name);
            let prerelease = p.sap.ends_with("BETA") || p.sap == "ILSTPR" || name.contains("(Beta)") || name.contains("Prerelease") || name.contains("(Preview");
            CatalogEntry {
                id: format!("adobe:{}:{}", p.sap, version::major(&p.version)),
                source: Source::Adobe,
                name,
                code,
                fg,
                bg,
                icon_url: p.icon.clone(),
                version: p.version.clone(),
                size: p.size,
                tagline: p.tagline.clone(),
                group: Source::Adobe.group().into(),
                prerelease,
                installed: false,
                pick_version: true,
            }
        })
        .collect();
    out.sort_by(|a, b| a.prerelease.cmp(&b.prerelease).then(a.name.cmp(&b.name)));
    out
}

/// Release notes on Adobe Experience League, which answers apps (adobe.com short links and
/// helpx.adobe.com reject anything that is not a browser, so those only open in the browser).
fn release_notes_url(sap: &str) -> Option<String> {
    let slug = match sap {
        "SBSTP" => "substance-3d-painter",
        "SBSTD" => "substance-3d-designer",
        "SBSTA" => "substance-3d-sampler",
        "STGR" => "substance-3d-stager",
        "SHPR" => "substance-3d-modeler",
        _ => return None,
    };
    Some(format!("https://experienceleague.adobe.com/en/docs/{slug}/using/release-notes/release-notes"))
}

pub fn versions(inst_sap: &str, platform: &str, installed_version: Option<&str>, cat: &Catalog) -> Vec<VersionInfo> {
    let mut list: Vec<&Product> = cat.for_sap(inst_sap).filter(|p| p.platform == platform).collect();
    list.sort_by(|a, b| version::cmp(&b.version, &a.version));
    list.dedup_by(|a, b| version::cmp(&a.version, &b.version).is_eq());
    let newest = list.first().map(|p| p.version.clone());
    list.into_iter()
        .take(25)
        .map(|p| VersionInfo {
            version: p.version.clone(),
            date: None,
            notes: vec![],
            notes_url: release_notes_url(inst_sap).or_else(|| p.whats_new.clone()),
            size: p.size,
            installed: installed_version.is_some_and(|v| version::cmp(v, &p.version).is_eq()),
            latest: newest.as_deref() == Some(p.version.as_str()),
            prerelease: false,
        })
        .collect()
}

// ---------------------------------------------------------------- Creative Cloud status

pub struct CcStatus {
    pub installed: bool,
    pub running: bool,
    pub signed_in: bool,
}

pub fn cc_status(procs: &[crate::procs::Proc]) -> CcStatus {
    let acc = paths::program_files_x86().join("Adobe\\Adobe Creative Cloud\\ACC\\Creative Cloud.exe");
    let installed = acc.exists() || hdbox_dir().join("Setup.exe").exists();
    let running = procs.iter().any(|p| {
        let n = p.name.to_ascii_lowercase();
        n == "adobe desktop service.exe" || n == "creative cloud.exe" || n == "adobeipcbroker.exe"
    });
    let signed_in = signed_in();
    CcStatus { installed, running, signed_in }
}

/// Creative Cloud keeps signed-in users in OOBE\opm.db (encrypted values, we only count rows).
fn signed_in() -> bool {
    let db = paths::local_app_data().join("Adobe\\OOBE\\opm.db");
    if !db.exists() {
        return false;
    }
    let copy = paths::data_dir().join("opm-copy.db");
    if std::fs::copy(&db, &copy).is_err() {
        return true;
    }
    let n: i64 = rusqlite::Connection::open_with_flags(&copy, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|c| c.query_row("SELECT count(*) FROM opm_data WHERE domain='IMSLib' AND subDomain='SignInUsers'", [], |r| r.get(0)))
        .unwrap_or(1);
    let _ = std::fs::remove_file(&copy);
    n > 0
}

pub fn open_creative_cloud() -> Result<()> {
    let acc = paths::program_files_x86().join("Adobe\\Adobe Creative Cloud\\ACC\\Creative Cloud.exe");
    if acc.exists() {
        return winutil::launch(&acc, &[]);
    }
    winutil::shell_open("https://creativecloud.adobe.com/apps/download/creative-cloud")
}

// ---------------------------------------------------------------- install plans

async fn app_json(p: &Product) -> Result<Value> {
    let url = format!("{APP_JSON_URL}?name={}&version={}&platform={}", p.sap, p.version, p.platform);
    let f = http::get_cached(&url, &[APP_ID_HEADER, API_KEY_HEADER, ("x-adobe-build-guid", &p.build_guid)], Duration::from_secs(6 * 3600)).await?;
    let v: Value = serde_json::from_slice(&f.body).with_context(|| format!("Adobe sent an unreadable manifest for {} {}", p.sap, p.version))?;
    if v.get("Packages").is_none() {
        bail!("Adobe has no packages for {} {}", p.name, p.version);
    }
    Ok(v)
}

fn as_list(v: &Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a.clone(),
        Value::Null => vec![],
        other => vec![other.clone()],
    }
}

/// Decide if a package's condition holds on this PC. Unknown conditions keep the package.
fn condition_ok(cond: &str, lang: &str) -> bool {
    let c = cond.trim();
    if c.is_empty() {
        return true;
    }
    // Conditions are joined with && or ||; handle the common forms.
    if c.contains("||") {
        return c.split("||").any(|p| condition_ok(p, lang));
    }
    if c.contains("&&") {
        return c.split("&&").all(|p| condition_ok(p, lang));
    }
    let c = c.trim_matches(|ch| ch == '(' || ch == ')' || ch == ' ');
    if let Some(rest) = c.strip_prefix("[installLanguage]") {
        let rest = rest.trim();
        if let Some(v) = rest.strip_prefix("==") {
            return v.trim().eq_ignore_ascii_case(lang);
        }
        if let Some(v) = rest.strip_prefix("!=") {
            return !v.trim().eq_ignore_ascii_case(lang);
        }
        return true;
    }
    if let Some(rest) = c.strip_prefix("[OSArchitecture]") {
        let arch = if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" };
        if let Some(v) = rest.trim().strip_prefix("==") {
            return v.trim().eq_ignore_ascii_case(arch) || (arch == "x64" && v.trim() == "64");
        }
        if let Some(v) = rest.trim().strip_prefix("!=") {
            return !v.trim().eq_ignore_ascii_case(arch);
        }
    }
    true
}

struct Prepared {
    sap: String,
    base_version: String,
    downloads: Vec<Download>,
    conflicting: Vec<String>,
}

/// Download list for one product into `<root>/products/<SAP>/`, and the filtered application.json.
async fn prepare(p: &Product, root: &Path, lang: &str) -> Result<Prepared> {
    let mut json = app_json(p).await?;
    let dir = root.join("products").join(&p.sap);
    std::fs::create_dir_all(&dir)?;
    let pkgs = as_list(&json["Packages"]["Package"]);
    let mut keep = Vec::new();
    let mut downloads = Vec::new();
    for pkg in pkgs {
        let cond = pkg["Condition"].as_str().unwrap_or("");
        if !condition_ok(cond, lang) {
            continue;
        }
        let path = pkg["Path"].as_str().unwrap_or("");
        let file = pkg["fullPackageName"].as_str().map(String::from).unwrap_or_else(|| format!("{}.zip", pkg["PackageName"].as_str().unwrap_or("package")));
        if path.is_empty() {
            continue;
        }
        let cdn = json["Cdn"]["Secure"].as_str().unwrap_or(CDN).trim_end_matches('/').to_string();
        downloads.push(Download {
            url: format!("{cdn}{path}"),
            dest: dir.join(&file),
            size: pkg["DownloadSize"].as_u64(),
            sha256: None,
            headers: vec![],
        });
        keep.push(pkg);
    }
    if downloads.is_empty() {
        bail!("No packages of {} {} fit this PC", p.name, p.version);
    }
    json["Packages"]["Package"] = Value::Array(keep);
    std::fs::write(dir.join("application.json"), serde_json::to_vec(&json)?)?;
    let mut conflicting = Vec::new();
    for c in as_list(&json["ConflictingProcesses"]["ConflictingProcess"]) {
        if let Some(rp) = c["RelativePath"].as_str() {
            if let Some(name) = rp.rsplit('\\').next() {
                if c["headless"].as_bool() != Some(true) {
                    conflicting.push(name.to_ascii_lowercase());
                }
            }
        }
    }
    Ok(Prepared { sap: p.sap.clone(), base_version: json["BaseVersion"].as_str().unwrap_or(&p.base_version).to_string(), downloads, conflicting })
}

/// Download size of a product (all packages for this language), from its manifest.
pub async fn download_size(p: &Product, lang: &str) -> Option<u64> {
    let json = app_json(p).await.ok()?;
    let total: u64 = as_list(&json["Packages"]["Package"])
        .iter()
        .filter(|pkg| condition_ok(pkg["Condition"].as_str().unwrap_or(""), lang))
        .filter_map(|pkg| pkg["DownloadSize"].as_u64())
        .sum();
    (total > 0).then_some(total)
}

pub struct InstallRequest<'a> {
    pub sap: &'a str,
    pub version: &'a str,
    pub platform: &'a str,
    pub kind: JobKind,
    pub app_id: String,
    pub name: String,
    /// Existing install of the same product (for update in place and for keep-previous logic).
    pub current: Option<&'a Installed>,
    /// Older release line to remove after success (keep previous versions is off).
    pub remove_after: Option<&'a Installed>,
    pub lang: &'a str,
    pub install_root: PathBuf,
}

pub async fn plan_install(req: InstallRequest<'_>, cat: &Catalog) -> Result<Plan> {
    let product = cat
        .find(req.sap, req.version, req.platform)
        .cloned()
        .ok_or_else(|| anyhow!("Adobe does not offer {} {} any more", req.name, req.version))?;
    let root = paths::downloads().join("adobe").join(format!("{}-{}-{}", product.sap, product.version, product.platform));
    std::fs::create_dir_all(root.join("products"))?;
    let main = prepare(&product, &root, req.lang).await?;

    // Shared components the app needs (Camera Raw, colour profiles, CoreSync...). Only fetch
    // the ones that are missing or older than what the catalog has for that base version.
    let (_, latest_installed) = read_db();
    let deps_json = app_json(&product).await?;
    let mut deps = Vec::new();
    for d in as_list(&deps_json["Dependencies"]["Dependency"]) {
        let (Some(dsap), Some(dbase)) = (d["SAPCode"].as_str(), d["BaseVersion"].as_str()) else { continue };
        let candidate = cat
            .for_sap(dsap)
            .filter(|p| p.base_version == dbase)
            .filter(|p| p.platform == product.platform || p.platform == "win32" || p.platform == "win64")
            .max_by(|a, b| version::cmp(&a.version, &b.version).then((a.platform == product.platform).cmp(&(b.platform == product.platform))))
            .or_else(|| cat.for_sap(dsap).max_by(|a, b| version::cmp(&a.version, &b.version)));
        let Some(candidate) = candidate.cloned() else { continue };
        let have = latest_installed.get(&(dsap.to_string(), dbase.to_string()));
        if let Some(h) = have {
            if !version::newer(&candidate.version, h) {
                continue;
            }
        }
        match prepare(&candidate, &root, req.lang).await {
            Ok(p) => deps.push(p),
            Err(e) => log::warn!("dependency {dsap}: {e}"),
        }
    }

    let mut driver = String::from("<DriverInfo>\n    <ProductInfo>\n");
    driver += &format!(
        "        <Name>Adobe {}</Name>\n        <SAPCode>{}</SAPCode>\n        <CodexVersion>{}</CodexVersion>\n        <Platform>{}</Platform>\n        <EsdDirectory>./{}</EsdDirectory>\n        <Dependencies>\n",
        xml_escape(&product.name),
        product.sap,
        product.version,
        product.platform,
        product.sap
    );
    for d in &deps {
        driver += &format!(
            "            <Dependency>\n                <SAPCode>{}</SAPCode>\n                <BaseVersion>{}</BaseVersion>\n                <EsdDirectory>./{}</EsdDirectory>\n            </Dependency>\n",
            d.sap, d.base_version, d.sap
        );
    }
    driver += "        </Dependencies>\n    </ProductInfo>\n    <RequestInfo>\n";
    driver += &format!(
        "        <InstallDir>{}</InstallDir>\n        <InstallLanguage>{}</InstallLanguage>\n    </RequestInfo>\n</DriverInfo>\n",
        xml_escape(&req.install_root.to_string_lossy()),
        req.lang
    );
    let driver_path = root.join("products").join("driver.xml");
    std::fs::write(&driver_path, driver)?;

    let mut downloads = main.downloads;
    for d in deps {
        downloads.extend(d.downloads);
    }

    let setup = hdbox_dir().join("Setup.exe");
    if !setup.exists() {
        bail!("Adobe's installer is missing. Install or repair Creative Cloud first.");
    }
    let log = paths::logs().join(format!("adobe-{}-{}.log", product.sap, product.version));
    let mut steps = vec![Step::Run {
        exe: setup,
        args: vec!["--install=1".into(), "--mode=silent".into(), format!("--driverXML={}", driver_path.display())],
        cwd: Some(root.join("products")),
        ok_codes: vec![0],
        elevated: true,
        label: format!("Installing {}", req.name),
        log: Some(log),
    }];
    if let Some(old) = req.remove_after {
        if let Some(u) = &old.uninstall {
            steps.push(Step::Run {
                exe: PathBuf::from(&u[0]),
                args: u[1..].to_vec(),
                cwd: None,
                ok_codes: vec![0],
                elevated: true,
                label: format!("Removing {} {}", old.name, old.version),
                log: None,
            });
        }
    }

    let mut block_dirs = Vec::new();
    if let Some(c) = req.current {
        if let Some(d) = &c.install_dir {
            block_dirs.push(d.clone());
        }
    }
    let new_id = format!("adobe:{}:{}", product.sap, version::major(&product.version));
    let fallback = if matches!(req.kind, JobKind::Update) && req.current.is_some() {
        Some(Box::new(plan_rum(req.sap, &req.app_id, &req.name, &product.version, block_dirs.clone())))
    } else {
        None
    };
    Ok(Plan {
        app_id: req.app_id,
        kind: req.kind,
        name: req.name,
        to_version: product.version.clone(),
        downloads,
        steps,
        block_dirs,
        block_names: main.conflicting,
        verify: Verify::Version { app_id: new_id, version: product.version.clone() },
        fallback,
        cleanup: vec![root],
    })
}

/// Update through Adobe's Remote Update Manager (official admin tool, downloads by itself).
pub fn plan_rum(sap: &str, app_id: &str, name: &str, to_version: &str, block_dirs: Vec<PathBuf>) -> Plan {
    let dir = paths::downloads().join("rum");
    let zip = dir.join("RemoteUpdateManager.zip");
    let exe_dir = dir.join("bin");
    Plan {
        app_id: app_id.to_string(),
        kind: JobKind::Update,
        name: name.to_string(),
        to_version: to_version.to_string(),
        downloads: vec![Download { url: RUM_URL.into(), dest: zip.clone(), size: None, sha256: None, headers: vec![] }],
        steps: vec![
            Step::ExtractZip { zip, dest: exe_dir.clone(), strip_root: false, elevated: false, label: "Preparing Adobe Remote Update Manager".into() },
            Step::Run {
                exe: exe_dir.join("RemoteUpdateManager.exe"),
                args: vec!["--action=install".into(), format!("--productVersions={sap}")],
                cwd: Some(exe_dir),
                ok_codes: vec![0],
                elevated: true,
                label: format!("Updating {name} with Adobe Remote Update Manager"),
                log: None,
            },
        ],
        block_dirs,
        block_names: vec![],
        verify: Verify::Version { app_id: app_id.to_string(), version: to_version.to_string() },
        fallback: None,
        cleanup: vec![],
    }
}

pub fn plan_uninstall(inst: &Installed) -> Result<Plan> {
    let u = inst.uninstall.clone().ok_or_else(|| anyhow!("{} has no uninstaller registered", inst.name))?;
    Ok(Plan {
        app_id: inst.id.clone(),
        kind: JobKind::Uninstall,
        name: inst.name.clone(),
        to_version: inst.version.clone(),
        downloads: vec![],
        steps: vec![Step::Run {
            exe: PathBuf::from(&u[0]),
            args: u[1..].to_vec(),
            cwd: None,
            ok_codes: vec![0],
            elevated: true,
            label: format!("Uninstalling {}", inst.name),
            log: None,
        }],
        block_dirs: inst.install_dir.iter().cloned().collect(),
        block_names: vec![],
        verify: Verify::Removed { app_id: inst.id.clone() },
        fallback: None,
        cleanup: vec![],
    })
}

/// Hand an install to the Creative Cloud app (used when direct install is not possible).
pub fn cc_uri(sap: &str, version: &str) -> String {
    format!("aam://adobecreativecloud/?SAPCode={sap}&version={version}")
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditions() {
        assert!(condition_ok("", "en_US"));
        assert!(condition_ok("[installLanguage]==en_US", "en_US"));
        assert!(!condition_ok("[installLanguage]==de_DE", "en_US"));
        assert!(condition_ok("[installLanguage]==de_DE || [installLanguage]==en_US", "en_US"));
        assert!(condition_ok("[OSVersion]>=10.0", "en_US"));
        assert!(condition_ok("[OSArchitecture]==x64", "en_US"));
    }

    #[test]
    fn catalog_parses_minimal_xml() {
        let xml = r#"<?xml version='1.0'?><response><channels><channel name="ccm"><products>
        <product version="12.2.0" id="SBSTP"><displayName>Substance 3D Painter</displayName>
        <productIcons><icon size="96x96">https://x/96.png</icon><icon size="192x192">https://x/192.png</icon></productIcons>
        <platforms><platform id="win64"><languageSet buildGuid="g" productVersion="12.2.0.5909" baseVersion="7.2.0"><esdData><size>2713516793</size></esdData></languageSet></platform></platforms>
        </product></products></channel></channels></response>"#;
        let c = parse_catalog(xml.as_bytes()).unwrap();
        assert_eq!(c.products.len(), 1);
        let p = &c.products[0];
        assert_eq!(p.icon.as_deref(), Some("https://x/192.png"));
        assert_eq!(p.size, Some(2713516793));
        assert_eq!(p.base_version, "7.2.0");
        assert_eq!(c.newest("SBSTP", "win64", None).unwrap().version, "12.2.0");
    }
}
