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
            updater_lib::model::Source::Other | updater_lib::model::Source::Programs | updater_lib::model::Source::Drivers => None,
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
    // Epic's new launcher (Preview) is installed here, per user. It is found by its own
    // setup program as uninstaller, never as the old MSI launcher.
    let epic = inst.iter().find(|i| i.id == "tools:epic").expect("Epic Games Launcher (Preview) is installed here");
    println!("Epic Games Launcher (Preview) detected as {} in {:?}", epic.version, epic.install_dir);
    // 0.97.0 when this was written; the app has since updated it.
    assert!(epic.version.starts_with("0."), "{}", epic.version);
    assert!(!epic.missing && epic.exe.as_ref().is_some_and(|e| e.ends_with("EpicGamesLauncher.exe")));

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
    let epic_rel = feed.list("epic").first().expect("Epic's latest launcher version resolves");
    println!("epic latest {} size {:?} url {}", epic_rel.version, epic_rel.size, epic_rel.url);
    assert!(epic_rel.size.is_some_and(|n| n > 50_000_000));
    let up = tools::latest(epic, &inst, &feed).expect("epic latest");
    // Current once installed: no update offered for the same or a newer copy.
    assert!(!updater_lib::version::newer(&epic.version, &up.version), "latest {} is not older than installed {}", up.version, epic.version);
    let ep = tools::plan_install("tools:epic", None, updater_lib::model::JobKind::Update, Some(epic), &inst, &feed, tools::Ctx { godot_root: &godot_root }).await.expect("epic plan");
    println!("epic plan: {:?} signer {:?} size {:?}", ep.steps.iter().map(|s| s.label().to_string()).collect::<Vec<_>>(), ep.downloads[0].signer, ep.downloads[0].size);
    assert_eq!(ep.downloads[0].signer.as_deref(), Some("Epic Games Inc."));
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
    match tools::plan_install("tools:unity", None, updater_lib::model::JobKind::Install, None, &inst, &feed, tools::Ctx { godot_root: &godot_root }).await {
        Ok(plan) => println!("unity plan: {} {:?} download size {:?} md5 {:?}", plan.app_id, plan.steps.iter().map(|s| s.label().to_string()).collect::<Vec<_>>(), plan.downloads.last().and_then(|d| d.size), plan.downloads.last().and_then(|d| d.md5.clone())),
        // The newest LTS editor is installed on this PC already.
        Err(e) => assert!(e.to_string().contains("already installed"), "unity plan: {e}"),
    }
    let plan = tools::plan_install("tools:krita", None, updater_lib::model::JobKind::Install, None, &inst, &feed, tools::Ctx { godot_root: &godot_root }).await.expect("krita plan");
    println!("krita plan: size {:?} sha256 {:?}", plan.downloads[0].size, plan.downloads[0].sha256);
    assert!(plan.downloads[0].size.is_some() && plan.downloads[0].sha256.is_some());
}

/// Creative Cloud itself: found from its uninstall entry, the newest full build comes from its
/// update service, and the plan downloads Adobe's offline installer for that exact build with the
/// size and MD5 Adobe's server reports, then runs its Set-up.exe silently. Nothing is downloaded
/// or installed here.
#[tokio::test]
#[ignore]
async fn creative_cloud_updates_with_adobes_offline_installer() {
    use updater_lib::sources::Step;
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
    // The full build (6.11.0.478.6), not the catalog's short "6.11.0".
    assert!(latest.version.split('.').count() >= 5, "{}", latest.version);
    let d = &plan.downloads[0];
    println!("{} {:?} bytes md5 {:?}", d.url, d.size, d.md5);
    assert!(d.url.contains("/StandaloneBuilds/ACCC/ESD/") && d.url.ends_with(".zip"));
    assert!(d.size.unwrap_or(0) > 100_000_000 && d.md5.as_deref().is_some_and(|m| m.len() == 32));
    assert!(matches!(&plan.steps[0], Step::ExtractZip { .. }));
    assert!(matches!(&plan.steps[1], Step::Run { exe, args, elevated: true, .. } if exe.ends_with("Set-up.exe") && args == &vec!["--silent".to_string()]));
    // Get apps lists Creative Cloud, marked installed, newest build only.
    let entry = adobe::catalog_entries(&cat, &all).into_iter().find(|e| e.id.starts_with("adobe:KCCC")).expect("Creative Cloud in Get apps");
    assert!(entry.installed && !entry.pick_version && entry.version == latest.version);
}

