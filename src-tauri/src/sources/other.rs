//! Other apps: general Windows apps matched to Microsoft's public winget catalog, which is read
//! directly (see `winget`). Opt-in, and kept apart from the fully handled apps: a match is made
//! only when it is certain, and an app whose catalog entry can not be installed silently over
//! the copy that is here is left out.
//!
//! Each uninstall list entry is matched by, strongest first:
//! 1. its product code: the uninstall key name (the MSI GUID, or the key any other installer writes)
//! 2. its MSI upgrade code, which stays the same from version to version
//! 3. winget's normalized display name and publisher, both equal
//! 4. its install folder or program name, among the packages of the same publisher
//!
//! A product or upgrade code match must also agree on the name or the publisher. When more than
//! one package is left, the locale and the name decide; when they can not, the entry is dropped.

use super::winget::{self, Index, Installer, Manifest, Pkg, VerRef};
use super::{msix, Download, Installed, Latest, Plan, SourceData, Step, StoreRef, Verify};
use crate::model::{CatalogEntry, JobKind, Source, VersionInfo};
use crate::{http, notes, paths, version, winutil};
use anyhow::{anyhow, bail, Result};
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const NO_NOTES: &str = "No change list is published for this release";
pub const SEE_NOTES: &str = "See the full release notes for the changes";

// ---------------------------------------------------------------- what is left out

/// Packages another source handles. A package matches its id and the ids below it
/// ("BlenderFoundation.Blender.LTS.4.5").
const HANDLED: &[&str] = &["BlenderFoundation.Blender", "OBSProject.OBSStudio", "KDE.Krita", "FreeCAD.FreeCAD", "GodotEngine.GodotEngine", "Unity.Unity", "Unity.UnityHub", "Adobe.CreativeCloud"];

/// Packages that update themselves, belong to Windows or are security software, with the reason.
const EXCLUDED: &[(&str, &str)] = &[
    ("Microsoft.Edge", "updates itself"),
    ("Microsoft.EdgeWebView2Runtime", "part of Windows"),
    ("Microsoft.Office", "updates itself"),
    ("Microsoft.Teams", "updates itself"),
    ("Microsoft.VisualStudio", "updated by the Visual Studio Installer"),
    ("Microsoft.WindowsSDK", "a Windows development kit"),
    ("Microsoft.WindowsADK", "a Windows development kit"),
    ("Microsoft.WindowsWDK", "a Windows development kit"),
    ("Microsoft.AppInstaller", "part of Windows"),
    ("Kaspersky", "security software"),
    ("ESET", "security software"),
    ("Bitdefender", "security software"),
    ("Avast", "security software"),
    ("AVG", "security software"),
    ("McAfee", "security software"),
    ("Norton", "security software"),
    ("NortonLifeLock", "security software"),
    ("Sophos", "security software"),
    ("TrendMicro", "security software"),
];

fn under(id: &str, root: &str) -> bool {
    id.eq_ignore_ascii_case(root) || (id.as_bytes().get(root.len()) == Some(&b'.') && id.get(..root.len()).is_some_and(|h| h.eq_ignore_ascii_case(root)))
}

pub fn handled_package(id: &str) -> bool {
    HANDLED.iter().any(|h| under(id, h))
}

pub fn excluded_package(id: &str) -> Option<&'static str> {
    EXCLUDED.iter().find(|(p, _)| under(id, p)).map(|(_, why)| *why)
}

/// Drivers are left to Windows Update and the hardware vendor.
fn is_driver(e: &winutil::UninstallEntry) -> bool {
    let pubr = winget::normalize_publisher(&e.publisher);
    let name = e.display_name.to_lowercase();
    ["nvidia", "advancedmicrodevices", "amd", "intel", "realtek", "realteksemiconductor"].contains(&pubr.as_str()) && (name.contains("driver") || name.contains("chipset") || name.contains("graphics"))
}

/// An entry that is an app of its own: shown in Add/Remove Programs, not an update of another
/// entry, and removable.
pub fn is_app(e: &winutil::UninstallEntry) -> bool {
    let rt = e.release_type.to_ascii_lowercase();
    !e.display_name.trim().is_empty()
        && !e.system_component
        && e.parent_key.is_empty()
        && !["update", "hotfix", "security update", "service pack"].contains(&rt.as_str())
        && (!e.uninstall_string.trim().is_empty() || !e.quiet_uninstall_string.trim().is_empty())
}

fn norm_path(p: &str) -> String {
    p.trim().trim_matches('"').trim_end_matches(['\\', '/']).to_ascii_lowercase()
}

/// An entry that one of the fully handled sources lists already.
fn handled_entry(e: &winutil::UninstallEntry, handled: &[Installed]) -> bool {
    if e.uninstall_string.contains("--sapCode=") {
        return true;
    }
    let loc = norm_path(&e.install_location);
    let key = e.key.to_ascii_lowercase();
    handled.iter().any(|i| {
        let dir = i.install_dir.as_ref().map(|d| norm_path(&d.to_string_lossy())).unwrap_or_default();
        (!loc.is_empty() && dir == loc)
            || (key.starts_with('{') && i.uninstall.as_ref().is_some_and(|u| u.iter().any(|a| a.to_ascii_lowercase().contains(&key))))
            || match &i.data {
                SourceData::Craft { msi_product_code: Some(c), .. } | SourceData::Blender { msi_product_code: Some(c), .. } => c.eq_ignore_ascii_case(&e.key),
                _ => false,
            }
            || (i.name.eq_ignore_ascii_case(e.display_name.trim()) && i.version == e.display_version)
    })
}

// ---------------------------------------------------------------- matching

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum How {
    /// A distinctive name the app is also published under, in one package only
    Name,
    Location,
    NamePublisher,
    UpgradeCode,
    ProductCode,
    /// An app package (MSIX) whose package family name the catalog lists for this package only
    PackageFamily,
}

impl How {
    pub fn label(self) -> &'static str {
        match self {
            How::ProductCode => "product code",
            How::UpgradeCode => "upgrade code",
            How::NamePublisher => "name and publisher",
            How::Location => "install folder",
            How::Name => "own name",
            How::PackageFamily => "package family name",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Match {
    pub entry: winutil::UninstallEntry,
    pub pkg: Pkg,
    pub how: How,
    pub upgrade_code: Option<String>,
    /// An app package (MSIX) this user has; its `entry` is made from the package
    pub package: Option<Box<msix::AppPackage>>,
}

/// Why an entry was not matched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Miss {
    /// Listed by one of the fully handled sources
    Handled,
    /// A package that is left out on purpose ("updates itself")
    Excluded(&'static str),
    NotInCatalog,
    /// Several packages fit and nothing tells them apart
    Ambiguous(Vec<String>),
    /// A code matched, but the package's names and publishers are different
    Unconfirmed(String),
    NoPublisher,
    /// No version in the uninstall list, so updates can not be told
    NoVersion,
    /// Another entry already matched this package
    Duplicate(String),
}

impl Miss {
    pub fn label(&self) -> String {
        match self {
            Miss::Handled => "handled by a fully supported source".into(),
            Miss::Excluded(w) => format!("left out: {w}"),
            Miss::NotInCatalog => "not in the winget catalog".into(),
            Miss::Ambiguous(c) => format!("ambiguous: {}", c.join(", ")),
            Miss::Unconfirmed(p) => format!("code matches {p} but name and publisher differ"),
            Miss::NoPublisher => "no publisher to confirm a name match".into(),
            Miss::NoVersion => "no version in the uninstall list".into(),
            Miss::Duplicate(p) => format!("another entry already matches {p}"),
        }
    }
}

#[derive(Default)]
pub struct Report {
    pub matches: Vec<Match>,
    pub misses: Vec<(winutil::UninstallEntry, Miss)>,
    /// System components, updates and entries without a name, not counted as apps
    pub hidden: usize,
}

struct Cand {
    row: i64,
    how: How,
}

/// Does the package agree with the entry on its name or its publisher?
fn agrees(idx: &Index, row: i64, name: &str, publisher: &str) -> (bool, bool) {
    let names = idx.names_of(row);
    let name_ok = !name.is_empty() && names.iter().any(|n| n == name || (name.len() >= 4 && n.len() >= 4 && (n.starts_with(name) || name.starts_with(n.as_str()))));
    let pub_ok = !publisher.is_empty() && idx.publishers_of(row).iter().any(|p| p == publisher);
    (name_ok, pub_ok)
}

/// Program names an entry gives through its install folder and program file.
fn location_names(e: &winutil::UninstallEntry) -> Vec<String> {
    let mut out = Vec::new();
    let mut push = |s: &str| {
        let n = winget::normalize_name(s).name;
        if n.len() >= 4 && !out.contains(&n) {
            out.push(n);
        }
    };
    if let Some(f) = Path::new(e.install_location.trim().trim_matches('"').trim_end_matches('\\')).file_name() {
        push(&f.to_string_lossy());
    }
    let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"');
    if icon.to_ascii_lowercase().ends_with(".exe") {
        if let Some(s) = Path::new(icon).file_stem() {
            push(&s.to_string_lossy());
        }
    }
    out
}

/// The language an app's name carries: a locale winget knows ("en-US"), or a bare language at the
/// end of the brackets ("Fox Browser (x64 de)").
fn locale_hint(name: &str, nn: &winget::NormName) -> Option<String> {
    if let Some(l) = &nn.locale {
        return Some(l.clone());
    }
    static BARE: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"\((?:[^()]*\s)?([A-Za-z]{2,3}(?:-[A-Za-z]{2,4})?)\)\s*$").unwrap());
    BARE.captures(name.trim()).map(|c| c[1].to_ascii_lowercase()).filter(|l| !["tm", "beta", "user", "x64", "x86"].contains(&l.as_str()))
}

/// Is this the package for that language? Packages per language end in it ("Fox.Browser.de") or
/// name it ("Fox Browser (en-US)").
fn locale_fits(p: &Pkg, l: &str) -> bool {
    let id = p.id.to_ascii_lowercase();
    let name = p.name.to_ascii_lowercase();
    id.ends_with(&format!(".{l}")) || name.contains(&format!("({l})")) || name.contains(&format!(" {l})"))
}

const CHANNELS: &[&str] = &["beta", "nightly", "dev", "canary", "preview", "insiders", "alpha", "ptb", "rc", "prerelease", "pre-release", "weekly"];

/// How well a package id fits the entry, from the parts of the id that name a release channel
/// ("Brave.Brave.Beta"), an architecture ("Discord.Discord.arm64") or a version series
/// ("Python.Python.3.12", "Maxon.ZBrush.2024"). None rules the package out.
fn id_fit(id: &str, e: &winutil::UninstallEntry, nn: &winget::NormName, publisher: &str) -> Option<i32> {
    let mut segs: Vec<String> = id.split('.').map(|s| s.to_ascii_lowercase()).collect();
    let name = e.display_name.to_ascii_lowercase();
    let mut fit = 0;
    if segs.first().is_some_and(|s| !publisher.is_empty() && winget::normalize_publisher(s) == publisher) {
        fit += 1;
    }
    // Channel: a beta package is not the stable app.
    if segs.iter().skip(1).any(|s| CHANNELS.contains(&s.as_str()) && !name.split(|c: char| !c.is_alphanumeric() && c != '-').any(|w| w == s)) {
        return None;
    }
    // Architecture.
    if let Some(last) = segs.last().cloned() {
        let entry_arch = match nn.arch {
            Some("X64") => Some("x64"),
            Some(_) => Some("x86"),
            None => None,
        };
        let arch = match last.as_str() {
            "x64" | "amd64" => Some("x64"),
            "x86" | "x32" | "32-bit" => Some("x86"),
            "arm64" | "arm" => Some("arm64"),
            _ => None,
        };
        if let Some(a) = arch {
            if a == "arm64" && machine_arch() != "arm64" {
                return None;
            }
            if let Some(ea) = entry_arch {
                if a != ea && a != "arm64" {
                    return None;
                }
                fit += 2;
            }
            segs.pop();
        }
    }
    // Version series: the trailing numbers of the id against the start of the installed version,
    // when they are the same kind of number (a year 2010 is not version 10).
    let mut nums: Vec<String> = Vec::new();
    while let Some(s) = segs.last() {
        if nums.len() < 2 && !s.is_empty() && s.replace('_', "").chars().all(|c| c.is_ascii_digit()) && segs.len() > 2 {
            nums.insert(0, segs.pop().unwrap_or_default());
        } else {
            break;
        }
    }
    if !nums.is_empty() {
        let series: Vec<u64> = nums.iter().flat_map(|s| s.split('_')).filter_map(|x| x.parse().ok()).collect();
        let have = version::parts(&e.display_version);
        let digits = |n: u64| n.to_string().len();
        if let (Some(s0), Some(h0)) = (series.first(), have.first()) {
            if digits(*s0) == digits(*h0) {
                if have.len() < series.len() || have[..series.len()] != series[..] {
                    return None;
                }
                fit += 3;
            }
        }
    }
    Some(fit)
}

fn is_guid(s: &str) -> bool {
    let t = s.trim();
    t.len() == 38 && t.starts_with('{') && t.ends_with('}')
}

/// The package row an entry matched, how, and its MSI upgrade code; or why it did not match.
type Matched = std::result::Result<(i64, How, Option<String>), Miss>;

/// Match one entry.
fn match_entry(e: &winutil::UninstallEntry, idx: &Index, upgrade_codes: &HashMap<String, String>) -> Matched {
    let nn = winget::normalize_name(&e.display_name);
    let publisher = winget::normalize_publisher(&e.publisher);
    let upgrade = if is_guid(&e.key) { upgrade_codes.get(&e.key.to_ascii_uppercase()).cloned() } else { None };
    let locale = locale_hint(&e.display_name, &nn);

    let mut tiers: Vec<Vec<Cand>> = Vec::new();
    tiers.push(idx.by_product_code(&e.key).into_iter().map(|row| Cand { row, how: How::ProductCode }).collect());
    if let Some(u) = &upgrade {
        tiers.push(idx.by_upgrade_code(u).into_iter().map(|row| Cand { row, how: How::UpgradeCode }).collect());
    }
    if !publisher.is_empty() {
        let by_pub = idx.by_publisher(&publisher);
        tiers.push(idx.by_name(&nn.name).into_iter().filter(|r| by_pub.binary_search(r).is_ok()).map(|row| Cand { row, how: How::NamePublisher }).collect());
        // Install folder and program name, among this publisher's packages (a publisher with a
        // few hundred packages is too broad for a guess like this).
        if by_pub.len() <= 300 {
            let names = location_names(e);
            if !names.is_empty() {
                tiers.push(by_pub.iter().copied().filter(|r| idx.names_of(*r).iter().any(|n| names.contains(n))).map(|row| Cand { row, how: How::Location }).collect());
            }
        }
        // An app published under its own name ("Bloxstrap" by "Bloxstrap") whose distinctive name
        // only one package has, and that package is named after it too.
        if publisher == nn.name && nn.name.len() >= 6 {
            let rows = idx.by_name(&nn.name);
            if let [row] = rows.as_slice() {
                let own = idx.by_row(*row).is_some_and(|p| p.id.rsplit('.').next().is_some_and(|last| winget::normalize_name(last).name == nn.name));
                if own {
                    tiers.push(vec![Cand { row: *row, how: How::Name }]);
                }
            }
        }
    } else {
        // No publisher to confirm the name: only a name one package has, which the install
        // folder or program file confirms.
        let rows = idx.by_name(&nn.name);
        if let [row] = rows.as_slice() {
            let names = location_names(e);
            if nn.name.len() >= 5 && idx.names_of(*row).iter().any(|n| names.contains(n)) {
                tiers.push(vec![Cand { row: *row, how: How::Location }]);
            }
        }
    }
    let mut unconfirmed = None;
    for tier in tiers {
        if tier.is_empty() {
            continue;
        }
        // Score what is left: the name counts most, then the publisher and the locale.
        let mut scored: Vec<(i32, &Cand)> = Vec::new();
        for c in &tier {
            let (name_ok, pub_ok) = agrees(idx, c.row, &nn.name, &publisher);
            if !name_ok && !pub_ok {
                if unconfirmed.is_none() {
                    unconfirmed = idx.by_row(c.row).map(|p| p.id);
                }
                continue;
            }
            let Some(p) = idx.by_row(c.row) else { continue };
            let Some(fit) = id_fit(&p.id, e, &nn, &publisher) else { continue };
            let mut s = if name_ok { 2 } else { 0 } + if pub_ok { 1 } else { 0 } + fit;
            if locale.as_deref().is_some_and(|l| locale_fits(&p, l)) {
                s += 4;
            }
            scored.push((s, c));
        }
        if scored.is_empty() {
            continue;
        }
        let top = scored.iter().map(|x| x.0).max().unwrap_or(0);
        let best: Vec<&Cand> = scored.iter().filter(|x| x.0 == top).map(|x| x.1).collect();
        if best.len() == 1 {
            return Ok((best[0].row, best[0].how, upgrade));
        }
        let ids: Vec<String> = best.iter().take(4).filter_map(|c| idx.by_row(c.row)).map(|p| p.id).collect();
        return Err(Miss::Ambiguous(ids));
    }
    if let Some(p) = unconfirmed {
        return Err(Miss::Unconfirmed(p));
    }
    if publisher.is_empty() && !idx.by_name(&nn.name).is_empty() {
        return Err(Miss::NoPublisher);
    }
    Err(Miss::NotInCatalog)
}

