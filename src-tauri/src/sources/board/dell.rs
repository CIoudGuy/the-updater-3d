//! Dell PCs: the newest BIOS, drivers and firmware Dell lists for the PC, from the catalogs Dell
//! Command Update reads on downloads.dell.com. An index (`CatalogIndexPC.cab`, about 450 KB) names
//! one small catalog per system id (the id a Dell PC reports as its SKU, "0A1F"), and that
//! catalog (a few hundred KB) lists every BIOS, driver and firmware package of the PC with its
//! version, date, checksum, hardware ids and a Dell page. Both are .cab files with one XML file
//! in them, unpacked with Windows' own expand.exe. Only the system id goes to Dell.

use super::{Answer, Board, Listed, MakerDriver, MakerInstall};
use crate::http::Parsed;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::BTreeMap;
use std::time::Duration;

const BASE: &str = "https://downloads.dell.com/";
const INDEX: &str = "https://downloads.dell.com/catalog/CatalogIndexPC.cab";
/// Dell's support start page, for a PC the catalog does not list.
const SUPPORT: &str = "https://www.dell.com/support/home/en-us";
/// The catalogs are kept for twelve hours.
const AGE: Duration = Duration::from_secs(12 * 3600);
/// The signer name of Dell's packages (read from a downloaded package's signature).
const SIGNER: &str = "Dell Inc";

// What was parsed, by the checksum of what it was parsed from: the catalogs are unpacked and read
// once, and only the small result is kept.
static PATHS: Parsed<Option<String>> = Parsed::new();
static MODELS: Parsed<Lookup> = Parsed::new();

/// The newest BIOS Dell lists for the PC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bios {
    pub version: String,
    /// "2026.10.16"
    pub date: Option<String>,
    pub page: Option<String>,
    pub notes: Vec<String>,
}

/// What Dell's catalog for one system id gives.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Model {
    pub bios: Option<Bios>,
    pub drivers: Vec<MakerDriver>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Lookup {
    /// The index has no catalog for this system id
    NotListed,
    Found(Model),
}

// ---------------------------------------------------------------- files

/// Text of an XML file in UTF-16 (Dell) or UTF-8, with or without its byte order mark.
pub(super) fn decode_text(b: &[u8]) -> String {
    fn utf16(b: &[u8], f: fn([u8; 2]) -> u16) -> String {
        let w: Vec<u16> = b.as_chunks::<2>().0.iter().map(|c| f(*c)).collect();
        String::from_utf16_lossy(&w)
    }
    if let Some(r) = b.strip_prefix(&[0xFF, 0xFE]) {
        utf16(r, u16::from_le_bytes)
    } else if let Some(r) = b.strip_prefix(&[0xFE, 0xFF]) {
        utf16(r, u16::from_be_bytes)
    } else {
        String::from_utf8_lossy(b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b)).into_owned()
    }
}

/// The one file inside a .cab, unpacked by Windows' expand.exe in a temp folder that is removed
/// again. Nothing in it is run.
pub(super) fn expand_cab(cab: &[u8]) -> Result<Vec<u8>> {
    expand_cab_in(&std::env::temp_dir(), cab)
}

fn expand_cab_in(base: &std::path::Path, cab: &[u8]) -> Result<Vec<u8>> {
    use std::sync::atomic::{AtomicU32, Ordering};
    /// The largest file taken out of a cabinet
    const MAX: u64 = 96 * 1024 * 1024;
    struct Dir(std::path::PathBuf);
    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    static N: AtomicU32 = AtomicU32::new(0);
    if !cab.starts_with(b"MSCF") {
        bail!("Not a cabinet file");
    }
    let dir = Dir(base.join(format!("u3d-cab-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed))));
    let out = dir.0.join("out");
    std::fs::create_dir_all(&out)?;
    let src = dir.0.join("in.cab");
    std::fs::write(&src, cab)?;
    let exe = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32").join("expand.exe");
    let run = crate::winutil::cmd(exe).arg(&src).arg("-F:*").arg(&out).output().context("Could not run expand.exe")?;
    if !run.status.success() {
        bail!("expand.exe could not unpack the cabinet");
    }
    let file = std::fs::read_dir(&out)?.filter_map(|e| e.ok()).map(|e| e.path()).find(|p| p.is_file()).ok_or_else(|| anyhow!("The cabinet is empty"))?;
    if std::fs::metadata(&file)?.len() > MAX {
        bail!("The file in the cabinet is too large");
    }
    Ok(std::fs::read(file)?)
}

// ---------------------------------------------------------------- the index

/// The system id Dell lists a PC under: 4 hex characters, as the PC reports in its SKU.
pub fn system_id(b: &Board) -> Option<String> {
    let s = b.system_sku.trim();
    (s.len() == 4 && s.chars().all(|c| c.is_ascii_hexdigit())).then(|| s.to_ascii_uppercase())
}

/// The index's address of the catalog of a system id, "FOLDER15162537M/1/Latitude_0A1F.cab".
pub fn model_path(index_xml: &str, sid: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(index_xml).ok()?;
    for g in doc.descendants().filter(|n| n.has_tag_name("GroupManifest")) {
        if !g.descendants().any(|n| n.has_tag_name("Model") && n.attribute("systemID").is_some_and(|s| s.eq_ignore_ascii_case(sid))) {
            continue;
        }
        let latest = g.attribute("latest");
        let infos: Vec<_> = g.children().filter(|n| n.has_tag_name("ManifestInformation")).collect();
        let pick = infos.iter().find(|n| latest.is_some() && n.attribute("id") == latest).or(infos.last())?;
        let path = pick.attribute("path")?;
        // Only a plain catalog file on Dell's server.
        let ok = path.ends_with(".cab") && !path.contains("..") && path.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '_' | '-' | '.'));
        return ok.then(|| path.to_string());
    }
    None
}

// ---------------------------------------------------------------- the catalog of one system id

/// One package of the catalog.
#[derive(Clone, Debug, Default)]
struct Comp {
    id: String,
    name: String,
    /// BIOS, DRVR (driver), FRMW (firmware), APAC (application)
    code: String,
    /// Dell's category: "Chipset", "Network", "Audio", "Video"
    category: String,
    version: String,
    date: Option<chrono::NaiveDate>,
    path: String,
    size: Option<u64>,
    md5: Option<String>,
    sha256: Option<String>,
    desc: String,
    rating: String,
    info: Option<String>,
    /// Is it for Windows 10 or 11 of the PC's kind (x64, ARM64)?
    os_ok: bool,
    /// Which device it is for: the same for the versions of one driver
    device: String,
    /// The hardware ids of the devices listed without a driver version
    plain_ids: Vec<String>,
    /// The devices of a DCH package, each with the driver version it ends up with
    dch: Vec<(String, Vec<String>)>,
}

fn child<'a, 'b>(n: roxmltree::Node<'a, 'b>, name: &str) -> Option<roxmltree::Node<'a, 'b>> {
    n.children().find(|c| c.has_tag_name(name))
}

/// The text of an element's `<Display>` ("Dell Latitude 5320 System BIOS").
fn display(n: roxmltree::Node, name: &str) -> String {
    child(n, name).and_then(|e| e.descendants().find(|d| d.has_tag_name("Display"))).and_then(|d| d.text()).unwrap_or("").trim().to_string()
}

/// "October 16, 2026" or "June 23,2026"
fn parse_date(s: &str) -> Option<chrono::NaiveDate> {
    let t = s.replace(',', ", ").split_whitespace().collect::<Vec<_>>().join(" ");
    chrono::NaiveDate::parse_from_str(&t, "%B %d, %Y").ok()
}

/// The hardware ids of a catalog entry: PCI ("PCI\VEN_8086&DEV_A0F0&SUBSYS_0A1F1028"), USB
/// ("USB\VID_0BDA&PID_4014") and the plain ids it lists ("USB\VID_8087&PID_0AA7").
fn hardware_ids(dev: roxmltree::Node) -> Vec<String> {
    let mut out = Vec::new();
    let hex = |v: Option<&str>| v.map(str::trim).filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_hexdigit())).map(str::to_ascii_uppercase);
    for e in dev.children().filter(|c| c.is_element()) {
        if e.has_tag_name("PCIInfo") {
            if let (Some(v), Some(d)) = (hex(e.attribute("vendorID")), hex(e.attribute("deviceID"))) {
                match (hex(e.attribute("subDeviceID")), hex(e.attribute("subVendorID"))) {
                    (Some(sd), Some(sv)) => out.push(format!("PCI\\VEN_{v}&DEV_{d}&SUBSYS_{sd}{sv}")),
                    _ => out.push(format!("PCI\\VEN_{v}&DEV_{d}")),
                }
            }
        } else if e.has_tag_name("PnPInfo") {
            let t = |n: &str| child(e, n).and_then(|x| x.text()).map(str::trim).filter(|v| v.len() == 4 && v.chars().all(|c| c.is_ascii_hexdigit())).map(str::to_ascii_uppercase);
            if let (Some(v), Some(p)) = (t("PNPID"), t("PnPProductID")) {
                out.push(format!("USB\\VID_{v}&PID_{p}"));
            }
        } else if e.has_tag_name("Generic") {
            if let Some(t) = e.text().map(str::trim).filter(|t| !t.is_empty()) {
                out.push(t.to_string());
            }
        }
    }
    out
}

