//! The AMD Ryzen AI NPU driver, from AMD's own Ryzen AI documentation: the page names the newest
//! driver zip, and the driver's version and device ids are read from the `kipudrv.inf` inside the
//! zip with a few range requests (the docs write the version another way than Windows does:
//! "32.0.203.376" for the INF's "32.00.20101.3760"). The zip has no checksum: Windows checks the
//! driver's signed catalog, and AMD's installer in the zip must be signed by AMD.

use super::drivers::Present;
use super::{Download, Plan, Step, Verify};
use crate::model::JobKind;
use anyhow::{anyhow, bail, Result};
use once_cell::sync::Lazy;
use regex::Regex;
use std::cell::Cell;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

const DOCS: &str = "https://ryzenai.docs.amd.com/en/main/inst.md";
/// The page people read it on
const DOCS_PAGE: &str = "https://ryzenai.docs.amd.com/en/latest/inst.html";
/// The NPU's PCI ids (Phoenix, Hawk Point, Strix, Strix Halo, Krackan Point), from the driver's INF.
/// A PC without one of them is not offered the driver.
const HARDWARE_IDS: [&str; 2] = ["PCI\\VEN_1022&DEV_1502", "PCI\\VEN_1022&DEV_17F0"];
/// How much of the end of the zip is read first (its directory is there; the zip reader looks up
/// to 64 KB back for the end record), and how much more for each part of it the program needs next.
const TAIL: u64 = 72 * 1024;
const CHUNK: u64 = 256 * 1024;
const MAX_FETCHES: usize = 12;
/// The INF is a few KB; a bigger file under that name is not it.
const MAX_MEMBER: u64 = 1024 * 1024;

/// AMD's newest NPU driver.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Release {
    /// The version Windows shows once it is installed ("32.0.20101.3760")
    pub version: String,
    /// The driver's date, "2026.04.04"
    pub date: Option<String>,
    pub url: String,
    pub size: Option<u64>,
    /// The PCI ids the driver's INF lists
    pub hardware_ids: Vec<String>,
    pub notes_url: String,
}

/// Is this installed driver for an AMD NPU?
pub fn is_npu(p: &Present) -> bool {
    HARDWARE_IDS.iter().any(|h| super::drivers::hw_match(h, &p.matching_id))
}

/// Does this driver's INF list the device?
pub fn supports(r: &Release, p: &Present) -> bool {
    r.hardware_ids.iter().any(|h| super::drivers::hw_match(h, &p.matching_id))
}

/// The newest driver zip the docs link: its number ("376") and address.
pub fn parse_docs(md: &str) -> Option<(u32, String)> {
    static ZIP: Lazy<Regex> = Lazy::new(|| Regex::new(r"https://download\.amd\.com/opendownload/RyzenAI/Driver/NPU_RAI_(\d+)_WHQL\.zip").unwrap());
    ZIP.captures_iter(md).filter_map(|c| Some((c[1].parse::<u32>().ok()?, c[0].to_string()))).max_by_key(|(n, _)| *n)
}

/// From the driver's INF: the version Windows shows ("DriverVer = 04/04/2026,32.00.20101.3760" is
/// "32.0.20101.3760", the way Windows writes the numbers), its date and the PCI ids it lists.
pub fn parse_inf(bytes: &[u8]) -> Option<(String, Option<String>, Vec<String>)> {
    static VER: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)^\s*DriverVer\s*=\s*(\d+)/(\d+)/(\d+)\s*,\s*(\d+(?:\.\d+){1,3})").unwrap());
    static ID: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\bPCI\\VEN_[0-9A-F]{4}&DEV_[0-9A-F]{4}\b").unwrap());
    let text = if bytes.starts_with(&[0xFF, 0xFE]) {
        String::from_utf16_lossy(&bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect::<Vec<_>>())
    } else {
        String::from_utf8_lossy(bytes).into_owned()
    };
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim_start().starts_with(';')).collect();
    let c = lines.iter().find_map(|l| VER.captures(l))?;
    let version = c[4].split('.').map(|p| p.parse::<u64>().map(|n| n.to_string())).collect::<Result<Vec<_>, _>>().ok()?.join(".");
    let date = chrono::NaiveDate::from_ymd_opt(c[3].parse().ok()?, c[1].parse().ok()?, c[2].parse().ok()?).map(|d| d.format("%Y.%m.%d").to_string());
    let mut ids: Vec<String> = Vec::new();
    for l in &lines {
        for m in ID.find_iter(l) {
            let id = m.as_str().to_ascii_uppercase();
            if !ids.contains(&id) {
                ids.push(id);
            }
        }
    }
    Some((version, date, ids))
}