/// Match every app in the uninstall list against the index.
pub fn match_all(entries: &[winutil::UninstallEntry], idx: &Index, handled: &[Installed], upgrade_codes: &HashMap<String, String>) -> Report {
    match_all_with(entries, idx, handled, |e| match_entry(e, idx, upgrade_codes))
}

/// Matches already made against one copy of the index on disk, per uninstall list entry: a
/// rescan only matches what is new or changed.
type Memo = (Option<std::time::SystemTime>, HashMap<String, Matched>);
static MEMO: Lazy<Mutex<Memo>> = Lazy::new(|| Mutex::new((None, HashMap::new())));

/// `match_all` against the index file at `path`, remembering each entry's match.
pub fn match_on_disk(entries: &[winutil::UninstallEntry], idx: &Index, path: &Path, handled: &[Installed]) -> Report {
    let stamp = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let mut memo = MEMO.lock();
    if memo.0 != stamp {
        *memo = (stamp, HashMap::new());
    }
    // Reading the MSI upgrade codes walks a big part of the registry: only when needed.
    let mut ups: Option<HashMap<String, String>> = None;
    match_all_with(entries, idx, handled, |e| {
        let key = format!("{}|{}|{}|{}|{}|{}|{}|{}", e.hive, e.key, e.display_name, e.publisher, e.display_version, e.install_location, e.display_icon, e.windows_installer);
        if let Some(hit) = memo.1.get(&key) {
            return hit.clone();
        }
        let r = match_entry(e, idx, ups.get_or_insert_with(winutil::msi_upgrade_codes));
        memo.1.insert(key, r.clone());
        r
    })
}

fn match_all_with(entries: &[winutil::UninstallEntry], idx: &Index, handled: &[Installed], mut find: impl FnMut(&winutil::UninstallEntry) -> Matched) -> Report {
    let mut r = Report::default();
    for e in &fold_bundles(entries) {
        if !is_app(e) {
            r.hidden += 1;
            continue;
        }
        if handled_entry(e, handled) {
            r.misses.push((e.clone(), Miss::Handled));
            continue;
        }
        if is_driver(e) {
            r.misses.push((e.clone(), Miss::Excluded("a driver")));
            continue;
        }
        match find(e) {
            Ok((row, how, upgrade_code)) => {
                let Some(pkg) = idx.by_row(row) else { continue };
                if handled_package(&pkg.id) {
                    r.misses.push((e.clone(), Miss::Handled));
                } else if let Some(why) = excluded_package(&pkg.id) {
                    r.misses.push((e.clone(), Miss::Excluded(why)));
                } else if e.display_version.trim().is_empty() {
                    // No version in the list: the program file's own version, when it has one.
                    match program_file(e).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"))).and_then(|p| winutil::exe_version(&p)).filter(|v| v != "0.0.0.0") {
                        Some(v) => {
                            let mut e2 = e.clone();
                            e2.display_version = v;
                            r.matches.push(Match { entry: e2, pkg, how, upgrade_code, package: None });
                        }
                        None => r.misses.push((e.clone(), Miss::NoVersion)),
                    }
                } else {
                    r.matches.push(Match { entry: e.clone(), pkg, how, upgrade_code, package: None });
                }
            }
            Err(m) => r.misses.push((e.clone(), m)),
        }
    }
    // One entry per copy of a package: the strongest match, then the newest version. A copy for
    // this user and one for all users are two copies; entries of one scope, or with the same
    // install folder, are the same copy.
    r.matches.sort_by(|a, b| a.pkg.id.cmp(&b.pkg.id).then(b.how.cmp(&a.how)).then_with(|| version::cmp(&b.entry.display_version, &a.entry.display_version)));
    let mut kept: Vec<Match> = Vec::new();
    for m in std::mem::take(&mut r.matches) {
        if kept.iter().rev().take_while(|k| k.pkg.id == m.pkg.id).any(|k| same_copy(&k.entry, &m.entry)) {
            let id = m.pkg.id.clone();
            r.misses.push((m.entry, Miss::Duplicate(id)));
        } else {
            kept.push(m);
        }
    }
    r.matches = kept;
    r
}

/// The `hive` of an entry made from an app package (MSIX) rather than read from the uninstall
/// list. Its `key` is the package's full name.
pub const APPX: &str = "APPX";

/// The entry an app package stands for: its name, publisher, version, folder and date.
fn package_entry(p: &msix::AppPackage) -> winutil::UninstallEntry {
    winutil::UninstallEntry {
        key: p.full_name.clone(),
        hive: APPX,
        display_name: if p.display_name.trim().is_empty() || p.display_name.starts_with("ms-resource:") { p.name.clone() } else { p.display_name.clone() },
        display_version: msix::display_version(&p.version),
        publisher: p.publisher.clone(),
        install_location: p.install_dir.as_ref().map(|d| d.to_string_lossy().to_string()).unwrap_or_default(),
        written: p.installed_at,
        ..Default::default()
    }
}

/// This user's app packages (MSIX) matched to the catalog by their exact package family name,
/// when only one package has it. A package already matched from the uninstall list (`classic`),
/// listed by another source (a Store copy of Blender) or left out on purpose stays out.
pub fn package_matches(pkgs: &[msix::AppPackage], idx: &Index, classic: &[Match], handled: &[Installed]) -> Vec<Match> {
    let mut out: Vec<Match> = Vec::new();
    for p in pkgs {
        let rows = idx.by_family(&p.family);
        let row = match rows.as_slice() {
            [row] => *row,
            [] if p.store_signed() => match store_copy_of(p, idx) {
                Some(row) => row,
                None => continue,
            },
            _ => continue,
        };
        let Some(pkg) = idx.by_row(row) else { continue };
        if handled_package(&pkg.id) || excluded_package(&pkg.id).is_some() {
            continue;
        }
        if handled.iter().any(|h| h.store.as_ref().is_some_and(|s| s.family.eq_ignore_ascii_case(&p.family))) {
            continue;
        }
        if classic.iter().chain(out.iter()).any(|m| m.pkg.id.eq_ignore_ascii_case(&pkg.id)) {
            continue;
        }
        out.push(Match { entry: package_entry(p), pkg, how: How::PackageFamily, upgrade_code: None, package: Some(Box::new(p.clone())) });
    }
    out
}

/// A Store copy of an app the catalog lists only as its classic installer (the Store's
/// "Minecraft Launcher" is package Microsoft.4297127D64EC6, the catalog's Mojang.MinecraftLauncher
/// names no package): the one catalog app with exactly its name, when that app is not itself a
/// package of another family.
fn store_copy_of(p: &msix::AppPackage, idx: &Index) -> Option<i64> {
    let norm = winget::normalize_name(&p.display_name).name;
    if norm.chars().filter(|c| c.is_alphanumeric()).count() < 4 {
        return None;
    }
    let mut rows = idx.by_name(&norm);
    rows.sort_unstable();
    rows.dedup();
    let [row] = rows.as_slice() else { return None };
    let families = idx.families_of(*row);
    (families.is_empty() || families.iter().any(|f| f.eq_ignore_ascii_case(&p.family))).then_some(*row)
}

/// Every match: the uninstall list's, then this user's app packages.
fn all_matches(entries: &[winutil::UninstallEntry], idx: &Index, path: &Path, handled: &[Installed]) -> Vec<Match> {
    let mut ms = match_on_disk(entries, idx, path, handled).matches;
    let more = package_matches(&msix::app_packages(), idx, &ms, handled);
    ms.extend(more);
    ms
}

/// The Store page of a catalog entry ("other:<package id>") by the package family name the
/// index gives it.
pub fn store_ref_of(id: &str) -> Option<StoreRef> {
    let pkg = id.strip_prefix("other:")?;
    let idx = Index::open(&winget::index_path()).ok()?;
    let row = idx.by_id(pkg)?.row;
    let family = idx.families_of(row).into_iter().next()?;
    Some(StoreRef { family, app: None })
}

/// Is this installed copy an app package (MSIX)? Its package family name, when it is.
pub fn package_family(i: &Installed) -> Option<String> {
    match &i.data {
        SourceData::Other { how: How::PackageFamily, entry, .. } => Some(msix::family_of(&entry.key)),
        _ => None,
    }
}

/// Is this entry a WiX Burn bundle? Its uninstaller is the installer kept in the Package Cache.
fn is_bundle(e: &winutil::UninstallEntry) -> bool {
    e.bundle || (!e.windows_installer && e.uninstall_string.to_ascii_lowercase().contains("\\package cache\\") && e.uninstall_string.to_ascii_lowercase().contains("/uninstall"))
}

/// A Burn bundle and the Windows Installer packages it put on the PC (same name and version, same
/// scope) are one app. The bundle stays the one listed and removed (its uninstaller removes the
/// whole chain); its packages add what the bundle does not record: the install date, the install
/// folder and their product codes, which tell where the program is.
pub(crate) fn fold_bundles(entries: &[winutil::UninstallEntry]) -> Vec<winutil::UninstallEntry> {
    let same = |a: &winutil::UninstallEntry, b: &winutil::UninstallEntry| a.display_name.trim().eq_ignore_ascii_case(b.display_name.trim()) && a.display_version.trim() == b.display_version.trim() && entry_scope(a) == entry_scope(b);
    let mut out = entries.to_vec();
    for i in 0..out.len() {
        if !is_bundle(&out[i]) || out[i].display_name.trim().is_empty() {
            continue;
        }
        let parts: Vec<winutil::UninstallEntry> = entries.iter().filter(|p| !is_bundle(p) && p.windows_installer && is_guid(&p.key) && same(p, &out[i])).cloned().collect();
        let b = &mut out[i];
        for p in parts {
            if b.install_date.trim().is_empty() {
                b.install_date = p.install_date.clone();
            }
            if (b.install_location.trim().is_empty() || super::locate::bad_place(&b.install_location)) && !p.install_location.trim().is_empty() && !super::locate::bad_place(&p.install_location) {
                b.install_location = p.install_location.clone();
            }
            if super::locate::icon_exe(b).is_none() && super::locate::icon_exe(&p).is_some() {
                b.display_icon = p.display_icon.clone();
            }
            if !b.msi_parts.iter().any(|k| k.eq_ignore_ascii_case(&p.key)) {
                b.msi_parts.push(p.key.clone());
            }
        }
    }
    out
}

/// "user" for a copy registered for this user only, else "machine".
fn entry_scope(e: &winutil::UninstallEntry) -> &'static str {
    // App packages are installed for each user.
    if e.hive == "HKCU" || e.hive == APPX {
        "user"
    } else {
        "machine"
    }
}

/// The folder an entry names: its install location, else the folder of its icon program.
fn entry_folder(e: &winutil::UninstallEntry) -> String {
    if !e.install_location.trim().is_empty() {
        return norm_path(&e.install_location);
    }
    let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"');
    if icon.to_ascii_lowercase().ends_with(".exe") {
        if let Some(p) = Path::new(icon).parent() {
            return norm_path(&p.to_string_lossy());
        }
    }
    String::new()
}

/// Do two entries of one package describe the same installed copy?
fn same_copy(a: &winutil::UninstallEntry, b: &winutil::UninstallEntry) -> bool {
    if entry_scope(a) == entry_scope(b) {
        return true;
    }
    let (fa, fb) = (entry_folder(a), entry_folder(b));
    !fa.is_empty() && fa == fb
}

/// A package installed both for this user and for all users is listed twice: the copy for all
/// users keeps the plain id, the one for this user gets ":user", and both names say which is which.
fn split_copies(list: &mut [Installed]) {
    let scope_of = |i: &Installed| match &i.data {
        SourceData::Other { entry, .. } => entry_scope(entry),
        _ => "machine",
    };
    let both: Vec<String> = list
        .iter()
        .filter(|i| scope_of(i) == "user")
        .map(|i| pkg_of(i).to_string())
        .filter(|p| list.iter().any(|o| pkg_of(o) == p && scope_of(o) == "machine"))
        .collect();
    for i in list.iter_mut() {
        if !both.iter().any(|p| p == pkg_of(i)) {
            continue;
        }
        if scope_of(i) == "user" {
            i.id = format!("other:{}:user", pkg_of(i));
            i.name = format!("{} (this user)", i.name);
        } else {
            i.name = format!("{} (all users)", i.name);
        }
    }
}

// ---------------------------------------------------------------- choosing an installer

/// What kind of installer an update has to be to replace the copy that is here.
#[derive(Clone, Debug)]
pub struct Want {
    /// x64 or arm64
    pub machine: &'static str,
    /// x64 or x86 when the installed copy tells
    pub arch: Option<&'static str>,
    /// user or machine for an installed copy; None for a new install
    pub scope: Option<&'static str>,
    pub msi: bool,
    pub product_code: Option<String>,
    pub upgrade_code: Option<String>,
    pub locale: Option<String>,
    /// An installed app package (MSIX): its package family name. Only a package of that family
    /// updates it.
    pub package: Option<String>,
}

/// The processor Windows runs on ("arm64" even for this app running emulated as x64).
pub fn machine_arch() -> &'static str {
    winutil::native_arch()
}

fn starts_dir(p: &str, root: &Path) -> bool {
    let r = format!("{}\\", norm_path(&root.to_string_lossy()));
    format!("{}\\", norm_path(p)).starts_with(&r)
}

impl Want {
    pub fn for_entry(e: &winutil::UninstallEntry, upgrade_code: Option<&str>) -> Want {
        let nn = winget::normalize_name(&e.display_name);
        let loc = if e.install_location.trim().is_empty() { e.display_icon.split(',').next().unwrap_or("").to_string() } else { e.install_location.clone() };
        let arch = match nn.arch {
            Some("X64") => Some("x64"),
            Some(_) => Some("x86"),
            None if starts_dir(&loc, &paths::program_files_x86()) => Some("x86"),
            None if starts_dir(&loc, &paths::program_files()) && !paths::program_files().to_string_lossy().contains("(x86)") => Some("x64"),
            // Registered in the 32 bit view and installed outside Program Files: a 32 bit copy.
            None if e.hive == "HKLM32" => Some("x86"),
            None => None,
        };
        Want {
            machine: machine_arch(),
            arch,
            scope: Some(if e.hive == "HKCU" { "user" } else { "machine" }),
            msi: e.windows_installer,
            product_code: Some(e.key.clone()),
            upgrade_code: upgrade_code.map(String::from),
            locale: nn.locale,
            package: None,
        }
    }

