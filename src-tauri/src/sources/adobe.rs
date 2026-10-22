//! Adobe Creative Cloud apps.
//!
//! Installed apps come from Adobe's own install database (caps\hdpim.db) checked against the
//! files on disk, with the Add/Remove Programs entries for the uninstall commands.
//! Latest versions come from the public Creative Cloud product catalog. Adobe's package server
//! only serves Adobe's own signed-in tools, so updates run through Adobe's Remote Update Manager
//! (the official admin updater) and new apps or other versions are handed to Creative Cloud
//! with its install link. Both keep Adobe's sign-in and licensing in charge.

use super::{Download, Installed, Latest, Plan, SourceData, Step, Verify};
use crate::model::{CatalogEntry, JobKind, Source, VersionInfo};
use crate::{http, paths, version, winutil};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

const CATALOG_URL: &str = "https://cdn-ffc.oobesaas.adobe.com/core/v6/products/all?_type=xml&channel=ccm&channel=sti&platform=win32,win64&productType=Desktop";
const APP_JSON_URL: &str = "https://cdn-ffc.oobesaas.adobe.com/core/v3/applications";
/// Creative Cloud's own update service: with no version it answers the newest full build
/// ("6.11.0.478.6"); the product catalog only says "6.11.0".
const CC_SERVICE_URL: &str = "https://cdn-ffc.oobesaas.adobe.com/core/v1/applications?name=CreativeCloud&platform=win64";
/// Adobe's offline Creative Cloud installers (listed on Adobe's "direct links" help page). The small
/// web setup only wakes the installed Creative Cloud, which then waits for a click on Update.
const CC_OFFLINE_BASE: &str = "https://ccmdls.adobe.com/AdobeProducts/StandaloneBuilds/ACCC/ESD";
const CC_NOTES_URL: &str = "https://helpx.adobe.com/creative-cloud/apps/whats-new/release-notes.html";
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
        "KCCC" => ("Cc", "#FF5A5A", "#2A0A0A", "Creative Cloud"),
        "SBSTD" => ("Ds", "#F0A33A", "#2B1A05", "Substance 3D Designer"),
        "SBSTA" => ("Sa", "#4FD8C4", "#062A26", "Substance 3D Sampler"),
        "STGR" => ("St", "#C08BFF", "#22093D", "Substance 3D Stager"),
        "SHPR" => ("Mo", "#7FB4FF", "#0B1F3A", "Substance 3D Modeler"),
        "ESHR" => ("Dn", "#5EE0C8", "#062A26", "Dimension"),
        "FRSC" => ("Fr", "#FFB45C", "#2B1503", "Fresco"),
        "UXPD" => ("Ux", "#C8C8C8", "#262626", "UXP Developer Tools"),
        // Shared components (catalog channel "sti") that Creative Cloud keeps updated too.
        "ACR" => ("Cr", "#6EB4FF", "#0C1D33", "Camera Raw"),
        "LIBS" => ("Lb", "#FF8A8A", "#2A0A0A", "Creative Cloud Libraries"),
        "COSY" => ("Sy", "#C8C8C8", "#262626", "CoreSync"),
        "CCXP" => ("Cx", "#C8C8C8", "#262626", "CCX Process"),
        "SEPS" => ("3D", "#FF5C6C", "#2A0A10", "Substance 3D Viewer for Photoshop"),
        "UXPW" => ("Ux", "#C8C8C8", "#262626", "UXP WebView Support"),
        _ => return None,
    })
}

/// A shared component rather than an app: it updates like an app but is not offered for
/// uninstalling (Creative Cloud and the apps rely on it).
pub fn is_component(sap: &str) -> bool {
    // Not the Notification Client (KANC): the catalog stops at an older version than Creative
    // Cloud installs, so there is nothing to compare or update with.
    matches!(sap, "ACR" | "LIBS" | "COSY" | "CCXP" | "SEPS" | "UXPW")
}