// ---------------------------------------------------------------- one file out of a zip on a server

/// A zip of which only some parts are held: reading anywhere else fails, and says where.
struct Sparse<'a> {
    total: u64,
    parts: &'a [(u64, Vec<u8>)],
    pos: u64,
    missed: &'a Cell<Option<u64>>,
}

impl Read for Sparse<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() || self.pos >= self.total {
            return Ok(0);
        }
        match self.parts.iter().find(|(s, b)| self.pos >= *s && self.pos < *s + b.len() as u64) {
            Some((s, b)) => {
                let at = (self.pos - s) as usize;
                let n = buf.len().min(b.len() - at);
                buf[..n].copy_from_slice(&b[at..at + n]);
                self.pos += n as u64;
                Ok(n)
            }
            None => {
                self.missed.set(Some(self.pos));
                Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "that part of the zip is not held yet"))
            }
        }
    }
}

impl Seek for Sparse<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::End(n) => self.total as i128 + n as i128,
            SeekFrom::Current(n) => self.pos as i128 + n as i128,
        };
        if p < 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "seek before the start"));
        }
        self.pos = p as u64;
        Ok(self.pos)
    }
}

enum Need {
    /// Read this part of the zip, then try again
    At(u64),
    Fail(anyhow::Error),
}

/// Try to read the file whose name ends with `suffix` from the parts held so far.
fn try_member(total: u64, parts: &[(u64, Vec<u8>)], suffix: &str) -> std::result::Result<Vec<u8>, Need> {
    let missed = Cell::new(None);
    let read = || -> zip::result::ZipResult<Option<Vec<u8>>> {
        // The archive starts at the start of the file: without saying so, the zip reader reads the
        // whole file to look for data in front of it.
        let config = zip::read::Config { archive_offset: zip::read::ArchiveOffset::Known(0) };
        let mut z = zip::ZipArchive::with_config(config, Sparse { total, parts, pos: 0, missed: &missed })?;
        let Some(name) = z.file_names().flatten().find(|n| n.to_ascii_lowercase().ends_with(suffix)).map(|n| n.into_owned()) else { return Ok(None) };
        let f = z.by_name(&name)?;
        if f.size() > MAX_MEMBER {
            return Ok(None);
        }
        let mut out = Vec::new();
        f.take(MAX_MEMBER).read_to_end(&mut out)?;
        Ok(Some(out))
    };
    match read() {
        Ok(Some(b)) => Ok(b),
        Ok(None) => Err(Need::Fail(anyhow!("the zip has no usable {suffix}"))),
        Err(e) => match missed.get() {
            Some(p) => Err(Need::At(p)),
            None => Err(Need::Fail(e.into())),
        },
    }
}

/// Read one file out of a zip of `total` bytes on a server, fetching only the parts of the zip it
/// needs (`fetch(from, to)` gets the bytes from..to): the end of the zip first, then what the
/// file's place in the directory points to. `tail` is that first part.
async fn read_member<F, Fut>(total: u64, tail: (u64, Vec<u8>), mut fetch: F, suffix: &str) -> Result<Vec<u8>>
where
    F: FnMut(u64, u64) -> Fut,
    Fut: std::future::Future<Output = Result<Vec<u8>>>,
{
    let mut parts = vec![tail];
    for _ in 0..MAX_FETCHES {
        match try_member(total, &parts, suffix) {
            Ok(b) => return Ok(b),
            Err(Need::Fail(e)) => return Err(e),
            Err(Need::At(from)) => {
                let to = (from + CHUNK).min(total);
                if from >= total {
                    bail!("the zip points past its end");
                }
                parts.push((from, fetch(from, to).await?));
            }
        }
    }
    bail!("the zip needed too many reads to find {suffix}")
}

