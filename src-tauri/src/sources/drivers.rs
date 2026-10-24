//! Drivers, listed with the apps (group "Drivers"): every graphics card with its maker's own
//! newest driver (NVIDIA, AMD, Intel), every other device that has a driver from its maker,
//! Intel's own drivers for Intel Wi-Fi, Bluetooth and network adapters, the AMD chipset drivers,
//! the BIOS (the newest one the board's or PC's maker lists, see `board`), and every driver update Windows
//! Update offers this PC (`Type='Driver'`, the same list as Settings > Windows Update > Optional
//! updates). Microsoft signs every Windows Update driver; a maker's own package must be signed by
//! that maker. Windows Update installs run this program again elevated
//! (`--install-driver <update id>`), because the agent only installs for administrators.

use super::{Plan, Step, Verify};
use crate::model::{AppStatus, DriverEntry, JobKind};
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// The command line flag the elevated helper runs a driver install with.
pub const INSTALL_FLAG: &str = "--install-driver";
/// Exit code of the install when Windows needs a restart to finish it (as Windows Installer).
pub const EXIT_REBOOT: u32 = 3010;

/// One driver update Windows Update offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// Windows Update's id for this update
    pub update_id: String,
    /// "Intel Corporation - Display - 31.0.101.5590"
    pub title: String,
    /// The hardware id it is for: "PCI\VEN_8086&DEV_9A49"
    pub hardware_id: String,
    pub manufacturer: String,
    /// Device class: "Display", "Net", "Bluetooth"
    pub class: String,
    pub model: String,
    /// Driver date, "2024.05.01" (dots, so dates compare like versions)
    pub date: Option<String>,
    pub size: Option<u64>,
    /// Windows Update's description of the update
    #[serde(default)]
    pub description: String,
    /// "More information" link (only a Microsoft or maker address is kept)
    #[serde(default)]
    pub support_url: Option<String>,
}

impl Offer {
    /// The driver version, from the end of the title (Windows Update has no field for it).
    pub fn version(&self) -> Option<String> {
        static VER: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+(?:\.\d+){1,3})\)?\s*$").unwrap());
        VER.captures(self.title.trim()).map(|c| c[1].to_string())
    }
}

/// What the last Windows Update check found.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Feed {
    pub offers: Vec<Offer>,
}

/// A driver installed on this PC, from the device class keys in the registry.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Present {
    /// The hardware id the driver was installed for
    pub matching_id: String,
    pub version: String,
    /// "2024.05.01"
    pub date: Option<String>,
    pub desc: String,
    pub provider: String,
    /// Device class: "Display", "Net", "MEDIA", "Bluetooth"
    pub class: String,
}

// ---------------------------------------------------------------- links and text

/// Is `url` an https address on one of `hosts` or a subdomain of one? Only the real host counts:
/// "https://evil.example/x.nvidia.com/" and "https://nvidia.com.evil.example/" do not.
pub fn host_is(url: &str, hosts: &[&str]) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let Some(h) = u.host_str().map(|h| h.to_ascii_lowercase()) else { return false };
    u.scheme() == "https" && hosts.iter().any(|w| h == *w || h.ends_with(&format!(".{w}")))
}

/// The makers' and Microsoft's sites: the only addresses a driver's links may open.
const LINK_HOSTS: [&str; 13] = ["nvidia.com", "amd.com", "intel.com", "gigabyte.com", "microsoft.com", "windowsupdate.com", "realtek.com", "asus.com", "msi.com", "asrock.com", "dell.com", "hp.com", "lenovo.com"];

/// A link from a maker's answer, kept only when it points to a maker's or Microsoft's site.
pub fn safe_link(url: Option<String>) -> Option<String> {
    url.map(|u| u.trim().to_string()).filter(|u| host_is(u, &LINK_HOSTS))
}

