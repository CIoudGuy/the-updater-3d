//! A live, read-only look at every app this PC has installed (all sources): how many have no
//! program file, folder, date or icon. Run by hand:
//! `cargo test --release --lib live_installed_apps_probe -- --ignored --nocapture`

use super::*;
use crate::sources::{adobe, blender, craft, tools};

fn bad_place(p: &Path) -> bool {
    let low = p.to_string_lossy().to_ascii_lowercase();
    low.contains("package cache") || low.contains("\\windows\\installer\\")
}

#[test]
#[ignore]
fn live_installed_apps_probe() {
    let s: crate::model::Settings = std::fs::read(paths::settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let un = winutil::uninstall_entries();
    let mut v = Vec::new();
    v.extend(adobe::scan(None, &un));
    v.extend(craft::scan(&un));
    v.extend(blender::scan(&un, &PathBuf::from(&s.blender_install_dir)));
    v.extend(tools::scan(&un, &PathBuf::from(&s.godot_install_dir)));
    // As `scan`, with this PC's Start menu and App Paths (tests leave them out otherwise).
    let path = winget::index_path();
    let idx = Index::open(&path).expect("the winget index; start the app once to download it");
    let r = match_on_disk(&un, &idx, &path, &v);
    let hints = std::cell::OnceCell::new();
    let _ = hints.set(crate::sources::locate::Hints::read());
    let found = installed_copies_with(&r.matches, None, &hints);
    v.extend(found);
    v.retain(|i| !i.missing);
    let mut before = Counts::default();
    let mut after = Counts::default();
    for i in &v {
        let store = i.store.is_some();
        let catalog = i.source == Source::Other && {
            let stem = crate::sources::icons::file_stem(pkg_of(i));
            ["png", "svg", "webp", "jpg", "gif", "ico"].iter().any(|e| paths::icons().join(format!("{stem}.{e}")).is_file())
        };
        let now = Facts { exe: i.exe.clone(), dir: i.install_dir.clone(), date: i.updated_at.clone(), icon: i.exe.as_ref().is_some_and(|e| e.is_file()) || i.icon_remote.is_some() || catalog };
        // What the code before this change gave: the entry as the uninstall list has it (no
        // bundle folding), the icon's program file, the install location as is, any 8 digits
        // as a date, and no catalog icon for installed apps.
        let old = match &i.data {
            SourceData::Other { entry, .. } => {
                let orig = un.iter().find(|u| u.key == entry.key && u.hive == entry.hive).cloned().unwrap_or_else(|| (**entry).clone());
                let exe = old_program_file(&orig);
                let d = &orig.install_date;
                Facts {
                    dir: old_install_dir(&orig),
                    date: (d.len() == 8 && d.chars().all(|c| c.is_ascii_digit())).then(|| format!("{}-{}-{}T00:00:00Z", &d[0..4], &d[4..6], &d[6..8])),
                    icon: exe.as_ref().is_some_and(|e| e.is_file()) || i.icon_remote.is_some(),
                    exe,
                }
            }
            _ => now.clone(),
        };
        let why_now = after.add(&now, store, &i.display_name_for_probe());
        let why_old = before.add(&old, store, &i.display_name_for_probe());
        println!("{:<44} {:<7} exe={:?} dir={:?} date={:?} [{}] (before [{}]: exe={:?})", i.id, format!("{:?}", i.source), now.exe, now.dir, now.date, why_now, why_old, old.exe);
    }
    println!("BEFORE {}", before.line(v.len()));
    println!("AFTER  {}", after.line(v.len()));
}

trait ProbeName {
    fn display_name_for_probe(&self) -> String;
}

impl ProbeName for Installed {
    fn display_name_for_probe(&self) -> String {
        match &self.data {
            SourceData::Other { entry, .. } => entry.display_name.clone(),
            _ => self.name.clone(),
        }
    }
}

#[derive(Clone)]
struct Facts {
    exe: Option<PathBuf>,
    dir: Option<PathBuf>,
    date: Option<String>,
    icon: bool,
}

#[derive(Default)]
struct Counts {
    no_exe: usize,
    no_dir: usize,
    no_date: usize,
    bad_date: usize,
    no_icon: usize,
    cache: usize,
    helper: usize,
}

impl Counts {
    fn add(&mut self, f: &Facts, store: bool, name: &str) -> String {
        let mut why = Vec::new();
        let exe_ok = f.exe.as_ref().is_some_and(|e| e.is_file() && e.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")));
        if !exe_ok && !store {
            self.no_exe += 1;
            why.push("no exe");
        }
        if f.dir.is_none() && !store {
            self.no_dir += 1;
            why.push("no dir");
        }
        match &f.date {
            None => {
                self.no_date += 1;
                why.push("no date");
            }
            Some(d) if chrono::DateTime::parse_from_rfc3339(d).is_err() => {
                self.bad_date += 1;
                why.push("bad date");
            }
            _ => {}
        }
        if !f.icon {
            self.no_icon += 1;
            why.push("no icon");
        }
        if f.exe.as_deref().is_some_and(bad_place) || f.dir.as_deref().is_some_and(bad_place) {
            self.cache += 1;
            why.push("cache path");
        }
        if f.exe.as_deref().is_some_and(|e| crate::sources::locate::helper_exe(e, name)) {
            self.helper += 1;
            why.push("helper exe");
        }
        why.join(",")
    }

    fn line(&self, total: usize) -> String {
        format!("apps {total} | no exe {} | no dir {} | no date {} | bad date {} | no icon {} | cache paths {} | helper exe {}", self.no_exe, self.no_dir, self.no_date, self.bad_date, self.no_icon, self.cache, self.helper)
    }
}

/// `program_file` and `install_dir` as they were before this change.
fn old_program_file(e: &winutil::UninstallEntry) -> Option<PathBuf> {
    let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"');
    let low = icon.to_ascii_lowercase();
    if !(low.ends_with(".exe") || low.ends_with(".ico")) || low.contains("\\installer\\") || low.contains("package cache") {
        return None;
    }
    let p = PathBuf::from(icon);
    p.exists().then_some(p)
}

pub(super) fn old_install_dir(e: &winutil::UninstallEntry) -> Option<PathBuf> {
    if !e.install_location.trim().is_empty() {
        let d = winutil::pathbuf(&e.install_location);
        if d.is_dir() && d.components().count() >= 3 {
            return Some(d);
        }
    }
    old_program_file(e).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"))).and_then(|p| p.parent().map(Path::to_path_buf)).filter(|d| d.components().count() >= 3)
}

/// The portable copies this PC's shortcuts lead to, and what the catalog matches them to.
#[test]
#[ignore]
fn live_portable_probe() {
    let un = winutil::uninstall_entries();
    let path = winget::index_path();
    let idx = Index::open(&path).expect("index");
    let shortcuts = crate::sources::locate::shortcuts_in(&crate::sources::portable::shortcut_dirs());
    let found = crate::sources::portable::candidates(&shortcuts, &un, &[], &crate::sources::portable::read_program);
    let classic = match_all(&un, &idx, &[], &winutil::msi_upgrade_codes()).matches;
    for e in &found {
        let r = match_entry(e, &idx, &HashMap::new());
        println!("CANDIDATE {} | {} | {} | {} -> {:?}", e.display_name, e.publisher, e.display_version, e.key, r.as_ref().map(|(row, _, _)| idx.by_row(*row).map(|p| p.id)).map_err(|m| m.label()));
    }
    for m in portable_matches(&found, &idx, &classic, &[]) {
        println!("PORTABLE MATCH {} -> {} words={:?}", m.entry.key, m.pkg.id, crate::sources::portable::build_words(&m.entry));
    }
}