/// Shared components (Camera Raw and the rest): found, compared with the catalog, updated through
/// Remote Update Manager while waiting for the apps Adobe lists as conflicting. Nothing runs.
#[tokio::test]
#[ignore]
async fn adobe_components_are_detected_and_update_with_their_blockers() {
    use updater_lib::sources::Step;
    let un = winutil::uninstall_entries();
    let cat = adobe::fetch_catalog().await.unwrap();
    let all = adobe::scan(Some(&cat), &un);
    let comps: Vec<_> = all.iter().filter(|i| adobe::is_component(i.id.split(':').nth(1).unwrap_or(""))).collect();
    for c in &comps {
        let l = adobe::latest(c, &all, &cat);
        println!("{} {} -> {:?} uninstall={}", c.name, c.version, l.as_ref().map(|l| &l.version), c.uninstall.is_some());
        assert!(c.uninstall.is_none() && !c.missing);
    }
    let acr = comps.iter().find(|i| i.id.starts_with("adobe:ACR")).expect("Camera Raw found");
    let latest = adobe::latest(acr, &all, &cat).expect("catalog has Camera Raw");
    let plan = adobe::plan_install(
        adobe::InstallRequest {
            sap: "ACR",
            version: &latest.version,
            platform: "win32",
            kind: updater_lib::model::JobKind::Update,
            app_id: acr.id.clone(),
            name: acr.name.clone(),
            current: Some(acr),
            remove_after: None,
            lang: "en_US",
            install_root: std::path::PathBuf::new(),
        },
        &cat,
    )
    .await
    .unwrap();
    println!("Camera Raw waits for {:?}", plan.block_names);
    assert!(plan.block_names.iter().any(|n| n == "photoshop.exe"));
    assert!(plan.block_dirs.is_empty());
    assert!(plan.steps.iter().any(|s| matches!(s, Step::Run { args, .. } if args.iter().any(|a| a == "--productVersions=ACR"))));
}

