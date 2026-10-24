//! AMD Radeon graphics drivers, from AMD's own site: the newest Adrenalin Edition release named
//! on AMD's driver page, and its full package from that release's notes. AMD's download server
//! only serves files to visitors coming from its pages, so each download says which page its
//! link was read from (Referer). AMD publishes no checksum: the package must be signed by AMD.

use super::drivers::Present;
use super::{Download, Plan, Step, Verify};
use crate::model::JobKind;
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::time::Duration;

const DRIVERS_PAGE: &str = "https://www.amd.com/en/support/download/drivers.html";
const NOTES_BASE: &str = "https://www.amd.com/en/resources/support-articles/release-notes";
/// The certificate name Windows shows for AMD's signature
pub const SIGNER: &str = "Advanced Micro Devices";

/// The newest Adrenalin Edition release.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Release {
    /// "26.10.1"
    pub version: String,
    /// Windows driver versions this release installs ("32.0.32015.7029"); the first is the main
    /// one, others are for older cards that AMD keeps on their own branch.
    pub store_versions: Vec<String>,
    pub url: String,
    pub size: Option<u64>,
    /// The release notes page the download link was read from
    pub notes_url: String,
    /// What is new, from that page
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Is this installed driver for an AMD graphics card?
pub fn is_gpu(p: &Present) -> bool {
    p.class.eq_ignore_ascii_case("Display") && p.matching_id.to_ascii_lowercase().starts_with("pci\\ven_1002")
}

/// The newest release named on AMD's driver page (its web installer's file name has it).
pub fn newest_version(drivers_page: &str) -> Option<String> {
    static RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"adrenalin-edition-(\d+\.\d+\.\d+)-minimalsetup").unwrap());
    RE.captures_iter(drivers_page).map(|c| c[1].to_string()).max_by(|a, b| super::tools::cmp_ver(a, b))
}

pub fn notes_url(version: &str) -> String {
    format!("{NOTES_BASE}/RN-RAD-WIN-{}.html", version.replace('.', "-"))
}

/// From a release notes page: the full package for this Windows ("win11"/"win10" in its name)
/// and the driver versions it installs.
pub fn parse_notes(html: &str, version: &str, windows11: bool) -> Result<(String, Vec<String>)> {
    static PKG: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https://drivers\.amd\.com/drivers/whql-amd-software-adrenalin-edition-[0-9.]+-win(10|11)[^"'<> ]*\.exe"#).unwrap());
    static STORE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)driver\s+store\s+version\s*:?\s*(?:</?[a-z][^>]*>\s*)*(\d+\.\d+\.\d+\.\d+)").unwrap());
    let want = if windows11 { "11" } else { "10" };
    let pkgs: Vec<(String, String)> = PKG.captures_iter(html).map(|c| (c[0].to_string(), c[1].to_string())).collect();
    let url = pkgs
        .iter()
        .find(|(u, w)| w == want && u.contains(&format!("-{version}-")))
        .or_else(|| pkgs.iter().find(|(u, _)| u.contains(&format!("-{version}-"))))
        .map(|(u, _)| u.clone())
        .ok_or_else(|| anyhow!("AMD's release notes for {version} have no full package"))?;
    let mut stores: Vec<String> = Vec::new();
    for c in STORE.captures_iter(html) {
        if !stores.contains(&c[1].to_string()) {
            stores.push(c[1].to_string());
        }
    }
    if stores.is_empty() {
        return Err(anyhow!("AMD's release notes for {version} name no driver version"));
    }
    Ok((url, stores))
}

/// Is the installed driver older than the release? A card on one of the release's other branches
/// (older cards) is current when it has that branch's driver.
pub fn needs_update(installed: &str, r: &Release) -> bool {
    if r.store_versions.iter().any(|v| v == installed) {
        return false;
    }
    r.store_versions.first().is_some_and(|main| super::update_offered(main, installed, false))
        && !r.store_versions.iter().skip(1).any(|v| !super::update_offered(v, installed, false))
}

/// Ask AMD for the newest release and its full package.
pub async fn latest(windows11: bool) -> Result<Release> {
    let page = crate::http::get_cached(DRIVERS_PAGE, &[], Duration::from_secs(3 * 3600)).await?;
    let version = newest_version(&String::from_utf8_lossy(&page.body)).ok_or_else(|| anyhow!("AMD's driver page names no Adrenalin release"))?;
    let notes_url = notes_url(&version);
    let notes = crate::http::get_cached(&notes_url, &[], Duration::from_secs(24 * 3600)).await?;
    let page = String::from_utf8_lossy(&notes.body);
    let (url, store_versions) = parse_notes(&page, &version, windows11)?;
    let notes = crate::notes::page_notes(&page, &version);
    let size = crate::http::DL_CLIENT
        .head(&url)
        .header(reqwest::header::REFERER, &notes_url)
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())
        // Read the header: reqwest counts a HEAD answer's (empty) body instead.
        .and_then(|r| r.headers().get(reqwest::header::CONTENT_LENGTH)?.to_str().ok()?.parse::<u64>().ok())
        .filter(|&n| n > 1_000_000);
    Ok(Release { version, store_versions, url, size, notes_url, notes })
}

