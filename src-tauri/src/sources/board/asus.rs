//! ASUS boards and PCs (its laptops too): the BIOS and driver lists ASUS's own support pages read,
//! from ASUS's support web API. Only the model name is sent ("ROG STRIX B650E-F GAMING WIFI", a
//! laptop's "G513QM"). Beta versions ("Beta Version" on ASUS's page) are left out.

use super::{Answer, Board, Listed, MakerDriver};
use anyhow::Result;
use serde_json::Value;
use std::time::Duration;

const API: &str = "https://www.asus.com/support/webapi/ProductV2";
const OS_LIST: &str = "https://www.asus.com/support/api/product.asmx/GetPDOS";
/// ASUS's download center, for a model ASUS does not list under the name Windows gives it.
const SEARCH: &str = "https://www.asus.com/support/download-center/";
const KEEP: Duration = Duration::from_secs(12 * 3600);

/// A file in one of ASUS's lists: a BIOS, a driver or a tool.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct File {
    /// "3881", a laptop's "335", a driver's "V10.049.0413.2021"
    pub version: String,
    pub title: String,
    /// HTML
    pub description: String,
    /// "2026/06/29"
    pub date: String,
    /// false: a beta
    pub release: bool,
    /// The devices a driver is for, each with the driver's own version: ("PCI\VEN_10EC&DEV_8125", "10.049.0413.2021")
    pub hardware: Vec<(String, String)>,
}

/// ASUS's answer for a model: the model as ASUS writes it and its files by group ("BIOS", "LAN",
/// "Networking"). Empty when ASUS does not know the model.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listing {
    pub model: String,
    pub groups: Vec<(String, Vec<File>)>,
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

/// An answer of GetPDBIOS or GetPDDrivers. A model ASUS does not know comes back as "FAIL".
pub fn parse(bytes: &[u8]) -> Result<Listing> {
    let v: Value = serde_json::from_slice(bytes)?;
    if !text(&v["Status"]).eq_ignore_ascii_case("success") {
        return Ok(Listing::default());
    }
    let r = &v["Result"];
    let file = |f: &Value| File {
        version: text(&f["Version"]),
        title: text(&f["Title"]),
        description: text(&f["Description"]),
        date: text(&f["ReleaseDate"]),
        release: text(&f["IsRelease"]) != "0",
        hardware: f["HardwareInfoList"].as_array().map(|h| h.iter().map(|x| (text(&x["hardwareid"]), text(&x["version"]))).filter(|(id, _)| !id.is_empty()).collect()).unwrap_or_default(),
    };
    let groups = r["Obj"].as_array().map(|a| a.iter().map(|g| (text(&g["Name"]), g["Files"].as_array().map(|fs| fs.iter().map(file).collect()).unwrap_or_default())).collect()).unwrap_or_default();
    Ok(Listing { model: text(&r["Model"]), groups })
}

/// The id of the Windows 11 64-bit list (Windows 10 64-bit for older models) in GetPDOS's answer.
pub fn os_id(bytes: &[u8]) -> Option<String> {
    let v: Value = serde_json::from_slice(bytes).ok()?;
    let list = v["Result"]["Obj"].as_array()?;
    ["Windows 11 64-bit", "Windows 10 64-bit"].iter().find_map(|want| list.iter().find(|o| text(&o["Name"]).eq_ignore_ascii_case(want)).map(|o| text(&o["Id"]))).filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()))
}

/// The same model, written another way ("rog strix b650e-f gaming wifi").
fn same(a: &str, b: &str) -> bool {
    let key = |s: &str| s.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect::<String>();
    !key(a).is_empty() && key(a) == key(b)
}

/// The model as ASUS's pages ask for it: "rog+strix+b650e-f+gaming+wifi".
fn query(product: &str) -> String {
    url::form_urlencoded::byte_serialize(product.trim().to_lowercase().as_bytes()).collect()
}

/// The model's page on ASUS's site: tab "helpdesk_bios" or "helpdesk_download".
pub fn page(product: &str, tab: &str) -> String {
    let path: String = product.trim().to_lowercase().bytes().map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
    format!("https://www.asus.com/supportonly/{path}/{tab}/")
}

/// "2026/06/29" as "2026.06.29".
fn date(d: &str) -> Option<String> {
    chrono::NaiveDate::parse_from_str(d.trim(), "%Y/%m/%d").ok().map(|d| d.format("%Y.%m.%d").to_string())
}

