//! Intel drivers (graphics, Wi-Fi, Bluetooth, network adapters), from the data Intel Driver &
//! Support Assistant reads (dsadata.intel.com): each driver package with the device ids it is
//! for, its download on downloadmirror.intel.com and its exact size. The package must be signed
//! by Intel. Intel marks most of its packages as not for silent install, so its own installer
//! opens.

use super::drivers::Present;
use super::{Download, Plan, Step, Verify};
use crate::model::JobKind;
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use std::io::Read;
use std::time::Duration;

const DATA_URL: &str = "https://dsadata.intel.com/data/en";
pub const SIGNER: &str = "Intel Corporation";

/// One package from Intel's data (only the fields used here).
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Config {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub display_release_date: Option<String>,
    #[serde(default)]
    pub install_options: Option<String>,
    #[serde(default)]
    pub allow_silent_installation: bool,
    #[serde(default)]
    pub is_beta: bool,
    #[serde(default)]
    pub url: Option<String>,
    /// What the package is and what changed (HTML)
    #[serde(default)]
    pub details: Option<String>,
    #[serde(default)]
    pub extended_details: Option<String>,
    #[serde(default)]
    pub files: Vec<File>,
    #[serde(default)]
    pub components: Vec<Component>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct File {
    pub url: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub operating_systems: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Component {
    #[serde(default)]
    pub category: Option<String>,
    /// null on some old packages: those are skipped
    #[serde(default)]
    pub version: Option<String>,
    /// Device ids it is for: "VEN_8086&DEV_5690", "VEN_8086&DEV_5690&SUBSYS_30268086"
    #[serde(default)]
    pub detection_values: Vec<String>,
}

/// The newest Intel driver for one device.
#[derive(Clone, Debug, PartialEq, serde::Serialize, Deserialize)]
pub struct Release {
    /// Windows driver version: "32.0.101.9150"
    pub version: String,
    /// "Intel Arc Graphics - Windows"
    pub name: String,
    /// "2026.10.02"
    pub date: Option<String>,
    pub url: String,
    pub size: Option<u64>,
    pub notes_url: Option<String>,
    pub args: Vec<String>,
    pub silent: bool,
    #[serde(default)]
    pub notes: Vec<String>,
}

pub fn is_gpu(p: &Present) -> bool {
    p.class.eq_ignore_ascii_case("Display") && p.matching_id.to_ascii_lowercase().starts_with("pci\\ven_8086")
}

/// The package list from Intel's data, a zip with software-configurations.json in it.
pub fn parse_data(zip_bytes: &[u8]) -> Result<Vec<Config>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_bytes)).context("Intel's data is not a zip")?;
    let mut f = z.by_name("software-configurations.json").context("Intel's data has no package list")?;
    let mut s = String::new();
    f.read_to_string(&mut s)?;
    Ok(serde_json::from_str(&s)?)
}

/// Does a device id from Intel's list name this device? Part by part ("VEN_8086", "DEV_5690"),
/// whole parts only (DEV_569 is not DEV_5690), with "*" for any text inside a part
/// ("SUBSYS_*8086"). PCI and USB devices ("VID_8087&PID_0029").
fn fits(matching_id: &str, value: &str) -> bool {
    let id = matching_id.to_ascii_uppercase();
    let id = match id.split_once('\\') {
        Some(("PCI" | "USB", rest)) => rest,
        Some(_) => return false,
        None => id.as_str(),
    };
    let v = value.to_ascii_uppercase();
    if id.is_empty() || v.contains('{') || v.contains('\\') {
        return false;
    }
    let ids: Vec<&str> = id.split('&').collect();
    let want: Vec<&str> = v.split('&').collect();
    want.len() <= ids.len() && want.iter().zip(&ids).all(|(w, i)| glob(w, i))
}

/// `pattern` with "*" for any text, against `text`.
fn glob(pattern: &str, text: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == text,
        Some((head, rest)) => {
            let Some(after) = text.strip_prefix(head) else { return false };
            if rest.is_empty() {
                return true;
            }
            (0..=after.len()).filter(|&k| after.is_char_boundary(k)).any(|k| glob(rest, &after[k..]))
        }
    }
}

