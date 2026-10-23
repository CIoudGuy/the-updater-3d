//! App packages (MSIX and APPX) installed for this user, and installing, updating and removing
//! them through Windows' package manager. `msstore` reads the Store copies that other sources
//! list; this lists every app package the winget catalog can match by its package family name.
//!
//! A package is only ever installed after its download matched the catalog's SHA-256, its
//! manifest names the expected package family (so its publisher is the one the family belongs
//! to), its signature file matches the catalog's signature checksum when it gives one, and
//! Windows trusts its signature. Windows checks that signature again when it installs it.

use anyhow::{anyhow, bail, Context, Result};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Microsoft's own publisher id: its Store-signed packages are parts of Windows (App Installer,
/// Terminal, Photos), which Windows and the Store keep up to date.
pub const WINDOWS_PUBLISHER: &str = "8wekyb3d8bbwe";

/// How a package is signed (Windows' PackageSignatureKind).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sig {
    #[default]
    None,
    Developer,
    Enterprise,
    Store,
    System,
}

/// One app package installed for this user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AppPackage {
    /// Identity name: "Canva.Affinity"
    pub name: String,
    /// "Canva.Affinity_3.3.1.4940_x64__8a0j1tnjnt4a4"
    pub full_name: String,
    /// "Canva.Affinity_8a0j1tnjnt4a4"
    pub family: String,
    /// Always four numbers: "3.3.1.4940"
    pub version: String,
    pub install_dir: Option<PathBuf>,
    /// When it was installed or last updated
    pub installed_at: Option<std::time::SystemTime>,
    pub signature: Sig,
    pub framework: bool,
    pub resource: bool,
    /// Registered from loose files by a developer: nothing to update it from
    pub dev_mode: bool,
    /// Its name and publisher as Windows shows them ("Affinity", "Canva")
    pub display_name: String,
    pub publisher: String,
}

impl AppPackage {
    /// Installed and updated by the Microsoft Store.
    pub fn store_signed(&self) -> bool {
        self.signature == Sig::Store
    }
}

/// Is this an app the user installed (not a framework, resource or Windows part)?
pub fn listed(p: &AppPackage) -> bool {
    if p.framework || p.resource || p.dev_mode || p.family.is_empty() {
        return false;
    }
    match p.signature {
        Sig::Developer | Sig::Enterprise => true,
        Sig::Store => !publisher_id(&p.family).eq_ignore_ascii_case(WINDOWS_PUBLISHER),
        Sig::System | Sig::None => false,
    }
}

/// The publisher id of a family or full package name ("8a0j1tnjnt4a4").
pub fn publisher_id(name: &str) -> &str {
    name.rsplit_once('_').map(|(_, p)| p).unwrap_or("")
}

/// The family name of a full package name: "Name_Version_Arch_Resource_Publisher" -> "Name_Publisher".
pub fn family_of(full_name: &str) -> String {
    let parts: Vec<&str> = full_name.split('_').collect();
    if parts.len() == 5 {
        format!("{}_{}", parts[0], parts[4])
    } else {
        full_name.to_string()
    }
}

/// The processor a full package name is for: "x64", "x86", "arm64", "neutral".
pub fn arch_of(full_name: &str) -> String {
    let parts: Vec<&str> = full_name.split('_').collect();
    if parts.len() == 5 {
        parts[2].to_ascii_lowercase()
    } else {
        String::new()
    }
}

/// The publisher id Windows derives from a package's publisher ("CN=..."): the first 64 bits of
/// the SHA-256 of the publisher in UTF-16, as 13 base32 letters.
pub fn publisher_hash(publisher: &str) -> String {
    const ALPHABET: &[u8] = b"0123456789abcdefghjkmnpqrstvwxyz";
    let utf16: Vec<u8> = publisher.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let d = Sha256::digest(&utf16);
    let bits = u64::from_be_bytes(d[..8].try_into().unwrap());
    // 64 bits and one zero bit make 13 letters of 5 bits.
    let v = (bits as u128) << 1;
    (0..13).rev().map(|k| ALPHABET[((v >> (k * 5)) & 31) as usize] as char).collect()
}

