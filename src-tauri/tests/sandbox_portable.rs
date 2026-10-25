//! Browsers from start to end on a clean Windows: a portable Thorium found by its Start menu
//! shortcut, updated in place from its portable zip, rolled back and uninstalled with its profile
//! kept; Thorium installed by its installer, told apart from its other builds; and Chrome, whose
//! installer has no silent switch in the catalog, updated by it.
//!
//! It installs real software, so it runs only inside Windows Sandbox:
//! cargo test --test sandbox_portable -- --ignored --nocapture

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use updater_lib::download::{self, Control, Outcome, Progress, Throttle};
use updater_lib::elevate;
use updater_lib::model::JobKind;
use updater_lib::sources::{other, portable, winget, Installed, Plan, Step};
use updater_lib::winutil;

const THORIUM: &str = "other:Alex313031.Thorium.AVX2";
const CHROME: &str = "other:Google.Chrome.EXE";

fn in_windows_sandbox() -> bool {
    Path::new(r"C:\Users\WDAGUtilityAccount").is_dir()
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
        println!("  step {}: ok {} code {:?} {} {}", s.label(), r.ok, r.code, r.message, r.output);
        assert!(r.ok, "step failed: {} {}", r.message, r.output);
    }
    tokio::time::sleep(Duration::from_secs(3)).await;
}

/// The app with this id as the app finds it now.
fn find(id: &str) -> Option<Installed> {
    other::scan(&winutil::uninstall_entries(), &[], None).into_iter().find(|i| i.id == id)
}

fn start_menu() -> PathBuf {
    PathBuf::from(std::env::var("APPDATA").unwrap()).join("Microsoft\\Windows\\Start Menu\\Programs")
}

#[tokio::test]
#[ignore]
async fn a_portable_browser_is_found_updated_rolled_back_and_uninstalled() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");
    let pf = other::read_package("Alex313031.Thorium.AVX2").await.expect("Thorium AVX2 in the catalog");
    let newest = pf.versions[0].version.clone();
    // The newest older version whose release has the portable zip.
    let mut older = None;
    for r in pf.versions.iter().skip(1).take(4) {
        let mut m = winget::manifest(r).await.expect("manifest");
        other::add_portable_zip(&mut m).await;
        if let Some(z) = m.installers.iter().find(|i| other::kind_of(i) == "portable") {
            older = Some((m.version.clone(), z.clone()));
            break;
        }
    }
    let (old_ver, zip) = older.expect("an older Thorium AVX2 with its portable zip");
    println!("newest {newest}, older {old_ver}: {}", zip.url);

    // 1. Unpacked by hand, as a user does, with a Start menu shortcut and a profile.
    let root = PathBuf::from(std::env::var("USERPROFILE").unwrap()).join("Browsers\\Thorium");
    let dl = std::env::temp_dir().join("thorium-old.zip");
    let d = updater_lib::sources::Download { url: zip.url.clone(), dest: dl.clone(), size: None, sha256: Some(zip.sha256.clone()), md5: None, signer: None, sha512: None, headers: vec![] };
    let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
    assert!(matches!(download::fetch(&d, Arc::new(Progress::new()), Arc::new(Throttle::new()), rx).await.expect("download"), Outcome::Done));
    assert!(elevate::execute(&Step::ExtractZip { zip: dl.clone(), dest: root.clone(), strip_root: false, elevated: false, label: String::new() }).ok, "unpacked");
    let exe = root.join("BIN\\thorium.exe");
    assert!(exe.is_file(), "the zip has BIN\\thorium.exe");
    std::fs::create_dir_all(root.join("USER_DATA\\Default")).unwrap();
    std::fs::write(root.join("USER_DATA\\Default\\Preferences"), "mine").unwrap();
    let lnk = start_menu().join("Thorium.lnk");
    assert!(elevate::execute(&Step::Shortcut { lnk: lnk.clone(), target: exe.clone(), label: String::new() }).ok);
    let _ = std::fs::remove_file(&dl);

    // 2. Found as Thorium's AVX2 build.
    let here = find(THORIUM).expect("the portable Thorium is listed");
    println!("found {} {} in {:?}, {:?}", here.name, here.version, here.install_dir, here.tagline);
    assert!(portable::is_portable(&here));
    assert_eq!(portable::version_now(&exe).as_deref(), Some(here.version.as_str()));
    let old_folders: Vec<String> = std::fs::read_dir(root.join("BIN")).unwrap().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    println!("program folders before: {old_folders:?}");

    // 3. The update the app offers it, from the portable zip, in place.
    let feed = other::fetch(std::slice::from_ref(&here), None).await.expect("feed");
    let offer = other::latest(&here, &feed).expect("an offer");
    println!("offered {}", offer.version);
    assert!(other::is_newer(&offer.version, &here.version));
    let plan = other::plan_install(THORIUM, None, JobKind::Update, Some(&here), Some(&feed)).await.expect("update plan");
    assert!(matches!(plan.steps.as_slice(), [Step::ReplacePortable { .. }]), "{:?}", plan.steps);
    run(&plan).await;
    let now = portable::version_now(&exe).expect("program after the update");
    println!("now {now}");
    assert_eq!(now, plan.to_version);
    assert_eq!(std::fs::read_to_string(root.join("USER_DATA\\Default\\Preferences")).unwrap(), "mine");
    let folders: Vec<String> = std::fs::read_dir(root.join("BIN")).unwrap().flatten().filter(|e| e.path().is_dir()).map(|e| e.file_name().to_string_lossy().to_string()).collect();
    println!("program folders after: {folders:?}");
    assert!(old_folders.iter().filter(|f| f.chars().next().is_some_and(|c| c.is_ascii_digit())).all(|f| !folders.contains(f)), "the old version's folder is gone");
    let updated = find(THORIUM).expect("still listed");
    assert_eq!(updated.version, now);

    // 4. Back to the older version.
    let plan = other::plan_install(THORIUM, Some(&old_ver), JobKind::Rollback, Some(&updated), Some(&feed)).await.expect("rollback plan");
    assert_eq!(plan.kind, JobKind::Rollback);
    run(&plan).await;
    assert_eq!(find(THORIUM).map(|i| i.version), Some(old_ver.clone()));
    assert_eq!(std::fs::read_to_string(root.join("USER_DATA\\Default\\Preferences")).unwrap(), "mine");

    // 5. Uninstalled: the program and its shortcut go, the profile stays for the leftovers step.
    let here = find(THORIUM).unwrap();
    let plan = other::plan_uninstall(&here).expect("uninstall plan");
    run(&plan).await;
    assert!(find(THORIUM).is_none(), "gone after the uninstall");
    assert!(!root.join("BIN").exists() && !lnk.exists());
    assert_eq!(std::fs::read_to_string(root.join("USER_DATA\\Default\\Preferences")).unwrap(), "mine");
    let _ = std::fs::remove_dir_all(&root);
    println!("portable Thorium: done");
}

