//! Device drivers that Windows Update offers for this PC. The Windows Update Agent finds them
//! (`Type='Driver'`, the same list as Settings > Windows Update > Optional updates), downloads
//! them and installs them; Microsoft has signed every one, so there is no checksum to fetch.
//! The install runs this program again elevated (`--install-driver <update id>`), because the
//! agent only installs for administrators.

use super::{Plan, Step, Verify};
use crate::model::{AppStatus, DriverEntry, JobKind};
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use regex::Regex;

/// The command line flag the elevated helper runs a driver install with.
pub const INSTALL_FLAG: &str = "--install-driver";
/// Exit code of the install when Windows needs a restart to finish it (as Windows Installer).
pub const EXIT_REBOOT: u32 = 3010;

/// One driver update Windows Update offers.
#[derive(Clone, Debug, Default, PartialEq)]
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
}

impl Offer {
    /// The driver version, from the end of the title (Windows Update has no field for it).
    pub fn version(&self) -> Option<String> {
        static VER: Lazy<Regex> = Lazy::new(|| Regex::new(r"(\d+(?:\.\d+){1,3})\)?\s*$").unwrap());
        VER.captures(self.title.trim()).map(|c| c[1].to_string())
    }
}

/// What the last check found.
#[derive(Clone, Debug, Default)]
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

// ---------------------------------------------------------------- checking

