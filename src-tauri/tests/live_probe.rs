//! Live check against this PC and the real update servers. Not run by default:
//! cargo test --test live_probe -- --ignored --nocapture

use updater_lib::sources::{adobe, blender, craft, tools};
use updater_lib::{version, winutil};

#[tokio::test]
#[ignore]
async fn detects_installed_apps_and_finds_latest_versions() {
    let un = winutil::uninstall_entries();
    let cat = adobe::fetch_catalog().await.expect("adobe catalog");
    let feed = craft::fetch(false).await.expect("craft feed");
    let mut all = adobe::scan(Some(&cat), &un);
    all.extend(craft::scan(&un));
    let root = std::path::PathBuf::from(std::env::var("ProgramFiles").unwrap()).join("Blender Foundation");
    let b = blender::scan(&un, &root);
    let bfeed = blender::fetch(&b, true).await.expect("blender feed");
    all.extend(b);
    let godot_root = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Programs").join("Godot");
    let t = tools::scan(&un, &godot_root);
    let tfeed = tools::fetch(&t).await.expect("more apps feed");
    all.extend(t);
    for i in &all {
        let latest = match i.source {
            updater_lib::model::Source::Adobe => adobe::latest(i, &all, &cat),
            updater_lib::model::Source::Craft => craft::latest(i, &feed),
            updater_lib::model::Source::Blender => blender::latest(i, &all, &bfeed),
            updater_lib::model::Source::Tools => tools::latest(i, &all, &tfeed),
            updater_lib::model::Source::Other => None,
        };
        let l = latest.as_ref().map(|l| l.version.clone()).unwrap_or_else(|| "?".into());
        let mark = if latest.as_ref().is_some_and(|l| version::newer(&l.version, &i.version)) { "UPDATE" } else { "" };
        println!(
            "{:<22} {:<28} {:<12} -> {:<12} {:6} missing={} exe={:?} note={:?}",
            i.id, i.name, i.version, l, mark, i.missing, i.exe.as_ref().map(|e| e.display().to_string()), latest.and_then(|l| l.note)
        );
    }
    println!("--- catalog");
    for c in adobe::catalog_entries(&cat, &all).iter().chain(craft::catalog_entries(&feed, &all).iter()).chain(blender::catalog_entries(&bfeed, &all, true).iter()).chain(tools::catalog_entries(&tfeed, &all).iter()) {
        println!("{:<24} {:<34} {:<10} pre={} size={:?}", c.id, c.name, c.version, c.prerelease, c.size);
    }
    assert!(!all.is_empty());
}

#[tokio::test]
#[ignore]
async fn adobe_updates_use_remote_update_manager() {
    let un = winutil::uninstall_entries();
    let cat = adobe::fetch_catalog().await.unwrap();
    let all = adobe::scan(Some(&cat), &un);
    let des = all.iter().find(|i| i.id.starts_with("adobe:SBSTD")).expect("Designer installed");
    let latest = adobe::latest(des, &all, &cat).unwrap();
    let plan = adobe::plan_install(
        adobe::InstallRequest {
            sap: "SBSTD",
            version: &latest.version,
            platform: "win64",
            kind: updater_lib::model::JobKind::Update,
            app_id: des.id.clone(),
            name: des.name.clone(),
            current: Some(des),
            remove_after: None,
            lang: "en_US",
            install_root: des.install_dir.as_ref().unwrap().parent().unwrap().to_path_buf(),
        },
        &cat,
    )
    .await
    .unwrap();
    // Updates to the newest version run through Adobe's Remote Update Manager.
    println!("{} -> {}: {:?}", des.version, plan.to_version, plan.steps);
    assert!(plan.steps.iter().any(|st| matches!(st, updater_lib::sources::Step::Run { exe, .. } if exe.to_string_lossy().contains("RemoteUpdateManager"))));
}

