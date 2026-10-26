//! What a program leaves behind after its uninstaller ran, found and shown before anything is
//! deleted. A thing counts as the program's when it points into the program's own folder, or
//! when what it points at is gone and its name fits the program's. Every item passes the checks
//! in `safety` first; services count only when their program is inside the program's folder.

use super::safety::{key_refused, Guard};
use super::{program_of, strictly_under, under, Hive, Target};
use crate::sources::locate::name_fit;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// What is left of the install folder.
    Folder,
    /// The entry in the uninstall list itself.
    Entry,
    Shortcut,
    /// A Run key value or a Startup folder shortcut.
    Startup,
    Registry,
    Service,
    Task,
    AppPath,
    FileType,
    /// Settings and add-ons (`prefs`).
    Settings,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: Kind,
    /// The path or key as shown.
    pub label: String,
    /// Why it is listed.
    pub detail: String,
    pub size: Option<u64>,
    /// Ticked when the list opens.
    pub checked: bool,
    /// Always deleted ("Remove from list" removes the entry).
    pub required: bool,
    pub elevated: bool,
    #[serde(skip)]
    pub target: Target,
}

/// The program that was removed.
#[derive(Clone, Debug, Default)]
pub struct Subject {
    pub name: String,
    pub publisher: String,
    pub install_dir: Option<PathBuf>,
    /// Its uninstall entry (the key as Windows stores it).
    pub entry: Option<(Hive, String)>,
    /// Program files of the program (its main exe): their names fit too.
    pub exes: Vec<PathBuf>,
    /// The program its uninstall command ran (a browser for its web app, a launcher for its game)
    pub runner: Option<PathBuf>,
}

/// An uninstaller's own program, which may stay in a folder that is otherwise left over.
fn uninstaller_like(p: &Path) -> bool {
    let n = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    ["unins", "uninst", "remove", "setup", "update", "maintenance"].iter().any(|w| n.contains(w))
}

/// A service as the registry records it.
#[derive(Clone, Debug)]
pub struct ServiceRec {
    pub name: String,
    pub display: String,
    pub image: String,
}

/// A scheduled task and the program it starts.
#[derive(Clone, Debug)]
pub struct TaskRec {
    pub path: String,
    pub command: String,
}

/// Where to look (tests point these at their own folders and keys).
pub struct Env {
    pub guard: Guard,
    /// Start menu folders (walked three levels deep).
    pub start_menus: Vec<PathBuf>,
    /// Desktop and Quick Launch folders (only the shortcuts right in them).
    pub desktops: Vec<PathBuf>,
    /// Startup folders: shortcuts in them count as startup entries.
    pub startup: Vec<PathBuf>,
    pub run_keys: Vec<(Hive, String)>,
    pub app_paths: Vec<(Hive, String)>,
    /// "Software\Classes" keys (file types, "Applications\x.exe").
    pub classes: Vec<(Hive, String)>,
    /// "Software" keys apps keep their own keys in.
    pub software: Vec<(Hive, String)>,
    pub services: Vec<ServiceRec>,
    pub tasks: Vec<TaskRec>,
    /// Names of the other installed programs: a name that fits one of them is shared.
    pub other_names: Vec<String>,
    pub shortcut_target: Box<dyn Fn(&Path) -> Option<PathBuf>>,
}

impl Env {
    /// This PC's places. `others` are the other installed programs' folders.
    pub fn system(others: Vec<PathBuf>, other_names: Vec<String>) -> Env {
        let env = |n: &str| std::env::var_os(n).map(PathBuf::from);
        let start_menus = crate::sources::locate::start_menu_dirs();
        let startup: Vec<PathBuf> = start_menus.iter().map(|d| d.join("Startup")).collect();
        // Where Windows keeps the desktops (OneDrive's Desktop folder when it backs them up).
        let mut desktops = crate::sources::locate::desktop_dirs();
        desktops.extend(env("APPDATA").map(|p| p.join("Microsoft\\Internet Explorer\\Quick Launch")));
        let run = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
        let run32 = "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Run";
        let ap = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths";
        let ap32 = "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\App Paths";
        Env {
            guard: Guard::system(others),
            start_menus,
            desktops,
            startup,
            run_keys: vec![(Hive::Hkcu, run.into()), (Hive::Hkcu, format!("{run}Once")), (Hive::Hklm, run.into()), (Hive::Hklm, format!("{run}Once")), (Hive::Hklm, run32.into()), (Hive::Hklm, format!("{run32}Once"))],
            app_paths: vec![(Hive::Hkcu, ap.into()), (Hive::Hklm, ap.into()), (Hive::Hklm, ap32.into())],
            classes: vec![(Hive::Hkcu, "Software\\Classes".into()), (Hive::Hklm, "SOFTWARE\\Classes".into())],
            software: vec![(Hive::Hkcu, "Software".into()), (Hive::Hklm, "SOFTWARE".into()), (Hive::Hklm, "SOFTWARE\\WOW6432Node".into())],
            services: read_services(),
            tasks: read_tasks(),
            other_names,
            shortcut_target: Box::new(crate::sources::locate::shortcut_target),
        }
    }
}

