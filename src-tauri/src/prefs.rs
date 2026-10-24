//! "Also delete settings and add-ons" on uninstall: where each app keeps them.
//!
//! Only per-user settings, caches and add-ons are listed. Never the user's own work: Documents,
//! Godot's signing keystores and the save data of games made with it, Substance assets, browser
//! profiles. Folders that every installed version shares go only with the last version.
//!
//! Apps without rules of their own (Other apps, and More apps tools without one) are matched by
//! name: a folder or registry key goes when its whole name is one of the app's names (see `keys`),
//! never on a part of a word, and never when another installed app maps to it as well.

use crate::sources::{Installed, SourceData, Step};
use crate::version;
use std::path::{Path, PathBuf};

/// Where per-user data lives (tests point these at a temp folder).
pub struct Roots {
    pub roaming: PathBuf,
    pub local: PathBuf,
    pub program_data: PathBuf,
    pub local_low: PathBuf,
    /// The user's profile folder, for dot-folders such as ".cline".
    pub home: PathBuf,
    /// Key under HKEY_CURRENT_USER apps keep their settings in ("Software").
    pub reg: String,
    /// Folders of every program in the uninstall list, ours or not: never deleted, nor what holds them.
    pub install_dirs: Vec<PathBuf>,
    /// The folders on PATH: a command-line tool found there may share a dot-folder of its name.
    pub path_dirs: Vec<PathBuf>,
    /// The winget moniker of a package id ("Microsoft.VisualStudioCode" is "vscode").
    pub moniker: Box<dyn Fn(&str) -> Option<String>>,
}

impl Roots {
    pub fn system() -> Roots {
        let home = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
        let mut install_dirs = Vec::new();
        for e in crate::winutil::uninstall_entries() {
            if !e.install_location.trim().is_empty() {
                install_dirs.push(crate::winutil::pathbuf(&e.install_location));
            }
            let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"').to_string();
            let uninstaller = crate::winutil::split_program_line(&e.uninstall_string).into_iter().next().unwrap_or_default();
            for prog in [icon, uninstaller] {
                let p = PathBuf::from(prog);
                if p.is_absolute() {
                    install_dirs.extend(p.parent().map(Path::to_path_buf));
                }
            }
        }
        let idx = crate::sources::winget::Index::open(&crate::sources::winget::index_path()).ok();
        Roots {
            roaming: crate::paths::app_data(),
            local: crate::paths::local_app_data(),
            program_data: PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())),
            local_low: home.join("AppData").join("LocalLow"),
            home,
            reg: "Software".into(),
            install_dirs,
            path_dirs: std::env::var_os("PATH").map(|p| std::env::split_paths(&p).collect()).unwrap_or_default(),
            moniker: Box::new(move |id: &str| idx.as_ref()?.by_id(id)?.moniker.filter(|m| !m.is_empty())),
        }
    }
}

/// What the remove steps would take, with sizes, for the uninstall dialog. Paths under the user's
/// profile are shown from "AppData" on.
pub fn preview(steps: &[Step]) -> Vec<crate::model::PrefsItem> {
    let profile = std::env::var("USERPROFILE").unwrap_or_default().to_ascii_lowercase();
    let show = |p: &Path| {
        let s = p.to_string_lossy().to_string();
        match s.to_ascii_lowercase().strip_prefix(&format!("{profile}\\")) {
            Some(_) if !profile.is_empty() => s[profile.len() + 1..].to_string(),
            _ => s,
        }
    };
    steps
        .iter()
        .filter_map(|st| match st {
            Step::RemoveDir { path, .. } => Some(crate::model::PrefsItem { path: show(path), size: Some(crate::winutil::dir_size(path)), registry: false }),
            Step::RemoveFile { path } => Some(crate::model::PrefsItem { path: show(path), size: path.metadata().ok().map(|m| m.len()), registry: false }),
            Step::RemoveUserRegKey { path, .. } => Some(crate::model::PrefsItem { path: format!("HKCU\\{path}"), size: None, registry: true }),
            _ => None,
        })
        .collect()
}