#[tokio::test]
#[ignore]
async fn version_history_works_for_every_installed_app() {
    let un = winutil::uninstall_entries();
    let cat = adobe::fetch_catalog().await.unwrap();
    let mut all: Vec<_> = adobe::scan(Some(&cat), &un).into_iter().filter(|i| !i.missing).collect();
    all.extend(craft::scan(&un));
    let root = std::path::PathBuf::from(std::env::var("ProgramFiles").unwrap()).join("Blender Foundation");
    let b = blender::scan(&un, &root);
    let bfeed = blender::fetch(&b, false).await.unwrap();
    all.extend(b);
    let godot_root = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Programs").join("Godot");
    let t = tools::scan(&un, &godot_root);
    let tfeed = tools::fetch(&t).await.unwrap();
    all.extend(t);
    for i in &all {
        let list = match &i.data {
            updater_lib::sources::SourceData::Adobe { sap, platform, .. } => {
                let mut l = adobe::versions(sap, platform, Some(&i.version), &cat);
                if let Some(first) = l.first_mut() {
                    if let Some(n) = updater_lib::notes::adobe(first.notes_url.as_deref().unwrap_or(""), &first.version).await {
                        first.notes = n.lines;
                    }
                }
                l
            }
            updater_lib::sources::SourceData::Craft { key, .. } => craft::versions(key, Some(&i.version), false).await.unwrap_or_default(),
            updater_lib::sources::SourceData::Blender { .. } => blender::versions(i, &bfeed).await.unwrap_or_default(),
            updater_lib::sources::SourceData::Tools { .. } => tools::versions(i, &tfeed).await.unwrap_or_default(),
            updater_lib::sources::SourceData::Other { .. } => vec![],
        };
        let installed = list.iter().any(|v| v.installed);
        println!("{:<26} {} versions, installed marked: {installed}", i.name, list.len());
        for v in list.iter().take(2) {
            println!("    {:<10} {:<22} notes={} first={:?} url={:?}", v.version, v.date.clone().unwrap_or_default(), v.notes.len(), v.notes.first().map(|s| s.chars().take(70).collect::<String>()), v.notes_url);
        }
        assert!(!list.is_empty(), "{} has no versions", i.name);
    }
    for id in ["blender:older", "blender:5.2", "craft:gridcraft", "adobe:ILST:30"] {
        let n = match id.split(':').next().unwrap() {
            "blender" => blender::catalog_versions(id, &bfeed).await.map(|v| v.len()),
            "craft" => craft::versions("gridcraft", None, false).await.map(|v| v.len()),
            _ => Ok(adobe::versions("ILST", "win64", None, &cat).len()),
        };
        println!("catalog {id}: {:?} versions", n);
    }
}

