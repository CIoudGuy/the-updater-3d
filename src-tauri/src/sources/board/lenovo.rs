//! Lenovo PCs: the newest BIOS, drivers and firmware Lenovo lists for the PC, from the catalog
//! Lenovo System Update and Thin Installer read on download.lenovo.com. Lenovo lists a PC by the
//! first four characters of its machine type ("21CB" for a ThinkPad X1 Carbon): one small catalog
//! per machine type and Windows (`catalog/21CB_Win11.xml`, about 14 KB) names a descriptor file
//! for each package (about 6 KB each, checked against the catalog's checksum), which has the
//! version, date, hardware ids, detection, installer file with its checksum and install command.
//! Only the machine type goes to Lenovo.

use super::dell::decode_text;
use super::{Answer, Board, Listed, MakerDriver, MakerInstall};
use anyhow::{bail, Context, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::time::Duration;

const CATALOG: &str = "https://download.lenovo.com/catalog/";
/// Lenovo's support search, for the machine type.
const SUPPORT: &str = "https://pcsupport.lenovo.com/us/en/search?query=";
/// The catalog is kept for twelve hours, a package descriptor for a week (it is checked against
/// the catalog's checksum, and asked for again when that differs), a readme for a month.
const AGE: Duration = Duration::from_secs(12 * 3600);
const PACKAGE_AGE: Duration = Duration::from_secs(7 * 24 * 3600);
const README_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// The signer name of Lenovo's packages (read from a downloaded package's signature).
const SIGNER: &str = "Lenovo";

/// A package of the catalog.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    /// The package's descriptor file
    pub url: String,
    /// Lenovo's category: "Audio", "BIOS UEFI", "Networking Wireless LAN"
    pub category: String,
    /// The descriptor's sha256
    pub sha256: String,
}

/// What a package's descriptor says.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pkg {
    pub id: String,
    pub version: String,
    pub title: String,
    pub date: Option<chrono::NaiveDate>,
    /// 1 application, 2 driver, 3 BIOS, 4 firmware
    pub kind: u8,
    pub install_cmd: String,
    pub install_rc: Vec<u32>,
    /// The installer file: name, checksum (sha256 or md5, in hex), size
    pub file: Option<(String, String, Option<u64>)>,
    pub readme: Option<String>,
    /// The driver versions the package installs, with the hardware ids each is for
    pub drivers: Vec<(String, Vec<String>)>,
    /// How to read the installed version of a tool from the registry: (key, value name)
    pub registry: Vec<(String, String)>,
    /// The version a tool has once installed (Lenovo: "2.0.1.0^", "or newer")
    pub registry_version: Option<String>,
    /// The BIOS ids a BIOS package is the newest of ("N3AET91*") and the ones it updates ("N3AET8*")
    pub bios_now: Vec<String>,
    pub bios_from: Vec<String>,
}

// ---------------------------------------------------------------- the PC

/// The first four characters of a Lenovo machine type: "21CB" from "21CBCTO1WW" or the SKU
/// "LENOVO_MT_21CB_BU_Think_FM_ThinkPad X1 Carbon Gen 10".
pub fn machine_type(b: &Board) -> Option<String> {
    let ok = |s: &str| s.len() == 4 && s.chars().all(|c| c.is_ascii_alphanumeric()) && s.chars().any(|c| c.is_ascii_digit());
    if let Some(i) = b.system_sku.find("MT_") {
        let s: String = b.system_sku[i + 3..].chars().take(4).collect();
        if ok(&s) {
            return Some(s.to_ascii_uppercase());
        }
    }
    let s: String = b.system_product.trim().chars().take(4).collect();
    ok(&s).then(|| s.to_ascii_uppercase())
}

pub fn catalog_url(mt: &str, windows11: bool) -> String {
    format!("{CATALOG}{mt}_{}.xml", if windows11 { "Win11" } else { "Win10" })
}

fn support_page(mt: &str) -> String {
    format!("{SUPPORT}{mt}")
}

// ---------------------------------------------------------------- reading the files

fn txt(n: roxmltree::Node, name: &str) -> String {
    n.children().find(|c| c.has_tag_name(name)).and_then(|c| c.text()).unwrap_or("").trim().to_string()
}

/// The packages of a machine type's catalog. A descriptor that is not an https .xml file on
/// Lenovo's server is not taken.
pub fn parse_catalog(xml: &str) -> Result<Vec<Entry>> {
    let doc = roxmltree::Document::parse(xml).context("Lenovo's catalog could not be read")?;
    let list: Vec<Entry> = doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("package"))
        .map(|n| Entry { url: txt(n, "location"), category: txt(n, "category"), sha256: txt(n, "checksum").to_ascii_lowercase() })
        .filter(|e| e.url.ends_with(".xml") && !e.url.contains("..") && crate::sources::drivers::host_is(&e.url, &["lenovo.com"]) && e.sha256.len() == 64)
        .collect();
    if list.is_empty() {
        bail!("Lenovo's catalog is empty");
    }
    Ok(list)
}

fn hex(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_hexdigit())
}

/// "6.0.9764.1^10.0" and "30.100.2148.1^" as "6.0.9764.1" and "30.100.2148.1": Lenovo's "^" means
/// "or newer".
fn clean_version(v: &str) -> String {
    v.split('^').next().unwrap_or("").trim().trim_start_matches(['v', 'V']).to_string()
}

/// A driver version Windows compares: digits and dots, at least two parts.
fn dotted(v: &str) -> bool {
    let v = v.trim();
    v.chars().all(|c| c.is_ascii_digit() || c == '.') && v.split('.').count() >= 2 && v.split('.').all(|p| !p.is_empty())
}

fn nums(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

fn cmp_nums(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    (0..n).map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0))).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)
}

/// A hardware id from a detection rule. An HD Audio codec is written without its bus
/// ("VEN_10EC&DEV_0257"): Windows names it "HDAUDIO\FUNC_01&VEN_10EC&DEV_0257&SUBSYS_...".
fn hardware_id(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() {
        None
    } else if id.to_ascii_uppercase().starts_with("VEN_") {
        Some(format!("HDAUDIO\\FUNC_01&{}", id.to_ascii_uppercase()))
    } else {
        Some(id.to_ascii_uppercase())
    }
}

/// What a package descriptor says.
pub fn parse_package(xml: &str) -> Result<Pkg> {
    let doc = roxmltree::Document::parse(xml).context("A Lenovo package descriptor could not be read")?;
    let root = doc.root_element();
    if !root.has_tag_name("Package") {
        bail!("Not a Lenovo package descriptor");
    }
    let id = root.attribute("id").unwrap_or("").trim().to_string();
    let version = root.attribute("version").unwrap_or("").trim().to_string();
    if id.is_empty() || version.is_empty() {
        bail!("A Lenovo package descriptor without an id or version");
    }
    let title = root.children().find(|c| c.has_tag_name("Title")).and_then(|t| t.children().find(|d| d.has_tag_name("Desc") && d.attribute("id").is_none_or(|i| i.eq_ignore_ascii_case("EN"))).or(t.children().find(|d| d.has_tag_name("Desc")))).and_then(|d| d.text()).unwrap_or("").trim().to_string();
    let install = root.children().find(|c| c.has_tag_name("Install") && c.attribute("type").is_none_or(|t| t.eq_ignore_ascii_case("cmd")));
    let file = root.children().find(|c| c.has_tag_name("Files")).and_then(|f| f.children().find(|c| c.has_tag_name("Installer"))).and_then(|i| i.children().find(|c| c.has_tag_name("File"))).map(|f| (txt(f, "Name"), txt(f, "CRC").to_ascii_lowercase(), txt(f, "Size").parse().ok())).filter(|f| !f.0.is_empty());
    let readme = root.children().find(|c| c.has_tag_name("Files")).and_then(|f| f.children().find(|c| c.has_tag_name("Readme"))).and_then(|r| r.children().find(|c| c.has_tag_name("File"))).map(|f| txt(f, "Name")).filter(|n| !n.is_empty());
    let detect_install: Vec<_> = root.children().filter(|c| c.has_tag_name("DetectInstall")).collect();
    // The driver versions with their hardware ids.
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for d in detect_install.iter().flat_map(|d| d.descendants()).filter(|n| n.has_tag_name("_Driver")) {
        let v = clean_version(&txt(d, "Version"));
        let ids: Vec<String> = d.children().filter(|c| c.has_tag_name("HardwareID")).filter_map(|c| c.text().and_then(hardware_id)).collect();
        if dotted(&v) && !ids.is_empty() {
            groups.entry(v).or_default().extend(ids);
        }
    }
    for ids in groups.values_mut() {
        ids.sort();
        ids.dedup();
    }
    let registry: Vec<(String, String)> = root.children().filter(|c| c.has_tag_name("DetectVersion") && c.attribute("type").is_some_and(|t| t.eq_ignore_ascii_case("Registry"))).flat_map(|d| d.children().filter(|c| c.has_tag_name("Registry")).map(|r| (txt(r, "Key"), txt(r, "ValueName"))).collect::<Vec<_>>()).filter(|(k, v)| !k.is_empty() && !v.is_empty()).collect();
    let registry_version = detect_install.iter().flat_map(|d| d.descendants()).find(|n| n.has_tag_name("_RegistryKeyValue")).map(|n| clean_version(&txt(n, "Version"))).filter(|v| dotted(v));
    let levels = |n: roxmltree::Node| -> Vec<String> { n.descendants().filter(|l| l.has_tag_name("Level")).filter_map(|l| l.text()).map(|t| t.trim().to_ascii_uppercase()).filter(|t| !t.is_empty() && t != "*").collect() };
    let bios_now = detect_install.iter().flat_map(|d| levels(*d)).collect();
    let bios_from = root.children().filter(|c| c.has_tag_name("Dependencies")).flat_map(levels).collect();
    Ok(Pkg {
        id,
        version,
        title,
        date: chrono::NaiveDate::parse_from_str(&txt(root, "ReleaseDate"), "%Y-%m-%d").ok(),
        kind: root.children().find(|c| c.has_tag_name("PackageType")).and_then(|p| p.attribute("type")).and_then(|t| t.parse().ok()).unwrap_or(0),
        install_cmd: install.and_then(|i| i.children().find(|c| c.has_tag_name("Cmdline"))).and_then(|c| c.text()).unwrap_or("").trim().to_string(),
        install_rc: install.and_then(|i| i.attribute("rc")).map(|rc| rc.split(',').filter_map(|c| c.trim().parse().ok()).collect()).unwrap_or_default(),
        file,
        readme,
        drivers: groups.into_iter().collect(),
        registry,
        registry_version,
        bios_now,
        bios_from,
    })
}