    /// An installed app package, by its full name.
    pub fn for_package(full_name: &str) -> Want {
        let arch = match msix::arch_of(full_name).as_str() {
            "x64" => Some("x64"),
            "x86" => Some("x86"),
            _ => None,
        };
        Want { machine: machine_arch(), arch, scope: Some("user"), msi: false, product_code: None, upgrade_code: None, locale: None, package: Some(msix::family_of(full_name)) }
    }

    pub fn for_installed(i: &Installed) -> Want {
        match &i.data {
            SourceData::Other { how: How::PackageFamily, entry, .. } => Want::for_package(&entry.key),
            SourceData::Other { entry, upgrade_code, .. } => Want::for_entry(entry, upgrade_code.as_deref()),
            _ => Want::fresh(),
        }
    }

    /// A new install: the computer's own architecture is preferred, any scope.
    pub fn fresh() -> Want {
        Want { machine: machine_arch(), arch: None, scope: None, msi: false, product_code: None, upgrade_code: None, locale: None, package: None }
    }
}

/// The installer type that actually runs (a zip holds another installer).
pub fn kind_of(i: &Installer) -> &str {
    if i.kind == "zip" {
        i.nested_kind.as_deref().unwrap_or("zip")
    } else {
        &i.kind
    }
}

fn msi_family(k: &str) -> bool {
    k == "msi" || k == "wix"
}

const OTHER_CPU: &str = "only for another kind of processor";

/// Why a package that only the Microsoft Store installs can not be installed here.
pub const STORE_ONLY: &str = "only in the Microsoft Store";

/// MSIX and APPX packages, which Windows' package manager installs.
pub fn package_kind(k: &str) -> bool {
    k == "msix" || k == "appx"
}

/// Can this installer run here, silently? `Err` says why not.
pub fn usable(i: &Installer, machine: &str) -> std::result::Result<(), &'static str> {
    match i.arch.as_str() {
        "x64" | "x86" | "neutral" => {}
        "arm64" if machine == "arm64" => {}
        _ => return Err(OTHER_CPU),
    }
    match kind_of(i) {
        "msstore" => return Err(STORE_ONLY),
        "msix" | "appx" if i.kind == "zip" => return Err("an MSIX package inside a zip"),
        "msix" | "appx" if i.family.as_deref().is_none_or(|f| f.trim().is_empty()) => return Err("an MSIX package without its package family name"),
        "msix" | "appx" => {}
        "portable" => return Err("only a portable copy"),
        "pwa" | "font" => return Err("not a desktop app installer"),
        "zip" => return Err("a zip without an installer in it"),
        // A plain exe needs its silent switch. "InstallModes: silent" alone is often copied from a
        // template, and such an installer opens its window and waits for clicks.
        "exe" if i.silent.is_none() && i.silent_progress.is_none() => return Err("its installer has no silent mode"),
        "msi" | "wix" | "nsis" | "inno" | "burn" | "exe" => {}
        _ => return Err("an installer type that is not supported"),
    }
    if i.kind == "zip" && i.nested_path.is_none() {
        return Err("a zip without an installer in it");
    }
    if i.sha256.len() != 64 || !i.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("no checksum");
    }
    if !(i.url.starts_with("https://") || i.url.starts_with("http://")) {
        return Err("no download address");
    }
    Ok(())
}

/// The installer that updates the copy described by `w` (or installs a new one). Only installers
/// that replace that copy are considered: same scope, same kind (MSI or not), same architecture,
/// unless the installer names the installed product or upgrade code.
pub fn choose<'a>(m: &'a Manifest, w: &Want) -> std::result::Result<&'a Installer, String> {
    choose_with(m, w, &LEARNED.read())
}

/// `choose`, with what earlier installs showed about this package (`Learned`).
pub fn choose_with<'a>(m: &'a Manifest, w: &Want, learned: &Learned) -> std::result::Result<&'a Installer, String> {
    // Why nothing fits: a reason about this copy (scope, kind, architecture) says more than one
    // about installers that can not run here at all.
    let mut why: (u8, &str) = (0, "no installer for Windows");
    let mut best: Option<(i32, &Installer)> = None;
    let same = |a: &Option<String>, b: &Option<String>| matches!((a, b), (Some(x), Some(y)) if x.eq_ignore_ascii_case(y.trim()));
    for i in &m.installers {
        if let Err(r) = usable(i, w.machine) {
            let level = if r == OTHER_CPU { 1 } else { 2 };
            if level >= why.0 {
                why = (level, r);
            }
            continue;
        }
        let pc = same(&w.product_code, &i.product_code) || i.arp.iter().any(|a| same(&w.product_code, &a.product_code));
        let uc = w.upgrade_code.is_some() && i.arp.iter().any(|a| same(&w.upgrade_code, &a.upgrade_code));
        let k = kind_of(i);
        // An app package is only updated by a package of its own family, and a copy from a
        // normal installer never by a package (that would install a second copy).
        let is_pkg = package_kind(k);
        match &w.package {
            Some(fam) => {
                if !is_pkg {
                    why = (3, "no MSIX package, this copy is an app package");
                    continue;
                }
                if i.family.as_deref().is_some_and(|f| !f.eq_ignore_ascii_case(fam)) {
                    why = (3, "only an MSIX package of another family");
                    continue;
                }
            }
            None if is_pkg && w.product_code.is_some() => {
                why = (3, "only an MSIX package, this copy did not come from one");
                continue;
            }
            None => {}
        }
        // A key name such as "7-Zip" is the same for every architecture; a GUID is not.
        let named = uc || (pc && w.product_code.as_deref().is_some_and(is_guid));
        // The scope the installer really installs for, when an earlier install showed it differs
        // from the catalog's.
        let iscope = learned.real_scope(&m.id, i.scope.as_deref());
        if let (Some(ws), Some(is), None) = (w.scope, iscope, &w.package) {
            if ws != is && !named {
                why = (3, if ws == "user" { "only an installer for all users, this copy is per user" } else { "only a per-user installer, this copy is for all users" });
                continue;
            }
        }
        if w.product_code.is_some() && !named && w.msi != msi_family(k) && k != "burn" {
            why = (3, if w.msi { "no MSI installer, this copy came from one" } else { "only an MSI installer, this copy did not come from one" });
            continue;
        }
        if let Some(a) = w.arch {
            if !named && i.arch != a && i.arch != "neutral" && (a == "x86" || a == "x64") && (i.arch == "x86" || i.arch == "x64") {
                why = (3, "only for another architecture than this copy");
                continue;
            }
        }
        let mut s = 0;
        // A new install takes a normal installer over an app package when there are both.
        if is_pkg && w.package.is_none() {
            s -= 40;
        }
        if pc {
            s += 100;
        }
        if uc {
            s += 90;
        }
        if w.msi == msi_family(k) {
            s += 30;
        }
        s += match w.arch {
            Some(a) if a == i.arch => 20,
            _ if i.arch == w.machine => 15,
            _ if i.arch == "neutral" => 10,
            _ => 0,
        };
        s += match (w.scope, iscope) {
            (Some(a), Some(b)) if a == b => 10,
            (_, None) => 5,
            (None, Some("machine")) => 6,
            _ => 0,
        };
        s += match (&w.locale, &i.locale) {
            (Some(l), Some(il)) if il.eq_ignore_ascii_case(l) => 8,
            (_, None) => 3,
            (_, Some(il)) if il.eq_ignore_ascii_case("en-US") => 2,
            _ => 0,
        };
        if best.is_none_or(|(b, _)| s > b) {
            best = Some((s, i));
        }
    }
    best.map(|b| b.1).ok_or_else(|| why.1.to_string())
}

/// The version the app will show in the uninstall list after this installer: what the manifest
/// says it writes there, else the package version.
pub fn expected_version(m: &Manifest, i: &Installer) -> String {
    i.arp.iter().find_map(|a| a.display_version.clone()).unwrap_or_else(|| m.version.clone())
}

/// Is `offered` an update for `installed`? Only when both are plain version numbers of the same
/// kind (a year version "2026.1" is never compared with "8.3").
pub fn is_newer(offered: &str, installed: &str) -> bool {
    let plain = |v: &str| v.trim().trim_start_matches(['v', 'V']).chars().next().is_some_and(|c| c.is_ascii_digit());
    if !plain(offered) || !plain(installed) {
        return false;
    }
    let year = |v: &str| (1990..=2100).contains(&version::major(v));
    if year(offered) != year(installed) {
        return false;
    }
    super::tools::cmp_ver(offered, installed) == std::cmp::Ordering::Greater
}

// ---------------------------------------------------------------- feed