/// The five "More apps": what is installed here (read only), the newest versions, and the
/// version lists of the Get apps picker. Nothing is downloaded or installed.
#[tokio::test]
#[ignore]
async fn more_apps_detection_latest_versions_and_catalog_lists() {
    let un = winutil::uninstall_entries();
    let godot_root = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Programs").join("Godot");
    let inst = tools::scan(&un, &godot_root);
    println!("--- detected: {} apps", inst.len());
    for i in &inst {
        println!("{:<28} {:<14} dir={:?} exe_exists={} uninstall={:?}", i.id, i.version, i.install_dir.as_ref().map(|d| d.display().to_string()), i.exe.as_ref().is_some_and(|e| e.exists()), i.uninstall);
    }
    let obs = inst.iter().find(|i| i.id == "tools:obs").expect("OBS Studio is installed here");
    println!("OBS Studio detected as {}", obs.version);
    assert_eq!(obs.version, "32.2.2");
    assert!(!obs.missing);

    let feed = tools::fetch(&inst).await.expect("more apps feed");
    println!("--- latest versions");
    for d in tools::APPS {
        let l = feed.list(d.key);
        println!("{:<8} {} releases, newest {:?}, newest stable {:?}", d.key, l.len(), l.first().map(|r| r.version.clone()), l.iter().find(|r| !r.prerelease).map(|r| r.version.clone()));
        assert!(!l.is_empty(), "{} has no releases", d.key);
    }
    for i in &inst {
        let l = tools::latest(i, &inst, &feed);
        println!("{:<28} {} -> {:?}", i.id, i.version, l.as_ref().map(|l| (l.version.clone(), l.size, l.note.clone())));
    }
    println!("--- catalog");
    for c in tools::catalog_entries(&feed, &inst) {
        println!("{:<16} {:<14} size={:?} installed={} pre={}", c.id, c.version, c.size, c.installed, c.prerelease);
    }
    println!("--- catalog versions");
    for d in tools::APPS {
        let v = tools::catalog_versions(d.key, &feed, &inst).await.unwrap_or_else(|e| panic!("{}: {e:#}", d.key));
        let with_notes = v.iter().filter(|x| !x.notes.is_empty()).count();
        println!("{:<8} {} versions ({} pre-release), {} with notes", d.key, v.len(), v.iter().filter(|x| x.prerelease).count(), with_notes);
        for x in v.iter().take(2) {
            println!("    {:<14} {:<22} size={:?} first={:?}", x.version, x.date.clone().unwrap_or_default(), x.size, x.notes.first().map(|s| s.chars().take(80).collect::<String>()));
        }
        assert!(!v.is_empty());
        assert!(v.len() <= 25 + 3);
    }
    println!("--- versions of installed apps");
    for i in &inst {
        let v = tools::versions(i, &feed).await.unwrap_or_default();
        println!("{:<28} {} versions, installed marked: {}, latest marked: {}", i.id, v.len(), v.iter().any(|x| x.installed), v.iter().any(|x| x.latest));
    }
    // Plans are built, never run.
    let plan = tools::plan_install("tools:unity", None, updater_lib::model::JobKind::Install, None, &inst, &feed, tools::Ctx { godot_root: &godot_root }).await.expect("unity plan");
    println!("unity plan: {} {:?} download size {:?} md5 {:?}", plan.app_id, plan.steps.iter().map(|s| s.label().to_string()).collect::<Vec<_>>(), plan.downloads[0].size, plan.downloads[0].md5);
    let plan = tools::plan_install("tools:krita", None, updater_lib::model::JobKind::Install, None, &inst, &feed, tools::Ctx { godot_root: &godot_root }).await.expect("krita plan");
    println!("krita plan: size {:?} sha256 {:?}", plan.downloads[0].size, plan.downloads[0].sha256);
    assert!(plan.downloads[0].size.is_some() && plan.downloads[0].sha256.is_some());
}

/// Creative Cloud itself: found from its uninstall entry, the catalog has a newer version, and the
/// update plan downloads Adobe's setup and accepts it only with a valid Adobe signature. Nothing
/// is installed: the setup is downloaded into a temp folder, checked and deleted.
#[tokio::test]
#[ignore]
async fn creative_cloud_updates_with_adobes_signed_setup() {
    use updater_lib::download::{self, Control, Outcome, Progress, Throttle};
    let un = winutil::uninstall_entries();
    let cat = adobe::fetch_catalog().await.unwrap();
    let all = adobe::scan(Some(&cat), &un);
    let cc = all.iter().find(|i| i.id.starts_with("adobe:KCCC")).expect("Creative Cloud found");
    let latest = adobe::latest(cc, &all, &cat).expect("catalog has Creative Cloud");
    println!("Creative Cloud {} -> {}", cc.version, latest.version);
    let plan = adobe::plan_install(
        adobe::InstallRequest {
            sap: "KCCC",
            version: &latest.version,
            platform: "win64",
            kind: updater_lib::model::JobKind::Update,
            app_id: cc.id.clone(),
            name: cc.name.clone(),
            current: Some(cc),
            remove_after: None,
            lang: "en_US",
            install_root: cc.install_dir.clone().unwrap(),
        },
        &cat,
    )
    .await
    .unwrap();
    assert!(matches!(&plan.steps[0], updater_lib::sources::Step::Run { args, elevated: true, .. } if args == &vec!["--silent".to_string()]));
    let mut d = plan.downloads[0].clone();
    assert_eq!(d.signer.as_deref(), Some("Adobe Inc."));
    let dir = std::env::temp_dir().join(format!("u3d-cc-{}", std::process::id()));
    d.dest = dir.join("setup.exe");
    let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
    let r = download::fetch(&d, std::sync::Arc::new(Progress::new()), std::sync::Arc::new(Throttle::new()), rx).await;
    let _ = std::fs::remove_dir_all(&dir);
    assert!(matches!(r, Ok(Outcome::Done)), "{:?}", r.err());
}