/// Settings and add-ons of `inst` that exist on disk, as remove steps. `others` are the other
/// installed apps (to keep what another version still uses).
pub fn steps(inst: &Installed, others: &[Installed], r: &Roots) -> Vec<Step> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut elevated_dirs: Vec<PathBuf> = Vec::new();
    let mut reg_keys: Vec<String> = Vec::new();
    let same_tool = |key: &str| others.iter().any(|o| matches!(&o.data, SourceData::Tools { key: k, .. } if k == key));
    match &inst.data {
        // A launcher keeps a game's saves and settings: never touched here.
        SourceData::Game(_) => {}
        SourceData::Adobe { sap, .. } => {
            // Creative Cloud is never uninstalled here; its data is shared by every Adobe app.
            if sap != "KCCC" {
                if let Some(name) = inst.install_dir.as_deref().and_then(Path::file_name) {
                    dirs.push(r.roaming.join("Adobe").join(name));
                    dirs.push(r.local.join("Adobe").join(name));
                }
            }
        }
        SourceData::Craft { key, .. } => {
            let squashed: String = inst.name.chars().filter(|c| !c.is_whitespace()).collect();
            for base in [&r.roaming, &r.local] {
                dirs.push(base.join(&inst.name));
                dirs.push(base.join(&squashed));
                dirs.push(base.join(format!("ai.storyteller.{key}")));
            }
        }
        SourceData::Blender { series, .. } => {
            // Settings and add-ons are per series; a beta copy of a series shares them with the
            // release of that series, so they stay while either is installed.
            let shared = others.iter().any(|o| matches!(&o.data, SourceData::Blender { series: s, .. } if s == series));
            if !shared {
                dirs.push(r.roaming.join("Blender Foundation").join("Blender").join(series));
                dirs.push(r.local.join("Blender Foundation").join("Blender").join(series));
            }
        }
        SourceData::Tools { key, series } => match key.as_str() {
            "obs" => {
                dirs.push(r.roaming.join("obs-studio"));
                elevated_dirs.push(r.program_data.join("obs-studio"));
            }
            "krita" => {
                dirs.push(r.roaming.join("krita"));
                dirs.push(r.local.join("krita"));
                files.push(r.local.join("kritarc"));
                files.push(r.local.join("kritadisplayrc"));
            }
            "freecad" => {
                // FreeCAD 1.1 and later keep each version apart ("v1-1"); older ones share one folder.
                let p = version::parts(series);
                let own = r.roaming.join("FreeCAD").join(format!("v{}-{}", p.first().unwrap_or(&0), p.get(1).unwrap_or(&0)));
                if own.exists() {
                    dirs.push(own);
                    dirs.push(r.local.join("FreeCAD").join(format!("v{}-{}", p.first().unwrap_or(&0), p.get(1).unwrap_or(&0))));
                } else if !same_tool("freecad") {
                    dirs.push(r.roaming.join("FreeCAD"));
                    dirs.push(r.local.join("FreeCAD"));
                }
            }
            "godot" => {
                let g = r.roaming.join("Godot");
                let ver = &inst.version;
                files.push(g.join(format!("editor_settings-{}.tres", version::trim_like(ver, "1.2"))));
                if let Ok(rd) = std::fs::read_dir(g.join("export_templates")) {
                    for e in rd.flatten() {
                        let n = e.file_name().to_string_lossy().to_string();
                        if n.split('.').take(3).collect::<Vec<_>>().join(".") == *ver || n.starts_with(&format!("{ver}.")) {
                            dirs.push(e.path());
                        }
                    }
                }
                if !same_tool("godot") {
                    for d in ["feature_profiles", "script_templates", "text_editor_themes", "shader_cache"] {
                        dirs.push(g.join(d));
                    }
                    files.push(g.join("projects.cfg"));
                    dirs.push(r.local.join("Godot"));
                }
            }
            "unityhub" => {
                dirs.push(r.roaming.join("UnityHub"));
            }
            "unity" => {
                // Editor preferences and the Asset Store download cache are shared by all editors.
                if !same_tool("unity") {
                    reg_keys.push("Software\\Unity Technologies\\Unity Editor 5.x".into());
                    dirs.push(r.roaming.join("Unity").join("Editor-5.x"));
                    dirs.push(r.roaming.join("Unity").join("Asset Store-5.x"));
                    dirs.push(r.local.join("Unity").join("Editor"));
                }
            }
            _ => {}
        },
        SourceData::Other { pkg, .. } => {
            // A second copy of the same package (per user and for all users) shares them.
            let copies = others.iter().any(|o| matches!(&o.data, SourceData::Other { pkg: p, .. } if p.eq_ignore_ascii_case(pkg)));
            for place in known::KNOWN.iter().filter(|(id, _)| id.eq_ignore_ascii_case(pkg) && !copies).flat_map(|(_, p)| p.iter()) {
                let (root, rest) = place.split_once('\\').unwrap_or((place, ""));
                let base = match root {
                    "Roaming" => &r.roaming,
                    "Local" => &r.local,
                    "LocalLow" => &r.local_low,
                    "Home" => &r.home,
                    "ProgramData" => &r.program_data,
                    "HKCU" => {
                        if reg_exists(&format!("{}\\{rest}", r.reg)) {
                            reg_keys.push(format!("{}\\{rest}", r.reg));
                        }
                        continue;
                    }
                    _ => continue,
                };
                // "JetBrains\IdeaIC*": the name with a version after it ("IdeaIC2025.2").
                let found: Vec<PathBuf> = match rest.strip_suffix('*') {
                    Some(stem) => {
                        let (parent, prefix) = stem.rsplit_once('\\').unwrap_or(("", stem));
                        subdirs(&base.join(parent))
                            .into_iter()
                            .filter(|(_, n)| {
                                let head = n.get(..prefix.len()).is_some_and(|h| h.eq_ignore_ascii_case(prefix));
                                let ver = n.get(prefix.len()..).unwrap_or("");
                                head && ver.starts_with(|c: char| c.is_ascii_digit()) && ver.chars().all(|c| c.is_ascii_digit() || c == '.')
                            })
                            .map(|(p, _)| p)
                            .collect()
                    }
                    None => vec![base.join(rest)],
                };
                // A settings file of its own ("Local\kdenliverc").
                let (found_files, found): (Vec<PathBuf>, Vec<PathBuf>) = found.into_iter().partition(|p| p.is_file());
                files.extend(found_files);
                if root == "ProgramData" {
                    elevated_dirs.extend(found);
                } else {
                    dirs.extend(found);
                }
            }
        }
    }
    if by_name(inst) {
        // A folder or key another installed app maps to as well (or one holding it) is shared:
        // Python 3.10, 3.12 and 3.14 all keep packages in "AppData\Roaming\Python". Neither
        // deletes it.
        let sc = Scan::new(r);
        let theirs: Vec<String> = others.iter().filter(|o| by_name(o)).flat_map(|o| found(o, r, &sc).all()).map(|p| p.to_ascii_lowercase()).collect();
        let shared = |p: &str| {
            let me = p.to_ascii_lowercase();
            theirs.iter().any(|t| *t == me || t.starts_with(&format!("{me}\\")))
        };
        // What is found by the app's full name in a folder that is not its publisher's: nor one
        // inside a folder another app takes whole.
        let shared_weak = |p: &str| shared(p) || theirs.iter().any(|t| p.to_ascii_lowercase().starts_with(&format!("{t}\\")));
        // A dot-folder named after a command another installed app has as well: Anaconda's
        // "Scripts\conda.exe" uses the ".conda" that Miniconda's "_conda.exe" finds.
        let their_cmd = |d: &Path| {
            let Some(cmd) = d.file_name().map(|n| n.to_string_lossy().to_string()).filter(|n| n.starts_with('.') && d.parent() == Some(r.home.as_path())) else { return false };
            let cmd = cmd.trim_start_matches('.');
            others.iter().any(|o| {
                let mut at: Vec<PathBuf> = o.install_dir.iter().flat_map(|i| [i.clone(), i.join("Scripts"), i.join("bin")]).collect();
                at.extend(o.exe.as_deref().and_then(Path::parent).map(Path::to_path_buf));
                at.iter().any(|p| ["exe", "cmd", "bat"].iter().any(|x| p.join(format!("{cmd}.{x}")).is_file()))
            })
        };
        let f = found(inst, r, &sc);
        dirs.extend(f.dirs.into_iter().filter(|d| !shared(&d.to_string_lossy()) && !their_cmd(d)));
        elevated_dirs.extend(f.elevated.into_iter().filter(|d| !shared(&d.to_string_lossy())));
        reg_keys.extend(f.reg.into_iter().filter(|k| !shared(&format!("hkcu\\{k}"))));
        for (d, elevated) in f.weak.into_iter().filter(|(d, _)| !shared_weak(&d.to_string_lossy())) {
            if elevated {
                elevated_dirs.push(d);
            } else {
                dirs.push(d);
            }
        }
        reg_keys.extend(f.weak_reg.into_iter().filter(|k| !shared_weak(&format!("hkcu\\{k}"))));
    }
    // Never a folder that holds an installed app (its own or another one, or any program in the
    // uninstall list), nor a browser profile.
    let holds_app = |d: &Path| {
        let me = d.to_string_lossy().to_ascii_lowercase();
        let me = me.trim_end_matches('\\');
        others.iter().chain(std::iter::once(inst)).filter_map(|o| o.install_dir.as_ref()).chain(r.install_dirs.iter()).any(|o| {
            let o = o.to_string_lossy().to_ascii_lowercase();
            let o = o.trim_end_matches('\\');
            o == me || o.starts_with(&format!("{me}\\"))
        })
    };
    let keep = |d: &PathBuf| !holds_app(d) && !user_content(d);
    let dirs: Vec<PathBuf> = dirs.into_iter().filter(keep).collect();
    let elevated_dirs: Vec<PathBuf> = elevated_dirs.into_iter().filter(keep).collect();
    // Never the install folder itself (the uninstall handles that) and nothing that is missing.
    let not_install = |p: &PathBuf| inst.install_dir.as_deref() != Some(p.as_path());
    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    let within = |seen: &[PathBuf], d: &Path| {
        let me = d.to_string_lossy().to_ascii_lowercase();
        seen.iter().any(|s| {
            let s = s.to_string_lossy().to_ascii_lowercase();
            s == me || me.starts_with(&format!("{s}\\"))
        })
    };
    for d in dirs.into_iter().filter(|d| d.is_dir() && not_install(d)) {
        if within(&seen, &d) {
            continue;
        }
        seen.push(d.clone());
        out.push(Step::RemoveDir { path: d, elevated: false, label: format!("Deleting {} settings", inst.name) });
    }
    for d in elevated_dirs.into_iter().filter(|d| d.is_dir()) {
        if within(&seen, &d) {
            continue;
        }
        seen.push(d.clone());
        out.push(Step::RemoveDir { path: d, elevated: true, label: format!("Deleting {} add-ons", inst.name) });
    }
    for f in files.into_iter().filter(|f| f.is_file()) {
        out.push(Step::RemoveFile { path: f });
    }
    let mut keys_seen: Vec<String> = Vec::new();
    for k in reg_keys {
        let low = k.to_ascii_lowercase();
        if keys_seen.iter().any(|s| *s == low || low.starts_with(&format!("{s}\\"))) {
            continue;
        }
        keys_seen.push(low);
        out.push(Step::RemoveUserRegKey { path: k, label: format!("Deleting {} settings", inst.name) });
    }
    out
}

