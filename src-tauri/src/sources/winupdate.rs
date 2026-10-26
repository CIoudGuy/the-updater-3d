//! Windows' own updates, listed next to the drivers in their own group "Windows": what Windows
//! Update offers this PC other than drivers (security and cumulative updates, .NET, Microsoft
//! Defender, the servicing stack, optional and preview updates), and a "Windows Update" entry that
//! says when Windows waits for a restart, is paused or is managed by an organization. One installs
//! through the Windows Update Agent, by running this program again elevated
//! (`--install-windows-update <update id> <progress file>`), like a Windows Update driver.
//! Feature updates (a new Windows version) and updates that ask questions while they install are
//! only listed: Settings > Windows Update installs them. Automatic mode never installs any of
//! these: Windows installs them itself, on its own schedule.

use super::drivers::{Found, How};
use super::{Plan, Step, Verify};
use crate::model::{AppStatus, DriverEntry, JobKind};
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use serde::{Deserialize, Serialize};

/// The command line flag the elevated helper installs a Windows update with.
pub const INSTALL_FLAG: &str = "--install-windows-update";
/// Settings > Windows Update.
pub const SETTINGS_URL: &str = "ms-settings:windowsupdate";
/// The group the apps list shows these in.
pub const GROUP: &str = "Windows";
/// Ids of the entries: "driver:win:<update id>", and the "Windows Update" entry.
pub const ID_PREFIX: &str = "driver:win:";
pub const STATUS_ID: &str = "driver:win:status";

/// Windows Update's classification categories, by id (their names are translated, the ids not).
const CLASS_SECURITY: &str = "0fa1201d-4330-4fa8-8ae9-b877473b6441";
const CLASS_DEFINITION: &str = "e0789628-ce08-4437-be74-2495b842f43b";
const CLASS_UPGRADES: &str = "3689bdc8-b205-4af4-8d4a-a63924c5e9d5";
const CLASS_DRIVERS: &str = "ebfc1fc5-71a4-4f7b-9aca-3b9a503104a0";

/// One update Windows Update offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    /// Windows Update's id for this update
    pub update_id: String,
    /// "2026-10 Cumulative Update for Windows 11 Version 24H2 for x64-based Systems (KB5000001)"
    pub title: String,
    pub description: String,
    /// "5000001" (Windows Update gives them without "KB")
    pub kbs: Vec<String>,
    /// Ids of its classification categories, lowercase
    pub classes: Vec<String>,
    pub size: Option<u64>,
    /// When Microsoft released it, "2026.10.14"
    pub date: Option<String>,
    /// "More information" link (only a Microsoft address is kept)
    pub support_url: Option<String>,
    /// Optional: Windows does not install it by itself (Settings lists it under Optional updates)
    pub optional: bool,
    /// It may ask questions while it installs
    pub needs_input: bool,
    /// A driver (listed under Drivers instead)
    pub driver: bool,
}

/// What a Windows Update check found.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Feed {
    pub offers: Vec<Offer>,
}

/// How Windows Update is set up on this PC: paused in Settings, or held back by an organization's
/// policy. Read from the registry, never changed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Policy {
    /// Paused in Settings until this day ("2026-11-02")
    pub paused_until: Option<String>,
    /// An organization's update server (WSUS) decides what this PC gets
    pub managed: bool,
    /// An organization turned Windows Update off for this PC
    pub blocked: bool,
    /// Quality updates are held back by policy this many days
    pub deferred_days: u32,
}

/// This PC's Windows: build "26100.1234" and "24H2".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Os {
    pub build: String,
    pub display_version: String,
}

// ---------------------------------------------------------------- what is listed, and how

/// Is this entry one of Windows' own updates (or the "Windows Update" entry)?
pub fn is_windows(id: &str) -> bool {
    id.starts_with(ID_PREFIX)
}

/// A feature update: a new version of Windows. Only Settings installs those.
pub fn is_upgrade(o: &Offer) -> bool {
    static TITLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bfeature update to windows\b|\bupgrade to windows\b").unwrap());
    o.classes.iter().any(|c| c == CLASS_UPGRADES) || TITLE.is_match(o.title.trim())
}