/// Unity Hub: newest version with its SHA-512, every version's notes from Unity's page, and a
/// Unity editor install that brings Hub along when Hub is missing. Nothing is downloaded.
#[tokio::test]
#[ignore]
async fn unity_hub_comes_with_unity_and_has_full_notes() {
    use updater_lib::sources::Step;
    let un = winutil::uninstall_entries();
    let godot_root = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap()).join("Programs").join("Godot");
    let installed = tools::scan(&un, &godot_root);
    let feed = tools::fetch(&installed).await.unwrap();
    let hub = feed.list("unityhub");
    println!("Unity Hub {} ({} versions, newest has {} notes)", hub[0].version, hub.len(), hub[0].notes.len());
    assert!(hub[0].sha512.as_deref().is_some_and(|s| s.len() == 128) && hub[0].size.unwrap_or(0) > 50_000_000);
    assert!(hub.len() > 10 && hub.iter().filter(|r| !r.notes.is_empty()).count() > 10);
    let has_hub = installed.iter().any(|i| i.id == "tools:unityhub");
    let plan = tools::plan_install("tools:unity", None, updater_lib::model::JobKind::Install, None, &installed, &feed, tools::Ctx { godot_root: &godot_root }).await;
    match plan {
        Ok(p) => {
            let hub_steps = p.steps.iter().filter(|s| matches!(s, Step::Run { exe, .. } if exe.to_string_lossy().contains("UnityHubSetup"))).count();
            println!("Unity {} plan: {} downloads, Hub installed here: {has_hub}", p.to_version, p.downloads.len());
            assert_eq!(hub_steps, if has_hub { 0 } else { 1 });
        }
        // The newest LTS editor is installed already on this PC.
        Err(e) => println!("Unity plan: {e}"),
    }
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
    let mut feed = other::fetch(&handled, None).await.expect("winget catalog");
    // The first start may not read the whole starter list in time; the next read has the rest.
    for _ in 0..3 {
        if !feed.unfinished {
            break;
        }
        feed = other::fetch(&handled, None).await.expect("winget catalog");
    }
    println!("catalog read in {:.1} s, {} packages fetched", t0.elapsed().as_secs_f64(), feed.pkgs.len());
    // Apps that are also in the Store (they have a package family name) but have a normal
    // installer stay in the starter list, with a checksum.
    for id in ["Mozilla.Firefox", "SlackTechnologies.Slack", "Microsoft.PowerToys", "Notion.Notion", "Rufus.Rufus"] {
        let Some((pf, _)) = feed.popular.iter().find(|(p, _)| p.pkg.id.eq_ignore_ascii_case(id)) else {
            // Not in the list: only right when the catalog has no installer that can run silently.
            let idx = winget::Index::open(&winget::index_path()).unwrap();
            let p = idx.by_id(id).unwrap_or_else(|| panic!("{id} is not in the catalog"));
            let v = winget::versions(&p).await.unwrap();
            let m = winget::manifest(v.first().unwrap()).await.unwrap();
            let why = other::choose(&m, &other::Want::fresh()).err().unwrap_or_else(|| panic!("{id} has a usable installer but is missing"));
            let kinds: Vec<&str> = m.installers.iter().map(other::kind_of).collect();
            println!("starter {id:<26} left out: {why} (installers: {kinds:?})");
            continue;
        };
        let m = pf.latest.as_ref().expect("manifest");
        let i = other::choose(m, &other::Want::fresh()).expect("a usable installer");
        println!("starter {id:<26} {:<6} {:<14} sha256 {} {}", other::kind_of(i), m.version, i.sha256, i.url);
        assert!(i.sha256.len() == 64 && i.sha256.chars().all(|c| c.is_ascii_hexdigit()), "{id}: no checksum");
    }
    let idx = winget::Index::open(&winget::index_path()).unwrap();
    println!("index: {} packages, {} KB on disk", idx.count(), std::fs::metadata(winget::index_path()).unwrap().len() / 1024);
    let rep = other::match_all(&un, &idx, &handled, &winutil::msi_upgrade_codes());
    let t1 = std::time::Instant::now();
    let installed = other::scan(&un, &handled, Some(&feed));
    println!("scan (index lookups and matching) took {} ms", t1.elapsed().as_millis());

    // Every update gets a download size from its server, so the update total is real.
    let urls = other::update_urls(&installed, &feed);
    let mut sized = 0;
    for (key, url) in &urls {
        let s = other::download_size(url).await;
        println!("size {key:<60} {s:?}");
        sized += s.is_some() as usize;
    }
    println!("download sizes: {sized} of {} updates", urls.len());
    assert!(urls.is_empty() || sized * 10 >= urls.len() * 8, "most updates have a size");

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
        let newest = list.first().map(|v| v.notes.clone()).unwrap_or_default();
        let heads = newest.iter().filter(|l| l.starts_with('#')).count();
        println!("{:<36} {:<10} {:<9} {:<9} {}/{} versions with lines, newest {} lines {} headings  {:?} {:?}", man.id, man.version, key, shown_key, with_lines, list.len(), newest.len(), heads, lines.first().map(|s| s.chars().take(50).collect::<String>()), url);
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

    // Browse all: a random sample of 100 entries of the whole catalog, read as the cards read
    // them (newest manifest, then the icon search). The icons found are not saved.
    use updater_lib::sources::{browse, icons};
    let list = browse::list().expect("browse list");
    let mut counts = vec![0usize; browse::CATEGORIES.len()];
    for i in &list.items {
        counts[i.category] += 1;
    }
    println!("\n=== browse all: {} entries of {} packages", list.items.len(), list.packages);
    for (n, c) in browse::CATEGORIES.iter().zip(&counts) {
        println!("    {n:<16} {c}");
    }
    let mut seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64 | 1;
    let mut picked = std::collections::BTreeSet::new();
    while picked.len() < 100.min(list.items.len()) {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        picked.insert((seed % list.items.len() as u64) as usize);
    }
    let sample: Vec<winget::Pkg> = picked.iter().map(|i| list.items[*i].pkg.clone()).collect();
    use futures_util::StreamExt;
    let got: Vec<(String, Option<browse::Detail>, bool)> = futures_util::stream::iter(sample)
        .map(|p| async move {
            let m = async { winget::manifest(winget::versions(&p).await.ok()?.first()?).await.ok() }.await;
            let d = m.as_ref().map(|m| browse::Detail::from_manifest(&p.hash, m));
            let pages: Vec<String> = m.iter().flat_map(|m| [m.package_url.clone(), m.publisher_url.clone()]).flatten().filter(|u| u.starts_with("http")).collect();
            let icon = tokio::time::timeout(std::time::Duration::from_secs(60), icons::find(&p.id, m.as_ref().and_then(|m| m.icon_url.as_deref()), &pages)).await.is_ok_and(|r| r.is_ok());
            (p.id, d, icon)
        })
        .buffer_unordered(6)
        .collect()
        .await;
    let read = got.iter().filter(|g| g.1.is_some()).count();
    let short = got.iter().filter(|g| g.1.as_ref().is_some_and(|d| d.short.is_some())).count();
    let long = got.iter().filter(|g| g.1.as_ref().is_some_and(|d| d.long.is_some())).count();
    let line = got.iter().filter(|g| g.1.as_ref().is_some_and(|d| browse::tagline_for(d.short.as_deref(), browse::popular_line(&g.0), d.publisher.as_deref()).is_some())).count();
    let ok = got.iter().filter(|g| g.1.as_ref().is_some_and(|d| d.blocked.is_none())).count();
    let icon = got.iter().filter(|g| g.2).count();
    for (id, d, i) in &got {
        println!("{id:<44} {:<5} {:<5} {:<5} {}", d.as_ref().is_some_and(|d| d.short.is_some()), d.as_ref().is_some_and(|d| d.long.is_some()), i, d.as_ref().and_then(|d| d.blocked.clone()).unwrap_or_default());
    }
    println!("sample of {}: manifest read {read}, short description {short}, long description {long}, card line {line}, installable silently {ok}, icon {icon}", got.len());
}

