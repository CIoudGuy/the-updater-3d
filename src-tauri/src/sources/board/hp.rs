//! HP PCs: the newest BIOS, drivers and firmware HP lists for the PC, from the reference files
//! HP Image Assistant reads on hpia.hpcloud.hp.com. HP lists a PC by its platform id (the four
//! hex characters a HP PC reports as its board, "8C5A") and keeps a reference file for it
//! (`ref/8c5a/8c5a_64_10.0.2009.cab` for the platforms up to 2023, `ref/8df7/8df7_64_11.0.cab`
//! for the newer ones; about 30 to 130 KB packed, 0.6 to 4 MB unpacked) with every SoftPaq (BIOS,
//! driver, firmware) of the PC: version, date, checksum, silent install command, the release
//! notes page, and the devices each driver is for. A .cab file with one XML file in it, unpacked
//! with Windows' own expand.exe. Only the platform id goes to HP.

use super::dell::{decode_text, expand_cab};
use super::{Answer, Board, Listed, MakerDriver, MakerInstall};
use crate::http::Parsed;
use anyhow::{bail, Context, Result};
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

const REF: &str = "https://hpia.hpcloud.hp.com/ref/";
/// HP's support start page, for a PC the reference does not list.
const SUPPORT: &str = "https://support.hp.com/us-en/drivers";
/// The reference files are kept for twelve hours, release notes for a month (a SoftPaq never changes).
const AGE: Duration = Duration::from_secs(12 * 3600);
const NOTES_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
/// The signer name of HP's SoftPaqs (read from a downloaded SoftPaq's signature).
const SIGNER: &str = "HP Inc.";
/// The reference files HP keeps for a platform, by the part of the name after `_64_`: one for
/// Windows 11 ("11.0", the only one of a platform newer than 2023) and one for each release of
/// Windows 10 the platform supports ("10.0.2009" for a newer PC, older ones the last they run).
const WINDOWS_11: &str = "11.0";
const WINDOWS_10: [&str; 5] = ["10.0.2009", "10.0.2004", "10.0.1909", "10.0.1903", "10.0.1809"];

static MODELS: Parsed<Lookup> = Parsed::new();

/// A BIOS SoftPaq: HP lists the newest for each BIOS family of a PC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bios {
    /// The BIOS families the SoftPaq is for ("U03", "V95", "W95"); none for the older "F.62" kind
    pub families: Vec<String>,
    pub version: String,
    /// "2026.08.19"
    pub date: Option<String>,
    /// The SoftPaq's release notes page
    pub page: Option<String>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub bios: Vec<Bios>,
    pub drivers: Vec<MakerDriver>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    /// HP has no reference file for this platform
    NotListed,
    Found(Model),
}

/// The platform id HP lists a PC under: the board's four hex characters.
pub fn platform_id(b: &Board) -> Option<String> {
    let s = b.product.trim();
    (s.len() == 4 && s.chars().all(|c| c.is_ascii_hexdigit())).then(|| s.to_ascii_lowercase())
}

pub fn ref_url(pid: &str, release: &str) -> String {
    format!("{REF}{pid}/{pid}_64_{release}.cab")
}

/// The files to try for a platform, the first that exists is used: the one for the PC's Windows
/// first.
pub fn releases(windows11: bool) -> Vec<&'static str> {
    let mut l: Vec<&'static str> = WINDOWS_10.to_vec();
    if windows11 {
        l.insert(0, WINDOWS_11);
    } else {
        l.push(WINDOWS_11);
    }
    l
}

// ---------------------------------------------------------------- versions

fn nums(v: &str) -> Vec<u64> {
    v.split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).filter_map(|s| s.parse().ok()).collect()
}

fn cmp_nums(a: &[u64], b: &[u64]) -> std::cmp::Ordering {
    let n = a.len().max(b.len());
    (0..n).map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0))).find(|o| o.is_ne()).unwrap_or(std::cmp::Ordering::Equal)
}

/// A driver version Windows compares: digits and dots, at least two parts.
fn dotted(v: &str) -> bool {
    let v = v.trim();
    v.chars().all(|c| c.is_ascii_digit() || c == '.') && v.split('.').count() >= 2 && v.split('.').all(|p| !p.is_empty())
}

/// A HP BIOS version: numbers ("02.23.00") or a letter and a number ("F.62").
#[derive(Clone, Debug, PartialEq)]
enum Ver {
    Num(Vec<u64>),
    Letter(char, u64),
}

/// "U03 Ver. 02.13.01" and "L01 v02.59" (family "U03" and "L01", version 02.13.01 and 02.59),
/// "F.62", "F.62 Rev.A".
fn parse_version(v: &str) -> (Option<String>, Option<Ver>) {
    let v = v.trim();
    let tokens: Vec<&str> = v.split_whitespace().collect();
    let is_family = |t: &str| (2..=5).contains(&t.len()) && t.chars().all(|c| c.is_ascii_alphanumeric()) && t.chars().any(|c| c.is_ascii_digit());
    // The word before the number: "Ver." or "v" fused with it ("v02.59").
    let (family, rest) = match tokens.as_slice() {
        [f, w, rest @ ..] if is_family(f) && w.to_ascii_lowercase().starts_with("ver") => (Some(f.to_ascii_uppercase()), rest.first().copied().unwrap_or("").to_string()),
        [f, w, ..] if is_family(f) && w.strip_prefix(['v', 'V']).is_some_and(|r| r.starts_with(|c: char| c.is_ascii_digit())) => (Some(f.to_ascii_uppercase()), w[1..].to_string()),
        _ => (None, v.to_string()),
    };
    let rest = rest.as_str();
    // A revision suffix is not part of the number.
    let rest = rest.split_whitespace().next().unwrap_or("");
    let letter = {
        let b = rest.as_bytes();
        (b.len() > 2 && b[0].is_ascii_alphabetic() && b[1] == b'.' && rest[2..].chars().all(|c| c.is_ascii_digit())).then(|| (b[0].to_ascii_uppercase() as char, rest[2..].parse::<u64>().ok())).and_then(|(c, n)| n.map(|n| Ver::Letter(c, n)))
    };
    if letter.is_some() {
        return (family, letter);
    }
    let num = (dotted(rest)).then(|| Ver::Num(nums(rest)));
    (family, num)
}

/// Is `latest` a newer BIOS than `have` (the version Windows reads: "U03 Ver. 02.13.01")?
/// None when the two are written differently.
pub fn newer(latest: &str, have: &str) -> Option<bool> {
    match (parse_version(latest).1?, parse_version(have).1?) {
        (Ver::Num(a), Ver::Num(b)) => Some(cmp_nums(&a, &b).is_gt()),
        (Ver::Letter(c, a), Ver::Letter(d, b)) if c == d => Some(a > b),
        _ => None,
    }
}

// ---------------------------------------------------------------- release notes

