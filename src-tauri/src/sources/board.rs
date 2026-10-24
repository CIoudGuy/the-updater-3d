//! The motherboard's BIOS: the installed version from Windows, and for Gigabyte boards the newest
//! one Gigabyte lists (the list Gigabyte Control Center reads, on Gigabyte's download server).
//! A BIOS is never flashed from here: a failed flash can leave a PC that does not start, so the
//! entry opens the board's page on Gigabyte's site (flash with Q-Flash from there).

use crate::model::{AppStatus, DriverEntry};
use anyhow::{anyhow, Result};
use std::time::Duration;

const GIGABYTE_LIST: &str = "https://mb.download.gigabyte.com/FileList/Swhttp/DriverUpd/BIOS/ver2.ini";

/// The board and its BIOS, as Windows reads them at start.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// "Gigabyte Technology Co., Ltd."
    pub maker: String,
    /// "B850 AORUS ELITE WIFI7 ICE"
    pub product: String,
    /// "F12a"
    pub bios: String,
    /// "2026.06.09"
    pub bios_date: Option<String>,
}

/// What the BIOS entry shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bios {
    pub board: Board,
    /// The newest BIOS the maker lists for this board (None: not known)
    pub latest: Option<String>,
    /// The maker's page for the board
    pub page: Option<String>,
    /// Why the newest is not known
    pub why: Option<String>,
}

/// This PC's board, from HKLM\HARDWARE\DESCRIPTION\System\BIOS.
pub fn read() -> Option<Board> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let k = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("HARDWARE\\DESCRIPTION\\System\\BIOS", KEY_READ).ok()?;
    let g = |n: &str| k.get_value::<String, _>(n).unwrap_or_default().trim().to_string();
    let product = g("BaseBoardProduct");
    let bios = g("BIOSVersion");
    if product.is_empty() || bios.is_empty() {
        return None;
    }
    let date = g("BIOSReleaseDate");
    let bios_date = chrono::NaiveDate::parse_from_str(&date, "%m/%d/%Y").ok().map(|d| d.format("%Y.%m.%d").to_string());
    Some(Board { maker: g("BaseBoardManufacturer"), product, bios, bios_date })
}

pub fn is_gigabyte(maker: &str) -> bool {
    maker.to_ascii_lowercase().starts_with("gigabyte")
}

/// Gigabyte's BIOS list (UTF-16 ini): each board's name ("B850 AORUS ELITE WIFI7 ICE(rev.1.x)")
/// and newest BIOS ("F11").
pub fn parse_gigabyte(bytes: &[u8]) -> Vec<(String, String)> {
    let text = if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let w: Vec<u16> = bytes[2..].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
        String::from_utf16_lossy(&w)
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let mut out = Vec::new();
    let mut name: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            name = None;
        } else if let Some(n) = line.strip_prefix("NAME=") {
            name = Some(n.trim().to_string());
        } else if let (Some(v), Some(n)) = (line.strip_prefix("BIOSVer0="), &name) {
            out.push((n.clone(), v.trim().to_string()));
        }
    }
    out
}

/// "B850 AORUS ELITE WIFI7 ICE(rev.1.x)" without its revision.
fn base_name(n: &str) -> &str {
    n.split("(rev").next().unwrap_or(n).trim()
}

/// The newest BIOS Gigabyte lists for this board, and the board's name in the list. Several
/// revisions of one board with different BIOSes: not known which this one is.
pub fn gigabyte_latest<'a>(product: &str, list: &'a [(String, String)]) -> Result<(&'a str, &'a str), &'static str> {
    let hits: Vec<&(String, String)> = list.iter().filter(|(n, _)| base_name(n).eq_ignore_ascii_case(product.trim())).collect();
    match hits.as_slice() {
        [] => Err("Gigabyte does not list this board"),
        [one, rest @ ..] if rest.iter().all(|h| h.1.eq_ignore_ascii_case(&one.1)) => Ok((one.0.as_str(), one.1.as_str())),
        _ => Err("Gigabyte lists several revisions of this board"),
    }
}

/// Gigabyte BIOS versions: "F11", "F12a" (a test build of F12 or a fix after it), "FC2". Is
/// `latest` a newer BIOS than `have`? Only a higher number counts (F12a and F12 are the same
/// release line); None when the two do not compare (another series).
pub fn gigabyte_newer(latest: &str, have: &str) -> Option<bool> {
    fn parts(v: &str) -> Option<(String, u32)> {
        let v = v.trim().to_ascii_uppercase();
        let rest = v.strip_prefix('F')?;
        let series: String = rest.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let num: String = rest[series.len()..].chars().take_while(|c| c.is_ascii_digit()).collect();
        Some((series, num.parse().ok()?))
    }
    let (a, b) = (parts(latest)?, parts(have)?);
    (a.0 == b.0).then_some(a.1 > b.1)
}