#[derive(Clone, Debug)]
pub struct PkgFeed {
    pub pkg: Pkg,
    /// Newest first
    pub versions: Vec<VerRef>,
    /// Manifest of the newest version
    pub latest: Option<Manifest>,
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    /// Lowercase package id -> what the catalog has for it
    pub pkgs: HashMap<String, PkgFeed>,
    /// Well-known apps offered in Get apps before any search (see `POPULAR`), with a short line
    /// about each; only those the catalog can install silently with a checksum.
    pub popular: Vec<(PkgFeed, &'static str)>,
    /// Some of `popular` were still loading when the feed was returned; read the feed again later.
    pub unfinished: bool,
}

/// How long a refresh waits for the starter list manifests (read as many at once as this PC is tuned for).
const POPULAR_BUDGET: Duration = Duration::from_secs(25);

/// Well-known apps that Get apps lists under Other apps without a search. Ids the catalog does not
/// have, apps another source handles, Store-only apps and installers that can not run silently
/// with a checksum drop out by themselves.
pub const POPULAR: &[(&str, &str)] = &[
    ("Google.Chrome", "Web browser"),
    ("Mozilla.Firefox", "Web browser"),
    ("Brave.Brave", "Web browser"),
    ("Opera.Opera", "Web browser"),
    ("Opera.OperaGX", "Browser for gamers"),
    ("Vivaldi.Vivaldi", "Web browser"),
    ("Zen-Team.Zen-Browser", "Web browser"),
    ("Waterfox.Waterfox", "Web browser"),
    ("LibreWolf.LibreWolf", "Private web browser"),
    ("TorProject.TorBrowser", "Private web browser"),
    ("Mozilla.Firefox.DeveloperEdition", "Browser for developers"),
    ("Ablaze.Floorp", "Web browser"),
    ("Mozilla.Thunderbird", "Email"),
    ("VideoLAN.VLC", "Video and music player"),
    ("Daum.PotPlayer", "Video player"),
    ("MPC-BE.MPC-BE", "Video player"),
    ("clsid2.mpc-hc", "Video player"),
    ("Jellyfin.JellyfinMediaPlayer", "Media player"),
    ("Plex.Plex", "Media player"),
    ("Plex.Plexamp", "Music player"),
    ("Spotify.Spotify", "Music"),
    ("AIMP.AIMP", "Music player"),
    ("Winamp.Winamp", "Music player"),
    ("Apple.iTunes", "Music and devices"),
    ("Telegram.TelegramDesktop", "Messaging"),
    ("Discord.Discord", "Voice and text chat"),
    ("SlackTechnologies.Slack", "Team chat"),
    ("OpenWhisperSystems.Signal", "Private messaging"),
    ("Zoom.Zoom", "Video meetings"),
    ("Element.Element", "Secure chat"),
    ("Mumble.Mumble.Client", "Voice chat"),
    ("TeamSpeakSystems.TeamSpeakClient", "Voice chat"),
    ("Guilded.Guilded", "Chat"),
    ("Vencord.Vesktop", "Discord client"),
    ("Cisco.Webex", "Video meetings"),
    ("Valve.Steam", "Games"),
    ("EpicGames.EpicGamesLauncher", "Games"),
    ("GOG.Galaxy", "Games"),
    ("ElectronicArts.EADesktop", "Games"),
    ("Ubisoft.Connect", "Games"),
    ("RiotGames.Valorant.EU", "Game"),
    ("PrismLauncher.PrismLauncher", "Minecraft launcher"),
    ("Playnite.Playnite", "Game library"),
    ("HeroicGamesLauncher.HeroicGamesLauncher", "Games"),
    ("Parsec.Parsec", "Game streaming"),
    ("MoonlightGameStreamingProject.Moonlight", "Game streaming"),
    ("LizardByte.Sunshine", "Game streaming host"),
    ("Modrinth.ModrinthApp", "Game mods"),
    ("Rainmeter.Rainmeter", "Desktop widgets"),
    ("voidtools.Everything", "Instant file search"),
    ("Microsoft.PowerToys", "Windows utilities"),
    ("AutoHotkey.AutoHotkey", "Automation scripts"),
    ("Flow-Launcher.Flow-Launcher", "App launcher"),
    ("lencx.ChatGPT", "Chat assistant"),
    ("Klocman.BulkCrapUninstaller", "Bulk uninstaller"),
    ("RevoUninstaller.RevoUninstaller", "Uninstaller"),
    ("BleachBit.BleachBit", "Disk cleaner"),
    ("Piriform.CCleaner", "PC cleaner"),
    ("Piriform.Recuva", "File recovery"),
    ("Piriform.Speccy", "System information"),
    ("CrystalDewWorld.CrystalDiskInfo", "Disk health"),
    ("CrystalDewWorld.CrystalDiskMark", "Disk speed test"),
    ("CPUID.CPU-Z", "System information"),
    ("CPUID.HWMonitor", "Hardware monitor"),
    ("TechPowerUp.GPU-Z", "Graphics card information"),
    ("REALiX.HWiNFO", "Hardware information"),
    ("Guru3D.Afterburner", "GPU tuning"),
    ("Guru3D.RTSS", "Frame rate overlay"),
    ("WinDirStat.WinDirStat", "Disk usage map"),
    ("AntibodySoftware.WizTree", "Disk usage map"),
    ("JAMSoftware.TreeSize.Free", "Disk usage"),
    ("CodeSector.TeraCopy", "Fast file copy"),
    ("Balena.Etcher", "Write disk images to USB"),
    ("7zip.7zip", "File archiver"),
    ("RARLab.WinRAR", "File archiver"),
    ("Giorgiotani.Peazip", "File archiver"),
    ("Bandisoft.Bandizip", "File archiver"),
    ("Notepad++.Notepad++", "Text and code editor"),
    ("Microsoft.VisualStudioCode", "Code editor"),
    ("Microsoft.VisualStudioCode.Insiders", "Code editor preview"),
    ("VSCodium.VSCodium", "Code editor"),
    ("SublimeHQ.SublimeText.4", "Text editor"),
    ("SublimeHQ.SublimeMerge", "Git client"),
    ("Neovim.Neovim", "Terminal editor"),
    ("vim.vim", "Terminal editor"),
    ("GNU.Emacs", "Text editor"),
    ("JetBrains.IntelliJIDEA.Community", "Java IDE"),
    ("JetBrains.IntelliJIDEA.Ultimate", "Java IDE"),
    ("JetBrains.PyCharm.Community", "Python IDE"),
    ("JetBrains.PyCharm.Professional", "Python IDE"),
    ("JetBrains.WebStorm", "JavaScript IDE"),
    ("JetBrains.Rider", ".NET IDE"),
    ("JetBrains.GoLand", "Go IDE"),
    ("JetBrains.CLion", "C and C++ IDE"),
    ("JetBrains.PhpStorm", "PHP IDE"),
    ("JetBrains.RustRover", "Rust IDE"),
    ("JetBrains.DataGrip", "Database IDE"),
    ("JetBrains.Toolbox", "JetBrains apps"),
    ("Google.AndroidStudio", "Android IDE"),
    ("Apache.NetBeans", "Java IDE"),
    ("Anysphere.Cursor", "AI code editor"),
    ("Git.Git", "Version control"),
    ("GitHub.GitHubDesktop", "Git client"),
    ("GitHub.cli", "GitHub command line"),
    ("Axosoft.GitKraken", "Git client"),
    ("TortoiseGit.TortoiseGit", "Git client"),
    ("Atlassian.Sourcetree", "Git client"),
    ("Fork.Fork", "Git client"),
    ("OpenJS.NodeJS.LTS", "JavaScript runtime"),
    ("OpenJS.NodeJS", "JavaScript runtime"),
    ("Yarn.Yarn", "Package manager"),
    ("Python.Python.3.13", "Python"),
    ("Python.Python.3.12", "Python"),
    ("Rustlang.Rustup", "Rust toolchain"),
    ("GoLang.Go", "Go language"),
    ("Microsoft.OpenJDK.21", "Java"),
    ("EclipseAdoptium.Temurin.21.JDK", "Java"),
    ("EclipseAdoptium.Temurin.17.JDK", "Java"),
    ("Oracle.JavaRuntimeEnvironment", "Java runtime"),
    ("Microsoft.DotNet.SDK.9", ".NET SDK"),
    ("Microsoft.DotNet.SDK.8", ".NET SDK"),
    ("Microsoft.DotNet.Runtime.8", ".NET runtime"),
    ("Microsoft.DotNet.DesktopRuntime.8", ".NET desktop runtime"),
    ("Microsoft.DotNet.DesktopRuntime.9", ".NET desktop runtime"),
    ("Microsoft.VCRedist.2015+.x64", "C++ runtime"),
    ("RubyInstallerTeam.Ruby.3.4", "Ruby"),
    ("StrawberryPerl.StrawberryPerl", "Perl"),
    ("Kitware.CMake", "Build tool"),
    ("LLVM.LLVM", "Compiler"),
    ("MSYS2.MSYS2", "Build tools"),
    ("Docker.DockerDesktop", "Containers"),
    ("Hashicorp.Vagrant", "Virtual machines"),
    ("Oracle.VirtualBox", "Virtual machines"),
    ("Postman.Postman", "API tool"),
    ("Insomnia.Insomnia", "API tool"),
    ("Bruno.Bruno", "API tool"),
    ("DBeaver.DBeaver.Community", "Database tool"),
    ("TablePlus.TablePlus", "Database tool"),
    ("HeidiSQL.HeidiSQL", "Database tool"),
    ("PostgreSQL.PostgreSQL.17", "Database"),
    ("MongoDB.Compass.Full", "MongoDB tool"),
    ("Oracle.MySQL", "Database"),
    ("WinSCP.WinSCP", "File transfer"),
    ("PuTTY.PuTTY", "SSH client"),
    ("Termius.Termius", "SSH client"),
    ("Alacritty.Alacritty", "Terminal"),
    ("wez.wezterm", "Terminal"),
    ("Warp.Warp", "Terminal"),
    ("WinMerge.WinMerge", "File comparison"),
    ("Meld.Meld", "File comparison"),
    ("Insecure.Nmap", "Network scanner"),
    ("Telerik.Fiddler.Classic", "Web debugging"),
    ("Obsidian.Obsidian", "Notes"),
    ("Notion.Notion", "Notes and documents"),
    ("Logseq.Logseq", "Notes"),
    ("Joplin.Joplin", "Notes"),
    ("Evernote.Evernote", "Notes"),
    ("AppFlowy.AppFlowy", "Notes"),
    ("Figma.Figma", "Design tool"),
    ("Figma.FigmaAgent", "Design fonts helper"),
    ("Canva.Canva", "Design tool"),
    ("GIMP.GIMP.3", "Image editor"),
    ("GIMP.GIMP", "Image editor"),
    ("dotPDN.PaintDotNet", "Image editor"),
    ("Inkscape.Inkscape", "Vector graphics"),
    ("darktable.darktable", "Photo editor"),
    ("RawTherapee.RawTherapee", "Photo editor"),
    ("Pinta.Pinta", "Image editor"),
    ("IrfanSkiljan.IrfanView", "Image viewer"),
    ("XnSoft.XnViewMP", "Image viewer"),
    ("ImageMagick.ImageMagick", "Image tools"),
    ("Autodesk.Fusion", "3D design"),
    ("Prusa3D.PrusaSlicer", "3D printing"),
    ("Ultimaker.Cura", "3D printing"),
    ("OpenSCAD.OpenSCAD", "3D modeling"),
    ("Wings3D.Wings3D", "3D modeling"),
    ("KDE.Kdenlive", "Video editor"),
    ("OpenShot.OpenShot", "Video editor"),
    ("HandBrake.HandBrake", "Video converter"),
    ("MediaArea.MediaInfo.GUI", "Media file details"),
    ("Audacity.Audacity", "Audio editor"),
    ("LMMS.LMMS", "Music production"),
    ("Cockos.REAPER", "Audio workstation"),
    ("VB-Audio.Voicemeeter.Banana", "Audio mixer"),
    ("ShareX.ShareX", "Screenshots and screen recording"),
    ("Greenshot.Greenshot", "Screenshots"),
    ("Flameshot.Flameshot", "Screenshots"),
    ("Elgato.StreamDeck", "Stream controller"),
    ("Elgato.4KCaptureUtility", "Capture card tool"),
    ("Elgato.ControlCenter", "Elgato lights"),
    ("TheDocumentFoundation.LibreOffice", "Office suite"),
    ("Apache.OpenOffice", "Office suite"),
    ("ONLYOFFICE.DesktopEditors", "Office suite"),
    ("SumatraPDF.SumatraPDF", "PDF reader"),
    ("Adobe.Acrobat.Reader.64-bit", "PDF reader"),
    ("Foxit.FoxitReader", "PDF reader"),
    ("PDFgear.PDFgear", "PDF tools"),
    ("PDFsam.PDFsam", "PDF tools"),
    ("calibre.calibre", "E-book manager"),
    ("Bitwarden.Bitwarden", "Password manager"),
    ("KeePassXCTeam.KeePassXC", "Password manager"),
    ("DominikReichl.KeePass", "Password manager"),
    ("Proton.ProtonVPN", "VPN"),
    ("Proton.ProtonMail", "Email"),
    ("Proton.ProtonDrive", "Cloud storage"),
    ("Cloudflare.Warp", "Private DNS and VPN"),
    ("Tailscale.Tailscale", "Private network"),
    ("WireGuard.WireGuard", "VPN"),
    ("OpenVPNTechnologies.OpenVPNConnect", "VPN"),
    ("ZeroTier.ZeroTierOne", "Virtual network"),
    ("AnyDesk.AnyDesk", "Remote desktop"),
    ("TeamViewer.TeamViewer", "Remote desktop"),
    ("Dropbox.Dropbox", "Cloud storage"),
    ("Google.GoogleDrive", "Cloud storage"),
    ("Microsoft.OneDrive", "Cloud storage"),
    ("Mega.MEGASync", "Cloud storage"),
    ("Nextcloud.NextcloudDesktop", "Cloud storage"),
    ("SyncTrayzor.SyncTrayzor", "File sync"),
    ("Box.Box", "Cloud storage"),
    ("qBittorrent.qBittorrent", "Torrent client"),
    ("Transmission.Transmission", "Torrent client"),
    ("DelugeTeam.Deluge", "Torrent client"),
    ("Malwarebytes.Malwarebytes", "Malware scanner"),
    ("SUPERAntiSpyware.SUPERAntiSpyware", "Malware scanner"),
    ("CodecGuide.K-LiteCodecPack.Standard", "Video codecs"),
    ("RedHat.Podman-Desktop", "Containers"),
    ("Google.CloudSDK", "Google Cloud tools"),
    ("Amazon.AWSCLI", "AWS command line"),
    ("Microsoft.AzureCLI", "Azure command line"),
    ("Microsoft.Azure.StorageExplorer", "Azure storage"),
    ("Cloudflare.cloudflared", "Cloudflare tunnels"),
    ("Foxit.PhantomPDF", "PDF editor"),
    ("Nvidia.CUDA", "GPU computing"),
    ("Logitech.GHUB", "Logitech gear"),
    ("Logitech.OptionsPlus", "Logitech gear"),
    ("Corsair.iCUE.5", "Corsair gear"),
    ("SteelSeries.GG", "SteelSeries gear"),
    ("Asus.ArmouryCrate", "ASUS tools"),
    ("Ditto.Ditto", "Clipboard manager"),
    ("Open-Shell.Open-Shell-Menu", "Start menu"),
    ("Nilesoft.Shell", "Context menus"),
    ("XnSoft.XnConvert", "Image converter"),
    ("nomacs.nomacs", "Image viewer"),
    ("Microsoft.MouseWithoutBorders", "Mouse sharing"),
    ("Deskflow.Deskflow", "Mouse sharing"),
    ("Garmin.Express", "Garmin devices"),
    ("Samsung.SmartSwitch", "Samsung phones"),
    ("Mojang.MinecraftLauncher", "Minecraft"),
    ("Glarysoft.GlaryUtilities", "PC tools"),
    ("IObit.AdvancedSystemCare", "PC tools"),
    ("IObit.DriverBooster", "Drivers"),
    ("Wagnardsoft.DisplayDriverUninstaller", "Graphics driver cleanup"),
    ("Cryptomator.Cryptomator", "Cloud encryption"),
    ("WinFsp.WinFsp", "File systems"),
    ("Hex-Rays.IDA.Free", "Disassembler"),
    ("icsharpcode.ILSpy", ".NET decompiler"),
    ("Starship.Starship", "Shell prompt"),
    ("Microsoft.Bicep", "Azure templates"),
    ("Microsoft.SQLServerManagementStudio", "SQL tool"),
    ("notable.notable", "Notes"),
    ("MarkText.MarkText", "Markdown editor"),
    ("Zettlr.Zettlr", "Markdown editor"),
    ("LyX.LyX", "Document editor"),
    ("MiKTeX.MiKTeX", "LaTeX"),
    ("TeXstudio.TeXstudio", "LaTeX editor"),
    ("Anki.Anki", "Flashcards"),
    ("GeoGebra.Classic", "Math"),
    ("RProject.R", "R language"),
    ("Anaconda.Anaconda3", "Python distribution"),
    ("Anaconda.Miniconda3", "Python distribution"),
    ("Ollama.Ollama", "Run AI models"),
    ("Jan.Jan", "Run AI models"),
    ("nomic.gpt4all", "Run AI models"),
    ("Perplexity.Comet", "AI browser"),
    ("Maxthon.Maxthon", "Web browser"),
    ("Yandex.Browser", "Web browser"),
    ("360.360Chrome", "Web browser"),
    ("MoonchildProductions.PaleMoon", "Web browser"),
    ("Mozilla.SeaMonkey", "Web suite"),
    ("Postbox.Postbox", "Email"),
    ("Mailbird.Mailbird", "Email"),
    ("Tutanota.Tutanota", "Private email"),
    ("Tonec.InternetDownloadManager", "Download manager"),
    ("Loom.Loom", "Screen recording"),
    ("Asana.Asana", "Tasks"),
    ("ClickUp.ClickUp", "Tasks"),
    ("Miro.Miro", "Whiteboard"),
    ("JGraph.Draw", "Diagrams"),
    ("Xmind.Xmind", "Mind maps"),
    ("Zoho.Mail", "Email"),
    ("GoTo.GoToMeeting", "Video meetings"),
    ("XBMCFoundation.Kodi", "Media center"),
    ("PeterPawlowski.foobar2000", "Music player"),
    ("Rakuten.Viber", "Messaging"),
    ("Kakao.KakaoTalk", "Messaging"),
    ("Overwolf.CurseForge", "Game mods"),
    ("GitExtensionsTeam.GitExtensions", "Git client"),
    ("RedisInsight.RedisInsight", "Redis tool"),
    ("Mobatek.MobaXterm", "Remote terminal"),
    ("Iterate.Cyberduck", "File transfer"),
    ("WiresharkFoundation.Wireshark", "Network analyzer"),
    ("AnyAssociation.Anytype", "Notes"),
    ("DigitalScholar.Zotero", "Research library"),
    ("Doist.Todoist", "To-do list"),
    ("Appest.TickTick", "To-do list"),
    ("sylikc.JPEGView", "Image viewer"),
    ("CNRISTI.MeshLab", "3D mesh tool"),
    ("Bambulab.Bambustudio", "3D printing"),
    ("JannisX11.Blockbench", "Low poly 3D models"),
    ("eTeks.SweetHome3D", "Home design"),
    ("Meltytech.Shotcut", "Video editor"),
    ("MoritzBunkus.MKVToolNix", "Video file tools"),
    ("OliveTeam.OliveVideoEditor", "Video editor"),
    ("LWKS.Lightworks", "Video editor"),
    ("ByteDance.CapCut", "Video editor"),
    ("TenacityTeam.Tenacity", "Audio editor"),
    ("VB-Audio.Voicemeeter", "Audio mixer"),
    ("NGWIN.PicPick", "Screenshots"),
    ("Skillbrains.Lightshot", "Screenshots"),
    ("ImageOptim.gifski", "GIF maker"),
    ("Streamlabs.Streamlabs", "Streaming"),
    ("Kingsoft.WPSOffice", "Office suite"),
    ("SoftMaker.FreeOffice.2024", "Office suite"),
    ("geeksoftwareGmbH.PDF24Creator", "PDF tools"),
    ("TrackerSoftware.PDF-XChangeEditor", "PDF editor"),
    ("Amazon.Kindle", "E-book reader"),
    ("NordSecurity.NordVPN", "VPN"),
    ("MullvadVPN.MullvadVPN", "VPN"),
    ("MullvadVPN.MullvadBrowser", "Private web browser"),
    ("Surfshark.Surfshark", "VPN"),
    ("Twingate.Client", "Remote access"),
    ("Google.ChromeRemoteDesktopHost", "Remote desktop"),
    ("pCloudAG.pCloudDrive", "Cloud storage"),
    ("AppWork.JDownloader", "Download manager"),
    ("SoftDeluxe.FreeDownloadManager", "Download manager"),
    ("agalwood.Motrix", "Download manager"),
    ("OpenRGB.OpenRGB", "RGB lighting"),
    ("Wacom.WacomTabletDriver", "Drawing tablet"),
    ("hluk.CopyQ", "Clipboard manager"),
    ("FlorianHeidenreich.Mp3tag", "Music tags"),
    ("BartelsMedia.ShareMouse", "Mouse sharing"),
    ("DebaucheeOpenSourceGroup.Barrier", "Mouse sharing"),
    ("GnuPG.Gpg4win", "Encryption"),
    ("IDRIX.VeraCrypt", "Disk encryption"),
    ("MHNexus.HxD", "Hex editor"),
    ("appmakes.Typora", "Markdown editor"),
    ("GNU.Octave", "Math"),
    ("ProjectJupyter.JupyterLab", "Notebooks"),
    ("ElementLabs.LMStudio", "Run AI models"),
    ("FlashPeak.Slimjet", "Web browser"),
    ("Foundry376.Mailspring", "Email"),
    ("Formagrid.Airtable", "Database"),
    ("ZedIndustries.Zed", "Code editor"),
    ("shinchiro.mpv", "Video player"),
    ("mpv.net", "Video player"),
    ("winaero.tweaker", "Windows tweaks"),
    ("Nvidia.GeForceNow", "Cloud gaming"),
    ("Garmin.BaseCamp", "Maps and routes"),
    ("Maximus5.ConEmu", "Terminal"),
    ("XnSoft.XnView.Classic", "Image viewer"),
    ("TortoiseSVN.TortoiseSVN", "Version control"),
    ("TortoiseHg.TortoiseHg", "Version control"),
    ("Trimble.SketchUp.2026", "3D modeling"),
];

impl Feed {
    /// A package's versions and newest manifest: read for an installed app, or for the starter
    /// list (an app just installed from Get apps is not in `pkgs` until the next refresh).
    pub fn get(&self, id: &str) -> Option<&PkgFeed> {
        self.pkgs.get(&id.to_ascii_lowercase()).or_else(|| self.popular.iter().map(|(p, _)| p).find(|p| p.pkg.id.eq_ignore_ascii_case(id)))
    }

