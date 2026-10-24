//! Listing, uninstall commands, leftovers and backups, from entries, folders and registry keys
//! made up here. Registry fixtures live under a key of their own in HKCU and are removed after.

use super::backup;
use super::leftovers::{self, Env, Kind, ServiceRec, Subject, TaskRec};
use super::programs;
use super::safety::{key_refused, value_refused, Guard};
use super::switches::{self, Kind as Sk};
use super::{Hive, Target};
use crate::model::Source;
use crate::sources::{Installed, SourceData};
use crate::winutil::UninstallEntry;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- fixtures

/// A folder next to the test program (Temp never counts as an install folder). Removed on drop.
struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str) -> Fixture {
        let d = std::env::current_exe().unwrap().parent().unwrap().join(format!("uninstall-fixture-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Fixture(d)
    }
    fn dir(&self, rel: &str, files: &[&str]) -> PathBuf {
        let d = self.0.join(rel);
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

/// A key of its own under HKCU\Software. Removed on drop.
struct TestKey(String);

impl TestKey {
    fn new(name: &str) -> TestKey {
        let k = format!("Software\\TheUpdater3DTest-{name}-{}", std::process::id());
        let _ = Hive::Hkcu.root().delete_subkey_all(&k);
        TestKey(k)
    }
    fn sub(&self, rel: &str) -> String {
        format!("{}\\{rel}", self.0)
    }
    fn create(&self, rel: &str) -> winreg::RegKey {
        Hive::Hkcu.root().create_subkey(self.sub(rel)).unwrap().0
    }
}

impl Drop for TestKey {
    fn drop(&mut self) {
        let _ = Hive::Hkcu.root().delete_subkey_all(&self.0);
    }
}

fn make_shortcut(lnk: &Path, target: &Path) {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    std::fs::create_dir_all(lnk.parent().unwrap()).unwrap();
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).unwrap();
        link.SetPath(&HSTRING::from(target.as_os_str())).unwrap();
        link.cast::<IPersistFile>().unwrap().Save(&HSTRING::from(lnk.as_os_str()), true).unwrap();
    }
}

fn entry(key: &str, name: &str, uninstall: &str) -> UninstallEntry {
    UninstallEntry { key: key.into(), hive: "HKLM64", display_name: name.into(), display_version: "1.0".into(), publisher: "Zap Corp".into(), uninstall_string: uninstall.into(), ..Default::default() }
}

fn listed(e: &UninstallEntry) -> Installed {
    programs::make(e, None, None, &|_| true, None)
}

// ---------------------------------------------------------------- listing

#[test]
fn every_program_is_listed_once_without_system_parts_updates_or_claimed_ones() {
    let app = entry("Zapster", "Zapster", r#""C:\Program Files\Zapster\uninst.exe""#);
    let mut twin = app.clone();
    twin.hive = "HKLM32";
    let mut part = entry("{11111111-2222-3333-4444-555555555555}", "Zapster runtime", "MsiExec.exe /X{11111111-2222-3333-4444-555555555555}");
    part.system_component = true;
    let mut update = entry("KB123", "Update for Zapster", "x.exe");
    update.parent_key = "Zapster".into();
    let nameless = entry("Nameless", " ", "x.exe");
    let adobe = entry("PHSP", "Photoshop", r#""C:\Program Files\Common Files\Adobe\Uninstall.exe" --sapCode=PHSP"#);
    let matched = entry("Painter", "Painter", r#""C:\Program Files\Painter\unins000.exe""#);
    // A Burn bundle and its MSI package: one program, the bundle.
    let mut bundle = entry("{aaaaaaaa-0000-0000-0000-000000000000}", "Player", r#""C:\ProgramData\Package Cache\{aaaaaaaa-0000-0000-0000-000000000000}\setup.exe" /uninstall"#);
    bundle.bundle = true;
    let mut pkg = entry("{BBBBBBBB-0000-0000-0000-000000000000}", "Player", "MsiExec.exe /X{BBBBBBBB-0000-0000-0000-000000000000}");
    pkg.windows_installer = true;
    let other_app = Installed { id: "other:Example.Painter".into(), source: Source::Other, data: SourceData::Other { pkg: "Example.Painter".into(), how: crate::sources::other::How::Name, entry: Box::new(matched.clone()), upgrade_code: None }, ..listed(&matched) };
    let all = vec![app, twin, part, update, nameless, adobe, matched, bundle, pkg];
    let got = programs::entries(&all, &[other_app]);
    let names: Vec<&str> = got.iter().map(|e| e.display_name.as_str()).collect();
    assert_eq!(names, vec!["Zapster", "Player"]);
    assert!(got[1].bundle && got[1].msi_parts == vec!["{BBBBBBBB-0000-0000-0000-000000000000}".to_string()]);
    let i = listed(&got[0]);
    assert_eq!(i.id, "prog:hklm64:zapster");
    assert_eq!(i.source, Source::Programs);
    assert_eq!(i.tagline.as_deref(), Some("Zap Corp"));
    assert_eq!(i.code, "Za");
}

/// Only drive roots exist (C:\, Q:\): any file on them is gone.
fn drive_only(p: &Path) -> bool {
    p.parent().is_none() || p.as_os_str().len() <= 3
}

#[test]
fn a_program_whose_uninstaller_is_gone_is_broken_and_never_runs() {
    let e = entry("Gone", "Gone App", r#""C:\Program Files\Gone\uninstall.exe" /x"#);
    let i = programs::make(&e, None, None, &drive_only, None);
    assert!(i.missing && i.uninstall.is_none());
    assert!(i.tagline.unwrap().contains("uninstall.exe is missing"));
    // There: not broken.
    assert!(!programs::make(&e, None, None, &|_| true, None).missing);
    // Run through Windows' own programs: their target is not checked.
    let r = entry("Dll", "Dll App", r#"C:\Windows\System32\rundll32.exe "C:\gone\x.dll",Uninstall"#);
    assert!(!programs::make(&r, None, None, &|_| false, None).missing);
    // An MSI product whose cached setup is gone can not be removed by Windows Installer.
    let mut m = entry("{11111111-2222-3333-4444-555555555555}", "Msi App", "MsiExec.exe /X{11111111-2222-3333-4444-555555555555}");
    m.windows_installer = true;
    assert!(programs::make(&m, None, None, &|_| false, Some(Path::new(r"C:\Windows\Installer\1a2b.msi"))).missing);
    assert!(!programs::make(&m, None, None, &|_| false, None).missing);
}

#[test]
fn packed_guids_round_trip() {
    let g = "{11111111-2222-3333-4444-555555555555}";
    let p = switches::pack_guid(g).unwrap();
    assert_eq!(crate::winutil::unpack_guid(&p).unwrap(), g);
}

// ---------------------------------------------------------------- silent switches

#[test]
fn the_silent_switch_is_used_only_when_the_installer_type_is_known() {
    let mut q = entry("Q", "Quiet", r#""C:\P\Q\uninst.exe""#);
    q.quiet_uninstall_string = r#""C:\P\Q\uninst.exe" /quiet"#.into();
    let c = switches::choose(&q, switches::detect(&q, b"")).unwrap();
    assert!(c.silent && c.args == vec![r"C:\P\Q\uninst.exe", "/quiet"]);

    let mut msi = entry("{11111111-2222-3333-4444-555555555555}", "Msi", "MsiExec.exe /I{11111111-2222-3333-4444-555555555555}");
    msi.windows_installer = true;
    let c = switches::choose(&msi, switches::detect(&msi, b"")).unwrap();
    assert_eq!(c.args, vec!["msiexec.exe", "/x", "{11111111-2222-3333-4444-555555555555}", "/qn", "/norestart"]);

    let nsis = entry("Nsis", "Nsis App", r#""C:\P\Nsis App\Uninstall.exe""#);
    assert_eq!(switches::detect(&nsis, b"MZ....NullsoftInst...."), Sk::Nsis);
    let c = switches::choose(&nsis, Sk::Nsis).unwrap();
    assert!(c.silent && c.args.contains(&"/S".to_string()) && c.args.contains(&r"_?=C:\P\Nsis App".to_string()));

    let inno = entry("Inno App_is1", "Inno App", r#""C:\P\Inno\unins000.exe""#);
    let c = switches::choose(&inno, switches::detect(&inno, b"")).unwrap();
    assert!(c.silent && c.args.contains(&"/VERYSILENT".to_string()));
    let inno2 = entry("InnoB", "Inno B", r#""C:\P\InnoB\unins000.exe""#);
    assert_eq!(switches::detect(&inno2, b"..Inno Setup.."), Sk::Inno);

    let mut burn = entry("{aaaaaaaa-0000-0000-0000-000000000000}", "Burn", r#""C:\ProgramData\Package Cache\{aaaaaaaa-0000-0000-0000-000000000000}\setup.exe" /uninstall"#);
    burn.bundle = true;
    let c = switches::choose(&burn, switches::detect(&burn, b"")).unwrap();
    assert!(c.silent && c.args.ends_with(&["/uninstall".to_string(), "/quiet".into(), "/norestart".into()]));

    let sq = entry("chatty", "Chatty", r#""C:\Users\A\AppData\Local\chatty\Update.exe" --uninstall"#);
    let c = switches::choose(&sq, switches::detect(&sq, b"")).unwrap();
    assert!(c.silent && c.args.last().unwrap() == "-s");

    // Anything else runs as the vendor made it, with its window.
    let other = entry("Odd", "Odd App", r#""C:\P\Odd\remove.exe" /remove"#);
    let c = switches::choose(&other, switches::detect(&other, b"MZ plain")).unwrap();
    assert!(!c.silent);
    assert_eq!(c.args, vec![r"C:\P\Odd\remove.exe", "/remove"]);
}

// ---------------------------------------------------------------- leftovers

fn env_for(fx: &Fixture, key: &TestKey) -> Env {
    Env {
        guard: Guard { roots: vec![], trees: vec![fx.0.join("Documents")], others: vec![fx.0.join("Vendor\\OtherApp")], shortcut_dirs: vec![fx.0.join("Start"), fx.0.join("Desktop")] },
        start_menus: vec![fx.0.join("Start")],
        desktops: vec![fx.0.join("Desktop")],
        startup: vec![fx.0.join("Start\\Startup")],
        run_keys: vec![(Hive::Hkcu, key.sub("Run"))],
        app_paths: vec![(Hive::Hkcu, key.sub("App Paths"))],
        classes: vec![(Hive::Hkcu, key.sub("Classes"))],
        software: vec![(Hive::Hkcu, key.sub("Soft"))],
        services: vec![],
        tasks: vec![],
        other_names: vec!["Painter".into()],
        shortcut_target: Box::new(crate::sources::locate::shortcut_target),
    }
}

#[test]
fn each_kind_of_leftover_is_found_and_nothing_else() {
    let fx = Fixture::new("kinds");
    let key = TestKey::new("kinds");
    let dir = fx.dir("Apps\\Zapster", &["data.bin"]);
    let gone_exe = dir.join("Zapster.exe");
    let elsewhere = fx.dir("Apps\\Painter", &["Painter.exe"]);
    // Shortcuts: to the removed program (in a folder of its own), on the desktop to a program
    // that is gone and named like it, in Startup; and ones that stay.
    make_shortcut(&fx.0.join("Start\\Zapster\\Zapster.lnk"), &gone_exe);
    make_shortcut(&fx.0.join("Desktop\\Zapster Tray.lnk"), Path::new(r"C:\Gone\Zap\ZapsterTray.exe"));
    make_shortcut(&fx.0.join("Start\\Startup\\Zapster Agent.lnk"), &dir.join("agent.exe"));
    make_shortcut(&fx.0.join("Start\\Painter.lnk"), &elsewhere.join("Painter.exe"));
    make_shortcut(&fx.0.join("Desktop\\Unrelated.lnk"), Path::new(r"C:\Gone\Other\Other.exe"));
    // Run values.
    let run = key.create("Run");
    run.set_value("Zapster", &format!("\"{}\" --tray", gone_exe.display())).unwrap();
    run.set_value("Painter", &format!("\"{}\"", elsewhere.join("Painter.exe").display())).unwrap();
    run.set_value("Notepad", &r"C:\Windows\notepad.exe".to_string()).unwrap();
    // App Paths, file types, "Open with".
    key.create("App Paths\\Zapster.exe").set_value("", &gone_exe.display().to_string()).unwrap();
    key.create("App Paths\\Painter.exe").set_value("", &elsewhere.join("Painter.exe").display().to_string()).unwrap();
    key.create("Classes\\Zapster.Document\\shell\\open\\command").set_value("", &format!("\"{}\" \"%1\"", gone_exe.display())).unwrap();
    key.create("Classes\\.zap").set_value("", &"Zapster.Document".to_string()).unwrap();
    key.create("Classes\\Applications\\Zapster.exe\\shell\\open\\command").set_value("", &format!("\"{}\" \"%1\"", gone_exe.display())).unwrap();
    key.create("Classes\\Painter.Image\\shell\\open\\command").set_value("", &format!("\"{}\" \"%1\"", elsewhere.join("Painter.exe").display())).unwrap();
    // Its own keys, its publisher's other product, another program's key.
    key.create("Soft\\ZapCorp\\Zapster").set_value("Theme", &"dark".to_string()).unwrap();
    key.create("Soft\\ZapCorp\\Zapper Pro");
    key.create("Soft\\Zapster");
    key.create("Soft\\Painter");
    // Its uninstall entry.
    key.create("Uninstall\\Zapster").set_value("DisplayName", &"Zapster".to_string()).unwrap();

    let mut env = env_for(&fx, &key);
    let sys = r"C:\Windows\System32\svchost.exe -k netsvcs";
    env.services = vec![
        ServiceRec { name: "ZapSvc".into(), display: "Zapster Service".into(), image: format!("\"{}\" -service", dir.join("zapsvc.exe").display()) },
        ServiceRec { name: "Netsvc".into(), display: "Windows".into(), image: sys.into() },
        // A folder whose name starts like it is another folder.
        ServiceRec { name: "Zap2".into(), display: "Zapster 2".into(), image: format!("{}2\\svc.exe", dir.display()) },
    ];
    env.tasks = vec![
        TaskRec { path: "\\ZapCorp\\Zapster Update".into(), command: dir.join("update.exe").display().to_string() },
        TaskRec { path: "\\Zapster Check".into(), command: r"C:\Gone\Zap\check.exe".into() },
        TaskRec { path: "\\Painter Update".into(), command: elsewhere.join("Painter.exe").display().to_string() },
        TaskRec { path: "\\Unrelated".into(), command: r"C:\Gone\Unrelated\x.exe".into() },
    ];
    let s = Subject { name: "Zapster".into(), publisher: "ZapCorp Ltd.".into(), install_dir: Some(dir.clone()), entry: Some((Hive::Hkcu, key.sub("Uninstall\\Zapster"))), exes: vec![gone_exe.clone()] };
    let items = leftovers::scan(&s, &env, &[]);
    let got: Vec<(Kind, String)> = items.iter().map(|i| (i.kind, i.label.replace(&key.0, "K").replace(&fx.0.display().to_string(), "F"))).collect();
    let want = vec![
        (Kind::Folder, r"F\Apps\Zapster".to_string()),
        (Kind::Entry, r"HKCU\K\Uninstall\Zapster".into()),
        (Kind::Shortcut, r"F\Desktop\Zapster Tray.lnk".into()),
        (Kind::Startup, r"F\Start\Startup\Zapster Agent.lnk".into()),
        (Kind::Shortcut, r"F\Start\Zapster\Zapster.lnk".into()),
        (Kind::Startup, r"HKCU\K\Run (Zapster)".into()),
        (Kind::AppPath, r"HKCU\K\App Paths\Zapster.exe".into()),
        (Kind::FileType, r"HKCU\K\Classes\Zapster.Document".into()),
        (Kind::FileType, r"HKCU\K\Classes\Applications\Zapster.exe".into()),
        (Kind::Registry, r"HKCU\K\Soft\ZapCorp\Zapster".into()),
        (Kind::Registry, r"HKCU\K\Soft\Zapster".into()),
        (Kind::Service, "Zapster Service (ZapSvc)".into()),
        (Kind::Task, r"\ZapCorp\Zapster Update".into()),
        (Kind::Task, r"\Zapster Check".into()),
    ];
    let mut a = got.clone();
    let mut b = want.clone();
    a.sort_by(|x, y| x.1.cmp(&y.1));
    b.sort_by(|x, y| x.1.cmp(&y.1));
    assert_eq!(a, b, "found: {got:#?}");
    // A gone program's entries are ticked; the folder and service go with it.
    assert!(items.iter().all(|i| i.checked), "{items:#?}");
    assert!(items.iter().all(|i| !i.elevated || matches!(i.target, Target::Service { .. } | Target::Task { .. } | Target::Dir { .. } | Target::File { .. })));
}

#[test]
fn a_shortcut_to_a_program_that_is_still_there_is_listed_but_not_ticked() {
    let fx = Fixture::new("there");
    let key = TestKey::new("there");
    let dir = fx.dir("Apps\\Zapster", &["Zapster.exe"]);
    make_shortcut(&fx.0.join("Start\\Zapster.lnk"), &dir.join("Zapster.exe"));
    let s = Subject { name: "Zapster".into(), install_dir: Some(dir.clone()), ..Default::default() };
    let items = leftovers::scan(&s, &env_for(&fx, &key), &[]);
    let sc = items.iter().find(|i| i.kind == Kind::Shortcut).unwrap();
    assert!(!sc.checked);
}

#[test]
fn shared_and_protected_folders_are_never_offered() {
    let fx = Fixture::new("guard");
    let key = TestKey::new("guard");
    let env = env_for(&fx, &key);
    // A vendor folder that holds another program's folder.
    let vendor = fx.dir("Vendor", &["a.dll"]);
    fx.dir("Vendor\\OtherApp", &["Other.exe"]);
    let s = Subject { name: "Vendor Tool".into(), install_dir: Some(vendor.clone()), ..Default::default() };
    assert!(leftovers::scan(&s, &env, &[]).iter().all(|i| i.kind != Kind::Folder));
    assert_eq!(env.guard.dir_refused(&vendor), Some("another installed program's folder"));
    // Inside the other program's folder.
    assert!(env.guard.dir_refused(&fx.0.join("Vendor\\OtherApp\\plugins")).is_some());
    // The user's own files.
    let docs = fx.dir("Documents\\Zapster", &["my project.zap"]);
    let s = Subject { name: "Zapster".into(), install_dir: Some(docs.clone()), ..Default::default() };
    assert!(leftovers::scan(&s, &env, &[]).is_empty());
    // A browser profile.
    let prof = fx.dir("Apps\\Browser", &[]);
    fx.dir("Apps\\Browser\\User Data", &[]);
    assert_eq!(env.guard.dir_refused(&prof), Some("a browser profile"));
    // Shared folder names and drive roots.
    assert!(env.guard.dir_refused(&fx.0.join("Common Files")).is_some());
    assert!(env.guard.dir_refused(Path::new("C:\\")).is_some());
    // Only shortcuts, and only where shortcuts live.
    assert!(env.guard.shortcut_refused(&fx.0.join("Start\\x.lnk")).is_none());
    assert!(env.guard.shortcut_refused(&fx.0.join("Start\\x.exe")).is_some());
    assert!(env.guard.shortcut_refused(&fx.0.join("Documents\\x.lnk")).is_some());
}

#[test]
fn the_real_system_folders_are_refused() {
    let g = Guard::system(vec![]);
    let env = |n: &str| PathBuf::from(std::env::var(n).unwrap());
    for p in [env("SystemRoot"), env("SystemRoot").join("System32"), env("ProgramFiles"), env("ProgramFiles(x86)"), env("ProgramData"), env("USERPROFILE"), env("APPDATA"), env("LOCALAPPDATA"), env("USERPROFILE").join("Documents\\Projects"), env("USERPROFILE").join("Desktop"), env("USERPROFILE").join("Downloads"), env("LOCALAPPDATA").join("Google\\Chrome\\User Data"), env("APPDATA").join("Mozilla\\Firefox"), env("ProgramFiles").join("Common Files"), env("LOCALAPPDATA").join("Programs")] {
        assert!(g.dir_refused(&p).is_some(), "{} must be refused", p.display());
    }
    assert!(g.dir_refused(&env("ProgramFiles").join("Zapster")).is_none());
}

/// Programs and games live on every drive: the folders that hold many of them are refused on
/// each drive, not only on the Windows one, while one program's or game's own folder there can
/// still be removed.
#[test]
fn the_folders_that_hold_programs_are_refused_on_every_drive() {
    let g = Guard::system(vec![]);
    for p in [r"D:\Program Files", r"E:\Program Files (x86)", r"D:\ProgramData", r"D:\SteamLibrary", r"F:\XboxGames", r"D:\Games", r"D:\GOG Games", r"d:\program files\"] {
        assert!(g.dir_refused(Path::new(p)).is_some(), "{p} must be refused");
    }
    for p in [r"D:\Program Files\Zapster", r"E:\Program Files (x86)\ZapCorp\Zapster", r"D:\Games\Hades", r"D:\Zapster", r"D:\SteamLibrary\steamapps\common\Some Game"] {
        assert!(g.dir_refused(Path::new(p)).is_none(), "{p} must stay removable");
    }
}

#[test]
fn only_one_programs_own_keys_and_startup_values_can_be_deleted() {
    for k in ["Software", "Software\\Microsoft", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall", "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run", "Software\\Classes", "Software\\Classes\\.txt", "Software\\Classes\\CLSID", "Software\\Classes\\Applications", "SOFTWARE\\WOW6432Node", "SOFTWARE\\Policies\\Zap", "SYSTEM\\CurrentControlSet", "Software\\Clients\\StartMenuInternet"] {
        assert!(key_refused(Hive::Hklm, k).is_some(), "{k}");
    }
    for k in ["SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Zapster", "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{X}", "Software\\Microsoft\\Windows\\CurrentVersion\\App Paths\\zap.exe", "Software\\Classes\\Zapster.Document", "Software\\Classes\\Applications\\zap.exe", "Software\\ZapCorp\\Zapster", "SOFTWARE\\WOW6432Node\\ZapCorp\\Zapster", "SYSTEM\\CurrentControlSet\\Services\\ZapSvc"] {
        assert!(key_refused(Hive::Hklm, k).is_none(), "{k}");
    }
    assert!(value_refused(Hive::Hkcu, "Software\\Microsoft\\Windows\\CurrentVersion\\Run").is_none());
    assert!(value_refused(Hive::Hklm, "SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\RunOnce").is_none());
    assert!(value_refused(Hive::Hkcu, "Software\\Microsoft\\Windows\\CurrentVersion\\Explorer").is_some());
}

#[test]
fn services_count_only_inside_the_folder_and_tasks_only_when_theirs() {
    let fx = Fixture::new("svc");
    let key = TestKey::new("svc");
    let dir = fx.dir("Apps\\Zapster", &[]);
    let mut env = env_for(&fx, &key);
    // Named like it, but its program is elsewhere: kept.
    env.services = vec![ServiceRec { name: "ZapsterSvc".into(), display: "Zapster".into(), image: r"C:\Gone\Zapster\svc.exe".into() }];
    let s = Subject { name: "Zapster".into(), install_dir: Some(dir), ..Default::default() };
    assert!(leftovers::scan(&s, &env, &[]).iter().all(|i| i.kind != Kind::Service));
    // No folder known: no services at all.
    env.services = vec![ServiceRec { name: "ZapSvc".into(), display: "Zapster".into(), image: fx.0.join("Apps\\Zapster\\svc.exe").display().to_string() }];
    let s = Subject { name: "Zapster".into(), ..Default::default() };
    assert!(leftovers::scan(&s, &env, &[]).iter().all(|i| i.kind != Kind::Service));
}

#[test]
fn settings_from_prefs_are_listed_unticked() {
    let fx = Fixture::new("prefs");
    let key = TestKey::new("prefs");
    let cfg = fx.dir("Roaming\\Zapster", &["settings.json"]);
    let steps = vec![crate::sources::Step::RemoveDir { path: cfg.clone(), elevated: false, label: String::new() }, crate::sources::Step::RemoveUserRegKey { path: key.sub("Soft\\Zapster"), label: String::new() }];
    key.create("Soft\\Zapster");
    let s = Subject { name: "Zapster".into(), ..Default::default() };
    let items = leftovers::scan(&s, &env_for(&fx, &key), &steps);
    let st: Vec<_> = items.iter().filter(|i| i.kind == Kind::Settings).collect();
    assert_eq!(st.len(), 2);
    assert!(st.iter().all(|i| !i.checked));
    // The key prefs found is not listed twice.
    assert!(items.iter().all(|i| i.kind != Kind::Registry));
}

// ---------------------------------------------------------------- deleting and backups

fn values(path: &str) -> Vec<(String, String)> {
    let k = super::open(Hive::Hkcu, path).unwrap();
    let mut v: Vec<(String, String)> = k.enum_values().flatten().map(|(n, val)| (n, format!("{:?}:{:?}", val.vtype, &val.bytes[..]))).collect();
    v.sort();
    v
}

#[test]
fn registry_keys_are_saved_before_deleting_and_come_back_on_restore() {
    let key = TestKey::new("backup");
    let fx = Fixture::new("backup");
    let path = key.sub("Soft\\ZapCorp\\Zapster");
    let k = key.create("Soft\\ZapCorp\\Zapster");
    k.set_value("", &"default \"quoted\" C:\\path".to_string()).unwrap();
    k.set_value("Lines", &"one\r\ntwo".to_string()).unwrap();
    k.set_value("Count", &42u32).unwrap();
    k.set_value("Big", &(1u64 << 40)).unwrap();
    k.set_value("List", &vec!["a".to_string(), "b c".to_string()]).unwrap();
    k.set_raw_value("Bin", &winreg::RegValue { bytes: vec![0, 1, 2, 255].into(), vtype: winreg::enums::RegType::REG_BINARY }).unwrap();
    let exp: Vec<u8> = "%TEMP%\\x\0".encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    k.set_raw_value("Exp", &winreg::RegValue { bytes: exp.into(), vtype: winreg::enums::RegType::REG_EXPAND_SZ }).unwrap();
    key.create("Soft\\ZapCorp\\Zapster\\Sub").set_value("Deep", &"yes".to_string()).unwrap();
    let before = values(&path);
    let before_sub = values(&format!("{path}\\Sub"));
    drop(k);

    let root = fx.0.join("backups");
    let saved = backup::save(&root, "Zapster", &[Target::Key { hive: Hive::Hkcu, path: path.clone() }]).unwrap().unwrap();
    super::remove::apply(&Target::Key { hive: Hive::Hkcu, path: path.clone() }).unwrap();
    assert!(!super::key_exists(Hive::Hkcu, &path));
    // The publisher's key left empty goes too.
    assert!(!super::key_exists(Hive::Hkcu, &key.sub("Soft\\ZapCorp")));
    assert_eq!(backup::list(&root).len(), 1);

    for step in backup::restore_steps(&backup::load(&root, &saved.id).unwrap()) {
        let r = crate::elevate::execute(&step);
        assert!(r.ok, "{} {}", r.message, r.output);
    }
    assert_eq!(values(&path), before);
    assert_eq!(values(&format!("{path}\\Sub")), before_sub);
}

#[test]
fn a_startup_value_is_saved_and_restored_alone() {
    let key = TestKey::new("value");
    let fx = Fixture::new("value");
    let run = key.create("Run");
    run.set_value("Zapster", &"\"C:\\Gone\\Zapster.exe\" --tray".to_string()).unwrap();
    run.set_value("Keep", &"C:\\Windows\\notepad.exe".to_string()).unwrap();
    let text = backup::export_value(Hive::Hkcu, &key.sub("Run"), "Zapster").unwrap();
    assert!(text.contains("\"Zapster\"=\"\\\"C:\\\\Gone\\\\Zapster.exe\\\" --tray\""), "{text}");
    assert!(!text.contains("Keep"));
    // Deleting a value anywhere but the startup lists is refused, whatever asks for it.
    let t = Target::Value { hive: Hive::Hkcu, path: key.sub("Run"), name: "Zapster".into() };
    assert!(super::remove::apply(&t).is_err());
    assert!(run.get_value::<String, _>("Zapster").is_ok());
    // Restoring the saved value puts it back next to the others.
    let root = fx.0.join("b");
    let saved = backup::save(&root, "Zapster", &[t]).unwrap().unwrap();
    run.delete_value("Zapster").unwrap();
    for step in backup::restore_steps(&saved) {
        assert!(crate::elevate::execute(&step).ok);
    }
    assert_eq!(run.get_value::<String, _>("Zapster").unwrap(), "\"C:\\Gone\\Zapster.exe\" --tray");
    assert!(run.get_value::<String, _>("Keep").is_ok());
}

#[test]
fn old_backups_are_pruned_after_30_days() {
    let fx = Fixture::new("prune");
    let root = fx.0.clone();
    let now = chrono::Utc::now();
    for (id, days) in [("old", 31), ("new", 2)] {
        std::fs::create_dir_all(root.join(id)).unwrap();
        let s = backup::Saved { id: id.into(), created: (now - chrono::Duration::days(days)).to_rfc3339(), app: "Zapster".into(), items: vec![backup::SavedItem::Task { path: "\\x".into(), file: "x.xml".into() }], restored: false };
        backup::write_manifest(&root, &s).unwrap();
    }
    backup::prune(&root, now);
    assert!(!root.join("old").exists());
    assert!(root.join("new").exists());
    assert_eq!(backup::list(&root).iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), vec!["new"]);
    // A backup id never reaches outside the backups folder.
    assert!(backup::load(&root, "..\\new").is_none());
}

#[test]
fn leftover_files_are_deleted_and_an_emptied_start_menu_folder_is_not_left() {
    let fx = Fixture::new("files");
    let dir = fx.dir("Apps\\Zapster", &["a.bin"]);
    super::remove::apply(&Target::Dir { path: dir.clone() }).unwrap();
    assert!(!dir.exists());
    // A file that is neither a shortcut where shortcuts live nor a settings file stays.
    let f = fx.dir("Apps\\Keep", &["keep.txt"]).join("keep.txt");
    assert!(super::remove::apply(&Target::File { path: f.clone() }).is_err());
    assert!(f.exists());
}

#[test]
fn a_launchers_uninstall_link_is_opened_as_the_user_and_waits_for_it() {
    // ShareX from Steam, as Windows lists it (machine wide).
    let e = entry("Steam App 400040", "ShareX", r#""C:\Program Files (x86)\Steam\steam.exe" steam://uninstall/400040"#);
    let i = programs::make(&e, None, None, &|_| true, None);
    let p = super::run::plan(&i).unwrap();
    assert_eq!(p.steps.len(), 1);
    let crate::sources::Step::OpenUri { uri, label } = &p.steps[0] else { panic!("{:?}", p.steps[0]) };
    assert_eq!(uri, "steam://uninstall/400040");
    assert_eq!(label, "Uninstalling ShareX: confirm it in Steam");
    assert_eq!(super::run::handed_to(&p).as_deref(), Some("Steam"));
    assert!(matches!(p.verify, crate::sources::Verify::Removed { .. }));
    // Matched to the catalog's ShareX (an Inno installer): no Inno switches go with the link.
    assert_eq!(crate::sources::other::uninstall_cmd(&e, Some("inno")), None);
    // Other launchers; an ordinary uninstaller and a web address are no hand-off.
    let link = |l: &str| super::run::launcher_link(&crate::winutil::split_program_line(l)).map(|(_, by)| by);
    assert_eq!(link(r#""C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\upc.exe" uplay://uninstall/5595"#).as_deref(), Some("Ubisoft Connect"));
    assert_eq!(link(r#""C:\Program Files\Odd\remove.exe" /S"#), None);
    assert_eq!(link(r#""C:\Program Files\Odd\remove.exe" https://example.com/bye"#), None);
}

#[test]
fn a_program_plan_runs_its_uninstaller_and_checks_it_is_gone() {
    let mut e = entry("Odd", "Odd App", r#""C:\Program Files\Odd\remove.exe""#);
    e.hive = "HKCU";
    let i = programs::make(&e, None, None, &|_| true, None);
    let p = super::run::plan(&i).unwrap();
    let crate::sources::Step::Run { exe, elevated, ok_codes, label, .. } = &p.steps[0] else { panic!() };
    assert_eq!(exe, Path::new(r"C:\Program Files\Odd\remove.exe"));
    // In Program Files: admin rights even for a per-user entry.
    assert!(*elevated);
    // Its own window: the list decides whether it worked.
    assert!(ok_codes.contains(&1) && label.contains("answer"));
    assert!(matches!(p.verify, crate::sources::Verify::Removed { .. }));
    let broken = programs::make(&e, None, None, &drive_only, None);
    assert!(super::run::plan(&broken).is_err());
}

/// The PC-wide cleanup lists shortcuts whose program is gone, and nothing else: not live ones,
/// not ones to a drive that is not there right now (an unplugged disk), not network shares.
#[test]
fn dead_shortcuts_are_found_anywhere_and_live_ones_stay() {
    let fx = Fixture::new("deadlinks");
    let key = TestKey::new("deadlinks");
    let live = fx.dir(r"Apps\Live", &["Live.exe"]);
    make_shortcut(&fx.0.join(r"Start\Old Sync\Old Sync.lnk"), &fx.0.join(r"Apps\OldSync\sync.exe"));
    make_shortcut(&fx.0.join(r"Start\Startup\Gone Agent.lnk"), &fx.0.join(r"Apps\Agent\agent.exe"));
    make_shortcut(&fx.0.join(r"Start\Live.lnk"), &live.join("Live.exe"));
    make_shortcut(&fx.0.join(r"Desktop\Unplugged.lnk"), Path::new(r"Q:\Games\game.exe"));
    let env = env_for(&fx, &key);
    let items = leftovers::dead_shortcuts(&env);
    let labels: Vec<&str> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(items.len(), 2, "{labels:?}");
    assert!(labels.iter().any(|l| l.ends_with("Old Sync.lnk")) && labels.iter().any(|l| l.ends_with("Gone Agent.lnk")));
    assert!(items.iter().any(|i| i.kind == Kind::Startup));
    assert!(!leftovers::target_is_gone(Path::new(r"\\server\share\app.exe")));
    assert!(!leftovers::target_is_gone(Path::new(r"Q:\Games\game.exe")) || Path::new(r"Q:\").is_dir());
}

/// An uninstaller on a drive that is not there right now (an unplugged disk) or on a network share
/// does not make the program broken, so its entry is never offered for removal.
#[test]
fn programs_on_an_unplugged_drive_or_a_share_are_not_broken() {
    let usb = entry("Usb", "Usb App", r#""Q:\Apps\Usb\uninstall.exe""#);
    assert!(!programs::make(&usb, None, None, &|_| false, None).missing);
    let share = entry("Share", "Share App", r#""\\server\apps\Share\uninstall.exe""#);
    assert!(!programs::make(&share, None, None, &drive_only, None).missing);
    // The drive is there but the file is not: broken.
    let gone = entry("Gone2", "Gone Two", r#""Q:\Apps\Gone\uninstall.exe""#);
    assert!(programs::make(&gone, None, None, &drive_only, None).missing);
}

/// A leftover scan never runs for an app that is still installed (it would offer its live files).
#[test]
fn installed_apps_are_never_scanned_for_leftovers() {
    assert!(!super::run::may_scan(true, false));
    assert!(super::run::may_scan(true, true));
    assert!(super::run::may_scan(false, false));
}

/// A leftover list goes stale after a while, so it is scanned again before deleting.
#[test]
fn old_leftover_lists_are_not_used_for_deleting() {
    let now = std::time::Instant::now();
    assert!(super::run::scan_fresh(now, now));
    assert!(!super::run::scan_fresh(now, now + super::run::SCAN_VALID));
}