// ---------------------------------------------------------------- reading this PC

/// Every service with a program (`ImagePath`).
pub fn read_services() -> Vec<ServiceRec> {
    let Some(base) = super::open(Hive::Hklm, "SYSTEM\\CurrentControlSet\\Services") else { return vec![] };
    base.enum_keys()
        .flatten()
        .filter_map(|name| {
            let k = base.open_subkey_with_flags(&name, winreg::enums::KEY_READ).ok()?;
            let image: String = k.get_value("ImagePath").ok()?;
            let display: String = k.get_value("DisplayName").unwrap_or_default();
            Some(ServiceRec { name, display, image })
        })
        .collect()
}

/// Every scheduled task (in every folder) and the program it starts.
pub fn read_tasks() -> Vec<TaskRec> {
    use windows::core::BSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::TaskScheduler::{ITaskFolder, ITaskService, TaskScheduler};
    use windows::Win32::System::Variant::VARIANT;
    fn walk(f: &ITaskFolder, out: &mut Vec<TaskRec>, depth: u8) {
        unsafe {
            if let Ok(tasks) = f.GetTasks(1) {
                let n = tasks.Count().unwrap_or(0);
                for i in 1..=n {
                    let Ok(t) = tasks.get_Item(&VARIANT::from(i)) else { continue };
                    let (Ok(path), Ok(xml)) = (t.Path(), t.Xml()) else { continue };
                    if let Some(c) = crate::elevate::task_command(&xml.to_string()) {
                        out.push(TaskRec { path: path.to_string(), command: c.to_string_lossy().into_owned() });
                    }
                }
            }
            if depth == 0 {
                return;
            }
            if let Ok(subs) = f.GetFolders(0) {
                let n = subs.Count().unwrap_or(0);
                for i in 1..=n {
                    if let Ok(s) = subs.get_Item(&VARIANT::from(i)) {
                        walk(&s, out, depth - 1);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    unsafe {
        // Joins the multithreaded apartment and stays (see `locate::shortcut_target`).
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let r = (|| -> windows::core::Result<()> {
            let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let none = VARIANT::default();
            svc.Connect(&none, &none, &none, &none)?;
            let root = svc.GetFolder(&BSTR::from("\\"))?;
            walk(&root, &mut out, 6);
            Ok(())
        })();
        if let Err(e) = r {
            log::warn!("scheduled tasks: {e}");
        }
    }
    out
}

// ---------------------------------------------------------------- the scan

struct Ctx<'a> {
    s: &'a Subject,
    env: &'a Env,
    dir: Option<&'a Path>,
}

impl Ctx<'_> {
    /// The program's names: its own, and its program files'.
    fn names(&self) -> Vec<String> {
        let mut v = vec![self.s.name.clone()];
        v.extend(self.s.exes.iter().filter_map(|e| e.file_stem().map(|s| s.to_string_lossy().into_owned())));
        v
    }
    /// The name fits the program (and no other installed program as well).
    fn fits(&self, name: &str) -> bool {
        self.names().iter().any(|n| name_fit(n, name) >= 1) && !self.env.other_names.iter().any(|o| name_fit(o, name) == 2)
    }
    /// Under a publisher's key (Software\Google\...): a key that only partly fits ("Chrome" for
    /// "Chrome Remote Desktop Host") counts only when no other installed program fits it at all,
    /// so another app of the same publisher keeps its key.
    fn fits_under_publisher(&self, name: &str) -> bool {
        self.fits_exactly(name) || (self.names().iter().any(|n| name_fit(n, name) >= 1) && !self.env.other_names.iter().any(|o| name_fit(o, name) >= 1))
    }
    fn fits_exactly(&self, name: &str) -> bool {
        self.names().iter().any(|n| name_fit(n, name) == 2) && !self.env.other_names.iter().any(|o| name_fit(o, name) == 2)
    }
    fn inside(&self, p: &Path) -> bool {
        self.dir.is_some_and(|d| under(p, d))
    }
    /// A path that belongs to the program: inside its folder, or gone and named like it.
    /// Returns whether it is gone.
    fn points_here(&self, p: &Path, name: &str) -> Option<bool> {
        let gone = !p.exists();
        if self.inside(p) {
            return Some(gone);
        }
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        (gone && (self.fits(name) || self.fits(&stem))).then_some(gone)
    }
}

fn item(kind: Kind, target: Target, label: String, detail: String, size: Option<u64>, checked: bool) -> Item {
    Item { id: target.id(), kind, elevated: target.elevated(), target, label, detail, size, checked, required: false }
}

fn key_label(hive: Hive, path: &str) -> String {
    format!("{}\\{path}", hive.short())
}

fn shortcuts(d: &Path, depth: u8, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(d) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth > 0 {
                shortcuts(&p, depth - 1, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
            out.push(p);
        }
    }
}

/// Everything the program left that exists now. `prefs` are its settings and add-ons as found
/// by `prefs::steps`.
/// Shortcuts anywhere in the Start menu, on the desktops or in Startup whose program is gone,
/// left by apps removed long ago (or removed without their uninstaller). A target on a drive
/// that is not there right now (an unplugged disk) or on a network share is never called gone.
pub fn dead_shortcuts(env: &Env) -> Vec<Item> {
    let mut lnks = Vec::new();
    for d in &env.start_menus {
        shortcuts(d, 3, &mut lnks);
    }
    for d in env.desktops.iter().chain(env.startup.iter()) {
        shortcuts(d, 0, &mut lnks);
    }
    lnks.sort();
    lnks.dedup();
    let mut out = Vec::new();
    for l in lnks {
        if env.guard.shortcut_refused(&l).is_some() {
            continue;
        }
        let Some(t) = (env.shortcut_target)(&l) else { continue };
        if !target_is_gone(&t) {
            continue;
        }
        let startup = env.startup.iter().any(|d| strictly_under(&l, d));
        let why = format!("Starts {}, which is gone", t.display());
        out.push(item(if startup { Kind::Startup } else { Kind::Shortcut }, Target::File { path: l.clone() }, l.display().to_string(), why, l.metadata().ok().map(|m| m.len()), true));
    }
    out
}

/// Is a shortcut's target really gone? Only for a full local path whose drive is there.
pub fn target_is_gone(t: &Path) -> bool {
    let s = t.to_string_lossy();
    if s.is_empty() || !t.is_absolute() || s.starts_with(r"\\") {
        return false;
    }
    let root: PathBuf = t.components().take(2).collect();
    root.is_dir() && !t.exists()
}

pub fn scan(s: &Subject, env: &Env, prefs: &[crate::sources::Step]) -> Vec<Item> {
    // A folder that still holds the program the uninstall command ran is that program's, not
    // what is left of this one (a browser's folder after its web app, a launcher's after a game).
    let runs_there = |d: &Path| s.runner.as_deref().is_some_and(|r| r.is_file() && r.starts_with(d) && !uninstaller_like(r));
    let dir = s.install_dir.as_deref().filter(|d| env.guard.dir_refused(d).is_none() && !runs_there(d));
    let cx = Ctx { s, env, dir };
    let mut out: Vec<Item> = Vec::new();

    // The install folder.
    if let Some(d) = dir.filter(|d| d.is_dir()) {
        out.push(item(Kind::Folder, Target::Dir { path: d.to_path_buf() }, d.display().to_string(), "What is left of its install folder".into(), Some(crate::winutil::dir_size(d)), true));
    }

    // Its entry in the uninstall list.
    if let Some((hive, key)) = &s.entry {
        if super::key_exists(*hive, key) && key_refused(*hive, key).is_none() {
            out.push(item(Kind::Entry, Target::Key { hive: *hive, path: key.clone() }, key_label(*hive, key), "Its entry in the list of installed programs".into(), None, true));
        }
    }

    // Shortcuts: Start menu, desktops, Startup.
    let mut lnks = Vec::new();
    for d in &env.start_menus {
        shortcuts(d, 3, &mut lnks);
    }
    for d in env.desktops.iter().chain(env.startup.iter()) {
        shortcuts(d, 0, &mut lnks);
    }
    lnks.sort();
    lnks.dedup();
    for l in lnks {
        if env.guard.shortcut_refused(&l).is_some() {
            continue;
        }
        let Some(t) = (env.shortcut_target)(&l) else { continue };
        if t.as_os_str().is_empty() || !t.is_absolute() {
            continue;
        }
        let name = l.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Some(gone) = cx.points_here(&t, &name) else { continue };
        let startup = env.startup.iter().any(|d| strictly_under(&l, d));
        let why = if gone { format!("Starts {}, which is gone", t.display()) } else { format!("Starts {}", t.display()) };
        out.push(item(if startup { Kind::Startup } else { Kind::Shortcut }, Target::File { path: l.clone() }, l.display().to_string(), why, l.metadata().ok().map(|m| m.len()), gone));
    }

    // Run keys.
    for (hive, path) in &env.run_keys {
        // Only the startup lists are given (`Env::system`); deleting checks `value_refused` again.
        let Some(k) = super::open(*hive, path) else { continue };
        for (name, _) in k.enum_values().flatten() {
            let Ok(cmd) = k.get_value::<String, _>(&name) else { continue };
            let Some(prog) = program_of(&cmd) else { continue };
            let Some(gone) = cx.points_here(&prog, &name) else { continue };
            let why = if gone { format!("Starts {} at sign-in, which is gone", prog.display()) } else { format!("Starts {} at sign-in", prog.display()) };
            out.push(item(Kind::Startup, Target::Value { hive: *hive, path: path.clone(), name: name.clone() }, format!("{} ({name})", key_label(*hive, path)), why, None, gone));
        }
    }

    // App Paths.
    for (hive, path) in &env.app_paths {
        let Some(base) = super::open(*hive, path) else { continue };
        for sub in base.enum_keys().flatten() {
            let Ok(k) = base.open_subkey_with_flags(&sub, winreg::enums::KEY_READ) else { continue };
            let Ok(v) = k.get_value::<String, _>("") else { continue };
            let Some(prog) = program_of(&format!("\"{}\"", v.trim().trim_matches('"'))) else { continue };
            let Some(gone) = cx.points_here(&prog, &sub) else { continue };
            if !gone {
                continue;
            }
            let full = format!("{path}\\{sub}");
            if key_refused(*hive, &full).is_some() {
                continue;
            }
            out.push(item(Kind::AppPath, Target::Key { hive: *hive, path: full.clone() }, key_label(*hive, &full), format!("Lets Windows start {}, which is gone", prog.display()), None, true));
        }
    }

    // File types and "Open with" entries whose program was in the removed folder.
    if dir.is_some() {
        for (hive, path) in &env.classes {
            let Some(base) = super::open(*hive, path) else { continue };
            let mut keys: Vec<String> = base.enum_keys().flatten().filter(|n| !n.starts_with('.')).map(|n| format!("{path}\\{n}")).collect();
            if let Some(apps) = super::open(*hive, &format!("{path}\\Applications")) {
                keys.extend(apps.enum_keys().flatten().map(|n| format!("{path}\\Applications\\{n}")));
            }
            for full in keys {
                let Some(cmd) = super::open(*hive, &format!("{full}\\shell\\open\\command")).and_then(|k| k.get_value::<String, _>("").ok()) else { continue };
                let Some(prog) = program_of(&cmd) else { continue };
                if !cx.inside(&prog) || prog.exists() || key_refused(*hive, &full).is_some() {
                    continue;
                }
                out.push(item(Kind::FileType, Target::Key { hive: *hive, path: full.clone() }, key_label(*hive, &full), format!("Opens files with {}, which is gone", prog.display()), None, true));
            }
        }
    }

    // Its own keys under Software: "<Publisher>\<App>" or "<App>".
    let publisher_words: Vec<String> = {
        let p = crate::sources::winget::normalize_publisher(&s.publisher);
        let first = s.publisher.split(|c: char| !c.is_alphanumeric()).find(|w| !w.is_empty()).unwrap_or("").to_ascii_lowercase();
        [p, first].into_iter().filter(|w| w.len() >= 3).collect()
    };
    let in_prefs: Vec<String> = prefs.iter().filter_map(|st| if let crate::sources::Step::RemoveUserRegKey { path, .. } = st { Some(path.to_ascii_lowercase()) } else { None }).collect();
    for (hive, base) in &env.software {
        let Some(b) = super::open(*hive, base) else { continue };
        for top in b.enum_keys().flatten() {
            let low: String = top.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect();
            let mut found: Vec<String> = Vec::new();
            if cx.fits_exactly(&top) {
                found.push(format!("{base}\\{top}"));
            } else if !low.is_empty() && publisher_words.contains(&low) {
                if let Ok(pk) = b.open_subkey_with_flags(&top, winreg::enums::KEY_READ) {
                    found.extend(pk.enum_keys().flatten().filter(|a| cx.fits_under_publisher(a)).map(|a| format!("{base}\\{top}\\{a}")));
                }
            }
            for full in found {
                if key_refused(*hive, &full).is_some() || (*hive == Hive::Hkcu && in_prefs.contains(&full.to_ascii_lowercase())) {
                    continue;
                }
                out.push(item(Kind::Registry, Target::Key { hive: *hive, path: full.clone() }, key_label(*hive, &full), "Its own registry key".into(), None, true));
            }
        }
    }

    // Services whose program is inside its folder.
    if let Some(d) = dir {
        for sv in &env.services {
            let Some(prog) = program_of(&sv.image) else { continue };
            if under(&prog, d) {
                let shown = if sv.display.is_empty() || sv.display.starts_with('@') { sv.name.clone() } else { format!("{} ({})", sv.display, sv.name) };
                out.push(item(Kind::Service, Target::Service { name: sv.name.clone() }, shown, format!("A service that runs {}", prog.display()), None, true));
            }
        }
    }

    // Scheduled tasks.
    for t in &env.tasks {
        let Some(prog) = program_of(&format!("\"{}\"", super::expand_env(&t.command))) else { continue };
        let leaf = t.path.rsplit('\\').next().unwrap_or(&t.path).to_string();
        let Some(gone) = cx.points_here(&prog, &leaf) else { continue };
        let why = if gone { format!("Runs {}, which is gone", prog.display()) } else { format!("Runs {}", prog.display()) };
        out.push(item(Kind::Task, Target::Task { path: t.path.clone() }, t.path.clone(), why, None, gone));
    }

    // Settings and add-ons, unticked: they are the user's (the uninstall dialog has its own box).
    for st in prefs {
        let (target, size) = match st {
            crate::sources::Step::RemoveDir { path, .. } => {
                if env.guard.dir_refused(path).is_some() {
                    continue;
                }
                (Target::Dir { path: path.clone() }, Some(crate::winutil::dir_size(path)))
            }
            crate::sources::Step::RemoveFile { path } => (Target::File { path: path.clone() }, path.metadata().ok().map(|m| m.len())),
            crate::sources::Step::RemoveUserRegKey { path, .. } => (Target::Key { hive: Hive::Hkcu, path: path.clone() }, None),
            _ => continue,
        };
        let label = match &target {
            Target::Key { hive, path } => key_label(*hive, path),
            Target::Dir { path } | Target::File { path } => path.display().to_string(),
            _ => continue,
        };
        out.push(item(Kind::Settings, target, label, "Its settings and add-ons".into(), size, false));
    }

    // Each thing once, and nothing inside a folder that is listed already.
    let mut seen = std::collections::HashSet::new();
    out.retain(|i| seen.insert(i.id.clone()));
    let dirs: Vec<PathBuf> = out.iter().filter_map(|i| if let Target::Dir { path } = &i.target { Some(path.clone()) } else { None }).collect();
    out.retain(|i| match &i.target {
        Target::Dir { path } | Target::File { path } => !dirs.iter().any(|d| strictly_under(path, d)),
        _ => true,
    });
    out
}
