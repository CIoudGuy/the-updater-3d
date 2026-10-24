//! A pre-release from start to end on a clean Windows: ShareX from the winget catalog, its
//! GitHub pre-release as an update, a rollback to the catalog's version and an uninstall, each
//! checked in Windows' list of installed programs the way the app reads it.
//!
//! It installs real software, so it runs only inside Windows Sandbox:
//! cargo test --test sandbox_beta -- --ignored --nocapture

use std::sync::Arc;
use std::time::Duration;
use updater_lib::download::{self, Control, Outcome, Progress, Throttle};
use updater_lib::elevate;
use updater_lib::model::JobKind;
use updater_lib::sources::{other, update_offered, winget, Installed, Plan};
use updater_lib::winutil;

const ID: &str = "other:ShareX.ShareX";

fn in_windows_sandbox() -> bool {
    std::path::Path::new(r"C:\Users\WDAGUtilityAccount").is_dir()
}

/// Download a plan's files and run its steps, as a job does.
async fn run(plan: &Plan) {
    println!("plan: {:?} {} to {}", plan.kind, plan.name, plan.to_version);
    for d in &plan.downloads {
        println!("  download {}", d.url);
        let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
        let out = download::fetch(d, Arc::new(Progress::new()), Arc::new(Throttle::new()), rx).await.expect("download");
        assert!(matches!(out, Outcome::Done), "download did not finish");
    }
    for s in &plan.steps {
        let r = tokio::task::spawn_blocking({
            let s = s.clone();
            move || elevate::execute(&s)
        })
        .await
        .unwrap();
        println!("  step: ok {} code {:?} {}", r.ok, r.code, r.message);
        assert!(r.ok, "step failed: {}", r.output);
    }
    // ShareX may start itself after an install: closed, as the app asks before the next one.
    let _ = std::process::Command::new("taskkill").args(["/F", "/IM", "ShareX.exe"]).output();
    tokio::time::sleep(Duration::from_secs(3)).await;
}

/// ShareX as the app finds it in Windows' list of installed programs.
fn sharex() -> Option<Installed> {
    other::scan(&winutil::uninstall_entries(), &[], None).into_iter().find(|i| i.id == ID)
}

#[tokio::test]
#[ignore]
async fn a_pre_release_installs_updates_rolls_back_and_uninstalls() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");
    assert!(sharex().is_none(), "ShareX is already installed");

    // 1. The catalog's ShareX.
    let plan = other::plan_install(ID, None, JobKind::Install, None, None).await.expect("install plan");
    let stable = plan.to_version.clone();
    run(&plan).await;
    let here = sharex().expect("ShareX found after the install");
    println!("installed {} ({})", here.version, here.name);
    assert!(!update_offered(&stable, &here.version, false));

    // 2. Its newest GitHub pre-release, as an update (not in the catalog).
    let base = other::read_package("ShareX.ShareX").await.and_then(|p| p.latest).expect("catalog manifest");
    let pres = other::pre_releases(&base, &other::Want::for_installed(&here)).await;
    let (offer, copy) = other::pre_offer(&pres, &here.version).expect("ShareX has a newer pre-release on GitHub");
    println!("pre-release offered: {} ({:?} bytes), copy is a pre-release: {copy}", offer.version, offer.size);
    assert!(!copy && offer.size.is_some() && update_offered(&offer.version, &here.version, false));
    let plan = other::plan_install(ID, Some(&offer.version), JobKind::Update, Some(&here), None).await.expect("pre-release plan");
    assert_eq!((plan.kind, plan.to_version.as_str()), (JobKind::Update, offer.version.as_str()));
    run(&plan).await;
    let beta = sharex().expect("ShareX still found after the pre-release");
    println!("now {} ({})", beta.version, beta.name);
    assert_eq!(beta.version, offer.version, "the pre-release is installed and read as itself");
    // The pre-release copy follows pre-releases; the catalog's version is no update for it.
    assert!(other::pre_offer(&pres, &beta.version).is_some_and(|(_, copy)| copy));
    assert!(!update_offered(&stable, &beta.version, false));

    // 3. Back to the catalog's version.
    let plan = other::plan_install(ID, Some(&stable), JobKind::Rollback, Some(&beta), None).await.expect("rollback plan");
    assert_eq!(plan.kind, JobKind::Rollback);
    run(&plan).await;
    let back = sharex().expect("ShareX found after the rollback");
    println!("rolled back to {}", back.version);
    assert_eq!(back.version, stable);

    // 4. Uninstalled.
    run(&other::plan_uninstall(&back).expect("uninstall plan")).await;
    assert!(sharex().is_none(), "ShareX is gone after the uninstall");
    println!("ALL STEPS PASSED");
}
