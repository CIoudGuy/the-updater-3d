//! NVIDIA graphics drivers, straight from NVIDIA: the newest Game Ready / Studio-branch driver
//! for each NVIDIA card in this PC, from the same lookup nvidia.com's driver search uses.
//! NVIDIA publishes no checksum, so the download must be signed by NVIDIA (Authenticode).

use super::drivers::Present;
use super::{Download, Plan, Step, Verify};
use crate::model::JobKind;
use anyhow::{anyhow, Context, Result};
use std::time::Duration;

/// Every NVIDIA product with its id ("pfid"), from NVIDIA's download API.
const PRODUCTS_URL: &str = "https://www.nvidia.com/Download/API/lookupValueSearch.aspx?TypeID=3";
const LOOKUP_URL: &str = "https://gfwsl.geforce.com/services_toolkit/services/com/nvidia/services/AjaxDriverService.php";
pub const SIGNER: &str = "NVIDIA Corporation";
/// NVIDIA's operating system ids
const OS_WIN11: &str = "135";
const OS_WIN10_64: &str = "57";

/// The newest driver NVIDIA offers for one product.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Release {
    /// "621.01"
    pub version: String,
    pub url: String,
    /// Exact size from NVIDIA's download server; the download is checked against it
    pub size: Option<u64>,
    /// NVIDIA's rounded size ("1.04 GB"), only to show
    pub approx_size: Option<u64>,
    /// "2026.10.21"
    pub date: Option<String>,
    pub notes_url: Option<String>,
    /// "GeForce Game Ready Driver"
    pub name: String,
    /// What is new, from NVIDIA's release highlights
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Is this installed driver for an NVIDIA graphics card?
pub fn is_gpu(p: &Present) -> bool {
    p.class.eq_ignore_ascii_case("Display") && p.matching_id.to_ascii_lowercase().starts_with("pci\\ven_10de")
}

/// Windows' driver version as NVIDIA numbers it: 32.0.16.1088 is 610.88 (the last five digits).
pub fn nv_version(windows: &str) -> Option<String> {
    let parts: Vec<&str> = windows.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    let digits = format!("{}{:0>4}", parts[2], parts[3]);
    if digits.len() < 5 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let last = &digits[digits.len() - 5..];
    Some(format!("{}.{}", &last[..3], &last[3..]))
}

/// "NVIDIA GeForce RTX 4060 Ti" and "GeForce RTX 4060 Ti" are the same product.
fn norm(name: &str) -> String {
    let n = name.to_ascii_lowercase().replace("(tm)", "").replace("(r)", "");
    let n = n.trim().strip_prefix("nvidia ").unwrap_or(n.trim()).to_string();
    n.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Product names and ids from NVIDIA's product list (XML).
pub fn parse_products(xml: &str) -> Vec<(String, String)> {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return vec![] };
    doc.descendants()
        .filter(|n| n.has_tag_name("LookupValue"))
        .filter_map(|n| {
            let child = |t: &str| n.children().find(|c| c.has_tag_name(t)).and_then(|c| c.text()).map(|s| s.trim().to_string());
            Some((child("Name")?, child("Value")?))
        })
        .collect()
}

/// The product id for a card, by the name Windows shows for it.
pub fn product_id(device: &str, products: &[(String, String)]) -> Option<String> {
    let want = norm(device);
    products.iter().find(|(n, _)| norm(n) == want).map(|(_, id)| id.clone())
}

/// "1.04 GB" as bytes (NVIDIA only gives a rounded size).
fn parse_size(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let mult = match it.next()?.to_ascii_uppercase().as_str() {
        "GB" => 1024.0 * 1024.0 * 1024.0,
        "MB" => 1024.0 * 1024.0,
        "KB" => 1024.0,
        _ => return None,
    };
    Some((n * mult) as u64)
}

/// "Wed Oct 21, 2026" as "2026.10.21"
fn parse_date(s: &str) -> Option<String> {
    let rest = s.split_once(' ').map(|(_, r)| r).unwrap_or(s);
    chrono::NaiveDate::parse_from_str(rest.trim(), "%b %d, %Y").ok().map(|d| d.format("%Y.%m.%d").to_string())
}

/// The lookup's answer (JSON): the first driver it lists.
pub fn parse_lookup(json: &str) -> Result<Release> {
    let v: serde_json::Value = serde_json::from_str(json).context("NVIDIA's answer is not JSON")?;
    if v["Success"].as_str() != Some("1") && v["Success"].as_i64() != Some(1) {
        return Err(anyhow!("NVIDIA has no driver for this card"));
    }
    let d = &v["IDS"][0]["downloadInfo"];
    let s = |k: &str| d[k].as_str().map(|x| x.trim().to_string()).filter(|x| !x.is_empty());
    let version = s("Version").ok_or_else(|| anyhow!("NVIDIA's answer has no version"))?;
    let url = s("DownloadURL").ok_or_else(|| anyhow!("NVIDIA's answer has no download"))?;
    if !super::drivers::host_is(&url, &["download.nvidia.com"]) {
        return Err(anyhow!("Unexpected NVIDIA download address: {url}"));
    }
    let notes = s("ReleaseNotes").map(|n| super::drivers::html_notes(&super::drivers::percent_decode(&n))).unwrap_or_default();
    Ok(Release {
        version,
        url,
        size: None,
        approx_size: s("DownloadURLFileSize").as_deref().and_then(parse_size),
        date: s("ReleaseDateTime").as_deref().and_then(parse_date),
        notes_url: s("DetailsURL").filter(|u| super::drivers::host_is(u, &["nvidia.com"])),
        name: s("Name").map(|n| super::drivers::percent_decode(&n)).unwrap_or_else(|| "NVIDIA driver".into()),
        notes,
    })
}