/// A part of a file on a server (the last `n` bytes with `from` None), and the file's size.
async fn range(url: &str, from: Option<u64>, to: u64) -> Result<(Vec<u8>, u64)> {
    let spec = match from {
        Some(f) => format!("bytes={f}-{}", to - 1),
        None => format!("bytes=-{to}"),
    };
    let r = crate::http::DL_CLIENT.get(url).header(reqwest::header::RANGE, spec).timeout(Duration::from_secs(60)).send().await?;
    if r.status() != reqwest::StatusCode::PARTIAL_CONTENT {
        bail!("AMD's server does not send parts of a file (HTTP {})", r.status());
    }
    let total = r.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(|v| v.rsplit('/').next()).and_then(|t| t.parse::<u64>().ok()).ok_or_else(|| anyhow!("AMD's server did not say how big the file is"))?;
    Ok((r.bytes().await?.to_vec(), total))
}

/// Ask AMD for the newest NPU driver: the docs name the zip, its INF names the version.
pub async fn latest() -> Result<Release> {
    let docs = crate::http::get_cached(DOCS, &[], Duration::from_secs(6 * 3600)).await?;
    let (_, url) = parse_docs(&String::from_utf8_lossy(&docs.body)).ok_or_else(|| anyhow!("AMD's Ryzen AI page names no NPU driver"))?;
    let (tail, total) = range(&url, None, TAIL).await?;
    let from = total.saturating_sub(tail.len() as u64);
    let inf = read_member(total, (from, tail), |a, b| {
        let url = url.clone();
        async move { Ok(range(&url, Some(a), b).await?.0) }
    }, "kipudrv.inf")
    .await?;
    let (version, date, hardware_ids) = parse_inf(&inf).ok_or_else(|| anyhow!("AMD's NPU driver has no readable version"))?;
    Ok(Release { version, date, url, size: Some(total), hardware_ids, notes_url: DOCS_PAGE.into() })
}