/// The changes a SoftPaq's release notes page lists, as note lines: the page is plain text with
/// line breaks and sections ("ENHANCEMENTS:", "FIXES:"), hard-wrapped at 80 columns.
pub fn release_notes(html: &str) -> Vec<String> {
    let text = regex::Regex::new(r"(?i)<br\s*/?>").unwrap().replace_all(html, "\n");
    let heading = |l: &str| -> Option<String> {
        let (h, _) = l.split_once(':')?;
        (h.len() >= 3 && h.chars().all(|c| c.is_ascii_uppercase() || matches!(c, ' ' | '(' | ')' | '/' | '-' | ',' | '\'' | '.' | '0'..='9'))).then(|| h.trim().to_string())
    };
    let mut out: Vec<String> = Vec::new();
    let mut keep = false;
    // Inside a "- " bullet whose line was wrapped: the next line may carry on.
    let mut in_bullet = false;
    let mut prev_len = 0;
    for raw in text.lines() {
        let line = crate::notes::strip_tags(raw);
        if line.is_empty() {
            in_bullet = false;
            continue;
        }
        if let Some(h) = heading(&line) {
            keep = matches!(h.as_str(), "FIXES" | "ENHANCEMENTS" | "IMPORTANT");
            in_bullet = false;
            if keep {
                let mut t = h.to_ascii_lowercase();
                t[..1].make_ascii_uppercase();
                out.push(format!("## {t}"));
            }
            continue;
        }
        if !keep {
            continue;
        }
        if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix('-')) {
            // A stray closing quote at the end of an item (a typo on some pages).
            let item = item.trim();
            let item = if item.ends_with('"') && item.matches('"').count() % 2 == 1 { item.trim_end_matches('"').trim_end() } else { item };
            out.push(item.to_string());
            in_bullet = !line.ends_with(':');
        } else if in_bullet && prev_len >= 60 && !out.is_empty() {
            let last = out.last_mut().unwrap();
            last.push(' ');
            last.push_str(&line);
            in_bullet = !line.ends_with(':');
        } else {
            out.push(line.clone());
            in_bullet = false;
        }
        prev_len = line.chars().count().max(raw.trim().chars().count());
    }
    crate::notes::tidy_headings(out)
}

// ---------------------------------------------------------------- the reference file

/// One SoftPaq of the reference.
#[derive(Clone, Debug, Default)]
struct Soft {
    id: String,
    name: String,
    /// "BIOS - System Firmware", "Driver - Network", "Firmware"
    category: String,
    version: String,
    date: Option<chrono::NaiveDate>,
    url: Option<String>,
    notes_url: Option<String>,
    sha256: Option<String>,
    md5: Option<String>,
    size: Option<u64>,
    silent: String,
    desc: String,
    os: Vec<String>,
}

fn text(n: roxmltree::Node, name: &str) -> String {
    n.children().find(|c| c.has_tag_name(name)).and_then(|c| c.text()).unwrap_or("").trim().to_string()
}

fn soft(n: roxmltree::Node) -> Option<Soft> {
    let id = text(n, "Id");
    if id.is_empty() {
        return None;
    }
    // "ftp.hp.com/pub/softpaq/sp174001-174500/sp174304.exe" is served over https too.
    let https = |u: String| (!u.is_empty()).then(|| if u.starts_with("http") { u } else { format!("https://{u}") });
    let hash = |t: String| (!t.is_empty()).then(|| t.to_ascii_lowercase());
    Some(Soft {
        id,
        name: text(n, "Name"),
        category: text(n, "Category"),
        version: text(n, "Version"),
        date: chrono::NaiveDate::parse_from_str(&text(n, "DateReleased"), "%Y-%m-%d").ok(),
        url: https(text(n, "Url")),
        notes_url: https(text(n, "ReleaseNotesUrl")),
        sha256: hash(text(n, "SHA256")),
        md5: hash(text(n, "MD5")),
        size: text(n, "Size").parse().ok(),
        silent: text(n, "SilentInstall"),
        desc: text(n, "Description"),
        os: text(n, "SupportedOS").split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
    })
}

/// Is a SoftPaq for Windows 10 or 11 of the PC's kind? Windows 11 is "W11", "W11_24H2"; 64-bit
/// Windows 10 "WT64_2009"; an ARM64 PC "W11A_24H2", "WTARM64".
fn os_ok(s: &Soft, arch: &str) -> bool {
    let arm = arch.eq_ignore_ascii_case("arm64");
    s.os.iter().any(|t| {
        if arm {
            t.starts_with("W11A") || t.starts_with("WTARM64")
        } else {
            (t.starts_with("W11") && !t.starts_with("W11IOT") && !t.starts_with("W11A")) || t.starts_with("WT64")
        }
    })
}

/// The BIOS families in a SoftPaq's name: "HP BIOS and System Firmware (U03)" is ["U03"],
/// "(V95/W95)" and "(V95, W95)" are ["V95", "W95"].
fn families_of(name: &str) -> Vec<String> {
    let Some((_, rest)) = name.rsplit_once('(') else { return vec![] };
    let Some(list) = rest.strip_suffix(')') else { return vec![] };
    let mut out: Vec<String> = list.split(['/', ',', ' ', '&']).map(|f| f.trim().to_ascii_uppercase()).filter(|f| (2..=5).contains(&f.len()) && f.chars().all(|c| c.is_ascii_alphanumeric()) && f.chars().any(|c| c.is_ascii_digit())).collect();
    out.sort();
    out.dedup();
    out
}

fn newest<'a>(items: impl Iterator<Item = &'a Soft>) -> Option<&'a Soft> {
    items.max_by(|a, b| a.date.cmp(&b.date).then(cmp_nums(&nums(&a.version), &nums(&b.version))))
}

fn date_text(s: &Soft) -> Option<String> {
    s.date.map(|d| d.format("%Y.%m.%d").to_string())
}

/// The newest BIOS SoftPaq of each BIOS family. A SoftPaq's name lists the families it is for, and
/// the list changes from one version to the next ("V95", "V95/W95"), so the newest naming each
/// family is taken; the ones without a family are told apart by name.
fn bios_of(softs: &[Soft]) -> Vec<Bios> {
    let all: Vec<&Soft> = softs.iter().filter(|s| s.category.to_ascii_uppercase().starts_with("BIOS")).collect();
    let mut ids: Vec<&str> = Vec::new();
    let codes: std::collections::BTreeSet<String> = all.iter().flat_map(|s| families_of(&s.name)).collect();
    for code in &codes {
        if let Some(s) = newest(all.iter().copied().filter(|s| families_of(&s.name).contains(code))) {
            if !ids.contains(&s.id.as_str()) {
                ids.push(&s.id);
            }
        }
    }
    let names: std::collections::BTreeSet<&str> = all.iter().filter(|s| families_of(&s.name).is_empty()).map(|s| s.name.as_str()).collect();
    for name in names {
        if let Some(s) = newest(all.iter().copied().filter(|s| s.name == name && families_of(&s.name).is_empty())) {
            if !ids.contains(&s.id.as_str()) {
                ids.push(&s.id);
            }
        }
    }
    let mut chosen: Vec<&Soft> = all.iter().copied().filter(|s| ids.contains(&s.id.as_str())).collect();
    chosen.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
    chosen.into_iter().map(|s| Bios { families: families_of(&s.name), version: s.version.clone(), date: date_text(s), page: s.notes_url.clone(), notes: vec![] }).collect()
}

fn kind_of(s: &Soft) -> String {
    let c = s.category.to_ascii_lowercase();
    let n = s.name.to_ascii_lowercase();
    let rest = c.split_once(" - ").map(|(_, r)| r).unwrap_or(&c);
    if c == "firmware" {
        return "Firmware".into();
    }
    match rest {
        "audio" => "Audio",
        "graphics" => "Graphics",
        "network" => {
            if n.contains("bluetooth") {
                "Bluetooth"
            } else {
                "Network"
            }
        }
        "chipset" => "Chipset",
        "storage" => "Storage",
        "keyboard, mouse and input devices" => "Input",
        "enabling" => "Utility",
        _ => "Driver",
    }
    .into()
}

/// Does the SoftPaq give a command that installs it without questions?
fn silent_ok(cmd: &str) -> bool {
    let l = cmd.trim().to_ascii_lowercase();
    !(l.is_empty() || l.contains("not support") || l.contains("do not support") || matches!(l.as_str(), "none" | "na" | "n/a"))
}