/// Listed under Windows: everything Windows Update offers but drivers (which are listed under
/// Drivers). Installed and hidden updates are not asked for.
pub fn listed(o: &Offer) -> bool {
    !o.driver && !o.classes.iter().any(|c| c == CLASS_DRIVERS) && is_update_id(&o.update_id)
}

/// Installed here: not a feature update, and Windows says it asks nothing while it installs.
/// The others open Settings > Windows Update.
pub fn installable(o: &Offer) -> bool {
    listed(o) && !is_upgrade(o) && !o.needs_input
}

/// Its license terms are accepted for the user only when it installs without asking anything.
pub fn may_accept_eula(o: &Offer) -> bool {
    installable(o)
}

/// An update id is a GUID: anything else is not passed to a search.
pub fn is_update_id(id: &str) -> bool {
    id.len() == 36 && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// What kind of update it is, in words people know.
pub fn kind(o: &Offer) -> &'static str {
    let t = o.title.to_ascii_lowercase();
    if is_upgrade(o) {
        "Feature update"
    } else if t.contains("defender") || t.contains("security intelligence") || o.classes.iter().any(|c| c == CLASS_DEFINITION) {
        "Microsoft Defender update"
    } else if t.contains("servicing stack") {
        "Servicing stack update"
    } else if t.contains(".net") {
        ".NET update"
    } else if t.contains("preview") {
        "Preview update"
    } else if o.classes.iter().any(|c| c == CLASS_SECURITY) || t.contains("security update") {
        "Security update"
    } else if t.contains("cumulative update") {
        "Cumulative update"
    } else {
        "Update"
    }
}

/// Its kind, "Optional" in front for an optional one: "Optional .NET update".
pub fn kind_line(o: &Offer) -> String {
    let k = kind(o);
    if !o.optional || is_upgrade(o) || k == "Preview update" {
        return k.to_string();
    }
    // "Security update" reads "Optional security update"; names keep their capitals.
    let k = if k.starts_with("Microsoft") || k.starts_with('.') { k.to_string() } else { k[..1].to_ascii_lowercase() + &k[1..] };
    format!("Optional {k}")
}

/// "KB5000001", from Windows Update's list or else the title.
pub fn kb(o: &Offer) -> Option<String> {
    static KB: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bKB(\d{6,8})\b").unwrap());
    let n = o.kbs.iter().map(|k| k.trim().trim_start_matches("KB").trim_start_matches("kb").to_string()).find(|k| !k.is_empty() && k.chars().all(|c| c.is_ascii_digit()));
    n.or_else(|| KB.captures(&o.title).map(|c| c[1].to_string())).map(|n| format!("KB{n}"))
}

/// The title without its month and KB number: "Cumulative Update for Windows 11 Version 24H2 for
/// x64-based Systems" (the KB number is shown as its version, the month as its date).
pub fn short_title(title: &str) -> String {
    static MONTH: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d{4}-\d{2}\s+").unwrap());
    static KB: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*\(KB\d+\)").unwrap());
    let t = KB.replace_all(title.trim(), "");
    let t = MONTH.replace(&t, "");
    let t = t.trim();
    if t.is_empty() { title.trim().to_string() } else { t.to_string() }
}

/// "Windows 11, version 24H2" for build 26100 and "24H2".
pub fn os_name(os: &Os) -> String {
    let major = os.build.split('.').next().and_then(|b| b.parse::<u32>().ok()).unwrap_or(0);
    let name = if major >= 22000 { "Windows 11" } else { "Windows 10" };
    if os.display_version.trim().is_empty() { name.to_string() } else { format!("{name}, version {}", os.display_version.trim()) }
}

/// "2026-11-02" as "2 November 2026".
fn long_date(day: &str) -> String {
    chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d").map(|d| d.format("%-d %B %Y").to_string()).unwrap_or_else(|_| day.to_string())
}

/// The size to show from Windows Update's largest download size. A cumulative update counts
/// every express variant in it (tens of GB) while it downloads only the part this PC needs:
/// that number is no size at all, so none is shown. Even a feature update is smaller.
pub fn download_size(max: Option<u64>) -> Option<u64> {
    max.filter(|&n| n > 0 && n <= 8_000_000_000)
}