/// "%3Cb%3EGame%20Ready" as "<b>Game Ready" (NVIDIA sends its notes this way).
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A maker's release notes (HTML) as note lines: a bold line on its own ("<b>Fixed Gaming
/// Bugs</b><br>") or a heading is a section heading, list items and the other lines are changes.
pub fn html_notes(html: &str) -> Vec<String> {
    static DROP: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<(script|style|img)\b[^>]*>(.*?</(script|style)>)?").unwrap());
    static HEAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<(?:h[1-6]|b|strong)\b[^>]*>(.*?)</(?:h[1-6]|b|strong)>\s*(?:<br\s*/?>|</p>|<p\b|<ul\b|$)").unwrap());
    static BREAK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<br\s*/?>|</?(p|div|ul|ol|tr|table|h[1-6])\b[^>]*>").unwrap());
    static ITEM: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)<li\b[^>]*>").unwrap());
    let t = DROP.replace_all(html, "");
    let t = HEAD.replace_all(&t, |c: &regex::Captures| format!("\n\u{1}{}\n", &c[1]));
    let t = ITEM.replace_all(&t, "\n");
    let t = BREAK.replace_all(&t, "\n");
    let mut out: Vec<String> = Vec::new();
    for line in t.lines() {
        let head = line.trim_start().starts_with('\u{1}');
        let text = crate::notes::strip_tags(line.trim_start().trim_start_matches('\u{1}'));
        if text.is_empty() {
            continue;
        }
        let line = if head && text.chars().count() <= 90 { format!("### {text}") } else { text };
        if out.last() != Some(&line) {
            out.push(line);
        }
        if out.len() >= 200 {
            break;
        }
    }
    crate::notes::tidy_headings(out)
}

// ---------------------------------------------------------------- checking Windows Update

/// Ask Windows Update for the driver updates it offers this PC. Slow (seconds to a minute): it
/// asks Microsoft's servers. Runs on a blocking thread.
pub async fn fetch() -> Result<Feed> {
    tauri::async_runtime::spawn_blocking(search).await.map_err(|e| anyhow!("{e}"))?
}

fn search() -> Result<Feed> {
    use windows::core::{Interface, BSTR};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::UpdateAgent::*;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let session: IUpdateSession = CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
        let _ = session.SetClientApplicationID(&BSTR::from("The Updater 3D"));
        let searcher = session.CreateUpdateSearcher()?;
        let found = searcher.Search(&BSTR::from("IsInstalled=0 and Type='Driver' and IsHidden=0"))?;
        let list = found.Updates()?;
        let mut offers = Vec::new();
        for k in 0..list.Count()? {
            let u = list.get_Item(k)?;
            let Ok(d) = u.cast::<IWindowsDriverUpdate>() else { continue };
            let s = |r: windows::core::Result<BSTR>| r.map(|b| b.to_string()).unwrap_or_default();
            let support = s(u.SupportUrl());
            offers.push(Offer {
                update_id: s(u.Identity().and_then(|i| i.UpdateID())),
                title: s(u.Title()),
                hardware_id: s(d.DriverHardwareID()),
                manufacturer: s(d.DriverManufacturer()),
                class: s(d.DriverClass()),
                model: s(d.DriverModel()),
                date: d.DriverVerDate().ok().and_then(ole_date),
                size: u.MaxDownloadSize().ok().map(|m| m.Anonymous2.Lo64).filter(|&n| n > 0),
                description: s(u.Description()),
                support_url: safe_link((!support.is_empty()).then_some(support)),
            });
        }
        Ok(Feed { offers: offers.into_iter().filter(|o| !o.update_id.is_empty()).collect() })
    }
}

/// An OLE date (days since 1899-12-30) as "2024.05.01".
fn ole_date(d: f64) -> Option<String> {
    let base = chrono::NaiveDate::from_ymd_opt(1899, 12, 30)?;
    (d > 0.0).then(|| (base + chrono::Duration::days(d as i64)).format("%Y.%m.%d").to_string())
}

/// "5-1-2024" (the registry's DriverDate) as "2024.05.01".
fn reg_date(s: &str) -> Option<String> {
    let mut p = s.trim().split('-').map(|x| x.parse::<u32>().ok());
    let (m, d, y) = (p.next()??, p.next()??, p.next()??);
    chrono::NaiveDate::from_ymd_opt(y as i32, m, d).map(|x| x.format("%Y.%m.%d").to_string())
}

// ---------------------------------------------------------------- what is installed

/// Drivers installed for devices, from HKLM\SYSTEM\CurrentControlSet\Control\Class.
pub fn present() -> Vec<Present> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let Ok(root) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("SYSTEM\\CurrentControlSet\\Control\\Class", KEY_READ) else { return vec![] };
    let mut out = Vec::new();
    for class in root.enum_keys().flatten() {
        let Ok(ck) = root.open_subkey_with_flags(&class, KEY_READ) else { continue };
        let class_name: String = ck.get_value("Class").unwrap_or_default();
        for inst in ck.enum_keys().flatten().filter(|k| k.chars().all(|c| c.is_ascii_digit())) {
            let Ok(k) = ck.open_subkey_with_flags(&inst, KEY_READ) else { continue };
            let g = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
            let matching_id = g("MatchingDeviceId");
            if matching_id.is_empty() {
                continue;
            }
            out.push(Present { matching_id, version: g("DriverVersion"), date: reg_date(&g("DriverDate")), desc: g("DriverDesc"), provider: g("ProviderName"), class: class_name.clone() });
        }
    }
    out
}