/// Process names a component's update must wait for, from Adobe's own install record
/// (`ConflictingProcessesXML`): "^[Pp][Hh][Oo][Tt][Oo][Ss][Hh][Oo][Pp].[Ee][Xx][Ee]$" is Photoshop.exe.
/// Processes Adobe's installer may stop by itself (`forceKillAllowed="true"`) do not block.
fn conflicting_names(xml: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for proc_xml in xml.split("<ConflictingProcess ").skip(1) {
        let head = proc_xml.split('>').next().unwrap_or("");
        if head.contains("forceKillAllowed=\"true\"") {
            continue;
        }
        let Some(part) = proc_xml.split("<RegularExpression>").nth(1) else { continue };
        let re = part.split("</RegularExpression>").next().unwrap_or("");
        let mut name = String::new();
        let mut chars = re.trim_start_matches('^').trim_end_matches('$').chars().peekable();
        let mut ok = true;
        while let Some(c) = chars.next() {
            match c {
                '[' => {
                    let class: String = chars.by_ref().take_while(|&x| x != ']').collect();
                    match class.chars().next() {
                        Some(first) if class.chars().all(|x| x.eq_ignore_ascii_case(&first)) => name.push(first.to_ascii_lowercase()),
                        _ => ok = false,
                    }
                }
                '\\' => {
                    if let Some(n) = chars.next() {
                        name.push(n);
                    }
                }
                '.' => name.push('.'),
                '*' | '+' | '?' | '(' | ')' | '|' | '{' | '}' => ok = false,
                c => name.push(c.to_ascii_lowercase()),
            }
        }
        if ok && name.ends_with(".exe") && !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

/// Catalog entries that are tools, legacy or store apps, never offered.
fn skip_in_catalog(sap: &str) -> bool {
    matches!(
        sap,
        "AAM" | "FLBR" | "FWKS" | "GSDK" | "KEMN" | "KETK" | "MUSE" | "ASCT" | "TAPI" | "PSXUWP" | "SPRKBE" | "ANML"
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
    let mut cat = tokio::task::spawn_blocking(move || parse_catalog(&f.body)).await??;
    // Creative Cloud: the full build number, which its offline installer's address needs.
    match creative_cloud_build().await {
        Ok(Some(full)) => {
            for p in cat.products.iter_mut().filter(|p| p.sap == "KCCC" && full.starts_with(&format!("{}.", p.version))) {
                p.version = full.clone();
            }
        }
        Ok(None) => {}
        Err(e) => log::warn!("creative cloud version: {e}"),
    }
    Ok(cat)
}

/// The newest Creative Cloud build from its update service.
async fn creative_cloud_build() -> Result<Option<String>> {
    let f = http::get_cached(CC_SERVICE_URL, &[APP_ID_HEADER, API_KEY_HEADER], Duration::from_secs(20 * 60)).await?;
    Ok(parse_cc_build(&String::from_utf8_lossy(&f.body)))
}

fn parse_cc_build(xml: &str) -> Option<String> {
    let at = xml.find("<application><name>CreativeCloud</name>")?;
    let rest = &xml[at..];
    let v = rest.split("<version>").nth(1)?.split('<').next()?.trim();
    (v.split('.').count() >= 4 && v.split('.').all(|p| p.parse::<u32>().is_ok())).then(|| v.to_string())
}

/// Address of the offline installer for a full build: 6.11.0.478.6 is
/// `.../ESD/6.11.0/478.6/win64/ACCCx6_11_0_478_6.zip`.
pub fn cc_offline_url(full: &str) -> Option<String> {
    let p: Vec<&str> = full.split('.').collect();
    if p.len() < 4 || p.iter().any(|x| x.parse::<u32>().is_err()) {
        return None;
    }
    Some(format!("{CC_OFFLINE_BASE}/{}/{}/win64/ACCCx{}.zip", p[..3].join("."), p[3..].join("."), p.join("_")))
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
        let args = winutil::split_program_line(&e.uninstall_string);
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
        let component = is_component(&p.sap);
        let tagline = catalog
            .and_then(|c| c.for_sap(&p.sap).find_map(|x| x.tagline.clone()))
            .or_else(|| component.then(|| "Shared Adobe component, used by Photoshop and other Adobe apps".to_string()));
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
            uninstall: if component { None } else { entry.map(|e| winutil::split_program_line(&e.uninstall_string)) },
            updated_at,
            size_hint: entry.map(|e| e.estimated_size_kb * 1024).filter(|s| *s > 0),
            tagline,
            channel: None,
            // Components have no program of their own to check.
            missing: missing && !component,
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
    if let Some(cc) = creative_cloud(catalog, uninstall) {
        out.push(cc);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The Creative Cloud desktop app, so it updates like the apps it manages. It cannot be removed
/// here (Adobe's uninstaller refuses while Adobe apps are installed).
fn creative_cloud(catalog: Option<&Catalog>, uninstall: &[winutil::UninstallEntry]) -> Option<Installed> {
    let e = uninstall.iter().find(|e| e.display_name == "Adobe Creative Cloud" && !e.display_version.is_empty())?;
    let dir = [paths::program_files(), paths::program_files_x86()]
        .into_iter()
        .map(|p| p.join("Adobe\\Adobe Creative Cloud"))
        .find(|d| d.join("ACC\\Creative Cloud.exe").exists())?;
    let exe = dir.join("ACC\\Creative Cloud.exe");
    let (code, fg, bg) = mark_for("KCCC", "Creative Cloud");
    Some(Installed {
        id: format!("adobe:KCCC:{}", version::major(&e.display_version)),
        source: Source::Adobe,
        name: "Creative Cloud".into(),
        code,
        fg,
        bg,
        version: e.display_version.clone(),
        install_dir: Some(dir),
        updated_at: winutil::install_date_iso(&e.install_date)
            .or_else(|| exe.metadata().ok().and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339())),
        exe: Some(exe),
        uninstall: None,
        size_hint: Some(e.estimated_size_kb * 1024).filter(|s| *s > 0),
        tagline: catalog.and_then(|c| c.for_sap("KCCC").find_map(|x| x.tagline.clone())),
        channel: None,
        missing: false,
        read_only: false,
        icon_remote: catalog.and_then(|c| c.icon("KCCC")).map(String::from),
        data: SourceData::Adobe { sap: "KCCC".into(), codex: e.display_version.clone(), platform: "win64".into(), product_name: "Adobe Creative Cloud".into() },
    })
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
        // Installed apps stay listed, marked installed (with Open), so the list is the whole catalog.
        .map(|p| {
            let name = display_name(&p.sap, &p.name);
            let (code, fg, bg) = mark_for(&p.sap, &name);
            let prerelease = p.sap.ends_with("BETA") || p.sap == "ILSTPR" || name.contains("(Beta)") || name.contains("Prerelease") || name.contains("(Preview");
            let cc = p.sap == "KCCC";
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
                installed: installed.iter().any(|i| sap_of(i).0 == p.sap && !i.missing),
                // Adobe offers only the newest Creative Cloud.
                pick_version: !cc,
            }
        })
        .collect();
    out.sort_by(|a, b| a.prerelease.cmp(&b.prerelease).then(a.name.cmp(&b.name)));
    out
}

/// Adobe community announcement categories ("community.adobe.com/announcements-710") per app,
/// with the names its release posts give it ("Photoshop v27.6 Is Live!").
const COMMUNITY: &[(&str, u32, &[&str])] = &[
    ("PHSP", 710, &["Photoshop"]),
    ("ILST", 651, &["Illustrator"]),
    ("PPRO", 727, &["Premiere Pro", "Premiere"]),
    ("AEFT", 527, &["After Effects", "AE"]),
    ("AME", 504, &["Media Encoder"]),
    ("AUDT", 542, &["Audition"]),
    ("IDSN", 670, &["InDesign"]),
    ("KBRG", 556, &["Bridge"]),
    ("LTRM", 673, &["Lightroom Classic"]),
    ("LRCC", 678, &["Lightroom Desktop", "Lightroom"]),
    ("ACR", 561, &["Camera Raw", "ACR"]),
    ("FRSC", 645, &["Fresco"]),
    ("SBSTA", 67, &["Substance 3D Sampler", "Sampler"]),
    ("KCCC", 600, &["Creative Cloud Desktop", "Creative Cloud"]),
    ("PHSPBETA", 698, &["Photoshop Beta", "Photoshop"]),
    ("ILSTPR", 654, &["Beta Build", "Beta", "Illustrator Beta", "Illustrator"]),
    ("AEFTBETA", 532, &["After Effects Beta", "AE Beta", "After Effects"]),
    ("PPROBETA", 732, &["Premiere Pro Beta", "Premiere Beta", "Premiere"]),
    ("AMEBETA", 509, &["Media Encoder Beta", "Media Encoder"]),
];

/// The app names of a community category and whether its posts are about a beta.
pub fn community_names(id: u32) -> Option<(&'static [&'static str], bool)> {
    COMMUNITY.iter().find(|c| c.1 == id).map(|(sap, _, names)| (*names, sap.ends_with("BETA") || *sap == "ILSTPR"))
}

/// The community category that also announces the releases of an app whose notes come from an
/// Experience League "All changes" page (Sampler 6.0.0 is only announced there).
pub fn community_fallback(notes_url: &str) -> Option<u32> {
    let sap = if notes_url.contains("/substance-3d-sampler/") { "SBSTA" } else { return None };
    COMMUNITY.iter().find(|c| c.0 == sap).map(|c| c.1)
}

/// Release notes that answer apps, which `notes::adobe` reads per version: Substance 3D "All
/// changes" pages on Adobe Experience League, Modeler's Steam news and the community
/// announcements of the other apps. adobe.com short links and helpx.adobe.com reject anything
/// that is not a browser, so those only open in the browser.
fn release_notes_url(sap: &str) -> Option<String> {
    let slug = match sap {
        "SBSTP" => "substance-3d-painter",
        "SBSTD" => "substance-3d-designer",
        "SBSTA" => "substance-3d-sampler",
        "SHPR" => return Some("https://store.steampowered.com/news/app/1745780".into()),
        _ => return COMMUNITY.iter().find(|c| c.0 == sap).map(|c| format!("https://community.adobe.com/announcements-{}", c.1)),
    };
    Some(format!("https://experienceleague.adobe.com/en/docs/{slug}/using/release-notes/all-changes"))
}

/// Release notes pages that only open in the browser (helpx.adobe.com rejects apps).
fn browser_notes_url(sap: &str) -> Option<String> {
    Some(
        match sap {
            "KCCC" => CC_NOTES_URL,
            "ACR" => "https://helpx.adobe.com/camera-raw/using/whats-new.html",
            _ => return None,
        }
        .to_string(),
    )
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
            notes_url: release_notes_url(inst_sap).or_else(|| p.whats_new.clone()).or_else(|| browser_notes_url(inst_sap)),
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
    let new_id = format!("adobe:{}:{}", product.sap, version::major(&product.version));
    if product.sap == "KCCC" {
        return plan_creative_cloud(&req, &product.version).await;
    }
    let block_dirs: Vec<PathBuf> = req.current.and_then(|c| c.install_dir.clone()).into_iter().collect();
    // An update to the newest version of an installed app: Adobe's Remote Update Manager.
    let newest = cat.newest(req.sap, req.platform, None).map(|p| p.version.clone());
    if req.kind == JobKind::Update && req.current.is_some() && newest.as_deref() == Some(product.version.as_str()) {
        if is_component(req.sap) {
            // A component has no window of its own; it waits for the apps that load it.
            let mut plan = plan_rum(req.sap, &req.app_id, &req.name, &product.version, vec![]);
            plan.block_names = component_blockers(req.sap);
            return Ok(plan);
        }
        return Ok(plan_rum(req.sap, &req.app_id, &req.name, &product.version, block_dirs));
    }
    // Anything else (a new app, another version, a repair): Creative Cloud installs it.
    Ok(Plan {
        app_id: req.app_id,
        kind: req.kind,
        name: req.name.clone(),
        to_version: product.version.clone(),
        downloads: vec![],
        steps: vec![Step::OpenUri { uri: cc_uri(&product.sap, &product.version), label: format!("Installing {} {} in Creative Cloud", req.name, product.version) }],
        block_dirs,
        block_names: vec![],
        verify: Verify::Wait { app_id: new_id, version: product.version.clone(), minutes: 90 },
        fallback: None,
        cleanup: vec![],
    })
}

/// Update Creative Cloud itself: Adobe's offline installer for the exact build, checked against
/// the size and MD5 Adobe's server reports, unpacked and run silently through the admin helper.
async fn plan_creative_cloud(req: &InstallRequest<'_>, to_version: &str) -> Result<Plan> {
    let url = cc_offline_url(to_version).ok_or_else(|| anyhow!("Adobe has not published the full Creative Cloud {to_version} installer yet. Try again later."))?;
    let head = http::CLIENT.head(&url).send().await.with_context(|| "Could not reach Adobe's download server")?;
    if !head.status().is_success() {
        bail!("Adobe's download server has no Creative Cloud {to_version} installer (HTTP {})", head.status());
    }
    let size = head.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    let md5 = head
        .headers()
        .get("content-md5")
        .and_then(|v| v.to_str().ok())
        .and_then(|b64| crate::download::md5_hex_from_integrity(&format!("md5-{b64}")));
    if md5.is_none() {
        bail!("Adobe's server sent no checksum for the Creative Cloud installer, so it was not downloaded");
    }
    let root = paths::downloads().join("creative-cloud");
    let zip = root.join(format!("ACCCx{}.zip", to_version.replace('.', "_")));
    let dir = root.join(format!("ACCC-{to_version}"));
    Ok(Plan {
        app_id: req.app_id.clone(),
        kind: req.kind,
        name: req.name.clone(),
        to_version: to_version.to_string(),
        downloads: vec![Download { url, dest: zip.clone(), size, sha256: None, md5, signer: None, sha512: None, headers: vec![] }],
        steps: vec![
            Step::ExtractZip { zip: zip.clone(), dest: dir.clone(), strip_root: false, elevated: false, label: "Unpacking Creative Cloud".into() },
            Step::Run {
                exe: dir.join("Set-up.exe"),
                args: vec!["--silent".into()],
                cwd: Some(dir.clone()),
                ok_codes: vec![0],
                elevated: true,
                label: "Updating Creative Cloud".into(),
                log: None,
            },
        ],
        block_dirs: vec![],
        block_names: vec![],
        verify: Verify::Version { app_id: format!("adobe:KCCC:{}", version::major(to_version)), version: to_version.to_string() },
        fallback: None,
        cleanup: vec![zip, dir],
    })
}

/// Apps that must be closed before a component updates, from Adobe's install database.
fn component_blockers(sap: &str) -> Vec<String> {
    let (db, _) = read_db();
    db.iter()
        .filter(|p| p.sap == sap)
        .filter_map(|p| p.meta.get("ConflictingProcessesXML"))
        .flat_map(|x| conflicting_names(x))
        .fold(Vec::new(), |mut acc, n| {
            if !acc.contains(&n) {
                acc.push(n);
            }
            acc
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
        downloads: vec![Download { url: RUM_URL.into(), dest: zip.clone(), size: None, sha256: None, md5: None, signer: None, sha512: None, headers: vec![] }],
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_updates_wait_for_the_apps_adobe_lists() {
        // Shape of Camera Raw's ConflictingProcessesXML in Adobe's install database.
        let xml = r#"<ConflictingProcesses><ConflictingProcess headless="false" forceKillAllowed="false" adobeOwned="true"><RegularExpression>^[Aa][Ff][Tt][Ee][Rr][Ff][Xx].[Ee][Xx][Ee]$</RegularExpression><ProcessDisplayName>Adobe After Effects 2027</ProcessDisplayName></ConflictingProcess><ConflictingProcess headless="false" forceKillAllowed="false" adobeOwned="true"><RegularExpression>^[Pp][Hh][Oo][Tt][Oo][Ss][Hh][Oo][Pp].[Ee][Xx][Ee]$</RegularExpression></ConflictingProcess><ConflictingProcess headless="false" forceKillAllowed="false" adobeOwned="true"><RegularExpression>^[Aa][Nn][Ii][Mm][Aa][Tt][Ee]\.[Ee][Xx][Ee]$</RegularExpression></ConflictingProcess><ConflictingProcess headless="false" forceKillAllowed="false" adobeOwned="true"><RegularExpression>^[Pp][Hh][Oo][Tt][Oo][Ss][Hh][Oo][Pp].[Ee][Xx][Ee]$</RegularExpression></ConflictingProcess><ConflictingProcess headless="true" forceKillAllowed="true" adobeOwned="true"><RegularExpression>^[Cc][Oo][Rr][Ee][Ss][Yy][Nn][Cc].[Ee][Xx][Ee]$</RegularExpression></ConflictingProcess><ConflictingProcess forceKillAllowed="false"><RegularExpression>^Light(room|room Classic).*$</RegularExpression></ConflictingProcess></ConflictingProcesses>"#;
        // Photoshop once, CoreSync left to Adobe's installer, the loose pattern skipped.
        assert_eq!(conflicting_names(xml), vec!["afterfx.exe", "photoshop.exe", "animate.exe"]);
        assert!(is_component("ACR") && !is_component("PHSP") && !is_component("KCCC"));
    }

    #[test]
    fn release_notes_links_that_answer_apps() {
        // Stager's and Modeler's Experience League release notes pages do not exist.
        assert_eq!(release_notes_url("STGR"), None);
        assert_eq!(release_notes_url("SHPR").as_deref(), Some("https://store.steampowered.com/news/app/1745780"));
        assert_eq!(release_notes_url("SBSTA").as_deref(), Some("https://experienceleague.adobe.com/en/docs/substance-3d-sampler/using/release-notes/all-changes"));
        assert_eq!(release_notes_url("PHSP").as_deref(), Some("https://community.adobe.com/announcements-710"));
        assert_eq!(release_notes_url("PHSPBETA").as_deref(), Some("https://community.adobe.com/announcements-698"));
        assert_eq!(release_notes_url("APRO"), None);
        assert_eq!(community_names(654).map(|n| n.1), Some(true));
        assert_eq!(community_names(673).map(|n| n.1), Some(false));
        assert_eq!(community_fallback(&release_notes_url("SBSTA").unwrap()), Some(67));
        assert_eq!(community_fallback(&release_notes_url("SBSTP").unwrap()), None);
    }

    #[test]
    fn creative_cloud_full_build_and_offline_installer_address() {
        let xml = r#"<?xml version='1.0'?><applications><cdn><secure>https://ccmdls.adobe.com</secure></cdn><application><name>CreativeCloud</name><platform>win64</platform><version>6.11.0.478.6</version><packageSets><packageSet><packages><package><name>IPC</name><version>10.0.0.262</version></package></packages></packageSet></packageSets></application></applications>"#;
        assert_eq!(parse_cc_build(xml).as_deref(), Some("6.11.0.478.6"));
        assert_eq!(parse_cc_build("<applications></applications>"), None);
        assert_eq!(
            cc_offline_url("6.11.0.478.6").as_deref(),
            Some("https://ccmdls.adobe.com/AdobeProducts/StandaloneBuilds/ACCC/ESD/6.11.0/478.6/win64/ACCCx6_11_0_478_6.zip")
        );
        assert_eq!(
            cc_offline_url("6.8.1.856").as_deref(),
            Some("https://ccmdls.adobe.com/AdobeProducts/StandaloneBuilds/ACCC/ESD/6.8.1/856/win64/ACCCx6_8_1_856.zip")
        );
        // The catalog's short version has no build number, so there is no installer address.
        assert_eq!(cc_offline_url("6.11.0"), None);
    }

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