/// "PCI\VEN_8086&DEV_02F0" from a device's id: the bus and the hardware id, no instance part.
fn device_id(id: &str) -> Option<String> {
    let id = id.trim();
    if id.is_empty() || id.starts_with('{') {
        return None;
    }
    let mut parts = id.split('\\');
    let (bus, hw) = (parts.next()?, parts.next()?);
    (!matches!(bus.to_ascii_uppercase().as_str(), "SWD" | "ROOT") && !hw.is_empty()).then(|| format!("{bus}\\{hw}").to_ascii_uppercase())
}

fn driver_entries(s: &Soft, devices: Option<&Vec<(String, String)>>) -> Vec<MakerDriver> {
    let driver = s.category.to_ascii_lowercase().starts_with("driver");
    let install = (driver && silent_ok(&s.silent) && s.url.is_some() && (s.sha256.is_some() || s.md5.is_some())).then(|| MakerInstall {
        url: s.url.clone().unwrap_or_default(),
        sha256: s.sha256.clone(),
        md5: s.md5.clone(),
        signer: SIGNER.into(),
        // A SoftPaq is a wrapper: "/s" unpacks it without any window and runs the install its
        // build names ("/e" would only unpack; the wrapper's own usage text); 3010 is "done, restart later".
        args: vec!["/s".into()],
        ok_codes: vec![0, crate::sources::drivers::EXIT_REBOOT],
        inner: None,
    });
    // The devices a driver is for, with the driver version each one ends up with.
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if driver {
        for (v, id) in devices.into_iter().flatten() {
            if dotted(v) {
                by.entry(v.clone()).or_default().push(id.clone());
            }
        }
    }
    for ids in by.values_mut() {
        ids.sort();
        ids.dedup();
    }
    let groups: Vec<(String, Vec<String>)> = if by.is_empty() { vec![(s.version.clone(), vec![])] } else { by.into_iter().collect() };
    let many = groups.len() > 1;
    groups
        .into_iter()
        .map(|(version, hardware_ids)| MakerDriver {
            id: if many && version != s.version { format!("{}:{version}", s.id) } else { s.id.clone() },
            name: s.name.clone(),
            kind: kind_of(s),
            version,
            date: date_text(s),
            page: s.notes_url.clone(),
            notes: Some(s.desc.clone()).filter(|d| !d.is_empty()).into_iter().collect(),
            hardware_ids,
            installed: None,
            size: s.size,
            install: install.clone(),
        })
        .collect()
}

/// What HP's reference file for a platform gives: `arch` is the PC's kind ("x64", "arm64").
pub fn parse(xml: &str, arch: &str) -> Result<Model> {
    let doc = roxmltree::Document::parse(xml).context("HP's reference could not be read")?;
    let softs: Vec<Soft> = doc.descendants().filter(|n| n.has_tag_name("UpdateInfo")).filter_map(soft).collect();
    if softs.is_empty() {
        bail!("HP's reference lists no SoftPaqs");
    }
    // Device by SoftPaq: (the driver version it ends up with, its hardware id).
    let mut devices: HashMap<String, Vec<(String, String)>> = HashMap::new();
    if let Some(list) = doc.root_element().children().find(|n| n.has_tag_name("Devices")) {
        for d in list.children().filter(|n| n.has_tag_name("Device")) {
            let id = device_id(&text(d, "DeviceId")).or_else(|| device_id(&text(d, "HardwareId")));
            let version = text(d, "DriverVersion");
            let Some(id) = id else { continue };
            for r in d.descendants().filter(|n| n.has_tag_name("UpdateInfo")).filter_map(|n| n.attribute("IdRef")) {
                devices.entry(r.to_string()).or_default().push((version.clone(), id.clone()));
            }
        }
    }
    // The newest SoftPaq of each driver or firmware.
    let mut best: BTreeMap<(String, String), &Soft> = BTreeMap::new();
    for s in softs.iter().filter(|s| os_ok(s, arch) && (s.category.to_ascii_lowercase().starts_with("driver") || s.category.eq_ignore_ascii_case("firmware"))) {
        let key = (s.category.clone(), s.name.clone());
        let better = best.get(&key).is_none_or(|b| newest([*b, s].into_iter()).is_some_and(|n| std::ptr::eq(n, s)));
        if better {
            best.insert(key, s);
        }
    }
    let mut drivers: Vec<MakerDriver> = best.values().flat_map(|s| driver_entries(s, devices.get(&s.id))).collect();
    drivers.sort_by(|a, b| (&a.kind, &a.name, &a.id).cmp(&(&b.kind, &b.name, &b.id)));
    Ok(Model { bios: bios_of(&softs), drivers })
}

// ---------------------------------------------------------------- asking HP

