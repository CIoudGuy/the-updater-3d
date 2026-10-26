//! The AMD chipset drivers (AMD Chipset Software: GPIO, PSP, SMBus, I2C, power plans and more for
//! Ryzen PCs), from AMD's own chipset download page. The installed version is the one AMD's
//! installer records (and Gigabyte Control Center reads). AMD's download server only serves files
//! to visitors coming from its pages (Referer). AMD publishes no checksum: the package must be
//! signed by AMD.

use super::{Download, Plan, Step, Verify};
use crate::model::JobKind;
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::time::Duration;

const PAGES: &str = "https://www.amd.com/en/support/downloads/drivers.html/chipsets";
const NOTES_BASE: &str = "https://www.amd.com/en/resources/support-articles/release-notes";

/// AMD's newest chipset drivers.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Release {
    /// "8.09.28.1124"
    pub version: String,
    pub url: String,
    pub size: Option<u64>,
    /// The download page the link was read from (sent as the Referer)
    pub page: String,
    pub notes_url: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// The installed AMD chipset drivers' version, when AMD's installer put them on this PC.
pub fn installed() -> Option<String> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WOW64_32KEY};
    winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags("SOFTWARE\\AMD\\AMD_Chipset_IODrivers", KEY_READ | KEY_WOW64_32KEY)
        .and_then(|k| k.get_value::<String, _>("ProductVersion"))
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// AMD's chipset page for this board ("B850 GAMING X WIFI6E ICE" is an AM5 B850 board). None when
/// the board's name does not say its chipset.
pub fn known_page_for(board: &str) -> Option<String> {
    const CHIPSETS: [(&str, &str); 18] = [
        ("x870e", "am5"), ("x870", "am5"), ("b850", "am5"), ("b840", "am5"), ("x670e", "am5"), ("x670", "am5"), ("b650e", "am5"), ("b650", "am5"), ("a620", "am5"),
        ("x570", "am4"), ("b550", "am4"), ("a520", "am4"), ("x470", "am4"), ("b450", "am4"), ("x370", "am4"), ("b350", "am4"), ("a320", "am4"), ("pro565", "am4"),
    ];
    let b = board.to_ascii_lowercase();
    CHIPSETS.iter().find(|(c, _)| b.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| w.starts_with(c) && w.len() <= c.len() + 1)).map(|(chip, socket)| format!("{PAGES}/{socket}/{chip}.html"))
}

/// AMD's chipset page for this board. All of them offer the same package; an unknown board gets
/// the newest chipset's page.
pub fn page_for(board: &str) -> String {
    known_page_for(board).unwrap_or_else(|| format!("{PAGES}/am5/x870e.html"))
}

/// The newest package on AMD's chipset page.
pub fn parse_page(html: &str) -> Option<(String, String)> {
    static PKG: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https://drivers\.amd\.com/drivers/amd_chipset_software_(\d+(?:\.\d+){2,3})\.exe"#).unwrap());
    PKG.captures_iter(html).map(|c| (c[1].to_string(), c[0].to_string())).max_by(|a, b| super::tools::cmp_ver(&a.0, &b.0))
}

pub fn notes_url(version: &str) -> String {
    format!("{NOTES_BASE}/RN-RYZEN-CHIPSET-{}.html", version.replace('.', "-"))
}

/// Ask AMD for the newest chipset drivers.
pub async fn latest(board: &str) -> Result<Release> {
    let page = page_for(board);
    let f = crate::http::get_cached(&page, &[], Duration::from_secs(6 * 3600)).await?;
    let (version, url) = parse_page(&String::from_utf8_lossy(&f.body)).ok_or_else(|| anyhow!("AMD's chipset page names no package"))?;
    let notes_url = notes_url(&version);
    let notes = match crate::http::get_cached(&notes_url, &[], Duration::from_secs(24 * 3600)).await {
        Ok(n) => crate::notes::page_notes(&String::from_utf8_lossy(&n.body), &version),
        Err(_) => vec![],
    };
    let size = super::amd::package_size(&url, &page).await;
    Ok(Release { version, url, size, page, notes_url, notes })
}