    /// The feed with one more package read (an app installed from a search result).
    pub fn with_package(&self, pf: PkgFeed) -> Feed {
        let mut f = self.clone();
        f.pkgs.insert(pf.pkg.id.to_ascii_lowercase(), pf);
        f
    }
}

/// For each installed app with an update: "<app id>@<new version>" and the installer address,
/// so the download size can be asked from the server (the catalog does not list sizes).
pub fn update_urls(installed: &[Installed], feed: &Feed) -> Vec<(String, String)> {
    installed
        .iter()
        .filter(|i| i.source == Source::Other)
        .filter_map(|i| {
            let m = feed.get(pkg_of(i))?.latest.as_ref()?;
            let inst = choose(m, &Want::for_installed(i)).ok()?;
            let offered = offered_update(i, feed)?;
            Some((format!("{}@{}", i.id, offered), inst.url.clone()))
        })
        .collect()
}

pub async fn download_size(url: &str) -> Option<u64> {
    remote_size(url).await
}

pub(crate) fn pkg_of(i: &Installed) -> &str {
    match &i.data {
        SourceData::Other { pkg, .. } => pkg,
        _ => "",
    }
}

pub(crate) type Loaded = (Pkg, Result<(Vec<VerRef>, Manifest)>);

pub(crate) async fn load_pkg(p: Pkg) -> Loaded {
    let r = async {
        let v = winget::versions(&p).await?;
        let first = v.first().ok_or_else(|| anyhow!("{} has no versions", p.id))?;
        let m = winget::manifest(first).await?;
        Ok((v, m))
    }
    .await;
    (p, r)
}

/// The starter list packages the catalog has. A package that is also in the Store (it has a
/// package family name) stays: most of them (Firefox, Slack, PowerToys) have a normal installer
/// too. `choose` drops the ones that only have a Store or MSIX package once their manifest is read.
pub fn popular_candidates(idx: &Index) -> Vec<(Pkg, &'static str)> {
    POPULAR.iter().filter(|(id, _)| !handled_package(id) && excluded_package(id).is_none()).filter_map(|(id, line)| idx.by_id(id).map(|p| (p, *line))).collect()
}

/// The packages an earlier feed read, by lowercase id.
fn read_before(prev: Option<&Feed>) -> HashMap<String, &PkgFeed> {
    let Some(f) = prev else { return HashMap::new() };
    f.popular.iter().map(|(p, _)| p).chain(f.pkgs.values()).filter(|p| p.latest.is_some()).map(|p| (p.pkg.id.to_ascii_lowercase(), p)).collect()
}

/// A package as an earlier check read it, when its index row still names the same version list
/// (by its checksum): then its versions and newest manifest are the same too, and nothing needs
/// reading again. The row itself is the new one.
fn reuse(before: &HashMap<String, &PkgFeed>, p: &Pkg) -> Option<Loaded> {
    let f = before.get(&p.id.to_ascii_lowercase())?;
    let m = f.latest.clone()?;
    (f.pkg.hash == p.hash && !p.hash.is_empty()).then(|| (p.clone(), Ok((f.versions.clone(), m))))
}

/// `load_pkg`, or the package as `before` has it when it did not change.
async fn load_or_reuse(before: &HashMap<String, &PkgFeed>, p: Pkg) -> Loaded {
    match reuse(before, &p) {
        Some(l) => l,
        None => load_pkg(p).await,
    }
}

/// Refresh the index, match what is installed and read the version list and newest manifest of
/// every matched package. `installed` are the apps the other sources found; `prev` is the feed of
/// the check before, whose packages are kept as they are when the index did not change them.
pub async fn fetch(installed: &[Installed], prev: Option<&Feed>) -> Result<Feed> {
    let path = winget::refresh_index(Duration::from_secs(30 * 60)).await?;
    let handled: Vec<Installed> = installed.iter().filter(|i| i.source != Source::Other).cloned().collect();
    let (pkgs, popular): (Vec<Pkg>, Vec<(Pkg, &'static str)>) = tokio::task::spawn_blocking(move || -> Result<(Vec<Pkg>, Vec<(Pkg, &'static str)>)> {
        let idx = Index::open(&path)?;
        let ms = all_matches(&winutil::uninstall_entries(), &idx, &path, &handled);
        let popular = popular_candidates(&idx);
        Ok((ms.into_iter().map(|m| m.pkg).collect(), popular))
    })
    .await
    .map_err(|e| anyhow!("{e}"))??;
    let before = read_before(prev);
    let got: Vec<Loaded> = futures_util::stream::iter(pkgs).map(|p| load_or_reuse(&before, p)).buffer_unordered(crate::machine::tuned().manifest_parallel).collect().await;
    let lines: HashMap<String, &'static str> = popular.iter().map(|(p, l)| (p.id.to_ascii_lowercase(), *l)).collect();
    let pop_pkgs: Vec<Pkg> = popular.into_iter().map(|(p, _)| p).collect();
    // The version lists and manifests are kept on disk by their checksum, so only the first start
    // downloads them all. That start must not hold Get apps back: what is not read within the
    // budget keeps loading in the background and shows up with the next refresh.
    let total = pop_pkgs.len();
    let mut pop: Vec<Loaded> = Vec::new();
    {
        let mut set = tokio::task::JoinSet::new();
        let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(crate::machine::tuned().popular_parallel));
        for p in pop_pkgs {
            if let Some(l) = reuse(&before, &p) {
                pop.push(l);
                continue;
            }
            let gate = gate.clone();
            set.spawn(async move {
                let _slot = gate.acquire_owned().await;
                load_pkg(p).await
            });
        }
        let deadline = tokio::time::Instant::now() + POPULAR_BUDGET;
        while let Ok(Some(done)) = tokio::time::timeout_at(deadline, set.join_next()).await {
            if let Ok(l) = done {
                pop.push(l);
            }
        }
        // Tasks still running are detached so their downloads finish and are kept.
        set.detach_all();
    }
    let mut feed = Feed { unfinished: pop.len() < total, ..Feed::default() };
    for (pkg, r) in pop {
        // Only what can be installed silently with a checksum here.
        if let Ok((versions, m)) = r {
            if choose(&m, &Want::fresh()).is_ok() {
                let line = lines.get(&pkg.id.to_ascii_lowercase()).copied().unwrap_or("");
                feed.popular.push((PkgFeed { pkg, versions, latest: Some(m) }, line));
            }
        }
    }
    // The list's own order (well-known apps first, grouped by kind), not the order the downloads finished.
    feed.popular.sort_by_key(|(p, _)| POPULAR.iter().position(|(id, _)| id.eq_ignore_ascii_case(&p.pkg.id)).unwrap_or(usize::MAX));
    let mut failed = 0;
    for (pkg, r) in got {
        match r {
            Ok((versions, m)) => {
                feed.pkgs.insert(pkg.id.to_ascii_lowercase(), PkgFeed { pkg, versions, latest: Some(m) });
            }
            Err(e) => {
                failed += 1;
                log::warn!("winget {}: {e:#}", pkg.id);
            }
        }
    }
    if failed > 0 && feed.pkgs.is_empty() {
        bail!("Could not read the winget catalog");
    }
    // Old version lists and manifests: looked for once a day, not on every check.
    static PRUNED: Mutex<Option<std::time::Instant>> = parking_lot::const_mutex(None);
    let due = PRUNED.lock().is_none_or(|t| t.elapsed() > Duration::from_secs(24 * 3600));
    if due {
        *PRUNED.lock() = Some(std::time::Instant::now());
        tokio::task::spawn_blocking(|| winget::prune_files(Duration::from_secs(60 * 24 * 3600))).await.ok();
    }
    Ok(feed)
}

/// One package's versions and newest manifest, read from the catalog (kept on disk after the
/// first time).
pub async fn read_package(id: &str) -> Option<PkgFeed> {
    let path = winget::index_path();
    let id = id.to_string();
    let pkg = tokio::task::spawn_blocking(move || Index::open(&path).ok().and_then(|i| i.by_id(&id))).await.ok().flatten()?;
    match load_pkg(pkg).await {
        (pkg, Ok((versions, m))) => Some(PkgFeed { pkg, versions, latest: Some(m) }),
        (pkg, Err(e)) => {
            log::warn!("winget {}: {e:#}", pkg.id);
            None
        }
    }
}

/// What the icon search needs about a package: the manifest's icon and its web pages.
#[derive(Clone, Debug)]
pub struct IconSource {
    pub icon: Option<String>,
    pub pages: Vec<String>,
}

fn icon_source(m: &Manifest) -> IconSource {
    IconSource { icon: m.icon_url.clone(), pages: [m.package_url.clone(), m.publisher_url.clone()].into_iter().flatten().filter(|u| u.starts_with("http")).collect() }
}

/// The icon sources of a catalog package: from the feed when it was read there, else its newest
/// manifest (searched apps); the manifests are kept on disk, so this is cheap after the first time.
pub async fn icon_source_of(feed: Option<&Feed>, id: &str) -> Option<IconSource> {
    if let Some(f) = feed {
        if let Some(m) = f.get(id).and_then(|p| p.latest.as_ref()).or_else(|| f.popular.iter().find(|(p, _)| p.pkg.id.eq_ignore_ascii_case(id)).and_then(|(p, _)| p.latest.as_ref())) {
            return Some(icon_source(m));
        }
    }
    let path = winget::index_path();
    let id = id.to_string();
    let pkg = tokio::task::spawn_blocking(move || Index::open(&path).ok().and_then(|i| i.by_id(&id))).await.ok().flatten()?;
    let (_, r) = load_pkg(pkg).await;
    r.ok().map(|(_, m)| icon_source(&m))
}

// ---------------------------------------------------------------- installed apps

const MARKS: &[(&str, &str)] = &[("#E4E4EC", "#24242C"), ("#E38BC4", "#2A1222"), ("#E8736A", "#2C1311"), ("#6FB1E3", "#0F2131"), ("#9FD69B", "#14261A"), ("#E3C26F", "#2B2311"), ("#B49BE8", "#1E1530"), ("#7FD1C7", "#10262A")];

/// Two letters and colors for the app's mark until its icon is known.
fn mark_for(name: &str, id: &str) -> (String, String, String) {
    let letters: Vec<char> = name.chars().filter(|c| c.is_alphanumeric()).collect();
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let code = if words.len() >= 2 {
        format!("{}{}", words[0].chars().next().unwrap_or('A').to_ascii_uppercase(), words[1].chars().next().unwrap_or('p').to_ascii_lowercase())
    } else {
        let mut s = String::new();
        s.extend(letters.first().map(|c| c.to_ascii_uppercase()));
        s.extend(letters.get(1).map(|c| c.to_ascii_lowercase()));
        s
    };
    let h = id.bytes().fold(7u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32)) as usize;
    let (fg, bg) = MARKS[h % MARKS.len()];
    (if code.is_empty() { "Ap".into() } else { code }, fg.into(), bg.into())
}

/// A program file named by the entry, outside Windows Installer's and bundles' caches.
fn program_file(e: &winutil::UninstallEntry) -> Option<PathBuf> {
    let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"');
    let low = icon.to_ascii_lowercase();
    if !(low.ends_with(".exe") || low.ends_with(".ico")) || low.contains("\\installer\\") || low.contains("package cache") {
        return None;
    }
    let p = PathBuf::from(icon);
    p.exists().then_some(p)
}

/// The command that removes the app without questions: the one the entry gives, Windows
/// Installer's for MSI products, or the uninstaller with the silent switch its type takes.
pub fn uninstall_cmd(e: &winutil::UninstallEntry, kind: Option<&str>) -> Option<Vec<String>> {
    let has = |v: &[String], s: &str| v.iter().any(|a| a.eq_ignore_ascii_case(s));
    if !e.quiet_uninstall_string.trim().is_empty() {
        let mut v = winutil::split_program_line(&e.quiet_uninstall_string);
        // Inno Setup registers "/SILENT", which still shows a progress window.
        if e.key.to_ascii_lowercase().ends_with("_is1") && has(&v, "/SILENT") {
            v.retain(|a| !a.eq_ignore_ascii_case("/SILENT"));
            for s in ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"] {
                if !has(&v, s) {
                    v.push(s.into());
                }
            }
        }
        return Some(v).filter(|v| !v.is_empty());
    }
    let u = e.uninstall_string.trim();
    if u.is_empty() {
        return None;
    }
    if (e.windows_installer || u.to_ascii_lowercase().contains("msiexec")) && is_guid(&e.key) {
        return Some(vec!["msiexec.exe".into(), "/x".into(), e.key.clone(), "/qn".into(), "/norestart".into()]);
    }
    let mut v = winutil::split_program_line(u);
    if v.is_empty() {
        return None;
    }
    if e.key.to_ascii_lowercase().ends_with("_is1") || kind == Some("inno") {
        for s in ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"] {
            if !has(&v, s) {
                v.push(s.into());
            }
        }
        return Some(v);
    }
    if kind == Some("nsis") {
        if !has(&v, "/S") {
            v.push("/S".into());
        }
        // Run in place, so it returns when it is done instead of handing off to a copy of itself.
        if let Some(dir) = Path::new(&v[0]).parent() {
            if !v.iter().any(|a| a.starts_with("_?=")) {
                v.push(format!("_?={}", dir.display()));
            }
        }
        return Some(v);
    }
    None
}

/// When the app was installed or last updated: the date the entry records, else when its entry
/// was last written (Burn bundles and some installers record no date).
fn entry_date(e: &winutil::UninstallEntry) -> Option<String> {
    winutil::install_date_iso(e.install_date.trim()).or_else(|| e.written.map(|w| chrono::DateTime::<chrono::Utc>::from(w).format("%Y-%m-%dT%H:%M:%SZ").to_string()))
}