fn os_id() -> &'static str {
    use winreg::enums::HKEY_LOCAL_MACHINE;
    let build: u32 = winreg::RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion")
        .and_then(|k| k.get_value::<String, _>("CurrentBuildNumber"))
        .ok()
        .and_then(|b| b.parse().ok())
        .unwrap_or(0);
    if build >= 22000 { OS_WIN11 } else { OS_WIN10_64 }
}

/// NVIDIA's product list, kept for a day.
pub async fn products() -> Result<Vec<(String, String)>> {
    let f = crate::http::get_cached(PRODUCTS_URL, &[], Duration::from_secs(24 * 3600)).await?;
    let list = parse_products(&String::from_utf8_lossy(&f.body));
    if list.is_empty() {
        return Err(anyhow!("NVIDIA's product list is empty"));
    }
    Ok(list)
}

/// The newest DCH (standard Windows 10/11) driver for a product id.
pub async fn latest(pfid: &str) -> Result<Release> {
    let url = format!("{LOOKUP_URL}?func=DriverManualLookup&pfid={pfid}&osID={}&dch=1&isWHQL=1", os_id());
    let f = crate::http::get_cached(&url, &[], Duration::from_secs(30 * 60)).await?;
    let mut r = parse_lookup(&String::from_utf8_lossy(&f.body))?;
    r.size = exact_size(&r.url).await;
    Ok(r)
}

/// The file's exact size from the download server (the lookup only gives a rounded one).
async fn exact_size(url: &str) -> Option<u64> {
    let r = crate::http::DL_CLIENT.head(url).send().await.ok().filter(|r| r.status().is_success())?;
    // Read the header: reqwest counts a HEAD answer's (empty) body instead.
    r.headers().get(reqwest::header::CONTENT_LENGTH)?.to_str().ok()?.parse::<u64>().ok().filter(|&n| n > 1_000_000)
}