// ---------------------------------------------------------------- BIOS

/// A BIOS package.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bios {
    /// "1.56" (a ThinkPad: Lenovo's number, with the BIOS id in the readme) or the BIOS id itself
    /// ("GKCN65WW")
    pub version: String,
    /// The BIOS id of that version, "N3AET91W", when the readme says it
    pub level: Option<String>,
    /// The id the descriptor gives it, "N3AET91" (the end of the id is left to a "*")
    pub hint: Option<String>,
    /// The BIOS ids of this BIOS family ("N3AET3*"): the PCs it is for
    pub family: Vec<String>,
    pub date: Option<String>,
    pub readme: Option<String>,
    pub notes: Vec<String>,
}

static LEVEL: Lazy<Regex> = Lazy::new(|| Regex::new(r"^([A-Z0-9]+?)(\d{1,3})([A-Z]{0,2})\*?$").unwrap());

/// "N3AET91W" as ("N3AET", 91); "N3AET3*" as ("N3AET", 3).
fn level_of(s: &str) -> Option<(String, u32)> {
    let s = s.trim().to_ascii_uppercase();
    let c = LEVEL.captures(&s)?;
    Some((c[1].to_string(), c[2].parse().ok()?))
}

/// "N3AET78W (1.52 )" as ("N3AET78W", [1, 52]); "GKCN64WW" as ("GKCN64WW", none).
fn have_parts(have: &str) -> (Option<String>, Option<Vec<u64>>) {
    static PAREN: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(\s*(\d+(?:\.\d+)+)\s*\)").unwrap());
    let level = have.split_whitespace().next().map(str::to_ascii_uppercase).filter(|t| level_of(t).is_some());
    let num = PAREN.captures(have).map(|c| nums(&c[1]));
    (level, num)
}

/// Is `latest` a newer BIOS than `have` (the version Windows reads: "N3AET78W (1.52 )", or
/// "GKCN64WW")? `latest` is Lenovo's number ("1.56") or a BIOS id; `hint` is the BIOS id of
/// `latest` when known. None when the two do not compare (another BIOS family).
pub fn newer(latest: &str, hint: Option<&str>, have: &str) -> Option<bool> {
    let (have_level, have_num) = have_parts(have);
    let by_level = |l: &str| -> Option<bool> {
        let (p, n) = level_of(l)?;
        let (hp, hn) = level_of(have_level.as_deref()?)?;
        (p == hp).then_some(n > hn)
    };
    if dotted(latest) {
        if let Some(h) = have_num {
            return Some(cmp_nums(&nums(latest), &h).is_gt());
        }
        return hint.and_then(by_level);
    }
    by_level(latest)
}

fn bios_key(b: &Bios) -> (Vec<u64>, u32) {
    (if dotted(&b.version) { nums(&b.version) } else { vec![] }, level_of(&b.version).or_else(|| b.hint.as_deref().and_then(level_of)).map(|(_, n)| n).unwrap_or(0))
}

/// The BIOS packages of a catalog.
pub fn bios_of(pkgs: &[Pkg]) -> Vec<Bios> {
    let mut out: Vec<Bios> = pkgs
        .iter()
        .filter(|p| p.kind == 3)
        .map(|p| Bios {
            version: p.version.clone(),
            level: None,
            hint: p.bios_now.first().map(|l| l.trim_end_matches('*').to_string()).filter(|l| !l.is_empty()),
            family: p.bios_from.iter().chain(&p.bios_now).cloned().collect(),
            date: p.date.map(|d| d.format("%Y.%m.%d").to_string()),
            readme: None,
            notes: vec![],
        })
        .collect();
    out.sort_by_key(bios_key);
    out
}

/// The BIOS package for this PC: the one of its BIOS family, the newest of them. The flag says
/// the PC's BIOS is of the family of the one chosen (so the two compare).
fn choose<'a>(b: &Board, list: &'a [Bios]) -> Option<(&'a Bios, bool)> {
    let family = have_parts(&b.bios).0.and_then(|l| level_of(&l)).map(|(p, _)| p);
    let same: Vec<&Bios> = list.iter().filter(|x| family.is_some() && x.family.iter().filter_map(|l| level_of(l)).any(|(p, _)| Some(&p) == family.as_ref())).collect();
    let newest = |l: Vec<&'a Bios>| l.into_iter().max_by_key(|x| bios_key(x));
    if same.is_empty() { newest(list.iter().collect()).map(|x| (x, false)) } else { newest(same).map(|x| (x, true)) }
}

/// What a BIOS shows as its version: "N3AET91W (1.56)" when the readme names the BIOS id.
fn shown(b: &Bios) -> String {
    match &b.level {
        Some(l) if !b.version.eq_ignore_ascii_case(l) => format!("{l} ({})", b.version),
        _ => b.version.clone(),
    }
}

/// The BIOS id of a version in a readme's table of releases ("1.56(N3AET91W)").
pub fn level_in_readme(html: &str, version: &str) -> Option<String> {
    let text = crate::notes::strip_tags(html);
    let re = Regex::new(&format!(r"(?:^|[^0-9.]){}\s*\(\s*([A-Z0-9]{{6,9}})\s*\)", regex::escape(version))).ok()?;
    re.captures(&text).map(|c| c[1].to_string())
}

/// What a readme says changed in the release, as note lines: its "Changes in This Release"
/// section, with the version lines and the empty "Nothing." sections left out.
pub fn readme_notes(html: &str) -> Vec<String> {
    let Some(start) = html.find("Changes in This Release") else { return vec![] };
    let rest = &html[start..];
    let Some(open) = rest.find("</button>") else { return vec![] };
    let rest = &rest[open + "</button>".len()..];
    let section = &rest[..rest.find("<button").unwrap_or(rest.len())];
    let mut out: Vec<String> = Vec::new();
    let mut started = false;
    for l in crate::sources::drivers::html_notes(section) {
        let t = l.trim().trim_start_matches("### ").trim();
        if let Some(h) = t.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
            started = true;
            out.push(format!("## {}", h.trim()));
            continue;
        }
        let item = t.trim_start_matches('-').trim();
        // Before the first section: "Version:" and the numbers.
        if !started || item.is_empty() || item.eq_ignore_ascii_case("nothing") || item.eq_ignore_ascii_case("nothing.") || item.eq_ignore_ascii_case("n/a") {
            continue;
        }
        out.push(item.to_string());
    }
    crate::notes::tidy_headings(out)
}

/// What Lenovo's catalog gives for the BIOS.
#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    /// The catalog has no file for this machine type
    NotListed,
    Found(Vec<Bios>),
}

/// The answer for this PC from Lenovo's catalog (None: Lenovo was not reached).
pub fn answer(b: &Board, data: Option<&Lookup>) -> Answer {
    let Some(mt) = machine_type(b) else { return Answer::missing("This PC does not report the machine type Lenovo lists it under") };
    let missing = |why: &str| Answer::Missing { why: why.into(), page: Some(support_page(&mt)) };
    match data {
        None => missing("Could not reach Lenovo"),
        Some(Lookup::NotListed) => missing("Lenovo does not list this machine type in its update catalog"),
        Some(Lookup::Found(list)) => match choose(b, list) {
            None => missing("Lenovo lists no BIOS for this machine type"),
            Some((bios, same)) => Answer::Listed(Listed {
                version: shown(bios),
                page: Some(support_page(&mt)),
                date: bios.date.clone(),
                notes: bios.notes.clone(),
                newer: if same { newer(&bios.version, bios.hint.as_deref(), &b.bios) } else { None },
            }),
        },
    }
}

// ---------------------------------------------------------------- drivers

/// Where a package is downloaded to (the same folder `board::plan_install` uses).
fn package_dir(id: &str) -> std::path::PathBuf {
    crate::paths::downloads().join("maker").join(crate::sources::other::safe_name(id))
}