/// A device driver worth listing: from the device's maker (not one of Windows' own), for real
/// hardware (not a software component or a virtual device).
pub fn listed(p: &Present) -> bool {
    const SKIP_CLASSES: [&str; 10] = ["softwarecomponent", "extension", "softwaredevice", "legacydriver", "volume", "volumesnapshot", "printqueue", "computer", "system devices filter", "netservice"];
    const BUSES: [&str; 9] = ["pci\\", "usb\\", "hdaudio\\", "acpi\\", "hid\\", "bth", "intelaudio\\", "scsi\\", "mf\\"];
    let provider = p.provider.trim().to_ascii_lowercase();
    let id = p.matching_id.to_ascii_lowercase();
    !provider.is_empty()
        && !provider.starts_with("microsoft")
        && !p.desc.trim().is_empty()
        && !SKIP_CLASSES.contains(&p.class.to_ascii_lowercase().as_str())
        && BUSES.iter().any(|b| id.starts_with(b))
}

/// The installed driver an offer replaces: same hardware id, or one that starts with the other
/// (Windows Update names the device more or less exactly than the driver matched it).
pub fn matching<'a>(o: &Offer, present: &'a [Present]) -> Option<&'a Present> {
    let h = o.hardware_id.to_ascii_lowercase();
    if h.is_empty() {
        return None;
    }
    present.iter().find(|p| p.matching_id.eq_ignore_ascii_case(&h)).or_else(|| {
        present
            .iter()
            .filter(|p| {
                let m = p.matching_id.to_ascii_lowercase();
                h.starts_with(&m) || m.starts_with(&h)
            })
            .max_by_key(|p| p.matching_id.len())
    })
}

/// The installed and offered version of an update, in a form that compares: driver versions when
/// both are known and the offer is newer, otherwise the driver dates.
pub fn versions(o: &Offer, p: Option<&Present>) -> (String, String) {
    let offered = o.version();
    if let (Some(have), Some(new)) = (p.map(|p| p.version.as_str()).filter(|v| !v.is_empty()), offered.as_deref()) {
        if super::update_offered(new, have, false) {
            return (have.to_string(), new.to_string());
        }
    }
    // Windows Update gives a date for every driver; "1" only so one without still compares.
    let new = o.date.clone().or(offered).unwrap_or_else(|| "1".into());
    match p.and_then(|p| p.date.clone()) {
        Some(d) if super::update_offered(&new, &d, false) => (d, new),
        // Windows Update offers it, so it is an update even when the dates do not say so.
        _ => ("0".into(), new),
    }
}

/// How an update is installed, kept next to its entry (and saved with it, so a queued driver job
/// still installs after the app restarts).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum How {
    WindowsUpdate { update_id: String },
    Nvidia(super::nvidia::Release),
    Amd(super::amd::Release),
    Intel(super::intel::Release),
    AmdChipset(super::chipset::Release),
    /// A package from the board's or PC's maker (Dell, HP, Lenovo catalogs)
    Maker(Box<super::board::MakerDriver>),
}

/// A driver as the apps list shows it, with how to install its update (if there is one).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Found {
    pub entry: DriverEntry,
    pub how: Option<How>,
}

/// AMD and Intel graphics when their maker could not be reached (or has no driver for the card):
/// Windows Update's tested build, often a little older, and a pointer to the maker's updater.
const VENDOR_GPUS: [(&str, &str, &str, &str); 2] = [
    ("pci\\ven_1002", "AMD", "AMD Software: Adrenalin Edition", "https://www.amd.com/en/support/download/drivers.html"),
    ("pci\\ven_8086", "Intel", "Intel Driver & Support Assistant", "https://www.intel.com/content/www/us/en/support/detect.html"),
];

