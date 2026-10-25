//! "Other programs": every program in the uninstall list that no source lists, as Windows'
//! Installed apps page shows them. System components, updates and parts of other products are
//! left out; a Burn bundle and its MSI packages are one program; the same program registered
//! twice (both registry views) is shown once.

use super::switches;
use crate::model::Source;
use crate::sources::{Installed, SourceData};
use crate::winutil::{self, UninstallEntry};
use std::path::{Path, PathBuf};

/// Id of a program: "prog:<hive>:<key>", lowercase.
pub fn id_of(e: &UninstallEntry) -> String {
    format!("prog:{}:{}", e.hive, e.key).to_ascii_lowercase()
}

fn norm_path(p: &str) -> String {
    p.trim().trim_matches('"').replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

/// An entry one of the listed apps already stands for.
pub fn claimed(e: &UninstallEntry, listed: &[Installed]) -> bool {
    if e.uninstall_string.contains("--sapCode=") {
        return true;
    }
    let loc = norm_path(&e.install_location);
    let key = e.key.to_ascii_lowercase();
    listed.iter().any(|i| {
        let dir = i.install_dir.as_ref().map(|d| norm_path(&d.to_string_lossy())).unwrap_or_default();
        match &i.data {
            SourceData::Other { entry, .. } if !entry.key.is_empty() => {
                return (entry.key.eq_ignore_ascii_case(&e.key) && entry.hive == e.hive) || entry.msi_parts.iter().any(|k| k.eq_ignore_ascii_case(&e.key));
            }
            SourceData::Craft { msi_product_code: Some(c), .. } | SourceData::Blender { msi_product_code: Some(c), .. } if c.eq_ignore_ascii_case(&e.key) => return true,
            _ => {}
        }
        (!loc.is_empty() && dir == loc)
            || (key.starts_with('{') && i.uninstall.as_ref().is_some_and(|u| u.iter().any(|a| a.to_ascii_lowercase().contains(&key))))
            || (i.name.eq_ignore_ascii_case(e.display_name.trim()) && i.version == e.display_version.trim())
    })
}

/// Entries to list as programs: apps of their own (see `other::is_app`), bundles folded, not
/// claimed by a listed app, and each program once.
pub fn entries(all: &[UninstallEntry], listed: &[Installed]) -> Vec<UninstallEntry> {
    let folded = crate::sources::other::fold_bundles(all);
    // The MSI packages a bundle stands for are not programs of their own.
    let parts: Vec<String> = folded.iter().flat_map(|e| e.msi_parts.iter().map(|k| k.to_ascii_lowercase())).collect();
    let mut out: Vec<UninstallEntry> = Vec::new();
    for e in folded {
        if !crate::sources::other::is_app(&e) || claimed(&e, listed) || parts.contains(&e.key.to_ascii_lowercase()) {
            continue;
        }
        let dup = out.iter().any(|o| {
            o.display_name.trim().eq_ignore_ascii_case(e.display_name.trim())
                && o.display_version.trim() == e.display_version.trim()
                && (norm_path(&o.uninstall_string) == norm_path(&e.uninstall_string) || o.key.eq_ignore_ascii_case(&e.key))
        });
        if !dup {
            out.push(e);
        }
    }
    out
}

/// Two letters and colors for a program without an icon.
fn mark(name: &str) -> (String, String, String) {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut code = String::new();
    if let Some(w) = words.first() {
        code.extend(w.chars().next().map(|c| c.to_ascii_uppercase()));
        match words.get(1) {
            Some(w2) => code.extend(w2.chars().next().map(|c| c.to_ascii_lowercase())),
            None => code.extend(w.chars().nth(1).map(|c| c.to_ascii_lowercase())),
        }
    }
    if code.is_empty() {
        code = "?".into();
    }
    const BG: &[&str] = &["#2B2F36", "#2F2B36", "#2B3630", "#36302B", "#2B3436", "#362B31"];
    let h = name.bytes().fold(0u32, |a, b| a.wrapping_mul(31).wrapping_add(b as u32));
    (code, "#EDEDED".into(), BG[h as usize % BG.len()].into())
}

/// The program entry as an installed app. `exists` and `msi_package` are given by tests.
pub fn make(e: &UninstallEntry, exe: Option<PathBuf>, dir: Option<PathBuf>, exists: &dyn Fn(&Path) -> bool, msi_package: Option<&Path>) -> Installed {
    // Told from the entry alone here (lists are read often); the uninstall reads the file too.
    let choice = switches::choose(e, switches::detect(e, &[]));
    let broken = switches::broken_reason(e, exists, msi_package);
    let name = e.display_name.trim().to_string();
    let (code, fg, bg) = mark(&name);
    Installed {
        id: id_of(e),
        source: Source::Programs,
        name: name.clone(),
        code,
        fg,
        bg,
        version: e.display_version.trim().to_string(),
        install_dir: dir,
        exe,
        // A missing uninstaller is not run: "Remove from list" cleans up instead.
        uninstall: if broken.is_some() { None } else { choice.map(|c| c.args) },
        updated_at: winutil::install_date_iso(e.install_date.trim()).or_else(|| e.written.map(|w| chrono::DateTime::<chrono::Utc>::from(w).format("%Y-%m-%dT%H:%M:%SZ").to_string())),
        size_hint: Some(e.estimated_size_kb * 1024).filter(|s| *s > 0),
        tagline: broken.clone().or_else(|| {
            let p = e.publisher.trim();
            // A web app's publisher is the browser that installed it.
            match crate::sources::locate::web_app(e) {
                Some(_) if !p.is_empty() => Some(format!("{p} (web app)")),
                _ => Some(p.to_string()).filter(|p| !p.is_empty()),
            }
        }),
        channel: None,
        missing: broken.is_some(),
        read_only: false,
        store: None,
        icon_remote: None,
        data: SourceData::Other { pkg: String::new(), how: crate::sources::other::How::Name, entry: Box::new(e.clone()), upgrade_code: None },
    }
}

/// The command that removes a program, with its uninstaller's file read to tell its type.
pub fn choice_for(i: &Installed) -> Option<switches::Choice> {
    match &i.data {
        SourceData::Other { entry, .. } => {
            let head = switches::uninstaller_file(entry).map(|p| switches::head_of(&p)).unwrap_or_default();
            switches::choose(entry, switches::detect(entry, &head))
        }
        _ => None,
    }
}

/// Every program no source lists, read from this PC.
pub fn scan(all: &[UninstallEntry], listed: &[Installed]) -> Vec<Installed> {
    let list = entries(all, listed);
    if list.is_empty() {
        return vec![];
    }
    let hints = crate::sources::locate::Hints::read();
    list.iter()
        .map(|e| {
            let mut msi = e.msi_parts.clone();
            if e.windows_installer && e.key.starts_with('{') {
                msi.insert(0, e.key.clone());
            }
            let (exe, dir) = crate::sources::locate::locate(e, &msi, &hints);
            let pkg = if e.windows_installer { switches::msi_local_package(&e.key) } else { None };
            make(e, exe, dir, &|p: &Path| p.exists(), pkg.as_deref())
        })
        .collect()
}