/// The program file and install folder of a matched entry (see `locate`). An `.ico` the entry
/// names still serves as the icon when no program file is found.
fn find_program(e: &winutil::UninstallEntry, hints: &std::cell::OnceCell<super::locate::Hints>) -> (Option<PathBuf>, Option<PathBuf>) {
    let mut msi = e.msi_parts.clone();
    if e.windows_installer && is_guid(&e.key) {
        msi.insert(0, e.key.clone());
    }
    // The Start menu is read only when an app does not name its program.
    let (exe, dir) = if super::locate::icon_exe(e).is_some() { super::locate::locate(e, &msi, &super::locate::Hints::default()) } else { super::locate::locate(e, &msi, hints.get_or_init(read_hints)) };
    (exe.or_else(|| program_file(e).filter(|p| !p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")))), dir)
}

#[cfg(not(test))]
fn read_hints() -> super::locate::Hints {
    super::locate::Hints::read()
}

/// Tests never read this PC's Start menu.
#[cfg(test)]
fn read_hints() -> super::locate::Hints {
    super::locate::Hints::default()
}

/// One match as an installed app, without shared location hints (tests use it).
#[cfg(test)]
fn make(m: &Match, feed: Option<&Feed>) -> Option<Installed> {
    make_with(m, feed, &std::cell::OnceCell::new())
}

fn make_with(m: &Match, feed: Option<&Feed>, hints: &std::cell::OnceCell<super::locate::Hints>) -> Option<Installed> {
    if let Some(p) = &m.package {
        return make_package(m, p, feed);
    }
    let e = &m.entry;
    let pf = feed.and_then(|f| f.get(&m.pkg.id));
    let man = pf.and_then(|p| p.latest.as_ref());
    let want = Want::for_entry(e, m.upgrade_code.as_deref());
    // Once the catalog is read: an app it can not update silently is not listed.
    let kind = match man {
        Some(man) => Some(kind_of(choose(man, &want).ok()?).to_string()),
        None => None,
    };
    let name = man.map(|x| x.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| m.pkg.name.clone());
    let (code, fg, bg) = mark_for(&name, &m.pkg.id);
    let (exe, dir) = find_program(e, hints);
    Some(Installed {
        id: format!("other:{}", m.pkg.id),
        source: Source::Other,
        name,
        code,
        fg,
        bg,
        version: e.display_version.trim().to_string(),
        install_dir: dir,
        exe,
        uninstall: uninstall_cmd(e, kind.as_deref()),
        updated_at: entry_date(e),
        size_hint: Some(e.estimated_size_kb * 1024).filter(|s| *s > 0),
        tagline: hand_off_client(&m.pkg.id, "").map(|c| format!("Updated by the {c}")).or_else(|| man.and_then(|x| x.description.clone())).or_else(|| Some(e.publisher.clone()).filter(|p| !p.is_empty())),
        channel: None,
        missing: false,
        read_only: false,
        store: None,
        icon_remote: None,
        data: SourceData::Other { pkg: m.pkg.id.clone(), how: m.how, entry: Box::new(e.clone()), upgrade_code: m.upgrade_code.clone() },
    })
}

/// The installed app for an app package (MSIX). One the Microsoft Store installed is listed for
/// the Store to update; a sideloaded one is updated and removed here, and like other apps only
/// listed when the catalog has a package of its family to update it with.
fn make_package(m: &Match, p: &msix::AppPackage, feed: Option<&Feed>) -> Option<Installed> {
    let man = feed.and_then(|f| f.get(&m.pkg.id)).and_then(|pf| pf.latest.as_ref());
    let store = p.store_signed();
    if let (Some(man), false) = (man, store) {
        choose(man, &Want::for_package(&p.full_name)).ok()?;
    }
    let name = man.map(|x| x.name.clone()).filter(|n| !n.is_empty()).unwrap_or_else(|| m.pkg.name.clone());
    let (code, fg, bg) = mark_for(&name, &m.pkg.id);
    let (app, exe, logo) = p.install_dir.as_deref().map(msix::read_visual).unwrap_or_default();
    let tagline = if store { Some("Updated by the Microsoft Store".to_string()) } else { man.and_then(|x| x.description.clone()).or_else(|| Some(m.entry.publisher.clone()).filter(|p| !p.is_empty())) };
    Some(Installed {
        id: format!("other:{}", m.pkg.id),
        source: Source::Other,
        name,
        code,
        fg,
        bg,
        version: m.entry.display_version.clone(),
        install_dir: p.install_dir.clone(),
        exe: exe.filter(|e| e.is_file()),
        uninstall: None,
        updated_at: entry_date(&m.entry),
        size_hint: None,
        tagline,
        channel: None,
        missing: false,
        read_only: store,
        store: Some(StoreRef { family: p.family.clone(), app }),
        icon_remote: logo.map(|l| l.to_string_lossy().to_string()),
        data: SourceData::Other { pkg: m.pkg.id.clone(), how: How::PackageFamily, entry: Box::new(m.entry.clone()), upgrade_code: None },
    })
}

/// Installed apps matched to the catalog (no network: the index on disk, if there is one yet).
pub fn scan(entries: &[winutil::UninstallEntry], handled: &[Installed], feed: Option<&Feed>) -> Vec<Installed> {
    let path = winget::index_path();
    if !path.is_file() {
        return vec![];
    }
    let idx = match Index::open(&path) {
        Ok(i) => i,
        Err(e) => {
            log::warn!("winget index: {e:#}");
            return vec![];
        }
    };
    installed_copies(&all_matches(entries, &idx, &path, handled), feed)
}

/// The installed apps for these matches, with a distinct id for each copy.
fn installed_copies(matches: &[Match], feed: Option<&Feed>) -> Vec<Installed> {
    installed_copies_with(matches, feed, &std::cell::OnceCell::new())
}

/// `installed_copies`, with the shortcuts and App Paths given (read once, when an app needs them).
fn installed_copies_with(matches: &[Match], feed: Option<&Feed>, hints: &std::cell::OnceCell<super::locate::Hints>) -> Vec<Installed> {
    let mut v: Vec<Installed> = matches.iter().filter_map(|m| make_with(m, feed, hints)).collect();
    split_copies(&mut v);
    v
}

pub fn latest(inst: &Installed, feed: &Feed) -> Option<Latest> {
    let m = feed.get(pkg_of(inst))?.latest.as_ref()?;
    // Its maker's client keeps it up to date: running the package again only opens the client.
    if let Some(client) = m.installers.first().and_then(|i| hand_off_client(&m.id, &i.url)) {
        return Some(Latest { version: inst.version.clone(), size: None, note: Some(format!("Updated by the {client}")) });
    }
    choose(m, &Want::for_installed(inst)).ok()?;
    match offered_update(inst, feed) {
        Some(v) => Some(Latest { version: v, size: None, note: None }),
        None => Some(Latest { version: inst.version.clone(), size: None, note: None }),
    }
}

/// The version an installed app is offered, if any: what the newest package version writes to
/// the uninstall list, when that is newer than what the app shows and the app is not already at
/// that package version (the catalog's range of uninstall list versions for it, or what an
/// earlier install of it showed). Without that an app that writes "6.7.0" for package 6.7.0.3
/// would be offered 6.7.0.3 again after every update.
pub fn offered_update(inst: &Installed, feed: &Feed) -> Option<String> {
    let pf = feed.get(pkg_of(inst))?;
    let m = pf.latest.as_ref()?;
    let i = choose(m, &Want::for_installed(inst)).ok()?;
    let offered = expected_version(m, i);
    let range = arp_range(pf, &m.version);
    let learned = learned_shows(&pf.pkg.id, &m.version);
    (is_newer(&offered, &inst.version) && !at_version(&inst.version, &offered, range.as_ref(), learned.as_deref())).then_some(offered)
}

/// The uninstall list versions the catalog lists for a package version (winget's ARP range).
pub fn arp_range(pf: &PkgFeed, ver: &str) -> Option<(String, String)> {
    if let Some(r) = pf.versions.iter().find(|r| r.version == ver) {
        if let (Some(lo), Some(hi)) = (&r.arp_min, &r.arp_max) {
            return Some((lo.clone(), hi.clone()));
        }
    }
    // The index row gives the range of its newest version.
    if pf.pkg.latest == ver {
        return Some((pf.pkg.arp_min.clone()?, pf.pkg.arp_max.clone()?));
    }
    None
}

/// Does an app showing `shown` in the uninstall list have the package version that writes
/// `expected` (with the catalog's `range` for it, and what an earlier install of it showed)?
pub fn at_version(shown: &str, expected: &str, range: Option<&(String, String)>, learned: Option<&str>) -> bool {
    !version::newer(expected, shown) || range.is_some_and(|(lo, hi)| !version::newer(lo, shown) && !version::newer(shown, hi)) || learned.is_some_and(|l| version::cmp(l, shown).is_eq())
}

/// "6.7.0" for "6.7.0.3": the same version with its last parts left off, as some installers
/// write it to the uninstall list.
pub fn truncated(shown: &str, expected: &str) -> bool {
    let (d, e) = (version::parts(shown), version::parts(expected));
    let plain = shown.trim().chars().next().is_some_and(|c| c.is_ascii_digit());
    plain && !d.is_empty() && d.len() < e.len() && e[..d.len()] == d[..]
}

// ---------------------------------------------------------------- what installs showed

/// What installs showed that the catalog does not say, kept on disk: the uninstall list version a
/// package version really writes, and the scope an installer really installs for.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Learned {
    /// "<package id, lower case>@<package version>" -> version in the uninstall list
    #[serde(default)]
    pub versions: std::collections::BTreeMap<String, String>,
    /// package id, lower case -> (scope the catalog gives, scope it really installs for)
    #[serde(default)]
    pub scopes: std::collections::BTreeMap<String, (String, String)>,
}

impl Learned {
    fn key(pkg: &str, ver: &str) -> String {
        format!("{}@{}", pkg.to_ascii_lowercase(), ver)
    }

    pub fn shows(&self, pkg: &str, ver: &str) -> Option<&str> {
        self.versions.get(&Self::key(pkg, ver)).map(String::as_str)
    }

    pub fn set_shows(&mut self, pkg: &str, ver: &str, shown: &str) {
        self.versions.insert(Self::key(pkg, ver), shown.to_string());
    }

    /// The scope an installer of `pkg` declared as `declared` really installs for.
    pub fn real_scope<'a>(&'a self, pkg: &str, declared: Option<&'a str>) -> Option<&'a str> {
        match (self.scopes.get(&pkg.to_ascii_lowercase()), declared) {
            (Some((from, to)), Some(d)) if from == d => Some(to.as_str()),
            _ => declared,
        }
    }

    pub fn set_scope(&mut self, pkg: &str, declared: &str, real: &str) {
        self.scopes.insert(pkg.to_ascii_lowercase(), (declared.to_string(), real.to_string()));
    }
}

static LEARNED: Lazy<parking_lot::RwLock<Learned>> = Lazy::new(|| parking_lot::RwLock::new(load_learned()));

#[cfg(not(test))]
fn learned_file() -> PathBuf {
    paths::data_dir().join("other-learned.json")
}

#[cfg(not(test))]
fn load_learned() -> Learned {
    std::fs::read(learned_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

// Tests never read or write the app's own file.
#[cfg(test)]
fn load_learned() -> Learned {
    Learned::default()
}

/// Remember what an install showed, on disk.
pub fn learn(f: impl FnOnce(&mut Learned)) {
    let mut l = LEARNED.write();
    f(&mut l);
    #[cfg(not(test))]
    if let Ok(b) = serde_json::to_vec_pretty(&*l) {
        let tmp = learned_file().with_extension("json.tmp");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, learned_file());
        }
    }
}

pub fn learned_shows(pkg: &str, ver: &str) -> Option<String> {
    LEARNED.read().shows(pkg, ver).map(String::from)
}

// ---------------------------------------------------------------- checking an install

/// One copy of a package in the uninstall list, whatever the catalog offers for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PkgCopy {
    pub id: String,
    /// user or machine
    pub scope: &'static str,
    pub version: String,
    /// Its uninstall entry was written after the install began.
    pub rewritten: bool,
}

/// The copies of a package in the uninstall list, matched like a rescan does but without leaving
/// out the ones the catalog can not update (an install can move an app to another scope).
pub fn package_copies(pkg: &str, entries: &[winutil::UninstallEntry], handled: &[Installed], since: std::time::SystemTime) -> Vec<PkgCopy> {
    let path = winget::index_path();
    let Ok(idx) = Index::open(&path) else { return vec![] };
    let ms: Vec<Match> = all_matches(entries, &idx, &path, handled).into_iter().filter(|m| m.pkg.id.eq_ignore_ascii_case(pkg)).collect();
    copies_of(&ms, since)
}

fn copies_of(ms: &[Match], since: std::time::SystemTime) -> Vec<PkgCopy> {
    installed_copies(ms, None)
        .into_iter()
        .filter_map(|i| match &i.data {
            SourceData::Other { entry, .. } => Some(PkgCopy { id: i.id.clone(), scope: entry_scope(entry), version: i.version.clone(), rewritten: entry.written.is_some_and(|w| w >= since) }),
            _ => None,
        })
        .collect()
}

/// What an install of package version `pkg_version` was expected to leave.
#[derive(Clone, Debug, Default)]
pub struct Expect {
    /// The copy the install was for ("other:<package>" or "other:<package>:user")
    pub app_id: String,
    pub pkg: String,
    pub pkg_version: String,
    /// What the package version writes to the uninstall list (`expected_version`)
    pub expected: String,
    pub range: Option<(String, String)>,
    pub learned: Option<String>,
    /// The scope the catalog gives the installer that ran
    pub declared_scope: Option<String>,
    /// The copy's version before the install (None for a new install)
    pub before: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    /// Installed. `shows`: the uninstall list version this package version writes, to remember;
    /// `scope`: (catalog scope, real scope) when the installer installed for another scope.
    Done { shows: Option<String>, scope: Option<(String, String)> },
    Missing,
    Shows(String),
}

/// Did the install leave what was expected? The copy is found by its id, or, when it is gone, as
/// the one copy whose entry the install wrote (an installer that moved the app to another
/// scope). A copy that shows the expected version with its last parts left off ("6.7.0" for
/// "6.7.0.3") counts when the install wrote its entry or changed its version.
pub fn check_install(x: &Expect, copies: &[PkgCopy]) -> Check {
    let found = copies.iter().find(|c| c.id == x.app_id).or_else(|| {
        let w: Vec<&PkgCopy> = copies.iter().filter(|c| c.rewritten).collect();
        (w.len() == 1).then(|| w[0])
    });
    let Some(c) = found else { return Check::Missing };
    let at = at_version(&c.version, &x.expected, x.range.as_ref(), x.learned.as_deref());
    let short = !at && truncated(&c.version, &x.expected) && (c.rewritten || x.before.as_deref().is_none_or(|b| b != c.version));
    if !at && !short {
        return Check::Shows(c.version.clone());
    }
    let scope = x.declared_scope.as_deref().filter(|d| *d != c.scope).map(|d| (d.to_string(), c.scope.to_string()));
    Check::Done { shows: short.then(|| c.version.clone()), scope }
}

/// Remember what a finished install showed (see `Check::Done`).
pub fn learn_from(x: &Expect, check: &Check) {
    if let Check::Done { shows, scope } = check {
        if shows.is_none() && scope.is_none() {
            return;
        }
        learn(|l| {
            if let Some(v) = shows {
                l.set_shows(&x.pkg, &x.pkg_version, v);
            }
            if let Some((from, to)) = scope {
                l.set_scope(&x.pkg, from, to);
            }
        });
    }
}

/// The scope the catalog gives the installer an update of this copy (or a new install) uses.
pub fn declared_scope(feed: Option<&Feed>, pkg: &str, pkg_version: &str, current: Option<&Installed>) -> Option<String> {
    let m = feed?.get(pkg)?.latest.as_ref().filter(|m| m.version == pkg_version)?;
    let w = current.map(Want::for_installed).unwrap_or_else(Want::fresh);
    choose_with(m, &w, &Learned::default()).ok()?.scope.clone()
}

// ---------------------------------------------------------------- Get apps

pub(crate) fn entry(p: &Pkg, version: String, tagline: Option<String>, installed: bool) -> CatalogEntry {
    let (code, fg, bg) = mark_for(&p.name, &p.id);
    CatalogEntry {
        id: format!("other:{}", p.id),
        source: Source::Other,
        name: p.name.clone(),
        code,
        fg,
        bg,
        icon_url: None,
        version,
        size: None,
        tagline,
        description: None,
        blocked: None,
        group: Source::Other.group().into(),
        prerelease: false,
        installed,
        pick_version: true,
    }
}

/// The installed apps the catalog matched, so their pages and versions can be opened, and the
/// well-known apps (`POPULAR`) not installed yet.
pub fn catalog_entries(feed: &Feed, installed: &[Installed]) -> Vec<CatalogEntry> {
    let mut out: Vec<CatalogEntry> = installed
        .iter()
        .filter(|i| i.source == Source::Other)
        .filter_map(|i| {
            let pf = feed.get(pkg_of(i))?;
            let m = pf.latest.as_ref();
            let line = super::browse::tagline_for(m.and_then(|m| m.description.as_deref()), super::browse::popular_line(&pf.pkg.id), m.map(|m| m.publisher.as_str()).or(i.tagline.as_deref()));
            let mut e = entry(&pf.pkg, m.map(|m| m.version.clone()).unwrap_or_else(|| pf.pkg.latest.clone()), line, true);
            e.description = m.and_then(|m| m.long_description.clone());
            e.name = i.name.trim_end_matches(" (all users)").trim_end_matches(" (this user)").to_string();
            Some(e)
        })
        .collect();
    // A package installed for this user and for all users has one page.
    let mut seen = HashSet::new();
    out.retain(|e| seen.insert(e.id.to_ascii_lowercase()));
    for (pf, line) in &feed.popular {
        let id = format!("other:{}", pf.pkg.id);
        if out.iter().any(|e| e.id.eq_ignore_ascii_case(&id)) || installed.iter().any(|i| pkg_of(i).eq_ignore_ascii_case(&pf.pkg.id)) {
            continue;
        }
        let m = pf.latest.as_ref();
        let short = super::browse::tagline_for(m.and_then(|m| m.description.as_deref()), Some(*line).filter(|l| !l.is_empty()), m.map(|m| m.publisher.as_str()));
        let mut e = entry(&pf.pkg, m.map(|m| m.version.clone()).unwrap_or_else(|| pf.pkg.latest.clone()), short, false);
        e.description = m.and_then(|m| m.long_description.clone());
        if let Some(n) = m.map(|m| m.name.clone()).filter(|n| !n.is_empty()) {
            e.name = n;
        }
        out.push(e);
    }
    out
}

// ---------------------------------------------------------------- versions and change lists

struct GhRel {
    tag: String,
    name: String,
    body: String,
    url: String,
}

/// "owner/repo" of a github.com address.
pub fn github_repo(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("https://github.com/").or_else(|| url.strip_prefix("http://github.com/")).or_else(|| url.strip_prefix("https://www.github.com/"))?;
    let mut it = rest.split(['/', '?', '#']);
    let owner = it.next().filter(|s| !s.is_empty())?;
    let repo = it.next().filter(|s| !s.is_empty())?.trim_end_matches(".git");
    if ["orgs", "sponsors", "topics", "users", "apps", "marketplace", "settings"].contains(&owner) {
        return None;
    }
    Some((owner.to_string(), repo.to_string()))
}

/// The release tag in a GitHub download or release page address.
pub fn github_tag(url: &str) -> Option<String> {
    for mark in ["/releases/download/", "/releases/tag/"] {
        if let Some(i) = url.find(mark) {
            let t = url[i + mark.len()..].split(['/', '?', '#']).next()?;
            if !t.is_empty() {
                return Some(percent_decode(t));
            }
        }
    }
    None
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn github_releases(owner: &str, repo: &str) -> Vec<GhRel> {
    let url = format!("https://api.github.com/repos/{owner}/{repo}/releases?per_page=50");
    let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(15), http::get_cached(&url, &[("Accept", "application/vnd.github+json")], Duration::from_secs(3600))).await else { return vec![] };
    let v: Value = serde_json::from_slice(&f.body).unwrap_or(Value::Null);
    v.as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["draft"].as_bool() != Some(true))
        .map(|r| GhRel {
            tag: r["tag_name"].as_str().unwrap_or("").to_string(),
            name: r["name"].as_str().unwrap_or("").to_string(),
            body: r["body"].as_str().unwrap_or("").to_string(),
            url: r["html_url"].as_str().unwrap_or("").to_string(),
        })
        .collect()
}