/// The install of a package Lenovo gives as `<installer>.exe <silent switches>`: the download is
/// the program. A package that needs its files unpacked first (`dpinst.exe /s`, `setup.exe`) is
/// not installed here: the install command runs a program inside the download.
fn install_of(p: &Pkg, descriptor: &str, entry_id: &str) -> Option<MakerInstall> {
    if p.kind != 2 {
        return None;
    }
    let (name, sum, _) = p.file.as_ref()?;
    if name.contains(['/', '\\']) || name.contains("..") || !name.to_ascii_lowercase().ends_with(".exe") {
        return None;
    }
    let tokens = crate::winutil::split_command_line(&p.install_cmd);
    let (first, rest) = tokens.split_first()?;
    if !first.rsplit(['\\', '/']).next()?.eq_ignore_ascii_case(name) || rest.is_empty() {
        return None;
    }
    let dir = package_dir(entry_id).to_string_lossy().into_owned();
    let args: Vec<String> = rest.iter().map(|a| a.replace("%PACKAGEPATH%", &dir)).collect();
    // Another placeholder is a path this does not know.
    if args.iter().any(|a| a.contains('%')) {
        return None;
    }
    let (sha256, md5) = match sum.len() {
        64 if hex(sum) => (Some(sum.clone()), None),
        32 if hex(sum) => (None, Some(sum.clone())),
        _ => return None,
    };
    let base = descriptor.rsplit_once('/').map(|(b, _)| b)?;
    let mut ok_codes = p.install_rc.clone();
    if !ok_codes.contains(&0) {
        ok_codes.insert(0, 0);
    }
    if !ok_codes.contains(&crate::sources::drivers::EXIT_REBOOT) {
        ok_codes.push(crate::sources::drivers::EXIT_REBOOT);
    }
    Some(MakerInstall { url: format!("{base}/{name}"), sha256, md5, signer: SIGNER.into(), args, ok_codes, inner: None })
}

/// "Intel AX211 Wireless LAN Driver - 10 (20H2 or Later)/11 (21H2 or Later)" without the Windows part.
fn clean_title(t: &str) -> String {
    static OS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s+-\s+(?:windows\s+)?1[01]\b.*$").unwrap());
    OS.replace(t.trim(), "").trim().to_string()
}

fn kind_of(p: &Pkg, category: &str) -> String {
    match p.kind {
        4 => return "Firmware".into(),
        1 => return "Utility".into(),
        _ => {}
    }
    let c = category.to_ascii_lowercase();
    let n = p.title.to_ascii_lowercase();
    if c.contains("bluetooth") {
        "Bluetooth"
    } else if c.starts_with("audio") {
        "Audio"
    } else if c.starts_with("display") {
        "Graphics"
    } else if c.starts_with("networking") {
        "Network"
    } else if c.starts_with("storage") {
        "Storage"
    } else if c.starts_with("motherboard") {
        "Chipset"
    } else if c.starts_with("mouse") {
        "Input"
    } else if c.starts_with("camera") {
        "Camera"
    } else if c.starts_with("fingerprint") {
        "Security"
    } else if c.starts_with("power") {
        "Power"
    } else if n.contains("firmware") {
        "Firmware"
    } else {
        "Driver"
    }
    .into()
}

/// The drivers and tools of a catalog's packages. `read` reads a registry value (key, value
/// name) for the tools Lenovo detects through the registry.
pub fn driver_list(mt: &str, pkgs: &[(Entry, Pkg)], read: &dyn Fn(&str, &str) -> Option<String>) -> Vec<MakerDriver> {
    let mut out = Vec::new();
    for (e, p) in pkgs.iter().filter(|(e, p)| p.kind != 3 && !e.category.to_ascii_uppercase().contains("BIOS")) {
        let name = clean_title(&p.title);
        let kind = kind_of(p, &e.category);
        let own = clean_version(&p.version);
        let groups: Vec<(String, Vec<String>)> = if p.drivers.is_empty() { vec![(p.registry_version.clone().unwrap_or(own.clone()), vec![])] } else { p.drivers.clone() };
        let many = groups.len() > 1;
        // A tool's installed version, from the registry key Lenovo names.
        let installed = (p.drivers.is_empty() && !p.registry.is_empty()).then(|| p.registry.iter().find_map(|(k, v)| read(k, v).map(|s| clean_version(&s)).filter(|s| dotted(s)))).flatten();
        for (version, hardware_ids) in groups {
            let id = if many && version != own { format!("{}:{version}", p.id) } else { p.id.clone() };
            out.push(MakerDriver {
                install: install_of(p, &e.url, &id),
                id,
                name: name.clone(),
                kind: kind.clone(),
                version,
                date: p.date.map(|d| d.format("%Y.%m.%d").to_string()),
                page: Some(support_page(mt)),
                notes: vec![],
                hardware_ids,
                installed: installed.clone(),
                size: p.file.as_ref().and_then(|f| f.2),
            });
        }
    }
    out.sort_by(|a, b| (&a.kind, &a.name, &a.id).cmp(&(&b.kind, &b.name, &b.id)));
    out
}

/// A registry value as text (HKLM and HKCU, the 64-bit and the 32-bit view).
fn read_registry(key: &str, value: &str) -> Option<String> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY};
    let (hive, path) = key.split_once('\\')?;
    let root = match hive.to_ascii_uppercase().as_str() {
        "HKEY_LOCAL_MACHINE" | "HKLM" => HKEY_LOCAL_MACHINE,
        "HKEY_CURRENT_USER" | "HKCU" => HKEY_CURRENT_USER,
        _ => return None,
    };
    [KEY_READ, KEY_READ | KEY_WOW64_32KEY].into_iter().find_map(|flags| winreg::RegKey::predef(root).open_subkey_with_flags(path, flags).ok()?.get_value::<String, _>(value).ok())
}

// ---------------------------------------------------------------- asking Lenovo

fn sha256_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

