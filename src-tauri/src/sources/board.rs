//! The BIOS of this PC: the installed version from Windows, and the newest one the board's or
//! PC's maker lists for it (each maker in its own module: `board/<maker>.rs`). A BIOS is never
//! flashed from here: a failed flash can leave a PC that does not start, so the entry opens the
//! maker's page for the board or PC, and the user flashes it with the maker's own tool.

use crate::model::{AppStatus, DriverEntry};

pub mod asrock;
pub mod asus;
pub mod dell;
pub mod gigabyte;
pub mod hp;
pub mod lenovo;
pub mod msi;

/// The board, the PC and its BIOS, as Windows reads them at start
/// (HKLM\HARDWARE\DESCRIPTION\System\BIOS).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Board {
    /// BaseBoardManufacturer: "Gigabyte Technology Co., Ltd.", "ASUSTeK COMPUTER INC."
    pub maker: String,
    /// BaseBoardProduct: "B850 GAMING X WIFI6E ICE"; on HP PCs the platform id ("8A78")
    pub product: String,
    /// BIOSVersion: "F12a", "3040", "1.21.0"
    pub bios: String,
    /// BIOSReleaseDate as "2026.06.09"
    pub bios_date: Option<String>,
    /// SystemManufacturer: "Dell Inc.", "HP", "LENOVO" (on a self-built PC the board's maker)
    pub system_maker: String,
    /// SystemProductName: "XPS 15 9520", "HP EliteBook 840 G9", Lenovo's machine type "21CBCTO1WW"
    pub system_product: String,
    /// SystemSKU: Dell's system id ("0B0F"), HP's product number, Lenovo's "LENOVO_MT_21CB_BU_Think_FM_ThinkPad X1"
    pub system_sku: String,
    /// SystemFamily: "ThinkPad X1 Carbon Gen 10", "XPS"
    pub system_family: String,
    /// BIOSVendor: "American Megatrends International, LLC.", "Dell Inc."
    pub bios_vendor: String,
}

impl Board {
    /// What the BIOS entry is called: the board for a self-built PC, the PC for a laptop or a
    /// ready-made PC (its board's name, "0X8DXD", means nothing to people).
    pub fn display_name(&self) -> String {
        match maker_of(self) {
            Some(Maker::Dell | Maker::Hp | Maker::Lenovo) if !self.system_product.trim().is_empty() => {
                // Lenovo's product name is the machine type; its family is the name people know.
                if maker_of(self) == Some(Maker::Lenovo) && !self.system_family.trim().is_empty() {
                    self.system_family.trim().to_string()
                } else {
                    self.system_product.trim().to_string()
                }
            }
            _ => self.product.trim().to_string(),
        }
    }
}

/// The makers whose BIOS lists are read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Maker {
    Gigabyte,
    Asus,
    Msi,
    Asrock,
    Dell,
    Hp,
    Lenovo,
}

impl Maker {
    pub fn name(self) -> &'static str {
        match self {
            Maker::Gigabyte => "Gigabyte",
            Maker::Asus => "ASUS",
            Maker::Msi => "MSI",
            Maker::Asrock => "ASRock",
            Maker::Dell => "Dell",
            Maker::Hp => "HP",
            Maker::Lenovo => "Lenovo",
        }
    }

    /// How a BIOS from this maker is flashed by the user.
    pub fn flash_how(self) -> &'static str {
        match self {
            Maker::Gigabyte => "put it on a USB stick and flash it yourself with Q-Flash in the BIOS",
            Maker::Asus => "put it on a USB stick and flash it yourself with EZ Flash in the BIOS",
            Maker::Msi => "put it on a USB stick and flash it yourself with M-Flash in the BIOS",
            Maker::Asrock => "put it on a USB stick and flash it yourself with Instant Flash in the BIOS",
            Maker::Dell | Maker::Hp | Maker::Lenovo => "run the maker's BIOS update yourself, plugged in and with nothing else open",
        }
    }
}

