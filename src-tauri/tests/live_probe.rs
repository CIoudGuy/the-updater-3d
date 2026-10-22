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
}