/// Lenovo's catalog for a machine type: the file for this Windows, else the other (None: Lenovo
/// has none).
pub async fn catalog(mt: &str) -> Result<Option<Vec<Entry>>> {
    let win11 = crate::sources::drivers::windows_11();
    for w in [win11, !win11] {
        match crate::http::get_cached(&catalog_url(mt, w), &[], AGE).await {
            Ok(f) => return Ok(Some(parse_catalog(&decode_text(&f.body))?)),
            Err(e) if e.to_string().contains("HTTP 404") => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

/// A package's descriptor, checked against the catalog's checksum (asked for again when the
/// copy kept is another version).
async fn descriptor(e: &Entry) -> Option<Pkg> {
    let mut f = crate::http::get_cached(&e.url, &[], PACKAGE_AGE).await.map_err(|er| log::warn!("lenovo package: {er:#}")).ok()?;
    if sha256_hex(&f.body) != e.sha256 {
        f = crate::http::get_cached(&e.url, &[], Duration::ZERO).await.ok()?;
        if sha256_hex(&f.body) != e.sha256 {
            log::warn!("lenovo package {}: not the file the catalog names", e.url);
            return None;
        }
    }
    parse_package(&decode_text(&f.body)).map_err(|er| log::warn!("lenovo package: {er:#}")).ok()
}

async fn packages(entries: Vec<Entry>) -> Vec<(Entry, Pkg)> {
    use futures_util::StreamExt;
    futures_util::stream::iter(entries).map(|e| async move { descriptor(&e).await.map(|p| (e, p)) }).buffer_unordered(8).filter_map(|x| async move { x }).collect().await
}

/// The BIOS packages for a machine type (None: not reached), with the notes and BIOS id of the
/// one that applies to `b`.
async fn bios_lookup(b: &Board, mt: &str) -> Result<Lookup> {
    let Some(entries) = catalog(mt).await? else { return Ok(Lookup::NotListed) };
    let bios_entries: Vec<Entry> = entries.into_iter().filter(|e| e.category.to_ascii_uppercase().contains("BIOS")).collect();
    let pkgs = packages(bios_entries).await;
    let mut list = bios_of(&pkgs.iter().map(|(_, p)| p.clone()).collect::<Vec<_>>());
    // The readme of the BIOS that applies: its changes and BIOS id.
    let target = choose(b, &list).map(|(x, _)| (x.version.clone(), x.hint.clone()));
    if let Some((version, _)) = target {
        let readme = pkgs.iter().find(|(_, p)| p.kind == 3 && p.version == version).and_then(|(e, p)| p.readme.as_ref().map(|r| (e.url.rsplit_once('/').map(|(b, _)| b.to_string()).unwrap_or_default(), r.clone())));
        if let Some((base, name)) = readme.filter(|(_, n)| n.to_ascii_lowercase().ends_with(".html")) {
            let url = format!("{base}/{name}");
            if crate::sources::drivers::host_is(&url, &["lenovo.com"]) {
                match crate::http::get_cached(&url, &[], README_AGE).await {
                    Ok(f) => {
                        let html = String::from_utf8_lossy(&f.body).into_owned();
                        if let Some(x) = list.iter_mut().find(|x| x.version == version) {
                            x.notes = readme_notes(&html);
                            x.level = level_in_readme(&html, &version);
                            x.readme = Some(url);
                        }
                    }
                    Err(e) => log::warn!("lenovo readme: {e:#}"),
                }
            }
        }
    }
    Ok(Lookup::Found(list))
}

pub async fn check(b: &Board) -> Answer {
    let Some(mt) = machine_type(b) else { return answer(b, None) };
    let data = bios_lookup(b, &mt).await.map_err(|e| log::warn!("lenovo catalog: {e:#}")).ok();
    answer(b, data.as_ref())
}

/// The drivers and firmware Lenovo lists for this PC.
pub async fn drivers(b: &Board) -> Vec<MakerDriver> {
    let Some(mt) = machine_type(b) else { return vec![] };
    match catalog(&mt).await {
        Ok(Some(entries)) => driver_list(&mt, &packages(entries).await, &read_registry),
        Ok(None) => vec![],
        Err(e) => {
            log::warn!("lenovo catalog: {e:#}");
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppStatus;
    use crate::sources::board::{entry, plan_install, Bios as BiosEntry, Maker};
    use crate::sources::drivers::{host_is, hw_match};

    /// Lenovo's catalog for machine type 21CB (a ThinkPad X1 Carbon Gen 10), cut down, with a
    /// package from another server added.
    const CATALOG_XML: &str = r####"<?xml version="1.0" encoding="utf-8"?><packages count="8">
  <package>
    <location>https://download.lenovo.com/pccbbs/mobiles/n3auj35w_2_.xml</location>
    <category>BIOS UEFI</category>
    <oslangs></oslangs>
    <checksum type="sha256">e7d7787998efa43d955dab5e266f9def3587d7abd0fa712a0ab5a2fb64147078</checksum>
  </package>
  <package>
    <location>https://download.lenovo.com/pccbbs/mobiles/n3awl20w_2_.xml</location>
    <category>Networking Wireless LAN</category>
    <oslangs></oslangs>
    <checksum type="sha256">e1fc73e7a1dbb77352f7739bb56b765222cd1377f8b8f9ec2dc1d24f6041e325</checksum>
  </package>
  <package>
    <location>https://download.lenovo.com/pccbbs/mobiles/n3aa123w_2_.xml</location>
    <category>Audio</category>
    <oslangs></oslangs>
    <checksum type="sha256">d8a8eb7e751456ed1fe9c3d90ee17a3804548d1933c18bac00abe68f762f3c4d</checksum>
  </package>
  <package>
    <location>https://download.lenovo.com/pccbbs/mobiles/n3ali04w_2_.xml</location>
    <category>Motherboard Devices Backplanes core chipset onboard video PCIe switches</category>
    <oslangs></oslangs>
    <checksum type="sha256">a13acf08f559378bc392f5ba0674264df30ec0a3314c58b61bf774c0b6f7eb87</checksum>
  </package>
  <package>
    <location>https://download.lenovo.com/pccbbs/mobiles/n3adr51w_2_.xml</location>
    <category>Display and Video Graphics</category>
    <oslangs></oslangs>
    <checksum type="sha256">6f2a5168503b723cd2d51d77ff090c12b3ac33431bf4c6803d91201ad3c2d859</checksum>
  </package>
  <package>
    <location>https://download.lenovo.com/pccbbs/mobiles/n3acv43w_2_.xml</location>
    <category>Software and Utilities</category>
    <oslangs></oslangs>
    <checksum type="sha256">53b63b4b800325179dedcf4b466bce94d79f4dcf750e0ac0485464b8ae886898</checksum>
  </package>
  <package>
    <location>https://download.lenovo.com/pccbbs/mobiles/r2slf18w_2_.xml</location>
    <category>Camera and Card Reader</category>
    <oslangs></oslangs>
    <checksum type="sha256">206084bb6aaf8a917de33c3154fafe6c562a5dc7aac2f5e83727864601200709</checksum>
  </package>
  <package>
    <location>https://evil.example/pccbbs/mobiles/x_2_.xml</location>
    <category>Audio</category>
    <oslangs></oslangs>
    <checksum type="sha256">0000000000000000000000000000000000000000000000000000000000000000</checksum>
  </package>
</packages>"####;
    /// The descriptors of those packages, cut down, each after a line "=== file name"; and a
    /// consumer PC's BIOS descriptor.
    const PACKAGES: &str = r####"=== n3auj35w_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<Package name="BIOS_N3AUJ" id="n3auj35w" version="1.56" hide="False" pcdDescriptor="0230">
  <Title default="EN">
    <Desc id="EN">BIOS Update Utility - 10/11</Desc>
  </Title>
  <DetectVersion type="File" show="False">
    <File>%WINDOWS%\explorer.exe</File>
  </DetectVersion>
  <Summary default="EN" />
  <Severity type="1" />
  <Vendor>Lenovo</Vendor>
  <ExtractCommand>n3auj35w.exe /VERYSILENT /DIR=%PACKAGEPATH% /EXTRACT="YES"</ExtractCommand>
  <ReleaseDate>2026-08-11</ReleaseDate>
  <DiskSpaceNeeded>85352337</DiskSpaceNeeded>
  <Reboot type="5" />
  <Brand type="2" />
  <PackageType type="3" />
  <Install rc="0,1" rccancel="-1,-10" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\winuptp.exe -s -f</Cmdline>
  </Install>
  <Uninstall />
  <DetectInstall>
    <_Bios>
      <Level>N3AET91*</Level>
    </_Bios>
  </DetectInstall>
  <Dependencies>
    <And>
      <_Bios>
        <Level>N3AET3*</Level>
        <Level>N3AET4*</Level>
        <Level>N3AET5*</Level>
        <Level>N3AET6*</Level>
      </_Bios>
    </And>
  </Dependencies>
  <Files>
    <Installer>
      <File>
        <Name>n3auj35w.exe</Name>
        <CRC>4702EA4B8417DC597642EA53CB19EF66674320CC80892B4CDD6A6E85B714A1A5</CRC>
        <Size>17173664</Size>
      </File>
    </Installer>
    <Readme default="EN">
      <File id="EN">
        <Name>n3auj35w.html</Name>
        <CRC>30FE38C6B569B3476A13AA519173B8FF5A88569F29D39C20A4DD1ED7C4A901B5</CRC>
        <Size>143857</Size>
      </File>
    </Readme>
  </Files>
</Package>
=== n3awl20w_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<Package name="WLAN_N3AWL" id="n3awl20w" version="24.30.1.1" hide="False" pcdDescriptor="0230">
  <Title default="EN">
    <Desc id="EN">Intel AX211 Wireless LAN Driver - 10 (20H2 or Later)/11</Desc>
  </Title>
  <DetectVersion type="Inf">
    <_PnPID><![CDATA[PCI\VEN_8086&DEV_51F0&SUBSYS_00908086]]></_PnPID>
  </DetectVersion>
  <Summary default="EN" />
  <Severity type="1" />
  <Vendor>Lenovo</Vendor>
  <ExtractCommand>n3awl20w.exe /VERYSILENT /DIR=%PACKAGEPATH% /EXTRACT="YES"</ExtractCommand>
  <ReleaseDate>2026-05-20</ReleaseDate>
  <DiskSpaceNeeded>149695687</DiskSpaceNeeded>
  <Reboot type="0" />
  <Brand type="2" />
  <PackageType type="2" />
  <Install rc="0,259,3010" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\n3awl20w.exe /verysilent /DIR=%PACKAGEPATH%\TMP</Cmdline>
  </Install>
  <Uninstall />
  <DetectInstall>
    <_Driver>
      <HardwareID><![CDATA[PCI\VEN_8086&DEV_51F0&SUBSYS_00908086]]></HardwareID>
      <Date>2026-03-03</Date>
      <Version>24.30.1.1^</Version>
    </_Driver>
  </DetectInstall>
  <Files>
    <Installer>
      <File>
        <Name>n3awl20w.exe</Name>
        <CRC>3DEC6D4929E52592851997926604D34004B45105E34CC55C0977C43E78CD1726</CRC>
        <Size>15774928</Size>
      </File>
    </Installer>
    <Readme default="EN">
      <File id="EN">
        <Name>n3awl20w.html</Name>
        <CRC>4E96B46ED3A8CD2DEB78C136F2CE143E83DB17FB4877FCC80FB559799E40E9AB</CRC>
        <Size>48151</Size>
      </File>
    </Readme>
  </Files>
</Package>
=== n3aa123w_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<Package name="AUD_N3AA1" id="n3aa123w" version="6.0.9764.1" hide="False" pcdDescriptor="0230">
  <Title default="EN">
    <Desc id="EN">Realtek Audio Driver - 10 (20H2 or Later)/11 (21H2 or Later)</Desc>
  </Title>
  <DetectVersion type="Inf" show="False">
    <_PnPID><![CDATA[VEN_10EC&DEV_0257]]></_PnPID>
    <_PnPID><![CDATA[VEN_10EC&DEV_0303]]></_PnPID>
    <_PnPID><![CDATA[VEN_10EC&DEV_0298]]></_PnPID>
  </DetectVersion>
  <Summary default="EN" />
  <Severity type="2" />
  <Vendor>Lenovo</Vendor>
  <ExtractCommand>n3aa123w.exe /VERYSILENT /DIR=%PACKAGEPATH% /EXTRACT="YES"</ExtractCommand>
  <ReleaseDate>2025-05-15</ReleaseDate>
  <DiskSpaceNeeded>580216836</DiskSpaceNeeded>
  <Reboot type="3" />
  <Brand type="2" />
  <PackageType type="2" />
  <Install rc="0,259,3010" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\n3aa123w.exe /verysilent /DIR=%PACKAGEPATH%\TMP</Cmdline>
  </Install>
  <Uninstall />
  <DetectInstall>
    <And>
      <_Driver>
        <HardwareID><![CDATA[VEN_10EC&DEV_0287]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0257]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0230]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0298]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0236]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0303]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0285]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0295]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0623]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0274]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_1220]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0235]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0256]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0897]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0222]]></HardwareID>
        <HardwareID><![CDATA[VEN_10EC&DEV_0662]]></HardwareID>
        <Version>6.0.9764.1^10.0</Version>
      </_Driver>
      <_Driver>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_A0C8]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_43C8]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_F0C8]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_7AD0]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_7A50]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51C8]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51C9]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51CA]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51CB]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51CC]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51CD]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51CE]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51CF]]></HardwareID>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_54C8]]></HardwareID>
        <Version>10.29.00.8582^</Version>
      </_Driver>
    </And>
  </DetectInstall>
  <Files>
    <Installer>
      <File>
        <Name>n3aa123w.exe</Name>
        <CRC>D284721780D00A0A23EB9C602B6E24B49B2F2A34EDEFAC63A6F09B9BE0FBC518</CRC>
        <Size>96799312</Size>
      </File>
    </Installer>
    <Readme default="EN">
      <File id="EN">
        <Name>n3aa123w.html</Name>
        <CRC>B2E2208CB0CED7DF98E5FDFE7B8E5E13C6DF150514CAE12A17993BE01C26DD06</CRC>
        <Size>49646</Size>
      </File>
    </Readme>
  </Files>