/// One short line on how Windows Update is held back here, or None when it is not.
pub fn policy_line(p: &Policy, today: chrono::NaiveDate) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if p.blocked {
        parts.push("Your organization turned off Windows Update on this PC.".into());
    } else if p.managed {
        parts.push("Your organization manages Windows Update on this PC.".into());
    }
    let paused = p.paused_until.as_deref().filter(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok_and(|d| d >= today));
    if let Some(d) = paused {
        parts.push(format!("Windows Update is paused until {}.", long_date(d)));
    }
    if p.deferred_days > 0 && !p.blocked {
        parts.push(format!("Your organization holds updates back for {} days.", p.deferred_days));
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// One short line for a failed search, from its error code.
pub fn search_error_line(code: u32) -> String {
    match code {
        // WU_E_WU_DISABLED, ERROR_SERVICE_DISABLED
        0x8024_002E => "Windows Update is turned off on this PC.".into(),
        0x8007_0422 => "The Windows Update service is turned off.".into(),
        // No connection, or Microsoft's servers did not answer.
        0x8024_402C | 0x8024_4022 | 0x8024_401C | 0x8024_4010 | 0x8024_0438 | 0x8007_2EE2 | 0x8007_2EE7 | 0x8007_2EFD | 0x8007_2EFE | 0x8007_2F8F => "Could not reach Windows Update. Check the connection.".into(),
        0 => "Could not check Windows Update.".into(),
        c => format!("Could not check Windows Update (error 0x{c:08X})."),
    }
}

/// Everything one Windows Update check knows.
pub struct Inputs<'a> {
    /// What Windows Update offers, or the error code of the search
    pub answer: Result<&'a Feed, u32>,
    pub policy: &'a Policy,
    /// Windows waits for a restart to finish installing updates
    pub restart: bool,
    pub os: &'a Os,
    pub today: chrono::NaiveDate,
}

fn base(id: String, name: String, kind: String, version: String) -> DriverEntry {
    DriverEntry { id, name, kind, maker: "Microsoft".into(), source: "windows".into(), version, latest: None, status: AppStatus::Current, size: None, date: None, notes_url: None, vendor_url: None, note: None, notes: vec![], pre_channel: None, studio: None, folder: None }
}

/// Everything listed under Windows: the "Windows Update" entry first, then one entry per update.
pub fn collect(inp: &Inputs) -> Vec<Found> {
    let offers: Vec<&Offer> = match inp.answer {
        Ok(f) => f.offers.iter().filter(|o| listed(o)).collect(),
        Err(_) => vec![],
    };
    let mut out = vec![status_entry(inp, offers.len())];
    for o in offers {
        let id = format!("{ID_PREFIX}{}", o.update_id.to_ascii_lowercase());
        if out.iter().any(|f| f.entry.id == id) {
            continue;
        }
        out.push(offer_entry(o, id));
    }
    out
}

/// The "Windows Update" entry: this PC's Windows build, and one line on what holds updates back.
fn status_entry(inp: &Inputs, offered: usize) -> Found {
    let version = if inp.os.build.is_empty() { "Windows".to_string() } else { inp.os.build.clone() };
    let mut e = base(STATUS_ID.into(), "Windows Update".into(), os_name(inp.os), version.clone());
    e.latest = Some(version);
    e.vendor_url = Some(SETTINGS_URL.into());
    let mut lines: Vec<String> = Vec::new();
    if inp.restart {
        lines.push("Restart your PC to finish installing updates.".into());
    }
    match inp.answer {
        Err(code) => {
            e.status = AppStatus::Unknown;
            lines.push(search_error_line(code));
        }
        Ok(_) => {}
    }
    lines.extend(policy_line(inp.policy, inp.today));
    if lines.is_empty() {
        lines.push(match offered {
            0 => "Windows Update has nothing new for this PC.".to_string(),
            1 => "1 update from Windows Update is listed here.".to_string(),
            n => format!("{n} updates from Windows Update are listed here."),
        });
    }
    e.note = Some(lines.join(" "));
    Found { entry: e, how: None }
}