/// "4.5.3.0" shows as "4.5.3": the fourth number is only kept when it is set.
pub fn display_version(v: &str) -> String {
    super::msstore::display_version(v)
}

// ---------------------------------------------------------------- package files

/// What a package file (.msix, .appx or a bundle of them) says it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Identity {
    pub name: String,
    /// "CN=..., O=..., C=..."
    pub publisher: String,
    pub version: String,
    /// A bundle (.msixbundle, .appxbundle) of packages for several processors
    pub bundle: bool,
}

impl Identity {
    pub fn family(&self) -> String {
        format!("{}_{}", self.name, publisher_hash(&self.publisher))
    }
}

/// The Identity element of a package manifest (AppxManifest.xml) or bundle manifest.
pub fn identity_of(xml: &str) -> Option<Identity> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let root = doc.root_element();
    let bundle = root.tag_name().name() == "Bundle";
    let id = root.children().find(|n| n.tag_name().name() == "Identity")?;
    Some(Identity { name: id.attribute("Name")?.to_string(), publisher: id.attribute("Publisher")?.to_string(), version: id.attribute("Version").unwrap_or("").to_string(), bundle })
}

fn read_entry(ar: &mut zip::ZipArchive<std::io::BufReader<std::fs::File>>, name: &str, max: u64) -> Result<Option<Vec<u8>>> {
    let mut e = match ar.by_name(name) {
        Ok(e) => e,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if e.size() > max {
        bail!("{name} in the package is too large");
    }
    let mut v = Vec::with_capacity(e.size() as usize);
    e.read_to_end(&mut v)?;
    Ok(Some(v))
}

/// Read a package file's identity and the SHA-256 of its signature file (AppxSignature.p7x).
pub fn read_package_file(file: &Path) -> Result<(Identity, Option<String>)> {
    let f = std::fs::File::open(file).with_context(|| format!("open {}", file.display()))?;
    let mut ar = zip::ZipArchive::new(std::io::BufReader::new(f)).with_context(|| format!("{} is not an app package", file.display()))?;
    let xml = match read_entry(&mut ar, "AppxMetadata/AppxBundleManifest.xml", 8 << 20)? {
        Some(x) => x,
        None => read_entry(&mut ar, "AppxManifest.xml", 8 << 20)?.ok_or_else(|| anyhow!("{} has no package manifest", file.display()))?,
    };
    let id = identity_of(&String::from_utf8_lossy(&xml)).ok_or_else(|| anyhow!("{} has no package identity", file.display()))?;
    let sig = read_entry(&mut ar, "AppxSignature.p7x", 4 << 20)?.map(|b| hex::encode(Sha256::digest(&b)));
    Ok((id, sig))
}

/// Is the package what the catalog says: its family (name and publisher) and, when the catalog
/// gives one, its signature file's checksum? `sig` is the file's signature checksum.
pub fn check_identity(id: &Identity, sig: Option<&str>, family: &str, signature_sha256: Option<&str>) -> Result<()> {
    if !id.family().eq_ignore_ascii_case(family) {
        bail!("The downloaded package is {} instead of {family}, so it was not installed", id.family());
    }
    if let Some(want) = signature_sha256.filter(|s| !s.is_empty()) {
        match sig {
            Some(got) if got.eq_ignore_ascii_case(want) => {}
            Some(_) => bail!("The downloaded package's signature does not match the catalog's, so it was not installed"),
            None => bail!("The downloaded package is not signed, so it was not installed"),
        }
    }
    Ok(())
}

/// The file extension Windows needs to read a package of this kind.
pub fn package_ext(id: &Identity, current: &Path) -> &'static str {
    let ext = current.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    match (id.bundle, ext.as_str()) {
        (true, "appxbundle") => "appxbundle",
        (true, _) => "msixbundle",
        (false, "appx") => "appx",
        (false, _) => "msix",
    }
}

// ---------------------------------------------------------------- package manifests

