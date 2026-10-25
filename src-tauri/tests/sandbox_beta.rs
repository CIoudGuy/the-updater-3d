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
        println!("  downloaded and checked");
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

/// Settings files as a ShareX version writes them (it cannot be started without a desktop here).
fn write_settings(docs: &std::path::Path, version: &str) {
    let d = docs.join("ShareX");
    std::fs::create_dir_all(d.join("Screenshots")).unwrap();
    std::fs::write(d.join("ApplicationConfig.json"), format!("{{\"ApplicationVersion\":\"{version}\",\"FirstTimeRunWindow\":false}}")).unwrap();
    std::fs::write(d.join("HotkeysConfig.json"), format!("{{\"ApplicationVersion\":\"{version}\",\"Hotkeys\":[]}}")).unwrap();
    std::fs::write(d.join("Screenshots").join(format!("shot-{version}.png")), "png").unwrap();
}

#[tokio::test]
#[ignore]
async fn sharex_settings_survive_the_pre_release_and_come_back_after_the_rollback() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");
    let docs = std::path::PathBuf::from(std::env::var("USERPROFILE").unwrap()).join("Documents");
    let config = docs.join("ShareX").join("ApplicationConfig.json");

    // ShareX 21 from the catalog, with its settings.
    run(&other::plan_install(ID, None, JobKind::Install, None, None).await.unwrap()).await;
    let here = sharex().unwrap();
    write_settings(&docs, &here.version);
    let v21 = std::fs::read(&config).unwrap();

    // Saved before the update, as a job does.
    let saved = updater_lib::snapshots::save_before(&here, &[], "22.0.0").unwrap().expect("settings saved");
    println!("saved {} places, {} bytes: {:?}", saved.places.len(), saved.bytes, saved.places.iter().map(|p| p.orig.clone()).collect::<Vec<_>>());
    assert!(saved.places.iter().any(|p| p.orig == docs.join("ShareX")));

    // ShareX 22's installer leaves the settings alone; then 22 rewrites them.
    let base = other::read_package("ShareX.ShareX").await.and_then(|p| p.latest).unwrap();
    let pres = other::pre_releases(&base, &other::Want::for_installed(&here)).await;
    let (offer, _) = other::pre_offer(&pres, &here.version).expect("ShareX has a newer pre-release on GitHub");
    run(&other::plan_install(ID, Some(&offer.version), JobKind::Update, Some(&here), None).await.unwrap()).await;
    let beta = sharex().unwrap();
    assert_eq!(std::fs::read(&config).unwrap(), v21, "the pre-release's installer left the settings as they were");
    write_settings(&docs, &beta.version);

    // Back to 21: the installer leaves 22's settings; the ones 21 had are offered and come back,
    // and the screenshots taken meanwhile stay.
    run(&other::plan_install(ID, Some(&here.version), JobKind::Rollback, Some(&beta), None).await.unwrap()).await;
    assert!(std::fs::read_to_string(&config).unwrap().contains(&beta.version));
    let root = updater_lib::snapshots::root();
    let s = updater_lib::snapshots::for_version(&root, ID, &here.version).expect("the settings 21 had are offered");
    let n = updater_lib::snapshots::put_back(&root, &s, updater_lib::snapshots::import_reg).unwrap();
    println!("put back {n} files");
    assert_eq!(std::fs::read(&config).unwrap(), v21, "21's settings are back as they were");
    assert!(docs.join("ShareX").join("Screenshots").join(format!("shot-{}.png", beta.version)).exists());

    // Uninstalling leaves the settings folder (ShareX keeps it in Documents).
    let back = sharex().unwrap();
    run(&other::plan_uninstall(&back).unwrap()).await;
    assert!(config.exists(), "uninstalling keeps the settings");
    println!("SETTINGS CHECK PASSED");
}