/// The board's page on Gigabyte's site: "B850 AORUS ELITE WIFI7 ICE(rev.1.x)" is
/// /Motherboard/B850-AORUS-ELITE-WIFI7-ICE-rev-1x/support. Revisions written another way go to
/// Gigabyte's search.
pub fn gigabyte_page(name: &str, product: &str) -> String {
    let base = base_name(name);
    let rev = name.split("(rev.").nth(1).and_then(|r| r.strip_suffix(')'));
    let slug = |s: &str| s.split_whitespace().collect::<Vec<_>>().join("-");
    match rev {
        Some(r) if r.chars().all(|c| c.is_ascii_alphanumeric() || c == '.') && !base.is_empty() => {
            format!("https://www.gigabyte.com/Motherboard/{}-rev-{}/support#support-dl-bios", slug(base), r.replace('.', ""))
        }
        _ => format!("https://www.gigabyte.com/Search?kw={}", url::form_urlencoded::byte_serialize(product.as_bytes()).collect::<String>()),
    }
}

/// Gigabyte's list, kept for twelve hours.
pub async fn fetch_gigabyte() -> Result<Vec<(String, String)>> {
    let f = crate::http::get_cached(GIGABYTE_LIST, &[], Duration::from_secs(12 * 3600)).await?;
    let list = parse_gigabyte(&f.body);
    if list.is_empty() {
        return Err(anyhow!("Gigabyte's BIOS list is empty"));
    }
    Ok(list)
}

/// The BIOS for this board: for a Gigabyte board, with the newest Gigabyte lists (`list`, None
/// when Gigabyte could not be reached).
pub fn bios(board: Board, list: Option<&[(String, String)]>) -> Bios {
    if !is_gigabyte(&board.maker) {
        return Bios { why: Some("Newer BIOS versions come from the board maker's site".into()), board, ..Default::default() };
    }
    let Some(list) = list else {
        return Bios { why: Some("Could not reach Gigabyte".into()), board, ..Default::default() };
    };
    match gigabyte_latest(&board.product, list) {
        Ok((name, v)) => Bios { page: Some(gigabyte_page(name, &board.product)), latest: Some(v.to_string()), board, why: None },
        Err(why) => Bios { page: Some(gigabyte_page("", &board.product)), why: Some(why.into()), board, latest: None },
    }
}