/// A browser profile (Chromium's "User Data", Firefox's profiles, or a Chromium profile right in
/// the folder as Opera keeps it: "Local State" with a "Default" profile): passwords, bookmarks
/// and history are the user's own, not settings.
fn user_content(d: &Path) -> bool {
    d.join("User Data").is_dir() || d.join("profiles.ini").is_file() || (d.join("Local State").is_file() && d.join("Default").is_dir())
}

/// Apps whose settings are found by their names: Other apps and the More apps tools without
/// rules of their own.
fn by_name(inst: &Installed) -> bool {
    match &inst.data {
        SourceData::Other { .. } => true,
        SourceData::Tools { key, .. } => !["obs", "krita", "freecad", "godot", "unityhub", "unity"].contains(&key.as_str()),
        _ => false,
    }
}

/// "LM Studio", "lm-studio", "LMStudio" and ".lmstudio" are all "lmstudio".
fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Names (normalized) that many programs or Windows itself use: never an app's own folder.
const GENERIC: &[&str] = &[
    "microsoft", "google", "programs", "packages", "package", "temp", "tmp", "adobe", "nvidia", "nvidiacorporation", "intel", "amd", "apple", "windows",
    "commonfiles", "programfiles", "programfilesx86", "crashdumps", "crashpad", "cache", "caches", "logs", "log", "app", "apps", "application", "applications",
    "update", "updater", "updates", "installer", "installers", "setup", "uninstall", "user", "users", "data", "local", "roaming", "locallow", "appdata", "exe",
    "msi", "beta", "preview", "insiders", "x64", "x86", "arm64", "client", "desktop", "launcher", "tools", "tool", "bin", "software", "server", "service",
    "services", "runtime", "runtimes", "sdk", "cli", "driver", "drivers", "system", "config", "settings", "profile", "profiles", "plugins", "extensions",
    "addons", "current", "default", "common", "shared", "studio", "editor", "player", "browser", "connect", "core", "free", "pro", "lts", "version",
    "versions", "classes", "policies", "wow6432node", "appdatalow", "clients", "registeredapplications", "documents", "downloads", "desktopruntime",
    "main", "electron", "java", "javaw", "node", "edition", "freeedition", "portable", "helper", "agent", "daemon", "manager", "the", "inc", "llc", "ltd", "games", "game",
];