/// Download AMD's NPU driver zip, unpack it and run its installer silently. The installer asks
/// for AMD's license: its own `--AcceptAmdEula` answers it, so installing here accepts it.
pub fn plan_install(app_id: &str, name: &str, r: &Release) -> Result<Plan> {
    if !super::drivers::host_is(&r.url, &["download.amd.com"]) {
        return Err(anyhow!("Unexpected AMD download address: {}", r.url));
    }
    let file = r.url.rsplit('/').next().filter(|f| f.ends_with(".zip")).ok_or_else(|| anyhow!("AMD's NPU download is not a zip"))?;
    let base = crate::paths::downloads().join("amd").join("npu");
    let zip = base.join(file);
    let dir = base.join("unpacked");
    let exe = dir.join("npu_sw_installer.exe");
    // No checksum from AMD and a zip carries no signature: the installer in it must be AMD's, and
    // Windows only installs the driver when its catalog is signed.
    let dl = Download { url: r.url.clone(), dest: zip.clone(), size: r.size, sha256: None, md5: None, signer: None, sha512: None, headers: vec![] };
    Ok(Plan {
        app_id: app_id.into(),
        kind: JobKind::Update,
        name: name.into(),
        to_version: r.version.clone(),
        downloads: vec![dl],
        steps: vec![
            Step::ExtractZip { zip: zip.clone(), dest: dir.clone(), strip_root: false, elevated: false, label: "Unpacking AMD's NPU driver".into() },
            Step::CheckSigner { path: exe.clone(), signer: super::amd::SIGNER.into(), label: "Checking AMD's signature".into() },
            Step::Run { exe, args: vec!["--AcceptAmdEula".into()], cwd: Some(dir.clone()), ok_codes: vec![0, super::drivers::EXIT_REBOOT], elevated: true, label: format!("Installing AMD's NPU driver {}", r.version), log: None },
        ],
        block_dirs: vec![],
        block_names: vec![],
        verify: Verify::Driver { app_id: app_id.into(), version: r.version.clone() },
        fallback: None,
        cleanup: vec![zip, dir],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const INF: &str = "; kipudrv.inf\r\n[Version]\r\nSignature = \"$Windows NT$\"\r\n; DriverVer = 01/01/2020,1.0.0.1\r\nDriverVer = 04/04/2026,32.00.20101.3760\r\nCatalogFile=kipudrv.cat\r\n[Strings]\r\n%IpuMcdmDriver% = IpuMcdmDriver,PCI\\VEN_1022&DEV_1502\r\n%IpuMcdmDriver% = IpuMcdmDriver,PCI\\VEN_1022&DEV_17F0\r\n";

    fn dev(id: &str) -> Present {
        Present { matching_id: id.into(), version: "32.0.20001.3000".into(), date: None, desc: "NPU Compute Accelerator Device".into(), provider: "Advanced Micro Devices, Inc.".into(), class: "ComputeAccelerator".into() }
    }

    fn release() -> Release {
        Release { version: "32.0.20101.3760".into(), date: Some("2026.04.04".into()), url: "https://download.amd.com/opendownload/RyzenAI/Driver/NPU_RAI_376_WHQL.zip".into(), size: Some(26_936_876), hardware_ids: vec!["PCI\\VEN_1022&DEV_1502".into(), "PCI\\VEN_1022&DEV_17F0".into()], notes_url: DOCS_PAGE.into() }
    }

    #[test]
    fn the_docs_name_the_newest_driver_zip() {
        let md = "- [`NPU Driver (Version 32.0.203.280)`](https://ryzenai.docs.amd.com/en/latest/https://download.amd.com/opendownload/RyzenAI/Driver/NPU_RAI_280_WHQL.zip)\n- [`NPU Driver (Version 32.0.203.376)`](https://ryzenai.docs.amd.com/en/latest/https://download.amd.com/opendownload/RyzenAI/Driver/NPU_RAI_376_WHQL.zip)";
        assert_eq!(parse_docs(md), Some((376, "https://download.amd.com/opendownload/RyzenAI/Driver/NPU_RAI_376_WHQL.zip".into())));
        assert_eq!(parse_docs("no links"), None);
        assert_eq!(parse_docs("https://evil.example/opendownload/RyzenAI/Driver/NPU_RAI_999_WHQL.zip"), None);
    }

    #[test]
    fn the_inf_gives_the_version_the_way_windows_shows_it_with_its_date_and_ids() {
        let (v, d, ids) = parse_inf(INF.as_bytes()).unwrap();
        assert_eq!((v.as_str(), d.as_deref()), ("32.0.20101.3760", Some("2026.04.04")));
        assert_eq!(ids, vec!["PCI\\VEN_1022&DEV_1502".to_string(), "PCI\\VEN_1022&DEV_17F0".to_string()]);
        // An INF saved as UTF-16 reads the same.
        let utf16: Vec<u8> = [0xFF, 0xFE].into_iter().chain(INF.encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        assert_eq!(parse_inf(&utf16).unwrap().0, "32.0.20101.3760");
        assert!(parse_inf(b"[Version]\r\nSignature = x").is_none());
        // The docs' "32.0.203.376" is another way to write a version: only the INF's compares with
        // what Windows has for the installed driver.
        assert!(crate::sources::update_offered(&v, "32.0.203.280", false));
        assert!(!crate::sources::update_offered(&v, "32.0.20101.3760", false));
        assert!(crate::sources::update_offered(&v, "32.0.20001.3000", false));
    }

    #[test]
    fn only_a_pc_with_one_of_the_npu_ids_gets_the_driver() {
        let r = release();
        assert!(is_npu(&dev("pci\\ven_1022&dev_1502")) && is_npu(&dev("PCI\\VEN_1022&DEV_17F0&SUBSYS_00011022")));
        assert!(supports(&r, &dev("pci\\ven_1022&dev_17f0")));
        // Another AMD device, an Intel NPU and a GPU: not offered.
        for id in ["pci\\ven_1022&dev_1503", "pci\\ven_8086&dev_7d1d", "pci\\ven_1002&dev_744c", "pci\\ven_1022&dev_15021"] {
            assert!(!is_npu(&dev(id)) && !supports(&r, &dev(id)), "{id}");
        }
    }

    /// A zip of the kind AMD ships: a big file, the driver folder with the INF, more files.
    fn zip_bytes() -> Vec<u8> {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        let deflated = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        w.start_file("npu_mcdm_stack_prod/big.bin", stored).unwrap();
        w.write_all(&vec![7u8; 1_500_000]).unwrap();
        w.start_file("npu_mcdm_stack_prod/kipudrv.inf", deflated).unwrap();
        w.write_all(INF.as_bytes()).unwrap();
        w.start_file("npu_mcdm_stack_prod/other.bin", stored).unwrap();
        w.write_all(&vec![9u8; 1_000_000]).unwrap();
        w.start_file("npu_sw_installer.exe", stored).unwrap();
        w.write_all(&vec![1u8; 300_000]).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[tokio::test]
    async fn the_inf_is_read_from_a_zip_with_a_few_range_requests() {
        let z = zip_bytes();
        let total = z.len() as u64;
        let asked = std::sync::Mutex::new(Vec::new());
        let from = total - TAIL;
        let inf = read_member(total, (from, z[from as usize..].to_vec()), |a, b| {
            asked.lock().unwrap().push((a, b));
            let part = z[a as usize..b as usize].to_vec();
            async move { Ok(part) }
        }, "kipudrv.inf")
        .await
        .unwrap();
        assert_eq!(String::from_utf8(inf).unwrap(), INF);
        // Not the whole zip: the end, then the part the INF is in.
        let got: u64 = asked.lock().unwrap().iter().map(|(a, b)| b - a).sum::<u64>() + TAIL;
        assert!(got < total / 4, "{got} of {total} bytes read");
        assert!(!asked.lock().unwrap().is_empty());
        // A name the zip does not have, and a zip that is not one.
        let err = read_member(total, (from, z[from as usize..].to_vec()), |a, b| {
            let part = z[a as usize..b as usize].to_vec();
            async move { Ok(part) }
        }, "missing.inf")
        .await;
        assert!(err.is_err());
        let junk = vec![0u8; 100_000];
        assert!(read_member(100_000, (36_000, junk[36_000..].to_vec()), |_, _| async { Ok(vec![0u8; 10]) }, "kipudrv.inf").await.is_err());
    }

    #[test]
    fn the_install_unpacks_checks_amds_signature_runs_silently_and_checks_the_version() {
        let r = release();
        let plan = plan_install("driver:dev:pci\\ven_1022&dev_17f0", "NPU Compute Accelerator Device", &r).unwrap();
        assert!(plan.downloads[0].headers.is_empty() && plan.downloads[0].sha256.is_none());
        assert!(matches!(&plan.steps[0], Step::ExtractZip { .. }));
        let Step::CheckSigner { path, signer, .. } = &plan.steps[1] else { panic!() };
        assert_eq!((signer.as_str(), path.file_name().and_then(|f| f.to_str())), (crate::sources::amd::SIGNER, Some("npu_sw_installer.exe")));
        let Step::Run { exe, args, elevated, cwd, .. } = &plan.steps[2] else { panic!() };
        assert!(*elevated && exe == path && cwd.is_some() && args == &vec!["--AcceptAmdEula".to_string()]);
        assert!(matches!(&plan.verify, Verify::Driver { version, .. } if version == "32.0.20101.3760"));
        assert!(plan_install("x", "x", &Release { url: "https://evil.example/download.amd.com/a.zip".into(), ..r.clone() }).is_err());
        assert!(plan_install("x", "x", &Release { url: "https://download.amd.com/a.exe".into(), ..r }).is_err());
    }

    /// Asks AMD's site (no download of the zip, only a few parts of it):
    /// `cargo test --lib live_npu -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_npu() {
        let r = latest().await.expect("AMD");
        println!("{r:#?}");
        assert!(r.hardware_ids.iter().any(|h| h.ends_with("DEV_1502")));
    }
}