/// What an installed package's manifest gives: its first application's id and program file,
/// and its logo files (relative paths, best first).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Visual {
    pub app_id: Option<String>,
    pub exe: Option<String>,
    pub logos: Vec<String>,
}

pub fn visual_of(xml: &str) -> Visual {
    let Ok(doc) = roxmltree::Document::parse(xml) else { return Visual::default() };
    let app = doc.descendants().find(|n| n.tag_name().name() == "Application");
    let ve = app.and_then(|a| a.descendants().find(|n| n.tag_name().name() == "VisualElements"));
    let mut logos = Vec::new();
    if let Some(v) = ve {
        for k in ["Square44x44Logo", "Square150x150Logo"] {
            if let Some(l) = v.attribute(k) {
                logos.push(l.to_string());
            }
        }
    }
    if let Some(l) = doc.descendants().find(|n| n.tag_name().name() == "Properties").and_then(|p| p.children().find(|n| n.tag_name().name() == "Logo")).and_then(|n| n.text()) {
        logos.push(l.trim().to_string());
    }
    Visual { app_id: app.and_then(|a| a.attribute("Id")).map(String::from), exe: app.and_then(|a| a.attribute("Executable")).filter(|e| !e.is_empty()).map(String::from), logos }
}

/// The best file for a logo the manifest names ("Assets\Square44x44Logo.png"): packages keep
/// it in sizes ("....targetsize-256_altform-unplated.png", "....scale-200.png"). The largest
/// target size up to 256 wins (unplated first), else the largest scale, else the file itself.
pub fn logo_file(dir: &Path, rel: &str) -> Option<PathBuf> {
    let base = dir.join(rel.replace('/', "\\"));
    let parent = base.parent()?;
    let stem = base.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let ext = base.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut best: Option<(u32, PathBuf)> = None;
    for e in std::fs::read_dir(parent).ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        let Some(q) = n.strip_prefix(&format!("{stem}.")).and_then(|r| r.strip_suffix(&format!(".{ext}"))) else { continue };
        let mut score = 0u32;
        for part in q.split('_') {
            if let Some(t) = part.strip_prefix("targetsize-").and_then(|t| t.parse::<u32>().ok()) {
                score += if t <= 256 { 10_000 + t * 10 } else { 5_000 };
            } else if let Some(s) = part.strip_prefix("scale-").and_then(|s| s.parse::<u32>().ok()) {
                score += s;
            } else if part == "altform-unplated" {
                score += 5;
            } else if part.starts_with("altform-") || part.starts_with("contrast-") || part.starts_with("theme-") {
                score = 0;
                break;
            }
        }
        if score > 0 && best.as_ref().is_none_or(|(b, _)| score > *b) {
            best = Some((score, e.path()));
        }
    }
    best.map(|b| b.1).or_else(|| base.is_file().then_some(base))
}

/// The application id, program file and logo of an installed package.
pub fn read_visual(dir: &Path) -> (Option<String>, Option<PathBuf>, Option<PathBuf>) {
    let Ok(xml) = std::fs::read_to_string(dir.join("AppxManifest.xml")) else { return (None, None, None) };
    let v = visual_of(&xml);
    let exe = v.exe.as_ref().map(|e| dir.join(e.replace('/', "\\")));
    let logo = v.logos.iter().find_map(|l| logo_file(dir, l));
    (v.app_id, exe, logo)
}

// ---------------------------------------------------------------- this PC

/// The app packages installed for this user (see `listed`). Empty when Windows can not list them.
/// Asking Windows takes about half a second, so the list is kept and asked for again only when
/// this user's package names changed (one was added, updated or removed) or after `HELD_FOR`.
pub fn this_users_packages() -> Vec<AppPackage> {
    let stamp = packages_stamp();
    if let Some(v) = still_current(&HELD.lock(), stamp, std::time::Instant::now()) {
        return v;
    }
    match read_app_packages() {
        Ok(v) => {
            let v: Vec<AppPackage> = v.into_iter().filter(listed).collect();
            if let Some(s) = stamp {
                *HELD.lock() = Some((s, std::time::Instant::now(), v.clone()));
            }
            v
        }
        Err(e) => {
            log::warn!("app packages: {e}");
            vec![]
        }
    }
}