/// Names (normalized) a vendor's subfolder may not have, though other generic words may there
/// ("Rockstar Games\Launcher" is the launcher's).
const SUB_GENERIC: &[&str] = &[
    "data", "cache", "caches", "logs", "log", "temp", "tmp", "common", "shared", "settings", "user", "users", "update", "updater", "installer", "setup",
    "crashpad", "crashdumps", "windows", "packages", "programs", "credentials", "crypto", "protect", "vault", "systemcertificates", "x64", "x86",
    "arm64", "exe", "msi", "sdk", "machine",
];

/// Dot-folders in the user's profile that belong to many tools (or are the user's own).
const HOME_NEVER: &[&str] = &["ssh", "gnupg", "config", "local", "cache", "git", "aws", "azure", "kube", "wellknown", "vscodeshared", "templateengine"];

/// Words after the name in an app id folder ("bot.cline.app", "ai.opencode.desktop").
const ID_SUFFIX: &[&str] = &["app", "desktop", "client", "electron", "tauri", "beta", "prod", "stable"];

/// The words of the legal form after a publisher's name.
const LEGAL: &[&str] = &["inc", "llc", "ltd", "limited", "gmbh", "corp", "corporation", "co", "sro", "bv", "ag", "sa", "ab", "as", "oy", "pty", "plc", "srl", "kk", "the"];

/// Names an Other app's own data folders may have: its name without version or architecture
/// ("7-Zip 26.04 (x64)" is "7-Zip"), the name in its package id ("Anysphere.Cursor" is "Cursor")
/// and the dashed form Electron apps use ("LM Studio" is "lm-studio"). Generic names that many
/// programs share are never used.
pub fn other_names(display: &str, pkg: &str) -> Vec<String> {
    let base = cut_version(display);
    let from_id = pkg.split('.').nth(1).unwrap_or("").to_string();
    let mut out: Vec<String> = Vec::new();
    for n in [base.clone(), from_id, base.to_ascii_lowercase().replace(' ', "-")] {
        let ok = n.chars().count() >= 3 && !GENERIC.contains(&norm(&n).as_str()) && !n.contains(['\\', '/', ':']);
        if ok && !out.iter().any(|o| o.eq_ignore_ascii_case(&n)) {
            out.push(n);
        }
    }
    out
}