/// Whose BIOS list this PC is in: a ready-made PC's maker first (its board is the maker's own),
/// then the board's maker.
pub fn maker_of(b: &Board) -> Option<Maker> {
    let sys = b.system_maker.to_ascii_lowercase();
    let board = b.maker.to_ascii_lowercase();
    let is = |s: &str, names: &[&str]| names.iter().any(|n| s.starts_with(n));
    // Alienware PCs are Dell's, in Dell's catalog by their system id; older ones name Alienware.
    if is(&sys, &["dell", "alienware"]) {
        return Some(Maker::Dell);
    }
    if is(&sys, &["hp", "hewlett-packard", "hewlett packard"]) {
        return Some(Maker::Hp);
    }
    if is(&sys, &["lenovo"]) {
        return Some(Maker::Lenovo);
    }
    if is(&board, &["gigabyte"]) {
        return Some(Maker::Gigabyte);
    }
    if is(&board, &["asustek", "asus"]) {
        return Some(Maker::Asus);
    }
    if is(&board, &["micro-star", "msi"]) {
        return Some(Maker::Msi);
    }
    if is(&board, &["asrock"]) {
        return Some(Maker::Asrock);
    }
    None
}

/// The newest BIOS a maker lists for this PC.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Listed {
    /// "F13", "3202", "1.24.0"
    pub version: String,
    /// The maker's page for this board or PC, where the BIOS is downloaded
    pub page: Option<String>,
    /// Release date of the newest BIOS, "2026.09.12"
    pub date: Option<String>,
    /// What changed in it, as note lines
    pub notes: Vec<String>,
    /// Is it newer than the installed one? None: the two do not compare (another series)
    pub newer: Option<bool>,
}

/// A maker's answer for this PC.
#[derive(Clone, Debug, PartialEq)]
pub enum Answer {
    Listed(Listed),
    /// Not known: why (said to the user), and the maker's page when there is one
    Missing { why: String, page: Option<String> },
}

impl Answer {
    pub fn missing(why: impl Into<String>) -> Answer {
        Answer::Missing { why: why.into(), page: None }
    }
}

/// What the BIOS entry shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Bios {
    pub board: Board,
    pub maker: Option<Maker>,
    pub answer: Answer,
}

/// A driver or tool the board's or PC's maker lists for it (what the maker's own update app
/// offers: chipset, network, audio, Bluetooth, storage, firmware).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MakerDriver {
    /// The maker's own id for the package ("XXXXX", "sp151234"): stable, for the entry id
    pub id: String,
    /// "Realtek High Definition Audio Driver"
    pub name: String,
    /// "Audio", "Network", "Chipset", "Bluetooth", "Storage", "Firmware", "Utility"
    pub kind: String,
    /// The newest version the maker lists
    pub version: String,
    /// "2026.09.12"
    pub date: Option<String>,
    /// The maker's page for it, or for the board or PC
    pub page: Option<String>,
    /// What changed, as note lines
    pub notes: Vec<String>,
    /// Hardware ids it is for ("PCI\VEN_10EC&DEV_8125"): matched with this PC's devices
    pub hardware_ids: Vec<String>,
    /// The installed version, when the maker's catalog says how to read it (tools, packages
    /// without a device). None: known only through `hardware_ids`.
    pub installed: Option<String>,
    pub size: Option<u64>,
    /// How to install it here. None: only from the maker's page (no checksum, no silent install).
    pub install: Option<MakerInstall>,
}

/// A maker's package that installs without questions: downloaded, checked against the
/// catalog's checksum and the maker's signature, then run elevated with the catalog's command.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct MakerInstall {
    /// https, on the maker's own download server
    pub url: String,
    pub sha256: Option<String>,
    pub md5: Option<String>,
    /// The certificate name its signature must have ("Dell Inc", "HP Inc.", "Lenovo")
    pub signer: String,
    /// The silent command line's arguments ("/s", "-s -a", "/VERYSILENT")
    pub args: Vec<String>,
    /// Exit codes that mean done (0, and the maker's "done, restart later" codes)
    pub ok_codes: Vec<u32>,
    /// The program to run inside a downloaded zip ("setup.exe"); None: the download is the program
    pub inner: Option<String>,
}