/// HP's reference file for a platform: tried in the order of `releases` (None: HP has none).
async fn reference(pid: &str) -> Result<Option<Vec<u8>>> {
    for r in releases(crate::sources::drivers::windows_11()) {
        match crate::http::get_cached(&ref_url(pid, r), &[], AGE).await {
            Ok(f) => return Ok(Some(f.body)),
            // No such file: try the next release. Anything else (offline) stops here.
            Err(e) if e.to_string().contains("HTTP 404") => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(None)
}

pub async fn fetch(pid: &str) -> Result<Lookup> {
    let Some(cab) = reference(pid).await? else { return Ok(Lookup::NotListed) };
    let (key, arch) = (pid.to_string(), crate::winutil::native_arch());
    tokio::task::spawn_blocking(move || {
        MODELS.get_or(&format!("hp-ref:{key}"), &cab, |cab| {
            let xml = decode_text(&expand_cab(cab)?);
            Ok(Lookup::Found(parse(&xml, arch)?))
        })
    })
    .await?
}

/// The BIOS SoftPaq for this PC: the newest of those for its BIOS family; with no family
/// ("F.62") the newest without one. The flag says the two can be compared.
fn choose<'a>(b: &Board, m: &'a Model) -> Option<(&'a Bios, bool)> {
    let pick = |l: Vec<&'a Bios>| l.into_iter().max_by(|a, b| a.date.cmp(&b.date).then(cmp_nums(&nums(&a.version), &nums(&b.version))));
    let (family, _) = parse_version(&b.bios);
    let same: Vec<&Bios> = match &family {
        Some(f) => m.bios.iter().filter(|x| x.families.contains(f)).collect(),
        None => m.bios.iter().filter(|x| x.families.is_empty()).collect(),
    };
    if same.is_empty() { pick(m.bios.iter().collect()).map(|x| (x, false)) } else { pick(same).map(|x| (x, true)) }
}

/// The answer for this PC from HP's reference (None: HP was not reached).
pub fn answer(b: &Board, data: Option<&Lookup>) -> Answer {
    let missing = |why: &str| Answer::Missing { why: why.into(), page: Some(SUPPORT.into()) };
    if platform_id(b).is_none() {
        return missing("This PC does not report the platform id HP lists it under");
    }
    match data {
        None => missing("Could not reach HP"),
        Some(Lookup::NotListed) => missing("HP does not list this PC in its update reference"),
        Some(Lookup::Found(m)) => match choose(b, m) {
            None => missing("HP lists no BIOS for this PC"),
            Some((bios, same)) => Answer::Listed(Listed { version: bios.version.clone(), page: bios.page.clone().or(Some(SUPPORT.into())), date: bios.date.clone(), notes: bios.notes.clone(), newer: if same { newer(&bios.version, &b.bios) } else { None } }),
        },
    }
}

pub async fn check(b: &Board) -> Answer {
    let Some(pid) = platform_id(b) else { return answer(b, None) };
    let mut data = fetch(&pid).await.map_err(|e| log::warn!("hp reference: {e:#}")).ok();
    // The release notes of the BIOS that applies (a small page, kept for a month).
    if let Some(Lookup::Found(m)) = &mut data {
        if let Some((pick, _)) = choose(b, m) {
            let (families, version) = (pick.families.clone(), pick.version.clone());
            if let Some(url) = pick.page.clone().filter(|u| crate::sources::drivers::host_is(u, &["hp.com"])) {
                let notes = crate::http::get_cached(&url, &[], NOTES_AGE).await.map(|f| release_notes(&String::from_utf8_lossy(&f.body))).map_err(|e| log::warn!("hp release notes: {e:#}")).unwrap_or_default();
                if let Some(x) = m.bios.iter_mut().find(|x| x.families == families && x.version == version) {
                    x.notes = notes;
                }
            }
        }
    }
    answer(b, data.as_ref())
}

/// The drivers and firmware HP lists for this PC.
pub async fn drivers(b: &Board) -> Vec<MakerDriver> {
    let Some(pid) = platform_id(b) else { return vec![] };
    match fetch(&pid).await {
        Ok(Lookup::Found(m)) => m.drivers,
        Ok(Lookup::NotListed) => vec![],
        Err(e) => {
            log::warn!("hp reference: {e:#}");
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

    /// HP's reference file for platform 8C5A (a Pro SFF 400 G9), cut down: three BIOS SoftPaqs
    /// of the U03 family, drivers with the devices they are for, and a firmware and a tool.
    const REFERENCE: &str = r####"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<ImagePal>
  <Version>3</Version>
  <HPReference>Yes</HPReference>
  <DateLastModified>20261023</DateLastModified>
  <SystemInfo>
    <System>
      <ProductName>HP Pro SFF 400 G9 Desktop PC</ProductName>
      <SystemID>8C5A</SystemID>
      <BiosVersion2>U03 Ver. 02.13.01</BiosVersion2>
      <Solutions>
        <UpdateInfo IdRef="sp174304" BiosDate="07/27/2026" />
      </Solutions>
    </System>
  </SystemInfo>
  <Devices>
    <Device>
      <ClassName>Extension</ClassName>
      <FriendlyName>Intel(R) Wi-Fi 6 AX201 160MHz</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_02F0</DeviceId>
      <HardwareId />
      <DriverVersion>23.170.1.1</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp165709" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Net</ClassName>
      <FriendlyName>Intel(R) Wi-Fi 6 AX201 160MHz</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_02F0&amp;SUBSYS_00308086</DeviceId>
      <HardwareId />
      <DriverVersion>23.170.1.1</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp165709" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Extension</ClassName>
      <FriendlyName>Realtek(R) Audio</FriendlyName>
      <DeviceId>HDAUDIO\FUNC_01&amp;VEN_10EC&amp;DEV_0215&amp;SUBSYS_103C8585</DeviceId>
      <HardwareId />
      <DriverVersion>17.20.39.4170</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp157440" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Extension</ClassName>
      <FriendlyName>Realtek(R) Audio</FriendlyName>
      <DeviceId>HDAUDIO\FUNC_01&amp;VEN_10EC&amp;DEV_0215&amp;SUBSYS_103C8586</DeviceId>
      <HardwareId />
      <DriverVersion>17.20.39.4170</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp157440" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>MEDIA</ClassName>
      <FriendlyName>Realtek High Definition Audio</FriendlyName>
      <DeviceId>HDAUDIO\FUNC_01&amp;VEN_10EC&amp;DEV_0221&amp;SUBSYS_103C2175</DeviceId>
      <HardwareId />
      <DriverVersion>6.0.9800.1</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp157440" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>MEDIA</ClassName>
      <FriendlyName>Realtek High Definition Audio</FriendlyName>
      <DeviceId>HDAUDIO\FUNC_01&amp;VEN_10EC&amp;DEV_0221&amp;SUBSYS_103C8055</DeviceId>
      <HardwareId />
      <DriverVersion>6.0.9800.1</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp157440" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) PEG10 - 460D</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_460D</DeviceId>
      <HardwareId>pci\ven_8086&amp;dev_460d</HardwareId>
      <DriverVersion>10.1.45.9</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) Host Bridge/DRAM Registers - 4617</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_4617</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.45.9</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) SMBus - 51A3</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_51A3</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.36.7</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) Shared SRAM - 51EF</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_51EF</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.36.7</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) SMBus - 7A23</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_7A23</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.46.5</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) SPI (flash) Controller - 7A24</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_7A24</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.46.5</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) LPC Controller/eSPI Controller - 7A80</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_7A80</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.37.7</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) LPC Controller/eSPI Controller (SuperSKU unlocked) - 7A81</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_7A81</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.37.7</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) PCIe RC 010 G5 - A70D</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_A70D</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.49.10</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>System</ClassName>
      <FriendlyName>Intel(R) PCIe RC 011 G5 - A72D</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_A72D</DeviceId>
      <HardwareId />
      <DriverVersion>10.1.49.10</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp160385" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Bluetooth</ClassName>
      <FriendlyName>Intel(R) Bluetooth(R) PCI Enumerator</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_A876&amp;SUBSYS_00008086</DeviceId>
      <HardwareId />
      <DriverVersion>23.100.1.1</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp156744" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Bluetooth</ClassName>
      <FriendlyName>Intel(R) Bluetooth(R) PCI Enumerator</FriendlyName>
      <DeviceId>PCI\VEN_8086&amp;DEV_A876&amp;SUBSYS_000E8086</DeviceId>
      <HardwareId />
      <DriverVersion>23.100.1.1</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp156744" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Bluetooth</ClassName>
      <FriendlyName>Intel(R) Wireless Bluetooth (R)</FriendlyName>
      <DeviceId>USB\VID_8087&amp;PID_0025&amp;REV_0001</DeviceId>
      <HardwareId />
      <DriverVersion>23.100.0.4</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp156744" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Bluetooth</ClassName>
      <FriendlyName>Intel(R) Wireless Bluetooth (R)</FriendlyName>
      <DeviceId>USB\VID_8087&amp;PID_0025&amp;REV_0002</DeviceId>
      <HardwareId />
      <DriverVersion>23.100.0.4</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp156744" />
      </Solutions>
    </Device>
    <Device>
      <ClassName>Extension</ClassName>
      <FriendlyName>HP Application Driver</FriendlyName>
      <DeviceId>ACPI\HPIC000C</DeviceId>
      <HardwareId />
      <DriverVersion>4.8.125.0</DriverVersion>
      <Solutions>
        <UpdateInfo IdRef="sp165987" />
      </Solutions>
    </Device>
  </Devices>
  <Solutions>
    <UpdateInfo Supersedes="sp170178">
      <Id>sp173363</Id>
      <Name>HP BIOS and System Firmware (U03)</Name>
      <Category>BIOS - System Firmware</Category>
      <Version>02.22.01</Version>
      <Vendor>HP</Vendor>
      <ReleaseType>Critical</ReleaseType>
      <SilentInstall>"HpFirmwareUpdRec.exe" -s -b -r</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp173001-173500/sp173363.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp173001-173500/sp173363.html</ReleaseNotesUrl>
      <MD5>B21902EA2349B8D4174FE4ED59BD10A6</MD5>
      <SHA256>A81650D469E8A2327DC4F9C2E07DABF0B071A8C434EDC707EA78DD2C60A73B9F</SHA256>
      <Size>20213152</Size>
      <DateReleased>2026-06-01</DateReleased>
      <SupportedOS>W11_21H2, W11_22H2, W11_23H2, W11_24H2</SupportedOS>
      <Description>This package is used to update the supported firmware on HP Business Desktop models with a U03 Family BIOS.</Description>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp173363">
      <Id>sp174304</Id>
      <Name>HP BIOS and System Firmware (U03)</Name>
      <Category>BIOS - System Firmware</Category>
      <Version>02.23.00</Version>
      <Vendor>HP</Vendor>
      <ReleaseType>Critical</ReleaseType>
      <SilentInstall>"HpFirmwareUpdRec.exe" -s -b -r</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp174001-174500/sp174304.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp174001-174500/sp174304.html</ReleaseNotesUrl>
      <MD5>03D619B4C2038B604832DE46F0AA564D</MD5>
      <SHA256>81716B2E610A3DC71D2931DD56D9748F4257EC471A8A570997731C08E7969A0F</SHA256>
      <Size>20180824</Size>
      <DateReleased>2026-08-19</DateReleased>
      <SupportedOS>W11_21H2, W11_22H2, W11_23H2, W11_24H2</SupportedOS>
      <Description>This package is used to update the supported firmware on HP Business Desktop models with a U03 Family BIOS.</Description>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp166151">
      <Id>sp170178</Id>
      <Name>HP BIOS and System Firmware (U03)</Name>
      <Category>BIOS - System Firmware</Category>
      <Version>02.21.00</Version>
      <Vendor>HP</Vendor>
      <ReleaseType>Critical</ReleaseType>
      <SilentInstall>"HpFirmwareUpdRec.exe" -s -b -r</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp170001-170500/sp170178.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp170001-170500/sp170178.html</ReleaseNotesUrl>
      <MD5>B1F3CC6CF7DB6516BAAA414D3F949451</MD5>
      <SHA256>C5C9967D4C0EBB32E337A7BC8B11B7232731122D374B5F8F00C614CB420940BE</SHA256>
      <Size>20199736</Size>
      <DateReleased>2026-02-04</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>This package is used to update the supported firmware on HP Business Desktop models with a U03 Family BIOS.</Description>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp157284">
      <Id>sp165709</Id>
      <Name>Intel WLAN Driver</Name>
      <Category>Driver - Network</Category>
      <Version>23.170.1.1</Version>
      <Vendor>Intel</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>HPUP.exe</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp165501-166000/sp165709.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp165501-166000/sp165709.html</ReleaseNotesUrl>
      <MD5>2A23B8B4E7C5676CF9734A551C56365B</MD5>
      <SHA256>AEB0590DF3441039C570BEB2FC05D638CBFD38837349943B3F81FCEF6DAD262E</SHA256>
      <Size>14899624</Size>
      <DateReleased>2025-11-05</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>Intel Wireless LAN drivers are required to enable the integrated Intel Wireless LAN adapter in supported computer models that are running a supported Windows operating system.</Description>
    </UpdateInfo>
    <UpdateInfo>
      <Id>sp157284</Id>
      <Name>Intel WLAN Driver</Name>
      <Category>Driver - Network</Category>
      <Version>23.120.0.3</Version>
      <Vendor>Intel</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>HPUP.exe</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp157001-157500/sp157284.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp157001-157500/sp157284.html</ReleaseNotesUrl>
      <MD5>92CB96CA9D55EEF976B19DF200668516</MD5>
      <SHA256>5D1BD991E1BD19C0FE16C2A6F60469DDB0C57FB3B40FA894B5E95522212CBEA6</SHA256>
      <Size>13909592</Size>
      <DateReleased>2025-03-04</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>Intel Wireless LAN drivers are required to enable the integrated Intel Wireless LAN adapter in supported computer models that are running a supported Windows operating system.</Description>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp153706">
      <Id>sp157440</Id>
      <Name>Realtek HD Audio Driver</Name>
      <Category>Driver - Audio</Category>
      <Version>6.0.9800.1</Version>
      <Vendor>REALTEK</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>HPUP.exe</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp157001-157500/sp157440.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp157001-157500/sp157440.html</ReleaseNotesUrl>
      <MD5>8E6658B2886AC5432FC5FFC90722E165</MD5>
      <SHA256>93DBC35D5E7B69ED8B8765E39FA08E0FA3110E304DDB406F8CEB23D9AC68A61C</SHA256>
      <Size>172271600</Size>
      <DateReleased>2025-03-21</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>This package provides the Realtek High Definition Audio Driver for supported desktop models that are running a supported operating system.</Description>
      <ContentTypes>Uwp</ContentTypes>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp157350">
      <Id>sp160385</Id>
      <Name>Intel Chipset Device Software</Name>
      <Category>Driver - Chipset</Category>
      <Version>10.1.19899.8597</Version>
      <Vendor>Intel</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>HPUP.exe</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp160001-160500/sp160385.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp160001-160500/sp160385.html</ReleaseNotesUrl>
      <MD5>9537C37F37D444771A43C57EFEA87FF7</MD5>
      <SHA256>5D6E9677DD25878299D6DFEC35DF84EA6E823DEE0AA4CFA07C12EE6880EF72D5</SHA256>
      <Size>887680</Size>
      <DateReleased>2025-07-07</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>This package contains the Intel Chipset Installation Utility for supported models running a supported operating system. This utility enables the operating system to show the correct name for the installed Intel hardware in the Microsoft Windows Device Manager.</Description>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp153711">
      <Id>sp156744</Id>
      <Name>Intel Bluetooth Driver</Name>
      <Category>Driver - Network</Category>
      <Version>23.100.1.1</Version>
      <Vendor>Intel</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>HPUP.exe</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp156501-157000/sp156744.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp156501-157000/sp156744.html</ReleaseNotesUrl>
      <MD5>F6D5CD50FC1C5AD13079262944B68B0D</MD5>
      <SHA256>2E106F5E90C2252B2D87A05337228A9DE2E4DD2E86AA779217EBC74B5C7164A3</SHA256>
      <Size>23982832</Size>
      <DateReleased>2025-01-21</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>This package provides the Intel Bluetooth Windows Universal drivers for supported computer models using a supported Windows operating system</Description>
    </UpdateInfo>
    <UpdateInfo>
      <Id>sp149650</Id>
      <Name>Samsung SSD Firmware Update</Name>
      <Category>Firmware</Category>
      <Version>HPS4NHAV</Version>
      <Vendor>Samsung</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>Not support</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp149501-150000/sp149650.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp149501-150000/sp149650.html</ReleaseNotesUrl>
      <MD5>4d75cc9e97ca89b5d95c320e1464fb43</MD5>
      <SHA256>49A1C86288A9A7C4E1C8B87B0B2428DEE6D2EDC077A895C9CB8943756231297F</SHA256>
      <Size>2358856</Size>
      <DateReleased>2023-10-17</DateReleased>
      <SupportedOS>W11, WT32, WT64, WT64_21H2</SupportedOS>
      <Description>This package provides a tool to update the firmware for the Samsung SSD drive in supported computer models using a supported operating system.</Description>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp161848">
      <Id>sp165987</Id>
      <Name>HP Services Scan for Commercial</Name>
      <Category>Driver - Enabling</Category>
      <Version>4.8.125.0</Version>
      <Vendor>HP</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>HPUP.exe</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp165501-166000/sp165987.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp165501-166000/sp165987.html</ReleaseNotesUrl>
      <MD5>E4E3B4020CCE7CF017C22CC0C033D3D9</MD5>
      <SHA256>C43A0423C95CA75FC4DE154547274FBABE69B174D43BB078F9D16C62B25A100F</SHA256>
      <Size>53896248</Size>
      <DateReleased>2025-11-27</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>HP Services Scan is a virtual driver that confirms what the device is entitled to, and downloads and persists applicable software on the device to keep the service running.</Description>
    </UpdateInfo>
    <UpdateInfo Supersedes="sp165719">
      <Id>sp171649</Id>
      <Name>HP PC Hardware Diagnostics UEFI</Name>
      <Category>Diagnostic</Category>
      <Version>10.8.5.0</Version>
      <Vendor>HP</Vendor>
      <ReleaseType>Routine</ReleaseType>
      <SilentInstall>HPUP.exe</SilentInstall>
      <Url>ftp.hp.com/pub/softpaq/sp171501-172000/sp171649.exe</Url>
      <ReleaseNotesUrl>ftp.hp.com/pub/softpaq/sp171501-172000/sp171649.html</ReleaseNotesUrl>
      <MD5>D63E8F157C4F4E80BDD3B9C82B0AE19B</MD5>
      <SHA256>5F96811D43509597BF6B0F143037A73AD6F1C20138802D733FF671C63EBBE890</SHA256>
      <Size>51306048</Size>
      <DateReleased>2026-03-13</DateReleased>
      <SupportedOS>W11, W11_21H2, W11_22H2, W11_23H2</SupportedOS>
      <Description>HP PC Hardware Diagnostics UEFI provides UEFI-based hardware diagnostics used to validate if a system is functioning correctly. Additional support is provided for updating and managing the system BIOS and other device firmware on the system.</Description>
    </UpdateInfo>
  </Solutions>