fn comp(n: roxmltree::Node, arch: &str) -> Option<Comp> {
    let id = n.attribute("packageID").or(n.attribute("releaseID"))?.to_string();
    let version = n.attribute("vendorVersion")?.trim().to_string();
    let path = n.attribute("path")?.to_string();
    let hash = |alg: &str| child(n, "Cryptography").and_then(|c| c.children().find(|h| h.has_tag_name("Hash") && h.attribute("algorithm").is_some_and(|a| a.eq_ignore_ascii_case(alg)))).and_then(|h| h.text()).map(|t| t.trim().to_ascii_lowercase()).filter(|t| !t.is_empty());
    let os_ok = child(n, "SupportedOperatingSystems").is_some_and(|s| s.children().any(|o| o.has_tag_name("OperatingSystem") && o.attribute("majorVersion") == Some("10") && o.attribute("osArch").is_some_and(|a| a.eq_ignore_ascii_case(arch))));
    let mut ids_key: Vec<u64> = Vec::new();
    let mut plain_ids = Vec::new();
    if let Some(sd) = child(n, "SupportedDevices") {
        for d in sd.children().filter(|d| d.has_tag_name("Device")) {
            if let Some(cid) = d.attribute("componentID").and_then(|c| c.parse().ok()) {
                ids_key.push(cid);
            }
            plain_ids.extend(hardware_ids(d));
        }
    }
    let mut dch = Vec::new();
    if let Some(sd) = child(n, "SupportedDCHDevices") {
        for d in sd.children().filter(|d| d.has_tag_name("Device")) {
            if let Some(cid) = d.attribute("componentID").and_then(|c| c.parse().ok()) {
                ids_key.push(cid);
            }
            dch.push((d.attribute("version").unwrap_or("").trim().to_string(), hardware_ids(d)));
        }
    }
    let name = display(n, "Name");
    // The versions of one driver have the same devices: the lowest device number is the key.
    let device = ids_key.iter().min().map(|c| c.to_string()).unwrap_or_else(|| name.clone());
    let info = child(n, "ImportantInfo").and_then(|i| i.attribute("URL")).map(str::to_string);
    Some(Comp {
        id,
        name,
        code: child(n, "ComponentType").and_then(|c| c.attribute("value")).unwrap_or("").to_string(),
        category: display(n, "Category"),
        version,
        date: n.attribute("releaseDate").and_then(parse_date).or_else(|| n.attribute("dateTime").and_then(|d| d.get(..10)).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())),
        path,
        size: n.attribute("size").and_then(|s| s.parse().ok()),
        md5: hash("MD5"),
        sha256: hash("SHA256"),
        desc: display(n, "Description"),
        rating: display(n, "Criticality"),
        info,
        os_ok,
        device,
        plain_ids,
        dch,
    })
}

/// The digits of a version in order, "1.21.0" as [1, 21, 0], "A24" as [24].
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

/// Dell BIOS versions: "1.21.0" (numbers) or "A24" (a letter and a number). Is `latest` newer
/// than `have`? None when the two are written differently.
pub fn newer(latest: &str, have: &str) -> Option<bool> {
    fn parts(v: &str) -> Option<(Option<char>, Vec<u64>)> {
        let v = v.trim();
        let (letter, rest) = match v.chars().next()? {
            c if c.is_ascii_alphabetic() => (Some(c.to_ascii_uppercase()), &v[1..]),
            _ => (None, v),
        };
        if rest.is_empty() || !rest.chars().all(|c| c.is_ascii_digit() || c == '.') || rest.starts_with('.') || rest.ends_with('.') || rest.contains("..") {
            return None;
        }
        let n: Option<Vec<u64>> = rest.split('.').map(|s| s.parse().ok()).collect();
        Some((letter, n?))
    }
    let (a, b) = (parts(latest)?, parts(have)?);
    (a.0 == b.0).then(|| cmp_nums(&a.1, &b.1).is_gt())
}

/// Sentences of Dell's description, without the boilerplate every BIOS package starts with.
fn sentences(desc: &str, skip_bios_intro: bool) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = desc.chars().collect();
    for (i, c) in chars.iter().enumerate() {
        cur.push(*c);
        if *c == '.' && chars.get(i + 1).is_none_or(|n| n.is_whitespace()) {
            out.push(std::mem::take(&mut cur).trim().to_string());
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out.retain(|s| {
        let l = s.to_ascii_lowercase();
        !s.is_empty() && !(skip_bios_intro && (l.starts_with("this package contains the dell system bios") || l.starts_with("bios is a firmware") || l.starts_with("it controls the keyboard")))
    });
    out
}

/// The page for a package: Dell's own link when it is on dell.com.
fn page_of(c: &Comp) -> Option<String> {
    c.info.clone().filter(|u| crate::sources::drivers::host_is(u, &["dell.com"]))
}

fn newest<'a>(items: impl Iterator<Item = &'a Comp>) -> Option<&'a Comp> {
    items.max_by(|a, b| cmp_nums(&nums(&a.version), &nums(&b.version)).then(a.date.cmp(&b.date)))
}

fn bios_of(comps: &[Comp]) -> Option<Bios> {
    let c = newest(comps.iter().filter(|c| c.code == "BIOS"))?;
    let mut notes = sentences(&c.desc, true);
    if !c.rating.is_empty() {
        notes.push(format!("Dell rates it: {}", c.rating));
    }
    Some(Bios { version: c.version.clone(), date: c.date.map(|d| d.format("%Y.%m.%d").to_string()), page: page_of(c), notes })
}

fn kind_of(c: &Comp) -> String {
    match c.code.as_str() {
        "FRMW" => return "Firmware".into(),
        "APAC" => return "Utility".into(),
        _ => {}
    }
    let n = c.name.to_ascii_lowercase();
    match c.category.to_ascii_lowercase().as_str() {
        "audio" => "Audio",
        "video" => "Graphics",
        "network" | "communications" => {
            if n.contains("bluetooth") {
                "Bluetooth"
            } else {
                "Network"
            }
        }
        "chipset" => "Chipset",
        "serial ata" | "storage" | "storage controller" | "sd card" => "Storage",
        "input" => "Input",
        "camera" => "Camera",
        "security" => "Security",
        "docks/stands" => "Dock",
        "power" => "Power",
        _ => "Driver",
    }
    .into()
}

/// The driver versions of a package with the hardware ids each one is for. A DCH package lists
/// its devices with the version each ends up with; an older package has one version for all.
/// Only a driver gets ids: the version of a tool or firmware is not a driver version.
fn groups(c: &Comp) -> Vec<(String, Vec<String>)> {
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    if c.code == "DRVR" {
        if c.dch.is_empty() {
            if dotted(&c.version) && !c.plain_ids.is_empty() {
                by.insert(c.version.clone(), c.plain_ids.clone());
            }
        } else {
            for (v, ids) in &c.dch {
                if dotted(v) && !ids.is_empty() {
                    by.entry(v.clone()).or_default().extend(ids.iter().cloned());
                }
            }
        }
    }
    for ids in by.values_mut() {
        ids.sort();
        ids.dedup();
    }
    if by.is_empty() {
        return vec![(c.version.clone(), vec![])];
    }
    by.into_iter().collect()
}