/// The packages last read, with the package names stamp and the time they were read at.
type Held = Option<(u64, std::time::Instant, Vec<AppPackage>)>;
static HELD: parking_lot::Mutex<Held> = parking_lot::const_mutex(None);
/// Read again after this long even with the same names (a package's status can change).
const HELD_FOR: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// The held list, when it was read under the same stamp not long ago.
fn still_current(held: &Held, stamp: Option<u64>, now: std::time::Instant) -> Option<Vec<AppPackage>> {
    let (s, at, v) = held.as_ref()?;
    (Some(*s) == stamp && now.saturating_duration_since(*at) < HELD_FOR).then(|| v.clone())
}

/// A stamp of this user's package names (each names its version), from the registry list Windows
/// keeps of them: a millisecond instead of asking the package manager. None when it can not be read.
fn packages_stamp() -> Option<u64> {
    use std::hash::{Hash, Hasher};
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    const KEY: &str = r"Software\Classes\Local Settings\Software\Microsoft\Windows\CurrentVersion\AppModel\Repository\Packages";
    let k = winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(KEY, KEY_READ).ok()?;
    let mut names: Vec<String> = k.enum_keys().collect::<std::io::Result<_>>().ok()?;
    if names.is_empty() {
        return None;
    }
    names.sort_unstable();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    names.hash(&mut h);
    Some(h.finish())
}

/// `this_users_packages`, which tests leave out.
#[cfg(not(test))]
pub fn app_packages() -> Vec<AppPackage> {
    this_users_packages()
}

/// Tests never read this PC's packages.
#[cfg(test)]
pub fn app_packages() -> Vec<AppPackage> {
    vec![]
}

/// Join the multithreaded apartment (and stay): see `msstore::read_packages`.
fn com_init() {
    let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
}

fn sig_of(k: windows::ApplicationModel::PackageSignatureKind) -> Sig {
    use windows::ApplicationModel::PackageSignatureKind as K;
    match k {
        K::Developer => Sig::Developer,
        K::Enterprise => Sig::Enterprise,
        K::Store => Sig::Store,
        K::System => Sig::System,
        _ => Sig::None,
    }
}

fn to_system_time(t: i64) -> Option<std::time::SystemTime> {
    // 100 ns steps since 1601
    let unix = t / 10_000_000 - 11_644_473_600;
    (t > 0 && unix > 0).then(|| std::time::UNIX_EPOCH + std::time::Duration::from_secs(unix as u64))
}

fn package_of(p: &windows::ApplicationModel::Package) -> Option<AppPackage> {
    let id = p.Id().ok()?;
    let v = id.Version().ok()?;
    Some(AppPackage {
        name: id.Name().ok()?.to_string(),
        full_name: id.FullName().ok()?.to_string(),
        family: id.FamilyName().ok()?.to_string(),
        version: format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision),
        install_dir: p.InstalledPath().ok().map(|s| PathBuf::from(s.to_string())).filter(|d: &PathBuf| !d.as_os_str().is_empty()),
        installed_at: p.InstalledDate().ok().and_then(|d| to_system_time(d.UniversalTime)),
        signature: p.SignatureKind().map(sig_of).unwrap_or_default(),
        framework: p.IsFramework().unwrap_or(true),
        resource: p.IsResourcePackage().unwrap_or(false),
        dev_mode: p.IsDevelopmentMode().unwrap_or(false),
        display_name: p.DisplayName().map(|s| s.to_string()).unwrap_or_default(),
        publisher: p.PublisherDisplayName().map(|s| s.to_string()).unwrap_or_default(),
    })
}

fn read_app_packages() -> windows::core::Result<Vec<AppPackage>> {
    use windows::Management::Deployment::PackageManager;
    com_init();
    let pm = PackageManager::new()?;
    // An empty user id is the current user, which needs no admin rights.
    let list = pm.FindPackagesByUserSecurityId(&windows::core::HSTRING::new())?;
    Ok(list.into_iter().filter_map(|p| package_of(&p)).collect())
}