fn find_release<'a>(rels: &'a [GhRel], ver: &str, tag: Option<&str>) -> Option<&'a GhRel> {
    if let Some(t) = tag {
        if let Some(r) = rels.iter().find(|r| r.tag == t) {
            return Some(r);
        }
    }
    rels.iter().find(|r| notes::names_version(&r.tag, ver)).or_else(|| rels.iter().find(|r| notes::names_version(&r.name, ver)))
}

/// What the versions of a package share: its GitHub releases (read at most once per version
/// list) and a release notes page that covers every version.
struct Gh {
    repo: Option<(String, String)>,
    rels: tokio::sync::OnceCell<Vec<GhRel>>,
    page: Option<String>,
}

impl Gh {
    async fn get(&self) -> &[GhRel] {
        match &self.repo {
            Some((o, r)) => self.rels.get_or_init(|| github_releases(o, r)).await,
            None => &[],
        }
    }
}

/// The GitHub repository a package comes from: where its installers are downloaded from, else
/// its release notes or home page.
fn repo_of(m: &Manifest) -> Option<(String, String)> {
    m.installers
        .iter()
        .find_map(|i| github_repo(&i.url).filter(|_| i.url.contains("/releases/download/")))
        .or_else(|| [&m.release_notes_url, &m.package_url, &m.publisher_url].into_iter().flatten().find_map(|u| github_repo(u)))
        .or_else(|| notes_known::repo_for(&m.id).map(|(o, r)| (o.to_string(), r.to_string())))
}

/// The notes on a GitHub or Codeberg release page ("/releases/tag/<tag>"): the release text of
/// the page the catalog names, read from the page itself (GitHub) or Codeberg's API.
async fn release_page_notes(url: &str) -> Vec<String> {
    if let Some(rest) = url.strip_prefix("https://codeberg.org/") {
        let mut it = rest.splitn(5, '/');
        let (Some(o), Some(r), Some("releases"), Some("tag"), Some(tag)) = (it.next(), it.next(), it.next(), it.next(), it.next()) else { return vec![] };
        let api = format!("https://codeberg.org/api/v1/repos/{o}/{r}/releases/tags/{tag}");
        let Some(body) = notes::fetch_page(&api).await else { return vec![] };
        let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
        return v["body"].as_str().map(notes::text_lines).unwrap_or_default();
    }
    if github_repo(url).is_none() || !url.contains("/releases/tag/") {
        return vec![];
    }
    notes::page(url).await.map(|html| github_release_body(&html)).unwrap_or_default()
}

/// The release text on a GitHub release page: its "markdown-body" block, up to the assets list.
pub fn github_release_body(html: &str) -> Vec<String> {
    let Some(start) = html.find("class=\"markdown-body") else { return vec![] };
    let rest = &html[start..];
    let end = ["data-test-selector=\"release-assets\"", "<details", "class=\"Box-footer", "id=\"repo-content-pjax-container\""].iter().filter_map(|m| rest.find(m)).min().unwrap_or(rest.len());
    let mut cut = end.min(400_000);
    while !rest.is_char_boundary(cut) {
        cut -= 1;
    }
    let lines = notes::outline(&rest[rest.find('>').map(|i| i + 1).unwrap_or(0)..cut]);
    // A contents list at the top repeats the headings that follow: not changes.
    let heads: std::collections::HashSet<String> = lines.iter().filter_map(|l| l.strip_prefix("## ").or_else(|| l.strip_prefix("### "))).map(str::to_string).collect();
    notes::tidy_headings(lines.into_iter().filter(|l| l.starts_with('#') || !heads.contains(l)).collect())
}

/// "https://github.com/o/r/blob/main/CHANGELOG.md" -> the raw file.
fn github_raw(url: &str) -> Option<String> {
    let (o, r) = github_repo(url)?;
    let i = url.find("/blob/")?;
    Some(format!("https://raw.githubusercontent.com/{o}/{r}/{}", &url[i + 6..]))
}

/// A change list and the page with the full notes.
type Notes = (Vec<String>, Option<String>);

static NOTES_CACHE: Lazy<Mutex<HashMap<String, Notes>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// Forget change lists kept for open pages (they are saved on disk too).
pub fn clear_notes_cache() {
    NOTES_CACHE.lock().clear();
}

/// The change list of one version and the page with the full notes, from the best source there
/// is: the manifest's own release notes, its release notes page, or the GitHub release.
async fn notes_for(m: &Manifest, gh: &Gh) -> Notes {
    let key = format!("{}@{}", m.id, m.version);
    if let Some(hit) = NOTES_CACHE.lock().get(&key).cloned() {
        return hit;
    }
    let r = notes_uncached(m, gh).await;
    NOTES_CACHE.lock().insert(key, r.clone());
    r
}

async fn notes_uncached(m: &Manifest, gh: &Gh) -> Notes {
    let (lines, url, _) = notes_detail(m, gh).await;
    (lines, url)
}

/// Change list, notes page and where the list came from (manifest, page, github, or 
/// when there is none).
async fn notes_detail(m: &Manifest, gh: &Gh) -> (Vec<String>, Option<String>, &'static str) {
    let ver = &m.version;
    let tag = m.installers.iter().find_map(|i| github_tag(&i.url)).or_else(|| m.release_notes_url.as_deref().and_then(github_tag));
    // A publisher page known for apps whose catalog entry names none.
    let known = notes_known::page_for(&m.id, ver);
    let mut link = m.release_notes_url.clone().or_else(|| gh.page.clone()).or_else(|| known.clone());
    // 1. The manifest's own notes.
    if let Some(t) = &m.release_notes {
        let lines = notes::text_lines(t);
        if !lines.is_empty() {
            if link.is_none() {
                link = find_release(gh.get().await, ver, tag.as_deref()).map(|r| r.url.clone());
            }
            return (lines, link, "manifest");
        }
    }
    // 2. The release notes page, or the one page another version names. A GitHub or Codeberg
    // release page is read for its release text; other GitHub pages as raw files.
    if let Some(u) = m.release_notes_url.as_ref() {
        let lines = release_page_notes(u).await;
        if !lines.is_empty() {
            return (lines, link, "page");
        }
    }
    if let Some(u) = m.release_notes_url.as_ref().or(gh.page.as_ref()).or(known.as_ref()) {
        let src = if github_repo(u).is_some() { github_raw(u) } else { Some(u.clone()) };
        if let Some(src) = src {
            // Kept on disk for a while: every listed version reads the same page.
            if let Some(body) = notes::page(&src).await {
                // Built by script: read as the browser shows it (see `render`).
                let lines = crate::render::page_notes(&src, &body, ver).await;
                if !lines.is_empty() {
                    return (lines, link, "page");
                }
            }
        }
    }
    // 3. The GitHub release.
    if let Some(rel) = find_release(gh.get().await, ver, tag.as_deref()) {
        let lines = notes::text_lines(&rel.body);
        let url = Some(rel.url.clone()).filter(|u| !u.is_empty());
        if !lines.is_empty() {
            return (lines, url, "github");
        }
        link = link.or(url);
    }
    // 4. The release page of the forge the download comes from (GitHub, Codeberg, GitLab), read
    // from the page itself: no API, so no request limit.
    let mut pages: Vec<String> = m.installers.iter().filter_map(|i| notes_web::release_page_of(&i.url)).collect();
    if let Some((o, r)) = repo_of(m) {
        // A project whose downloads are elsewhere: its release for this version, as tagged.
        for t in [format!("v{ver}"), ver.to_string()] {
            pages.push(format!("https://github.com/{o}/{r}/releases/tag/{t}"));
        }
    }
    pages.dedup();
    for page in pages.iter().filter(|p| Some(*p) != m.release_notes_url.as_ref()).take(3) {
        let lines = if page.starts_with("https://gitlab.com/") { notes_web::gitlab_release_notes(page).await } else { release_page_notes(page).await };
        if !lines.is_empty() {
            return (lines, Some(page.clone()), "release page");
        }
    }
    // 5. The publisher's own site: the changelog its homepage links to.
    let home = [&m.package_url, &m.publisher_url].into_iter().flatten().find(|u| github_repo(u).is_none() && !u.contains("gitlab.com/") && u.starts_with("https://"));
    if let Some(home) = home {
        if let Some((page, lines)) = notes_web::publisher_notes(home, ver).await {
            return (lines, Some(page), "site");
        }
    }
    (vec![], link, "")
}

/// For the live probe: the change list of one manifest and where it came from.
pub async fn probe_notes(m: &Manifest) -> (Vec<String>, Option<String>, &'static str) {
    let gh = Gh { repo: repo_of(m), rels: tokio::sync::OnceCell::new(), page: None };
    notes_detail(m, &gh).await
}

fn iso_date(d: &Option<String>) -> Option<String> {
    let d = d.as_deref()?.trim();
    (d.len() == 10 && d.as_bytes()[4] == b'-').then(|| format!("{d}T00:00:00Z")).or_else(|| Some(d.to_string()))
}

/// How long one version's change list may take before the list shows without it.
const NOTES_WAIT: Duration = Duration::from_secs(8);

/// Version list with change lists for a package. `installed` is the version in the uninstall list.
async fn version_infos(list: &[VerRef], installed: Option<&str>, want: &Want, latest_manifest: Option<&Manifest>) -> Vec<VersionInfo> {
    let top: Vec<VerRef> = list.iter().take(15).cloned().collect();
    let manifests: Vec<Option<Manifest>> = futures_util::stream::iter(top.iter().cloned())
        .map(|r| {
            let cached = latest_manifest.filter(|m| m.version == r.version).cloned();
            async move {
                match cached {
                    Some(m) => Some(m),
                    None => winget::manifest(&r).await.ok(),
                }
            }
        })
        .buffered(6)
        .collect()
        .await;
    // A notes page that is not about one version (no version or GitHub tag in its address).
    let page = manifests.iter().flatten().filter_map(|m| m.release_notes_url.as_ref().map(|u| (m, u))).find(|(m, u)| github_tag(u).is_none() && !notes::names_version(u, &m.version) && !u.contains(&m.version.replace('.', "_"))).map(|(_, u)| u.clone());
    let gh = Gh { repo: manifests.iter().flatten().find_map(repo_of), rels: tokio::sync::OnceCell::new(), page };
    // Change lists of all versions at once, each with a time limit: a slow notes site must not
    // keep the version list from showing (one page per version, one after another, could take
    // many minutes). A version whose notes are too slow keeps its link to the full notes.
    let gh = std::sync::Arc::new(gh);
    let fetched: Vec<(Vec<String>, Option<String>)> = futures_util::stream::iter(manifests.clone())
        .map(|m| {
            let gh = gh.clone();
            async move {
                match m {
                    Some(m) => tokio::time::timeout(NOTES_WAIT, notes_for(&m, &gh)).await.unwrap_or_else(|_| (vec![], m.release_notes_url.clone())),
                    None => (vec![], None),
                }
            }
        })
        .buffered(6)
        .collect()
        .await;
    // The catalog lists no sizes: ask the download server for the newest version's installer.
    let newest_size = match manifests.first().and_then(|m| m.as_ref()).and_then(|m| choose(m, want).ok()) {
        Some(i) => tokio::time::timeout(Duration::from_secs(5), remote_size(&i.url)).await.ok().flatten(),
        None => None,
    };
    let mut out = Vec::new();
    for (k, ((r, m), (lines, url))) in top.iter().zip(manifests.iter()).zip(fetched).enumerate() {
        let mut lines = lines;
        if lines.is_empty() {
            lines.push(if url.is_some() { SEE_NOTES.to_string() } else { NO_NOTES.to_string() });
        }
        let shows = m.as_ref().and_then(|m| choose(m, want).ok().map(|i| expected_version(m, i)));
        let is_installed = installed.is_some_and(|iv| version::cmp(iv, &r.version).is_eq() || shows.as_deref().is_some_and(|s| version::cmp(iv, s).is_eq()));
        out.push(VersionInfo {
            version: r.version.clone(),
            date: m.as_ref().and_then(|m| iso_date(&m.release_date)),
            notes: lines,
            notes_url: url.or_else(|| m.as_ref().and_then(|m| m.package_url.clone())),
            size: if k == 0 { newest_size } else { None },
            installed: is_installed,
            latest: k == 0,
            prerelease: super::tools::is_pre(&r.version),
        });
    }
    out
}

async fn pkg_and_versions(id: &str, feed: Option<&Feed>) -> Result<(Pkg, Vec<VerRef>, Option<Manifest>)> {
    if let Some(pf) = feed.and_then(|f| f.get(id)) {
        return Ok((pf.pkg.clone(), pf.versions.clone(), pf.latest.clone()));
    }
    let path = winget::refresh_index(Duration::from_secs(6 * 3600)).await?;
    let id2 = id.to_string();
    let p = tokio::task::spawn_blocking(move || Index::open(&path).ok().and_then(|i| i.by_id(&id2))).await.ok().flatten().ok_or_else(|| anyhow!("{id} is not in the winget catalog"))?;
    let v = winget::versions(&p).await?;
    Ok((p, v, None))
}

/// Versions an installed app can be set to, newest first, with their change lists.
pub async fn versions(inst: &Installed, feed: &Feed) -> Result<Vec<VersionInfo>> {
    let (_, list, latest) = pkg_and_versions(pkg_of(inst), Some(feed)).await?;
    if list.is_empty() {
        bail!("{} has no versions in the winget catalog", inst.name);
    }
    Ok(version_infos(&list, Some(&inst.version), &Want::for_installed(inst), latest.as_ref()).await)
}

/// Versions offered on a Get apps page ("other:<package id>").
pub async fn catalog_versions(id: &str, feed: Option<&Feed>, installed: &[Installed]) -> Result<Vec<VersionInfo>> {
    let pkg_id = id.strip_prefix("other:").unwrap_or(id);
    let (_, list, latest) = pkg_and_versions(pkg_id, feed).await?;
    let here = installed.iter().find(|i| pkg_of(i).eq_ignore_ascii_case(pkg_id));
    let want = here.map(Want::for_installed).unwrap_or_else(Want::fresh);
    Ok(version_infos(&list, here.map(|i| i.version.as_str()), &want, latest.as_ref()).await)
}