/// ASUS's change text as note lines. Its "Label :" lines are headings; the help links under
/// "Introduction", "How to Install" and "FAQ" are left out. A laptop BIOS says what changed under
/// "Important Information" (the rest is the same on every BIOS): then only that is kept.
pub fn notes(html: &str) -> Vec<String> {
    fn unnumbered(l: &str) -> &str {
        let l = l.trim_start_matches('\u{2022}').trim_start();
        let d = l.chars().take_while(char::is_ascii_digit).count();
        match l[d..].strip_prefix(['.', ')']) {
            Some(rest) if (1..=2).contains(&d) && !rest.starts_with(|c: char| c.is_ascii_digit()) => rest.trim_start(),
            _ => l,
        }
    }
    let mut sections: Vec<(String, Vec<String>)> = vec![(String::new(), vec![])];
    for line in crate::sources::drivers::html_notes(html.trim().trim_matches('"')) {
        let line = line.trim().trim_matches('"').trim();
        let head = line.strip_prefix("### ").map(|h| h.trim_end_matches(':').trim()).or_else(|| line.strip_suffix(':').map(str::trim).filter(|h| !h.is_empty() && h.chars().count() <= 40 && !h.contains(['.', ':'])));
        match head {
            Some(h) => sections.push((h.to_string(), vec![])),
            None => {
                let l = unnumbered(line);
                if !l.is_empty() {
                    sections.last_mut().expect("a section").1.push(l.to_string());
                }
            }
        }
    }
    sections.retain(|(h, lines)| !lines.is_empty() && !["introduction", "how to install", "faq"].contains(&h.to_ascii_lowercase().as_str()));
    if let Some(i) = sections.iter().position(|(h, _)| h.eq_ignore_ascii_case("important information")) {
        return sections.swap_remove(i).1;
    }
    let mut out = Vec::new();
    for (h, lines) in sections {
        if !h.is_empty() && !h.eq_ignore_ascii_case("description") {
            out.push(format!("### {h}"));
        }
        out.extend(lines);
    }
    out
}

/// The newest BIOS (not a beta) ASUS lists, in the list for EZ Flash ("BIOS"; "BIOS Update
/// (Windows)" is ASUS's Windows flasher).
pub fn latest(list: &Listing) -> Option<&File> {
    list.groups.iter().filter(|(n, _)| n.eq_ignore_ascii_case("BIOS")).flat_map(|(_, f)| f).filter(|f| f.release).max_by(|a, b| a.date.cmp(&b.date).then_with(|| crate::version::cmp(&a.version, &b.version)))
}

/// ASUS BIOS versions: a board's "3881", a laptop's "335" (installed as "G513QM.335"). Is `latest`
/// newer than `have`? None when the two do not compare.
pub fn newer(latest: &str, have: &str) -> Option<bool> {
    fn number(v: &str) -> Option<u32> {
        let v = v.trim();
        let n = match v.rsplit_once('.') {
            Some((model, n)) if model.chars().any(|c| c.is_ascii_alphabetic()) => n,
            Some(_) => return None,
            None => v,
        };
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        n.parse().ok()
    }
    Some(number(latest)? > number(have)?)
}

/// The answer for this board or PC from ASUS's BIOS list (None: ASUS was not reached).
pub fn answer(b: &Board, list: Option<&Listing>) -> Answer {
    let Some(list) = list else { return Answer::missing("Could not reach ASUS") };
    if !same(&list.model, &b.product) {
        return Answer::Missing { why: "ASUS does not list this board".into(), page: Some(SEARCH.into()) };
    }
    let page = page(&b.product, "helpdesk_bios");
    match latest(list) {
        Some(f) => Answer::Listed(Listed { version: f.version.clone(), page: Some(page), date: date(&f.date), notes: notes(&f.description), newer: newer(&f.version, &b.bios) }),
        None => Answer::Missing { why: "ASUS lists no BIOS for this board".into(), page: Some(page) },
    }
}

