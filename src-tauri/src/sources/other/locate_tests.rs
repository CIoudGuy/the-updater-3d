//! Finding an Other app's program file, folder and date, from uninstall entries made up here.

use super::*;
use crate::sources::locate::{self, Hints};
use crate::sources::winget::test_index;

/// A folder for made-up app files, next to the test program (not under Temp, which never counts
/// as an install folder). Removed when dropped.
struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Fixture {
        let exe = std::env::current_exe().unwrap();
        let d = exe.parent().unwrap().join(format!("locate-fixture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Fixture(d)
    }

    /// "<fixture>/<dir>" holding the given files.
    fn app(&self, dir: &str, files: &[&str]) -> PathBuf {
        let d = self.0.join(dir);
        std::fs::create_dir_all(&d).unwrap();
        for f in files {
            std::fs::write(d.join(f), b"MZ").unwrap();
        }
        d
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write a real shortcut to `target`.
fn make_shortcut(lnk: &Path, target: &Path) {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    unsafe {
        // Stays in the apartment, as `locate::shortcut_target` does.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        {
            // Released before COM is torn down.
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
            link.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
            link.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(lnk.as_os_str()), true).unwrap();
        }
    }
}

fn index() -> Index {
    let idx = test_index();
    idx.add_test_package("Example.MediaPlayer", "Media Player", "1.12.0", &["{EEBA99CE-0000-4545-834F-BE6FA6DE0E47}"], &[], &["Media Player"], "Example Media");
    idx.add_test_package("Example.Painter", "Painter", "4.0", &["Painter"], &[], &["Painter"], "Example Paint");
    idx
}

const BUNDLE: &str = "{EEBA99CE-0000-4545-834F-BE6FA6DE0E47}";
const PACKAGE: &str = "{D935BD04-0000-4D85-A13F-63FBC9B6000F}";

/// A WiX Burn bundle as it registers itself: its program is the installer kept in the Package
/// Cache, with no install folder and no install date.
fn bundle() -> winutil::UninstallEntry {
    let cached = format!(r"C:\ProgramData\Package Cache\{BUNDLE}\MediaPlayer-1.12.0-windows-x64.exe");
    winutil::UninstallEntry {
        key: BUNDLE.into(),
        hive: "HKLM32",
        display_name: "Media Player".into(),
        display_version: "1.12.0".into(),
        publisher: "Example Media".into(),
        display_icon: format!("{cached},0"),
        uninstall_string: format!("\"{cached}\"  /uninstall"),
        quiet_uninstall_string: format!("\"{cached}\" /uninstall /quiet"),
        bundle: true,
        ..Default::default()
    }
}

/// The MSI package the bundle installed: hidden, with the install date, no folder and no icon.
fn package() -> winutil::UninstallEntry {
    winutil::UninstallEntry {
        key: PACKAGE.into(),
        hive: "HKLM64",
        display_name: "Media Player".into(),
        display_version: "1.12.0".into(),
        publisher: "Example Media".into(),
        uninstall_string: format!("MsiExec.exe /I{PACKAGE}"),
        install_date: "20261022".into(),
        windows_installer: true,
        system_component: true,
        ..Default::default()
    }
}

fn listed(entries: Vec<winutil::UninstallEntry>, hints: Hints) -> Vec<Installed> {
    let r = match_all(&entries, &index(), &[], &HashMap::new());
    let cell = std::cell::OnceCell::new();
    let _ = cell.set(hints);
    installed_copies_with(&r.matches, None, &cell)
}

#[test]
fn a_burn_bundle_and_its_msi_package_are_one_app_with_its_date_program_and_folder() {
    let fx = Fixture::new("bundle");
    let dir = fx.app("Example\\Media Player", &["MediaPlayer.exe", "QtWebEngineProcess.exe", "vc_redist.x64.exe"]);
    // The Start menu shortcut, in a folder of its own, is what says where the program is.
    let menu = fx.app("Start Menu\\Programs\\Media Player", &[]);
    make_shortcut(&menu.join("Media Player.lnk"), &dir.join("MediaPlayer.exe"));
    let hints = Hints { shortcuts: locate::shortcuts_in(&[fx.0.join("Start Menu\\Programs")]), app_paths: vec![] };
    assert_eq!(hints.shortcuts.len(), 1);

    let list = listed(vec![bundle(), package()], hints);
    assert_eq!(list.len(), 1);
    let app = &list[0];
    assert_eq!(app.id, "other:Example.MediaPlayer");
    assert_eq!(app.updated_at.as_deref(), Some("2026-10-22T00:00:00Z"));
    assert_eq!(app.exe.as_deref(), Some(dir.join("MediaPlayer.exe").as_path()));
    assert_eq!(app.install_dir.as_deref(), Some(dir.as_path()));
    // Removed through the bundle, which removes its packages too.
    let un = app.uninstall.as_ref().unwrap();
    assert!(un.iter().any(|a| a == "/uninstall") && un[0].contains("Package Cache"), "{un:?}");
}

#[test]
fn a_bundle_without_a_dated_package_is_dated_by_its_entry() {
    let mut b = bundle();
    let when = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_792_656_000);
    b.written = Some(when);
    let list = listed(vec![b], Hints::default());
    assert_eq!(list[0].updated_at.as_deref(), Some("2026-10-22T08:00:00Z"));
}