fn driver_entries(c: &Comp) -> Vec<MakerDriver> {
    let install = (c.code == "DRVR" && (c.md5.is_some() || c.sha256.is_some()) && !c.path.contains("..")).then(|| MakerInstall {
        url: format!("{BASE}{}", c.path.trim_start_matches('/')),
        sha256: c.sha256.clone(),
        md5: c.md5.clone(),
        signer: SIGNER.into(),
        // Dell's update package: /s is silent and does not restart; 2 and 3010 are "done, restart later".
        args: vec!["/s".into()],
        ok_codes: vec![0, 2, crate::sources::drivers::EXIT_REBOOT],
        inner: None,
    });
    let groups = groups(c);
    let many = groups.len() > 1;
    groups
        .into_iter()
        .map(|(version, hardware_ids)| MakerDriver {
            id: if many && version != c.version { format!("{}:{version}", c.id) } else { c.id.clone() },
            name: c.name.clone(),
            kind: kind_of(c),
            version,
            date: c.date.map(|d| d.format("%Y.%m.%d").to_string()),
            page: page_of(c),
            notes: sentences(&c.desc, false).into_iter().take(3).collect(),
            hardware_ids,
            installed: None,
            size: c.size,
            install: install.clone(),
        })
        .collect()
}

/// The newest package of each driver, firmware and tool for Windows 10 and 11 (the BIOS is read
/// on its own).
fn drivers_of(comps: &[Comp]) -> Vec<MakerDriver> {
    let mut best: BTreeMap<(String, String), &Comp> = BTreeMap::new();
    for c in comps.iter().filter(|c| c.code != "BIOS" && c.os_ok) {
        let key = (c.code.clone(), c.device.clone());
        let better = best.get(&key).is_none_or(|b| newest([*b, c].into_iter()).is_some_and(|n| std::ptr::eq(n, c)));
        if better {
            best.insert(key, c);
        }
    }
    let mut out: Vec<MakerDriver> = best.values().flat_map(|c| driver_entries(c)).collect();
    out.sort_by(|a, b| (&a.kind, &a.name, &a.id).cmp(&(&b.kind, &b.name, &b.id)));
    out
}

/// What the catalog of a system id gives: `arch` is the PC's kind ("x64", "arm64").
pub fn parse(xml: &str, arch: &str) -> Result<Model> {
    let doc = roxmltree::Document::parse(xml).context("Dell's catalog could not be read")?;
    let comps: Vec<Comp> = doc.root_element().children().filter(|n| n.has_tag_name("SoftwareComponent")).filter_map(|n| comp(n, arch)).collect();
    if comps.is_empty() {
        bail!("Dell's catalog is empty");
    }
    Ok(Model { bios: bios_of(&comps), drivers: drivers_of(&comps) })
}

// ---------------------------------------------------------------- asking Dell

/// Dell's catalog for a system id.
pub async fn fetch(sid: &str) -> Result<Lookup> {
    let idx = crate::http::get_cached(INDEX, &[], AGE).await?;
    let key = sid.to_string();
    let path = tokio::task::spawn_blocking(move || {
        PATHS.get_or(&format!("dell-index:{key}"), &idx.body, |cab| {
            let xml = decode_text(&expand_cab(cab)?);
            Ok(model_path(&xml, &key))
        })
    })
    .await??;
    let Some(path) = path else { return Ok(Lookup::NotListed) };
    let cab = crate::http::get_cached(&format!("{BASE}{path}"), &[], AGE).await?;
    let (key, arch) = (sid.to_string(), crate::winutil::native_arch());
    tokio::task::spawn_blocking(move || {
        MODELS.get_or(&format!("dell-model:{key}"), &cab.body, |cab| {
            let xml = decode_text(&expand_cab(cab)?);
            Ok(Lookup::Found(parse(&xml, arch)?))
        })
    })
    .await?
}

/// The answer for this PC from Dell's catalog (None: Dell was not reached).
pub fn answer(b: &Board, data: Option<&Lookup>) -> Answer {
    let missing = |why: &str| Answer::Missing { why: why.into(), page: Some(SUPPORT.into()) };
    if system_id(b).is_none() {
        return missing("This PC does not report the system id Dell lists it under");
    }
    match data {
        None => missing("Could not reach Dell"),
        Some(Lookup::NotListed) => missing("Dell does not list this PC in its update catalog"),
        Some(Lookup::Found(m)) => match &m.bios {
            None => missing("Dell lists no BIOS for this PC"),
            Some(bios) => Answer::Listed(Listed { version: bios.version.clone(), page: bios.page.clone().or(Some(SUPPORT.into())), date: bios.date.clone(), notes: bios.notes.clone(), newer: newer(&bios.version, &b.bios) }),
        },
    }
}

pub async fn check(b: &Board) -> Answer {
    let Some(sid) = system_id(b) else { return answer(b, None) };
    let data = fetch(&sid).await.map_err(|e| log::warn!("dell catalog: {e:#}")).ok();
    answer(b, data.as_ref())
}