fn offer_entry(o: &Offer, id: String) -> Found {
    let version = kb(o).unwrap_or_else(|| "Update".into());
    let kind = kind_line(o);
    let mut e = base(id, short_title(&o.title), kind, version.clone());
    e.latest = Some(version);
    e.status = AppStatus::Update;
    e.size = o.size;
    e.date = o.date.clone();
    e.notes_url = super::drivers::safe_link(o.support_url.clone()).filter(|u| super::drivers::host_is(u, &["microsoft.com"]));
    e.notes = [o.title.trim(), o.description.trim()].iter().filter(|s| !s.is_empty()).map(|s| s.to_string()).collect();
    let installs = installable(o);
    e.note = Some(if is_upgrade(o) {
        "A new version of Windows. Install it from Windows Update in Settings.".into()
    } else if !installs {
        "It asks questions while it installs. Install it from Windows Update in Settings.".into()
    } else if o.optional {
        "Optional. Windows does not install it by itself. Some updates need a restart.".into()
    } else {
        "From Windows Update. Some updates need a restart.".into()
    });
    if !installs {
        e.vendor_url = Some(SETTINGS_URL.into());
    }
    let how = installs.then(|| How::Windows { update_id: o.update_id.clone() });
    Found { entry: e, how }
}

/// The page a Windows entry opens: Settings > Windows Update. Any other link: a maker's (see
/// `drivers::safe_link`).
pub fn page_link(url: Option<String>) -> Option<String> {
    match url {
        Some(u) if u == SETTINGS_URL => Some(u),
        u => super::drivers::safe_link(u),
    }
}

/// What automatic mode may install: never one of Windows' own updates, which Windows installs
/// itself on its own schedule.
pub fn auto_installs(id: &str) -> bool {
    !is_windows(id)
}

// ---------------------------------------------------------------- progress

/// The two parts of an install, as the progress file names them.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Phase {
    Download,
    Install,
}

/// One line of the progress file the elevated install writes: "download 45".
pub fn progress_line(phase: Phase, percent: i32) -> String {
    format!("{} {}", if phase == Phase::Download { "download" } else { "install" }, percent.clamp(0, 100))
}

/// Reads the progress file while the install runs (see `install_detail::watch`).
#[derive(Default)]
pub struct ProgressLog {
    now: Option<(Phase, i32)>,
}

impl ProgressLog {
    pub fn feed(&mut self, line: &str) {
        let mut p = line.split_whitespace();
        let phase = match p.next() {
            Some("download") => Phase::Download,
            Some("install") => Phase::Install,
            _ => return,
        };
        if let Some(n) = p.next().and_then(|n| n.parse::<i32>().ok()) {
            self.now = Some((phase, n.clamp(0, 100)));
        }
    }
    pub fn text(&self) -> Option<String> {
        self.now.map(|(p, n)| match p {
            Phase::Download => format!("Downloading from Windows Update, {n}%"),
            Phase::Install => format!("Installing, {n}%"),
        })
    }
    /// The download is the first half, the install the second.
    pub fn fraction(&self) -> Option<f64> {
        self.now.map(|(p, n)| (if p == Phase::Download { 0.0 } else { 0.5 }) + 0.5 * n as f64 / 100.0)
    }
}

// ---------------------------------------------------------------- installing

/// The job that installs one Windows update.
pub fn plan_install(e: &DriverEntry, update_id: &str) -> Result<Plan> {
    if !is_update_id(update_id) {
        return Err(anyhow!("Not a Windows Update id: {update_id}"));
    }
    let me = std::env::current_exe()?;
    let log = crate::paths::logs().join(format!("windows-update-{}.log", &update_id[..8]));
    Ok(Plan {
        app_id: e.id.clone(),
        kind: JobKind::Update,
        name: e.name.clone(),
        to_version: e.latest.clone().unwrap_or_default(),
        downloads: vec![],
        steps: vec![Step::Run {
            exe: me,
            args: vec![INSTALL_FLAG.into(), update_id.into(), log.to_string_lossy().into()],
            cwd: None,
            ok_codes: vec![0, super::drivers::EXIT_REBOOT],
            elevated: true,
            label: format!("Installing {} through Windows Update", e.latest.as_deref().unwrap_or("the update")),
            log: Some(log),
        }],
        block_dirs: vec![],
        block_names: vec![],
        verify: Verify::None,
        fallback: None,
        cleanup: vec![],
    })
}

