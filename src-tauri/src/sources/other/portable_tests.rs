use super::tests::{entry, inst};
use super::*;
use crate::sources::portable::{self, Program};
use crate::sources::winget::test_index;

/// A folder next to the test program (Temp never counts as an install folder). Removed on drop.
struct Fx(PathBuf);

impl Fx {
    fn new(name: &str) -> Fx {
        let d = std::env::current_exe().unwrap().parent().unwrap().join(format!("other-portable-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Fx(d)
    }
    fn file(&self, rel: &str, body: &str) -> PathBuf {
        let p = self.0.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, body).unwrap();
        p
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The catalog's Thorium packages: three builds of the browser under one name, publisher and
/// product code, and an e-book reader of the same name.
fn thorium_index() -> Index {
    let idx = test_index();
    idx.add_test_package("Alex313031.Thorium", "Thorium", "154.0.8037.45", &["Thorium"], &[], &[], "The Thorium Authors");
    idx.add_test_package("Alex313031.Thorium.AVX2", "Thorium (AVX2)", "156.0.8078.28", &["Thorium"], &[], &["Thorium"], "The Thorium Authors");
    idx.add_test_package("Alex313031.Thorium.SSE4", "Thorium (SSE4)", "156.0.8078.28", &["Thorium"], &[], &["Thorium"], "The Thorium Authors");
    idx.add_test_package("EDRLab.Thorium", "Thorium", "3.5.1", &["13998dff-ad91-57f4-b530-aa7ad6d4fea5"], &[], &[], "EDRLab");
    idx
}

/// A portable Thorium: its program file names Alex313031 as its maker, and `build` (when given)
/// is written next to it as Thorium does.
fn portable_thorium(fx: &Fx, build: Option<&str>) -> winutil::UninstallEntry {
    let exe = fx.file("Browsers\\Thorium\\BIN\\thorium.exe", "MZ");
    if let Some(b) = build {
        fx.file("Browsers\\Thorium\\BIN\\thor_ver", &format!("Win\n{b}\n"));
    }
    portable::entry_for(&fx.0.join("Browsers\\Thorium"), &Program { exe, name: "Thorium".into(), company: "Alex313031".into(), version: "156.0.8078.28".into() })
}

#[test]
fn a_portable_copy_is_matched_to_the_build_it_names() {
    let idx = thorium_index();
    let fx = Fx::new("avx2");
    let ms = portable_matches(&[portable_thorium(&fx, Some("AVX2"))], &idx, &[], &[]);
    assert_eq!(ms.iter().map(|m| (m.pkg.id.as_str(), m.how)).collect::<Vec<_>>(), vec![("Alex313031.Thorium.AVX2", How::Portable)]);
    let fx = Fx::new("sse4");
    let ms = portable_matches(&[portable_thorium(&fx, Some("SSE4"))], &idx, &[], &[]);
    assert_eq!(ms.iter().map(|m| m.pkg.id.as_str()).collect::<Vec<_>>(), vec!["Alex313031.Thorium.SSE4"]);
    // Nothing names the build: no guess between them.
    let fx = Fx::new("none");
    assert!(portable_matches(&[portable_thorium(&fx, None)], &idx, &[], &[]).is_empty());
    // A copy of the package that Windows' list has already: that one is the app.
    let fx = Fx::new("listed");
    let listed = Match { entry: entry("Thorium", "HKCU", "Thorium", "156.0.8078.28", "Alex313031"), pkg: idx.by_id("Alex313031.Thorium.AVX2").unwrap(), how: How::ProductCode, upgrade_code: None, package: None };
    assert!(portable_matches(&[portable_thorium(&fx, Some("AVX2"))], &idx, &[listed], &[]).is_empty());
}

#[test]
fn an_installed_build_is_matched_by_the_build_its_folder_names() {
    // Thorium installed by its installer: the uninstall list entry is the same for every build.
    let idx = thorium_index();
    let fx = Fx::new("installed");
    let exe = fx.file("Thorium\\Application\\thorium.exe", "MZ");
    let mut e = entry("Thorium", "HKCU", "Thorium", "156.0.8078.28", "Alex313031");
    e.install_location = fx.0.join("Thorium\\Application").to_string_lossy().to_string();
    e.display_icon = format!("{},0", exe.display());
    let r = match_all(std::slice::from_ref(&e), &idx, &[], &HashMap::new());
    assert!(matches!(r.misses.first().map(|m| &m.1), Some(Miss::Ambiguous(_))));
    fx.file("Thorium\\Application\\thor_ver", "Win\nAVX2\n");
    let r = match_all(&[e], &idx, &[], &HashMap::new());
    assert_eq!(r.matches.iter().map(|m| m.pkg.id.as_str()).collect::<Vec<_>>(), vec!["Alex313031.Thorium.AVX2"]);
}

fn thorium_manifest(installers: Vec<Installer>) -> Manifest {
    Manifest { id: "Alex313031.Thorium.AVX2".into(), version: "156.0.8078.28".into(), name: "Thorium (AVX2)".into(), installers, ..Default::default() }
}

fn portable_zip() -> Installer {
    Installer { nested_kind: Some("portable".into()), url: "https://github.com/gz83/thorium/releases/download/M156.0.8078.28/Thorium_AVX2_156.0.8078.28.zip".into(), ..inst("x64", "zip", None) }
}

fn mini_installer() -> Installer {
    Installer { silent: Some("--silent".into()), custom: Some("--do-not-launch-chrome".into()), ..inst("x64", "exe", Some("user")) }
}

#[test]
fn a_portable_copy_is_updated_from_a_portable_download_only() {
    let fx = Fx::new("choose");
    let e = portable_thorium(&fx, Some("AVX2"));
    let w = Want::for_entry(&e, None);
    assert!(w.portable);
    let both = thorium_manifest(vec![mini_installer(), portable_zip()]);
    assert_eq!(kind_of(choose(&both, &w).unwrap()), "portable");
    // Anything else would install a second copy next to it.
    assert_eq!(choose(&thorium_manifest(vec![mini_installer()]), &w).unwrap_err(), "no portable download, this copy is portable");
    // A new install never takes the portable download.
    assert_eq!(choose(&both, &Want::fresh()).unwrap().kind, "exe");
}

#[test]
fn a_portable_copy_updates_in_place_and_is_listed_with_its_own_program() {
    let idx = thorium_index();
    let fx = Fx::new("plan");
    let e = portable_thorium(&fx, Some("AVX2"));
    let m = portable_matches(std::slice::from_ref(&e), &idx, &[], &[]).remove(0);
    let here = make_portable(&m, None);
    assert_eq!((here.id.as_str(), here.version.as_str(), here.tagline.as_deref()), ("other:Alex313031.Thorium.AVX2", "156.0.8078.28", Some("Portable copy")));
    assert!(portable::is_portable(&here) && here.uninstall.is_none());
    let older = Installed { version: "155.0.7999.12".into(), ..here.clone() };
    let man = thorium_manifest(vec![mini_installer(), portable_zip()]);
    let zip = choose(&man, &Want::for_installed(&older)).unwrap();
    let plan = build_plan(&m.pkg, &man, zip, JobKind::Update, Some(&older), None).unwrap();
    match &plan.steps[..] {
        [Step::ReplacePortable { program, elevated: false, .. }] => assert_eq!(program, here.exe.as_ref().unwrap()),
        s => panic!("{s:?}"),
    }
    assert_eq!(plan.block_dirs, vec![fx.0.join("Browsers\\Thorium\\BIN")]);
    assert!(matches!(&plan.verify, Verify::Version { version, .. } if version == "156.0.8078.28"));
    assert_eq!(plan.downloads[0].sha256.as_deref(), Some("a".repeat(64).as_str()));
    // A new install from the portable download is refused: it has no copy to replace.
    assert!(build_plan(&m.pkg, &man, zip, JobKind::Install, None, None).is_err());
}

#[test]
fn chromiums_installer_runs_without_a_silent_switch() {
    // Chrome's catalog entry: "InstallModes: silent" and Chromium's own switches only.
    let chrome = Installer { install_modes: vec!["silent".into()], custom: Some("--do-not-launch-chrome --system-level".into()), ..inst("x64", "exe", Some("machine")) };
    assert!(usable(&chrome, "x64").is_ok());
    let (_, args) = silent_args(&chrome, "exe", Path::new("C:\\dl\\chrome_installer.exe"), Path::new("C:\\dl\\log.txt"), true);
    assert_eq!(args, vec!["--do-not-launch-chrome", "--system-level"]);
    // Without them, an exe the catalog only calls silent is still not run.
    let other = Installer { install_modes: vec!["silent".into()], ..inst("x64", "exe", Some("machine")) };
    assert_eq!(usable(&other, "x64").unwrap_err(), "its installer has no silent mode");
    // Chrome installed for all users is updated with its installer for all users.
    let mut here = entry("Google Chrome", "HKLM32", "Google Chrome", "155.0.8059.85", "Google LLC");
    here.install_location = "C:\\Program Files\\Google\\Chrome\\Application".into();
    let user = Installer { custom: Some("--do-not-launch-chrome".into()), ..chrome.clone() };
    let m = Manifest { installers: vec![Installer { scope: Some("user".into()), ..user }, chrome], ..thorium_manifest(vec![]) };
    assert_eq!(choose(&m, &Want::for_entry(&here, None)).unwrap().scope.as_deref(), Some("machine"));
}

#[test]
fn get_apps_counts_an_app_from_another_installer_kind_as_installed() {
    assert!(same_app_ids("Google.Chrome.EXE").contains(&"google.chrome".to_string()));
    assert!(same_app_ids("Google.Chrome").contains(&"google.chrome.exe".to_string()));
    assert!(!same_app_ids("Google.Chrome").contains(&"google.chrome.beta".to_string()));
}

#[test]
fn this_app_in_the_catalog_is_left_to_its_own_updates() {
    assert_eq!(excluded_package("CIoudGuy.TheUpdater3D"), Some("updates itself"));
    assert_eq!(excluded_package("cioudguy.theupdater3d"), Some("updates itself"));
}