/// Download AMD's full package and install it silently (AMD's documented `-INSTALL`), keeping the
/// user's AMD Software settings. It asks for no restart.
pub fn plan_install(app_id: &str, name: &str, r: &Release) -> Result<Plan> {
    let file = r.url.rsplit('/').next().filter(|f| f.ends_with(".exe")).ok_or_else(|| anyhow!("AMD's download is not a program"))?;
    let dest = crate::paths::downloads().join("amd").join(file);
    let dl = Download {
        url: r.url.clone(),
        dest: dest.clone(),
        size: r.size,
        sha256: None,
        md5: None,
        signer: Some(SIGNER.into()),
        sha512: None,
        headers: vec![("Referer".into(), r.notes_url.clone())],
    };
    super::require_checked(&dl, &format!("AMD Adrenalin {}", r.version))?;
    Ok(Plan {
        app_id: app_id.into(),
        kind: JobKind::Update,
        name: name.into(),
        to_version: r.store_versions.first().cloned().unwrap_or_else(|| r.version.clone()),
        downloads: vec![dl],
        steps: vec![Step::Run {
            exe: dest.clone(),
            args: vec!["-INSTALL".into()],
            cwd: None,
            ok_codes: vec![0],
            elevated: true,
            label: format!("Installing AMD Adrenalin {}", r.version),
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

    const DRIVERS: &str = r#"<a href="https://drivers.amd.com/drivers/installer/26.20/whql/amd-software-adrenalin-edition-26.10.1-minimalsetup-261018_web.exe">Download</a>
        <a href="https://drivers.amd.com/drivers/installer/26.10/whql/amd-software-adrenalin-edition-26.9.2-minimalsetup-260930_web.exe">Older</a>"#;
    const NOTES: &str = r#"<h2>Package Contents</h2><p>Driver Version 26.20.15.07</p><p>Driver Store Version 32.0.32015.7029</p>
        <p>Polaris and Vega: Driver Store Version <b>32.0.21045.16010</b></p>
        <a href="https://drivers.amd.com/drivers/whql-amd-software-adrenalin-edition-26.10.1-win10-c.exe">Windows 10</a>
        <a href="https://drivers.amd.com/drivers/whql-amd-software-adrenalin-edition-26.10.1-win11-c.exe">Windows 11</a>"#;

    fn rel() -> Release {
        let (url, store_versions) = parse_notes(NOTES, "26.10.1", true).unwrap();
        Release { version: "26.10.1".into(), store_versions, url, size: Some(1_782_243_424), notes_url: notes_url("26.10.1"), notes: vec![] }
    }

    #[test]
    fn the_newest_release_comes_from_the_driver_page() {
        assert_eq!(newest_version(DRIVERS).as_deref(), Some("26.10.1"));
        assert_eq!(newest_version("nothing"), None);
        assert_eq!(notes_url("26.10.1"), "https://www.amd.com/en/resources/support-articles/release-notes/RN-RAD-WIN-26-10-1.html");
    }

    #[test]
    fn the_notes_give_the_package_for_this_windows_and_its_driver_versions() {
        let (u11, v) = parse_notes(NOTES, "26.10.1", true).unwrap();
        assert!(u11.ends_with("26.10.1-win11-c.exe"));
        assert_eq!(v, vec!["32.0.32015.7029".to_string(), "32.0.21045.16010".to_string()]);
        let (u10, _) = parse_notes(NOTES, "26.10.1", false).unwrap();
        assert!(u10.ends_with("26.10.1-win10-c.exe"));
        assert!(parse_notes(NOTES, "26.11.1", true).is_err());
        assert!(parse_notes("Driver Store Version 32.0.1.1", "26.10.1", true).is_err());
    }

    #[test]
    fn older_cards_on_their_own_branch_are_current() {
        let r = rel();
        assert!(needs_update("32.0.21001.9024", &r));
        assert!(!needs_update("32.0.32015.7029", &r));
        assert!(!needs_update("32.0.21045.16010", &r), "Polaris/Vega branch, already the newest for it");
        assert!(!needs_update("32.0.33000.1", &r), "newer than AMD's page (a preview driver)");
    }

    #[test]
    fn the_install_is_signed_by_amd_silent_and_says_where_its_link_came_from() {
        let plan = plan_install("driver:amd:x", "AMD Radeon RX 7900 XTX", &rel()).unwrap();
        let d = &plan.downloads[0];
        assert_eq!(d.signer.as_deref(), Some(SIGNER));
        assert_eq!(d.headers, vec![("Referer".to_string(), notes_url("26.10.1"))]);
        assert_eq!(plan.to_version, "32.0.32015.7029");
        let Step::Run { args, elevated, .. } = &plan.steps[0] else { panic!() };
        assert!(*elevated && args == &vec!["-INSTALL".to_string()]);
    }

    /// Asks AMD's site (no download): `cargo test --lib live_amd -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_amd() {
        let r = latest(true).await.expect("AMD");
        println!("{r:#?}");
        assert!(r.url.starts_with("https://drivers.amd.com/"));
    }
}