/// The first release of a Blender series carries its whole release notes, by topic.
#[tokio::test]
#[ignore]
async fn blender_first_release_has_the_full_notes() {
    let m = updater_lib::notes::blender_major("5.2").await;
    let topics: Vec<&String> = m.lines.iter().filter(|l| l.starts_with("# ")).collect();
    let items = m.lines.iter().filter(|l| !l.starts_with('#')).count();
    println!("Blender 5.2.0: {} topics, {} items, date {:?}", topics.len(), items, m.date);
    for l in m.lines.iter().take(14) {
        println!("  {l}");
    }
    assert!(topics.len() >= 10 && items >= 100, "{} topics {} items", topics.len(), items);
    assert!(m.date.is_some());
}

/// How many listed versions carry a real change list (not a placeholder), per app, for the
/// release notes sources: Adobe, Blender 5.2 and older, Krita and the Craft apps.
#[tokio::test]
#[ignore]
async fn release_notes_coverage() {
    use updater_lib::notes;
    let placeholder = |l: &[String]| {
        l.is_empty()
            || l.len() == 1
                && (l[0].starts_with("No change list") || l[0].starts_with("Bug-fix release") || l[0].starts_with("First release of Blender") || l[0].starts_with("Maintenance release"))
    };
    let mut report: Vec<(String, usize, usize)> = Vec::new();
    // Every line shown, to check that none has a long dash.
    let mut all: Vec<String> = Vec::new();
    let cat = adobe::fetch_catalog().await.expect("adobe catalog");
    let saps = ["PHSP", "ILST", "PPRO", "AEFT", "AME", "AUDT", "IDSN", "KBRG", "LTRM", "LRCC", "ACR", "FRSC", "KCCC", "SBSTA", "SBSTP", "SBSTD", "SHPR", "PHSPBETA", "ILSTPR", "AEFTBETA", "PPROBETA", "AMEBETA"];
    for sap in saps {
        let mut list = adobe::versions(sap, "win64", None, &cat);
        if list.is_empty() {
            // Shared components such as Camera Raw are listed for win32 only.
            list = adobe::versions(sap, "win32", None, &cat);
        }
        let found = futures_util::future::join_all(list.iter().take(25).map(|v| {
            let (url, ver) = (v.notes_url.clone(), v.version.clone());
            async move {
                match url {
                    Some(u) => notes::adobe(&u, &ver).await,
                    None => None,
                }
            }
        }))
        .await;
        let with = found.iter().filter(|n| n.as_ref().is_some_and(|n| !n.lines.is_empty())).count();
        all.extend(found.iter().flatten().flat_map(|n| n.lines.iter().cloned()));
        if let Some((v, n)) = list.iter().zip(&found).find(|(_, n)| n.as_ref().is_some_and(|n| !n.lines.is_empty())) {
            let n = n.as_ref().unwrap();
            println!("  {sap} {} ({:?}): {} lines, first {:?}", v.version, n.date, n.lines.len(), n.lines.first());
        }
        report.push((format!("Adobe {sap}"), with, list.len().min(25)));
    }
    let bfeed = blender::fetch(&[], false).await.expect("blender feed");
    for id in ["blender:5.2", "blender:older"] {
        let list = blender::catalog_versions(id, &bfeed).await.unwrap_or_default();
        for v in &list {
            println!("  {id} {:<8} {:<4} lines, first {:?}", v.version, v.notes.len(), v.notes.first().map(|s| s.chars().take(60).collect::<String>()));
        }
        report.push((id.to_string(), list.iter().filter(|v| !placeholder(&v.notes)).count(), list.len()));
        all.extend(list.iter().flat_map(|v| v.notes.iter().cloned()));
    }
    let tfeed = tools::fetch(&[]).await.expect("more apps feed");
    let kr = tools::catalog_versions("krita", &tfeed, &[]).await.unwrap_or_default();
    for v in &kr {
        println!("  krita {:<8} {:<4} lines, first {:?} url {:?}", v.version, v.notes.len(), v.notes.first().map(|s| s.chars().take(60).collect::<String>()), v.notes_url);
    }
    report.push(("Krita".into(), kr.iter().filter(|v| !placeholder(&v.notes)).count(), kr.len()));
    all.extend(kr.iter().flat_map(|v| v.notes.iter().cloned()));
    for a in craft::APPS {
        match craft::versions(a.key, None, false).await {
            Ok(list) => {
                report.push((format!("Craft {}", a.key), list.iter().filter(|v| !placeholder(&v.notes)).count(), list.len()));
                all.extend(list.iter().flat_map(|v| v.notes.iter().cloned()));
            }
            Err(e) => println!("  craft {}: unavailable ({e})", a.key),
        }
    }
    println!("--- versions with real notes, of listed");
    for (name, with, of) in &report {
        println!("{name:<24} {with:>3} of {of}");
    }
    let dashed: Vec<&String> = all.iter().filter(|l| l.contains(['\u{2013}', '\u{2014}'])).collect();
    println!("{} lines in all, {} with a long dash", all.len(), dashed.len());
    assert!(dashed.is_empty(), "long dashes: {:?}", dashed.iter().take(5).collect::<Vec<_>>());
    let got = |name: &str| report.iter().find(|r| r.0 == name).map_or(0, |r| r.1);
    // Clear gains over before (Photoshop 0 of 25, Blender 5.2 1 of 3, older Blender 0 of 18, Krita 3 of 25).
    assert!(got("Adobe PHSP") >= 5 && got("Adobe SHPR") >= 10 && got("Adobe SBSTA") >= 20, "Adobe");
    assert!(got("blender:5.2") >= 3 && got("blender:older") >= 18, "Blender");
    assert!(got("Krita") >= 15, "Krita");
}