</Package>
=== n3ali04w_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<?pcdDescriptor version="0230"?>
<Package name="SERIALIO_N3ALI" id="n3ali04w" version="30.100.2148.1" hide="False">
  <Title default="EN">
    <Desc id="EN">Intel Serial IO Driver - 10 (20H2 or Later)/11 (21H2 or Later)</Desc>
  </Title>
  <DetectVersion type="Inf" show="False">
    <_PnPID><![CDATA[ACPI\INTC1055]]></_PnPID>
    <_PnPID><![CDATA[PCI\VEN_8086&DEV_51E8]]></_PnPID>
  </DetectVersion>
  <Summary default="EN" />
  <Severity type="1" />
  <Vendor>Lenovo</Vendor>
  <ExtractCommand>n3ali04w.exe /VERYSILENT /DIR=%PACKAGEPATH% /EXTRACT="YES"</ExtractCommand>
  <ReleaseDate>2022-05-30</ReleaseDate>
  <DiskSpaceNeeded>4531941</DiskSpaceNeeded>
  <Reboot type="3" />
  <Brand type="2" />
  <PackageType type="2" />
  <Install rc="0,1" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\dpinst.exe /s /sa</Cmdline>
  </Install>
  <Uninstall />
  <DetectInstall>
    <And>
      <_Driver>
        <HardwareID><![CDATA[ACPI\INTC1055]]></HardwareID>
        <Date>2021-11-22</Date>
        <Version>30.100.2148.1^</Version>
      </_Driver>
      <_Driver>
        <HardwareID><![CDATA[PCI\VEN_8086&DEV_51E8]]></HardwareID>
        <Date>2021-11-22</Date>
        <Version>30.100.2148.1^</Version>
      </_Driver>
    </And>
  </DetectInstall>
  <Files>
    <Installer>
      <File>
        <Name>n3ali04w.exe</Name>
        <CRC>7B523EA91EF8B959EEE41ABD621E04BF1306BE8B26B5AEF05639F9382BAF0C9C</CRC>
        <Size>1418000</Size>
      </File>
    </Installer>
    <Readme default="EN">
      <File id="EN">
        <Name>n3ali04w.txt</Name>
        <CRC>209847C9C669177119C9936AAA3C62F20602711D600934A03D0C2226339C9852</CRC>
        <Size>7316</Size>
      </File>
    </Readme>
  </Files>
</Package>
=== n3adr51w_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<Package name="VGA_N3ADR" id="n3adr51w" version="32.0.101.7026" hide="False" pcdDescriptor="0230">
  <Title default="EN">
    <Desc id="EN">Intel Graphics Driver - 10 (22H2 or Later)/11 (21H2 or Later)</Desc>
  </Title>
  <DetectVersion type="Inf">
    <_PnPID><![CDATA[PCI\VEN_8086&DEV_4626]]></_PnPID>
    <_PnPID><![CDATA[PCI\VEN_8086&DEV_46B3]]></_PnPID>
    <_PnPID><![CDATA[PCI\VEN_8086&DEV_46A3]]></_PnPID>
  </DetectVersion>
  <Summary default="EN" />
  <Severity type="1" />
  <Vendor>Lenovo</Vendor>
  <ExtractCommand>n3adr51w.exe /VERYSILENT /DIR=%PACKAGEPATH% /EXTRACT="YES"</ExtractCommand>
  <ReleaseDate>2025-09-28</ReleaseDate>
  <DiskSpaceNeeded>3963914406</DiskSpaceNeeded>
  <Reboot type="3" />
  <Brand type="2" />
  <PackageType type="2" />
  <Install rc="0,259,3010" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\n3adr51w.exe /verysilent /DIR=%PACKAGEPATH%\TMP</Cmdline>
  </Install>
  <Uninstall />
  <DetectInstall>
    <_Driver>
      <HardwareID><![CDATA[PCI\VEN_8086&DEV_46A3]]></HardwareID>
      <HardwareID><![CDATA[PCI\VEN_8086&DEV_46A6]]></HardwareID>
      <HardwareID><![CDATA[PCI\VEN_8086&DEV_46A8]]></HardwareID>
      <HardwareID><![CDATA[PCI\VEN_8086&DEV_4626]]></HardwareID>
      <HardwareID><![CDATA[PCI\VEN_8086&DEV_4628]]></HardwareID>
      <HardwareID><![CDATA[PCI\VEN_8086&DEV_46B3]]></HardwareID>
      <Date>2025-08-19</Date>
      <Version>32.0.101.7026^</Version>
    </_Driver>
  </DetectInstall>
  <Files>
    <Installer>
      <File>
        <Name>n3adr51w.exe</Name>
        <CRC>E82E38CD4F724E03B602AEBC9D470B8E170032655F293579B01D336A1135EF14</CRC>
        <Size>641067504</Size>
      </File>
    </Installer>
    <Readme default="EN">
      <File id="EN">
        <Name>n3adr51w.html</Name>
        <CRC>2EB69B0E24DC0060B557650A40391A448B1C2F3905E547F070ABB2102169B0CE</CRC>
        <Size>55763</Size>
      </File>
    </Readme>
  </Files>
</Package>
=== n3acv43w_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<Package name="FWU_CV_N3ACV" id="n3acv43w" version="1.40.605.1122" hide="False" pcdDescriptor="0230">
  <Title default="EN">
    <Desc id="EN">Lenovo Computer Vision Firmware - 10 (20H2 or Later)/11 (21H2 or Later)</Desc>
  </Title>
  <DetectVersion type="File" show="False">
    <File>%WINDOWS%\explorer.exe</File>
  </DetectVersion>
  <Summary default="EN" />
  <Severity type="2" />
  <Vendor>Lenovo</Vendor>
  <ExtractCommand>n3acv43w.exe /VERYSILENT /DIR=%PACKAGEPATH% /EXTRACT="YES"</ExtractCommand>
  <ReleaseDate>2025-04-29</ReleaseDate>
  <DiskSpaceNeeded>234040296</DiskSpaceNeeded>
  <Reboot type="1" />
  <Brand type="2" />
  <PackageType type="4" />
  <Install rc="0" rccancel="20" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\Lenovo01w_update.exe /tvsuupdate</Cmdline>
  </Install>
  <Uninstall />
  <DetectInstall />
  <Files>
    <Installer>
      <File>
        <Name>n3acv43w.exe</Name>
        <CRC>168ECFA6631264ABE07D7378B733792A6FF04A0DAEC640B2905436AE74D5BB16</CRC>
        <Size>44932760</Size>
      </File>
    </Installer>
    <Readme default="EN">
      <File id="EN">
        <Name>n3acv43w.html</Name>
        <CRC>527855093C7FE0E560E55FF0D3E56570E2177B0FDEB4E9CA1B07CB15C52E4A78</CRC>
        <Size>38904</Size>
      </File>
    </Readme>
    <External>
      <File>
        <Name>mcufwrevcheck.exe</Name>
        <CRC>757167D9F7A4283234752CE6A75C6126BF53FD07149416C62D91DEF7CCFC641E</CRC>
        <Size>47392</Size>
      </File>
      <File>
        <Name>lsccspi01w_version_01.exe</Name>
        <CRC>922AB7B3C8A4C0AEEE4E74325D9B421415CE5BEAA3233C07631C9101DF56E373</CRC>
        <Size>363296</Size>
      </File>
      <File>
        <Name>lsccspi01w_version_n3acv43w.xml</Name>
        <CRC>A6AAC25FB8D55B63A9E676264640CB0A65F4A614766AF33DCD6D7A61E85C5BBF</CRC>
        <Size>1933</Size>
      </File>
    </External>
  </Files>