/// "Display" is a graphics card, "Net" a network adapter: names people know.
fn kind_of(class: &str) -> String {
    match class.to_ascii_lowercase().as_str() {
        "display" => "Graphics",
        "net" | "networking" => "Network",
        "bluetooth" => "Bluetooth",
        "media" | "audioendpoint" => "Audio",
        "camera" | "image" => "Camera",
        "hidclass" | "keyboard" | "mouse" => "Input",
        "usb" => "USB",
        "system" => "System",
        "hdc" | "scsiadapter" | "diskdrive" => "Storage",
        "printer" => "Printer",
        "monitor" => "Monitor",
        "firmware" => "Firmware",
        "securitydevices" => "Security",
        "modem" => "Modem",
        "" | "otherhardware" => "Other",
        _ => return class.to_string(),
    }
    .into()
}

/// "Realtek Semiconductor Corp." as "Realtek", "Advanced Micro Devices, Inc." as "AMD".
pub fn short_maker(m: &str) -> String {
    let l = m.trim().to_ascii_lowercase();
    if l.starts_with("advanced micro devices") || l.starts_with("amd") {
        return "AMD".into();
    }
    if l.starts_with("nvidia") {
        return "NVIDIA".into();
    }
    if l.starts_with("intel") {
        return "Intel".into();
    }
    if l.starts_with("gigabyte") {
        return "Gigabyte".into();
    }
    let cut = [" corporation", " corp.", " corp", " semiconductor", " technology", " co.,", " co.", ", inc", " inc.", " inc", " ltd", " limited", " gmbh"];
    let mut s = m.trim().to_string();
    for c in cut {
        if let Some(i) = s.to_ascii_lowercase().find(c) {
            s.truncate(i);
        }
    }
    let s = s.trim().trim_end_matches(',').trim().to_string();
    if s.is_empty() { m.trim().to_string() } else { s }
}

fn base_entry(id: String, name: String, kind: String, maker: String, source: &str, version: String) -> DriverEntry {
    DriverEntry { id, name, kind, maker, source: source.into(), version, latest: None, status: AppStatus::Current, size: None, date: None, notes_url: None, vendor_url: None, note: None, notes: vec![] }
}

/// NVIDIA's answer for one card: its matching id, product id ("" when NVIDIA does not list it)
/// and newest release (None when NVIDIA could not be reached).
pub type NvidiaAnswer = (String, String, Option<super::nvidia::Release>);

/// Everything one driver check knows. None: that source was not reached (or not asked).
#[derive(Default)]
pub struct Inputs<'a> {
    pub wu: Option<&'a Feed>,
    pub present: &'a [Present],
    pub nvidia: &'a [NvidiaAnswer],
    pub amd: Option<&'a super::amd::Release>,
    pub intel: Option<&'a [super::intel::Config]>,
    /// The AMD chipset drivers: the installed version and AMD's newest
    pub chipset: Option<(&'a str, Option<&'a super::chipset::Release>)>,
    pub bios: Option<&'a super::board::Bios>,
    /// Drivers and tools the board's or PC's maker lists for it, and the maker's name
    pub maker: &'a [super::board::MakerDriver],
    pub maker_name: &'a str,
    pub windows11: bool,
}

/// Does a hardware id from a maker's list name this installed device? The same id, or one
/// that is the other with more parts ("PCI\VEN_8086&DEV_A0F0" and "...&SUBSYS_0A1F1028").
pub fn hw_match(listed: &str, installed: &str) -> bool {
    let (a, b) = (listed.trim().to_ascii_uppercase(), installed.trim().to_ascii_uppercase());
    !a.is_empty() && !b.is_empty() && (a == b || a.starts_with(&format!("{b}&")) || b.starts_with(&format!("{a}&")))
}