/// This user's packages of a family (usually one; two while an update is half done).
pub fn installed(family: &str) -> Vec<AppPackage> {
    use windows::Management::Deployment::PackageManager;
    com_init();
    let r = (|| -> windows::core::Result<Vec<AppPackage>> {
        let pm = PackageManager::new()?;
        let list = pm.FindPackagesByUserSecurityIdPackageFamilyName(&windows::core::HSTRING::new(), &windows::core::HSTRING::from(family))?;
        Ok(list.into_iter().filter_map(|p| package_of(&p)).collect())
    })();
    r.unwrap_or_default()
}

fn deployment_error(what: &str, e: &windows::core::Error) -> anyhow::Error {
    let msg = e.message();
    let msg = msg.trim();
    if msg.is_empty() {
        anyhow!("{what}: Windows reported error 0x{:08X}", e.code().0 as u32)
    } else {
        anyhow!("{what}: {msg} (0x{:08X})", e.code().0 as u32)
    }
}

/// The result of a finished deployment operation.
fn finish(what: &str, r: windows::core::Result<windows::Management::Deployment::DeploymentResult>) -> Result<String> {
    let r = r.map_err(|e| deployment_error(what, &e))?;
    let code = r.ExtendedErrorCode().map(|c| c.0).unwrap_or(0);
    if code < 0 {
        let text = r.ErrorText().map(|t| t.to_string()).unwrap_or_default();
        bail!("{what}: {} (0x{:08X})", if text.trim().is_empty() { "Windows could not do it" } else { text.trim() }, code as u32);
    }
    Ok(String::new())
}

/// Install (or update to) the package in `file` for this user, after checking it (see the
/// module notes). `update`: close the app if it is still running (the job asked the user to
/// close it first); `any_version`: allow an older version (a roll back).
pub fn install(file: &Path, family: &str, signature_sha256: Option<&str>, update: bool, any_version: bool) -> Result<String> {
    let (id, sig) = read_package_file(file)?;
    check_identity(&id, sig.as_deref(), family, signature_sha256)?;
    if crate::winutil::authenticode_signer(file).is_none() {
        bail!("Windows does not trust the downloaded package's signature, so it was not installed");
    }
    // Windows reads a package by its extension.
    let ext = package_ext(&id, file);
    let have = file.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let path = if have == ext {
        file.to_path_buf()
    } else {
        let p = file.with_extension(ext);
        std::fs::copy(file, &p).with_context(|| format!("copy {}", file.display()))?;
        p
    };
    let r = add(&path, update, any_version);
    if path != file {
        let _ = std::fs::remove_file(&path);
    }
    r
}

fn add(path: &Path, update: bool, any_version: bool) -> Result<String> {
    use windows::Foundation::Uri;
    use windows::Management::Deployment::{AddPackageOptions, PackageManager};
    com_init();
    let what = "Windows could not install the package";
    let url = url::Url::from_file_path(path).map_err(|_| anyhow!("{what}: bad path {}", path.display()))?;
    let op = (|| {
        let pm = PackageManager::new()?;
        let uri = Uri::CreateUri(&windows::core::HSTRING::from(url.as_str()))?;
        let opts = AddPackageOptions::new()?;
        opts.SetForceAppShutdown(update)?;
        opts.SetForceUpdateFromAnyVersion(any_version)?;
        pm.AddPackageByUriAsync(&uri, &opts)?.join()
    })();
    finish(what, op)
}

/// Remove this user's packages of the family. Nothing to remove is fine.
pub fn remove(family: &str) -> Result<String> {
    use windows::Management::Deployment::PackageManager;
    let what = "Windows could not remove the package";
    for p in installed(family) {
        let r = PackageManager::new().and_then(|pm| pm.RemovePackageAsync(&windows::core::HSTRING::from(p.full_name.as_str()))?.join());
        finish(what, r)?;
    }
    if !installed(family).is_empty() {
        bail!("{what}: it is still installed");
    }
    Ok(String::new())
}

#[cfg(test)]
mod tests;