/// The starter list of Other apps: how much of it survives the filter, how long a cold read takes
/// and how many real icons are found (nothing is written to the icon cache).
/// cargo test --release --test live_probe other_apps_starter_list -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn other_apps_starter_list_and_icons() {
    use futures_util::StreamExt;
    use updater_lib::sources::{icons, other, winget};
    let t0 = std::time::Instant::now();
    let feed = other::fetch(&[], None).await.expect("winget catalog");
    println!("starter list read in {:.1} s (unfinished: {})", t0.elapsed().as_secs_f64(), feed.unfinished);
    let idx = winget::Index::open(&winget::index_path()).unwrap();
    let kept: std::collections::HashSet<String> = feed.popular.iter().map(|(p, _)| p.pkg.id.to_ascii_lowercase()).collect();
    let mut dropped = Vec::new();
    for (id, _) in other::POPULAR {
        if kept.contains(&id.to_ascii_lowercase()) || other::handled_package(id) || other::excluded_package(id).is_some() {
            continue;
        }
        let why = match idx.by_id(id) {
            None => "not in the catalog".to_string(),
            Some(p) if idx.is_msix(p.row) => "Store package".to_string(),
            Some(p) => match winget::versions(&p).await {
                Err(e) => format!("versions: {e}"),
                Ok(v) => match winget::manifest(&v[0]).await {
                    Err(e) => format!("manifest: {e}"),
                    Ok(m) => other::choose(&m, &other::Want::fresh()).err().unwrap_or_default(),
                },
            },
        };
        dropped.push(format!("{id}: {why}"));
    }
    println!("starter list: {} ids, {} kept, {} dropped", other::POPULAR.len(), feed.popular.len(), dropped.len());
    for d in &dropped {
        println!("  DROPPED {d}");
    }
    let ids: Vec<String> = feed.popular.iter().map(|(p, _)| p.pkg.id.clone()).collect();
    let t1 = std::time::Instant::now();
    let feed_ref = &feed;
    let results: Vec<(String, Option<(usize, &'static str, &'static str)>)> = futures_util::stream::iter(ids)
        .map(|id| async move {
            let src = other::icon_source_of(Some(feed_ref), &id).await;
            let got = match src {
                Some(s) => icons::find(&id, s.icon.as_deref(), &s.pages).await.ok().map(|g| (g.bytes.len(), g.ext, g.via)),
                None => None,
            };
            (id, got)
        })
        .buffer_unordered(6)
        .collect()
        .await;
    let hits = results.iter().filter(|(_, g)| g.is_some()).count();
    println!("icons: {hits} of {} found in {:.1} s", results.len(), t1.elapsed().as_secs_f64());
    let mut by_ext = std::collections::BTreeMap::<String, usize>::new();
    for (id, g) in &results {
        if let Some((n, e, via)) = g {
            *by_ext.entry(format!("{via}/{e}")).or_default() += 1;
            if *via == "favicon" || *via == "page" {
                println!("  WEAK {id} {via} {e} {n} bytes");
            }
        }
    }
    println!("by source/type: {by_ext:?}");
    let mut misses: Vec<&String> = results.iter().filter(|(_, g)| g.is_none()).map(|(i, _)| i).collect();
    misses.sort();
    for m in misses {
        println!("  NO ICON {m}");
    }
}