/// The name without the version and what follows it: "Inno Setup 6 (all users)" is "Inno Setup".
fn cut_version(display: &str) -> String {
    let mut cut: Vec<&str> = Vec::new();
    for w in display.split_whitespace() {
        let bare = w.trim_matches(|c| c == '(' || c == ')');
        // "26.04", "0.4.14+4", "v2": a version, though never the first word ("7-Zip").
        let digits = |t: &str| !t.is_empty() && t.starts_with(|c: char| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || c == '.' || c == '+');
        let versionish = !cut.is_empty() && (digits(bare) || (bare.len() > 1 && bare.starts_with(['v', 'V']) && digits(&bare[1..])));
        if versionish || w.starts_with('(') {
            break;
        }
        cut.push(w);
    }
    cut.join(" ").trim_end_matches(['-', ':', ',']).trim().to_string()
}

/// The year right after the name, as editions are named: "2024" of "SoftMaker FreeOffice 2024".
fn edition_year(display: &str) -> Option<&str> {
    let base = cut_version(display);
    let w = display.split_whitespace().nth(base.split_whitespace().count())?;
    (w.len() == 4 && (w.starts_with("19") || w.starts_with("20")) && w.chars().all(|c| c.is_ascii_digit()) && !base.is_empty()).then_some(w)
}

/// The normalized names an app's own folders and registry keys may have.
struct Keys {
    /// For folders right under AppData, ProgramData, the profile and HKCU\Software.
    top: Vec<String>,
    /// For a subfolder of the publisher's folder (these may be more generic words).
    sub: Vec<String>,
    /// The publisher's folder ("Piriform", "Jordan Russell").
    pubs: Vec<String>,
    /// Folders of the app's own programs (its command-line tool lives there).
    own: Vec<PathBuf>,
}

fn add(v: &mut Vec<String>, s: &str, generic: &[&str]) {
    let n = norm(s);
    if n.chars().count() >= 3 && !n.chars().all(|c| c.is_ascii_digit()) && !generic.contains(&n.as_str()) && !v.contains(&n) {
        v.push(n);
    }
}

/// A folder name with a version or build in it ("app-1.0.9", "jdk-17.0.2", "version-9ac8").
fn versioned(name: &str) -> bool {
    let low = name.to_ascii_lowercase();
    let dotted_digit = low.as_bytes().windows(2).any(|w| (w[0] == b'.' && w[1].is_ascii_digit()) || (w[0].is_ascii_digit() && w[1] == b'.'));
    dotted_digit || low.starts_with("version-") || low.starts_with("app-") || low.contains("_8wekyb3d8bbwe")
}