/// Other apps: how much of this PC the winget catalog covers, and with what change lists.
/// cargo test --release --test live_probe other_apps -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn other_apps_coverage_and_changelogs() {
    use std::collections::BTreeMap;
    use updater_lib::sources::{other, winget};
    let un = winutil::uninstall_entries();
    let cat = adobe::fetch_catalog().await.ok();
    let mut handled = adobe::scan(cat.as_ref(), &un);
    handled.extend(craft::scan(&un));
    handled.extend(blender::scan(&un, &std::path::PathBuf::from(std::env::var("ProgramFiles").unwrap()).join("Blender Foundation")));
    handled.extend(tools::scan(&un, &std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Programs").join("Godot")));
    let t0 = std::time::Instant::now();
    let feed = other::fetch(&handled).await.expect("winget catalog");
    println!("catalog read in {:.1} s, {} packages fetched", t0.elapsed().as_secs_f64(), feed.pkgs.len());
    let idx = winget::Index::open(&winget::index_path()).unwrap();
    println!("index: {} packages, {} KB on disk", idx.count(), std::fs::metadata(winget::index_path()).unwrap().len() / 1024);
    let rep = other::match_all(&un, &idx, &handled, &winutil::msi_upgrade_codes());
    let t1 = std::time::Instant::now();
    let installed = other::scan(&un, &handled, Some(&feed));
    println!("scan (index lookups and matching) took {} ms", t1.elapsed().as_millis());

    println!("\n--- matched");
    let mut unsupported: Vec<(String, String, String)> = Vec::new();
    for m in &rep.matches {
        let pf = feed.get(&m.pkg.id);
        let man = pf.and_then(|p| p.latest.as_ref());
        let shown = installed.iter().find(|i| i.id == format!("other:{}", m.pkg.id));
        match (shown, man) {
            (Some(i), Some(man)) => {
                let l = other::latest(i, &feed).map(|l| l.version).unwrap_or_default();
                let inst = other::choose(man, &other::Want::for_installed(i)).ok();
                let mark = if other::is_newer(&l, &i.version) { "UPDATE" } else { "" };
                println!(
                    "{:<44} {:<36} {:<16} {:<14} -> {:<14} {:6} {:<5} {} {:?}",
                    m.entry.display_name.chars().take(44).collect::<String>(),
                    m.pkg.id,
                    m.how.label(),
                    i.version,
                    l,
                    mark,
                    inst.map(other::kind_of).unwrap_or("-"),
                    if i.uninstall.is_some() { "uninstall" } else { "-" },
                    inst.map(|x| x.arch.as_str())
                );
            }
            (None, Some(man)) => {
                let why = other::choose(man, &other::Want::for_entry(&m.entry, m.upgrade_code.as_deref())).err().unwrap_or_default();
                unsupported.push((m.entry.display_name.clone(), m.pkg.id.clone(), why));
            }
            _ => unsupported.push((m.entry.display_name.clone(), m.pkg.id.clone(), "catalog entry could not be read".into())),
        }
    }
    println!("\n--- matched, but not updatable here");
    for (n, p, why) in &unsupported {
        println!("{n:<44} {p:<36} {why}");
    }
    let mut by: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (e, m) in &rep.misses {
        let key = match m {
            other::Miss::Ambiguous(_) | other::Miss::Unconfirmed(_) | other::Miss::Duplicate(_) | other::Miss::Excluded(_) => m.label(),
            _ => m.label(),
        };
        by.entry(key).or_default().push(format!("{} {}", e.display_name, e.display_version));
    }
    println!("\n--- not matched, by reason");
    for (k, v) in &by {
        println!("{k} ({}):", v.len());
        for n in v {
            println!("    {n}");
        }
    }

    println!("\n--- change lists of the newest version");
    let mut sources: BTreeMap<&str, usize> = BTreeMap::new();
    let mut shown_counts: BTreeMap<&str, usize> = BTreeMap::new();
    let (mut all_versions, mut all_with_lines) = (0usize, 0usize);
    for i in &installed {
        let Some(man) = feed.get(match &i.data {
            updater_lib::sources::SourceData::Other { pkg, .. } => pkg,
            _ => "",
        }).and_then(|p| p.latest.as_ref()) else { continue };
        let (lines, url, src) = other::probe_notes(man).await;
        let key = if !lines.is_empty() { src } else if url.is_some() { "link only" } else { "none" };
        *sources.entry(key).or_default() += 1;
        // What the version list shows (it can also use a notes page another version names).
        let list = other::versions(i, &feed).await.unwrap_or_default();
        let shown = list.first().map(|v| v.notes.first().cloned().unwrap_or_default()).unwrap_or_default();
        let shown_key = if shown == other::NO_NOTES { "none" } else if shown == other::SEE_NOTES { "link only" } else { "lines" };
        *shown_counts.entry(shown_key).or_default() += 1;
        let with_lines = list.iter().filter(|v| v.notes.first().is_some_and(|n| n != other::NO_NOTES && n != other::SEE_NOTES)).count();
        all_versions += list.len();
        all_with_lines += with_lines;
        println!("{:<36} {:<10} {:<9} {:<9} {}/{} versions with lines  {:?} {:?}", man.id, man.version, key, shown_key, with_lines, list.len(), lines.first().map(|s| s.chars().take(50).collect::<String>()), url);
    }

    println!("
--- install plans for the updates (built, not run)");
    for i in installed.iter().filter(|i| other::latest(i, &feed).is_some_and(|l| other::is_newer(&l.version, &i.version))) {
        match other::plan_install(&i.id, None, updater_lib::model::JobKind::Update, Some(i), Some(&feed)).await {
            Ok(p) => {
                for s in &p.steps {
                    if let updater_lib::sources::Step::Run { exe, args, elevated, .. } = s {
                        println!("{:<36} {:?} {} elevated={} fallback={} size={:?} kind={:?}", i.id, exe.file_name().unwrap_or_default(), args.join(" "), elevated, p.fallback.is_some(), p.downloads[0].size, p.kind);
                    }
                }
            }
            Err(e) => println!("{:<36} PLAN FAILED: {e:#}", i.id),
        }
        if let Some(u) = &i.uninstall {
            println!("{:<36}   uninstall: {}", "", u.join(" "));
        }
    }

    let apps = un.iter().filter(|e| other::is_app(e)).count();
    let handled_n = rep.misses.iter().filter(|(_, m)| *m == other::Miss::Handled).count();
    println!("\n=== coverage");
    println!("apps in the uninstall list: {apps} ({} system parts and updates not counted)", rep.hidden);
    println!("handled by the full sources: {handled_n}");
    println!("matched to the catalog: {} of {} ({:.0}%)", rep.matches.len(), apps - handled_n, 100.0 * rep.matches.len() as f64 / (apps - handled_n).max(1) as f64);
    println!("listed as Other apps (updatable): {}", installed.len());
    println!("updates available: {}", installed.iter().filter(|i| other::latest(i, &feed).is_some_and(|l| other::is_newer(&l.version, &i.version))).count());
    println!("change list of the newest version, by source: {sources:?}");
    println!("newest version as shown in the version list: {shown_counts:?}");
    println!("all listed versions with a change list: {all_with_lines} of {all_versions}");
    assert!(!rep.matches.is_empty());
}
