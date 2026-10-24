//! Gigabyte boards: the newest BIOS of each board from the list Gigabyte Control Center reads, on
//! Gigabyte's download server (a UTF-16 ini, one section per board).

use super::{Answer, Board, Listed};
use anyhow::{anyhow, Result};
use std::time::Duration;

const LIST: &str = "https://mb.download.gigabyte.com/FileList/Swhttp/DriverUpd/BIOS/ver2.ini";

/// Gigabyte's BIOS list: each board's name ("B850 AORUS ELITE WIFI7 ICE(rev.1.x)") and newest
/// BIOS ("F11").
pub fn parse(bytes: &[u8]) -> Vec<(String, String)> {
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
pub fn latest<'a>(product: &str, list: &'a [(String, String)]) -> Result<(&'a str, &'a str), &'static str> {
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
pub fn newer(latest: &str, have: &str) -> Option<bool> {
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
pub fn page(name: &str, product: &str) -> String {
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
pub async fn fetch() -> Result<Vec<(String, String)>> {
    let f = crate::http::get_cached(LIST, &[], Duration::from_secs(12 * 3600)).await?;
    let list = parse(&f.body);
    if list.is_empty() {
        return Err(anyhow!("Gigabyte's BIOS list is empty"));
    }
    Ok(list)
}

/// The answer for this board from Gigabyte's list (None: Gigabyte was not reached).
pub fn answer(b: &Board, list: Option<&[(String, String)]>) -> Answer {
    let Some(list) = list else { return Answer::missing("Could not reach Gigabyte") };
    match latest(&b.product, list) {
        Ok((name, v)) => Answer::Listed(Listed { version: v.to_string(), page: Some(page(name, &b.product)), date: None, notes: vec![], newer: newer(v, &b.bios) }),
        Err(why) => Answer::Missing { why: why.into(), page: Some(page("", &b.product)) },
    }
}

/// Gigabyte's driver list (Control Center's "DriverTable") is one list for whole chipset
/// families with no per-board download, and its versions lag behind the chip makers' own: the
/// board's devices get their drivers from Windows Update, AMD, Intel and NVIDIA instead.
pub async fn drivers(_b: &Board) -> Vec<super::MakerDriver> {
    vec![]
}

pub async fn check(b: &Board) -> Answer {
    let list = fetch().await.map_err(|e| log::warn!("gigabyte bios list: {e:#}")).ok();
    answer(b, list.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppStatus;
    use crate::sources::board::{entry, Bios, Maker};

    fn utf16(s: &str) -> Vec<u8> {
        let mut b = vec![0xFF, 0xFE];
        for u in s.encode_utf16() {
            b.extend(u.to_le_bytes());
        }
        b
    }

    const LIST: &str = "[8ARPL308]\r\nNAME=B850 AORUS ELITE WIFI7 ICE(rev.1.x)\r\n0=AMD B850, AMI BIOS , Ver.F11\r\n; Update time 2026-05-21 17:36:23\r\nBIOSVer0=F11\r\n\r\n[8ARPL331]\r\nNAME=B850 AORUS ELITE WIFI7(rev.1.x)\r\nBIOSVer0=F13\r\n\r\n[8ARPL005]\r\nNAME=X670 AORUS ELITE AX(rev.1.0x/1.1x/1.2x)\r\nBIOSVer0=F41\r\n[8ARPL006]\r\nNAME=X670 AORUS ELITE AX(rev.1.3)\r\nBIOSVer0=F43\r\n";

    fn board(product: &str, bios: &str) -> Board {
        crate::sources::board::tests::board("Gigabyte Technology Co., Ltd.", product, bios)
    }

    #[test]
    fn gigabytes_list_gives_each_boards_newest_bios() {
        let list = parse(&utf16(LIST));
        assert_eq!(list.len(), 4);
        assert_eq!(latest("B850 AORUS ELITE WIFI7 ICE", &list), Ok(("B850 AORUS ELITE WIFI7 ICE(rev.1.x)", "F11")));
        // "WIFI7" is not "WIFI7 ICE".
        assert_eq!(latest("B850 AORUS ELITE WIFI7", &list).unwrap().1, "F13");
        assert!(latest("X670 AORUS ELITE AX", &list).is_err(), "two revisions, two BIOSes");
        assert!(latest("Z999", &list).is_err());
        assert_eq!(page("B850 AORUS ELITE WIFI7 ICE(rev.1.x)", ""), "https://www.gigabyte.com/Motherboard/B850-AORUS-ELITE-WIFI7-ICE-rev-1x/support#support-dl-bios");
        assert_eq!(page("X670 AORUS ELITE AX(rev.1.0x/1.1x/1.2x)", "X670 AORUS ELITE AX"), "https://www.gigabyte.com/Search?kw=X670+AORUS+ELITE+AX");
    }

    #[test]
    fn bios_versions_compare_by_their_number() {
        assert_eq!(newer("F13", "F12a"), Some(true));
        // Seen on a B850 board: F12a installed while Gigabyte's list still says F11.
        assert_eq!(newer("F11", "F12a"), Some(false));
        assert_eq!(newer("F12", "F12a"), Some(false));
        assert_eq!(newer("F10", "F9"), Some(true));
        assert_eq!(newer("FC2", "F40"), None);
        assert_eq!(newer("1.20", "F4"), None);
    }

    #[test]
    fn a_gigabyte_board_gets_its_entry_from_the_list() {
        let list = parse(&utf16(LIST));
        let bios = |b: Board, l: Option<&[(String, String)]>| Bios { answer: answer(&b, l), board: b, maker: Some(Maker::Gigabyte) };
        let cur = entry(&bios(board("B850 AORUS ELITE WIFI7 ICE", "F12a"), Some(&list)));
        assert_eq!((cur.status, cur.version.as_str(), cur.latest.as_deref()), (AppStatus::Current, "F12a", Some("F12a")));
        let old = entry(&bios(board("B850 AORUS ELITE WIFI7", "F10"), Some(&list)));
        assert_eq!((old.status, old.latest.as_deref()), (AppStatus::Update, Some("F13")));
        assert!(old.vendor_url.as_deref().is_some_and(|u| u.starts_with("https://www.gigabyte.com/Motherboard/B850-AORUS-ELITE-WIFI7-rev-1x/")));
        assert!(old.note.unwrap().contains("Q-Flash"));
        let off = entry(&bios(board("B850 AORUS ELITE WIFI7 ICE", "F12a"), None));
        assert_eq!(off.status, AppStatus::Unknown);
    }
}