/// Everything listed under Drivers, one entry per device.
pub fn collect(inp: &Inputs) -> Vec<Found> {
    let present = inp.present;
    let intel_for = |p: &Present, cat: Option<&str>| inp.intel.and_then(|list| super::intel::for_device(p, list, inp.windows11, cat));
    // A maker's own driver covers these devices: Windows Update's is not listed for them.
    let mut maker_has: Vec<String> = Vec::new();
    let mut out: Vec<Found> = Vec::new();
    // Which entry each device is listed under (devices with the same driver share one).
    let mut by_device: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

    // Graphics cards first.
    for p in present.iter().filter(|p| p.class.eq_ignore_ascii_case("Display")) {
        let low = p.matching_id.to_ascii_lowercase();
        if super::nvidia::is_gpu(p) {
            let have = super::nvidia::nv_version(&p.version).unwrap_or_else(|| p.version.clone());
            let (pfid, rel) = inp.nvidia.iter().find(|(m, _, _)| m.eq_ignore_ascii_case(&p.matching_id)).map(|(_, id, r)| (id.clone(), r.clone())).unwrap_or_default();
            let id = if pfid.is_empty() { format!("driver:nvidia:{low}") } else { format!("driver:nvidia:{pfid}") };
            if out.iter().any(|f| f.entry.id == id) {
                continue;
            }
            let mut e = base_entry(id, p.desc.clone(), "Graphics".into(), "NVIDIA".into(), "nvidia", have.clone());
            let how = match rel {
                Some(r) => {
                    e.latest = Some(r.version.clone());
                    e.date = r.date.clone();
                    e.notes_url = r.notes_url.clone();
                    e.notes = r.notes.clone();
                    let newer = super::update_offered(&r.version, &have, false);
                    e.size = if newer { r.size.or(r.approx_size) } else { None };
                    e.status = if newer { AppStatus::Update } else { AppStatus::Current };
                    e.note = Some(format!("{} from NVIDIA", r.name));
                    newer.then_some(How::Nvidia(r))
                }
                None => {
                    e.status = AppStatus::Unknown;
                    e.note = Some(if pfid.is_empty() { "NVIDIA does not list this card".into() } else { "Could not reach NVIDIA".into() });
                    None
                }
            };
            out.push(Found { entry: e, how });
        } else if let (true, Some(r)) = (super::amd::is_gpu(p), inp.amd) {
            let id = format!("driver:amd:{low}");
            if out.iter().any(|f| f.entry.id == id) {
                continue;
            }
            let mut e = base_entry(id, p.desc.clone(), "Graphics".into(), "AMD".into(), "amd", p.version.clone());
            let newer = super::amd::needs_update(&p.version, r);
            e.latest = Some(if newer { r.store_versions[0].clone() } else { p.version.clone() });
            e.status = if newer { AppStatus::Update } else { AppStatus::Current };
            e.size = if newer { r.size } else { None };
            e.notes_url = safe_link(Some(r.notes_url.clone()));
            e.notes = r.notes.clone();
            e.note = Some(format!("Adrenalin Edition {} from AMD", r.version));
            maker_has.push(low.clone());
            out.push(Found { entry: e, how: newer.then(|| How::Amd(r.clone())) });
        } else if let (true, Some(r)) = (super::intel::is_gpu(p), intel_for(p, Some("Graphics"))) {
            let id = format!("driver:intel:{low}");
            if out.iter().any(|f| f.entry.id == id) {
                continue;
            }
            let mut e = base_entry(id, p.desc.clone(), "Graphics".into(), "Intel".into(), "intel", p.version.clone());
            intel_entry(&mut e, &r, &p.version);
            let how = (e.status == AppStatus::Update).then_some(How::Intel(r));
            maker_has.push(low.clone());
            out.push(Found { entry: e, how });
        } else if let Some((_, maker, tool, url)) = VENDOR_GPUS.iter().find(|(v, ..)| low.starts_with(v)) {
            let mut e = base_entry(format!("driver:gpu:{low}"), p.desc.clone(), "Graphics".into(), maker.to_string(), "vendor", p.version.clone());
            e.date = p.date.clone();
            e.vendor_url = Some(url.to_string());
            e.status = if inp.wu.is_some() { AppStatus::Current } else { AppStatus::Unknown };
            e.note = Some(format!("Newest {maker} drivers come from {tool}"));
            out.push(Found { entry: e, how: None });
        } else {
            continue;
        }
        by_device.insert(low, out.len() - 1);
    }

    // Every other device with its maker's driver. Intel's own newer driver when Intel lists one.
    for p in present.iter().filter(|p| !p.class.eq_ignore_ascii_case("Display") && listed(p)) {
        let low = p.matching_id.to_ascii_lowercase();
        if by_device.contains_key(&low) {
            continue;
        }
        // The same driver on several devices (USB ports, audio endpoints): one entry.
        if let Some(k) = out.iter().position(|f| f.entry.source == "device" && f.entry.name.eq_ignore_ascii_case(&p.desc) && f.entry.version == p.version) {
            by_device.insert(low, k);
            continue;
        }
        let maker = short_maker(&p.provider);
        let mut e = base_entry(format!("driver:dev:{low}"), p.desc.clone(), kind_of(&p.class), maker.clone(), "device", p.version.clone());
        e.date = p.date.clone();
        let how = match intel_for(p, None) {
            Some(r) => {
                e.source = "intel".into();
                e.maker = "Intel".into();
                intel_entry(&mut e, &r, &p.version);
                maker_has.push(low.clone());
                (e.status == AppStatus::Update).then_some(How::Intel(r))
            }
            None => {
                // No offer from Windows Update means it has nothing newer; without an answer
                // from it, nobody said.
                e.status = if inp.wu.is_some() { AppStatus::Current } else { AppStatus::Unknown };
                e.latest = inp.wu.is_some().then(|| p.version.clone());
                e.note = Some(format!("Driver from {maker}"));
                None
            }
        };
        out.push(Found { entry: e, how });
        by_device.insert(low, out.len() - 1);
    }

    // What Windows Update offers, one per device, the newer of two for the same one. Devices
    // with their maker's own newer driver keep that one.
    if let Some(feed) = inp.wu {
        for o in &feed.offers {
            let p = matching(o, present);
            if p.is_some_and(super::nvidia::is_gpu) {
                continue;
            }
            let (version, latest) = versions(o, p);
            let low = p.map(|p| p.matching_id.to_ascii_lowercase());
            let slot = low.as_ref().and_then(|l| by_device.get(l).copied());
            if let (Some(k), Some(l)) = (slot, low.as_ref()) {
                // The maker's own driver is at least as new: it stays.
                if maker_has.contains(l) && out[k].entry.latest.as_deref().is_some_and(|m| !super::update_offered(&latest, m, false)) {
                    continue;
                }
            }
            let id = match (&slot, &low) {
                (Some(k), _) => out[*k].entry.id.clone(),
                (None, Some(l)) => format!("driver:dev:{l}"),
                (None, None) => format!("driver:wu:{}", if o.hardware_id.is_empty() { &o.update_id } else { &o.hardware_id }.to_ascii_lowercase()),
            };
            let name = p.map(|p| p.desc.clone()).filter(|d| !d.is_empty()).or_else(|| (!o.model.is_empty()).then(|| o.model.clone())).unwrap_or_else(|| o.title.clone());
            let maker = short_maker(if o.manufacturer.is_empty() { p.map(|p| p.provider.as_str()).unwrap_or_default() } else { &o.manufacturer });
            let kind = kind_of(if o.class.is_empty() { p.map(|p| p.class.as_str()).unwrap_or("") } else { &o.class });
            let mut e = base_entry(id.clone(), name, kind, maker, "windowsUpdate", if version == "0" { "None".into() } else { version });
            e.latest = Some(latest.clone());
            e.status = AppStatus::Update;
            e.size = o.size;
            e.date = o.date.clone();
            e.notes_url = o.support_url.clone();
            e.notes = [o.title.trim(), o.description.trim()].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
            e.note = Some("From Windows Update. Some drivers need a restart.".into());
            let found = Found { entry: e, how: Some(How::WindowsUpdate { update_id: o.update_id.clone() }) };
            match slot {
                Some(k) => {
                    let old = &out[k];
                    // Two offers for one device: the newer one.
                    if old.entry.source == "windowsUpdate" && old.entry.latest.as_deref().is_some_and(|l| !super::update_offered(&latest, l, false)) {
                        continue;
                    }
                    let mut f = found;
                    // An AMD or Intel card keeps the link to its maker's updater.
                    f.entry.vendor_url = old.entry.vendor_url.clone();
                    if old.entry.kind == "Graphics" {
                        f.entry.kind = "Graphics".into();
                    }
                    out[k] = f;
                }
                None => {
                    if let Some(l) = low {
                        by_device.insert(l, out.len());
                    }
                    match out.iter_mut().find(|f| f.entry.id == id) {
                        Some(f) if f.entry.latest.as_deref().is_some_and(|l| super::update_offered(&latest, l, false)) => *f = found,
                        Some(_) => {}
                        None => out.push(found),
                    }
                }
            }
        }
    }

    // What the board's or PC's maker lists: for its devices when it is newer than what is
    // listed for them, and its own tools whose installed version the maker says how to read.
    let maker_src = inp.maker_name.to_ascii_lowercase();
    for d in inp.maker {
        let how = d.install.clone().map(|_| How::Maker(Box::new(d.clone())));
        let fill = |e: &mut DriverEntry, have: &str| {
            let newer = super::update_offered(&d.version, have, false);
            e.latest = Some(d.version.clone());
            e.status = if newer { AppStatus::Update } else { AppStatus::Current };
            e.size = if newer { d.size } else { None };
            e.date = d.date.clone();
            e.notes = d.notes.clone();
            e.notes_url = safe_link(d.page.clone());
            e.source = maker_src.clone();
            if newer && d.install.is_none() {
                e.vendor_url = safe_link(d.page.clone());
                e.note = Some(format!("{} from {}: get it from {}'s page", d.name, inp.maker_name, inp.maker_name));
            } else {
                e.note = Some(format!("{} from {}", d.name, inp.maker_name));
            }
            newer
        };
        let devices: Vec<&Present> = present.iter().filter(|p| d.hardware_ids.iter().any(|h| hw_match(h, &p.matching_id))).collect();
        if !devices.is_empty() {
            for p in devices {
                let Some(&k) = by_device.get(&p.matching_id.to_ascii_lowercase()) else { continue };
                // Not newer than the installed driver, or than what is already offered for it.
                if !super::update_offered(&d.version, &p.version, false) {
                    continue;
                }
                let cur = &out[k];
                let offered = cur.how.is_some() && cur.entry.latest.as_deref().is_some_and(|l| !super::update_offered(&d.version, l, false));
                if offered {
                    continue;
                }
                let mut e = cur.entry.clone();
                fill(&mut e, &p.version);
                out[k] = Found { entry: e, how: how.clone() };
            }
            continue;
        }
        if let Some(have) = d.installed.as_deref() {
            let mut e = base_entry(format!("driver:maker:{maker_src}:{}", d.id.to_ascii_lowercase()), d.name.clone(), d.kind.clone(), inp.maker_name.to_string(), &maker_src, have.to_string());
            let newer = fill(&mut e, have);
            out.push(Found { entry: e, how: if newer { how } else { None } });
        }
    }

    // The AMD chipset drivers (one package for all of them).
    if let Some((have, rel)) = inp.chipset {
        let mut e = base_entry("driver:amd-chipset".into(), "AMD Chipset Software".into(), "Chipset".into(), "AMD".into(), "amd", have.to_string());
        let how = match rel {
            Some(r) => {
                let newer = super::update_offered(&r.version, have, false);
                e.latest = Some(r.version.clone());
                e.status = if newer { AppStatus::Update } else { AppStatus::Current };
                e.size = if newer { r.size } else { None };
                e.notes_url = safe_link(Some(r.notes_url.clone()));
                e.notes = r.notes.clone();
                e.note = Some("AMD's chipset drivers for Ryzen processors and AMD motherboards".into());
                newer.then(|| How::AmdChipset(r.clone()))
            }
            None => {
                e.status = AppStatus::Unknown;
                e.note = Some("Could not reach AMD".into());
                None
            }
        };
        out.push(Found { entry: e, how });
    }

    if let Some(b) = inp.bios {
        out.push(Found { entry: super::board::entry(b), how: None });
    }
    out
}