/// The drivers and firmware Dell lists for this PC.
pub async fn drivers(b: &Board) -> Vec<MakerDriver> {
    let Some(sid) = system_id(b) else { return vec![] };
    match fetch(&sid).await {
        Ok(Lookup::Found(m)) => m.drivers,
        Ok(Lookup::NotListed) => vec![],
        Err(e) => {
            log::warn!("dell catalog: {e:#}");
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

    /// The index of Dell's catalogs, cut down to two systems.
    const INDEX_XML: &str = r####"<?xml version="1.0" encoding="utf-16"?>
<ManifestIndex baseLocation="downloads.dell.com" creationDateTime="2026-10-23T00:48:22-05:00" version="2026.10.23" baseLocationAccessProtocols="HTTPS" schemaVersion="3.0" xmlns="openmanage/cm/dm">
  <GroupManifest type="MTPDK" id="0f0d950c-c19e-4811-ab82-8098654ab986" latest="e34f5a3e-5656-43b4-b401-dbc1530d3984" eol="false" BKCCompliance="false" creationDateTime="2026-10-23T00:48:25-05:00">
    <Display lang="en"><![CDATA[ PDK Catalog for Latitude E4300]]></Display>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="024D" systemIDType="BIOS">
          <Display lang="en"><![CDATA[E4300]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <ManifestInformation id="e34f5a3e-5656-43b4-b401-dbc1530d3984" releaseID="Y4KND" creationDateTime="2025-12-28T00:40:44-06:00" version="2025.12.28" path="FOLDER14025503M/1/Latitude_024D.cab" size="21844" inventoryCollectorType="COMPACT">
      <Display lang="en"><![CDATA[ PDK Catalog for Latitude E4300]]></Display>
    </ManifestInformation>
  </GroupManifest>
  <GroupManifest type="MTPDK" id="af3b783c-84e3-4ed5-914b-29342444ab9b" latest="960d5ce3-5cc2-42e0-9b77-eb7e06b5b8bf" eol="false" BKCCompliance="true" creationDateTime="2026-10-23T01:05:26-05:00">
    <Display lang="en"><![CDATA[ PDK Catalog for Latitude 5320]]></Display>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F" systemIDType="BIOS">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <ManifestInformation id="960d5ce3-5cc2-42e0-9b77-eb7e06b5b8bf" releaseID="48K2J" creationDateTime="2026-10-21T23:37:54-05:00" version="2026.10.21" path="FOLDER15162537M/1/Latitude_0A1F.cab" size="342194" inventoryCollectorType="COMPACT">
      <Display lang="en"><![CDATA[ PDK Catalog for Latitude 5320]]></Display>
    </ManifestInformation>
  </GroupManifest>
</ManifestIndex>"####;
    /// Dell's catalog for system id 0A1F (a Latitude 5320), cut down: three BIOS versions, a few
    /// drivers (some DCH with per-device versions, some older), a tool and a firmware package.
    const CATALOG: &str = r####"<?xml version="1.0" encoding="utf-16"?>
<Manifest baseLocation="downloads.dell.com" dateTime="2026-10-21T23:37:54-05:00" identifier="00000000-0000-0000-0000-000000000000" releaseID="D0T8G" version="2026.10.21" xmlns="openmanage/cm/dm">
  <SoftwareComponent schemaVersion="3.0" releaseID="96J9J" releaseDate="June 09, 2026" vendorVersion="1.51.0" dellVersion="1.51.0" packageType="LWXP" path="FOLDER14488367M/1/Latitude_5320_1.51.0.exe" packageID="96J9J" dateTime="2026-04-14T01:40:52-05:00" size="26641176" identifier="724cdf8e-88a0-4b17-997f-0f06f4ff205c">
    <Name>
      <Display lang="en"><![CDATA[Dell Latitude 5320 System BIOS]]></Display>
    </Name>
    <ComponentType value="BIOS">
      <Display lang="en"><![CDATA[BIOS]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Dell system BIOS update. BIOS is a firmware package that is embedded on a small memory chip on the system board. It controls the keyboard, monitor, disk drives, and other devices. This update addresses the Dell Security Advisories (DSA). A security advisory is a statement when a security vulnerability impacts a product, and a remedy is available for the vulnerability.]]></Display>
    </Description>
    <Category value="BI">
      <Display lang="en"><![CDATA[BIOS]]></Display>
    </Category>
    <SupportedDevices>
      <Device componentID="159" embedded="false">
        <Display><![CDATA[Latitude 5320 2-in-1/ 5320]]></Display>
      </Device>
    </SupportedDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=96J9J">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="2">
      <Display lang="en"><![CDATA[Urgent]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">feb2d543337dedfc8cb2c1555e6692f4</Hash>
      <Hash algorithm="SHA256">3879f6faf65ea210b59c281dd86da1de5195417d2f20415f9b806203e99c464f</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="H7VD4" releaseDate="October 16, 2026" vendorVersion="1.53.0" dellVersion="1.53.0" packageType="LWXP" path="FOLDER14981284M/1/Latitude_5320_1.53.0.exe" packageID="H7VD4" dateTime="2026-06-17T08:36:19-05:00" size="26576848" identifier="29f017ad-2c44-4769-ba68-f4cee1513d79">
    <Name>
      <Display lang="en"><![CDATA[Dell Latitude 5320 System BIOS]]></Display>
    </Name>
    <ComponentType value="BIOS">
      <Display lang="en"><![CDATA[BIOS]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Dell system BIOS update. BIOS is a firmware package that is embedded on a small memory chip on the system board. It controls the keyboard, monitor, disk drives, and other devices. This update addresses the Dell Security Advisories (DSA). A security advisory is a statement when a security vulnerability impacts a product, and a remedy is available for the vulnerability.]]></Display>
    </Description>
    <Category value="BI">
      <Display lang="en"><![CDATA[BIOS]]></Display>
    </Category>
    <SupportedDevices>
      <Device componentID="159" embedded="false">
        <Display><![CDATA[Latitude 5320 2-in-1/ 5320]]></Display>
      </Device>
    </SupportedDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=H7VD4">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="2">
      <Display lang="en"><![CDATA[Urgent]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">4d0ccb51c028da9b171b946573560058</Hash>
      <Hash algorithm="SHA256">41f12f4d699c77b7760bcb34ce9db325b5fcef54d4ffa8fcfc5b5f0a82e7b60b</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="578W9" releaseDate="August 11, 2026" vendorVersion="1.52.0" dellVersion="1.52.0" packageType="LWXP" path="FOLDER14734270M/1/Latitude_5320_1.52.0.exe" packageID="578W9" dateTime="2026-05-20T07:34:35-05:00" size="26578024" identifier="8766cb27-1a4e-4977-b7e3-eacc778020b3">
    <Name>
      <Display lang="en"><![CDATA[Dell Latitude 5320 System BIOS]]></Display>
    </Name>
    <ComponentType value="BIOS">
      <Display lang="en"><![CDATA[BIOS]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Dell system BIOS update. BIOS is a firmware package that is embedded on a small memory chip on the system board. It controls the keyboard, monitor, disk drives, and other devices. This update addresses the Dell Security Advisories (DSA). A security advisory is a statement when a security vulnerability impacts a product, and a remedy is available for the vulnerability.]]></Display>
    </Description>
    <Category value="BI">
      <Display lang="en"><![CDATA[BIOS]]></Display>
    </Category>
    <SupportedDevices>
      <Device componentID="159" embedded="false">
        <Display><![CDATA[Latitude 5320 2-in-1/ 5320]]></Display>
      </Device>
    </SupportedDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=578W9">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="2">
      <Display lang="en"><![CDATA[Urgent]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">85ae12553bbfc21822af0223e58b7f9f</Hash>
      <Hash algorithm="SHA256">2831f350f3e40614790212cb45b643ec7c89201ad39a93cd28cde9e12fd256b0</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="FY1DP" releaseDate="October 15, 2025" vendorVersion="10.0.26200.21385" dellVersion="A01" packageType="LW64" path="FOLDER13578498M/5/Realtek-PCIe-Memory-Card-Reader-Driver_FY1DP_WIN64_10.0.26200.21385_A01_01.EXE" packageID="FY1DP" dateTime="2025-07-23T06:35:01-05:00" size="11715128" identifier="0daa70ee-a52f-49ef-bdde-da69cfc69530" baseBuildNumber="0">
    <Name>
      <Display lang="en"><![CDATA[Realtek PCIe Memory Card Reader Driver]]></Display>
    </Name>
    <ComponentType value="DRVR">
      <Display lang="en"><![CDATA[Driver]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Realtek PCIe memory card reader driver. The Realtek memory card reader driver enables communication between the Realtek card reader and the operating system.]]></Display>
    </Description>
    <Category value="CS">
      <Display lang="en"><![CDATA[Chipset]]></Display>
    </Category>
    <SupportedDCHDevices>
      <Device componentID="103018" guid="109a48ee-fb81-43a7-a2e0-f9b4e6c91dd3" infType="base" componentType="DRVR" version="10.0.26200.21385">
        <PCIInfo deviceID="5249" vendorID="10EC" subDeviceID="" subVendorID="" />
        <PCIInfo deviceID="5250" vendorID="10EC" subDeviceID="" subVendorID="" />
        <PCIInfo deviceID="5260" vendorID="10EC" subDeviceID="" subVendorID="" />
      </Device>
    </SupportedDCHDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="IOT01" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 10 64-Bit]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="W21P4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=FY1DP">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="2">
      <Display lang="en"><![CDATA[Urgent]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">1f1f77936ac6cc724411e170ee74cc4b</Hash>
      <Hash algorithm="SHA256">57059211a20a464e464d129d71898455ef935ae8a50f94fdb522f9b0a8ab2b4e</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="3GWTV" releaseDate="January 09, 2025" vendorVersion="10.0.26100.21374" dellVersion="A00" packageType="LW64" path="FOLDER12558088M/1/Realtek-PCIe-Memory-Card-Reader-Driver_3GWTV_WIN64_10.0.26100.21374_A00.EXE" packageID="3GWTV" dateTime="2024-10-28T14:15:33-05:00" size="22842112" identifier="2c1ea0be-6d17-4b7f-9cee-3f6cf5934c3f">
    <Name>
      <Display lang="en"><![CDATA[Realtek PCIe Memory Card Reader Driver]]></Display>
    </Name>
    <ComponentType value="DRVR">
      <Display lang="en"><![CDATA[Driver]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Realtek PCIe memory card reader driver. The Realtek memory card reader driver enables communication between the Realtek card reader and the operating system. This update addresses the Dell Security Advisories (DSA). A security advisory is a statement when a security vulnerability impacts a product, and a remedy is available for the vulnerability.]]></Display>
    </Description>
    <Category value="CS">
      <Display lang="en"><![CDATA[Chipset]]></Display>
    </Category>
    <SupportedDevices>
      <Device componentID="103018" embedded="false">
        <PCIInfo deviceID="525A" vendorID="10EC" subDeviceID="" subVendorID="" />
        <PCIInfo deviceID="525A" vendorID="10EC" subDeviceID="09A4" subVendorID="1028" />
        <PCIInfo deviceID="525A" vendorID="10EC" subDeviceID="09A5" subVendorID="1028" />
        <PnPInfo>
          <PNPID>0BDA</PNPID>
          <PnPProductID>0129</PnPProductID>
        </PnPInfo>
        <PnPInfo>
          <PNPID>0BDA</PNPID>
          <PnPProductID>0138</PnPProductID>
        </PnPInfo>
        <PnPInfo>
          <PNPID>0BDA</PNPID>
          <PnPProductID>0139</PnPProductID>
        </PnPInfo>
        <PnPInfo>
          <PNPID>0BDA</PNPID>
          <PnPProductID>0140</PnPProductID>
        </PnPInfo>
        <PnPInfo>
          <PNPID>0BDA</PNPID>
          <PnPProductID>0159</PnPProductID>
        </PnPInfo>
        <PnPInfo>
          <PNPID>0BDA</PNPID>
          <PnPProductID>0328</PnPProductID>
        </PnPInfo>
        <Display><![CDATA[Realtek Memory Card Reader Driver]]></Display>
      </Device>
      <Device componentID="103231" embedded="false">
        <PCIInfo deviceID="525A" vendorID="10EC" subDeviceID="" subVendorID="" />
        <Display><![CDATA[Realtek Memory Card Reader Driver]]></Display>
      </Device>
    </SupportedDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="W21H4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="W21P4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=3GWTV">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="2">
      <Display lang="en"><![CDATA[Urgent]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">74b624988875b4213d6437d304b9fe46</Hash>
      <Hash algorithm="SHA256">50bc73fe21fc9d9bf54244eb38ff3c9afa866aa0ec9641bc0dfda65116ef2ab2</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="M6WYJ" releaseDate="July 08, 2026" vendorVersion="2613.9.28.0" dellVersion="A16" packageType="LW64" path="FOLDER14430267M/2/Intel-Management-Engine-Components-Installer_M6WYJ_WIN64_2613.9.28.0_A16.EXE" packageID="M6WYJ" dateTime="2025-09-09T08:26:13-05:00" size="101892992" identifier="a9968afb-1ee1-4ab3-87a0-0eb346956506" baseBuildNumber="16299">
    <Name>
      <Display lang="en"><![CDATA[Intel Management Engine Components Installer]]></Display>
    </Name>
    <ComponentType value="DRVR">
      <Display lang="en"><![CDATA[Driver]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the driver for the Intel Management Engine Components Installer. This driver installs Intel Active Management Technology - Serial Over LAN driver, Intel Management and Security Application Local Management Service, Intel Management Engine Interface, Intel Management Engine Windows Management Instrumentation (WMI) provider, Intel Platform License Manager Service, Intel Serial Over LAN (SOL) Local Manageability Service (LMS) extension, Intel Wireless Manageability, Intel Wireless Manageability Driver extension, and Intel Management and Security Status application. This update addresses the Dell Security Advisories (DSA). A security advisory is a statement when a security vulnerability impacts a product, and a remedy is available for the vulnerability.]]></Display>
    </Description>
    <Category value="CS">
      <Display lang="en"><![CDATA[Chipset]]></Display>
    </Category>
    <SupportedDCHDevices>
      <Device componentID="100950" guid="02fed6e6-9161-4974-99a3-a0efd9d8cd1b" infType="base" componentType="DRVR" version="2603.9.4.0">
        <PCIInfo deviceID="4D70" vendorID="8086" subDeviceID="" subVendorID="" />
        <PCIInfo deviceID="A360" vendorID="8086" subDeviceID="" subVendorID="" />
        <PCIInfo deviceID="43E0" vendorID="8086" subDeviceID="" subVendorID="" />
      </Device>
      <Device componentID="100950" guid="03d15813-1257-406f-9b90-250712f704ff" infType="base" componentType="DRVR" version="2603.9.4.0">
        <PCIInfo deviceID="A360" vendorID="8086" subDeviceID="" subVendorID="" />
        <PCIInfo deviceID="98E0" vendorID="8086" subDeviceID="" subVendorID="" />
        <PCIInfo deviceID="38E0" vendorID="8086" subDeviceID="" subVendorID="" />
      </Device>
      <Device componentID="100950" guid="083f8020-c94f-4070-b8f3-e2b88a7cb1f2" infType="component" componentType="DRVR" version="1.80.173.0">
        <Generic>SWC\PROVIDER_INTEL_COMPONENT_ICLSCLIENT</Generic>
      </Device>
      <Device componentID="100950" guid="106523ef-d59d-4e1a-a629-8d77c08faf9d" infType="component" componentType="DRVR" version="1.80.173.0">
        <Generic>SWC\PROVIDER_INTEL_COMPONENT_ICLSCLIENT</Generic>
      </Device>
    </SupportedDCHDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="W10TM" osVendor="Microsoft" osArch="x64" majorVersion="0" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows Teams Rooms]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="IOT01" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 10 64-Bit]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=M6WYJ">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="2">
      <Display lang="en"><![CDATA[Urgent]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">c58f40d627b8aae8fee7fd4de44b96e8</Hash>
      <Hash algorithm="SHA256">ec493c8c3f3a3fceb8a2614e878bb80762d488083220b8c7212a6514631f7309</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="J9FYC" releaseDate="September 11, 2026" vendorVersion="24.70.0.3" dellVersion="A78" packageType="LW64" path="FOLDER14897649M/1/Intel-BE2xx-AX4xx-AX2xx-9xxx-Wi-Fi-Driver_J9FYC_WIN64_24.70.0.3_A78.EXE" packageID="J9FYC" dateTime="2026-08-13T08:16:12-05:00" size="19627848" identifier="7f779eae-06dd-4b59-b712-d89c891e2461" baseBuildNumber="17735">
    <Name>
      <Display lang="en"><![CDATA[Intel BE2xx/AX4xx/AX2xx/9xxx Wi-Fi Driver]]></Display>
    </Name>
    <ComponentType value="DRVR">
      <Display lang="en"><![CDATA[Driver]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Intel BE211, BE213, BE202, BE201, BE200, AX411, AX211, AX210, AX201, 9560, 9462, and 9260 Wi-Fi driver. Wireless drivers help your operating system to communicate with wireless devices such as wireless cards and network interface controllers.]]></Display>
    </Description>
    <Category value="NI">
      <Display lang="en"><![CDATA[Network]]></Display>
    </Category>
    <SupportedDCHDevices>
      <Device componentID="103305" guid="4fc06c22-c237-4c85-a23b-795b9922d1de" infType="base" componentType="DRVR" version="24.70.0.3">
        <PCIInfo deviceID="E440" vendorID="8086" subDeviceID="0110" subVendorID="8086" />
        <PCIInfo deviceID="A840" vendorID="8086" subDeviceID="4110" subVendorID="8086" />
        <PCIInfo deviceID="54F0" vendorID="8086" subDeviceID="009C" subVendorID="8086" />
      </Device>
      <Device componentID="103305" guid="6ec2da9f-04e5-4d1b-ae66-f339fc8d6710" infType="base" componentType="DRVR" version="24.70.0.3">
        <PCIInfo deviceID="3DF0" vendorID="8086" subDeviceID="0034" subVendorID="8086" />
        <PCIInfo deviceID="43F0" vendorID="8086" subDeviceID="02A0" subVendorID="8086" />
        <PCIInfo deviceID="3DF0" vendorID="8086" subDeviceID="4274" subVendorID="8086" />
      </Device>
      <Device componentID="103305" guid="bfb63c98-706b-445d-ac39-83800e26cae7" infType="base" componentType="DRVR" version="24.70.0.3">
        <PCIInfo deviceID="E440" vendorID="8086" subDeviceID="0110" subVendorID="8086" />
        <PCIInfo deviceID="A840" vendorID="8086" subDeviceID="4110" subVendorID="8086" />
        <PCIInfo deviceID="54F0" vendorID="8086" subDeviceID="009C" subVendorID="8086" />
      </Device>
    </SupportedDCHDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="W21H4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="W21P4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=J9FYC">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="1">
      <Display lang="en"><![CDATA[Recommended]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">81e7800c8729baee65f6a3a83f1f3e73</Hash>
      <Hash algorithm="SHA256">fa4dc87c0a171302a3aaa12f9ce76952eacf16a6077227dc7bd8f346c5baead2</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="8TH9N" releaseDate="September 04, 2026" vendorVersion="24.60.0.3" dellVersion="A77" packageType="LW64" path="FOLDER14782866M/1/Intel-BE2xx-AX4xx-AX2xx-9xxx-Wi-Fi-Driver_8TH9N_WIN64_24.60.0.3_A77.EXE" packageID="8TH9N" dateTime="2026-07-14T07:10:58-05:00" size="18760672" identifier="f1d18302-1c1e-413e-9430-32f9604bb4a4" baseBuildNumber="17735">
    <Name>
      <Display lang="en"><![CDATA[Intel BE2xx/AX4xx/AX2xx/9xxx Wi-Fi Driver]]></Display>
    </Name>
    <ComponentType value="DRVR">
      <Display lang="en"><![CDATA[Driver]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Intel BE211, BE213, BE202, BE201, BE200, AX411, AX211, AX210, AX201, 9560, 9462, and 9260 Wi-Fi driver. Wireless drivers help your operating system to communicate with wireless devices such as wireless cards and network interface controllers.]]></Display>
    </Description>
    <Category value="NI">
      <Display lang="en"><![CDATA[Network]]></Display>
    </Category>
    <SupportedDCHDevices>
      <Device componentID="103305" guid="27fb21f4-18bb-4e2e-8397-d72a627a077e" infType="base" componentType="DRVR" version="24.60.0.3">
        <PCIInfo deviceID="3DF0" vendorID="8086" subDeviceID="0034" subVendorID="8086" />
        <PCIInfo deviceID="43F0" vendorID="8086" subDeviceID="02A0" subVendorID="8086" />
        <PCIInfo deviceID="3DF0" vendorID="8086" subDeviceID="4274" subVendorID="8086" />
      </Device>
      <Device componentID="103305" guid="2e3e64c0-9543-4861-a543-3d4ed2b13d0d" infType="base" componentType="DRVR" version="24.60.0.3">
        <PCIInfo deviceID="E440" vendorID="8086" subDeviceID="0110" subVendorID="8086" />
        <PCIInfo deviceID="A840" vendorID="8086" subDeviceID="4110" subVendorID="8086" />
        <PCIInfo deviceID="54F0" vendorID="8086" subDeviceID="009C" subVendorID="8086" />
      </Device>
      <Device componentID="103305" guid="6c6f5cb8-7b16-449d-9435-1b9827f84a7a" infType="base" componentType="DRVR" version="24.60.0.3">
        <PCIInfo deviceID="E440" vendorID="8086" subDeviceID="0110" subVendorID="8086" />
        <PCIInfo deviceID="A840" vendorID="8086" subDeviceID="4110" subVendorID="8086" />
        <PCIInfo deviceID="54F0" vendorID="8086" subDeviceID="009C" subVendorID="8086" />
      </Device>
    </SupportedDCHDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="W21H4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="W21P4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=8TH9N">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="1">
      <Display lang="en"><![CDATA[Recommended]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">3e47a863c3bb485815fa71519471ab2d</Hash>
      <Hash algorithm="SHA256">c17b7505289d406acabbffa30aaac9e9a3c94f1369b57cc4f621876fd97a0d01</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="M2FMW" releaseDate="September 04, 2026" vendorVersion="24.60.0.4" dellVersion="A77" packageType="LW64" path="FOLDER14782445M/1/Intel-BE2xx-AX4xx-AX2xx-9xxx-and-Killer-BE17xx-AX16xx_M2FMW_WIN64_24.60.0.4_A77.EXE" packageID="M2FMW" dateTime="2026-07-14T07:20:28-05:00" size="12355952" identifier="faec931e-d383-428d-b9dd-17c60e810c8b" baseBuildNumber="16299">
    <Name>
      <Display lang="en"><![CDATA[Intel BE2xx/AX4xx/AX2xx/9xxx and Killer BE17xx/AX16xx Bluetooth Driver]]></Display>
    </Name>
    <ComponentType value="DRVR">
      <Display lang="en"><![CDATA[Driver]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Intel BE211, BE213, BE202, BE201, BE200, AX411, AX211, AX210, AX201, 9560, 9462, 9260 and Killer BE1775, BE1750, AX1690, AX1675, and AX1650 Bluetooth driver. Bluetooth driver is the software that helps your operating system to communicate with Bluetooth devices.]]></Display>
    </Description>
    <Category value="NI">
      <Display lang="en"><![CDATA[Network]]></Display>
    </Category>
    <SupportedDCHDevices>
      <Device componentID="103306" guid="2e93cbd3-f647-499f-91da-dda0184d6c44" infType="base" componentType="DRVR" version="24.40.11.1">
        <Generic>USB\VID_8087&amp;PID_0033&amp;REV_0000</Generic>
        <Generic>USB\VID_8087&amp;PID_0033&amp;REV_0001</Generic>
        <Generic>USB\VID_8087&amp;PID_0043&amp;REV_0000</Generic>
        <Generic>USB\VID_8087&amp;PID_0043&amp;REV_0001</Generic>
      </Device>
      <Device componentID="103306" guid="5e24631a-2dc2-4e55-9e4b-285d79311692" infType="base" componentType="DRVR" version="24.40.11.1">
        <Generic>USB\VID_8087&amp;PID_0032&amp;REV_0000</Generic>
        <Generic>USB\VID_8087&amp;PID_0032&amp;REV_0001</Generic>
      </Device>
      <Device componentID="103306" guid="62f0fcb7-07f7-411e-8dcb-f661a3cb1540" infType="base" componentType="DRVR" version="24.60.0.4">
        <Generic>USB\VID_8087&amp;PID_0038&amp;REV_0000</Generic>
        <Generic>USB\VID_8087&amp;PID_0038&amp;REV_0001</Generic>
      </Device>
      <Device componentID="103306" guid="674c4a2c-3a4e-4754-8219-e2399ada66fd" infType="base" componentType="DRVR" version="24.40.11.1">
        <Generic>USB\VID_8087&amp;PID_0026&amp;REV_0000</Generic>
        <Generic>USB\VID_8087&amp;PID_0026&amp;REV_0001</Generic>
        <Generic>USB\VID_8087&amp;PID_0026&amp;REV_0002</Generic>
      </Device>
    </SupportedDCHDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="W21H4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="W21P4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=M2FMW">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="1">
      <Display lang="en"><![CDATA[Recommended]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">20528bfda1a500e2df38bdf02e8d6116</Hash>
      <Hash algorithm="SHA256">10f5fade0147592b17ac7efd00ebf87b92db565e8ec08eb6515e6064a62d77f9</Hash>
    </Cryptography>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="WYJ59" releaseDate="September 11, 2026" vendorVersion="5.7.2" dellVersion="A00" packageType="LW64" path="FOLDER15013494M/2/Dell-Command-Update-for-Win32_WYJ59_WIN64_5.7.2_A00.EXE" packageID="WYJ59" dateTime="2026-04-28T19:07:00-05:00" size="90755928" identifier="376d37e3-ae4e-4549-8237-aaaf6e3842f0">
    <Name>
      <Display lang="en"><![CDATA[Dell Command | Update]]></Display>
    </Name>
    <ComponentType value="APAC">
      <Display lang="en"><![CDATA[Application]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the Dell Command | Update application. Dell Command Update is a stand-alone application for client systems that provides updates for system software that is released by Dell. This application simplifies the BIOS, firmware, driver, and application update experience for Dell client hardware. This update addresses the Dell Security Advisories (DSA). A security advisory is a statement when a security vulnerability impacts a product, and a remedy is available for the vulnerability.]]></Display>
    </Description>
    <Category value="SM">
      <Display lang="en"><![CDATA[Systems Management]]></Display>
    </Category>
    <SupportedDevices>
      <Device componentID="23400" embedded="false">
        <Display><![CDATA[Dell Command | Update]]></Display>
      </Device>
    </SupportedDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="W21H4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="W21P4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=WYJ59">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="1">
      <Display lang="en"><![CDATA[Recommended]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">a9b7dd0c60281e424abf373e60194401</Hash>
      <Hash algorithm="SHA256">5c20e1a352fdbc0a9759504db37c7b8c694f4024f5f5313c5130c0959be8d484</Hash>
    </Cryptography>
    <SharedModules>
      <SharedModule moduleID="107714" />
    </SharedModules>
  </SoftwareComponent>
  <SoftwareComponent schemaVersion="3.0" releaseID="XTHG0" releaseDate="August 23, 2021" vendorVersion="2111.3012" dellVersion="A00" packageType="LW64" path="FOLDER07599485M/1/Western-Digital-Corp-SN530-Solid-State-Drive-Firmware_XTHG0_WIN64_2111.3012_A00.EXE" packageID="XTHG0" dateTime="2021-07-05T04:32:24-05:00" size="13981784" identifier="3b5d3a1d-bae8-4d12-ae5d-eea134f69dd4">
    <Name>
      <Display lang="en"><![CDATA[Western Digital Corp SN530 Solid-State Drive Firmware Update]]></Display>
    </Name>
    <ComponentType value="FRMW">
      <Display lang="en"><![CDATA[Firmware]]></Display>
    </ComponentType>
    <Description>
      <Display lang="en"><![CDATA[This package contains the firmware for the Western Digital Corp SN530 256 GB, 512 GB, and 1 TB, Revision 21113012. Storage firmware is a microcode that is embedded on storage devices such as hard drives or solid-state drives. The firmware manages the functionality of the devices.]]></Display>
    </Description>
    <Category value="SA">
      <Display lang="en"><![CDATA[Serial ATA]]></Display>
    </Category>
    <SupportedDevices>
      <Device componentID="109153" embedded="false">
        <Display><![CDATA[WESTERN DIGITAL SN530 1TB PCIE NVME 2230 SSD (CLASS 35)]]></Display>
      </Device>
    </SupportedDevices>
    <SupportedSystems>
      <Brand key="28" prefix="LAT">
        <Display lang="en"><![CDATA[Latitude]]></Display>
        <Model systemID="0A1F">
          <Display lang="en"><![CDATA[5320]]></Display>
        </Model>
      </Brand>
    </SupportedSystems>
    <SupportedOperatingSystems>
      <OperatingSystem osCode="W21H4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
      <OperatingSystem osCode="W21P4" osVendor="Microsoft" osArch="x64" majorVersion="10" minorVersion="0" spMajorVersion="0" spMinorVersion="0">
        <Display lang="en"><![CDATA[Windows 11]]></Display>
      </OperatingSystem>
    </SupportedOperatingSystems>
    <ImportantInfo URL="https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=XTHG0">
      <Display lang="en"><![CDATA[NA]]></Display>
    </ImportantInfo>
    <Criticality value="1">
      <Display lang="en"><![CDATA[Recommended]]></Display>
    </Criticality>
    <Cryptography>
      <Hash algorithm="MD5">9344f2a9941a28f4407ce1c2e6068ed5</Hash>
      <Hash algorithm="SHA256">a58d1ddaca0df23e2df39c1bf906bc13dee386c25687d9671a9bab4c0d251065</Hash>
    </Cryptography>
  </SoftwareComponent>
  <Prerequisites />
</Manifest>"####;

    fn board(sku: &str, bios: &str) -> Board {
        Board { maker: "Dell Inc.".into(), product: "0X8DXD".into(), bios: bios.into(), bios_date: Some("2026.06.09".into()), system_maker: "Dell Inc.".into(), system_product: "Latitude 5320".into(), system_sku: sku.into(), ..Default::default() }
    }

    fn model() -> Model {
        parse(CATALOG, "x64").unwrap()
    }

    fn find<'a>(m: &'a Model, id: &str) -> &'a MakerDriver {
        m.drivers.iter().find(|d| d.id == id).unwrap_or_else(|| panic!("no {id} in {:?}", m.drivers.iter().map(|d| &d.id).collect::<Vec<_>>()))
    }

    #[test]
    fn the_system_id_is_four_hex_characters_of_the_sku() {
        assert_eq!(system_id(&board("0a1f", "1.0")), Some("0A1F".into()));
        assert_eq!(system_id(&board(" 0B0F ", "1.0")), Some("0B0F".into()));
        assert_eq!(system_id(&board("Latitude 5320", "1.0")), None);
        assert_eq!(system_id(&board("", "1.0")), None);
        assert_eq!(system_id(&board("0A1F2", "1.0")), None);
    }

    #[test]
    fn the_index_gives_each_system_its_catalog() {
        assert_eq!(model_path(INDEX_XML, "0A1F").as_deref(), Some("FOLDER15162537M/1/Latitude_0A1F.cab"));
        assert_eq!(model_path(INDEX_XML, "024d").as_deref(), Some("FOLDER14025503M/1/Latitude_024D.cab"));
        assert_eq!(model_path(INDEX_XML, "FFFF"), None);
        // A path that is not a plain catalog file on Dell's server is not taken.
        let odd = INDEX_XML.replace("FOLDER15162537M/1/Latitude_0A1F.cab", "../../evil/x.exe");
        assert_eq!(model_path(&odd, "0A1F"), None);
    }

    #[test]
    fn the_newest_bios_is_chosen_by_version_with_its_date_page_and_notes() {
        let b = model().bios.unwrap();
        assert_eq!((b.version.as_str(), b.date.as_deref()), ("1.53.0", Some("2026.10.16")));
        assert_eq!(b.page.as_deref(), Some("https://www.dell.com/support/home/us/en/19/Drivers/DriversDetails?driverId=H7VD4"));
        // Dell's boilerplate is left out; what the update does and Dell's rating stay.
        assert!(b.notes.iter().any(|n| n.starts_with("This update addresses the Dell Security Advisories")), "{:?}", b.notes);
        assert!(!b.notes.iter().any(|n| n.contains("firmware package that is embedded")));
        assert_eq!(b.notes.last().map(String::as_str), Some("Dell rates it: Urgent"));
    }

    #[test]
    fn dell_bios_versions_compare_by_number() {
        assert_eq!(newer("1.53.0", "1.21.0"), Some(true));
        assert_eq!(newer("1.53.0", "1.53.0"), Some(false));
        // Installed newer than the list: a test build.
        assert_eq!(newer("1.53.0", "1.60.1"), Some(false));
        assert_eq!(newer("1.9.0", "1.10.0"), Some(false));
        assert_eq!(newer("1.2", "1.2.1"), Some(false));
        // Older Dell PCs: a letter and a number.
        assert_eq!(newer("A24", "A09"), Some(true));
        assert_eq!(newer("A09", "A24"), Some(false));
        // Written differently: no answer.
        assert_eq!(newer("A24", "1.2.3"), None);
        assert_eq!(newer("1.2.3", "A24"), None);
        assert_eq!(newer("1.2.3", ""), None);
        assert_eq!(newer("A24", "B24"), None);
    }

    #[test]
    fn the_bios_answer_is_listed_missing_or_not_reached() {
        let m = Lookup::Found(model());
        let Answer::Listed(l) = answer(&board("0A1F", "1.21.0"), Some(&m)) else { panic!() };
        assert_eq!((l.version.as_str(), l.newer, l.date.as_deref()), ("1.53.0", Some(true), Some("2026.10.16")));
        let Answer::Listed(cur) = answer(&board("0A1F", "1.53.0"), Some(&m)) else { panic!() };
        assert_eq!(cur.newer, Some(false));
        let Answer::Listed(odd) = answer(&board("0A1F", "A24"), Some(&m)) else { panic!() };
        assert_eq!(odd.newer, None);
        // No data: Dell was not reached.
        assert!(matches!(answer(&board("0A1F", "1.21.0"), None), Answer::Missing { why, page: Some(_) } if why.contains("Could not reach")));
        assert!(matches!(answer(&board("0A1F", "1.21.0"), Some(&Lookup::NotListed)), Answer::Missing { why, .. } if why.contains("does not list")));
        assert!(matches!(answer(&board("", "1.21.0"), Some(&m)), Answer::Missing { .. }));
        assert!(matches!(answer(&board("0A1F", "1.21.0"), Some(&Lookup::Found(Model::default()))), Answer::Missing { .. }));
        // And as the BIOS entry.
        let b = board("0A1F", "1.21.0");
        let e = entry(&BiosEntry { answer: answer(&b, Some(&m)), board: b, maker: Some(Maker::Dell) });
        assert_eq!((e.status, e.latest.as_deref()), (AppStatus::Update, Some("1.53.0")));
        assert!(e.vendor_url.as_deref().is_some_and(|u| host_is(u, &["dell.com"])));
    }

    #[test]
    fn only_the_newest_of_each_driver_is_listed_and_the_bios_is_not() {
        let m = model();
        assert!(m.drivers.iter().all(|d| !d.name.contains("BIOS")));
        // Two Wi-Fi versions of one driver: the newer.
        let wifi: Vec<_> = m.drivers.iter().filter(|d| d.name.contains("Wi-Fi")).collect();
        assert_eq!(wifi.len(), 1);
        assert_eq!((wifi[0].id.as_str(), wifi[0].version.as_str(), wifi[0].kind.as_str()), ("J9FYC", "24.70.0.3", "Network"));
        // The card reader driver has an older package with another list of devices: the newest version only.
        let cr: Vec<_> = m.drivers.iter().filter(|d| d.name.contains("Card Reader")).collect();
        assert_eq!(cr.len(), 1);
        assert_eq!((cr[0].id.as_str(), cr[0].version.as_str(), cr[0].kind.as_str()), ("FY1DP", "10.0.26200.21385", "Chipset"));
    }

    #[test]
    fn a_driver_gets_its_hardware_ids_with_the_version_each_device_ends_up_with() {
        let m = model();
        let cr = find(&m, "FY1DP");
        assert_eq!(cr.hardware_ids, vec!["PCI\\VEN_10EC&DEV_5249", "PCI\\VEN_10EC&DEV_5250", "PCI\\VEN_10EC&DEV_5260"]);
        // The matching the app does: the installed id has more parts.
        assert!(hw_match(&cr.hardware_ids[0], "PCI\\VEN_10EC&DEV_5249&SUBSYS_0A1F1028&REV_01"));
        assert!(!hw_match(&cr.hardware_ids[0], "PCI\\VEN_10EC&DEV_5250&SUBSYS_0A1F1028"));
        // The Bluetooth package has devices at two driver versions: one entry for each.
        let bt: Vec<_> = m.drivers.iter().filter(|d| d.name.contains("Bluetooth")).collect();
        assert_eq!(bt.len(), 2, "{bt:?}");
        let new = bt.iter().find(|d| d.version == "24.60.0.4").unwrap();
        assert_eq!((new.id.as_str(), new.kind.as_str()), ("M2FMW", "Bluetooth"));
        assert_eq!(new.hardware_ids, vec!["USB\\VID_8087&PID_0038&REV_0000", "USB\\VID_8087&PID_0038&REV_0001"]);
        let old = bt.iter().find(|d| d.version == "24.40.11.1").unwrap();
        assert_eq!(old.id, "M2FMW:24.40.11.1");
        assert_eq!(old.hardware_ids.len(), 9);
        // The Wi-Fi package: its devices with their subsystem ids.
        let w = find(&m, "J9FYC");
        assert!(w.hardware_ids.contains(&"PCI\\VEN_8086&DEV_E440&SUBSYS_01108086".to_string()), "{:?}", w.hardware_ids);
        assert!(w.hardware_ids.iter().all(|i| i.starts_with("PCI\\VEN_8086&DEV_") && i.contains("&SUBSYS_")));
    }

    #[test]
    fn a_package_whose_devices_are_at_other_versions_is_not_given_the_package_version() {
        let m = model();
        // The Intel ME installer lists its devices at versions that are not the package's
        // 2613.9.28.0: each group has the version its devices end up with.
        let me: Vec<_> = m.drivers.iter().filter(|d| d.name.contains("Management Engine")).collect();
        assert_eq!(me.len(), 2, "{me:?}");
        assert!(me.iter().all(|d| d.version != "2613.9.28.0" && !d.hardware_ids.is_empty()));
        let base = me.iter().find(|d| d.version == "2603.9.4.0").unwrap();
        assert_eq!((base.id.as_str(), base.hardware_ids.len()), ("M6WYJ:2603.9.4.0", 5));
        // A tool and a firmware package: no ids, whatever their version.
        let tool = m.drivers.iter().find(|d| d.name == "Dell Command | Update").unwrap();
        assert_eq!((tool.kind.as_str(), tool.version.as_str(), tool.hardware_ids.len(), tool.installed.clone()), ("Utility", "5.7.2", 0, None));
        assert!(tool.install.is_none(), "only drivers are installed here");
        let fw = m.drivers.iter().find(|d| d.name.contains("SN530")).unwrap();
        assert_eq!((fw.kind.as_str(), fw.hardware_ids.len(), fw.install.is_none()), ("Firmware", 0, true));
    }

    #[test]
    fn a_dell_driver_installs_checked_and_signed_with_the_silent_switch() {
        let m = model();
        let d = find(&m, "FY1DP");
        let i = d.install.as_ref().unwrap();
        assert_eq!(i.url, "https://downloads.dell.com/FOLDER13578498M/5/Realtek-PCIe-Memory-Card-Reader-Driver_FY1DP_WIN64_10.0.26200.21385_A01_01.EXE");
        assert_eq!((i.md5.as_deref(), i.signer.as_str(), i.args.clone()), (Some("1f1f77936ac6cc724411e170ee74cc4b"), "Dell Inc", vec!["/s".to_string()]));
        assert_eq!(i.sha256.as_deref(), Some("57059211a20a464e464d129d71898455ef935ae8a50f94fdb522f9b0a8ab2b4e"));
        assert_eq!((d.size, d.date.as_deref()), (Some(11715128), Some("2025.10.15")));
        let plan = plan_install("driver:maker:dell:fy1dp", "Realtek PCIe Memory Card Reader Driver", d).unwrap();
        assert_eq!(plan.downloads[0].sha256.as_deref(), i.sha256.as_deref());
        assert!(matches!(&plan.steps[0], crate::sources::Step::CheckSigner { signer, .. } if signer == "Dell Inc"));
        let crate::sources::Step::Run { args, ok_codes, elevated, .. } = &plan.steps[1] else { panic!() };
        assert!(*elevated && args == &vec!["/s".to_string()] && ok_codes.contains(&2) && ok_codes.contains(&3010));
        assert_eq!(plan.to_version, "10.0.26200.21385");
    }

    #[test]
    fn a_pc_of_another_kind_gets_the_packages_for_it() {
        // No ARM64 packages in the cut-down catalog, and an x64 one is not for an ARM64 PC.
        assert!(parse(CATALOG, "arm64").unwrap().drivers.is_empty());
        assert!(parse(CATALOG, "arm64").unwrap().bios.is_some());
        assert!(parse("<Manifest/>", "x64").is_err());
        assert!(parse("not xml", "x64").is_err());
    }

    #[test]
    fn text_is_read_from_utf16_and_utf8_files() {
        let utf16: Vec<u8> = [0xFFu8, 0xFE].into_iter().chain("<a>é</a>".encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(decode_text(&utf16), "<a>é</a>");
        assert_eq!(decode_text(&[0xEF, 0xBB, 0xBF, b'<', b'a', b'/', b'>']), "<a/>");
        assert_eq!(decode_text(b"<a/>"), "<a/>");
        // The catalogs say utf-16 in their first line though the text is already decoded.
        assert!(INDEX_XML.contains("encoding=\"utf-16\"") && roxmltree::Document::parse(INDEX_XML).is_ok());
    }

    #[test]
    fn a_cabinet_is_unpacked_and_its_folder_removed() {
        // Windows' own makecab builds a small cabinet to unpack again.
        let dir = std::env::temp_dir().join(format!("u3d-cabtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("catalog.xml"), b"<Manifest><SoftwareComponent/></Manifest>").unwrap();
        let sys = std::path::PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into())).join("System32");
        let made = crate::winutil::cmd(sys.join("makecab.exe")).current_dir(&dir).arg("catalog.xml").arg("catalog.cab").output().unwrap();
        assert!(made.status.success());
        let bytes = std::fs::read(dir.join("catalog.cab")).unwrap();
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        assert_eq!(expand_cab_in(&work, &bytes).unwrap(), b"<Manifest><SoftwareComponent/></Manifest>");
        assert_eq!(std::fs::read_dir(&work).unwrap().count(), 0, "the unpack folder is removed");
        assert!(expand_cab_in(&work, b"not a cabinet").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Dell's real catalogs for a public sample model (a Latitude 5320): `cargo test --release
    /// --lib live_dell -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_dell() {
        let b = board("0A1F", "1.21.0");
        println!("{:#?}", check(&b).await);
        let list = drivers(&b).await;
        println!("{} entries; {} with hardware ids", list.len(), list.iter().filter(|d| !d.hardware_ids.is_empty()).count());
        for d in list.iter().take(12) {
            println!("{} | {} | {} | {} | ids {} | install {}", d.id, d.kind, d.name, d.version, d.hardware_ids.len(), d.install.is_some());
        }
        assert!(matches!(check(&b).await, Answer::Listed(_)));
        assert!(!list.is_empty());
        assert_eq!(fetch("FFFF").await.unwrap(), Lookup::NotListed);
        // An older Latitude (E4300), whose BIOS version is a letter and a number.
        let old = board("024D", "A24");
        println!("{:?} / {} entries", check(&old).await, drivers(&old).await.len());
    }
}