#[test]
fn an_uninstaller_named_as_the_icon_is_not_the_program() {
    let fx = Fixture::new("uninst");
    let dir = fx.app("Painter", &["Painter.exe", "Uninstall.exe", "crashpad_handler.exe"]);
    let e = winutil::UninstallEntry {
        key: "Painter".into(),
        hive: "HKLM64",
        display_name: "Painter".into(),
        display_version: "4.0".into(),
        publisher: "Example Paint".into(),
        display_icon: format!("\"{}\",0", dir.join("Uninstall.exe").display()),
        uninstall_string: format!("\"{}\"", dir.join("Uninstall.exe").display()),
        ..Default::default()
    };
    let list = listed(vec![e], Hints::default());
    assert_eq!(list[0].exe.as_deref(), Some(dir.join("Painter.exe").as_path()));
    assert_eq!(list[0].install_dir.as_deref(), Some(dir.as_path()));
}

#[test]
fn an_install_folder_in_the_package_cache_is_not_the_apps_folder() {
    let fx = Fixture::new("cache");
    let cache = fx.app(&format!("Package Cache\\{BUNDLE}"), &["Painter-4.0-setup.exe"]);
    let e = winutil::UninstallEntry {
        key: "Painter".into(),
        hive: "HKLM64",
        display_name: "Painter".into(),
        display_version: "4.0".into(),
        publisher: "Example Paint".into(),
        install_location: cache.display().to_string(),
        uninstall_string: format!("\"{}\" /uninstall", cache.join("Painter-4.0-setup.exe").display()),
        ..Default::default()
    };
    let list = listed(vec![e], Hints::default());
    assert_eq!(list[0].install_dir, None);
    assert_eq!(list[0].exe, None);
}

#[test]
fn an_app_just_installed_from_the_starter_list_has_its_latest_version() {
    let list = listed(vec![bundle(), package()], Hints::default());
    let idx = index();
    let pkg = idx.by_id("Example.MediaPlayer").unwrap();
    let m = Manifest {
        id: "Example.MediaPlayer".into(),
        version: "1.12.0".into(),
        name: "Media Player".into(),
        installers: vec![Installer { arch: "x64".into(), kind: "burn".into(), url: "https://example.com/mp.exe".into(), sha256: "a".repeat(64), scope: Some("machine".into()), product_code: Some(BUNDLE.into()), ..Default::default() }],
        ..Default::default()
    };
    // The feed read before the install: the package only in the starter list.
    let feed = Feed { pkgs: HashMap::new(), popular: vec![(PkgFeed { pkg: pkg.clone(), versions: vec![], latest: Some(m.clone()) }, "Media player")], unfinished: false };
    assert_eq!(latest(&list[0], &feed).map(|l| l.version), Some("1.12.0".to_string()));
    // A package from a search result is added once read.
    let feed = Feed::default().with_package(PkgFeed { pkg, versions: vec![], latest: Some(m) });
    assert_eq!(latest(&list[0], &feed).map(|l| l.version), Some("1.12.0".to_string()));
}

#[test]
fn a_date_with_the_day_before_the_month_is_not_shown() {
    assert_eq!(winutil::install_date_iso("20262408"), None);
    assert_eq!(winutil::install_date_iso("20261022").as_deref(), Some("2026-10-22T00:00:00Z"));
    let mut b = bundle();
    b.install_date = "20262408".into();
    b.written = Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_792_656_000));
    let list = listed(vec![b], Hints::default());
    assert_eq!(list[0].updated_at.as_deref(), Some("2026-10-22T08:00:00Z"));
}