/// Which of ShareX's Windows links are there: the right-click menu, its file types, the browser
/// extension link, Send To and startup.
fn sharex_links() -> Vec<(&'static str, bool)> {
    let key = |k: &str| std::process::Command::new("reg").args(["query", k]).output().is_ok_and(|o| o.status.success());
    let appdata = std::path::PathBuf::from(std::env::var("APPDATA").unwrap());
    vec![
        ("right-click menu", key(r"HKCU\Software\Classes\*\shell\ShareX")),
        ("folder menu", key(r"HKCU\Software\Classes\Directory\shell\ShareX")),
        (".sxcu files", key(r"HKCU\Software\Classes\.sxcu")),
        (".sxie files", key(r"HKCU\Software\Classes\.sxie")),
        ("Chrome extension link", key(r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.getsharex.sharex")),
        ("Firefox extension link", key(r"HKCU\Software\Mozilla\NativeMessagingHosts\ShareX")),
        ("Send To", appdata.join(r"Microsoft\Windows\SendTo\ShareX.lnk").exists()),
        ("startup", appdata.join(r"Microsoft\Windows\Start Menu\Programs\Startup\ShareX.lnk").exists()),
    ]
}

#[tokio::test]
#[ignore]
async fn repair_brings_back_what_sharexs_own_cleanup_removes() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");
    run(&other::plan_install(ID, None, JobKind::Install, None, None).await.unwrap()).await;
    let here = sharex().unwrap();
    let after_install = sharex_links();
    println!("after the install: {after_install:?}");
    // What Steam's uninstall of its ShareX runs is ShareX's own cleanup ("-uninstall"), which
    // removes these by name for every copy (IntegrationHelpers.Uninstall in ShareX 21). It cannot
    // run without a desktop here, so the same things are removed directly.
    for k in [r"HKCU\Software\Classes\*\shell\ShareX", r"HKCU\Software\Classes\Directory\shell\ShareX", r"HKCU\Software\Classes\.sxcu", r"HKCU\Software\Classes\ShareX.sxcu", r"HKCU\Software\Classes\.sxie", r"HKCU\Software\Classes\ShareX.sxie", r"HKCU\Software\Google\Chrome\NativeMessagingHosts\com.getsharex.sharex", r"HKCU\Software\Mozilla\NativeMessagingHosts\ShareX"] {
        let _ = std::process::Command::new("reg").args(["delete", k, "/f"]).output();
    }
    let appdata = std::path::PathBuf::from(std::env::var("APPDATA").unwrap());
    let _ = std::fs::remove_file(appdata.join(r"Microsoft\Windows\SendTo\ShareX.lnk"));
    let _ = std::fs::remove_file(appdata.join(r"Microsoft\Windows\Start Menu\Programs\Startup\ShareX.lnk"));
    let after_cleanup = sharex_links();
    println!("after ShareX's cleanup: {after_cleanup:?}");
    assert!(after_cleanup.iter().all(|(_, there)| !there));
    // Repair: the same version installed again.
    run(&other::plan_install(ID, None, JobKind::Repair, Some(&here), None).await.unwrap()).await;
    let after_repair = sharex_links();
    println!("after Repair: {after_repair:?}");
    for ((name, installed), (_, repaired)) in after_install.iter().zip(&after_repair) {
        assert!(!installed || *repaired, "{name} did not come back with Repair");
    }
    run(&other::plan_uninstall(&sharex().unwrap()).unwrap()).await;
    println!("REPAIR CHECK PASSED");
}

/// WinRAR's beta from its own site (rarlab.com, no checksum, signed): offered with its change
/// list, installed as an update checked by its signature, read as a pre-release of 7.30 (its
/// Windows entry says "7.30.1"), then rolled back to the catalog's version.
#[tokio::test]
#[ignore]
async fn winrars_site_beta_installs_reads_as_a_beta_and_rolls_back() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    const W: &str = "other:RARLab.WinRAR";
    let find = || other::scan(&winutil::uninstall_entries(), &[], None).into_iter().find(|i| i.id == W);
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");
    run(&other::plan_install(W, None, JobKind::Install, None, None).await.unwrap()).await;
    let here = find().expect("WinRAR installed");
    println!("installed {}", here.version);

    let base = other::read_package("RARLab.WinRAR").await.and_then(|p| p.latest).unwrap();
    let pres = other::pre_releases(&base, &other::Want::for_installed(&here)).await;
    let (offer, copy) = other::pre_offer(&pres, &here.version).expect("a WinRAR beta on rarlab.com");
    let notes = updater_lib::notes::text_lines(&pres[0].0.body);
    println!("offered {} from {}: {} note lines, first {:?}", offer.version, pres[0].1.url, notes.len(), notes.first());
    assert!(!copy && notes.len() >= 3);
    let plan = other::plan_install(W, Some(&offer.version), JobKind::Update, Some(&here), None).await.unwrap();
    assert_eq!((plan.downloads[0].sha256.as_deref(), plan.downloads[0].signer.as_deref()), (None, Some("win.rar GmbH")));
    run(&plan).await;
    let beta = find().expect("WinRAR found after the beta");
    println!("now {} ({})", beta.version, beta.name);
    assert_eq!(beta.version, offer.version, "the beta reads as itself");
    // It follows betas now; the catalog's release is no update for it, the final 7.30 will be.
    assert!(other::pre_offer(&pres, &beta.version).is_some_and(|(_, c)| c));
    assert!(!update_offered(&base.version, &beta.version, false));
    let series: String = beta.version.split('-').next().unwrap().to_string();
    assert!(update_offered(&format!("{series}.0"), &beta.version, false));

    run(&other::plan_install(W, Some(&here.version), JobKind::Rollback, Some(&beta), None).await.unwrap()).await;
    let back = find().unwrap();
    println!("rolled back to {}", back.version);
    assert_eq!(back.version, here.version);
    // Not uninstalled here: WinRAR's uninstaller opens its own window, which needs a desktop.
    println!("WINRAR CHECK PASSED");
}