/// Drivers and tools this PC's maker lists for it. Empty when the maker has no list the app
/// can read (its devices still get drivers from Windows Update and the chip makers).
pub async fn drivers(board: &Board) -> Vec<MakerDriver> {
    match maker_of(board) {
        Some(Maker::Gigabyte) => gigabyte::drivers(board).await,
        Some(Maker::Asus) => asus::drivers(board).await,
        Some(Maker::Msi) => msi::drivers(board).await,
        Some(Maker::Asrock) => asrock::drivers(board).await,
        Some(Maker::Dell) => dell::drivers(board).await,
        Some(Maker::Hp) => hp::drivers(board).await,
        Some(Maker::Lenovo) => lenovo::drivers(board).await,
        None => vec![],
    }
}

/// The job that installs a maker's package (`MakerDriver::install`).
pub fn plan_install(app_id: &str, name: &str, d: &MakerDriver) -> anyhow::Result<super::Plan> {
    use super::{Download, Plan, Step, Verify};
    let how = d.install.as_ref().ok_or_else(|| anyhow::anyhow!("{name} is installed from its maker's page"))?;
    let hosts = ["dell.com", "hp.com", "lenovo.com", "asus.com", "msi.com", "asrock.com", "gigabyte.com"];
    if !super::drivers::host_is(&how.url, &hosts) {
        return Err(anyhow::anyhow!("Unexpected download address for {name}: {}", how.url));
    }
    let file = how.url.rsplit('/').next().map(|f| f.split(['?', '#']).next().unwrap_or(f)).filter(|f| !f.is_empty() && !f.contains("..")).ok_or_else(|| anyhow::anyhow!("{name}: no file name in its download address"))?;
    let base = crate::paths::downloads().join("maker").join(crate::sources::other::safe_name(&d.id));
    let dest = base.join(file);
    let dl = Download { url: how.url.clone(), dest: dest.clone(), size: d.size, sha256: how.sha256.clone(), md5: how.md5.clone(), signer: None, sha512: None, headers: vec![] };
    // A checksum from the catalog, and the maker's signature on the program it runs.
    if how.sha256.is_none() && how.md5.is_none() {
        return Err(anyhow::anyhow!("{name}: its maker gives no checksum, so it is installed from the maker's page"));
    }
    if how.signer.trim().is_empty() {
        return Err(anyhow::anyhow!("{name}: no signer to check"));
    }
    let mut steps = Vec::new();
    let mut cleanup = vec![dest.clone()];
    let exe = match &how.inner {
        Some(inner) => {
            if inner.contains("..") || std::path::Path::new(inner).is_absolute() {
                return Err(anyhow::anyhow!("{name}: unexpected program path in its package"));
            }
            let dir = base.join("unpacked");
            steps.push(Step::ExtractZip { zip: dest.clone(), dest: dir.clone(), strip_root: false, elevated: false, label: format!("Unpacking {name}") });
            cleanup.push(dir.clone());
            dir.join(inner)
        }
        None => dest.clone(),
    };
    steps.push(Step::CheckSigner { path: exe.clone(), signer: how.signer.clone(), label: format!("Checking {name}") });
    steps.push(Step::Run {
        exe,
        args: how.args.clone(),
        cwd: None,
        ok_codes: if how.ok_codes.is_empty() { vec![0, super::drivers::EXIT_REBOOT] } else { how.ok_codes.clone() },
        elevated: true,
        label: format!("Installing {name} {}", d.version),
        log: None,
    });
    // The whole package folder goes afterwards: some packages unpack into it (Lenovo's "TMP").
    cleanup.push(base);
    Ok(Plan { app_id: app_id.into(), kind: crate::model::JobKind::Update, name: name.into(), to_version: d.version.clone(), downloads: vec![dl], steps, block_dirs: vec![], block_names: vec![], verify: Verify::None, fallback: None, cleanup })
}