/// An entry for Intel's own driver package.
fn intel_entry(e: &mut DriverEntry, r: &super::intel::Release, have: &str) {
    let newer = super::update_offered(&r.version, have, false);
    e.latest = Some(r.version.clone());
    e.status = if newer { AppStatus::Update } else { AppStatus::Current };
    e.size = if newer { r.size } else { None };
    e.date = r.date.clone();
    e.notes_url = safe_link(r.notes_url.clone());
    e.notes = r.notes.clone();
    e.note = Some(if newer && !r.silent { format!("{} from Intel. Intel's installer opens: click through it to finish.", r.name) } else { format!("{} from Intel", r.name) });
}

/// Does Windows wait for a restart to finish installing something (driver, update)?
pub fn restart_pending() -> bool {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let hklm = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    [
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Component Based Servicing\\RebootPending",
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\WindowsUpdate\\Auto Update\\RebootRequired",
    ]
    .iter()
    .any(|k| hklm.open_subkey_with_flags(k, KEY_READ).is_ok())
}

/// Windows 11 (build 22000 and later): AMD and Intel have a package for each.
pub fn windows_11() -> bool {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
        .ok()
        .and_then(|b| b.parse::<u32>().ok())
        .is_some_and(|b| b >= 22000)
}

// ---------------------------------------------------------------- the saved list