</ImagePal>"####;
    /// The release notes page of the newest BIOS SoftPaq.
    const NOTES: &str = r####"<html>TITLE:  HP BIOS and System Firmware (U03)<br/>VERSION:  02.23.00 REV:  A PASS:  1<br/>DESCRIPTION:  <br/>This package is used to update the supported firmware on HP Business Desktop<br/>models with a U03 Family BIOS.<br/><br/>PURPOSE:  Critical<br/>SOFTPAQ NUMBER:  sp174304<br/>SOFTPAQ FILE NAME:  sp174304.exe<br/>SOFTPAQ MD5:  03D619B4C2038B604832DE46F0AA564D<br/>SUPERSEDES:  sp173363<br/>EFFECTIVE DATE:  August 19, 2026<br/>CATEGORY:  BIOS - System Firmware<br/>SSM SUPPORTED:  Yes<br/><br/>PRODUCT TYPE(S):<br/>Desktops<br/>HARDWARE PRODUCT MODEL(S):  <br/>HP Pro Tower 400 G9 PCI Desktop PC<br/>HP Pro Tower 480 G9 PCI Desktop PC<br/>HP Pro SFF 400 G9 Desktop PC<br/><br/>OPERATING SYSTEM(S):  <br/>Microsoft Windows 10 version 20H2 (64-bit)<br/>Microsoft Windows 10 version 21H1 (64-bit)<br/>Microsoft Windows 10 version 21H2 (64-bit)<br/>Microsoft Windows 10 version 22H2 (64-bit)<br/>Microsoft Windows 11 version 21H2 (64-bit)<br/>Microsoft Windows 11 version 22H2 (64-bit)<br/>Microsoft Windows 11 version 23H2 (64-bit)<br/>Microsoft Windows 11 version 24H2 (64-bit)<br/>Microsoft Windows 11 version 25H2 (64-bit)<br/><br/>LANGUAGE(S):  Global<br/><br/>WINDOWS UPDATE DRIVER-BIOS NAME LIST:<br/>Driver1=HP Inc. Firmware Driver Update (2.23.0.0)<br/><br/>ENHANCEMENTS:  <br/>- Address security vulnerabilities: CVE-2026-20708, CVE-2026-20715, and<br/>CVE-2026-20734<br/><br/>- Enables 6 GHz WLAN band support in Paraguay for Intel, MediaTek, and Realtek<br/>wireless adapters on supported platforms.<br/>- Implements 2025 HP Secure Erase improvements for supported desktop platforms.<br/>- Updates the Intel microcode to support 12th/13th/14th Generation CPUs.<br/><br/>- System component information:<br/>HP Embedded Controller (EC) / Super I/O (SIO) Firmware, version 12.3.57<br/>Intel GOP Driver, version 17.0.1085<br/>Intel Management Engine Firmware, version 16.1.42.2872<br/>Reference Code, version 0C.00.F6.20<br/><br/>- Includes the following firmware and driver:<br/>Intel/Realtek UEFI PXE ROM EFI Firmware, version 0.2.4<br/>USB Type-C Power Delivery (PD) Firmware, version 3.1.0<br/><br/>Intel MCU version:<br/>m_07_90672_0000003e (updated)<br/>m_32_b0671_00000137 (updated)<br/>CVE NUMBERS:  <br/>CVE-2026-20734<br/>CVE-2026-20715<br/>CVE-2026-20708<br/>PREREQUISITES:  NONE<br/>HOW TO USE:  <br/>1. Download the file by clicking the Download or Obtain Software button and<br/>saving the file to a folder on your hard drive (make a note of the folder where<br/>the downloaded file is saved).<br/><br/>2. Double-click the downloaded file and follow the on-screen instructions<br/><br/>Copyright (c) 2026 HP Development Company, L.P.<br/></html>"####;

    fn board(product: &str, bios: &str) -> Board {
        Board { maker: "HP".into(), product: product.into(), bios: bios.into(), bios_date: Some("2026.06.09".into()), system_maker: "HP".into(), system_product: "HP Pro SFF 400 G9 Desktop PC".into(), ..Default::default() }
    }

    fn model() -> Model {
        parse(REFERENCE, "x64").unwrap()
    }

    fn find<'a>(m: &'a Model, id: &str) -> &'a MakerDriver {
        m.drivers.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("no {id} in {:?}", m.drivers.iter().map(|d| &d.id).collect::<Vec<_>>()))
    }

    #[test]
    fn the_platform_id_is_the_boards_four_hex_characters() {
        assert_eq!(platform_id(&board("8C5A", "x")), Some("8c5a".into()));
        assert_eq!(platform_id(&board(" 8a78 ", "x")), Some("8a78".into()));
        assert_eq!(platform_id(&board("0X8DXD", "x")), None);
        assert_eq!(platform_id(&board("", "x")), None);
        assert_eq!(ref_url("8c5a", "10.0.2009"), "https://hpia.hpcloud.hp.com/ref/8c5a/8c5a_64_10.0.2009.cab");
        assert_eq!(ref_url("8df7", "11.0"), "https://hpia.hpcloud.hp.com/ref/8df7/8df7_64_11.0.cab");
        // Windows 11 PCs try the Windows 11 file first; Windows 10 PCs the newest Windows 10 one.
        assert_eq!(releases(true).first(), Some(&"11.0"));
        assert_eq!((releases(false).first(), releases(false).last()), (Some(&"10.0.2009"), Some(&"11.0")));
        assert_eq!(releases(true).len(), 6);
    }

    #[test]
    fn hp_bios_versions_compare_by_number() {
        assert_eq!(newer("02.23.00", "U03 Ver. 02.13.01"), Some(true));
        assert_eq!(newer("02.23.00", "U03 Ver. 02.23.00"), Some(false));
        // Installed newer than the list.
        assert_eq!(newer("02.22.01", "U03 Ver. 02.23.00"), Some(false));
        assert_eq!(newer("01.12.00", "T76 Ver. 01.09.01"), Some(true));
        assert_eq!(newer("02.10.00", "02.9.00"), Some(true));
        // The older kind: a letter, a dot and a number.
        assert_eq!(newer("F.62", "F.45"), Some(true));
        assert_eq!(newer("F.62 Rev.A", "F.62"), Some(false));
        // Older HP desktops write "v" for the version.
        assert_eq!(newer("02.52", "N06 v02.45"), Some(true));
        assert_eq!(newer("02.52", "N06 v02.52"), Some(false));
        assert_eq!(newer("F.45", "F.62"), Some(false));
        // Written differently: no answer.
        assert_eq!(newer("02.23.00", "F.62"), None);
        assert_eq!(newer("F.62", "U03 Ver. 02.13.01"), None);
        assert_eq!(newer("F.0D", "F.62"), None);
        assert_eq!(newer("02.23.00", ""), None);
        assert_eq!(parse_version("U03 Ver. 02.13.01"), (Some("U03".into()), Some(Ver::Num(vec![2, 13, 1]))));
        assert_eq!(parse_version("F.62"), (None, Some(Ver::Letter('F', 62))));
        assert_eq!(parse_version("N06 v02.52"), (Some("N06".into()), Some(Ver::Num(vec![2, 52]))));
        assert_eq!(parse_version("F.62 Rev.A"), (None, Some(Ver::Letter('F', 62))));
    }

    #[test]
    fn the_bios_of_the_pcs_family_is_chosen_with_its_date_page_and_notes() {
        let m = model();
        assert_eq!(m.bios.len(), 1, "one BIOS family: the newest of the three SoftPaqs");
        let b = &m.bios[0];
        assert_eq!((b.families.clone(), b.version.as_str(), b.date.as_deref()), (vec!["U03".to_string()], "02.23.00", Some("2026.08.19")));
        assert_eq!(b.page.as_deref(), Some("https://ftp.hp.com/pub/softpaq/sp174001-174500/sp174304.html"));
        let Answer::Listed(l) = answer(&board("8C5A", "U03 Ver. 02.13.01"), Some(&Lookup::Found(m.clone()))) else { panic!() };
        assert_eq!((l.version.as_str(), l.newer, l.date.as_deref()), ("02.23.00", Some(true), Some("2026.08.19")));
        let Answer::Listed(cur) = answer(&board("8C5A", "U03 Ver. 02.23.00"), Some(&Lookup::Found(m.clone()))) else { panic!() };
        assert_eq!(cur.newer, Some(false));
        // Another BIOS family than the list has: not comparable.
        let Answer::Listed(other) = answer(&board("8C5A", "S78 Ver. 01.05.00"), Some(&Lookup::Found(m.clone()))) else { panic!() };
        assert_eq!(other.newer, None);
    }

    #[test]
    fn a_bios_name_may_list_several_families_and_they_change_between_versions() {
        assert_eq!(families_of("HP BIOS and System Firmware (U03)"), vec!["U03"]);
        assert_eq!(families_of("HP BIOS and System Firmware (V95/W95)"), vec!["V95", "W95"]);
        assert_eq!(families_of("HP BIOS and System Firmware (V95, W95)"), vec!["V95", "W95"]);
        assert_eq!(families_of("HP BIOS and System Firmware (T70/T85/T87/T88/T89/T99)").len(), 6);
        assert!(families_of("HP System BIOS").is_empty() && families_of("HP BIOS (Intel)").is_empty());
        let soft = |id: &str, name: &str, version: &str, date: &str| Soft { id: id.into(), name: name.into(), category: "BIOS - System Firmware".into(), version: version.into(), date: chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok(), ..Default::default() };
        let list = bios_of(&[
            soft("a", "HP BIOS and System Firmware (V95)", "01.04.00", "2023-08-31"),
            soft("b", "HP BIOS and System Firmware (V95/W95)", "01.15.02", "2026-08-06"),
            soft("c", "HP BIOS and System Firmware (V95, W95)", "01.14.01", "2026-05-20"),
            soft("d", "HP BIOS and System Firmware (X73)", "01.05.03", "2026-09-04"),
            soft("e", "HP System BIOS", "F.60", "2018-01-01"),
            soft("f", "HP System BIOS", "F.62", "2019-01-01"),
        ]);
        // V95 and W95 both: the newest naming them; X73; and the family-less one by name.
        assert_eq!(list.iter().map(|b| (b.families.join("/"), b.version.as_str())).collect::<Vec<_>>(), vec![("V95/W95".to_string(), "01.15.02"), ("X73".to_string(), "01.05.03"), ("".to_string(), "F.62")]);
        let m = Model { bios: list, drivers: vec![] };
        let pick = |have: &str| choose(&board("8C5A", have), &m).map(|(b, same)| (b.version.clone(), same));
        assert_eq!(pick("W95 Ver. 01.10.00"), Some(("01.15.02".into(), true)));
        assert_eq!(pick("V95 Ver. 01.14.01"), Some(("01.15.02".into(), true)));
        assert_eq!(pick("F.45"), Some(("F.62".into(), true)));
        // A family the list does not have: the newest of the list, which does not compare.
        assert_eq!(pick("Z11 Ver. 01.00.00"), Some(("01.05.03".into(), false)));
        assert_eq!(pick(""), Some(("F.62".into(), true)));
    }

    #[test]
    fn the_answer_is_missing_when_hp_has_nothing_or_was_not_reached() {
        let m = Lookup::Found(model());
        assert!(matches!(answer(&board("8C5A", "U03 Ver. 02.13.01"), None), Answer::Missing { why, page: Some(_) } if why.contains("Could not reach")));
        assert!(matches!(answer(&board("8C5A", "U03 Ver. 02.13.01"), Some(&Lookup::NotListed)), Answer::Missing { why, .. } if why.contains("does not list")));
        assert!(matches!(answer(&board("0X8DXD", "U03 Ver. 02.13.01"), Some(&m)), Answer::Missing { .. }));
        assert!(matches!(answer(&board("8C5A", "U03 Ver. 02.13.01"), Some(&Lookup::Found(Model::default()))), Answer::Missing { .. }));
        // As the BIOS entry, with the notes page as its page.
        let b = board("8C5A", "U03 Ver. 02.13.01");
        let e = entry(&BiosEntry { answer: answer(&b, Some(&m)), board: b, maker: Some(Maker::Hp) });
        assert_eq!((e.status, e.latest.as_deref()), (AppStatus::Update, Some("02.23.00")));
        assert!(e.vendor_url.as_deref().is_some_and(|u| host_is(u, &["hp.com"])));
    }

    #[test]
    fn release_notes_become_note_lines() {
        let n = release_notes(NOTES);
        // One section on its own needs no heading.
        assert_eq!(n.first().map(String::as_str), Some("Address security vulnerabilities: CVE-2026-20708, CVE-2026-20715, and CVE-2026-20734"), "{n:?}");
        // Two sections keep theirs.
        let two = release_notes("<html>TITLE:  X<br/>FIXES:  <br/>- Fixes a hang on resume.<br/><br/>ENHANCEMENTS:  <br/>- Adds a setting.<br/>CVE NUMBERS:  <br/>CVE-2026-1<br/></html>");
        assert_eq!(two, vec!["## Fixes", "Fixes a hang on resume.", "## Enhancements", "Adds a setting."]);
        // A wrapped bullet is one line.
        assert!(n.iter().any(|l| l == "Address security vulnerabilities: CVE-2026-20708, CVE-2026-20715, and CVE-2026-20734"), "{n:?}");
        assert!(n.iter().any(|l| l.starts_with("Enables 6 GHz WLAN band support in Paraguay") && l.ends_with("on supported platforms.")), "{n:?}");
        assert!(n.iter().any(|l| l == "Intel Management Engine Firmware, version 16.1.42.2872"), "{n:?}");
        // The page's other sections are not changes.
        assert!(!n.iter().any(|l| l.contains("HOW TO USE") || l.contains("Double-click") || l.contains("CVE NUMBERS") || l.contains("PREREQUISITES")), "{n:?}");
        assert!(!n.iter().any(|l| l.contains("SOFTPAQ") || l.contains("sp174304")));
        assert!(release_notes("<html>nothing here</html>").is_empty());
        // A stray closing quote on an item is dropped.
        assert_eq!(release_notes("<html>FIXES:  <br/>- Fixes security vulnerability CVE-2021-1.\"<br/>- Second fix.<br/>PREREQUISITES:  NONE<br/></html>"), vec!["Fixes security vulnerability CVE-2021-1.", "Second fix."]);
    }

    #[test]
    fn only_the_newest_softpaq_of_each_driver_is_listed_and_the_bios_is_not() {
        let m = model();
        assert!(m.drivers.iter().all(|d| !d.name.contains("BIOS")));
        // Intel WLAN Driver has two SoftPaqs in the reference: the newer one.
        let wlan: Vec<_> = m.drivers.iter().filter(|d| d.name == "Intel WLAN Driver").collect();
        assert_eq!(wlan.len(), 1);
        assert_eq!((wlan[0].id.as_str(), wlan[0].version.as_str(), wlan[0].kind.as_str()), ("sp165709", "23.170.1.1", "Network"));
        assert_eq!(wlan[0].date.as_deref(), Some("2025.11.05"));
        assert_eq!(wlan[0].page.as_deref(), Some("https://ftp.hp.com/pub/softpaq/sp165501-166000/sp165709.html"));
        // A diagnostics tool is not a driver or firmware.
        assert!(!m.drivers.iter().any(|d| d.id == "sp171649"));
        // Kinds.
        assert_eq!(find(&m, "sp156744").kind, "Bluetooth");
        assert_eq!(find(&m, "sp149650").kind, "Firmware");
        assert_eq!(find(&m, "sp165987").kind, "Utility");
    }

    #[test]
    fn a_driver_gets_the_hardware_ids_of_its_devices_grouped_by_the_version_they_end_up_with() {
        let m = model();
        let w = find(&m, "sp165709");
        assert_eq!(w.hardware_ids, vec!["PCI\\VEN_8086&DEV_02F0", "PCI\\VEN_8086&DEV_02F0&SUBSYS_00308086"]);
        assert!(hw_match(&w.hardware_ids[0], "PCI\\VEN_8086&DEV_02F0&SUBSYS_00348086&REV_1A"));
        // The audio SoftPaq: HD Audio devices at one driver version, extension devices at another.
        let audio: Vec<_> = m.drivers.iter().filter(|d| d.name == "Realtek HD Audio Driver").collect();
        assert_eq!(audio.len(), 2, "{audio:?}");
        let hd = audio.iter().find(|d| d.version == "6.0.9800.1").unwrap();
        assert_eq!(hd.id, "sp157440");
        assert_eq!(hd.hardware_ids, vec!["HDAUDIO\\FUNC_01&VEN_10EC&DEV_0221&SUBSYS_103C2175", "HDAUDIO\\FUNC_01&VEN_10EC&DEV_0221&SUBSYS_103C8055"]);
        let ext = audio.iter().find(|d| d.version == "17.20.39.4170").unwrap();
        assert_eq!(ext.id, "sp157440:17.20.39.4170");
        assert_eq!(ext.hardware_ids.len(), 2);
        // Chipset devices are at several versions.
        let chip: Vec<_> = m.drivers.iter().filter(|d| d.name == "Intel Chipset Device Software").collect();
        assert!(chip.len() >= 3 && chip.iter().all(|d| !d.hardware_ids.is_empty()), "{chip:?}");
        // A firmware package has no driver version to compare, so no ids.
        assert!(find(&m, "sp149650").hardware_ids.is_empty());
    }

    #[test]
    fn a_softpaq_installs_checked_and_signed_with_its_silent_switch() {
        let m = model();
        let d = find(&m, "sp165709");
        let i = d.install.as_ref().unwrap();
        assert_eq!(i.url, "https://ftp.hp.com/pub/softpaq/sp165501-166000/sp165709.exe");
        assert_eq!((i.signer.as_str(), i.args.clone(), i.inner.clone()), ("HP Inc.", vec!["/s".to_string()], None));
        assert!(i.sha256.as_deref().is_some_and(|s| s.len() == 64) && host_is(&i.url, &["hp.com"]));
        let plan = plan_install("driver:maker:hp:sp165709", "Intel WLAN Driver", d).unwrap();
        assert_eq!(plan.downloads[0].sha256.as_deref(), i.sha256.as_deref());
        assert!(matches!(&plan.steps[0], crate::sources::Step::CheckSigner { signer, .. } if signer == "HP Inc."));
        let crate::sources::Step::Run { args, ok_codes, elevated, .. } = &plan.steps[1] else { panic!() };
        assert!(*elevated && args == &vec!["/s".to_string()] && ok_codes.contains(&3010));
        // A firmware SoftPaq, and one whose silent install HP says is not supported, are not installed here.
        assert!(find(&m, "sp149650").install.is_none());
        assert!(!silent_ok("SSD do not support silent install.") && !silent_ok("Not support") && !silent_ok("") && silent_ok("HPUP.exe"));
    }

    #[test]
    fn device_ids_lose_their_instance_part_and_the_software_ones_are_left_out() {
        assert_eq!(device_id("PCI\\VEN_8086&DEV_02F0\\3&11583659&0&A2").as_deref(), Some("PCI\\VEN_8086&DEV_02F0"));
        assert_eq!(device_id("hdaudio\\func_01&ven_10ec").as_deref(), Some("HDAUDIO\\FUNC_01&VEN_10EC"));
        assert_eq!(device_id("SWD\\MMDEVAPI\\{0.0.0.00000000}"), None);
        assert_eq!(device_id("{5D624F94-8850-40C3-A3FA-A4FD2080BAF3}\\VWIFIMP_WFD\\5&1&0&11"), None);
        assert_eq!(device_id("nobus"), None);
    }

    #[test]
    fn a_pc_of_another_kind_gets_the_packages_for_it() {
        assert!(parse(REFERENCE, "arm64").unwrap().drivers.is_empty());
        assert!(parse("<ImagePal/>", "x64").is_err());
        assert!(parse("not xml", "x64").is_err());
    }

    /// HP's real reference files for a public sample platform (a Pro SFF 400 G9): `cargo test
    /// --release --lib live_hp -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_hp() {
        let b = board("8C5A", "U03 Ver. 02.13.01");
        println!("{:#?}", check(&b).await);
        let list = drivers(&b).await;
        println!("{} entries; {} with hardware ids", list.len(), list.iter().filter(|d| !d.hardware_ids.is_empty()).count());
        for d in list.iter().take(12) {
            println!("{} | {} | {} | {} | ids {} | install {}", d.id, d.kind, d.name, d.version, d.hardware_ids.len(), d.install.is_some());
        }
        assert!(matches!(check(&b).await, Answer::Listed(_)));
        assert!(!list.is_empty());
        // A platform HP has no reference file for.
        assert_eq!(fetch("0000").await.unwrap(), Lookup::NotListed);
        // A platform from 2024 (only a Windows 11 file), one from 2021, an old one.
        for (id, bios) in [("8E2E", "X73 Ver. 01.02.02"), ("8A73", "T70 Ver. 01.20.00"), ("805A", "N01 Ver. 02.10.00")] {
            let b = board(id, bios);
            println!("{id}: {:?} / {} entries", check(&b).await, drivers(&b).await.len());
        }
    }
}