/// The BIOS as a Drivers entry. A newer one is an update to get from the maker's page.
pub fn entry(b: &Bios) -> DriverEntry {
    let maker = super::drivers::short_maker(&b.board.maker);
    let newer = b.latest.as_deref().and_then(|l| gigabyte_newer(l, &b.board.bios));
    let (status, note) = match (newer, &b.why) {
        (Some(true), _) => (AppStatus::Update, format!("BIOS {} is out. It is never flashed from here: download it from {maker}'s page, put it on a USB stick and flash it yourself with Q-Flash in the BIOS.", b.latest.as_deref().unwrap_or_default())),
        (Some(false), _) => (AppStatus::Current, format!("{maker} lists {} as the newest", b.latest.as_deref().unwrap_or_default())),
        (None, Some(why)) => (AppStatus::Unknown, why.clone()),
        (None, None) => (AppStatus::Unknown, "This BIOS does not compare with the one listed".into()),
    };
    DriverEntry {
        id: "driver:bios".into(),
        name: format!("{} BIOS", b.board.product),
        kind: "BIOS".into(),
        maker,
        source: if is_gigabyte(&b.board.maker) { "gigabyte" } else { "vendor" }.into(),
        version: b.board.bios.clone(),
        // Installed newer than the list (a test build): this one is the newest.
        latest: match newer {
            Some(true) => b.latest.clone(),
            Some(false) => Some(b.board.bios.clone()),
            None => None,
        },
        status,
        size: None,
        date: b.board.bios_date.clone(),
        notes_url: None,
        vendor_url: super::drivers::safe_link(b.page.clone()),
        note: Some(note),
        notes: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let mut b = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            b.extend(u.to_le_bytes());
        }
        b
    }

    const LIST: &str = "[8ARPL308]\r\nNAME=B850 AORUS ELITE WIFI7 ICE(rev.1.x)\r\n0=AMD B850, AMI BIOS , Ver.F11\r\n; Update time 2026-05-21 17:36:23\r\nBIOSVer0=F11\r\n\r\n[8ARPL331]\r\nNAME=B850 AORUS ELITE WIFI7(rev.1.x)\r\nBIOSVer0=F13\r\n\r\n[8ARPL005]\r\nNAME=X670 AORUS ELITE AX(rev.1.0x/1.1x/1.2x)\r\nBIOSVer0=F41\r\n[8ARPL006]\r\nNAME=X670 AORUS ELITE AX(rev.1.3)\r\nBIOSVer0=F43\r\n";

    fn board(product: &str, bios: &str) -> Board {
        Board { maker: "Gigabyte Technology Co., Ltd.".into(), product: product.into(), bios: bios.into(), bios_date: Some("2026.06.09".into()) }
    }

    #[test]
    fn gigabytes_list_gives_each_boards_newest_bios() {
        let list = parse_gigabyte(&utf16(LIST));
        assert_eq!(list.len(), 4);
        assert_eq!(gigabyte_latest("B850 AORUS ELITE WIFI7 ICE", &list), Ok(("B850 AORUS ELITE WIFI7 ICE(rev.1.x)", "F11")));
        // "WIFI7" is not "WIFI7 ICE".
        assert_eq!(gigabyte_latest("B850 AORUS ELITE WIFI7", &list).unwrap().1, "F13");
        assert!(gigabyte_latest("X670 AORUS ELITE AX", &list).is_err(), "two revisions, two BIOSes");
        assert!(gigabyte_latest("Z999", &list).is_err());
        assert_eq!(gigabyte_page("B850 AORUS ELITE WIFI7 ICE(rev.1.x)", ""), "https://www.gigabyte.com/Motherboard/B850-AORUS-ELITE-WIFI7-ICE-rev-1x/support#support-dl-bios");
        assert_eq!(gigabyte_page("X670 AORUS ELITE AX(rev.1.0x/1.1x/1.2x)", "X670 AORUS ELITE AX"), "https://www.gigabyte.com/Search?kw=X670+AORUS+ELITE+AX");
    }

    #[test]
    fn bios_versions_compare_by_their_number() {
        assert_eq!(gigabyte_newer("F13", "F12a"), Some(true));
        // Seen on a B850 board: F12a installed while Gigabyte's list still says F11.
        assert_eq!(gigabyte_newer("F11", "F12a"), Some(false));
        assert_eq!(gigabyte_newer("F12", "F12a"), Some(false));
        assert_eq!(gigabyte_newer("F10", "F9"), Some(true));
        assert_eq!(gigabyte_newer("FC2", "F40"), None);
        assert_eq!(gigabyte_newer("1.20", "F4"), None);
    }

    #[test]
    fn the_bios_entry_points_to_gigabyte_and_is_never_installed_here() {
        let list = parse_gigabyte(&utf16(LIST));
        let cur = entry(&bios(board("B850 AORUS ELITE WIFI7 ICE", "F12a"), Some(&list)));
        assert_eq!((cur.status, cur.version.as_str(), cur.latest.as_deref()), (AppStatus::Current, "F12a", Some("F12a")));
        let old = entry(&bios(board("B850 AORUS ELITE WIFI7", "F10"), Some(&list)));
        assert_eq!((old.status, old.latest.as_deref()), (AppStatus::Update, Some("F13")));
        assert!(old.vendor_url.as_deref().is_some_and(|u| u.starts_with("https://www.gigabyte.com/Motherboard/B850-AORUS-ELITE-WIFI7-rev-1x/")));
        assert!(old.note.unwrap().contains("Q-Flash"));
        let off = entry(&bios(board("B850 AORUS ELITE WIFI7 ICE", "F12a"), None));
        assert_eq!(off.status, AppStatus::Unknown);
        let asus = entry(&bios(Board { maker: "ASUSTeK COMPUTER INC.".into(), ..board("ROG STRIX B650E-F", "3040") }, Some(&list)));
        assert_eq!((asus.status, asus.vendor_url), (AppStatus::Unknown, None));
    }
}

#[cfg(test)]
mod live {
    /// Reads this PC's board and Gigabyte's list: `cargo test --lib live_bios -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_bios() {
        let b = super::read().expect("board");
        let list = super::fetch_gigabyte().await.ok();
        println!("{} lists; {:#?}", list.as_ref().map_or(0, |l| l.len()), super::entry(&super::bios(b, list.as_deref())));
    }
}