/// This PC's board, PC and BIOS.
pub fn read() -> Option<Board> {
    use winreg::enums::{HKEY_LOCAL_MACHINE, KEY_READ};
    let k = winreg::RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("HARDWARE\\DESCRIPTION\\System\\BIOS", KEY_READ).ok()?;
    let g = |n: &str| k.get_value::<String, _>(n).unwrap_or_default().trim().to_string();
    let bios = g("BIOSVersion");
    let product = g("BaseBoardProduct");
    let system_product = g("SystemProductName");
    if bios.is_empty() || (product.is_empty() && system_product.is_empty()) {
        return None;
    }
    let date = g("BIOSReleaseDate");
    let bios_date = chrono::NaiveDate::parse_from_str(&date, "%m/%d/%Y").ok().map(|d| d.format("%Y.%m.%d").to_string());
    Some(Board {
        maker: g("BaseBoardManufacturer"),
        product,
        bios,
        bios_date,
        system_maker: g("SystemManufacturer"),
        system_product,
        system_sku: g("SystemSKU"),
        system_family: g("SystemFamily"),
        bios_vendor: g("BIOSVendor"),
    })
}

/// Ask this PC's maker for its newest BIOS.
pub async fn check(board: Board) -> Bios {
    let maker = maker_of(&board);
    let answer = match maker {
        Some(Maker::Gigabyte) => gigabyte::check(&board).await,
        Some(Maker::Asus) => asus::check(&board).await,
        Some(Maker::Msi) => msi::check(&board).await,
        Some(Maker::Asrock) => asrock::check(&board).await,
        Some(Maker::Dell) => dell::check(&board).await,
        Some(Maker::Hp) => hp::check(&board).await,
        Some(Maker::Lenovo) => lenovo::check(&board).await,
        None => Answer::missing("Newer BIOS versions come from the board maker's site"),
    };
    Bios { board, maker, answer }
}