/// `run`, reporting a failed download or step instead of stopping the test.
async fn try_run(plan: &Plan) -> Result<(), String> {
    for d in &plan.downloads {
        let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
        download::fetch(d, Arc::new(Progress::new()), Arc::new(Throttle::new()), rx).await.map_err(|e| format!("download: {e:#}"))?;
    }
    for s in &plan.steps {
        let s = s.clone();
        let r = tokio::task::spawn_blocking(move || elevate::execute(&s)).await.unwrap();
        if !r.ok {
            return Err(format!("step: code {:?} {}", r.code, r.message));
        }
    }
    Ok(())
}

/// A beta from a publisher's own site, installed over the catalog's version and read back as
/// the offered version, then a rollback tried (its result printed).
async fn site_beta_cycle(id: &str, pkg: &str) {
    let find = || other::scan(&winutil::uninstall_entries(), &[], None).into_iter().find(|i| i.id == id);
    run(&other::plan_install(id, None, JobKind::Install, None, None).await.unwrap()).await;
    let here = find().unwrap_or_else(|| panic!("{pkg} installed"));
    let base = other::read_package(pkg).await.and_then(|p| p.latest).unwrap();
    println!("{pkg}: installed {} (catalog {})", here.version, base.version);
    let pres = other::pre_releases(&base, &other::Want::for_installed(&here)).await;
    let (offer, copy) = other::pre_offer(&pres, &here.version).unwrap_or_else(|| panic!("{pkg}: a beta is offered"));
    println!("  offered {} ({:?} bytes) from {}, sha {}, signer {:?}", offer.version, offer.size, pres[0].1.url, !pres[0].1.sha256.is_empty(), pres[0].1.signer);
    assert!(!copy);
    run(&other::plan_install(id, Some(&offer.version), JobKind::Update, Some(&here), None).await.unwrap()).await;
    let beta = find().unwrap();
    println!("  now {} ({})", beta.version, beta.name);
    assert_eq!(beta.version, offer.version, "{pkg}: the beta reads as offered");
    let back = try_run(&other::plan_install(id, Some(&here.version), JobKind::Rollback, Some(&beta), None).await.unwrap()).await;
    println!("  rollback: {back:?}, now {:?}", find().map(|i| i.version));
}

/// Tailscale's unstable track, checked by its SHA-256 files.
#[tokio::test]
#[ignore]
async fn site_beta_tailscale() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");
    site_beta_cycle("other:Tailscale.Tailscale", "Tailscale.Tailscale").await;
    println!("TAILSCALE CHECK PASSED");
}

/// Cloudflare WARP's beta track, checked by Cloudflare's signature.
#[tokio::test]
#[ignore]
async fn site_beta_warp() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");
    site_beta_cycle("other:Cloudflare.Warp", "Cloudflare.Warp").await;
    println!("WARP CHECK PASSED");
}