/// ASUS's group names as the app's kinds.
fn kind(group: &str) -> String {
    let g = group.to_ascii_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| g.contains(w));
    let word = |w: &str| g.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == w);
    let k = if has(&["bluetooth"]) {
        "Bluetooth"
    } else if word("lan") || has(&["network", "wireless", "wi-fi", "wifi"]) {
        "Network"
    } else if has(&["audio"]) {
        "Audio"
    } else if has(&["chipset"]) {
        "Chipset"
    } else if has(&["vga", "graphic"]) {
        "Graphics"
    } else if has(&["sata", "raid", "storage"]) {
        "Storage"
    } else if has(&["pointing", "keyboard", "touchpad"]) {
        "Input"
    } else if has(&["camera"]) {
        "Camera"
    } else if has(&["firmware"]) {
        "Firmware"
    } else if has(&["utilit", "software"]) {
        "Utility"
    } else {
        return group.trim().to_string();
    };
    k.into()
}

/// A driver's name without its version and Windows ("Intel I225/I226 LAN driver v2.1.5.7 for
/// Windows 11 64-bit." is "intel i225/i226 lan driver"): the versions of one driver share it.
fn title_key(t: &str) -> String {
    let t = t.to_lowercase();
    let t = t.split(" for win").next().unwrap_or(&t);
    t.split_whitespace().filter(|w| !(w.trim_start_matches('v').starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))).collect::<Vec<_>>().join(" ")
}

fn slug(s: &str) -> String {
    s.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).collect::<Vec<_>>().join("-")
}

/// The fewest hardware ids that match the same devices: "PCI\VEN_10EC&DEV_8125&SUBSYS_..." is
/// left out when "PCI\VEN_10EC&DEV_8125" is listed (ASUS lists thousands per driver).
fn fewest<'a>(ids: impl Iterator<Item = &'a str>) -> Vec<String> {
    let mut all: Vec<String> = ids.map(|s| s.trim().to_ascii_uppercase()).filter(|s| !s.is_empty()).collect();
    all.sort_by(|a, b| a.matches('&').count().cmp(&b.matches('&').count()).then_with(|| a.cmp(b)));
    all.dedup();
    let mut kept = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in all {
        if !id.match_indices('&').any(|(i, _)| kept.contains(&id[..i])) {
            kept.insert(id.clone());
            out.push(id);
        }
    }
    out
}

/// The newest of each driver and tool in ASUS's list for this model, for the devices ASUS names.
/// ASUS names devices (with each driver's own version) for its laptops, not for its boards: a
/// file without them could not be matched with this PC, so it is left out.
pub fn driver_list(product: &str, list: &Listing) -> Vec<MakerDriver> {
    if !same(&list.model, product) {
        return vec![];
    }
    let mut newest: Vec<(String, &str, &File)> = Vec::new();
    for (group, files) in &list.groups {
        if group.to_ascii_lowercase().contains("bios") {
            continue;
        }
        for f in files.iter().filter(|f| f.release && !f.hardware.is_empty()) {
            let key = format!("{}-{}", slug(group), slug(&title_key(&f.title)));
            let order = |x: &File| (crate::version::parts(&x.version), x.date.clone());
            match newest.iter_mut().find(|(k, ..)| *k == key) {
                Some(n) if order(f) > order(n.2) => n.2 = f,
                Some(_) => {}
                None => newest.push((key, group, f)),
            }
        }
    }
    let page = page(product, "helpdesk_download");
    let mut out = Vec::new();
    for (key, group, f) in newest {
        // The driver's own version for each device (one package can hold drivers of several versions).
        let own = |v: &str| if v.starts_with(|c: char| c.is_ascii_digit()) { v.to_string() } else { f.version.trim_start_matches(['v', 'V']).to_string() };
        let mut versions: Vec<String> = Vec::new();
        for (_, v) in &f.hardware {
            let v = own(v);
            if v.starts_with(|c: char| c.is_ascii_digit()) && !versions.contains(&v) {
                versions.push(v);
            }
        }
        let n = versions.len();
        for (i, v) in versions.into_iter().enumerate() {
            let ids = fewest(f.hardware.iter().filter(|(_, hv)| own(hv) == v).map(|(id, _)| id.as_str()));
            out.push(MakerDriver {
                id: if n > 1 { format!("{key}-{}", i + 1) } else { key.clone() },
                name: f.title.trim().trim_end_matches('.').to_string(),
                kind: kind(group),
                version: v,
                date: date(&f.date),
                page: Some(page.clone()),
                notes: notes(&f.description),
                hardware_ids: ids,
                installed: None,
                // ASUS gives only a rounded size ("17.25 MB").
                size: None,
                // ASUS gives a checksum and a silent command, but the signature its packages must
                // have is not confirmed: they are installed from ASUS's page.
                install: None,
            });
        }
    }
    out
}