/// Microsoft Store copies on this PC (read only: nothing is changed). Lists every Store package
/// for this user, then what Blender and More apps make of them.
#[test]
#[ignore]
fn store_copies_on_this_pc() {
    use updater_lib::sources::msstore;
    let pkgs = msstore::packages();
    println!("--- {} Store packages for this user", pkgs.len());
    for p in &pkgs {
        println!("{:<60} {:<16} {:?}", p.family, p.version, p.install_dir.as_ref().map(|d| d.display().to_string()));
    }
    println!("--- listed as Store copies");
    let found: Vec<_> = blender::store_copies(&pkgs).into_iter().chain(tools::store_copies(&pkgs)).collect();
    for i in &found {
        println!("{:<28} {:<34} {:<10} read_only={} exe={:?} store={:?}", i.id, i.name, i.version, i.read_only, i.exe.as_ref().map(|e| e.display().to_string()), i.store);
        assert!(i.read_only && i.uninstall.is_none() && i.store.is_some());
    }
    if found.is_empty() {
        println!("(none: no Blender or Krita from the Microsoft Store here)");
    }
}

/// Other apps in Get apps: how quick the whole catalog list, its pages, orders and searches are,
/// what the searches find first, and how much of a random sample gets an icon, a description,
/// a version list and change notes (nothing is written to the icon cache).
/// cargo test --release --test live_probe other_apps_catalog_browse -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn other_apps_catalog_browse_search_and_sample() {
    use futures_util::StreamExt;
    use std::collections::HashSet;
    use std::time::Instant;
    use updater_lib::sources::{browse, icons, other, winget};
    winget::refresh_index(std::time::Duration::from_secs(6 * 3600)).await.expect("winget index");
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let t = Instant::now();
    let list = browse::list().unwrap();
    println!("list of {} entries ({} packages) built in {:.0} ms", list.items.len(), list.packages, ms(t));
    let t = Instant::now();
    let _ = browse::list().unwrap();
    println!("list again (kept): {:.2} ms", ms(t));
    let here = HashSet::new();
    let req = |query: Option<&str>, sort: &str, offset: usize| browse::Request { query: query.map(String::from), category: None, letter: None, sort: Some(sort.into()), offset, limit: 60 };
    let timed = |r: browse::Request| {
        let t = Instant::now();
        let (p, _) = browse::page_for(&list, &r, &here, false);
        (p, ms(t))
    };
    let (p, a) = timed(req(None, "popular", 0));
    println!("first page (popular): {:.1} ms, {} of {}; first: {:?}", a, p.entries.len(), p.total, p.entries.iter().take(5).map(|e| e.name.as_str()).collect::<Vec<_>>());
    assert!(p.total > 10_000, "the whole catalog is listed");
    let (_, b) = timed(req(None, "popular", 60));
    let (_, c) = timed(req(None, "popular", 6000));
    println!("next page: {b:.1} ms, page 101: {c:.1} ms");
    for s in ["az", "za", "updated"] {
        let (p, x) = timed(req(None, s, 0));
        println!("sort {s}: {x:.1} ms, dated {} of {}, first: {:?}", p.dated, p.total, p.entries.iter().take(3).map(|e| e.name.as_str()).collect::<Vec<_>>());
    }
    let (p, x) = timed(browse::Request { category: Some("Browsers".into()), ..req(None, "popular", 0) });
    println!("category Browsers: {x:.1} ms, {} entries", p.total);
    println!("\n--- searches (top 5)");
    let mut worst: f64 = 0.0;
    for q in ["chrome", "vlc", "7zip", "obs", "blender", "vscode", "visual studio code", "discord", "steam", "python", "notepad", "spotify", "zoom"] {
        let (p, x) = timed(req(Some(q), "popular", 0));
        worst = worst.max(x);
        let top: Vec<String> = p.entries.iter().take(5).map(|e| e.id.trim_start_matches("other:").to_string()).collect();
        println!("{q:<20} {x:5.1} ms {:>5} found  {top:?}", p.total);
    }
    println!("slowest search: {worst:.1} ms");
    let first = |q: &str| browse::page_for(&list, &req(Some(q), "popular", 0), &here, false).0.entries.first().map(|e| e.id.clone()).unwrap_or_default();
    assert_eq!(first("chrome"), "other:Google.Chrome");
    assert_eq!(first("vscode"), "other:Microsoft.VisualStudioCode");
    assert_eq!(first("visual studio code"), "other:Microsoft.VisualStudioCode");
    assert_eq!(first("vlc"), "other:VideoLAN.VLC");
    assert_eq!(first("7zip"), "other:7zip.7zip");

    // A random sample (fixed seed) of the whole list.
    let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
    let mut pick = HashSet::new();
    while pick.len() < 100 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        pick.insert((seed % list.items.len() as u64) as usize);
    }
    let pkgs: Vec<winget::Pkg> = pick.into_iter().map(|i| list.items[i].pkg.clone()).collect();
    let t = Instant::now();
    let rows: Vec<(String, bool, bool, usize, bool, bool, String, bool)> = futures_util::stream::iter(pkgs)
        .map(|p| async move {
            let id = p.id.clone();
            let m = match winget::versions(&p).await {
                Ok(v) => match v.first() {
                    Some(r) => winget::manifest(r).await.ok(),
                    None => None,
                },
                Err(_) => None,
            };
            let desc = m.as_ref().is_some_and(|m| m.description.as_deref().is_some_and(|d| !d.trim().is_empty()) || m.long_description.is_some());
            let icon = match other::icon_source_of(None, &id).await {
                Some(s) => icons::find(&id, s.icon.as_deref(), &s.pages).await.is_ok(),
                None => false,
            };
            let vers = other::catalog_versions(&format!("other:{id}"), None, &[]).await.unwrap_or_default();
            let notes = vers.first().is_some_and(|v| !v.notes.is_empty() || v.notes_url.is_some());
            let newest = vers.first().is_some_and(|v| !v.notes.is_empty());
            let blocked = m.as_ref().map(|m| other::choose(m, &other::Want::fresh()).err().unwrap_or_default()).unwrap_or_else(|| "no manifest".into());
            let dated = m.as_ref().is_some_and(|m| m.release_date.is_some());
            (id, icon, desc, vers.len(), notes, newest, blocked, dated)
        })
        .buffer_unordered(6)
        .collect()
        .await;
    let n = |f: &dyn Fn(&(String, bool, bool, usize, bool, bool, String, bool)) -> bool| rows.iter().filter(|r| f(r)).count();
    println!("\n--- sample of {} in {:.0} s", rows.len(), t.elapsed().as_secs_f64());
    println!("icon {}, description {}, version list {}, newest version with note lines {}, with lines or a notes link {}, installable {}, release date {}", n(&|r| r.1), n(&|r| r.2), n(&|r| r.3 > 0), n(&|r| r.5), n(&|r| r.4), n(&|r| r.6.is_empty()), n(&|r| r.7));
    for r in &rows {
        if !r.1 || !r.2 || r.3 == 0 || !r.4 {
            println!("  GAP {:<48} icon {} desc {} versions {} notes {} {}", r.0, r.1, r.2, r.3, r.4, r.6);
        }
    }
}