/// Download AMD's chipset package and install it silently (`/S`). Windows finishes some of the
/// drivers after a restart (3010). AMD's setup also ends with 2 after a good install while
/// Windows waits for a restart from something else (seen after graphics driver updates), so the
/// installed version is read back: only a new version counts as done.
pub fn plan_install(app_id: &str, name: &str, r: &Release) -> Result<Plan> {
    if !super::drivers::host_is(&r.url, &["drivers.amd.com"]) {
        return Err(anyhow!("Unexpected AMD download address: {}", r.url));
    }
    let file = r.url.rsplit('/').next().filter(|f| f.ends_with(".exe")).ok_or_else(|| anyhow!("AMD's download is not a program"))?;
    let dest = crate::paths::downloads().join("amd").join(file);
    let dl = Download {
        url: r.url.clone(),
        dest: dest.clone(),
        size: r.size,
        sha256: None,
        md5: None,
        signer: Some(super::amd::SIGNER.into()),
        sha512: None,
        headers: vec![("Referer".into(), r.page.clone())],
    };
    super::require_checked(&dl, &format!("AMD Chipset Software {}", r.version))?;
    Ok(Plan {
        app_id: app_id.into(),
        kind: JobKind::Update,
        name: name.into(),
        to_version: r.version.clone(),
        downloads: vec![dl],
        steps: vec![Step::Run {
            exe: dest.clone(),
            args: vec!["/S".into()],
            cwd: None,
            ok_codes: vec![0, 2, super::drivers::EXIT_REBOOT],
            elevated: true,
            label: format!("Installing AMD Chipset Software {}", r.version),
            log: None,
        }],
        block_dirs: vec![],
        block_names: vec![],
        verify: Verify::Driver { app_id: app_id.into(), version: r.version.clone() },
        fallback: None,
        cleanup: vec![dest],
    })
}

// ---------------------------------------------------------------- AMD RAID

/// AMD's RAID software for Ryzen boards (SATA and NVMe RAID), from the same chipset pages: a zip
/// with AMD's installer, which installs the RAID driver (and AMD's RAIDXpert2).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct RaidRelease {
    /// The RAID driver the package installs, as Windows shows it ("9.3.3.00291")
    pub version: String,
    /// The package's own version ("8.03.24.733")
    pub package: String,
    pub url: String,
    pub size: Option<u64>,
    /// The download page the link was read from (sent as the Referer)
    pub page: String,
    pub notes_url: String,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Is this installed driver part of AMD's RAID driver ("AMD-RAID Bottom Device", "AMD-RAID Config
/// Device", "AMD-RAID Controller [storport]")? Only a PC that has it has an AMD RAID controller.
pub fn is_raid(p: &super::drivers::Present) -> bool {
    let provider = p.provider.to_ascii_lowercase();
    (provider.contains("advanced micro devices") || provider.starts_with("amd")) && p.desc.to_ascii_lowercase().starts_with("amd-raid")
}

/// The installed AMD RAID driver's version (the newest of its parts), when this PC has one.
pub fn raid_installed(present: &[super::drivers::Present]) -> Option<String> {
    present.iter().filter(|p| is_raid(p) && !p.version.is_empty()).map(|p| p.version.clone()).max_by(|a, b| super::tools::cmp_ver(a, b))
}

/// The newest RAID software package on a board's chipset page: its version and link. Only the zip
/// of the current generation (AM5 boards): the older boards' page links a 2021 program instead,
/// which the app does not install.
pub fn parse_raid_page(html: &str) -> Option<(String, String)> {
    static PKG: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https://drivers\.amd\.com/drivers/amd_raid_software_(\d+(?:\.\d+){2,3})\.zip"#).unwrap());
    PKG.captures_iter(html).map(|c| (c[1].to_string(), c[0].to_string())).max_by(|a, b| super::tools::cmp_ver(&a.0, &b.0))
}

pub fn raid_notes_url(package: &str) -> String {
    format!("{NOTES_BASE}/RN-RYZEN-RAID-{}.html", package.replace('.', "-"))
}

/// From the package's release notes: the RAID driver version it installs.
pub fn parse_raid_notes(html: &str) -> Option<String> {
    static DRIVER: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)AMD RAID Driver\b.{0,80}?(\d+\.\d+\.\d+(?:\.\d+)?)").unwrap());
    DRIVER.captures(&super::amd::page_text(html)).map(|c| c[1].to_string())
}