fn keys(inst: &Installed, r: &Roots) -> Keys {
    let (pkg, publisher, display) = match &inst.data {
        SourceData::Other { pkg, entry, .. } => (pkg.as_str(), entry.publisher.as_str(), entry.display_name.as_str()),
        _ => ("", "", ""),
    };
    let mut top = Vec::new();
    for n in other_names(&inst.name, pkg).into_iter().chain(other_names(display, "")) {
        add(&mut top, &n, GENERIC);
    }
    // An edition year is part of the name: "SoftMaker FreeOffice 2024" keeps its own key.
    for n in [inst.name.as_str(), display] {
        if let Some(y) = edition_year(n) {
            add(&mut top, &format!("{} {y}", cut_version(n)), GENERIC);
        }
    }
    if !pkg.is_empty() {
        if let Some(m) = (r.moniker)(pkg) {
            add(&mut top, &m, GENERIC);
        }
    }
    // The publisher's folder: its first word ("Discord Inc." is "Discord") or its whole name
    // without the legal form ("Brave Software Inc" is "BraveSoftware").
    let words: Vec<&str> = publisher.split([' ', ',']).map(|w| w.trim().trim_end_matches('.')).filter(|w| !w.is_empty()).collect();
    let mut pubs = Vec::new();
    if let Some(first) = words.iter().find(|w| !w.eq_ignore_ascii_case("the")) {
        add(&mut pubs, first, &["the"]);
        // "CPUID" keeps its apps in "CPUID Software".
        add(&mut pubs, &format!("{first}Software"), &["the"]);
    }
    let whole: String = words.iter().filter(|w| !LEGAL.contains(&norm(w).as_str())).copied().collect::<Vec<_>>().join(" ");
    add(&mut pubs, &whole, &["the"]);
    // "The Document Foundation" keeps its name whole; "MediaArea.net" is "MediaArea".
    let with_the: String = words.iter().filter(|w| w.eq_ignore_ascii_case("the") || !LEGAL.contains(&norm(w).as_str())).copied().collect::<Vec<_>>().join(" ");
    add(&mut pubs, &with_the, &["the"]);
    if let Some((name, tld)) = words.first().and_then(|w| w.rsplit_once('.')) {
        if (2..=6).contains(&tld.len()) && tld.chars().all(|c| c.is_ascii_alphabetic()) {
            add(&mut pubs, name, &["the"]);
        }
    }
    // Qt apps keep the name whole ("Menlo Research Pte. Ltd.").
    add(&mut pubs, publisher, &["the"]);
    // The program's and the install folder's names: "Code.exe" keeps "AppData\Roaming\Code",
    // "PotPlayerMini64.exe" "PotPlayerMini64", "Ubisoft Game Launcher" its own folder. Never the
    // publisher's name ("Maxon.exe" of Maxon's app manager is not all of Maxon's folder) and
    // never a generic or versioned folder name ("Application", "app-1.0.9").
    let mut files = Vec::new();
    if let Some(stem) = inst.exe.as_deref().and_then(Path::file_stem).map(|s| s.to_string_lossy().to_ascii_lowercase()) {
        if !["unins", "setup", "install", "update"].iter().any(|w| stem.contains(w)) && norm(&stem).chars().count() >= 4 {
            add(&mut files, &stem, GENERIC);
        }
    }
    if let Some(name) = inst.install_dir.as_deref().and_then(Path::file_name).map(|n| n.to_string_lossy().to_string()) {
        if !versioned(&name) {
            add(&mut files, &name, GENERIC);
        }
    }
    for k in files {
        if !pubs.contains(&k) && !top.contains(&k) {
            top.push(k);
        }
    }
    // The package id starts with the maker ("Perplexity.Comet" of "The Comet Authors"), though
    // its program may have that name ("GeoGebra.exe" keeps "Roaming\GeoGebra").
    if let Some((maker, _)) = pkg.split_once('.') {
        add(&mut pubs, maker, &["the"]);
    }
    let mut sub = Vec::new();
    for k in &top {
        add(&mut sub, k, SUB_GENERIC);
    }
    // "Jellyfin Server" under "Jellyfin" is "Server"; the package id's parts after the publisher.
    let pub_words: Vec<String> = words.iter().map(|w| norm(w)).collect();
    let short = cut_version(&inst.name);
    let rest: Vec<&str> = short.split_whitespace().filter(|w| !pub_words.contains(&norm(w))).collect();
    add(&mut sub, &rest.join(" "), SUB_GENERIC);
    for part in pkg.split('.').skip(1) {
        add(&mut sub, part, SUB_GENERIC);
    }
    let mut own: Vec<PathBuf> = inst.install_dir.iter().cloned().collect();
    own.extend(inst.exe.as_deref().and_then(Path::parent).map(Path::to_path_buf));
    own.extend(inst.uninstall.as_ref().and_then(|u| u.first()).map(PathBuf::from).filter(|p| p.is_absolute()).and_then(|p| p.parent().map(Path::to_path_buf)));
    Keys { top, sub, pubs, own }
}

/// Whether a folder or key name is one of these names: the whole name, the Electron updater's
/// folder ("lm-studio-updater"), its cache ("calibre-cache"), a version after it ("WizTree3",
/// "LibreOffice 26.8"), an id after it, a .NET settings folder of it, or an app id that ends
/// with it ("bot.cline.app", "com.lbjlaq.antigravity-tools").
fn name_is(raw: &str, keys: &[String]) -> bool {
    // "Waterfox-1de4eec8-1241-4177-a864-e594e8d1fb38": the app's name with an id after it.
    let raw = match raw.len().checked_sub(37).filter(|&i| raw.is_char_boundary(i)) {
        Some(i) if raw[i..].starts_with('-') && is_guid(&raw[i + 1..]) && i > 0 => &raw[..i],
        _ => raw,
    };
    // "SourceTree.exe_Url_pvbhzmuk...": where .NET keeps a program's user settings.
    let raw = dotnet_settings(raw).unwrap_or(raw);
    // "LibreOffice 26.8"
    let raw = strip_version(raw);
    let cache_of = raw.len().checked_sub(6).filter(|&i| raw.is_char_boundary(i) && raw[i..].eq_ignore_ascii_case("-cache")).map(|i| &raw[..i]);
    let n = norm(raw);
    let segs: Vec<&str> = raw.split('.').collect();
    keys.iter().any(|k| {
        if n == *k {
            return true;
        }
        // A Tauri app id: the app's web domain with a word after it ("jan.ai.app").
        let tld = |t: &str| (2..=6).contains(&t.len()) && t.chars().all(|c| c.is_ascii_lowercase());
        if segs.len() >= 3 && norm(segs[0]) == *k && tld(segs[1]) && segs[2..].iter().all(|s| ID_SUFFIX.contains(&norm(s).as_str())) {
            return true;
        }
        if k.chars().count() < 4 {
            return false;
        }
        // The updater of "evernote-client" and "tutanota-desktop" too.
        // Also with more than one id word ("theia-ide-electron-app-updater").
        let updater_of = n.strip_suffix("updater").and_then(|h| h.strip_prefix(k.as_str())).is_some_and(|mut rest| {
            while let Some(r) = ID_SUFFIX.iter().find_map(|s| rest.strip_prefix(s)) {
                rest = r;
            }
            rest.is_empty()
        });
        if updater_of || cache_of.is_some_and(|c| norm(c) == *k) {
            return true;
        }
        if let Some(rest) = n.strip_prefix(k.as_str()) {
            // "WizTree3", "foobar2000-v2"
            let rest = rest.strip_prefix('v').filter(|r| !r.is_empty()).unwrap_or(rest);
            if rest.len() <= 2 && rest.chars().all(|c| c.is_ascii_digit()) {
                return true;
            }
        }
        if segs.len() >= 3 && (2..=4).contains(&segs[0].len()) && segs[0].chars().all(|c| c.is_ascii_alphabetic()) {
            if let Some(i) = segs.iter().skip(1).position(|s| norm(s) == *k) {
                return segs[i + 2..].iter().all(|s| ID_SUFFIX.contains(&norm(s).as_str()));
            }
        }
        false
    })
}