// ---------------------------------------------------------------- plans

pub(crate) fn safe_name(s: &str) -> String {
    s.chars().map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect()
}

/// File name for a download: the one in the address when it has a usable extension.
fn file_name(i: &Installer, pkg: &str, ver: &str) -> String {
    let last = i.url.split(['?', '#']).next().unwrap_or("").rsplit('/').next().unwrap_or("");
    let last = safe_name(&percent_decode(last));
    let exts: &[&str] = match i.kind.as_str() {
        "msi" | "wix" => &["msi"],
        "zip" => &["zip"],
        // Windows reads a package by its extension.
        "msix" => &["msix", "msixbundle", "appx", "appxbundle"],
        "appx" => &["appx", "appxbundle", "msix", "msixbundle"],
        _ => &["exe"],
    };
    let low = last.to_ascii_lowercase();
    if exts.iter().any(|e| low.ends_with(&format!(".{e}")) && low.len() > e.len() + 1) {
        last
    } else {
        format!("{}-{}.{}", safe_name(pkg), safe_name(ver), exts[0])
    }
}

fn args_of(s: &Option<String>) -> Vec<String> {
    s.as_deref().map(winutil::split_command_line).unwrap_or_default()
}

/// Does installing need admin rights? The manifest's scope and elevation say so, else the copy
/// that is here (per user or for all users).
fn needs_admin(i: &Installer, w: &Want) -> bool {
    match i.elevation.as_deref() {
        Some("elevationRequired") | Some("elevatesSelf") => return true,
        Some("elevationProhibited") => return false,
        _ => {}
    }
    match i.scope.as_deref() {
        Some("machine") => true,
        Some("user") => false,
        _ => w.scope != Some("user"),
    }
}

/// Arguments that make an installer of type `kind` run silently.
fn silent_args(i: &Installer, kind: &str, file: &Path, log: &Path, update: bool) -> (PathBuf, Vec<String>) {
    let mut extra = args_of(&i.custom);
    if update {
        extra.extend(args_of(&i.upgrade));
    }
    match kind {
        "msi" | "wix" => {
            let mut a = vec!["/i".into(), file.to_string_lossy().to_string(), "/qn".into(), "/norestart".into(), "MSIFASTINSTALL=7".into(), "/l*".into(), log.to_string_lossy().to_string()];
            a.extend(extra);
            (PathBuf::from("msiexec.exe"), a)
        }
        _ => {
            let mut a = match (&i.silent, kind) {
                (Some(_), _) => args_of(&i.silent),
                (None, "nsis") => vec!["/S".into()],
                (None, "inno") => vec!["/VERYSILENT".into(), "/SUPPRESSMSGBOXES".into(), "/NORESTART".into(), "/SP-".into()],
                (None, "burn") => vec!["/quiet".into(), "/norestart".into()],
                (None, _) => args_of(&i.silent_progress),
            };
            a.extend(extra);
            // Inno Setup writes what it does to a log, which the job reads for its detail line.
            if kind == "inno" && !a.iter().any(|x| x.to_ascii_lowercase().starts_with("/log")) {
                a.push(format!("/LOG={}", log.display()));
            }
            (file.to_path_buf(), a)
        }
    }
}

/// A folder the job may watch for running programs: a real app folder, not a shared root.
pub(crate) fn own_dir(d: &Path) -> bool {
    let me = norm_path(&d.to_string_lossy());
    d.components().count() >= 3 && ![paths::program_files(), paths::program_files_x86(), paths::local_app_data(), paths::local_app_data().join("Programs"), paths::app_data()].iter().any(|r| norm_path(&r.to_string_lossy()) == me)
}

/// Everything an install needs once the manifest and installer are known. No network.
pub fn build_plan(p: &Pkg, m: &Manifest, i: &Installer, kind: JobKind, current: Option<&Installed>, size: Option<u64>) -> Result<Plan> {
    let w = current.map(Want::for_installed).unwrap_or_else(Want::fresh);
    let name = current.map(|c| c.name.clone()).unwrap_or_else(|| if m.name.is_empty() { p.name.clone() } else { m.name.clone() });
    // The copy this plan is for: "other:<package>", or "other:<package>:user" next to a copy for
    // all users. Each copy downloads and logs into its own place.
    let app_id = current.map(|c| c.id.clone()).unwrap_or_else(|| format!("other:{}", p.id));
    let tail = safe_name(app_id.strip_prefix("other:").unwrap_or(&p.id));
    let base = paths::downloads().join("other").join(&tail);
    let dest = base.join(file_name(i, &p.id, &m.version));
    let log = paths::logs().join(format!("other-{}-{}.log", tail, safe_name(&m.version)));
    let update = current.is_some() && kind != JobKind::Install;
    let block_dirs: Vec<PathBuf> = current.and_then(|c| c.install_dir.clone()).filter(|d| own_dir(d)).into_iter().collect();
    let download = Download { url: i.url.clone(), dest: dest.clone(), size, sha256: Some(i.sha256.clone()), sha512: None, md5: None, signer: None, headers: vec![] };
    if package_kind(&i.kind) {
        // An app package: Windows' package manager installs it for this user, no admin rights.
        let family = i.family.clone().filter(|f| !f.trim().is_empty()).ok_or_else(|| anyhow!("{name}: the catalog does not name its package family"))?;
        let doing = match kind {
            JobKind::Update => "Updating",
            JobKind::Repair => "Reinstalling",
            _ => "Installing",
        };
        let label = format!("{doing} {name} {}", m.version);
        return Ok(Plan {
            app_id: app_id.clone(),
            kind,
            name: name.clone(),
            to_version: m.version.clone(),
            downloads: vec![download],
            steps: vec![Step::AddPackage { path: dest.clone(), family, signature_sha256: i.signature_sha256.clone(), update, any_version: kind == JobKind::Rollback, elevated: false, label }],
            block_dirs,
            block_names: vec![],
            verify: Verify::Version { app_id, version: expected_version(m, i) },
            fallback: None,
            cleanup: vec![dest],
        });
    }
    let admin = needs_admin(i, &w);
    // A game that installs through its maker's client: the client is installed, the user clicks
    // Install there, and the game shows up here once the client has put it on the PC.
    let hand_off = hand_off_client(&p.id, &i.url);
    let label = match hand_off {
        Some(client) => format!("{}: {name} installs from the {client}, click Install there", super::WAITING_FOR_YOU),
        None => format!("Installing {} {}", name, m.version),
    };
    let mut steps = Vec::new();
    let mut cleanup = vec![dest.clone()];
    if update && i.upgrade_behavior.as_deref() == Some("uninstallPrevious") {
        if let Some(u) = current.and_then(|c| c.uninstall.clone()).filter(|u| !u.is_empty()) {
            steps.push(Step::Run { exe: PathBuf::from(&u[0]), args: u[1..].to_vec(), cwd: None, ok_codes: vec![0, 3010, 1641, 1605], elevated: admin || w.scope == Some("machine"), label: format!("Removing the old {name}"), log: None });
        }
    }
    let (file, run_kind) = if i.kind == "zip" {
        let dir = base.join(format!("unpacked-{}", safe_name(&m.version)));
        let inner = i.nested_path.clone().ok_or_else(|| anyhow!("{name}: the zip has no installer named"))?;
        let rooted = inner.starts_with('/') || inner.starts_with('\\') || inner.contains(':') || Path::new(&inner).is_absolute();
        if inner.contains("..") || rooted {
            bail!("{name}: the installer path in the zip leaves the zip");
        }
        steps.push(Step::ExtractZip { zip: dest.clone(), dest: dir.clone(), strip_root: false, elevated: false, label: format!("Unpacking {name}") });
        cleanup.push(dir.clone());
        (dir.join(inner.replace('/', "\\")), kind_of(i).to_string())
    } else {
        (dest.clone(), i.kind.clone())
    };
    let (exe, args) = silent_args(i, &run_kind, &file, &log, update);
    let mut ok_codes = vec![0];
    ok_codes.extend(i.success_codes.iter().copied());
    if msi_family(&run_kind) {
        ok_codes.extend([3010, 1641]);
    }
    let is_msi = msi_family(&run_kind);
    let logged = is_msi || args.contains(&format!("/LOG={}", log.display()));
    if logged {
        cleanup.push(log.clone());
    }
    let run = |elevated: bool| Step::Run { exe: exe.clone(), args: args.clone(), cwd: None, ok_codes: ok_codes.clone(), elevated, label: label.clone(), log: logged.then(|| log.clone()) };
    let block_names: Vec<String> = if block_dirs.is_empty() { current.and_then(|c| c.exe.as_ref()).and_then(|e| e.file_name()).map(|n| n.to_string_lossy().to_ascii_lowercase()).filter(|n| n.ends_with(".exe")).into_iter().collect() } else { vec![] };
    let make = |steps: Vec<Step>| Plan {
        app_id: app_id.clone(),
        kind,
        name: name.clone(),
        to_version: m.version.clone(),
        downloads: vec![download.clone()],
        steps,
        block_dirs: block_dirs.clone(),
        block_names: block_names.clone(),
        verify: if hand_off.is_some() { Verify::None } else { Verify::Version { app_id: app_id.clone(), version: expected_version(m, i) } },
        fallback: None,
        cleanup: cleanup.clone(),
    };
    let mut first = steps.clone();
    first.push(run(admin));
    let mut plan = make(first);
    // A per-user installer that Windows will only start with admin rights gets them on a second
    // try (see `needs_elevation`). Only the installer runs again: removing the old copy already
    // happened, and its uninstaller is gone.
    if !admin {
        let mut again: Vec<Step> = steps.into_iter().filter(|s| !matches!(s, Step::Run { label, .. } if label.starts_with("Removing the old"))).collect();
        again.push(run(true));
        plan.fallback = Some(Box::new(make(again)));
    }
    Ok(plan)
}

/// Packages that only install a game maker's own client, which then downloads, installs and
/// updates the game: the client's name. Riot's (VALORANT and League of Legends in every region)
/// install the Riot Client and open it on the game's Install button.
pub fn hand_off_client(pkg_id: &str, installer_url: &str) -> Option<&'static str> {
    let riot = pkg_id.to_ascii_lowercase().starts_with("riotgames.") || super::drivers::host_is(installer_url, &["riotcdn.net"]);
    riot.then_some("Riot Client")
}

/// Did Windows refuse to start an installer because it asks for admin rights (error 740)?
pub fn needs_elevation(error: &str) -> bool {
    error.contains("os error 740") || error.contains("requires elevation")
}

/// Size of the installer at its address, when the server says.
async fn remote_size(url: &str) -> Option<u64> {
    let r = http::CLIENT.head(url).timeout(Duration::from_secs(15)).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    r.headers().get(reqwest::header::CONTENT_LENGTH).and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok()).filter(|n| *n > 0)
}

/// `id` is "other:<package id>"; `ver` a package version, or None for the newest.
pub async fn plan_install(id: &str, ver: Option<&str>, kind: JobKind, current: Option<&Installed>, feed: Option<&Feed>) -> Result<Plan> {
    // An installed copy names its package (its id may carry ":user").
    let pkg_id = current.map(pkg_of).filter(|p| !p.is_empty()).unwrap_or_else(|| id.strip_prefix("other:").unwrap_or(id));
    let (p, list, latest) = pkg_and_versions(pkg_id, feed).await?;
    let label = current.map(|c| c.name.clone()).unwrap_or_else(|| p.name.clone());
    let w = current.map(Want::for_installed).unwrap_or_else(Want::fresh);
    let m = match (kind, ver, current) {
        // The installed version again: the package version that writes it to the uninstall list.
        (JobKind::Repair, _, Some(c)) => {
            let mut found = None;
            for r in list.iter().take(15) {
                let m = match latest.as_ref().filter(|m| m.version == r.version) {
                    Some(m) => m.clone(),
                    None => winget::manifest(r).await?,
                };
                let shows = choose(&m, &w).ok().map(|i| expected_version(&m, i));
                if version::cmp(&r.version, &c.version).is_eq() || shows.is_some_and(|s| version::cmp(&s, &c.version).is_eq()) {
                    found = Some(m);
                    break;
                }
            }
            found.ok_or_else(|| anyhow!("{label} {} is not in the winget catalog any more", c.version))?
        }
        _ => {
            let r = match ver {
                Some(v) => list.iter().find(|r| r.version == v).or_else(|| list.iter().find(|r| version::cmp(&r.version, v).is_eq())),
                None => list.first(),
            }
            .ok_or_else(|| anyhow!("{label} {} is not in the winget catalog", ver.unwrap_or_default()))?;
            match latest.filter(|m| m.version == r.version) {
                Some(m) => m,
                None => winget::manifest(r).await?,
            }
        }
    };
    let i = choose(&m, &w).map_err(|why| anyhow!("{label} {} can not be installed here: {why}", m.version))?;
    if current.is_some() && kind != JobKind::Repair && i.upgrade_behavior.as_deref() == Some("deny") {
        bail!("The catalog says {label} can not be installed over the copy that is here. Uninstall it first.");
    }
    // An older version than the one here is a roll back, told by what the uninstall list will show.
    let kind = match (kind, current) {
        (JobKind::Update | JobKind::Rollback, Some(c)) => {
            if is_newer(&c.version, &expected_version(&m, i)) {
                JobKind::Rollback
            } else {
                JobKind::Update
            }
        }
        (k, _) => k,
    };
    let size = remote_size(&i.url).await;
    build_plan(&p, &m, i, kind, current, size)
}

pub fn plan_uninstall(inst: &Installed) -> Result<Plan> {
    if let Some(family) = package_family(inst) {
        // An app package: Windows' package manager removes it (and its data) for this user.
        return Ok(Plan {
            app_id: inst.id.clone(),
            kind: JobKind::Uninstall,
            name: inst.name.clone(),
            to_version: inst.version.clone(),
            downloads: vec![],
            steps: vec![Step::RemovePackage { family, elevated: false, label: format!("Uninstalling {}", inst.name) }],
            block_dirs: inst.install_dir.iter().filter(|d| own_dir(d)).cloned().collect(),
            block_names: vec![],
            verify: Verify::Removed { app_id: inst.id.clone() },
            fallback: None,
            cleanup: vec![],
        });
    }
    // No silent command known: its own uninstaller runs (silently when its kind is recognised,
    // otherwise in its window), as for Other programs.
    let Some(u) = inst.uninstall.clone().filter(|u| !u.is_empty()) else { return crate::uninstall::run::plan(inst) };
    let user = matches!(&inst.data, SourceData::Other { entry, .. } if entry.hive == "HKCU");
    let exe = PathBuf::from(&u[0]);
    let in_pf = starts_dir(&u[0], &paths::program_files()) || starts_dir(&u[0], &paths::program_files_x86());
    let elevated = !user || in_pf;
    Ok(Plan {
        app_id: inst.id.clone(),
        kind: JobKind::Uninstall,
        name: inst.name.clone(),
        to_version: inst.version.clone(),
        downloads: vec![],
        steps: vec![Step::Run { exe, args: u[1..].to_vec(), cwd: None, ok_codes: vec![0, 3010, 1641, 1605], elevated, label: format!("Uninstalling {}", inst.name), log: None }],
        block_dirs: inst.install_dir.iter().filter(|d| own_dir(d)).cloned().collect(),
        block_names: vec![],
        verify: Verify::Removed { app_id: inst.id.clone() },
        fallback: None,
        cleanup: vec![],
    })
}

/// Guess of how long an install takes, in seconds, before it has been learned.
pub fn install_seconds(mb: f64) -> f64 {
    25.0 + mb / 25.0
}

#[cfg(test)]
mod tests;
pub mod notes_known;
pub mod notes_web;
#[cfg(test)]
mod package_tests;
#[cfg(test)]
mod package_probe;
#[cfg(test)]
mod probe;
#[cfg(test)]
mod locate_tests;