/// Ask AMD for the RAID software on this board's chipset page. None: the board's chipset is not
/// known, or its page has no RAID software the app installs.
pub async fn latest_raid(board: &str) -> Result<Option<RaidRelease>> {
    let Some(page) = known_page_for(board) else { return Ok(None) };
    let f = crate::http::get_cached(&page, &[], Duration::from_secs(6 * 3600)).await?;
    let Some((package, url)) = parse_raid_page(&String::from_utf8_lossy(&f.body)) else { return Ok(None) };
    let notes_url = raid_notes_url(&package);
    let n = crate::http::get_cached(&notes_url, &[], Duration::from_secs(24 * 3600)).await?;
    let text = String::from_utf8_lossy(&n.body);
    // The page names no driver version, only the package's: the notes say which driver is in it.
    let version = parse_raid_notes(&text).ok_or_else(|| anyhow!("AMD's release notes for RAID software {package} name no driver version"))?;
    let notes = crate::notes::page_notes_at(&notes_url, &text, &package);
    let size = super::amd::package_size(&url, &page).await;
    Ok(Some(RaidRelease { version, package, url, size, page, notes_url, notes }))
}

/// Download AMD's RAID software and install it silently (`/S`, as its release notes say), after
/// checking that its program is signed by AMD (AMD publishes no checksum). It is a boot driver:
/// AMD says Windows needs two restarts to finish it.
pub fn plan_raid_install(app_id: &str, name: &str, r: &RaidRelease) -> Result<Plan> {
    if !super::drivers::host_is(&r.url, &["drivers.amd.com"]) {
        return Err(anyhow!("Unexpected AMD download address: {}", r.url));
    }
    let file = r.url.rsplit('/').next().filter(|f| f.ends_with(".zip")).ok_or_else(|| anyhow!("AMD's RAID download is not a zip"))?;
    let base = crate::paths::downloads().join("amd").join("raid");
    let zip = base.join(file);
    let dir = base.join("unpacked");
    let exe = dir.join("AMD_RAID_Software.exe");
    let dl = Download { url: r.url.clone(), dest: zip.clone(), size: r.size, sha256: None, md5: None, signer: None, sha512: None, headers: vec![("Referer".into(), r.page.clone())] };
    Ok(Plan {
        app_id: app_id.into(),
        kind: JobKind::Update,
        name: name.into(),
        to_version: r.version.clone(),
        downloads: vec![dl],
        steps: vec![
            Step::ExtractZip { zip: zip.clone(), dest: dir.clone(), strip_root: false, elevated: false, label: "Unpacking AMD RAID software".into() },
            Step::CheckSigner { path: exe.clone(), signer: super::amd::SIGNER.into(), label: "Checking AMD's signature".into() },
            Step::Run { exe, args: vec!["/S".into()], cwd: Some(dir.clone()), ok_codes: vec![0, super::drivers::EXIT_REBOOT], elevated: true, label: format!("Installing AMD RAID software {}", r.package), log: None },
        ],
        block_dirs: vec![],
        block_names: vec![],
        // The new driver shows only after the restarts.
        verify: Verify::None,
        fallback: None,
        cleanup: vec![zip, dir],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newest_package_comes_from_the_boards_chipset_page() {
        assert_eq!(page_for("B850 GAMING X WIFI6E ICE"), "https://www.amd.com/en/support/downloads/drivers.html/chipsets/am5/b850.html");
        assert_eq!(page_for("X870E AORUS MASTER"), "https://www.amd.com/en/support/downloads/drivers.html/chipsets/am5/x870e.html");
        assert_eq!(page_for("B550M DS3H"), "https://www.amd.com/en/support/downloads/drivers.html/chipsets/am4/b550.html");
        assert_eq!(page_for("Some laptop"), "https://www.amd.com/en/support/downloads/drivers.html/chipsets/am5/x870e.html");
        let html = r#"<a href="https://drivers.amd.com/drivers/amd_chipset_software_8.09.28.1124.exe">Download</a>
            <a href="https://drivers.amd.com/drivers/amd_chipset_software_8.06.30.112.exe">Previous</a>
            <a href="https://www.amd.com/en/resources/support-articles/release-notes/RN-RYZEN-CHIPSET-8-09-28-1124.html">Notes</a>"#;
        let (v, url) = parse_page(html).unwrap();
        assert_eq!((v.as_str(), url.as_str()), ("8.09.28.1124", "https://drivers.amd.com/drivers/amd_chipset_software_8.09.28.1124.exe"));
        assert_eq!(notes_url(&v), "https://www.amd.com/en/resources/support-articles/release-notes/RN-RYZEN-CHIPSET-8-09-28-1124.html");
        assert!(parse_page("nothing").is_none());
        // Seen on an AM5 board: 8.06.30.112 installed, 8.09.28.1124 on AMD's page.
        assert!(crate::sources::update_offered("8.09.28.1124", "8.06.30.112", false));
    }

    #[test]
    fn the_install_is_signed_by_amd_silent_and_says_where_its_link_came_from() {
        let r = Release { version: "8.09.28.1124".into(), url: "https://drivers.amd.com/drivers/amd_chipset_software_8.09.28.1124.exe".into(), size: Some(81_551_288), page: page_for("B850"), notes_url: notes_url("8.09.28.1124"), notes: vec![] };
        let plan = plan_install("driver:amd-chipset", "AMD Chipset Software", &r).unwrap();
        let d = &plan.downloads[0];
        assert_eq!((d.signer.as_deref(), d.size), (Some(crate::sources::amd::SIGNER), Some(81_551_288)));
        assert_eq!(d.headers, vec![("Referer".to_string(), page_for("B850"))]);
        let Step::Run { args, elevated, ok_codes, .. } = &plan.steps[0] else { panic!() };
        assert!(*elevated && args == &vec!["/S".to_string()] && ok_codes.contains(&3010));
        // Seen: 8.06.30.112 to 8.09.28.1124 installed fine and AMD's setup ended with 2. It
        // counts when the chipset version really changed.
        assert!(ok_codes.contains(&2));
        assert!(matches!(&plan.verify, Verify::Driver { app_id, version } if app_id == "driver:amd-chipset" && version == "8.09.28.1124"));
        assert!(plan_install("x", "x", &Release { url: "https://evil.example/drivers.amd.com/a.exe".into(), ..r }).is_err());
    }

    fn raid_dev(desc: &str, ver: &str) -> crate::sources::drivers::Present {
        crate::sources::drivers::Present { matching_id: "pci\\ven_1022&dev_43bd&cc_0104".into(), version: ver.into(), date: None, desc: desc.into(), provider: "Advanced Micro Devices, Inc.".into(), class: "SCSIAdapter".into(), inf: String::new() }
    }

    fn raid_release() -> RaidRelease {
        RaidRelease { version: "9.3.3.00291".into(), package: "8.03.24.733".into(), url: "https://drivers.amd.com/drivers/amd_raid_software_8.03.24.733.zip".into(), size: Some(121_580_338), page: page_for("X870E AORUS MASTER"), notes_url: raid_notes_url("8.03.24.733"), notes: vec![] }
    }

    #[test]
    fn only_a_pc_with_amds_raid_driver_has_an_amd_raid_controller() {
        let amd = raid_dev("AMD-RAID Bottom Device", "9.3.2.267");
        assert!(is_raid(&amd) && is_raid(&raid_dev("AMD-RAID Config Device", "9.3.3.291")));
        // Another maker's RAID, AMD's other storage drivers and an AMD chipset driver: not it.
        assert!(!is_raid(&crate::sources::drivers::Present { provider: "Intel Corporation".into(), desc: "Intel(R) RST VMD Controller".into(), ..amd.clone() }));
        assert!(!is_raid(&crate::sources::drivers::Present { desc: "AMD PSP 11.0 Device".into(), ..amd.clone() }));
        assert!(!is_raid(&crate::sources::drivers::Present { provider: "Microsoft".into(), ..amd.clone() }));
        assert_eq!(raid_installed(&[amd.clone(), raid_dev("AMD-RAID Config Device", "9.3.3.291"), raid_dev("Some Device", "99.0")]).as_deref(), Some("9.3.3.291"));
        assert_eq!(raid_installed(&[raid_dev("Some Device", "1.0")]), None);
        assert_eq!(raid_installed(&[]), None);
    }

    #[test]
    fn the_raid_package_comes_from_the_boards_page_and_its_notes_name_the_driver() {
        assert_eq!(known_page_for("X870E AORUS MASTER").as_deref(), Some("https://www.amd.com/en/support/downloads/drivers.html/chipsets/am5/x870e.html"));
        assert_eq!(known_page_for("Some laptop"), None);
        assert_eq!(known_page_for("WRX90E-SAGE"), None);
        let page = r#"<h4>AMD RAID Installer (SATA, NVMe RAID)</h4><strong>Revision Number</strong><p>8.03.24.733</p>
            <a href="https://drivers.amd.com/drivers/amd_raid_software_8.03.24.733.zip">Download</a>
            <h4>AMD RAID Driver (SATA, NVMe RAID)</h4><a href="https://drivers.amd.com/drivers/raid_windows_driver_933_00291.zip">Download</a>"#;
        let (v, url) = parse_raid_page(page).unwrap();
        assert_eq!((v.as_str(), url.as_str()), ("8.03.24.733", "https://drivers.amd.com/drivers/amd_raid_software_8.03.24.733.zip"));
        assert_eq!(raid_notes_url(&v), "https://www.amd.com/en/resources/support-articles/release-notes/RN-RYZEN-RAID-8-03-24-733.html");
        // Seen on an AM4 board's page: a 2021 program, not a zip. Not offered.
        assert!(parse_raid_page(r#"<a href="https://drivers.amd.com/drivers/amd_raid_software_2.20.19.037.exe">Download</a>"#).is_none());
        assert!(parse_raid_page("nothing").is_none());
        let notes = "<p>Package Contents</p><table><tr><td>Driver Name</td><td>OS Supported</td><td>Supported Version</td></tr><tr><td>AMD RAID Driver</td><td>Windows 10/Windows 11</td><td>9.3.3.00291</td></tr><tr><td>AMD RAIDXpert2 Utility</td><td>Windows 10/Windows 11</td><td>9.3.3.00291</td></tr></table>";
        assert_eq!(parse_raid_notes(notes).as_deref(), Some("9.3.3.00291"));
        assert_eq!(parse_raid_notes("<p>AMD RAID Drivers and tools</p>"), None);
        // Windows shows 9.3.3.291 for the INF's 9.3.3.00291: the same version, not an update.
        assert!(!crate::sources::update_offered("9.3.3.00291", "9.3.3.291", false));
        assert!(crate::sources::update_offered("9.3.3.00291", "9.3.2.267", false));
    }

    #[test]
    fn the_raid_install_is_signed_by_amd_silent_and_waits_for_the_restarts() {
        let r = raid_release();
        let plan = plan_raid_install("driver:amd-raid", "AMD RAID Driver", &r).unwrap();
        assert_eq!(plan.downloads[0].headers, vec![("Referer".to_string(), r.page.clone())]);
        assert_eq!(plan.to_version, "9.3.3.00291");
        // No checksum from AMD and a zip cannot carry a signature: the program in it must.
        assert!(matches!(&plan.steps[0], Step::ExtractZip { .. }));
        let Step::CheckSigner { path, signer, .. } = &plan.steps[1] else { panic!() };
        assert_eq!((signer.as_str(), path.file_name().and_then(|f| f.to_str())), (crate::sources::amd::SIGNER, Some("AMD_RAID_Software.exe")));
        let Step::Run { exe, args, elevated, ok_codes, cwd, .. } = &plan.steps[2] else { panic!() };
        assert!(*elevated && args == &vec!["/S".to_string()] && ok_codes.contains(&3010) && exe == path && cwd.is_some());
        assert!(matches!(plan.verify, Verify::None), "the version shows after the restarts");
        assert_eq!(plan.cleanup.len(), 2);
        assert!(plan_raid_install("x", "x", &RaidRelease { url: "https://evil.example/drivers.amd.com/a.zip".into(), ..r.clone() }).is_err());
        assert!(plan_raid_install("x", "x", &RaidRelease { url: "https://drivers.amd.com/drivers/a.exe".into(), ..r }).is_err());
    }

    /// Asks AMD's site (no download): `cargo test --lib live_chipset -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_chipset() {
        let r = latest("B850 GAMING X WIFI6E ICE").await.expect("AMD");
        println!("{r:#?}\ninstalled: {:?}", installed());
        for board in ["X870E AORUS MASTER", "B650 AORUS ELITE", "B550M DS3H", "Some laptop"] {
            match latest_raid(board).await {
                Ok(Some(r)) => println!("{board}: RAID driver {} in package {} ({:?} bytes), {}", r.version, r.package, r.size, r.url),
                Ok(None) => println!("{board}: no RAID software the app installs"),
                Err(e) => println!("{board}: {e:#}"),
            }
        }
    }
}