/// Download NVIDIA's package and run it silently. Its own setup keeps the user's settings and
/// asks for no restart; Windows switches to the new driver right away (the screen may blink).
pub fn plan_install(app_id: &str, name: &str, r: &Release) -> Result<Plan> {
    let file = r.url.rsplit('/').next().filter(|f| f.ends_with(".exe")).ok_or_else(|| anyhow!("NVIDIA's download is not a program"))?;
    let dest = crate::paths::downloads().join("nvidia").join(file);
    let dl = Download { url: r.url.clone(), dest: dest.clone(), size: r.size, sha256: None, md5: None, signer: Some(SIGNER.into()), sha512: None, headers: vec![] };
    super::require_checked(&dl, &format!("{name} {}", r.version))?;
    Ok(Plan {
        app_id: app_id.into(),
        kind: JobKind::Update,
        name: name.into(),
        to_version: r.version.clone(),
        downloads: vec![dl],
        steps: vec![Step::Run {
            exe: dest.clone(),
            // -s: silent, -noreboot: never restart on its own. 1 means "done, restart later".
            args: vec!["-s".into(), "-noreboot".into()],
            cwd: None,
            ok_codes: vec![0, 1],
            elevated: true,
            label: format!("Installing NVIDIA driver {}", r.version),
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
    fn windows_driver_versions_read_as_nvidia_numbers() {
        assert_eq!(nv_version("32.0.16.1088").as_deref(), Some("610.88"));
        assert_eq!(nv_version("32.0.15.6094").as_deref(), Some("560.94"));
        assert_eq!(nv_version("31.0.15.0101").as_deref(), Some("501.01"));
        assert_eq!(nv_version("garbage"), None);
    }

    #[test]
    fn cards_are_found_by_the_name_windows_shows() {
        let xml = r#"<?xml version="1.0"?><LookupValueSearch><LookupValues>
            <LookupValue ParentID="127"><Name>NVIDIA GeForce RTX 4060 Ti</Name><Value>1022</Value></LookupValue>
            <LookupValue ParentID="127"><Name>NVIDIA GeForce RTX 4060</Name><Value>1023</Value></LookupValue>
            <LookupValue ParentID="129"><Name>GeForce RTX 4060 Laptop GPU</Name><Value>1007</Value></LookupValue>
        </LookupValues></LookupValueSearch>"#;
        let p = parse_products(xml);
        assert_eq!(p.len(), 3);
        assert_eq!(product_id("NVIDIA GeForce RTX 4060 Ti", &p).as_deref(), Some("1022"));
        assert_eq!(product_id("NVIDIA GeForce RTX 4060", &p).as_deref(), Some("1023"));
        assert_eq!(product_id("NVIDIA GeForce RTX 4060 Laptop GPU", &p).as_deref(), Some("1007"));
        assert_eq!(product_id("NVIDIA GeForce RTX 9999", &p), None);
    }

    #[test]
    fn the_lookup_gives_version_download_size_and_date() {
        let json = r#"{"Success":"1","IDS":[{"downloadInfo":{"Name":"GeForce%20Game%20Ready%20Driver","Version":"621.01","ReleaseDateTime":"Wed Oct 21, 2026","DetailsURL":"https://www.nvidia.com/en-gb/drivers/details/282478/","DownloadURL":"https://uk.download.nvidia.com/Windows/621.01/621.01-desktop-win10-win11-64bit-international-dch-whql.exe","DownloadURLFileSize":"1.04 GB","ReleaseNotes":"%09%3Cb%3EGame%20Ready%20for%20Phantom%20Blade%20Zero%3C/b%3E%3Cbr%20/%3E%3Cb%3EFixed%20Gaming%20Bugs%3C/b%3E%3Cul%3E%3Cli%3EStar%20Citizen%3A%20may%20crash%20with%20Vulkan%3C/li%3E%3C/ul%3E"}}]}"#;
        let r = parse_lookup(json).unwrap();
        assert!(r.notes.iter().any(|n| n.contains("Star Citizen: may crash with Vulkan")), "{:?}", r.notes);
        assert_eq!(r.version, "621.01");
        assert_eq!(r.name, "GeForce Game Ready Driver");
        assert_eq!(r.date.as_deref(), Some("2026.10.21"));
        // Rounded: only to show. The download must not be checked against it.
        assert_eq!((r.size, r.approx_size), (None, Some((1.04 * 1024.0 * 1024.0 * 1024.0) as u64)));
        assert!(parse_lookup(r#"{"Success":"0"}"#).is_err());
        assert!(parse_lookup(&json.replace("uk.download.nvidia.com", "evil.example.com")).is_err());
        // The name of NVIDIA's server only somewhere in the address is not NVIDIA's server.
        assert!(parse_lookup(&json.replace("https://uk.download.nvidia.com/", "https://evil.example.com/x/.download.nvidia.com/")).is_err());
        assert!(parse_lookup(&json.replace("https://uk.download.nvidia.com/", "https://uk.download.nvidia.com.evil.example.com/")).is_err());
        assert!(parse_lookup(&json.replace("https://uk.download.nvidia.com/", "http://uk.download.nvidia.com/")).is_err());
        // A release notes link that is not NVIDIA's is not opened.
        assert_eq!(r.notes_url.as_deref(), Some("https://www.nvidia.com/en-gb/drivers/details/282478/"));
        let odd = parse_lookup(&json.replace("https://www.nvidia.com/en-gb/drivers/details/282478/", "https://evil.example.com/nvidia.com/")).unwrap();
        assert_eq!(odd.notes_url, None);
    }

    #[test]
    fn the_install_is_signed_by_nvidia_and_silent() {
        let r = parse_lookup(r#"{"Success":1,"IDS":[{"downloadInfo":{"Version":"621.01","DownloadURL":"https://us.download.nvidia.com/Windows/621.01/621.01-desktop-win10-win11-64bit-international-dch-whql.exe"}}]}"#).unwrap();
        let plan = plan_install("driver:nvidia:1022", "NVIDIA GeForce RTX 4060 Ti", &Release { approx_size: Some(1_116_691_496), ..r.clone() }).unwrap();
        assert_eq!(plan.downloads[0].signer.as_deref(), Some(SIGNER));
        assert_eq!(plan.downloads[0].size, None, "a rounded size would fail the exact check");
        let plan = plan_install("driver:nvidia:1022", "x", &Release { size: Some(1_037_190_000), ..r }).unwrap();
        assert_eq!(plan.downloads[0].size, Some(1_037_190_000));
        let Step::Run { args, elevated, ok_codes, .. } = &plan.steps[0] else { panic!() };
        assert!(*elevated && args.contains(&"-s".to_string()) && ok_codes.contains(&0));
    }

    #[test]
    fn only_nvidia_display_adapters_count() {
        let gpu = Present { matching_id: "PCI\\VEN_10DE&DEV_2803".into(), class: "Display".into(), ..Default::default() };
        let audio = Present { matching_id: "HDAUDIO\\FUNC_01&VEN_10DE".into(), class: "MEDIA".into(), ..Default::default() };
        let intel = Present { matching_id: "PCI\\VEN_8086&DEV_9A49".into(), class: "Display".into(), ..Default::default() };
        assert!(is_gpu(&gpu) && !is_gpu(&audio) && !is_gpu(&intel));
    }
}
