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

/// AMD's chipset page for this board ("B850 GAMING X WIFI6E ICE" is an AM5 B850 board). All of
/// them offer the same package; an unknown board gets the newest chipset's page.
pub fn page_for(board: &str) -> String {
    const CHIPSETS: [(&str, &str); 18] = [
        ("x870e", "am5"), ("x870", "am5"), ("b850", "am5"), ("b840", "am5"), ("x670e", "am5"), ("x670", "am5"), ("b650e", "am5"), ("b650", "am5"), ("a620", "am5"),
        ("x570", "am4"), ("b550", "am4"), ("a520", "am4"), ("x470", "am4"), ("b450", "am4"), ("x370", "am4"), ("b350", "am4"), ("a320", "am4"), ("pro565", "am4"),
    ];
    let b = board.to_ascii_lowercase();
    let (chip, socket) = CHIPSETS.iter().find(|(c, _)| b.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|w| w.starts_with(c) && w.len() <= c.len() + 1)).copied().unwrap_or(("x870e", "am5"));
    format!("{PAGES}/{socket}/{chip}.html")
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
    let size = crate::http::DL_CLIENT
        .head(&url)
        .header(reqwest::header::REFERER, &page)
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())
        // Read the header: reqwest counts a HEAD answer's (empty) body instead.
        .and_then(|r| r.headers().get(reqwest::header::CONTENT_LENGTH)?.to_str().ok()?.parse::<u64>().ok())
        .filter(|&n| n > 1_000_000);
    Ok(Release { version, url, size, page, notes_url, notes })
}

/// Download AMD's chipset package and install it silently (`/S`). Windows finishes some of the
/// drivers after a restart (3010).
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
            ok_codes: vec![0, super::drivers::EXIT_REBOOT],
            elevated: true,
            label: format!("Installing AMD Chipset Software {}", r.version),
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
        assert!(plan_install("x", "x", &Release { url: "https://evil.example/drivers.amd.com/a.exe".into(), ..r }).is_err());
    }

    /// Asks AMD's site (no download): `cargo test --lib live_chipset -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_chipset() {
        let r = latest("B850 GAMING X WIFI6E ICE").await.expect("AMD");
        println!("{r:#?}\ninstalled: {:?}", installed());
    }
}
