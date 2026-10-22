//! Live check against this PC and the real update servers. Not run by default:
//! cargo test --test live_probe -- --ignored --nocapture

use updater_lib::sources::{adobe, blender, craft};
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
    for i in &all {
        let latest = match i.source {
            updater_lib::model::Source::Adobe => adobe::latest(i, &all, &cat),
            updater_lib::model::Source::Craft => craft::latest(i, &feed),
            updater_lib::model::Source::Blender => blender::latest(i, &all, &bfeed),
        };
        let l = latest.as_ref().map(|l| l.version.clone()).unwrap_or_else(|| "?".into());
        let mark = if latest.as_ref().is_some_and(|l| version::newer(&l.version, &i.version)) { "UPDATE" } else { "" };
        println!(
            "{:<22} {:<28} {:<12} -> {:<12} {:6} missing={} exe={:?} note={:?}",
            i.id, i.name, i.version, l, mark, i.missing, i.exe.as_ref().map(|e| e.display().to_string()), latest.and_then(|l| l.note)
        );
    }
    println!("--- catalog");
    for c in adobe::catalog_entries(&cat, &all).iter().chain(craft::catalog_entries(&feed, &all).iter()).chain(blender::catalog_entries(&bfeed, &all, true).iter()) {
        println!("{:<24} {:<34} {:<10} pre={} size={:?}", c.id, c.name, c.version, c.prerelease, c.size);
    }
    assert!(!all.is_empty());
}

#[tokio::test]
#[ignore]
async fn builds_an_adobe_update_plan_without_installing() {
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
    let total: u64 = plan.downloads.iter().filter_map(|d| d.size).sum();
    println!("{} -> {}: {} files, {:.2} GB", des.version, plan.to_version, plan.downloads.len(), total as f64 / 1e9);
    for d in &plan.downloads {
        println!("  {} {:?}", d.url, d.size);
    }
    for s in &plan.steps {
        println!("  step {:?}", s);
    }
    let root = plan.cleanup[0].clone();
    println!("{}", std::fs::read_to_string(root.join("products").join("driver.xml")).unwrap());
    assert!(plan.downloads.iter().all(|d| d.url.starts_with("https://")));
    assert!(plan.fallback.is_some());
    let _ = std::fs::remove_dir_all(root);
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