/// The name without a dotted version after it: "LibreOffice" of "LibreOffice 26.8".
fn strip_version(raw: &str) -> &str {
    match raw.rsplit_once(' ') {
        Some((head, tail)) if !head.trim().is_empty() && tail.contains('.') && tail.starts_with(|c: char| c.is_ascii_digit()) && tail.chars().all(|c| c.is_ascii_digit() || c == '.') => head.trim_end(),
        _ => raw,
    }
}

/// Whether a subfolder or subkey has one of the app's full names, long enough to be its own
/// ("StreamDeck", "MEGAsync"), with or without a version after it, or is the .NET settings
/// folder of its program ("GARMIN_Corp\BaseCamp.exe_Url_urbejbdz...").
fn full_name(raw: &str, top: &[String]) -> bool {
    let raw = dotnet_settings(raw).unwrap_or(raw);
    let n = norm(strip_version(raw));
    n.chars().count() >= 6 && !GENERIC.contains(&n.as_str()) && !SUB_GENERIC.contains(&n.as_str()) && top.contains(&n)
}

/// The program's name in a .NET user settings folder: "SourceTree" of
/// "SourceTree.exe_Url_pvbhzmuknxzic1u2pbbpoooc2yeizzcm".
fn dotnet_settings(raw: &str) -> Option<&str> {
    let low = raw.to_ascii_lowercase();
    let i = low.find(".exe_")?;
    let (kind, hash) = low[i + 5..].split_once('_')?;
    (["url", "strongname", "path"].contains(&kind) && hash.len() == 32 && hash.chars().all(|c| c.is_ascii_alphanumeric()) && i > 0).then(|| &raw[..i])
}

/// "1de4eec8-1241-4177-a864-e594e8d1fb38"
fn is_guid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12]) && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Whether a folder or key is the publisher's: its name ("Piriform"), or a web domain of the publisher or app as Qt
/// and Flutter apps name it ("jellyfin.org", "plex.tv", "io.appflowy").
fn is_pub(raw: &str, pubs: &[String], top: &[String]) -> bool {
    let n = norm(raw);
    if pubs.contains(&n) {
        return true;
    }
    let tld = |t: &str| (2..=6).contains(&t.len()) && t.chars().all(|c| c.is_ascii_lowercase());
    let named = |w: &str| !GENERIC.contains(&norm(w).as_str()) && (pubs.contains(&norm(w)) || top.contains(&norm(w)));
    match raw.split_once('.') {
        // "jellyfin.org", or the other way round as Flutter apps name it ("io.appflowy").
        Some((first, last)) if !last.contains('.') => (tld(last) && named(first)) || (tld(first) && named(last)),
        _ => false,
    }
}

/// What is in the folders and the registry key apps keep settings in, read once per call.
struct Scan {
    /// (folder, name, needs admin rights, its subfolders) right under Roaming, Local, LocalLow
    /// and ProgramData.
    dirs: Vec<(PathBuf, String, bool, Vec<(PathBuf, String)>)>,
    /// Dot-folders in the user's profile.
    home: Vec<(PathBuf, String)>,
    /// Keys right under HKCU\Software, with their subkeys.
    reg: Vec<(String, Vec<String>)>,
}

fn subdirs(base: &Path) -> Vec<(PathBuf, String)> {
    let Ok(rd) = std::fs::read_dir(base) else { return vec![] };
    rd.flatten()
        .filter(|e| e.file_type().map(|t| t.is_dir() && !t.is_symlink()).unwrap_or(false))
        .map(|e| (e.path(), e.file_name().to_string_lossy().to_string()))
        .collect()
}

fn reg_exists(path: &str) -> bool {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey_with_flags(path, winreg::enums::KEY_READ).is_ok()
}

fn subkeys(path: &str) -> Vec<String> {
    use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};
    match winreg::RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(path, KEY_READ) {
        Ok(k) => k.enum_keys().flatten().collect(),
        Err(_) => vec![],
    }
}