#[tokio::test]
#[ignore]
async fn an_installed_thorium_build_and_chrome_are_updated_by_their_installers() {
    if !in_windows_sandbox() {
        println!("not in Windows Sandbox: nothing installed");
        return;
    }
    winget::refresh_index(Duration::from_secs(3600)).await.expect("catalog index");

    // Thorium's AVX2 build from its installer: listed as that build, not as one of the others.
    let plan = other::plan_install(THORIUM, None, JobKind::Install, None, None).await.expect("Thorium install plan");
    run(&plan).await;
    let entries = winutil::uninstall_entries();
    for e in entries.iter().filter(|e| e.display_name.contains("Thorium")) {
        println!("entry {} [{}] {} by {} at {} icon {}", e.display_name, e.hive, e.display_version, e.publisher, e.install_location, e.display_icon);
    }
    let thorium = find(THORIUM);
    println!("Thorium installed: {:?}", thorium.as_ref().map(|i| (&i.name, &i.version, &i.install_dir)));
    let here = thorium.expect("the installed Thorium is listed as its AVX2 build");
    assert!(!portable::is_portable(&here));
    let plan = other::plan_uninstall(&here).expect("uninstall plan");
    run(&plan).await;
    assert!(find(THORIUM).is_none());

    // Chrome, a version before the newest, then updated by its installer.
    let pf = other::read_package("Google.Chrome.EXE").await.expect("Chrome in the catalog");
    let older = pf.versions[1].version.clone();
    let plan = other::plan_install(CHROME, Some(&older), JobKind::Install, None, None).await.expect("Chrome install plan");
    run(&plan).await;
    let here = find(CHROME).expect("Chrome is listed as a catalog app");
    println!("Chrome {} ({})", here.version, here.name);
    let feed = other::fetch(std::slice::from_ref(&here), None).await.expect("feed");
    let offer = other::latest(&here, &feed).expect("an offer");
    println!("Chrome offered {}", offer.version);
    assert!(other::is_newer(&offer.version, &here.version));
    let plan = other::plan_install(CHROME, None, JobKind::Update, Some(&here), Some(&feed)).await.expect("Chrome update plan");
    run(&plan).await;
    let now = find(CHROME).expect("Chrome after the update");
    println!("Chrome now {}", now.version);
    assert_eq!(now.version, offer.version);
    println!("installed Thorium and Chrome: done");
}