/// `--install-windows-update <update id> [<progress file>]`: download and install one update
/// through Windows Update. Exit code 0 when done, `EXIT_REBOOT` when Windows needs a restart, 1
/// on failure.
pub fn install_main(args: &[String]) -> i32 {
    let mut rest = args.iter().skip_while(|a| *a != INSTALL_FLAG).skip(1);
    let Some(id) = rest.next() else {
        eprintln!("No update id");
        return 1;
    };
    let log = rest.next().map(std::path::PathBuf::from);
    match install(id, log.as_deref()) {
        Ok(true) => super::drivers::EXIT_REBOOT as i32,
        Ok(false) => 0,
        Err(e) => {
            eprintln!("{e:#}");
            1
        }
    }
}

// ---------------------------------------------------------------- Windows Update Agent

/// Ask Windows Update what it offers this PC, how it is set up and whether it waits for a
/// restart. Slow (seconds to a minute): it asks Microsoft's servers. Runs on a blocking thread.
pub async fn check() -> Vec<Found> {
    tauri::async_runtime::spawn_blocking(|| {
        let answer = search().map_err(|e| e.code().0 as u32);
        if let Err(c) = &answer {
            log::warn!("windows update: search failed, 0x{c:08x}");
        }
        let (policy, os) = (policy(), os());
        collect(&Inputs { answer: answer.as_ref().map_err(|c| *c), policy: &policy, restart: super::drivers::restart_pending(), os: &os, today: chrono::Local::now().date_naive() })
    })
    .await
    .unwrap_or_default()
}

/// The updates Windows Update offers that are not installed or hidden, drivers left out.
pub fn search() -> windows::core::Result<Feed> {
    use windows::core::BSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::UpdateAgent::*;
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let session: IUpdateSession = CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
        let _ = session.SetClientApplicationID(&BSTR::from("The Updater 3D"));
        let found = session.CreateUpdateSearcher()?.Search(&BSTR::from("IsInstalled=0 and IsHidden=0 and Type='Software'"))?;
        let list = found.Updates()?;
        let mut offers = Vec::new();
        for k in 0..list.Count()? {
            offers.push(offer_of(&list.get_Item(k)?));
        }
        Ok(Feed { offers })
    }
}

/// What Windows Update says about one update.
fn offer_of(u: &windows::Win32::System::UpdateAgent::IUpdate) -> Offer {
    use windows::core::{Interface, BSTR};
    use windows::Win32::System::UpdateAgent::*;
    let s = |r: windows::core::Result<BSTR>| r.map(|b| b.to_string()).unwrap_or_default();
    unsafe {
        let kbs = u.KBArticleIDs().ok().map(|c| (0..c.Count().unwrap_or(0)).filter_map(|i| c.get_Item(i).ok()).map(|b| b.to_string()).collect()).unwrap_or_default();
        let classes = u
            .Categories()
            .ok()
            .map(|c| (0..c.Count().unwrap_or(0)).filter_map(|i| c.get_Item(i).ok()).filter(|c| s(c.Type()) == "UpdateClassification").map(|c| s(c.CategoryID()).to_ascii_lowercase()).collect())
            .unwrap_or_default();
        let support = s(u.SupportUrl());
        Offer {
            update_id: s(u.Identity().and_then(|i| i.UpdateID())),
            title: s(u.Title()),
            description: s(u.Description()),
            kbs,
            classes,
            size: download_size(u.MaxDownloadSize().ok().map(|m| m.Anonymous2.Lo64)),
            date: u.LastDeploymentChangeTime().ok().and_then(super::drivers::ole_date),
            support_url: (!support.is_empty()).then_some(support),
            optional: u.cast::<IUpdate3>().ok().and_then(|u| u.BrowseOnly().ok()).is_some_and(|b| b.as_bool()),
            needs_input: u.InstallationBehavior().ok().and_then(|b| b.CanRequestUserInput().ok()).is_some_and(|b| b.as_bool()),
            driver: u.Type().is_ok_and(|t| t == utDriver),
        }
    }
}