/// The BIOS as a Drivers entry. A newer one is an update to get from the maker's page.
pub fn entry(b: &Bios) -> DriverEntry {
    let maker = b.maker.map(|m| m.name().to_string()).unwrap_or_else(|| super::drivers::short_maker(if b.board.maker.is_empty() { &b.board.system_maker } else { &b.board.maker }));
    let how = b.maker.map(Maker::flash_how).unwrap_or("flash it yourself with the board maker's tool");
    let (status, note, latest, page, notes, date) = match &b.answer {
        Answer::Listed(l) => {
            let (status, note, latest) = match l.newer {
                Some(true) => (AppStatus::Update, format!("BIOS {} is out. It is never flashed from here: download it from {maker}'s page, then {how}.", l.version), Some(l.version.clone())),
                // Installed newer than the list (a test build): this one is the newest.
                Some(false) => (AppStatus::Current, format!("{maker} lists {} as the newest", l.version), Some(b.board.bios.clone())),
                None => (AppStatus::Unknown, format!("{maker} lists {}, which does not compare with this BIOS", l.version), None),
            };
            let date = if l.newer == Some(true) { l.date.clone() } else { b.board.bios_date.clone() };
            (status, note, latest, l.page.clone(), if l.newer == Some(true) { l.notes.clone() } else { vec![] }, date)
        }
        Answer::Missing { why, page } => (AppStatus::Unknown, why.clone(), None, page.clone(), vec![], b.board.bios_date.clone()),
    };
    DriverEntry {
        id: "driver:bios".into(),
        name: format!("{} BIOS", b.board.display_name()),
        kind: "BIOS".into(),
        maker,
        source: b.maker.map(|m| m.name().to_ascii_lowercase()).unwrap_or_else(|| "vendor".into()),
        version: b.board.bios.clone(),
        latest,
        status,
        size: None,
        date,
        notes_url: None,
        vendor_url: super::drivers::safe_link(page),
        note: Some(note),
        notes,
        pre_channel: None,
        studio: None,
        folder: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn board(maker: &str, product: &str, bios: &str) -> Board {
        Board { maker: maker.into(), product: product.into(), bios: bios.into(), bios_date: Some("2026.06.09".into()), system_maker: maker.into(), ..Default::default() }
    }

    #[test]
    fn the_maker_is_the_pcs_for_a_ready_made_pc_and_the_boards_otherwise() {
        let gb = board("Gigabyte Technology Co., Ltd.", "B850 GAMING X WIFI6E ICE", "F12a");
        assert_eq!(maker_of(&gb), Some(Maker::Gigabyte));
        assert_eq!(maker_of(&board("ASUSTeK COMPUTER INC.", "ROG STRIX B650E-F GAMING WIFI", "3040")), Some(Maker::Asus));
        assert_eq!(maker_of(&board("Micro-Star International Co., Ltd.", "MAG B650 TOMAHAWK WIFI (MS-7D75)", "1.A0")), Some(Maker::Msi));
        assert_eq!(maker_of(&board("ASRock", "B650M Pro RS", "3.10")), Some(Maker::Asrock));
        let dell = Board { maker: "Dell Inc.".into(), product: "0X8DXD".into(), system_maker: "Dell Inc.".into(), system_product: "XPS 15 9520".into(), system_sku: "0A1F".into(), ..Default::default() };
        assert_eq!((maker_of(&dell), dell.display_name().as_str()), (Some(Maker::Dell), "XPS 15 9520"));
        let alienware = Board { maker: "Alienware".into(), product: "0J8H5R".into(), system_maker: "Alienware".into(), system_product: "Alienware Aurora R11".into(), system_sku: "0974".into(), ..Default::default() };
        assert_eq!(maker_of(&alienware), Some(Maker::Dell));
        let lenovo = Board { maker: "LENOVO".into(), product: "21CBCTO1WW".into(), system_maker: "LENOVO".into(), system_product: "21CBCTO1WW".into(), system_family: "ThinkPad X1 Carbon Gen 10".into(), ..Default::default() };
        assert_eq!((maker_of(&lenovo), lenovo.display_name().as_str()), (Some(Maker::Lenovo), "ThinkPad X1 Carbon Gen 10"));
        let hp = Board { maker: "HP".into(), product: "8A78".into(), system_maker: "HP".into(), system_product: "HP EliteBook 840 G9 Notebook PC".into(), ..Default::default() };
        assert_eq!(maker_of(&hp), Some(Maker::Hp));
        // A board maker's PC sold as a laptop (an ASUS laptop) is still ASUS.
        assert_eq!(maker_of(&Board { maker: "ASUSTeK COMPUTER INC.".into(), system_maker: "ASUSTeK COMPUTER INC.".into(), ..Default::default() }), Some(Maker::Asus));
        assert_eq!(maker_of(&board("Contoso", "X", "1")), None);
        assert_eq!(gb.display_name(), "B850 GAMING X WIFI6E ICE");
    }

    #[test]
    fn the_bios_entry_points_to_the_maker_and_is_never_installed_here() {
        let b = board("Gigabyte Technology Co., Ltd.", "B850 GAMING X WIFI6E", "F10");
        let page = Some("https://www.gigabyte.com/Motherboard/B850-GAMING-X-WIFI6E-rev-1x/support#support-dl-bios".to_string());
        let newer = Bios { board: b.clone(), maker: Some(Maker::Gigabyte), answer: Answer::Listed(Listed { version: "F13".into(), page: page.clone(), date: Some("2026.09.01".into()), notes: vec!["Improves memory compatibility".into()], newer: Some(true) }) };
        let e = entry(&newer);
        assert_eq!((e.status, e.latest.as_deref(), e.vendor_url.as_deref(), e.date.as_deref()), (AppStatus::Update, Some("F13"), page.as_deref(), Some("2026.09.01")));
        assert!(e.note.as_deref().unwrap().contains("Q-Flash") && e.notes.len() == 1);
        let cur = entry(&Bios { answer: Answer::Listed(Listed { version: "F11".into(), newer: Some(false), ..Default::default() }), ..newer.clone() });
        assert_eq!((cur.status, cur.latest.as_deref()), (AppStatus::Current, Some("F10")));
        let off = entry(&Bios { answer: Answer::missing("Could not reach Gigabyte"), ..newer.clone() });
        assert_eq!((off.status, off.latest), (AppStatus::Unknown, None));
        // A page from somewhere else is not opened.
        let odd = entry(&Bios { answer: Answer::Missing { why: "x".into(), page: Some("https://evil.example/gigabyte.com".into()) }, ..newer });
        assert_eq!(odd.vendor_url, None);
    }
}

#[cfg(test)]
mod live {
    /// Reads this PC's board and asks its maker: `cargo test --lib live_bios -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_bios() {
        let b = super::read().expect("board");
        println!("{b:#?}");
        let bios = super::check(b).await;
        println!("{:#?}\n{:#?}", bios.answer, super::entry(&bios));
    }
}