/// "Intel® Arc™ Graphics - Windows*" as "Intel Arc Graphics - Windows"
fn clean(name: &str) -> String {
    name.replace(['®', '™', '*'], "").replace("\u{fffd}", "").split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The newest non-beta package for this device that has a 64-bit program for this Windows.
/// `category`: only packages of that kind ("Graphics"); None: any kind.
pub fn for_device(p: &Present, configs: &[Config], windows11: bool, category: Option<&str>) -> Option<Release> {
    let os = if windows11 { "windows-11" } else { "windows-10" };
    configs
        .iter()
        .filter(|c| !c.is_beta)
        .filter_map(|c| {
            let comp = c.components.iter().find(|k| category.is_none_or(|want| k.category.as_deref().is_some_and(|c| c.eq_ignore_ascii_case(want))) && k.version.is_some() && k.detection_values.iter().any(|v| fits(&p.matching_id, v)))?;
            let file = c.files.iter().find(|f| f.url.ends_with(".exe") && f.operating_systems.iter().any(|o| o.starts_with(os) && o.ends_with("-64")))?;
            if !super::drivers::host_is(&file.url, &["downloadmirror.intel.com"]) {
                return None;
            }
            let html = c.extended_details.as_deref().filter(|d| !d.trim().is_empty()).or(c.details.as_deref()).unwrap_or_default();
            Some(Release {
                version: comp.version.clone()?,
                name: clean(c.name.as_deref().unwrap_or("Intel driver")),
                date: c.display_release_date.as_deref().and_then(|d| d.get(..10)).map(|d| d.replace('-', ".")),
                url: file.url.clone(),
                size: file.size,
                notes_url: super::drivers::safe_link(c.url.clone()),
                args: c.install_options.as_deref().map(|o| o.split_whitespace().map(String::from).collect()).unwrap_or_default(),
                silent: c.allow_silent_installation,
                notes: super::drivers::html_notes(html),
            })
        })
        .max_by(|a, b| super::tools::cmp_ver(&a.version, &b.version))
}

/// `for_device` for graphics packages only.
pub fn for_card(p: &Present, configs: &[Config], windows11: bool) -> Option<Release> {
    for_device(p, configs, windows11, Some("Graphics"))
}

/// Intel's package list, kept for six hours.
pub async fn fetch() -> Result<Vec<Config>> {
    let f = crate::http::get_cached(DATA_URL, &[], Duration::from_secs(6 * 3600)).await?;
    let list = tokio::task::spawn_blocking(move || parse_data(&f.body)).await.map_err(|e| anyhow!("{e}"))??;
    if list.is_empty() {
        return Err(anyhow!("Intel's package list is empty"));
    }
    Ok(list)
}

/// Download Intel's package and run its installer. Intel does not allow a silent install of its
/// graphics drivers, so its installer opens and the user clicks through it.
pub fn plan_install(app_id: &str, name: &str, r: &Release) -> Result<Plan> {
    let file = r.url.rsplit('/').next().filter(|f| f.ends_with(".exe")).ok_or_else(|| anyhow!("Intel's download is not a program"))?;
    let dest = crate::paths::downloads().join("intel").join(file);
    let dl = Download { url: r.url.clone(), dest: dest.clone(), size: r.size, sha256: None, md5: None, signer: Some(SIGNER.into()), sha512: None, headers: vec![] };
    super::require_checked(&dl, &format!("{} {}", r.name, r.version))?;
    let mut args = r.args.clone();
    if r.silent {
        args.push("-s".into());
    }
    Ok(Plan {
        app_id: app_id.into(),
        kind: JobKind::Update,
        name: name.into(),
        to_version: r.version.clone(),
        downloads: vec![dl],
        steps: vec![Step::Run {
            exe: dest.clone(),
            args,
            cwd: None,
            ok_codes: vec![0],
            elevated: true,
            label: if r.silent { format!("Installing {} {}", r.name, r.version) } else { format!("{}: finish Intel's installer to install {}", super::WAITING_FOR_YOU, r.version) },
            log: None,
        }],
        block_dirs: vec![],
        block_names: vec![],
        verify: Verify::None,
        fallback: None,
        cleanup: vec![dest],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"[
      {"Id":930514,"Version":"32.0.101.9150 WHQL Certified","Name":"Intel® Arc™ Graphics - Windows*","DisplayReleaseDate":"2026-10-02T00:00:00Z","InstallOptions":"--nouninstall","AllowSilentInstallation":false,"IsBeta":false,
       "Url":"https://www.intel.com/content/www/us/en/download/785597/930514/intel-arc-graphics-windows.html",
       "Files":[{"Url":"https://downloadmirror.intel.com/930514/gfx_win_101.9150.exe","Hash":"eea4","Size":904501568,"OperatingSystems":["windows-10-22h2-64","windows-11-24h2-64"]}],
       "Components":[{"Category":"Graphics","Version":"32.0.101.9150","DetectionValues":["VEN_8086&DEV_5690","VEN_8086&DEV_56A0"]}]},
      {"Id":929468,"Version":"32.0.101.7092","Name":"Intel® 11th to 14th Gen Processor Graphics - Windows*","DisplayReleaseDate":"2026-09-25T00:00:00Z","IsBeta":false,
       "Files":[{"Url":"https://downloadmirror.intel.com/929468/gfx_win_101.7092.exe","Size":700000000,"OperatingSystems":["windows-11-24h2-64"]}],
       "Components":[{"Category":"Graphics","Version":"32.0.101.7092","DetectionValues":["VEN_8086&DEV_9A49"]}]},
      {"Id":1,"Version":"99.0","Name":"Beta","DisplayReleaseDate":"2026-10-20T00:00:00Z","IsBeta":true,
       "Files":[{"Url":"https://downloadmirror.intel.com/1/b.exe","OperatingSystems":["windows-11-24h2-64"]}],
       "Components":[{"Category":"Graphics","Version":"99.0.0.1","DetectionValues":["VEN_8086&DEV_56A0"]}]},
      {"Id":2,"Version":"24.70.0","Name":"Wi-Fi","DisplayReleaseDate":"2026-09-08T00:00:00Z",
       "Files":[{"Url":"https://downloadmirror.intel.com/2/w.exe","OperatingSystems":["windows-11-24h2-64"]}],
       "Components":[{"Category":"Wireless","Version":"24.70.0","DetectionValues":["VEN_8086&DEV_56A0"]}]},
      {"Id":3,"Version":null,"Name":null,"DisplayReleaseDate":null,"Files":[],"Components":[{"Category":"Graphics","Version":null,"DetectionValues":["VEN_8086&DEV_56A0"]}]}
    ]"#;

    fn card(id: &str) -> Present {
        Present { matching_id: id.into(), class: "Display".into(), version: "32.0.101.6987".into(), desc: "Intel(R) Arc(TM) A770 Graphics".into(), ..Default::default() }
    }

    #[test]
    fn a_card_gets_the_newest_stable_graphics_package_listing_its_device() {
        let list: Vec<Config> = serde_json::from_str(LIST).unwrap();
        let r = for_card(&card("PCI\\VEN_8086&DEV_56A0&SUBSYS_10208086"), &list, true).unwrap();
        assert_eq!((r.version.as_str(), r.name.as_str(), r.date.as_deref()), ("32.0.101.9150", "Intel Arc Graphics - Windows", Some("2026.10.02")));
        assert_eq!((r.size, r.args.clone(), r.silent), (Some(904501568), vec!["--nouninstall".to_string()], false));
        let igpu = for_card(&card("PCI\\VEN_8086&DEV_9A49"), &list, true).unwrap();
        assert_eq!(igpu.version, "32.0.101.7092");
        // Windows 10 has no file for the 11th-14th Gen package here.
        assert!(for_card(&card("PCI\\VEN_8086&DEV_9A49"), &list, false).is_none());
        assert!(for_card(&card("PCI\\VEN_8086&DEV_569"), &list, true).is_none(), "DEV_569 is not DEV_5690");
    }

    #[test]
    fn wifi_and_bluetooth_get_intels_own_driver_too() {
        let list: Vec<Config> = serde_json::from_str(r#"[
          {"Id":927576,"Version":"24.70.0","Name":"Intel® Wireless Bluetooth® Drivers for Windows® 10 and Windows 11*","DisplayReleaseDate":"2026-09-08T00:00:00Z","IsBeta":false,"AllowSilentInstallation":false,
           "Url":"https://www.intel.com/content/www/us/en/download/18649/intel-wireless-bluetooth-drivers.html","ExtendedDetails":"<p><strong>What's new</strong></p><ul><li>Fixed audio drops with some headsets</li></ul>",
           "Files":[{"Url":"https://downloadmirror.intel.com/927576/BT-24.70.0-64UWD-Win10-Win11.exe","Size":12000000,"OperatingSystems":["windows-11-24h2-64"]}],
           "Components":[{"Category":"Bluetooth","Version":"24.70.0.4","DetectionValues":["VID_8087&PID_0029","VEN_8086&DEV_A876&SUBSYS_*8086"]}]},
          {"Id":923981,"Version":"31.2.2","Name":"Network","DisplayReleaseDate":"2026-09-01T00:00:00Z","IsBeta":false,
           "Files":[{"Url":"https://downloadmirror.intel.com/923981/Wired_driver_31.2.2_x64.zip","OperatingSystems":["windows-11-24h2-64"]}],
           "Components":[{"Category":"LAN","Version":"12.18.9.10","DetectionValues":["VEN_8086&DEV_0D4C"]}]}
        ]"#).unwrap();
        let bt = Present { matching_id: "USB\\VID_8087&PID_0029&REV_0001".into(), class: "Bluetooth".into(), version: "23.60.0.1".into(), ..Default::default() };
        let r = for_device(&bt, &list, true, None).unwrap();
        assert_eq!(r.version, "24.70.0.4");
        assert!(r.notes.iter().any(|n| n.contains("Fixed audio drops")), "{:?}", r.notes);
        assert_eq!(r.notes_url.as_deref(), Some("https://www.intel.com/content/www/us/en/download/18649/intel-wireless-bluetooth-drivers.html"));
        // "SUBSYS_*8086": any subsystem made by Intel.
        let pci = Present { matching_id: "PCI\\VEN_8086&DEV_A876&SUBSYS_00948086".into(), ..bt.clone() };
        assert!(for_device(&pci, &list, true, None).is_some());
        let other = Present { matching_id: "PCI\\VEN_8086&DEV_A876&SUBSYS_00941A56".into(), ..bt.clone() };
        assert!(for_device(&other, &list, true, None).is_none());
        // Only programs are run, not a zip.
        let lan = Present { matching_id: "PCI\\VEN_8086&DEV_0D4C".into(), ..bt.clone() };
        assert!(for_device(&lan, &list, true, None).is_none());
        // Only graphics when asked for graphics.
        assert!(for_device(&bt, &list, true, Some("Graphics")).is_none());
        assert!(glob("SUBSYS_*8086", "SUBSYS_00948086") && !glob("SUBSYS_*8086", "SUBSYS_0094") && glob("A*", "A"));
    }

    #[test]
    fn the_install_is_signed_by_intel_and_opens_intels_installer() {
        let list: Vec<Config> = serde_json::from_str(LIST).unwrap();
        let r = for_card(&card("PCI\\VEN_8086&DEV_5690"), &list, true).unwrap();
        let plan = plan_install("driver:intel:x", "Intel Arc A770", &r).unwrap();
        assert_eq!((plan.downloads[0].signer.as_deref(), plan.downloads[0].size), (Some(SIGNER), Some(904501568)));
        let Step::Run { args, elevated, .. } = &plan.steps[0] else { panic!() };
        assert!(*elevated && args == &vec!["--nouninstall".to_string()], "not silent: Intel does not allow it");
        assert!(plan.steps[0].label().starts_with(crate::sources::WAITING_FOR_YOU));
    }

    /// Reads Intel's live data: `cargo test --lib live_intel -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_intel() {
        let list = fetch().await.expect("Intel data");
        let arc = for_card(&card("PCI\\VEN_8086&DEV_56A0"), &list, true).expect("A770");
        println!("{} packages; A770: {arc:#?}", list.len());
    }
}