/// How Windows Update is set up here, from the registry (read only).
pub fn policy() -> Policy {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let hklm = winreg::RegKey::predef(HKEY_LOCAL_MACHINE);
    let mut p = Policy::default();
    if let Ok(k) = hklm.open_subkey_with_flags("SOFTWARE\\Microsoft\\WindowsUpdate\\UX\\Settings", KEY_READ) {
        // "2026-11-02T07:00:00Z": the latest of the pauses set in Settings.
        p.paused_until = ["PauseUpdatesExpiryTime", "PauseQualityUpdatesEndTime"].iter().filter_map(|n| k.get_value::<String, _>(n).ok()).filter_map(|v| v.get(..10).map(str::to_string)).max();
    }
    if let Ok(k) = hklm.open_subkey_with_flags("SOFTWARE\\Policies\\Microsoft\\Windows\\WindowsUpdate", KEY_READ) {
        let dw = |n: &str| k.get_value::<u32, _>(n).unwrap_or(0);
        p.blocked = dw("DisableWindowsUpdateAccess") == 1;
        if dw("DeferQualityUpdates") == 1 {
            p.deferred_days = dw("DeferQualityUpdatesPeriodInDays");
        }
        let server: String = k.get_value("WUServer").unwrap_or_default();
        let use_server = k.open_subkey_with_flags("AU", KEY_READ).ok().and_then(|a| a.get_value::<u32, _>("UseWUServer").ok()) == Some(1);
        p.managed = use_server && !server.trim().is_empty();
    }
    p
}

/// This PC's Windows build and version name, from the registry.
pub fn os() -> Os {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let Ok(k) = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", KEY_READ) else { return Os::default() };
    let build: String = k.get_value("CurrentBuildNumber").unwrap_or_default();
    let ubr = k.get_value::<u32, _>("UBR").ok();
    Os { build: match ubr { Some(u) if !build.is_empty() => format!("{build}.{u}"), _ => build }, display_version: k.get_value("DisplayVersion").unwrap_or_default() }
}

/// Windows Update's asynchronous download and install want callbacks to call; the progress is
/// read from the job instead, so these do nothing.
#[windows_core::implement(
    windows::Win32::System::UpdateAgent::IDownloadProgressChangedCallback,
    windows::Win32::System::UpdateAgent::IDownloadCompletedCallback,
    windows::Win32::System::UpdateAgent::IInstallationProgressChangedCallback,
    windows::Win32::System::UpdateAgent::IInstallationCompletedCallback
)]
struct Quiet;

mod quiet {
    use super::Quiet_Impl;
    use windows::Win32::System::UpdateAgent::*;
    use windows_core::{Ref, Result};
    impl IDownloadProgressChangedCallback_Impl for Quiet_Impl {
        fn Invoke(&self, _: Ref<IDownloadJob>, _: Ref<IDownloadProgressChangedCallbackArgs>) -> Result<()> {
            Ok(())
        }
    }
    impl IDownloadCompletedCallback_Impl for Quiet_Impl {
        fn Invoke(&self, _: Ref<IDownloadJob>, _: Ref<IDownloadCompletedCallbackArgs>) -> Result<()> {
            Ok(())
        }
    }
    impl IInstallationProgressChangedCallback_Impl for Quiet_Impl {
        fn Invoke(&self, _: Ref<IInstallationJob>, _: Ref<IInstallationProgressChangedCallbackArgs>) -> Result<()> {
            Ok(())
        }
    }
    impl IInstallationCompletedCallback_Impl for Quiet_Impl {
        fn Invoke(&self, _: Ref<IInstallationJob>, _: Ref<IInstallationCompletedCallbackArgs>) -> Result<()> {
            Ok(())
        }
    }
}

/// Appends progress lines to the file the app reads while the install runs.
struct Progress {
    file: Option<std::fs::File>,
    last: Option<(Phase, i32)>,
}

impl Progress {
    fn new(path: Option<&std::path::Path>) -> Self {
        Progress { file: path.and_then(|p| std::fs::File::create(p).ok()), last: None }
    }
    fn say(&mut self, phase: Phase, percent: i32) {
        use std::io::Write;
        if self.last == Some((phase, percent)) {
            return;
        }
        self.last = Some((phase, percent));
        if let Some(f) = self.file.as_mut() {
            let _ = writeln!(f, "{}", progress_line(phase, percent));
            let _ = f.flush();
        }
    }
}