async fn get(url: &str) -> Result<Listing> {
    let f = crate::http::get_cached(url, &[], KEEP).await?;
    parse(&f.body)
}

/// ASUS's BIOS list for the model, kept for twelve hours.
pub async fn fetch_bios(product: &str) -> Result<Listing> {
    get(&format!("{API}/GetPDBIOS?website=global&model={}", query(product))).await
}

/// ASUS's driver and tool list for the model (Windows 11 64-bit, else Windows 10 64-bit).
pub async fn fetch_drivers(product: &str) -> Result<Listing> {
    let q = query(product);
    let os = crate::http::get_cached(&format!("{OS_LIST}?website=global&model={q}"), &[], KEEP).await?;
    let Some(os) = os_id(&os.body) else { return Ok(Listing::default()) };
    get(&format!("{API}/GetPDDrivers?website=global&model={q}&osid={os}")).await
}

pub async fn check(b: &Board) -> Answer {
    if b.product.trim().is_empty() {
        return Answer::Missing { why: "ASUS does not list this board".into(), page: Some(SEARCH.into()) };
    }
    let list = fetch_bios(&b.product).await.map_err(|e| log::warn!("asus bios list: {e:#}")).ok();
    answer(b, list.as_ref())
}

/// The drivers and tools ASUS lists for this board or PC.
pub async fn drivers(b: &Board) -> Vec<super::MakerDriver> {
    if b.product.trim().is_empty() {
        return vec![];
    }
    match fetch_drivers(&b.product).await {
        Ok(list) => driver_list(&b.product, &list),
        Err(e) => {
            log::warn!("asus driver list: {e:#}");
            vec![]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AppStatus;
    use crate::sources::board::{entry, Bios, Maker};

    // Trimmed from ASUS's answers for public models.
    const BOARD_BIOS: &str = r#"{"Status":"SUCCESS","Message":"SUCCESS","Result":{"Count":35,"Model":"ROG STRIX B650E-F GAMING WIFI","Obj":[{"LangOptions":null,"Name":"BIOS","Count":1,"Files":[
        {"Id":"1@66@2@2@-1@3@ROG-STRIX-B650E-F-GAMING-WIFI-ASUS-4004.ZIP","Version":"4004","Title":"","Description":"\"1. Update to AGESA ComboAM5 PI 1.3.0.1d.<br/>2. Enhanced memory performance.\"","FileSize":"17.25 MB","ReleaseDate":"2026/09/21","IsRelease":"0","HardwareInfoList":null},
        {"Id":"1@66@2@2@-1@3@ROG-STRIX-B650E-F-GAMING-WIFI-ASUS-3886.ZIP","Version":"3886","Title":"","Description":"\"Updated AGESA ComboAM5 PI 1.3.0.1b Patch A.\"","FileSize":"17.23 MB","ReleaseDate":"2026/07/02","IsRelease":"0","HardwareInfoList":null},
        {"Id":"1@66@2@2@-1@3@ROG-STRIX-B650E-F-GAMING-WIFI-ASUS-3881.ZIP","Version":"3881","Title":"","Description":"\"1.Update AGESA ComboAM5 PI 1.3.0.1b.<br/>2.Add support for EXPO ULL memory profiles.<br/>3.Optimize ECC-UDIMM memory performance.<br/>Before  running  the USB  BIOS  Flashback  tool, please  rename  the  BIOS  file (SB650EF.CAP)  using  BIOSRenamer.\"","FileSize":"17.23 MB","ReleaseDate":"2026/06/29","IsRelease":"1","HardwareInfoList":null},
        {"Id":"1@66@2@2@-1@3@ROG-STRIX-B650E-F-GAMING-WIFI-ASUS-3854.ZIP","Version":"3854","Title":"","Description":"• Update AGESA ComboAM5 PI 1.3.0.1.","FileSize":"17.2 MB","ReleaseDate":"2026/04/23","IsRelease":"1","HardwareInfoList":null}]},
        {"LangOptions":null,"Name":"Firmware","Count":1,"Files":[{"Id":"1@66@2@2@-1@20@USB_Audio_FW_Update_Tool_.zip","Version":"-","Title":"Audio ALC4080/ALC4082 FW update tool","Description":"Fixed SPDIF playing music is noisy","ReleaseDate":"2023/02/23","IsRelease":"1","HardwareInfoList":null}]}]}}"#;

    const LAPTOP_BIOS: &str = r#"{"Status":"SUCCESS","Message":"SUCCESS","Result":{"Count":5,"Model":"G513QM","Obj":[
        {"Name":"BIOS Update (Windows)","Count":1,"Files":[{"Id":"56@112@0@7@-1@62@ASUS_G513QM_331_BIOS_Update_3.exe","Version":"331","Title":"BIOS Update for Windows","Description":"","ReleaseDate":"2023/03/30","IsRelease":"1"}]},
        {"Name":"BIOS","Count":1,"Files":[
        {"Id":"56@112@0@7@-1@3@G513QMAS335.zip","Version":"335","Title":"BIOS for ASUS EZ Flash Utility","Description":"Description :<br/>It is recommended to download and update the BIOS and firmware for system performance optimization.<br/><br/>Important Information:<br/>Improvements to overall system responsiveness<br/><br/>Introduction :<br/><a href=\"https://www.asus.com/support/FAQ/1047740\" target=\"_blank\">- MyASUS in UEFI BIOS - Introduction</a><br/><br/>How to Install :<br/><a href=\"https://www.asus.com/support/FAQ/1008859\" target=\"_blank\">- How to update BIOS with EZ Flash</a><br/>","ReleaseDate":"2025/12/24","IsRelease":"1"},
        {"Id":"56@112@0@7@-1@3@G513QMAS331.zip","Version":"331","Title":"BIOS for ASUS EZ Flash Utility","Description":"","ReleaseDate":"2023/03/30","IsRelease":"1"}]}]}}"#;

    const UNKNOWN: &str = r#"{"Status":"FAIL","Message":"Input string was not in a correct format.","Result":null}"#;

    const OS: &str = r#"{"Result":{"Obj":[{"Id":"52","Name":"Windows 11 64-bit"},{"Id":"45","Name":"Windows 10 64-bit"},{"Id":"8","Name":"Others"}]},"Status":"SUCCESS","Message":"SUCCESS"}"#;

    const LAPTOP_DRIVERS: &str = r#"{"Status":"SUCCESS","Message":"SUCCESS","Result":{"Count":28,"Model":"G513QM","Obj":[
        {"Name":"Networking","Count":3,"Files":[
        {"Id":"56@112@0@7@52@67@LAN_ROG_Realtek_J_V10.049.0413.2021_23045_2.exe","Version":"V10.049.0413.2021","Title":"Realtek LAN Driver","Description":"<br/>Description :<br/>Realtek LAN Driver<br/><br>Minimum OS Version :<br/>Windows 11 64-bit 21H2<br/><br/>Device List :<br/>RTL8111H<br/><br/>FAQ :<br/><a href=\"https://www.asus.com/support/FAQ/1045091\">- [Notebook] Laptop Frequently Asked Question (FAQ)</a><br/>","ReleaseDate":"2021/11/03","IsRelease":"1","HardwareInfoList":[{"hardwareid":"PCI\\VEN_10EC&DEV_8125","date":"04-13-2021","version":"10.049.0413.2021"},{"hardwareid":"PCI\\VEN_10EC&DEV_8125&SUBSYS_86771043","date":"04-13-2021","version":"10.049.0413.2021"},{"hardwareid":"PCI\\VEN_10EC&DEV_8168","date":"04-13-2021","version":"10.049.0413.2021"}],"ExeModule":"LAN_ROG_Realtek_J_V10.049.0413.2021_23045_2.exe%%/SUPPRESSMSGBOXES /VERYSILENT /NORESTART","sha256":"085DFAA622FCD40C54AB90CBCCA035D1C0BA69313ED885C21FFC9B3F7EEF064F"},
        {"Id":"56@112@0@7@52@67@LAN_ROG_Realtek_J_V10.045.0928.2020_21445_2.exe","Version":"V10.045.0928.2020","Title":"Realtek LAN Driver","Description":"","ReleaseDate":"2021/11/03","IsRelease":"1","HardwareInfoList":[{"hardwareid":"PCI\\VEN_10EC&DEV_8125","date":"09-28-2020","version":"10.045.0928.2020"}]},
        {"Id":"56@112@0@7@52@67@WirelessLan_ROG_MediaTek_J_V3.00.01.1400_40000_1.exe","Version":"V3.00.01.1400","Title":"MediaTek WLAN Driver","Description":"","ReleaseDate":"2026/01/10","IsRelease":"0","HardwareInfoList":[{"hardwareid":"PCI\\VEN_14C3&DEV_7961","date":"01-01-2026","version":"3.00.01.1400"}]},
        {"Id":"56@112@0@7@52@67@WirelessLan_ROG_MediaTek_J_V3.00.01.1325Sub1_38028_1.exe","Version":"V3.00.01.1325","Title":"MediaTek WLAN Driver","Description":"","ReleaseDate":"2024/06/06","IsRelease":"1","HardwareInfoList":[{"hardwareid":"PCI\\VEN_14C3&DEV_7961&SUBSYS_E0B61A3B","date":"04-18-2024","version":"3.00.01.1325"},{"hardwareid":"PCI\\VEN_14C3&DEV_7961&SUBSYS_E0B61A3B","date":"04-18-2024","version":"3.00.01.1325"}]}]},
        {"Name":"Audio","Count":2,"Files":[
        {"Id":"56@112@0@7@52@4@Realtek Audio HSA","Version":"latest version at the MS store","Title":"Realtek Codec Console Application","Description":"","ReleaseDate":"2021/07/08","IsRelease":"1","HardwareInfoList":[]},
        {"Id":"56@112@0@7@52@4@Audio_DriverOnly_Dolby_ROG_Realtek_J_V6.0.9394.1_30490_1.exe","Version":"V6.0.9394.1","Title":"Realtek Audio","Description":"","ReleaseDate":"2022/10/04","IsRelease":"1","HardwareInfoList":[{"hardwareid":"HDAUDIO\\FUNC_01&VEN_10EC&DEV_0294&SUBSYS_10431F11","date":"08-09-2022","version":"6.0.9394.1"},{"hardwareid":"INTELAUDIO\\FUNC_01&VEN_10EC&DEV_0294","date":"08-09-2022","version":"6.0.9394.2"}]}]},
        {"Name":"Software and Utility","Count":1,"Files":[
        {"Id":"56@112@0@7@52@14@SUSSmartDisplayControl_ASUS_Z_V1.10.13_14430_5.exe","Version":"V1.10.13","Title":"ASUS Smart Display Control","Description":"","ReleaseDate":"2022/08/08","IsRelease":"1","HardwareInfoList":null}]}]}}"#;

    const BOARD_DRIVERS: &str = r#"{"Status":"SUCCESS","Message":"SUCCESS","Result":{"Count":79,"Model":"ROG STRIX B650E-F GAMING WIFI","Obj":[
        {"Name":"LAN","Count":1,"Files":[{"Id":"1@66@2@2@52@5@DRV_LAN_Intel_I225_I226_SZ-TSD_W11_64_V2157_20251128R.zip","Version":"2.1.5.7","Title":"Intel I225/I226 LAN driver v2.1.5.7 for Windows 11 64-bit.","Description":"","ReleaseDate":"2025/12/31","IsRelease":"1","HardwareInfoList":[],"SWID":"","ExeModule":""}]}]}}"#;

    fn board(product: &str, bios: &str) -> Board {
        crate::sources::board::tests::board("ASUSTeK COMPUTER INC.", product, bios)
    }

    #[test]
    fn asus_lists_give_the_newest_bios_that_is_not_a_beta() {
        let list = parse(BOARD_BIOS.as_bytes()).unwrap();
        assert_eq!((list.model.as_str(), list.groups.len()), ("ROG STRIX B650E-F GAMING WIFI", 2));
        // 4004 and 3886 are betas.
        assert_eq!(latest(&list).map(|f| f.version.as_str()), Some("3881"));
        let lap = parse(LAPTOP_BIOS.as_bytes()).unwrap();
        // The EZ Flash list, not the Windows flasher's.
        assert_eq!(latest(&lap).map(|f| f.version.as_str()), Some("335"));
        let none = parse(UNKNOWN.as_bytes()).unwrap();
        assert_eq!(none, Listing::default());
        assert!(parse(b"<html>blocked</html>").is_err());
        assert_eq!(os_id(OS.as_bytes()).as_deref(), Some("52"));
        assert_eq!(os_id(br#"{"Result":{"Obj":[{"Id":"45","Name":"Windows 10 64-bit"}]}}"#).as_deref(), Some("45"));
        assert_eq!(os_id(br#"{"Result":{"Obj":[{"Id":"8","Name":"Others"}]}}"#), None);
    }

    #[test]
    fn models_are_asked_and_linked_by_their_name() {
        assert_eq!(query("ROG STRIX B650E-F GAMING WIFI"), "rog+strix+b650e-f+gaming+wifi");
        assert_eq!(query("ROG STRIX B550-F GAMING (WI-FI)"), "rog+strix+b550-f+gaming+%28wi-fi%29");
        assert_eq!(page("ROG STRIX B550-F GAMING (WI-FI)", "helpdesk_bios"), "https://www.asus.com/supportonly/rog%20strix%20b550-f%20gaming%20%28wi-fi%29/helpdesk_bios/");
        assert_eq!(page("G513QM", "helpdesk_download"), "https://www.asus.com/supportonly/g513qm/helpdesk_download/");
        assert!(same("ROG STRIX B650E-F GAMING WIFI", "rog strix b650e-f gaming wifi"));
        assert!(!same("ROG STRIX B650E-F GAMING WIFI", "ROG STRIX B650E-E GAMING WIFI"));
        assert!(!same("", ""));
    }

    #[test]
    fn asus_bios_versions_compare_by_their_number() {
        assert_eq!(newer("3881", "3040"), Some(true));
        // A beta installed is newer than the newest release.
        assert_eq!(newer("3881", "4004"), Some(false));
        assert_eq!(newer("3881", "3881"), Some(false));
        // Laptops: installed as "model.number".
        assert_eq!(newer("335", "G513QM.331"), Some(true));
        assert_eq!(newer("335", "G513QM.335"), Some(false));
        assert_eq!(newer("1.0.3", "3040"), None);
        assert_eq!(newer("3881", "1.02"), None);
        assert_eq!(newer("-", "3040"), None);
    }

    #[test]
    fn notes_keep_what_changed() {
        let list = parse(BOARD_BIOS.as_bytes()).unwrap();
        let n = notes(&latest(&list).unwrap().description);
        assert_eq!(n[..3], ["Update AGESA ComboAM5 PI 1.3.0.1b.", "Add support for EXPO ULL memory profiles.", "Optimize ECC-UDIMM memory performance."]);
        assert_eq!(n[3], "Before running the USB BIOS Flashback tool, please rename the BIOS file (SB650EF.CAP) using BIOSRenamer.");
        assert_eq!(notes("\u{2022} Update AGESA ComboAM5 PI 1.3.0.1."), ["Update AGESA ComboAM5 PI 1.3.0.1."]);
        // A laptop BIOS: only what changed, not the text every BIOS has or the help links.
        let lap = parse(LAPTOP_BIOS.as_bytes()).unwrap();
        assert_eq!(notes(&latest(&lap).unwrap().description), ["Improvements to overall system responsiveness"]);
        let drv = parse(LAPTOP_DRIVERS.as_bytes()).unwrap();
        assert_eq!(notes(&drv.groups[0].1[0].description), ["Realtek LAN Driver", "### Minimum OS Version", "Windows 11 64-bit 21H2", "### Device List", "RTL8111H"]);
    }

    #[test]
    fn an_asus_board_gets_its_bios_entry() {
        let list = parse(BOARD_BIOS.as_bytes()).unwrap();
        let bios = |b: Board, l: Option<&Listing>| Bios { answer: answer(&b, l), board: b, maker: Some(Maker::Asus) };
        let old = entry(&bios(board("ROG STRIX B650E-F GAMING WIFI", "3040"), Some(&list)));
        assert_eq!((old.status, old.latest.as_deref(), old.date.as_deref()), (AppStatus::Update, Some("3881"), Some("2026.06.29")));
        assert_eq!(old.vendor_url.as_deref(), Some("https://www.asus.com/supportonly/rog%20strix%20b650e-f%20gaming%20wifi/helpdesk_bios/"));
        assert!(old.note.unwrap().contains("EZ Flash") && !old.notes.is_empty());
        let beta = entry(&bios(board("ROG STRIX B650E-F GAMING WIFI", "4004"), Some(&list)));
        assert_eq!((beta.status, beta.latest.as_deref()), (AppStatus::Current, Some("4004")));
        let lap = parse(LAPTOP_BIOS.as_bytes()).unwrap();
        let laptop = entry(&bios(board("G513QM", "G513QM.331"), Some(&lap)));
        assert_eq!((laptop.status, laptop.latest.as_deref()), (AppStatus::Update, Some("335")));
        // Another model's list, or none.
        let other = answer(&board("ROG STRIX B650E-E GAMING WIFI", "3040"), Some(&list));
        assert!(matches!(other, Answer::Missing { page: Some(ref p), .. } if p == SEARCH));
        assert!(matches!(answer(&board("NOPE 1", "1"), Some(&Listing::default())), Answer::Missing { .. }));
        let off = entry(&bios(board("ROG STRIX B650E-F GAMING WIFI", "3040"), None));
        assert_eq!((off.status, off.latest), (AppStatus::Unknown, None));
        assert_eq!(off.note.as_deref(), Some("Could not reach ASUS"));
    }

    #[test]
    fn asus_drivers_are_the_newest_of_each_for_the_devices_asus_names() {
        let list = parse(LAPTOP_DRIVERS.as_bytes()).unwrap();
        let d = driver_list("G513QM", &list);
        let short: Vec<(&str, &str, &str)> = d.iter().map(|d| (d.id.as_str(), d.kind.as_str(), d.version.as_str())).collect();
        assert_eq!(
            short,
            [
                ("networking-realtek-lan-driver", "Network", "10.049.0413.2021"),
                // The beta 3.00.01.1400 is left out.
                ("networking-mediatek-wlan-driver", "Network", "3.00.01.1325"),
                ("audio-realtek-audio-1", "Audio", "6.0.9394.1"),
                ("audio-realtek-audio-2", "Audio", "6.0.9394.2"),
            ]
        );
        // One id for each device family: the longer ids are the same devices.
        assert_eq!(d[0].hardware_ids, ["PCI\\VEN_10EC&DEV_8125", "PCI\\VEN_10EC&DEV_8168"]);
        assert_eq!(d[1].hardware_ids, ["PCI\\VEN_14C3&DEV_7961&SUBSYS_E0B61A3B"]);
        assert_eq!((d[0].name.as_str(), d[0].date.as_deref()), ("Realtek LAN Driver", Some("2021.11.03")));
        assert_eq!(d[0].page.as_deref(), Some("https://www.asus.com/supportonly/g513qm/helpdesk_download/"));
        assert!(d.iter().all(|d| d.install.is_none() && d.installed.is_none() && d.size.is_none()));
        assert!(crate::sources::drivers::hw_match(&d[0].hardware_ids[0], "PCI\\VEN_10EC&DEV_8125&SUBSYS_86771043&REV_05"));
        // A board's list names no devices: nothing could be matched.
        assert!(driver_list("ROG STRIX B650E-F GAMING WIFI", &parse(BOARD_DRIVERS.as_bytes()).unwrap()).is_empty());
        // Another model's list.
        assert!(driver_list("G513QR", &list).is_empty());
        assert_eq!(kind("Utilities: Armoury Crate"), "Utility");
        assert_eq!(kind("VGA Drivers"), "Graphics");
        assert_eq!(kind("SATA"), "Storage");
        assert_eq!(kind("Biometric Authentication"), "Biometric Authentication");
        assert_eq!(title_key("Intel I225/I226 LAN Driver V2.1.4.3 for Windows 11 64-bit."), "intel i225/i226 lan driver");
    }
}

#[cfg(test)]
mod live {
    /// Asks ASUS for public models: `cargo test --lib live_asus -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_asus() {
        for (product, bios) in [("ROG STRIX B650E-F GAMING WIFI", "3040"), ("G513QM", "G513QM.331")] {
            let b = crate::sources::board::tests::board("ASUSTeK COMPUTER INC.", product, bios);
            println!("{product}: {:#?}", super::check(&b).await);
            let d = super::drivers(&b).await;
            println!("{product}: {} drivers", d.len());
            for x in &d {
                println!("  {} | {} | {} | {} | {} ids", x.id, x.kind, x.name, x.version, x.hardware_ids.len());
            }
        }
    }
}
