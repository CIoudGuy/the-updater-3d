//! A live, read-only look at this PC: every program in Windows' list, how many a source lists,
//! the Other programs, broken ones, and what a leftover scan would offer for a few (nothing is
//! deleted). Run by hand:
//! `cargo test --release --lib live_programs_probe -- --ignored --nocapture`

use super::{leftovers, programs, run};
use crate::sources::{adobe, blender, craft, other, tools};
use crate::{paths, winutil};
use std::path::PathBuf;

#[test]
#[ignore]
fn live_programs_probe() {
    let s: crate::model::Settings = std::fs::read(paths::settings_file()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let un = winutil::uninstall_entries();
    let apps = un.iter().filter(|e| other::is_app(e)).count();
    let mut v = Vec::new();
    v.extend(adobe::scan(None, &un));
    v.extend(craft::scan(&un));
    v.extend(blender::scan(&un, &PathBuf::from(&s.blender_install_dir)));
    v.extend(tools::scan(&un, &PathBuf::from(&s.godot_install_dir)));
    let found = other::scan(&un, &v, None);
    v.extend(found);
    v.retain(|i| !i.missing);
    let t0 = std::time::Instant::now();
    let progs = programs::scan(&un, &v);
    let took = t0.elapsed();
    let broken: Vec<_> = progs.iter().filter(|p| p.missing).collect();
    let silent = progs.iter().filter(|p| programs::choice_for(p).is_some_and(|c| c.silent)).count();
    println!("uninstall entries: {} ({} apps of their own)", un.len(), apps);
    println!("listed by a source (matched): {}", v.len());
    println!("Other programs (unmatched): {} (listed in {:?}), silent uninstall known for {silent}", progs.len(), took);
    println!("broken (uninstaller missing): {}", broken.len());
    for b in &broken {
        println!("  broken: {} | {}", b.name, b.tagline.as_deref().unwrap_or(""));
    }
    let with_size = progs.iter().filter(|p| p.size_hint.is_some()).count();
    let with_dir = progs.iter().filter(|p| p.install_dir.is_some()).count();
    let with_exe = progs.iter().filter(|p| p.exe.is_some()).count();
    println!("Other programs with size {with_size}, folder {with_dir}, program file {with_exe}");
    // Leftover previews for a few (as if each were uninstalled now; read only).
    let mut all = v.clone();
    all.extend(progs.iter().cloned());
    let pick: Vec<_> = broken.iter().map(|b| (*b).clone()).chain(progs.iter().filter(|p| !p.missing).take(4).cloned()).collect();
    for p in pick {
        let (dirs, names) = run::others(&p, &un);
        let rest: Vec<_> = all.iter().filter(|i| i.id != p.id).cloned().collect();
        let prefs = crate::prefs::steps(&p, &rest, &crate::prefs::Roots::system());
        let t0 = std::time::Instant::now();
        let env = leftovers::Env::system(dirs, names);
        let items = leftovers::scan(&run::subject(&p), &env, &prefs);
        println!("leftovers of {} ({:?}): {} items", p.name, t0.elapsed(), items.len());
        for i in items {
            println!("  {:?} {} | {} | {:?} | ticked {}", i.kind, i.label, i.detail, i.size, i.checked);
        }
    }
    // Shortcuts whose program is gone (MEGAsync's leftovers and the like), anywhere in the Start
    // menu or on the desktops.
    let env = leftovers::Env::system(vec![], vec![]);
    let mut gone = 0;
    let mut lnks = Vec::new();
    fn walk(d: &std::path::Path, depth: u8, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && depth > 0 {
                walk(&p, depth - 1, out);
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                out.push(p);
            }
        }
    }
    for d in &env.start_menus {
        walk(d, 3, &mut lnks);
    }
    for d in &env.desktops {
        walk(d, 0, &mut lnks);
    }
    for l in &lnks {
        if let Some(t) = crate::sources::locate::shortcut_target(l) {
            if t.is_absolute() && !t.exists() {
                gone += 1;
                println!("  shortcut to a missing program: {} -> {}", l.display(), t.display());
            }
        }
    }
    println!("shortcuts: {} checked, {gone} point at missing programs", lnks.len());
    println!("services read: {}, scheduled tasks read: {}", env.services.len(), env.tasks.len());
}

/// The PC-wide dead shortcut cleanup on this PC, read only:
/// `cargo test --lib live_dead_shortcuts_probe -- --ignored --nocapture`
#[test]
#[ignore]
fn live_dead_shortcuts_probe() {
    let env = super::leftovers::Env::system(vec![], vec![]);
    println!("desktops: {:?}", env.desktops);
    for i in super::leftovers::dead_shortcuts(&env) {
        println!("{:?} {} | {}", i.kind, i.label, i.detail);
    }
}