</Package>
=== r2slf18w_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<Package name="LVD_R1SLF" id="r2slf18w" version="4.3.0.3" hide="False" pcdDescriptor="0230">
  <Title default="EN">
    <Desc id="EN">Lenovo View Driver - 10 /11 (22H2 or Later)</Desc>
  </Title>
  <DetectVersion type="Registry">
    <Registry>
      <Key>HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\{7F644C1A-51CF-4C10-9268-87C4B97692D9}\LVFSettings</Key>
      <ValueName>LvfDriverVer</ValueName>
    </Registry>
  </DetectVersion>
  <Summary default="EN" />
  <Severity type="1" />
  <Vendor>Lenovo</Vendor>
  <ExtractCommand>r2slf18w.exe /VERYSILENT /DIR=%PACKAGEPATH% /EXTRACT="YES"</ExtractCommand>
  <ReleaseDate>2026-04-07</ReleaseDate>
  <DiskSpaceNeeded>76471947</DiskSpaceNeeded>
  <Reboot type="3" />
  <Brand type="2" />
  <PackageType type="2" />
  <Install rc="0,259,3010" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\r2slf18w.exe /verysilent /DIR=%PACKAGEPATH%\TMP</Cmdline>
  </Install>
  <Uninstall />
  <DetectInstall>
    <_RegistryKeyValue type="REG_SZ">
      <Key>HKEY_LOCAL_MACHINE\SOFTWARE\Classes\CLSID\{7F644C1A-51CF-4C10-9268-87C4B97692D9}\LVFSettings</Key>
      <KeyName>LvfDriverVer</KeyName>
      <Version>4.3.0.3^</Version>
    </_RegistryKeyValue>
  </DetectInstall>
  <Files>
    <Installer>
      <File>
        <Name>r2slf18w.exe</Name>
        <CRC>41DA8A6301EA8CFBF45AAA078344463DA116EC2C91B049613F090C986F9D9F9D</CRC>
        <Size>25005608</Size>
      </File>
    </Installer>
    <Readme default="EN">
      <File id="EN">
        <Name>r2slf18w.html</Name>
        <CRC>76BAE7AA4E13F2E178AD123C0774C130763C87449B45DE6C8F2E7773F783150E</CRC>
        <Size>113135</Size>
      </File>
    </Readme>
  </Files>
</Package>
=== gkcn65ww_2_.xml
<?xml version="1.0" encoding="utf-8"?>
<Package name="BIOS_GKCN" id="gkcn65ww" version="GKCN65WW" hide="False" pcdDescriptor="0230">
  <Title default="EN">
    <Desc id="EN">BIOS Update Utility - 10/11</Desc>
  </Title>
  <DetectVersion type="File" show="False">
    <File>%WINDOWS%\explorer.exe</File>
  </DetectVersion>
  <Vendor>Lenovo</Vendor>
  <ReleaseDate>2024-05-22</ReleaseDate>
  <PackageType type="3" />
  <Install rc="0,1073807364" rccancel="259,-1,-10" type="cmd" default="EN">
    <Cmdline id="EN">%PACKAGEPATH%\GKCN65WW.exe /verysilent /PARAM="-s"</Cmdline>
  </Install>
  <DetectInstall>
    <_Bios>
      <Level>GKCN65*</Level>
    </_Bios>
  </DetectInstall>
  <Dependencies>
    <And>
      <_Bios>
        <Level>GKCN64W*</Level>
        <Level>GKCN60W*</Level>
      </_Bios>
    </And>
  </Dependencies>
  <Files>
    <Installer>
      <File>
        <Name>GKCN65WW.exe</Name>
        <CRC>320B5022B834EDF22CB666363A4863D806DED90D11D9A5A761E4E7351BF79BED</CRC>
        <Size>15029535</Size>
      </File>
    </Installer>
  </Files>
</Package>"####;
    /// The readme of the BIOS package, cut down to the parts read.
    const README: &str = r####"<!DOCTYPE html>
<html lang="en">
<head><meta charset="utf-8"><title>ReadMe</title></head>
<body>
<div id="readmeContent">
<button class="collapsible" type="button" data-toggle="collapse" data-target="#collapseExample"
            aria-expanded="true" aria-controls="collapseExample">
            Changes in This Release <i class="gg-chevron"></i>
        </button>
        <div class="collapse" id="collapseExample">
            <div class="card card-body"><span style="font-family: Verdana; font-size: 11pt;"><p><span style="font-size: 12pt; font-family: Verdana;">Version:&nbsp;</span></p><p><span style="font-size: 12pt; font-family: Verdana;">&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;1.56&nbsp;(UEFI&nbsp;BIOS)</span></p><p><span style="font-size: 12pt; font-family: Verdana;">&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;&nbsp;1.26&nbsp;(ECP)</span></p><p><br></p><p><span style="font-size: 12pt; font-family: Verdana;">[Important&nbsp;updates]</span></p><p><span style="font-size: 12pt; font-family: Verdana;">-&nbsp;Corrected&nbsp;Security&nbsp;fix&nbsp;CVE&nbsp;numbers:&nbsp;CVE-2026-46658,CVE-2026-46659.</span></p><p><br></p><p><span style="font-size: 12pt; font-family: Verdana;">[New&nbsp;functions&nbsp;or&nbsp;enhancements]</span></p><p><span style="font-size: 12pt; font-family: Verdana;">&nbsp;&nbsp;Nothing.</span></p><p><br></p><p><span style="font-size: 12pt; font-family: Verdana;">[Problem&nbsp;fixes]</span></p><p><span style="font-size: 12pt; font-family: Verdana;">&nbsp;&nbsp;Nothing.</span></p><p><br></p></span></div>
        </div>
      
      <button class="collapsible" type="button" data-toggle="collapse" 
      data-target="#collapseExample" aria-expanded="true" aria-controls="collapseExample">
      Version Information <i class="gg-chevron"></i>
      </button>
      <div class="collapse" id="collapseExample">
          <div class="card card-body">
              <span style="font-family: Verdana; font-size: 11pt;"><p><span style="font-size: 11pt; font-family: Verdana;">The&nbsp;following&nbsp;versions&nbsp;of&nbsp;UEFI&nbsp;BIOS&nbsp;and&nbsp;ECP&nbsp;(Embedded&nbsp;Controller&nbsp;Program)&nbsp;have&nbsp;been&nbsp;released&nbsp;to&nbsp;date.</span></p><p><br></p><table><tbody><tr><td style="height: 14pt; width: 149px;"><p data-cellwidth="149px" data-cellheight="14pt"><span style="font-size: 11pt; font-family: Verdana;">Package&nbsp;ID&nbsp;</span></p></td><td style="height: 14pt; width: 204px;"><p data-cellwidth="204px" data-cellheight="14pt"><span style="font-size: 11pt; font-family: Verdana;">UEFI&nbsp;BIOS&nbsp;(BIOS&nbsp;ID)</span></p></td><td style="height: 14pt; width: 204px;"><p data-cellwidth="204px" data-cellheight="14pt"><span style="font-size: 11pt; font-family: Verdana;">ECP&nbsp;(ECP&nbsp;ID)</span></p></td><td style="height: 14pt; width: 58px;"><p data-cellwidth="58px" data-cellheight="14pt"><span style="font-size: 11pt; font-family: Verdana;">Rev.</span></p></td><td style="height: 14pt; width: 149px;"><p data-cellwidth="149px" data-cellheight="14pt"><span style="font-size: 11pt; font-family: Verdana;">Issue&nbsp;Date</span></p></td></tr><tr><td style="width: 149px;"><p data-cellwidth="149px"><span style="font-size: 11pt; font-family: Verdana;">N3AUJ35W</span></p></td><td style="width: 204px;"><p data-cellwidth="204px"><span style="font-size: 11pt; font-family: Verdana;">1.56(N3AET91W)</span></p></td><td style="width: 204px;"><p data-cellwidth="204px"><span style="font-size: 11pt; font-family: Verdana;">1.26(N3AHT54W)</span></p></td><td style="width: 58px;"><p data-cellwidth="58px"><span style="font-size: 11pt; font-family: Verdana;">2</span></p></td><td style="width: 149px;"><p data-cellwidth="149px"><span style="font-size: 11pt; font-family: Verdana;">2026/08/31</span></p></td></tr><tr><td style="width: 149px;"><p data-cellwidth="149px"><span style="font-size: 11pt; font-family: Verdana;">N3AUJ34W</span></p></td><td style="width: 204px;"><p data-cellwidth="204px"><span style="font-size: 11pt; font-family: Verdana;">1.55(N3AET90W)</span></p></td><td style="width: 204px;"><p data-cellwidth="204px"><span style="font-size: 11pt; font-family: Verdana;">1.26(N3AHT54W)</span></p></td><td style="width: 58px;"><p data-cellwidth="58px"><span style="font-size: 11pt; font-family: Verdana;">1</span></p></td><td style="width: 149px;"><p data-cellwidth="149px"><span style="font-size: 11pt; font-family: Verdana;">2026/04/28</span></p></td></tr><tr><td style="width: 149px;"><p data-cellwidth="149px"><span style="font-size: 11pt; font-family: Verdana;">N3AUJ33W</span></p></td><td style="width: 204px;"><p data-cellwidth="204px"><span style="font-size: 11pt; font-family: Verdana;">1.54(N3AET89W)</span></p></td><td style="width: 204px;"><p data-cellwidth="204px"><span style="font-size: 11pt; font-family: Verdana;">1.26(N3AHT54W)</span></p></td><td style="width: 58px;"><p data-cellwidth="58px"><span style="font-size: 11pt; font-family: Verdana;">1</span></p></td><td style="width: 149px;"><p data-cellwidth="149px"><span style="font-size: 11pt; font-family: Verdana;">2026/03/09</span></p></td></tr></tbody></table></div></div>