/// Ask Windows Update for the driver updates it offers this PC. Slow (seconds to a minute): it
/// asks Microsoft's servers. Runs on a blocking thread.
pub async fn fetch() -> Result<Feed> {
    tokio::task::spawn_blocking(search).await.map_err(|e| anyhow!("{e}"))?
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
            offers.push(Offer {
                update_id: s(u.Identity().and_then(|i| i.UpdateID())),
                title: s(u.Title()),
                hardware_id: s(d.DriverHardwareID()),
                manufacturer: s(d.DriverManufacturer()),
                class: s(d.DriverClass()),
                model: s(d.DriverModel()),
                date: d.DriverVerDate().ok().and_then(ole_date),
                size: u.MaxDownloadSize().ok().map(|m| m.Anonymous2.Lo64).filter(|&n| n > 0),
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

/// The installed driver an offer replaces: same hardware id, or one that starts with the other
/// (Windows Update names the device more or less exactly than the driver matched it).
pub fn matching<'a>(o: &Offer, present: &'a [Present]) -> Option<&'a Present> {
    let h = o.hardware_id.to_ascii_lowercase();
    if h.is_empty() {
        return None;
    }
    present.iter().find(|p| p.matching_id.eq_ignore_ascii_case(&h)).or_else(|| {
        present.iter().filter(|p| {
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

/// How an update is installed, kept next to its entry.
#[derive(Clone, Debug, PartialEq)]
pub enum How {
    WindowsUpdate { update_id: String },
    Nvidia(super::nvidia::Release),
    Amd(super::amd::Release),
    Intel(super::intel::Release),
}

/// A driver as the Drivers window lists it, with how to install its update (if there is one).
#[derive(Clone, Debug, PartialEq)]
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
        "" | "otherhardware" => "Other",
        _ => return class.to_string(),
    }
    .into()
}

fn base_entry(id: String, name: String, kind: String, maker: String, source: &str, version: String) -> DriverEntry {
    DriverEntry { id, name, kind, maker, source: source.into(), version, latest: None, status: AppStatus::Current, size: None, date: None, notes_url: None, vendor_url: None, note: None, job_id: None }
}

/// NVIDIA's answer for one card: its matching id, product id ("" when NVIDIA does not list it)
/// and newest release (None when NVIDIA could not be reached).
pub type NvidiaAnswer = (String, String, Option<super::nvidia::Release>);

/// Everything Drivers lists: every graphics card (its maker's own newest driver: NVIDIA, AMD,
/// Intel) and every driver update Windows Update offers. `amd` is AMD's newest release and
/// `intel` Intel's package list, None when the maker could not be reached.
pub fn collect(wu: Option<&Feed>, present: &[Present], nvidia: &[NvidiaAnswer], amd: Option<&super::amd::Release>, intel: Option<&[super::intel::Config]>) -> Vec<Found> {
    let intel_for = |p: &Present| intel.and_then(|list| super::intel::for_card(p, list, windows_11()));
    let mut maker_has: Vec<String> = Vec::new();
    let mut out: Vec<Found> = Vec::new();
    // Graphics cards first.
    for p in present.iter().filter(|p| p.class.eq_ignore_ascii_case("Display")) {
        let low = p.matching_id.to_ascii_lowercase();
        if super::nvidia::is_gpu(p) {
            let have = super::nvidia::nv_version(&p.version).unwrap_or_else(|| p.version.clone());
            let (pfid, rel) = nvidia.iter().find(|(m, _, _)| m.eq_ignore_ascii_case(&p.matching_id)).map(|(_, id, r)| (id.clone(), r.clone())).unwrap_or_default();
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
        } else if let (true, Some(r)) = (super::amd::is_gpu(p), amd) {
            let id = format!("driver:amd:{low}");
            if out.iter().any(|f| f.entry.id == id) {
                continue;
            }
            let mut e = base_entry(id, p.desc.clone(), "Graphics".into(), "AMD".into(), "amd", p.version.clone());
            let newer = super::amd::needs_update(&p.version, r);
            e.latest = Some(if newer { r.store_versions[0].clone() } else { p.version.clone() });
            e.status = if newer { AppStatus::Update } else { AppStatus::Current };
            e.size = if newer { r.size } else { None };
            e.notes_url = Some(r.notes_url.clone());
            e.note = Some(format!("Adrenalin Edition {} from AMD", r.version));
            maker_has.push(p.matching_id.to_ascii_lowercase());
            out.push(Found { entry: e, how: newer.then(|| How::Amd(r.clone())) });
        } else if let (true, Some(r)) = (super::intel::is_gpu(p), intel_for(p)) {
            let id = format!("driver:intel:{low}");
            if out.iter().any(|f| f.entry.id == id) {
                continue;
            }
            let mut e = base_entry(id, p.desc.clone(), "Graphics".into(), "Intel".into(), "intel", p.version.clone());
            let newer = super::update_offered(&r.version, &p.version, false);
            e.latest = Some(r.version.clone());
            e.status = if newer { AppStatus::Update } else { AppStatus::Current };
            e.size = if newer { r.size } else { None };
            e.date = r.date.clone();
            e.notes_url = r.notes_url.clone();
            e.note = Some(if newer && !r.silent { format!("{} from Intel. Intel's installer opens: click through it to finish.", r.name) } else { format!("{} from Intel", r.name) });
            maker_has.push(p.matching_id.to_ascii_lowercase());
            out.push(Found { entry: e, how: newer.then(|| How::Intel(r)) });
        } else if let Some((_, maker, tool, url)) = VENDOR_GPUS.iter().find(|(v, ..)| low.starts_with(v)) {
            let mut e = base_entry(format!("driver:gpu:{low}"), p.desc.clone(), "Graphics".into(), maker.to_string(), "vendor", p.version.clone());
            e.date = p.date.clone();
            e.vendor_url = Some(url.to_string());
            e.note = Some(format!("Newest {maker} drivers come from {tool}"));
            out.push(Found { entry: e, how: None });
        }
    }
    // Then what Windows Update offers, one per device, the newer of two for the same one. NVIDIA
    // and AMD cards are already covered by their maker's own (newer) driver.
    let Some(feed) = wu else { return out };
    for o in &feed.offers {
        let p = matching(o, present);
        if p.is_some_and(|p| super::nvidia::is_gpu(p) || maker_has.contains(&p.matching_id.to_ascii_lowercase())) {
            continue;
        }
        let (version, latest) = versions(o, p);
        let id = format!("driver:wu:{}", if o.hardware_id.is_empty() { &o.update_id } else { &o.hardware_id }.to_ascii_lowercase());
        let name = p.map(|p| p.desc.clone()).filter(|d| !d.is_empty()).or_else(|| (!o.model.is_empty()).then(|| o.model.clone())).unwrap_or_else(|| o.title.clone());
        let maker = if o.manufacturer.is_empty() { p.map(|p| p.provider.clone()).unwrap_or_default() } else { o.manufacturer.clone() };
        let kind = kind_of(if o.class.is_empty() { p.map(|p| p.class.as_str()).unwrap_or("") } else { &o.class });
        let mut e = base_entry(id.clone(), name, kind, maker, "windowsUpdate", if version == "0" { "None".into() } else { version });
        e.latest = Some(latest.clone());
        e.status = AppStatus::Update;
        e.size = o.size;
        e.date = o.date.clone();
        e.note = Some("From Windows Update. Some drivers need a restart.".into());
        let found = Found { entry: e, how: Some(How::WindowsUpdate { update_id: o.update_id.clone() }) };
        // An AMD or Intel card with a Windows Update driver: the update takes the place of the
        // "use the vendor's tool" entry and keeps its link.
        if let Some(gpu) = p.map(|p| format!("driver:gpu:{}", p.matching_id.to_ascii_lowercase())).and_then(|g| out.iter().position(|f| f.entry.id == g)) {
            let mut f = found;
            f.entry.vendor_url = out[gpu].entry.vendor_url.clone();
            out[gpu] = f;
            continue;
        }
        match out.iter_mut().find(|f| f.entry.id == id) {
            Some(f) if f.entry.latest.as_deref().is_some_and(|l| super::update_offered(&latest, l, false)) => *f = found,
            Some(_) => {}
            None => out.push(found),
        }
    }
    out
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

/// The job that installs a driver update.
pub fn plan(f: &Found) -> Result<Plan> {
    let e = &f.entry;
    match f.how.as_ref().ok_or_else(|| anyhow!("{} is up to date", e.name))? {
        How::Nvidia(r) => super::nvidia::plan_install(&e.id, &e.name, r),
        How::Amd(r) => super::amd::plan_install(&e.id, &e.name, r),
        How::Intel(r) => super::intel::plan_install(&e.id, &e.name, r),
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
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let session: IUpdateSession = CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
        let _ = session.SetClientApplicationID(&BSTR::from("The Updater 3D"));
        let found = session.CreateUpdateSearcher()?.Search(&BSTR::from(format!("UpdateID='{}'", update_id.replace('\'', ""))))?.Updates()?;
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
mod tests {
    use super::*;

    fn offer(title: &str, hw: &str, date: Option<&str>) -> Offer {
        Offer { update_id: "u1".into(), title: title.into(), hardware_id: hw.into(), class: "Display".into(), manufacturer: "Intel".into(), date: date.map(Into::into), ..Default::default() }
    }

    fn present(id: &str, ver: &str, date: Option<&str>) -> Present {
        Present { matching_id: id.into(), version: ver.into(), date: date.map(Into::into), desc: "Intel(R) Iris(R) Xe Graphics".into(), provider: "Intel Corporation".into(), class: "Display".into() }
    }

    #[test]
    fn the_version_comes_from_the_end_of_the_title() {
        assert_eq!(offer("Intel Corporation - Display - 31.0.101.5590", "", None).version().as_deref(), Some("31.0.101.5590"));
        assert_eq!(offer("Realtek Semiconductor Corp. - Extension - 6/12/2023 12:00:00 AM - 10.0.22621.31227", "", None).version().as_deref(), Some("10.0.22621.31227"));
        assert_eq!(offer("Intel net Driver Update (24.40.0.4)", "", None).version().as_deref(), Some("24.40.0.4"));
        assert_eq!(offer("Some driver update", "", None).version(), None);
    }

    #[test]
    fn dates_read_from_both_formats() {
        assert_eq!(reg_date("5-1-2024").as_deref(), Some("2024.05.01"));
        assert_eq!(reg_date("garbage"), None);
        assert_eq!(ole_date(45413.0).as_deref(), Some("2024.05.01"));
        assert_eq!(ole_date(0.0), None);
    }

    #[test]
    fn an_offer_matches_the_installed_driver_for_its_device() {
        let list = vec![present("pci\\ven_8086&dev_9a49", "31.0.101.4502", None), present("usb\\vid_046d", "1.0", None)];
        let o = offer("x", "PCI\\VEN_8086&DEV_9A49", None);
        assert_eq!(matching(&o, &list).unwrap().version, "31.0.101.4502");
        let longer = offer("x", "PCI\\VEN_8086&DEV_9A49&SUBSYS_00011028", None);
        assert_eq!(matching(&longer, &list).unwrap().version, "31.0.101.4502");
        assert!(matching(&offer("x", "HDAUDIO\\FUNC_01", None), &list).is_none());
        assert!(matching(&offer("x", "", None), &list).is_none());
    }

    #[test]
    fn every_offer_shows_as_an_update() {
        let o = offer("Intel - Display - 31.0.101.5590", "PCI\\VEN_8086&DEV_9A49", Some("2024.05.01"));
        // Versions when they compare.
        let p = present("PCI\\VEN_8086&DEV_9A49", "31.0.101.4502", Some("2023.06.01"));
        assert_eq!(versions(&o, Some(&p)), ("31.0.101.4502".into(), "31.0.101.5590".into()));
        // Dates when the version does not say it is newer.
        let p = present("PCI\\VEN_8086&DEV_9A49", "32.0.0.1", Some("2023.06.01"));
        assert_eq!(versions(&o, Some(&p)), ("2023.06.01".into(), "2024.05.01".into()));
        // Seen on a real PC: the version in brackets, and dates a few months apart.
        let wifi = offer("Intel net Driver Update (24.40.0.4)", "PCI\\X", Some("2026.04.13"));
        let p = present("PCI\\X", "24.20.0.4", Some("2026.01.06"));
        assert_eq!(versions(&wifi, Some(&p)), ("24.20.0.4".into(), "24.40.0.4".into()));
        let p = present("PCI\\X", "", Some("2026.01.06"));
        assert_eq!(versions(&wifi, Some(&p)), ("2026.01.06".into(), "2026.04.13".into()));
        // A device with no driver yet.
        let (have, new) = versions(&o, None);
        for (have, new) in [(have, new), versions(&offer("Driver", "x", None), None)] {
            assert!(super::super::update_offered(&new, &have, false), "{have} -> {new}");
        }
    }

    fn gpu(id: &str, desc: &str, ver: &str) -> Present {
        Present { matching_id: id.into(), version: ver.into(), desc: desc.into(), provider: "x".into(), class: "Display".into(), date: Some("2026.01.01".into()) }
    }

    #[test]
    fn one_entry_per_device_named_after_it() {
        let mut a = offer("Intel - Display - 31.0.101.5590", "PCI\\VEN_8086&DEV_9A49", None);
        let mut b = offer("Intel - Display - 31.0.101.5333", "pci\\ven_8086&dev_9a49", None);
        a.update_id = "a".into();
        b.update_id = "b".into();
        let p = vec![present("PCI\\VEN_8086&DEV_9A49", "31.0.101.4502", None)];
        let list = collect(Some(&Feed { offers: vec![b, a] }), &p, &[], None, None);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].entry.name, "Intel(R) Iris(R) Xe Graphics");
        assert_eq!((list[0].entry.kind.as_str(), list[0].entry.maker.as_str(), list[0].entry.latest.as_deref()), ("Graphics", "Intel", Some("31.0.101.5590")));
        assert_eq!(list[0].how, Some(How::WindowsUpdate { update_id: "a".into() }));
        // Without Windows Update the Intel card still shows, pointing to Intel's own updater.
        let alone = collect(None, &p, &[], None, None);
        assert_eq!((alone.len(), alone[0].entry.source.as_str(), alone[0].how.is_none()), (1, "vendor", true));
        assert!(collect(None, &[], &[], None, None).is_empty());
    }

    #[test]
    fn every_graphics_card_is_listed() {
        let rel = crate::sources::nvidia::Release { version: "621.01".into(), url: "u".into(), name: "GeForce Game Ready Driver".into(), ..Default::default() };
        let cards = vec![
            gpu("PCI\\VEN_10DE&DEV_2803", "NVIDIA GeForce RTX 4060 Ti", "32.0.16.1088"),
            gpu("PCI\\VEN_1002&DEV_744C", "AMD Radeon RX 7900 XTX", "32.0.21001.9024"),
            gpu("PCI\\VEN_8086&DEV_56A0", "Intel(R) Arc(TM) A770 Graphics", "32.0.101.6987"),
        ];
        let nv = vec![("PCI\\VEN_10DE&DEV_2803".to_string(), "1022".to_string(), Some(rel.clone()))];
        let list = collect(None, &cards, &nv, None, None);
        assert_eq!(list.len(), 3);
        let n = &list[0];
        assert_eq!((n.entry.id.as_str(), n.entry.version.as_str(), n.entry.latest.as_deref(), n.entry.status), ("driver:nvidia:1022", "610.88", Some("621.01"), AppStatus::Update));
        assert_eq!(n.how, Some(How::Nvidia(rel.clone())));
        assert_eq!((list[1].entry.source.as_str(), list[1].entry.maker.as_str()), ("vendor", "AMD"));
        assert!(list[1].how.is_none() && list[1].entry.vendor_url.is_some());
        assert_eq!(list[2].entry.maker, "Intel");
        // Up to date: listed, nothing to install.
        let cur = collect(None, &cards[..1], &[("PCI\\VEN_10DE&DEV_2803".into(), "1022".into(), Some(crate::sources::nvidia::Release { version: "610.88".into(), ..rel }))], None, None);
        assert_eq!((cur[0].entry.status, cur[0].how.is_none()), (AppStatus::Current, true));
        // NVIDIA not reached: still listed, status unknown.
        let off = collect(None, &cards[..1], &[], None, None);
        assert_eq!(off[0].entry.status, AppStatus::Unknown);
    }

    #[test]
    fn windows_update_does_not_double_up_graphics_cards() {
        let cards = vec![gpu("PCI\\VEN_10DE&DEV_2803", "NVIDIA GeForce RTX 4060 Ti", "32.0.16.1088"), gpu("PCI\\VEN_1002&DEV_744C", "AMD Radeon RX 7900 XTX", "32.0.21001.9024")];
        let mut nv_offer = offer("NVIDIA - Display - 32.0.15.9000", "PCI\\VEN_10DE&DEV_2803", None);
        nv_offer.update_id = "nv".into();
        let mut amd_offer = offer("Advanced Micro Devices - Display - 32.0.21025.1000", "PCI\\VEN_1002&DEV_744C", None);
        amd_offer.update_id = "amd".into();
        let list = collect(Some(&Feed { offers: vec![nv_offer, amd_offer] }), &cards, &[], None, None);
        assert_eq!(list.len(), 2, "{list:#?}");
        // NVIDIA's own driver wins; the AMD card gets Windows Update's driver and keeps AMD's link.
        assert_eq!(list[0].entry.source, "nvidia");
        assert_eq!((list[1].entry.source.as_str(), list[1].entry.latest.as_deref()), ("windowsUpdate", Some("32.0.21025.1000")));
        assert!(list[1].entry.vendor_url.is_some());
    }

    #[test]
    fn amd_cards_get_amds_own_driver_when_amds_site_answers() {
        let cards = vec![gpu("PCI\\VEN_1002&DEV_744C", "AMD Radeon RX 7900 XTX", "32.0.21001.9024"), gpu("PCI\\VEN_1002&DEV_67DF", "Radeon RX 580 Series", "32.0.21045.16010")];
        let r = crate::sources::amd::Release { version: "26.10.1".into(), store_versions: vec!["32.0.32015.7029".into(), "32.0.21045.16010".into()], url: "u".into(), size: Some(1), notes_url: "n".into() };
        let mut wu = offer("Advanced Micro Devices - Display - 32.0.21025.1000", "PCI\\VEN_1002&DEV_744C", None);
        wu.update_id = "amd".into();
        let list = collect(Some(&Feed { offers: vec![wu] }), &cards, &[], Some(&r), None);
        assert_eq!(list.len(), 2, "{list:#?}");
        let new = &list[0];
        assert_eq!((new.entry.source.as_str(), new.entry.status, new.entry.latest.as_deref()), ("amd", AppStatus::Update, Some("32.0.32015.7029")));
        assert_eq!(new.how, Some(How::Amd(r.clone())));
        // The RX 580 is on AMD's older branch and already has its newest driver.
        assert_eq!((list[1].entry.status, list[1].how.is_none()), (AppStatus::Current, true));
    }

    /// Asks this PC's Windows Update and NVIDIA (no install): `cargo test --lib live_driver_search -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_driver_search() {
        let t = std::time::Instant::now();
        let feed = fetch().await.expect("Windows Update search");
        println!("windows update: {} ms", t.elapsed().as_millis());
        let have = super::present();
        let products = crate::sources::nvidia::products().await.expect("NVIDIA products");
        let mut nv = vec![];
        for p in have.iter().filter(|p| crate::sources::nvidia::is_gpu(p)) {
            let id = crate::sources::nvidia::product_id(&p.desc, &products).unwrap_or_default();
            let rel = if id.is_empty() { None } else { crate::sources::nvidia::latest(&id).await.ok() };
            nv.push((p.matching_id.clone(), id, rel));
        }
        println!("{} installed drivers, {} offered", have.len(), feed.offers.len());
        for f in collect(Some(&feed), &have, &nv, None, None) {
            let e = f.entry;
            println!("{:<9} {:<36} {:<12} -> {:<14} {:?} [{}] {:?}", e.kind, e.name, e.version, e.latest.unwrap_or_default(), e.status, e.source, e.note);
        }
    }

    #[test]
    fn the_install_runs_elevated_and_a_restart_counts_as_done() {
        let list = collect(Some(&Feed { offers: vec![offer("Intel - Display - 31.0.101.5590", "PCI\\X", None)] }), &[], &[], None, None);
        let plan = plan(&list[0]).unwrap();
        let Step::Run { args, ok_codes, elevated, .. } = &plan.steps[0] else { panic!() };
        assert_eq!(args, &vec![INSTALL_FLAG.to_string(), "u1".to_string()]);
        assert!(ok_codes.contains(&EXIT_REBOOT) && *elevated);
        assert!(plan.downloads.is_empty() && plan.runs_installer());
    }
}