/// What the last driver check found, so the list shows at once when the app starts (and a
/// driver job queued before a restart still knows what to install).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Saved {
    pub found: Vec<Found>,
    pub last_check: Option<String>,
    /// Windows Update's last answer: used again while it is asked anew (it is the slow one)
    #[serde(default)]
    pub wu: Option<Feed>,
}

pub fn load(file: &std::path::Path) -> Saved {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

pub fn save(file: &std::path::Path, s: &Saved) {
    if let Ok(b) = serde_json::to_vec(s) {
        let tmp = file.with_extension("json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, file);
        }
    }
}

pub fn saved_file() -> std::path::PathBuf {
    crate::paths::data_dir().join("drivers.json")
}

// ---------------------------------------------------------------- installing

/// The job that installs a driver update.
pub fn plan(f: &Found) -> Result<Plan> {
    let e = &f.entry;
    match f.how.as_ref().ok_or_else(|| anyhow!("{} is up to date", e.name))? {
        How::Nvidia(r) => super::nvidia::plan_install(&e.id, &e.name, r),
        How::Amd(r) => super::amd::plan_install(&e.id, &e.name, r),
        How::Intel(r) => super::intel::plan_install(&e.id, &e.name, r),
        How::AmdChipset(r) => super::chipset::plan_install(&e.id, &e.name, r),
        How::Maker(d) => super::board::plan_install(&e.id, &e.name, d),
        How::WindowsUpdate { update_id } => {
            let me = std::env::current_exe()?;
            Ok(Plan {
                app_id: e.id.clone(),
                kind: JobKind::Update,
                name: e.name.clone(),
                to_version: e.latest.clone().unwrap_or_default(),
                downloads: vec![],
                steps: vec![Step::Run {
                    exe: me,
                    args: vec![INSTALL_FLAG.into(), update_id.clone()],
                    cwd: None,
                    ok_codes: vec![0, EXIT_REBOOT],
                    elevated: true,
                    label: format!("Installing the {} driver through Windows Update", e.name),
                    log: None,
                }],
                block_dirs: vec![],
                block_names: vec![],
                verify: Verify::None,
                fallback: None,
                cleanup: vec![],
            })
        }
    }
}

/// `--install-driver <update id>`: download and install one driver update through Windows
/// Update. Exit code 0 when done, `EXIT_REBOOT` when Windows needs a restart, 1 on failure.
pub fn install_main(args: &[String]) -> i32 {
    let Some(id) = args.iter().skip_while(|a| *a != INSTALL_FLAG).nth(1) else {
        eprintln!("No update id");
        return 1;
    };
    match install(id) {
        Ok(true) => EXIT_REBOOT as i32,
        Ok(false) => 0,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}

/// Returns whether a restart is needed.
fn install(update_id: &str) -> Result<bool> {
    use windows::core::BSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::UpdateAgent::*;
    // An update id is a GUID: anything else is not passed to the search.
    if update_id.is_empty() || !update_id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        return Err(anyhow!("Not a Windows Update id: {update_id}"));
    }
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let session: IUpdateSession = CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
        let _ = session.SetClientApplicationID(&BSTR::from("The Updater 3D"));
        let found = session.CreateUpdateSearcher()?.Search(&BSTR::from(format!("UpdateID='{update_id}'")))?.Updates()?;
        if found.Count()? == 0 {
            return Err(anyhow!("Windows Update no longer offers this driver. Check for updates again."));
        }
        let update = found.get_Item(0)?;
        if !update.EulaAccepted()?.as_bool() {
            update.AcceptEula()?;
        }
        let todo: IUpdateCollection = CoCreateInstance(&UpdateCollection, None, CLSCTX_INPROC_SERVER)?;
        todo.Add(&update)?;

        if !update.IsDownloaded()?.as_bool() {
            println!("Downloading");
            let dl = session.CreateUpdateDownloader()?;
            dl.SetUpdates(&todo)?;
            let r = dl.Download()?;
            let code = r.ResultCode()?;
            if code != orcSucceeded && code != orcSucceededWithErrors {
                return Err(anyhow!("Windows Update could not download the driver (result {}, 0x{:08x})", code.0, r.HResult()? as u32));
            }
        }
        println!("Installing");
        let ins = session.CreateUpdateInstaller()?;
        ins.SetUpdates(&todo)?;
        let r = ins.Install()?;
        let code = r.ResultCode()?;
        if code != orcSucceeded && code != orcSucceededWithErrors {
            return Err(anyhow!("Windows Update could not install the driver (result {}, 0x{:08x})", code.0, r.HResult()? as u32));
        }
        Ok(r.RebootRequired()?.as_bool())
    }
}

#[cfg(test)]
mod tests;
