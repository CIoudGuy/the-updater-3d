//! AMD Radeon graphics drivers, from AMD's own site: the newest Adrenalin Edition release named
//! on AMD's driver page, and its full package from that release's notes. Radeon PRO workstation
//! cards get AMD Software: PRO Edition instead, from the card's own download page. AMD's download
//! server only serves files to visitors coming from its pages, so each download says which page
//! its link was read from (Referer). AMD publishes no checksum: the package must be signed by AMD.

use super::drivers::Present;
use super::{Download, Plan, Step, Verify};
use crate::model::JobKind;
use anyhow::{anyhow, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::time::Duration;

const DRIVERS_PAGE: &str = "https://www.amd.com/en/support/download/drivers.html";
const NOTES_BASE: &str = "https://www.amd.com/en/resources/support-articles/release-notes";
const GRAPHICS_PAGES: &str = "https://www.amd.com/en/support/downloads/drivers.html/graphics";
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
    /// The page the download link was read from, sent as the Referer (the release notes page
    /// when empty). A Radeon PRO card's package is read from the card's own download page.
    #[serde(default)]
    pub page: String,
}

impl Release {
    /// Is this AMD Software: PRO Edition (for Radeon PRO cards) and not Adrenalin?
    pub fn is_pro(&self) -> bool {
        self.url.contains("/prographics/")
    }
}

/// Is this card a Radeon PRO workstation card ("AMD Radeon PRO W7800", "AMD Radeon(TM) Pro W6800",
/// "AMD Radeon AI PRO R9700")? Its driver is AMD Software: PRO Edition.
pub fn is_pro(desc: &str) -> bool {
    let d = desc.to_ascii_lowercase().replace("(tm)", "").replace(['\u{2122}', '\u{ae}'], "");
    let d = d.split_whitespace().collect::<Vec<_>>().join(" ");
    d.contains("radeon pro") || d.contains("radeon ai pro")
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
    let size = package_size(&url, &notes_url).await;
    Ok(Release { version, store_versions, url, size, notes_url, notes, page: String::new() })
}

/// A page's text with a space where each tag was (so "</td><td>" does not join two cells).
pub(super) fn page_text(html: &str) -> String {
    static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"<[^>]+>").unwrap());
    crate::notes::strip_tags(&TAG.replace_all(html, " "))
}

/// The size of a package, from AMD's server (it answers visitors that come from `referer`).
pub(super) async fn package_size(url: &str, referer: &str) -> Option<u64> {
    crate::http::DL_CLIENT
        .head(url)
        .header(reqwest::header::REFERER, referer)
        .send()
        .await
        .ok()
        .filter(|r| r.status().is_success())
        // Read the header: reqwest counts a HEAD answer's (empty) body instead.
        .and_then(|r| r.headers().get(reqwest::header::CONTENT_LENGTH)?.to_str().ok()?.parse::<u64>().ok())
        .filter(|&n| n > 1_000_000)
}

/// AMD's download page for a Radeon PRO card and the model its title names, from the card's name.
/// Only the series whose pages follow one pattern: Radeon PRO W6000 and W7000 and Radeon AI PRO
/// R9000 ("AMD Radeon PRO W7800" is on .../radeon-pro-w7000-series/amd-radeon-pro-w7800.html).
/// The older W5000 and WX cards and the mobile ones (W6600M) have no such page: they are left out.
pub fn pro_page(desc: &str) -> Option<(String, String)> {
    static W: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bW([67])\d{3}D?\b").unwrap());
    static R: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bR9\d{3}[SD]?\b").unwrap());
    if desc.to_ascii_lowercase().contains("ai pro") {
        let m = R.find(desc)?.as_str().to_ascii_lowercase();
        return Some((format!("{GRAPHICS_PAGES}/radeon-ai-pro/radeon-ai-pro-r9000-series/amd-radeon-ai-pro-{m}.html"), m.to_ascii_uppercase()));
    }
    let c = W.captures(desc)?;
    let m = c[0].to_ascii_lowercase();
    Some((format!("{GRAPHICS_PAGES}/radeon-pro/radeon-pro-w{}000-series/amd-radeon-pro-{m}.html", &c[1]), m.to_ascii_uppercase()))
}