</div>
</body>
</html>"####;

    fn board(product: &str, sku: &str, bios: &str) -> Board {
        Board { maker: "LENOVO".into(), product: product.into(), bios: bios.into(), bios_date: Some("2026.06.09".into()), system_maker: "LENOVO".into(), system_product: product.into(), system_sku: sku.into(), system_family: "ThinkPad X1 Carbon Gen 10".into(), ..Default::default() }
    }

    fn fixture(file: &str) -> Pkg {
        let part = PACKAGES.split("=== ").find(|p| p.starts_with(file)).unwrap_or_else(|| panic!("no {file}"));
        parse_package(part.split_once('\n').unwrap().1).unwrap()
    }

    /// The catalog's packages with their descriptors, as the fixture has them.
    fn pkgs() -> Vec<(Entry, Pkg)> {
        parse_catalog(CATALOG_XML).unwrap().into_iter().map(|e| {
            let file = e.url.rsplit('/').next().unwrap().to_string();
            let p = fixture(&file);
            (e, p)
        }).collect()
    }

    fn no_registry(_: &str, _: &str) -> Option<String> {
        None
    }

    fn find<'a>(l: &'a [MakerDriver], id: &str) -> &'a MakerDriver {
        l.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("no {id} in {:?}", l.iter().map(|d| &d.id).collect::<Vec<_>>()))
    }

    #[test]
    fn the_machine_type_is_the_first_four_characters() {
        assert_eq!(machine_type(&board("21CBCTO1WW", "", "x")).as_deref(), Some("21CB"));
        assert_eq!(machine_type(&board("21cbs0v300", "", "x")).as_deref(), Some("21CB"));
        assert_eq!(machine_type(&board("ThinkPad X1", "LENOVO_MT_21CB_BU_Think_FM_ThinkPad X1 Carbon Gen 10", "x")).as_deref(), Some("21CB"));
        assert_eq!(machine_type(&board("82JU", "", "x")).as_deref(), Some("82JU"));
        // Not a machine type: no letters-only name.
        assert_eq!(machine_type(&board("ThinkPad X1 Carbon", "", "x")), None);
        assert_eq!(machine_type(&board("", "", "x")), None);
        assert_eq!(catalog_url("21CB", true), "https://download.lenovo.com/catalog/21CB_Win11.xml");
        assert_eq!(catalog_url("21CB", false), "https://download.lenovo.com/catalog/21CB_Win10.xml");
    }

    #[test]
    fn the_catalog_lists_descriptors_on_lenovos_server_only() {
        let l = parse_catalog(CATALOG_XML).unwrap();
        assert_eq!(l.len(), 7, "the package from another server is left out");
        assert_eq!((l[0].url.as_str(), l[0].category.as_str()), ("https://download.lenovo.com/pccbbs/mobiles/n3auj35w_2_.xml", "BIOS UEFI"));
        assert_eq!(l[0].sha256.len(), 64);
        assert!(parse_catalog("<packages/>").is_err());
        assert!(parse_catalog("nope").is_err());
        assert_eq!(sha256_hex(b"abc"), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn a_descriptor_gives_version_hardware_ids_and_the_installer() {
        let p = fixture("n3awl20w");
        assert_eq!((p.id.as_str(), p.version.as_str(), p.kind), ("n3awl20w", "24.30.1.1", 2));
        assert_eq!(p.drivers, vec![("24.30.1.1".to_string(), vec!["PCI\\VEN_8086&DEV_51F0&SUBSYS_00908086".to_string()])]);
        assert_eq!(p.date, chrono::NaiveDate::from_ymd_opt(2026, 5, 20));
        assert_eq!(p.install_rc, vec![0, 259, 3010]);
        let (name, sum, size) = p.file.clone().unwrap();
        assert_eq!((name.as_str(), sum.len(), size.is_some()), ("n3awl20w.exe", 64, true));
        assert!(parse_package("<Package/>").is_err());
        assert!(parse_package("<Other id=\"a\" version=\"1\"/>").is_err());
    }

    #[test]
    fn audio_with_two_driver_versions_is_listed_for_each() {
        let l = driver_list("21CB", &pkgs(), &no_registry);
        let audio: Vec<_> = l.iter().filter(|d| d.kind == "Audio").collect();
        assert_eq!(audio.len(), 2, "{audio:?}");
        let codec = audio.iter().find(|d| d.version == "6.0.9764.1").unwrap();
        assert_eq!(codec.id, "n3aa123w");
        // A codec is written without its bus in the catalog; Windows names it with HDAUDIO.
        assert!(codec.hardware_ids.contains(&"HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257".to_string()), "{:?}", codec.hardware_ids);
        assert!(hw_match("HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257", "HDAUDIO\\FUNC_01&VEN_10EC&DEV_0257&SUBSYS_17AA2314&REV_1000"));
        let sst = audio.iter().find(|d| d.version == "10.29.00.8582").unwrap();
        assert_eq!(sst.id, "n3aa123w:10.29.00.8582");
        assert!(sst.hardware_ids.iter().all(|i| i.starts_with("PCI\\VEN_8086&DEV_")) && sst.hardware_ids.len() == 14);
    }

    #[test]
    fn titles_kinds_dates_and_sizes_come_from_the_descriptor() {
        let l = driver_list("21CB", &pkgs(), &no_registry);
        let w = find(&l, "n3awl20w");
        assert_eq!((w.name.as_str(), w.kind.as_str(), w.date.as_deref()), ("Intel AX211 Wireless LAN Driver", "Network", Some("2026.05.20")));
        assert_eq!(w.size, Some(fixture("n3awl20w").file.unwrap().2.unwrap()));
        assert_eq!(w.page.as_deref(), Some("https://pcsupport.lenovo.com/us/en/search?query=21CB"));
        assert_eq!(find(&l, "n3adr51w").kind, "Graphics");
        assert_eq!(find(&l, "n3ali04w").kind, "Chipset");
        assert_eq!(find(&l, "n3acv43w").kind, "Firmware");
        // The BIOS is not in the list.
        assert!(!l.iter().any(|d| d.id == "n3auj35w"));
        assert_eq!(clean_title("Fibocom L860-GL-16 Software - 11 (22H2 or Later)"), "Fibocom L860-GL-16 Software");
        assert_eq!(clean_title("Lenovo View Driver - 10 /11 (22H2 or Later)"), "Lenovo View Driver");
        assert_eq!(clean_title("ThinkPad Thunderbolt 3 dock Gen2 Firmware"), "ThinkPad Thunderbolt 3 dock Gen2 Firmware");
    }

    #[test]
    fn a_tool_detected_through_the_registry_gets_its_installed_version() {
        let seen = std::cell::RefCell::new(Vec::new());
        let read = |k: &str, v: &str| -> Option<String> {
            seen.borrow_mut().push((k.to_string(), v.to_string()));
            Some("4.2.0.1".into())
        };
        let l = driver_list("21CB", &pkgs(), &read);
        let t = find(&l, "r2slf18w");
        assert_eq!((t.version.as_str(), t.installed.as_deref(), t.hardware_ids.len()), ("4.3.0.3", Some("4.2.0.1"), 0));
        assert_eq!(seen.borrow().len(), 1);
        assert!(seen.borrow()[0].0.starts_with("HKEY_LOCAL_MACHINE\\SOFTWARE\\Classes\\CLSID\\"));
        // Not installed (or not a version): none.
        assert_eq!(find(&driver_list("21CB", &pkgs(), &no_registry), "r2slf18w").installed, None);
        assert_eq!(find(&driver_list("21CB", &pkgs(), &|_: &str, _: &str| Some("garbage".to_string())), "r2slf18w").installed, None);
        // The real registry: a key every Windows has.
        assert!(read_registry("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "CurrentBuildNumber").is_some_and(|b| b.parse::<u32>().is_ok()));
        assert_eq!(read_registry("HKEY_LOCAL_MACHINE\\SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", "NoSuchValue"), None);
        assert_eq!(read_registry("HKEY_BAD\\x", "y"), None);
    }

    #[test]
    fn a_package_that_runs_its_own_installer_is_installed_checked_and_signed() {
        let l = driver_list("21CB", &pkgs(), &no_registry);
        let w = find(&l, "n3awl20w");
        let i = w.install.as_ref().unwrap();
        assert_eq!(i.url, "https://download.lenovo.com/pccbbs/mobiles/n3awl20w.exe");
        assert_eq!((i.signer.as_str(), i.inner.clone(), i.md5.clone()), ("Lenovo", None, None));
        assert!(i.sha256.as_deref().is_some_and(|s| s.len() == 64 && s == s.to_ascii_lowercase()));
        // The catalog's "/DIR=%PACKAGEPATH%\TMP" is a folder next to the download.
        assert_eq!(i.args[0], "/verysilent");
        let dir = package_dir("n3awl20w");
        assert_eq!(i.args[1], format!("/DIR={}\\TMP", dir.display()));
        assert_eq!(i.ok_codes, vec![0, 259, 3010]);
        let plan = plan_install("driver:maker:lenovo:n3awl20w", &w.name, w).unwrap();
        assert_eq!(plan.downloads[0].sha256.as_deref(), i.sha256.as_deref());
        assert!(plan.downloads[0].dest.starts_with(&dir));
        assert!(matches!(&plan.steps[0], crate::sources::Step::CheckSigner { signer, .. } if signer == "Lenovo"));
        let crate::sources::Step::Run { args, elevated, .. } = &plan.steps[1] else { panic!() };
        assert!(*elevated && args == &i.args);
        assert!(host_is(&i.url, &["lenovo.com"]));
    }

    #[test]
    fn a_package_that_needs_unpacking_first_or_is_firmware_is_not_installed_here() {
        let l = driver_list("21CB", &pkgs(), &no_registry);
        // dpinst.exe /s inside the unpacked download.
        assert!(find(&l, "n3ali04w").install.is_none());
        // Firmware.
        assert!(find(&l, "n3acv43w").install.is_none());
        let p = fixture("n3awl20w");
        // A command for another program, without switches, or with an unknown folder.
        let mut x = p.clone();
        x.install_cmd = "%PACKAGEPATH%\\setup.exe /s".into();
        assert!(install_of(&x, "https://download.lenovo.com/pccbbs/mobiles/n3awl20w_2_.xml", "n3awl20w").is_none());
        x.install_cmd = "%PACKAGEPATH%\\n3awl20w.exe".into();
        assert!(install_of(&x, "https://download.lenovo.com/pccbbs/mobiles/n3awl20w_2_.xml", "n3awl20w").is_none());
        x.install_cmd = "%PACKAGEPATH%\\n3awl20w.exe /s /LOG=%TEMP%\\x.log".into();
        assert!(install_of(&x, "https://download.lenovo.com/pccbbs/mobiles/n3awl20w_2_.xml", "n3awl20w").is_none());
        // Without a checksum.
        x.install_cmd = p.install_cmd.clone();
        x.file = Some(("n3awl20w.exe".into(), "".into(), None));
        assert!(install_of(&x, "https://download.lenovo.com/pccbbs/mobiles/n3awl20w_2_.xml", "n3awl20w").is_none());
    }

    #[test]
    fn lenovo_bios_versions_compare_by_number_or_id() {
        // A ThinkPad: Lenovo's number against the number in the installed BIOS version.
        assert_eq!(newer("1.56", Some("N3AET91"), "N3AET78W (1.52 )"), Some(true));
        assert_eq!(newer("1.56", None, "N3AET91W (1.56 )"), Some(false));
        // Installed newer than the list.
        assert_eq!(newer("1.56", Some("N3AET91"), "N3AET99W (1.64 )"), Some(false));
        assert_eq!(newer("1.9", None, "N3AET78W (1.10 )"), Some(false));
        // Only the BIOS id installed: compared with the id of the package.
        assert_eq!(newer("1.56", Some("N3AET91"), "N3AET78W"), Some(true));
        assert_eq!(newer("1.56", Some("N3AET91"), "N3AET91W"), Some(false));
        // Another family.
        assert_eq!(newer("1.56", Some("N3AET91"), "R1AET60W"), None);
        assert_eq!(newer("1.56", None, "N3AET78W"), None);
        // A consumer PC: the BIOS id is the version.
        assert_eq!(newer("GKCN65WW", None, "GKCN60WW"), Some(true));
        assert_eq!(newer("GKCN65WW", None, "GKCN65WW"), Some(false));
        assert_eq!(newer("GKCN65WW", None, "GKCN70WW"), Some(false));
        assert_eq!(newer("GKCN65WW", None, "N3AET78W (1.52 )"), None);
        assert_eq!(newer("GKCN65WW", None, ""), None);
        assert_eq!(level_of("N3AET91W"), Some(("N3AET".into(), 91)));
        assert_eq!(level_of("N3AET3*"), Some(("N3AET".into(), 3)));
        assert_eq!(level_of("GKCN65WW"), Some(("GKCN".into(), 65)));
        assert_eq!(level_of("1.56"), None);
    }

    #[test]
    fn the_bios_of_the_pcs_family_is_chosen_with_its_notes_and_id() {
        let list = bios_of(&pkgs().into_iter().map(|(_, p)| p).collect::<Vec<_>>());
        assert_eq!(list.len(), 1, "one BIOS package in the catalog");
        let mut b = list[0].clone();
        assert_eq!((b.version.as_str(), b.hint.as_deref(), b.date.as_deref()), ("1.56", Some("N3AET91"), Some("2026.08.11")));
        // From the readme: the id of the version and the changes.
        b.level = level_in_readme(README, "1.56");
        b.notes = readme_notes(README);
        assert_eq!(b.level.as_deref(), Some("N3AET91W"));
        assert_eq!(b.notes, vec!["Corrected Security fix CVE numbers: CVE-2026-46658,CVE-2026-46659."]);
        assert_eq!(level_in_readme(README, "1.5"), None, "1.5 is not 1.56");
        assert_eq!(shown(&b), "N3AET91W (1.56)");
        let data = Lookup::Found(vec![b]);
        let me = board("21CBCTO1WW", "", "N3AET78W (1.52 )");
        let Answer::Listed(l) = answer(&me, Some(&data)) else { panic!() };
        assert_eq!((l.version.as_str(), l.newer, l.date.as_deref()), ("N3AET91W (1.56)", Some(true), Some("2026.08.11")));
        assert_eq!(l.page.as_deref(), Some("https://pcsupport.lenovo.com/us/en/search?query=21CB"));
        assert_eq!(l.notes.len(), 1);
        let Answer::Listed(cur) = answer(&board("21CBCTO1WW", "", "N3AET91W (1.56 )"), Some(&data)) else { panic!() };
        assert_eq!(cur.newer, Some(false));
        // Another BIOS family than the package's: not comparable.
        let Answer::Listed(other) = answer(&board("21CBCTO1WW", "", "R1AET60W (1.10 )"), Some(&data)) else { panic!() };
        assert_eq!(other.newer, None);
        // As the BIOS entry.
        let e = entry(&BiosEntry { answer: answer(&me, Some(&data)), board: me, maker: Some(Maker::Lenovo) });
        assert_eq!((e.status, e.latest.as_deref()), (AppStatus::Update, Some("N3AET91W (1.56)")));
        assert!(e.vendor_url.as_deref().is_some_and(|u| host_is(u, &["lenovo.com"])));
    }

    #[test]
    fn a_consumer_pc_compares_the_bios_ids() {
        let p = fixture("gkcn65ww");
        let list = bios_of(&[p]);
        assert_eq!((list[0].version.as_str(), list[0].hint.as_deref()), ("GKCN65WW", Some("GKCN65")));
        let data = Lookup::Found(list);
        let Answer::Listed(l) = answer(&board("82JU", "", "GKCN60WW"), Some(&data)) else { panic!() };
        assert_eq!((l.version.as_str(), l.newer), ("GKCN65WW", Some(true)));
        let Answer::Listed(cur) = answer(&board("82JU", "", "GKCN65WW"), Some(&data)) else { panic!() };
        assert_eq!(cur.newer, Some(false));
    }

    #[test]
    fn the_answer_is_missing_when_lenovo_has_nothing_or_was_not_reached() {
        let me = board("21CBCTO1WW", "", "N3AET78W (1.52 )");
        assert!(matches!(answer(&me, None), Answer::Missing { why, page: Some(p) } if why.contains("Could not reach") && p.ends_with("query=21CB")));
        assert!(matches!(answer(&me, Some(&Lookup::NotListed)), Answer::Missing { why, .. } if why.contains("does not list")));
        assert!(matches!(answer(&me, Some(&Lookup::Found(vec![]))), Answer::Missing { why, .. } if why.contains("no BIOS")));
        assert!(matches!(answer(&board("ThinkPad", "", "x"), None), Answer::Missing { page: None, .. }));
    }

    #[test]
    fn readme_notes_keep_the_changes_and_drop_the_empty_sections() {
        let n = readme_notes(README);
        assert_eq!(n, vec!["Corrected Security fix CVE numbers: CVE-2026-46658,CVE-2026-46659."]);
        let two = "<button>Changes in This Release</button><div><p>Version:</p><p>1.0</p><p>[Important updates]</p><p>- Fixes a hang.</p><p>[Problem fixes]</p><p>- Fixes a crash.</p><p>[New functions or enhancements]</p><p>Nothing.</p></div><button>Next</button><p>- not this</p>";
        assert_eq!(readme_notes(two), vec!["## Important updates", "Fixes a hang.", "## Problem fixes", "Fixes a crash."]);
        assert!(readme_notes("<html>no such section</html>").is_empty());
    }

    /// Lenovo's real catalog for a public sample machine type (a ThinkPad X1 Carbon Gen 10): `cargo
    /// test --release --lib live_lenovo -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_lenovo() {
        let b = board("21CBCTO1WW", "", "N3AET78W (1.52 )");
        println!("{:#?}", check(&b).await);
        let list = drivers(&b).await;
        println!("{} entries; {} with hardware ids; {} installable", list.len(), list.iter().filter(|d| !d.hardware_ids.is_empty()).count(), list.iter().filter(|d| d.install.is_some()).count());
        for d in list.iter().take(14) {
            println!("{} | {} | {} | {} | ids {} | install {}", d.id, d.kind, d.name, d.version, d.hardware_ids.len(), d.install.is_some());
        }
        assert!(matches!(check(&b).await, Answer::Listed(_)));
        assert!(!list.is_empty());
        // A machine type Lenovo has no catalog for.
        assert!(catalog("ZZZ9").await.unwrap().is_none());
        // A consumer PC (IdeaPad), whose BIOS version is its BIOS id.
        let idea = board("82JUCTO1WW", "", "GKCN60WW");
        println!("{:?} / {} entries", check(&idea).await, drivers(&idea).await.len());
    }
}