impl Scan {
    fn new(r: &Roots) -> Scan {
        let mut dirs = Vec::new();
        for (base, elevated) in [(&r.roaming, false), (&r.local, false), (&r.local_low, false), (&r.program_data, true)] {
            dirs.extend(subdirs(base).into_iter().map(|(p, n)| {
                let subs = subdirs(&p);
                (p, n, elevated, subs)
            }));
        }
        let home = subdirs(&r.home).into_iter().filter(|(_, n)| n.starts_with('.')).collect();
        let reg = subkeys(&r.reg).into_iter().map(|k| {
            let subs = subkeys(&format!("{}\\{k}", r.reg));
            (k, subs)
        });
        Scan { dirs, home, reg: reg.collect() }
    }
}

/// What an app found by name keeps: folders, folders that need admin rights, and HKCU keys.
struct Found {
    dirs: Vec<PathBuf>,
    elevated: Vec<PathBuf>,
    reg: Vec<String>,
    /// Found by the app's full name in a folder or key that is not its publisher's: never a
    /// reason for another app to keep its own.
    weak: Vec<(PathBuf, bool)>,
    weak_reg: Vec<String>,
}

impl Found {
    fn all(self) -> impl Iterator<Item = String> {
        let reg: Vec<String> = self.reg.iter().map(|k| format!("hkcu\\{k}")).collect();
        self.dirs.into_iter().chain(self.elevated).map(|p| p.to_string_lossy().to_string()).chain(reg)
    }
}

fn found(inst: &Installed, r: &Roots, sc: &Scan) -> Found {
    let k = keys(inst, r);
    let mut f = Found { dirs: vec![], elevated: vec![], reg: vec![], weak: vec![], weak_reg: vec![] };
    let put = |f: &mut Found, p: PathBuf, elevated: bool| if elevated { f.elevated.push(p) } else { f.dirs.push(p) };
    for (path, name, elevated, subs) in &sc.dirs {
        if GENERIC.contains(&norm(name).as_str()) {
            // A vendor's folder ("Microsoft", "Google"): only the app's own subfolder in it, and
            // only through the vendor's name as the publisher.
            if !k.pubs.iter().any(|p| *p == norm(name)) {
                continue;
            }
        } else if name_is(name, &k.top) {
            put(&mut f, path.clone(), *elevated);
            continue;
        }
        let vendor = is_pub(name, &k.pubs, &k.top);
        for (sp, sn) in subs {
            // In the publisher's folder by any of the app's names; in any other folder only by
            // its full, distinctive name ("Elgato\StreamDeck", "Mega Limited\MEGAsync").
            if vendor && name_is(sn, &k.sub) {
                put(&mut f, sp.clone(), *elevated);
            } else if !GENERIC.contains(&norm(name).as_str()) && full_name(sn, &k.top) {
                f.weak.push((sp.clone(), *elevated));
            }
        }
    }
    // An Electron app's updater "Local\<name>-updater" goes with its "Roaming\<name>" (Tuta Mail's
    // "tutanota-desktop").
    let updated: Vec<String> = f.dirs.iter().filter(|d| d.parent() == Some(r.local.as_path())).filter_map(|d| Some(d.file_name()?.to_string_lossy().strip_suffix("-updater")?.to_string())).collect();
    for (path, name, elevated, _) in &sc.dirs {
        if !*elevated && path.parent() == Some(r.roaming.as_path()) && updated.iter().any(|u| u.eq_ignore_ascii_case(name)) && !f.dirs.contains(path) {
            f.dirs.push(path.clone());
        }
    }
    // ".cline", ".vscode", ".lmstudio": unless a separate command-line tool of that name uses it
    // too (the "claude" command of Claude Code shares ".claude" with the Claude app).
    for (path, name) in &sc.home {
        let bare = norm(name);
        if HOME_NEVER.contains(&bare.as_str()) || GENERIC.contains(&bare.as_str()) || !k.top.contains(&bare) {
            continue;
        }
        let cmd = name.trim_start_matches('.');
        let other_tool = r.path_dirs.iter().any(|d| {
            let has = ["exe", "cmd", "bat", "ps1"].iter().any(|x| d.join(format!("{cmd}.{x}")).is_file());
            let low = d.to_string_lossy().to_ascii_lowercase();
            let inside = |o: &Path| {
                let o = o.to_string_lossy().to_ascii_lowercase();
                low == o || low.starts_with(&format!("{o}\\"))
            };
            has && !inside(path) && !k.own.iter().any(|o| inside(o))
        });
        if !other_tool {
            put(&mut f, path.clone(), false);
        }
    }
    for (name, subs) in &sc.reg {
        let n = norm(name);
        if !GENERIC.contains(&n.as_str()) && name_is(name, &k.top) {
            f.reg.push(format!("{}\\{name}", r.reg));
            continue;
        }
        let vendor = is_pub(name, &k.pubs, &k.top);
        for s in subs {
            if vendor && name_is(s, &k.sub) {
                f.reg.push(format!("{}\\{name}\\{s}", r.reg));
            } else if !GENERIC.contains(&n.as_str()) && full_name(s, &k.top) {
                f.weak_reg.push(format!("{}\\{name}\\{s}", r.reg));
            }
        }
    }
    f
}

#[path = "prefs_known.rs"]
mod known;

#[cfg(test)]
#[path = "prefs_tests.rs"]
mod tests;