/// Returns whether a restart is needed.
fn install(update_id: &str, log: Option<&std::path::Path>) -> Result<bool> {
    use windows::core::{IUnknown, Interface, BSTR};
    use windows::Win32::Foundation::{VARIANT_FALSE, VARIANT_TRUE};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::UpdateAgent::*;
    use windows::Win32::System::Variant::VARIANT;
    if !is_update_id(update_id) {
        return Err(anyhow!("Not a Windows Update id: {update_id}"));
    }
    let mut progress = Progress::new(log);
    let poll = std::time::Duration::from_millis(500);
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let session: IUpdateSession = CoCreateInstance(&UpdateSession, None, CLSCTX_INPROC_SERVER)?;
        let _ = session.SetClientApplicationID(&BSTR::from("The Updater 3D"));
        let found = session.CreateUpdateSearcher()?.Search(&BSTR::from(format!("UpdateID='{update_id}'")))?.Updates()?;
        if found.Count()? == 0 {
            return Err(anyhow!("Windows Update no longer offers this update. Check for updates again."));
        }
        let update = found.get_Item(0)?;
        if update.IsInstalled()?.as_bool() {
            println!("Already installed");
            return Ok(false);
        }
        let o = offer_of(&update);
        if is_upgrade(&o) {
            return Err(anyhow!("Feature updates install from Windows Update in Settings."));
        }
        if !installable(&o) {
            return Err(anyhow!("This update asks questions while it installs. Install it from Windows Update in Settings."));
        }
        if !update.EulaAccepted()?.as_bool() && may_accept_eula(&o) {
            update.AcceptEula()?;
        }
        let todo: IUpdateCollection = CoCreateInstance(&UpdateCollection, None, CLSCTX_INPROC_SERVER)?;
        todo.Add(&update)?;
        let quiet: IUnknown = Quiet.into();
        let state = VARIANT::default();

        if !update.IsDownloaded()?.as_bool() {
            println!("Downloading");
            progress.say(Phase::Download, 0);
            let dl = session.CreateUpdateDownloader()?;
            dl.SetUpdates(&todo)?;
            // With progress when Windows Update starts it that way, else in one call.
            let r = match dl.BeginDownload(&quiet, &quiet, &state) {
                Ok(job) => {
                    while !job.IsCompleted()?.as_bool() {
                        if let Ok(n) = job.GetProgress().and_then(|p| p.PercentComplete()) {
                            progress.say(Phase::Download, n);
                        }
                        std::thread::sleep(poll);
                    }
                    dl.EndDownload(&job)?
                }
                Err(_) => dl.Download()?,
            };
            let code = r.ResultCode()?;
            if code != orcSucceeded && code != orcSucceededWithErrors {
                return Err(anyhow!("Windows Update could not download the update (result {}, 0x{:08x})", code.0, r.HResult()? as u32));
            }
            progress.say(Phase::Download, 100);
        }
        println!("Installing");
        progress.say(Phase::Install, 0);
        let ins = session.CreateUpdateInstaller()?;
        ins.SetUpdates(&todo)?;
        let _ = ins.SetAllowSourcePrompts(VARIANT_FALSE);
        // Nothing may ask the user anything: an update that would, fails instead.
        if let Ok(i2) = ins.cast::<IUpdateInstaller2>() {
            let _ = i2.SetForceQuiet(VARIANT_TRUE);
        }
        let r = match ins.BeginInstall(&quiet, &quiet, &state) {
            Ok(job) => {
                while !job.IsCompleted()?.as_bool() {
                    if let Ok(n) = job.GetProgress().and_then(|p| p.PercentComplete()) {
                        progress.say(Phase::Install, n);
                    }
                    std::thread::sleep(poll);
                }
                ins.EndInstall(&job)?
            }
            Err(_) => ins.Install()?,
        };
        let code = r.ResultCode()?;
        if code != orcSucceeded && code != orcSucceededWithErrors {
            return Err(anyhow!("Windows Update could not install the update (result {}, 0x{:08x})", code.0, r.HResult()? as u32));
        }
        progress.say(Phase::Install, 100);
        Ok(r.RebootRequired()?.as_bool())
    }
}

#[cfg(test)]
mod tests;