/// Install data for the settings audit in Windows Sandbox: for each package, the newest silent
/// installer with its checksum and the install steps, as JSON lines in U3D_OUT. The packages are
/// the popular list, or the ids in the file U3D_IDS (one per line).
/// cargo test --release --test live_probe sandbox_install_data -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn sandbox_install_data() {
    use futures_util::StreamExt;
    use updater_lib::sources::{other, winget, Step};
    let out = std::env::var("U3D_OUT").expect("U3D_OUT");
    let ids: Vec<String> = match std::env::var("U3D_IDS") {
        Ok(f) => std::fs::read_to_string(f).unwrap().lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect(),
        Err(_) => other::POPULAR.iter().map(|(id, _)| id.to_string()).collect(),
    };
    winget::refresh_index(std::time::Duration::from_secs(24 * 3600)).await.expect("index");
    let got: Vec<(String, Option<other::PkgFeed>)> = futures_util::stream::iter(ids)
        .map(|id| async move {
            let pf = other::read_package(&id).await;
            (id, pf)
        })
        .buffer_unordered(8)
        .collect()
        .await;
    let mut lines = Vec::new();
    for (id, pf) in got {
        let mut row = serde_json::json!({ "id": id });
        let skip = other::excluded_package(&id).map(str::to_string).or_else(|| other::handled_package(&id).then(|| "handled by its own source".to_string()));
        match (skip, pf.as_ref().and_then(|p| p.latest.as_ref().map(|m| (p, m)))) {
            (Some(why), _) => row["skip"] = why.into(),
            (None, None) => row["skip"] = "not in the catalog".into(),
            (None, Some((pf, m))) => match other::choose(m, &other::Want::fresh()) {
                Err(e) => row["skip"] = e.into(),
                Ok(i) => match other::build_plan(&pf.pkg, m, i, updater_lib::model::JobKind::Install, None, None) {
                    Err(e) => row["skip"] = format!("{e:#}").into(),
                    Ok(plan) => {
                        let d = &plan.downloads[0];
                        let dest = d.dest.to_string_lossy().to_string();
                        let base = d.dest.parent().unwrap().to_string_lossy().to_string();
                        let fix = |s: &str| s.replace(&dest, "{FILE}").replace(&base, "{BASE}").replace(&updater_lib::paths::logs().to_string_lossy().to_string(), "{LOGS}");
                        let steps: Vec<serde_json::Value> = plan
                            .steps
                            .iter()
                            .map(|s| match s {
                                Step::Run { exe, args, ok_codes, .. } => serde_json::json!({ "t": "run", "exe": fix(&exe.to_string_lossy()), "args": args.iter().map(|a| fix(a)).collect::<Vec<_>>(), "ok": ok_codes }),
                                Step::ExtractZip { zip, dest, .. } => serde_json::json!({ "t": "unzip", "zip": fix(&zip.to_string_lossy()), "dest": fix(&dest.to_string_lossy()) }),
                                other => serde_json::json!({ "t": "other", "label": other.label() }),
                            })
                            .collect();
                        row["name"] = m.name.clone().into();
                        row["publisher"] = m.publisher.clone().into();
                        row["version"] = m.version.clone().into();
                        row["kind"] = i.kind.clone().into();
                        row["scope"] = i.scope.clone().into();
                        row["url"] = d.url.clone().into();
                        row["sha256"] = d.sha256.clone().unwrap_or_default().into();
                        row["file"] = d.dest.file_name().unwrap().to_string_lossy().to_string().into();
                        row["steps"] = steps.into();
                    }
                },
            },
        }
        lines.push(row.to_string());
    }
    std::fs::write(out, lines.join("\n")).unwrap();
}