/// From a card's download page: its AMD Software: PRO Edition package for Windows (the link, the
/// revision "26.Q3" and AMD's release notes page for it). None when the page is not for `model`
/// (AMD answers an unknown card with another page) or names no single package for the card.
pub fn parse_pro_page(html: &str, model: &str) -> Option<(String, String, String)> {
    static TITLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap());
    static PKG: Lazy<Regex> = Lazy::new(|| Regex::new(r"https://drivers\.amd\.com/drivers/prographics/amd-software-pro-edition-[0-9a-z.]+-win11-[a-z]\.exe").unwrap());
    static REV: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)Revision Number\s*</strong>\s*<p>\s*([^<]+?)\s*</p>").unwrap());
    static NOTES: Lazy<Regex> = Lazy::new(|| Regex::new(r#"href="(/en/resources/support-articles/release-notes/RN-PRO-WIN-[^"]+\.html)""#).unwrap());
    let title = TITLE.captures(html)?[1].to_ascii_uppercase();
    if !title.split(|c: char| !c.is_ascii_alphanumeric()).any(|w| w == model.to_ascii_uppercase()) {
        return None;
    }
    // The Windows 11 and Windows 10 sections of the page link the same file.
    let mut urls: Vec<&str> = PKG.find_iter(html).map(|m| m.as_str()).collect();
    urls.sort();
    urls.dedup();
    let [url] = urls[..] else { return None };
    // The package's own block of the page: its revision and the link to its release notes.
    let at = html.find(url)?;
    let block = html[..at].rfind("<article").map(|a| &html[a..at + html[at..].find("</article>").unwrap_or(0)]).unwrap_or(html);
    let revision = REV.captures(block)?[1].trim().to_string();
    let notes = NOTES.captures(block).or_else(|| NOTES.captures(html))?;
    Some((url.to_string(), revision, format!("https://www.amd.com{}", &notes[1])))
}

/// From a PRO Edition release notes page: the Windows driver version the package installs. The
/// page links each package under the cards it is for ("RDNA3 graphics and newer") and lists the
/// driver version of each of those groups.
pub fn parse_pro_notes(html: &str, url: &str) -> Option<String> {
    static VERSIONS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)([A-Za-z0-9/ ]+?)\s*\([^)]*\)\s*WHQL Driver Package Version:\s*[0-9.]+\s*Windows Store Version:\s*(\d+(?:\.\d+){3})").unwrap());
    let key = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let found: Vec<(String, String)> = VERSIONS.captures_iter(&page_text(html)).map(|c| (key(&c[1]), c[2].to_string())).collect();
    if let [(_, only)] = &found[..] {
        return Some(only.clone());
    }
    let link = Regex::new(&format!(r#"href="{}"[^>]*>([^<]+)</a>"#, regex::escape(url))).ok()?;
    let label = key(&link.captures(html)?[1]);
    found.into_iter().find(|(k, _)| !label.is_empty() && k.ends_with(&label)).map(|(_, v)| v)
}

/// Ask AMD for the AMD Software: PRO Edition of a Radeon PRO card. None: AMD has no page for
/// this card that the app can read (see `pro_page`).
pub async fn latest_pro(desc: &str) -> Result<Option<Release>> {
    let Some((page, model)) = pro_page(desc) else { return Ok(None) };
    let f = crate::http::get_cached(&page, &[], Duration::from_secs(3 * 3600)).await?;
    let Some((url, revision, notes_url)) = parse_pro_page(&String::from_utf8_lossy(&f.body), &model) else { return Ok(None) };
    let n = crate::http::get_cached(&notes_url, &[], Duration::from_secs(24 * 3600)).await?;
    let text = String::from_utf8_lossy(&n.body);
    let store = parse_pro_notes(&text, &url).ok_or_else(|| anyhow!("AMD's release notes for PRO Edition {revision} name no driver version for this package"))?;
    let notes = crate::notes::page_notes_at(&notes_url, &text, &revision);
    let size = package_size(&url, &page).await;
    Ok(Some(Release { version: revision, store_versions: vec![store], url, size, notes_url, notes, page }))
}

/// Download AMD's full package and install it silently (AMD's documented `-INSTALL`), keeping the
/// user's AMD Software settings. It asks for no restart.
pub fn plan_install(app_id: &str, name: &str, r: &Release) -> Result<Plan> {
    let file = r.url.rsplit('/').next().filter(|f| f.ends_with(".exe")).ok_or_else(|| anyhow!("AMD's download is not a program"))?;
    let dest = crate::paths::downloads().join("amd").join(file);
    let what = if r.is_pro() { format!("AMD Software: PRO Edition {}", r.version) } else { format!("AMD Adrenalin {}", r.version) };
    let dl = Download {
        url: r.url.clone(),
        dest: dest.clone(),
        size: r.size,
        sha256: None,
        md5: None,
        signer: Some(SIGNER.into()),
        sha512: None,
        headers: vec![("Referer".into(), if r.page.is_empty() { r.notes_url.clone() } else { r.page.clone() })],
    };
    super::require_checked(&dl, &what)?;
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
            label: format!("Installing {what}"),
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
        Release { version: "26.10.1".into(), store_versions, url, size: Some(1_782_243_424), notes_url: notes_url("26.10.1"), notes: vec![], page: String::new() }
    }

    /// A Radeon PRO card's download page, cut down to what the parser reads.
    const PRO_PAGE: &str = r#"<html><head><title>AMD Radeon&trade; PRO W7800 Drivers and Downloads | Latest Version</title></head><body>
        <h2>Windows 11 - 64-Bit Edition</h2>
        <article class="driver-download-details"><h4>AMD Software: PRO Edition</h4>
        <strong>Revision Number</strong> <p>26.Q3</p> <strong>File Size</strong> <p>873 MB</p>
        <a href="https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-b.exe">Download</a>
        <a href="/en/resources/support-articles/release-notes/RN-PRO-WIN-26-Q3.html">Release Notes</a></article>
        <article class="driver-download-details"><h4>AMD Software: Adrenalin Edition</h4>
        <strong>Revision Number</strong> <p>Adrenalin 26.10.1 WHQL</p>
        <a href="https://drivers.amd.com/drivers/whql-amd-software-adrenalin-edition-26.10.1-win11-b.exe">Download</a></article>
        <h2>Windows 10 - 64-Bit Edition</h2>
        <article class="driver-download-details"><h4>AMD Software: PRO Edition</h4>
        <strong>Revision Number</strong> <p>26.Q3</p>
        <a href="https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-b.exe">Download</a></article></body></html>"#;
    const PRO_NOTES: &str = r#"<table><tr><td>Windows 10/11 (64-bit)</td><td><a href="https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-b.exe">RDNA3 graphics and newer</a></td></tr>
        <tr><td>Windows 10/11 (64-bit)</td><td><a href="https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-a.exe">RDNA/RDNA2 graphics</a></td></tr></table>
        <h3>Packaged Driver Versions &amp; APIs</h3><p>RDNA3 Graphics and newer (Windows &reg; 10/11, Windows &reg; Server 2022)<br>WHQL Driver Package Version: 26.10.36<br>Windows Store Version: 32.0.31036.15</p>
        <p>RDNA/RDNA2 Graphics (Windows &reg; 10/11, Windows &reg; Server 2022)<br>WHQL Driver Package Version: 25.10.45.01<br>Windows Store Version: 32.0.21045.1000</p>"#;

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

    #[test]
    fn radeon_pro_cards_are_told_from_the_others() {
        assert!(is_pro("AMD Radeon PRO W7800"));
        assert!(is_pro("AMD Radeon(TM) Pro W6800"));
        assert!(is_pro("AMD Radeon\u{2122} PRO W7600"));
        assert!(is_pro("AMD Radeon AI PRO R9700"));
        assert!(!is_pro("AMD Radeon RX 7900 XTX"));
        assert!(!is_pro("AMD Radeon(TM) Graphics"));
        assert!(!is_pro("AMD Radeon 780M Graphics"));
    }

    #[test]
    fn a_pro_card_is_found_on_its_own_page_when_the_series_has_one() {
        let g = "https://www.amd.com/en/support/downloads/drivers.html/graphics";
        assert_eq!(pro_page("AMD Radeon PRO W7800"), Some((format!("{g}/radeon-pro/radeon-pro-w7000-series/amd-radeon-pro-w7800.html"), "W7800".into())));
        assert_eq!(pro_page("AMD Radeon(TM) Pro W6800").unwrap().0, format!("{g}/radeon-pro/radeon-pro-w6000-series/amd-radeon-pro-w6800.html"));
        assert_eq!(pro_page("AMD Radeon PRO W7900D").unwrap().1, "W7900D");
        assert_eq!(pro_page("AMD Radeon PRO W7900 Dual Slot").unwrap().1, "W7900");
        assert_eq!(pro_page("AMD Radeon AI PRO R9700S"), Some((format!("{g}/radeon-ai-pro/radeon-ai-pro-r9000-series/amd-radeon-ai-pro-r9700s.html"), "R9700S".into())));
        // Left out: no page of this pattern (W5000 and WX cards, mobile cards, unknown names).
        assert_eq!(pro_page("AMD Radeon PRO W5700"), None);
        assert_eq!(pro_page("AMD Radeon PRO W6600M"), None);
        assert_eq!(pro_page("Radeon Pro WX 5100"), None);
        assert_eq!(pro_page("AMD Radeon PRO V620"), None);
    }

    #[test]
    fn the_cards_page_names_one_pro_edition_package_with_its_revision_and_notes() {
        let (url, rev, notes) = parse_pro_page(PRO_PAGE, "W7800").unwrap();
        assert_eq!(url, "https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-b.exe");
        assert_eq!((rev.as_str(), notes.as_str()), ("26.Q3", "https://www.amd.com/en/resources/support-articles/release-notes/RN-PRO-WIN-26-Q3.html"));
        // AMD answers a card it does not know with another page: not this card's.
        assert!(parse_pro_page(PRO_PAGE, "W7600").is_none());
        assert!(parse_pro_page(PRO_PAGE, "W780").is_none());
        // Two different packages for one card: nothing says which is the card's.
        let two = PRO_PAGE.replace("</body>", r#"<a href="https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-a.exe">x</a></body>"#);
        assert!(parse_pro_page(&two, "W7800").is_none());
        assert!(parse_pro_page("<title>AMD Radeon PRO W7800</title>no package", "W7800").is_none());
    }

    #[test]
    fn the_notes_give_the_driver_version_of_the_cards_own_package() {
        let b = "https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-b.exe";
        let a = "https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-a.exe";
        assert_eq!(parse_pro_notes(PRO_NOTES, b).as_deref(), Some("32.0.31036.15"));
        assert_eq!(parse_pro_notes(PRO_NOTES, a).as_deref(), Some("32.0.21045.1000"));
        // A package the notes do not link, with two groups listed: not guessed.
        assert_eq!(parse_pro_notes(PRO_NOTES, "https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-c.exe"), None);
        // One group only: its version is the package's.
        let one = r#"<p>RDNA3 Graphics and newer (Windows 10/11)<br>WHQL Driver Package Version: 26.10.36<br>Windows Store Version: 32.0.31036.15</p>"#;
        assert_eq!(parse_pro_notes(one, b).as_deref(), Some("32.0.31036.15"));
        assert_eq!(parse_pro_notes("nothing here", b), None);
    }

    #[test]
    fn a_pro_edition_is_signed_by_amd_silent_and_says_which_page_its_link_came_from() {
        let page = "https://www.amd.com/en/support/downloads/drivers.html/graphics/radeon-pro/radeon-pro-w7000-series/amd-radeon-pro-w7800.html";
        let r = Release {
            version: "26.Q3".into(),
            store_versions: vec!["32.0.31036.15".into()],
            url: "https://drivers.amd.com/drivers/prographics/amd-software-pro-edition-26.q3-win11-b.exe".into(),
            size: Some(915_537_728),
            notes_url: "https://www.amd.com/en/resources/support-articles/release-notes/RN-PRO-WIN-26-Q3.html".into(),
            notes: vec![],
            page: page.into(),
        };
        assert!(r.is_pro() && !rel().is_pro());
        let plan = plan_install("driver:amd:x", "AMD Radeon PRO W7800", &r).unwrap();
        assert_eq!(plan.downloads[0].headers, vec![("Referer".to_string(), page.to_string())]);
        assert_eq!(plan.downloads[0].signer.as_deref(), Some(SIGNER));
        assert_eq!(plan.to_version, "32.0.31036.15");
        let Step::Run { args, label, .. } = &plan.steps[0] else { panic!() };
        assert_eq!((args, label.as_str()), (&vec!["-INSTALL".to_string()], "Installing AMD Software: PRO Edition 26.Q3"));
        // One driver branch: older than it is an update, the same is current.
        assert!(needs_update("32.0.31000.1", &r) && !needs_update("32.0.31036.15", &r) && !needs_update("32.0.32000.1", &r));
    }

    /// Asks AMD's site (no download): `cargo test --lib live_amd -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_amd() {
        let r = latest(true).await.expect("AMD");
        println!("{r:#?}");
        assert!(r.url.starts_with("https://drivers.amd.com/"));
        for card in ["AMD Radeon PRO W7800", "AMD Radeon PRO W6800", "AMD Radeon PRO W7900D", "AMD Radeon AI PRO R9700", "AMD Radeon PRO W5700"] {
            match latest_pro(card).await {
                Ok(Some(p)) => println!("{card}: PRO Edition {} installs {:?}, {:?} bytes, {}", p.version, p.store_versions, p.size, p.url),
                Ok(None) => println!("{card}: no PRO Edition page the app can read"),
                Err(e) => println!("{card}: {e:#}"),
            }
        }
    }
}
