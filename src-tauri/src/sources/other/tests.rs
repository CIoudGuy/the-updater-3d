use super::*;
use crate::sources::winget::{test_index, ArpEntry};

pub(super) fn entry(key: &str, hive: &'static str, name: &str, ver: &str, publisher: &str) -> winutil::UninstallEntry {
    winutil::UninstallEntry {
        key: key.into(),
        hive,
        display_name: name.into(),
        display_version: ver.into(),
        publisher: publisher.into(),
        uninstall_string: format!("\"C:\\Program Files\\{name}\\uninstall.exe\""),
        ..Default::default()
    }
}

fn reasons(r: &Report) -> Vec<(String, Miss)> {
    r.misses.iter().map(|(e, m)| (e.display_name.clone(), m.clone())).collect()
}

fn index() -> Index {
    let idx = test_index();
    idx.add_test_package("Example.Zipper", "Zipper", "26.05", &["Zipper", "{AAAA0000-0000-0000-0000-000000002604}"], &["{AAAA0000-0000-0000-0000-00000000FFFF}"], &["Zipper 26.04 (x64)"], "Example Pavlov");
    idx.add_test_package("Example.Player", "Media Player", "3.0.24", &[], &[], &["Media Player"], "Example Media");
    // Two packages with the same name and publisher: nothing tells them apart.
    idx.add_test_package("Twins.App", "Twin App", "1.0", &[], &[], &[], "Twins Corp");
    idx.add_test_package("Twins.AppClassic", "Twin App", "1.1", &[], &[], &[], "Twins Corp");
    // Channels, architectures and version series in package ids.
    idx.add_test_package("Lion.Browser", "Lion", "155.0", &["Lion"], &[], &[], "Lion Software");
    idx.add_test_package("Lion.Browser.Beta", "Lion", "156.0", &["Lion"], &[], &[], "Lion Software");
    idx.add_test_package("Chat.Chat", "Chat", "1.0.9264", &["Chat"], &[], &[], "Chat Inc.");
    idx.add_test_package("Chat.Chat.arm64", "Chat", "1.0.9264", &["Chat"], &[], &[], "Chat Inc.");
    idx.add_test_package("Snake.Snake.3.12", "Snake", "3.12.10", &[], &[], &["Snake 3.12.3 (64-bit)"], "Snake Foundation");
    idx.add_test_package("Snake.Snake.3.13", "Snake", "3.13.5", &[], &[], &["Snake 3.13.1 (64-bit)"], "Snake Foundation");
    idx.add_test_package("Sculpt.Sculpt.2024", "Sculpt 2024", "2024.0.3", &[], &[], &["Sculpt 2024"], "Sculpt Co");
    idx.add_test_package("Selfmade.Bootstrapper", "Bootstrapper", "2.11.3", &[], &[], &[], "Some Developer");
    // One package per language, told apart by the locale in the name.
    idx.add_test_package("Fox.Browser", "Fox Browser (en-US)", "157.0", &["Fox Browser"], &[], &["Fox Browser (x64 en-US)"], "Fox");
    idx.add_test_package("Fox.Browser.de", "Fox Browser (de)", "157.0", &["Fox Browser"], &[], &["Fox Browser (x64 de)"], "Fox");
    // A product code that another, unrelated package also claims.
    idx.add_test_package("Wrong.Thing", "Completely Different", "9.9", &["Shared_is1"], &[], &[], "Someone Else");
    idx.add_test_package("Studio.Paint", "Paint Studio", "4.0", &[], &[], &["Paint Studio"], "Studio Folks");
    idx.add_test_package("Microsoft.Edge", "Microsoft Edge", "155.0", &["Microsoft Edge"], &[], &[], "Microsoft Corporation");
    idx.add_test_package("BlenderFoundation.Blender", "Blender", "5.2.2", &["{BBBB0000-0000-0000-0000-000000000001}"], &[], &[], "Blender Foundation");
    idx
}

#[test]
fn matches_by_product_code_upgrade_code_and_name() {
    let idx = index();
    let mut msi_old = entry("{AAAA0000-0000-0000-0000-000000002301}", "HKLM64", "Zipper 23.01 (x64 edition)", "23.01.00.0", "Example Pavlov");
    msi_old.windows_installer = true;
    let mut ups = HashMap::new();
    ups.insert("{AAAA0000-0000-0000-0000-000000002301}".to_string(), "{AAAA0000-0000-0000-0000-00000000FFFF}".to_string());
    let entries = vec![
        entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", "26.04", "Example Pavlov"),
        msi_old,
        entry("{F00D0000-0000-0000-0000-000000000000}", "HKLM32", "Media Player", "3.0.20.0", "Example Media"),
    ];
    let r = match_all(&entries, &idx, &[], &ups);
    let got: Vec<(&str, How)> = r.matches.iter().map(|m| (m.pkg.id.as_str(), m.how)).collect();
    // The first two are the same package: the exact product code wins, the MSI copy is a duplicate.
    assert_eq!(got, vec![("Example.Player", How::NamePublisher), ("Example.Zipper", How::ProductCode)]);
    assert_eq!(reasons(&r), vec![("Zipper 23.01 (x64 edition)".into(), Miss::Duplicate("Example.Zipper".into()))]);

    // Alone, the old MSI copy is found through its upgrade code.
    let r = match_all(&entries[1..2], &idx, &[], &ups);
    assert_eq!(r.matches[0].how, How::UpgradeCode);
    assert_eq!(r.matches[0].upgrade_code.as_deref(), Some("{AAAA0000-0000-0000-0000-00000000FFFF}"));
}

#[test]
fn drops_what_is_not_certain() {
    let idx = index();
    let mut hidden = entry("{0000}", "HKLM64", "Some Runtime Part", "1.0", "X");
    hidden.system_component = true;
    let mut update = entry("KB123", "HKLM64", "Update for Something", "1.0", "X");
    update.parent_key = "Something".into();
    let entries = vec![
        entry("TwinKey", "HKLM64", "Twin App 1.0", "1.0", "Twins Corp"),
        entry("Shared_is1", "HKLM64", "My Own Tool", "2.0", "Tool Maker"),
        entry("Media Player", "HKLM64", "Media Player", "3.0", ""),
        entry("Unknown_is1", "HKLM64", "Nothing Like It", "1.0", "Nobody"),
        entry("Microsoft Edge", "HKLM32", "Microsoft Edge", "155.0", "Microsoft Corporation"),
        entry("PaintKey", "HKCU", "Paint Studio", "", "Studio Folks"),
        hidden,
        update,
    ];
    let r = match_all(&entries, &idx, &[], &HashMap::new());
    assert!(r.matches.is_empty(), "{:?}", r.matches.iter().map(|m| &m.pkg.id).collect::<Vec<_>>());
    assert_eq!(r.hidden, 2);
    assert_eq!(
        reasons(&r),
        vec![
            ("Twin App 1.0".into(), Miss::Ambiguous(vec!["Twins.App".into(), "Twins.AppClassic".into()])),
            ("My Own Tool".into(), Miss::Unconfirmed("Wrong.Thing".into())),
            ("Media Player".into(), Miss::NoPublisher),
            ("Nothing Like It".into(), Miss::NotInCatalog),
            ("Microsoft Edge".into(), Miss::Excluded("updates itself")),
            ("Paint Studio".into(), Miss::NoVersion),
        ]
    );
}

#[test]
fn locale_and_folder_tell_packages_apart() {
    let idx = index();
    let mut paint = entry("{C0FFEE00-0000-0000-0000-000000000000}", "HKLM64", "PS Suite", "4.0", "Studio Folks");
    paint.install_location = "C:\\Program Files\\Paint Studio".into();
    let entries = vec![entry("Fox Browser", "HKLM64", "Fox Browser (x64 de)", "150.0", "Fox"), paint];
    let r = match_all(&entries, &idx, &[], &HashMap::new());
    let got: Vec<(&str, How)> = r.matches.iter().map(|m| (m.pkg.id.as_str(), m.how)).collect();
    assert_eq!(got, vec![("Fox.Browser.de", How::ProductCode), ("Studio.Paint", How::Location)]);
}

#[test]
fn package_ids_rule_out_channels_architectures_and_other_series() {
    let idx = index();
    let entries = vec![
        entry("Lion", "HKLM32", "Lion", "155.0.1", "Lion Software"),
        entry("Chat", "HKCU", "Chat", "1.0.9000", "Chat Inc."),
        entry("{5A5A0000-0000-0000-0000-000000000312}", "HKCU", "Snake 3.12.3 (64-bit)", "3.12.3150.0", "Snake Foundation"),
        entry("SculptKey", "HKLM64", "Sculpt 2026.2.1", "2026.2.1", "Sculpt Co"),
        entry("Bootstrapper", "HKCU", "Bootstrapper", "2.11.0", "Bootstrapper"),
    ];
    let r = match_all(&entries, &idx, &[], &HashMap::new());
    let got: Vec<(&str, How)> = r.matches.iter().map(|m| (m.pkg.id.as_str(), m.how)).collect();
    assert_eq!(got, vec![("Chat.Chat", How::ProductCode), ("Lion.Browser", How::ProductCode), ("Selfmade.Bootstrapper", How::Name), ("Snake.Snake.3.12", How::NamePublisher)]);
    // Sculpt 2026 is not in the catalog: the 2024 package is another product line.
    assert_eq!(reasons(&r), vec![("Sculpt 2026.2.1".into(), Miss::NotInCatalog)]);
}

#[test]
fn rescans_reuse_matches_until_an_entry_or_the_index_changes() {
    let path = std::env::temp_dir().join(format!("u3d-winget-index-{}.db", std::process::id()));
    index().save_to(&path);
    let idx = Index::open(&path).unwrap();
    let mut entries = vec![entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", "26.04", "Example Pavlov"), entry("Media Player", "HKLM64", "Media Player", "3.0", "Example Media")];
    let first = match_on_disk(&entries, &idx, &path, &[]);
    let plain = match_all(&entries, &idx, &[], &HashMap::new());
    let ids = |r: &Report| r.matches.iter().map(|m| (m.pkg.id.clone(), m.entry.display_version.clone())).collect::<Vec<_>>();
    assert_eq!(ids(&first), ids(&plain));
    // A changed entry is matched again; the result follows it.
    entries[1].display_version = "3.0.24".into();
    entries[0].display_name = "Unrelated Thing".into();
    entries[0].key = "Unrelated".into();
    let second = match_on_disk(&entries, &idx, &path, &[]);
    assert_eq!(ids(&second), vec![("Example.Player".to_string(), "3.0.24".to_string())]);
    drop(idx);
    let _ = std::fs::remove_file(&path);
}

#[test]
fn leaves_apps_of_the_other_sources_alone() {
    let idx = index();
    let mut blender = entry("{BBBB0000-0000-0000-0000-000000000001}", "HKLM64", "Blender", "5.2.2", "Blender Foundation");
    blender.windows_installer = true;
    let mut adobe = entry("Adobe Thing", "HKLM32", "Zipper", "26.04", "Example Pavlov");
    adobe.uninstall_string = "\"C:\\Program Files\\Common Files\\Adobe\\Installer\\Setup.exe\" --uninstall=1 --sapCode=ZIPR".into();
    let mut zipper = entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", "26.04", "Example Pavlov");
    zipper.install_location = "C:\\Program Files\\Zipper".into();
    // An app another source already lists (same folder) is not listed twice.
    let listed = Installed {
        id: "tools:zipper".into(),
        source: Source::Tools,
        name: "Zipper".into(),
        code: "Zi".into(),
        fg: String::new(),
        bg: String::new(),
        version: "26.04".into(),
        install_dir: Some(PathBuf::from("C:\\Program Files\\Zipper\\")),
        exe: None,
        uninstall: None,
        updated_at: None,
        size_hint: None,
        tagline: None,
        channel: None,
        missing: false,
        read_only: false,
        store: None,
        icon_remote: None,
        data: SourceData::Tools { key: "zipper".into(), series: String::new() },
    };
    let r = match_all(&[blender, adobe, zipper], &idx, &[listed], &HashMap::new());
    assert!(r.matches.is_empty());
    assert!(r.misses.iter().all(|(_, m)| *m == Miss::Handled));
    assert!(handled_package("BlenderFoundation.Blender.LTS.4.5") && handled_package("Unity.Unity.6000") && handled_package("Unity.UnityHub") && !handled_package("Unity.TuanjieHub"));
    assert_eq!(excluded_package("Microsoft.VisualStudio.2022.Community"), Some("updated by the Visual Studio Installer"));
    assert_eq!(excluded_package("Microsoft.VisualStudioCode"), None);
}

pub(super) fn inst(arch: &str, kind: &str, scope: Option<&str>) -> Installer {
    Installer { arch: arch.into(), kind: kind.into(), url: format!("https://example.com/{arch}-{kind}"), sha256: "a".repeat(64), scope: scope.map(String::from), ..Default::default() }
}

fn pkg_inst(arch: &str, kind: &str, family: &str) -> Installer {
    Installer { family: Some(family.into()), signature_sha256: Some("b".repeat(64)), ..inst(arch, kind, None) }
}

fn manifest(installers: Vec<Installer>) -> Manifest {
    Manifest { id: "Example.Zipper".into(), version: "26.05".into(), name: "Zipper".into(), installers, ..Default::default() }
}

#[test]
fn picks_the_installer_that_replaces_the_installed_copy() {
    let mut msi = inst("x64", "wix", None);
    msi.product_code = Some("{AAAA0000-0000-0000-0000-000000002605}".into());
    msi.arp = vec![ArpEntry { upgrade_code: Some("{AAAA0000-0000-0000-0000-00000000FFFF}".into()), display_version: Some("26.05.00.0".into()), ..Default::default() }];
    let mut exe64 = inst("x64", "exe", None);
    exe64.silent = Some("/S".into());
    exe64.product_code = Some("Zipper".into());
    let mut exe86 = inst("x86", "exe", None);
    exe86.silent = Some("/S".into());
    let msix = inst("x64", "msix", None);
    let m = manifest(vec![msix, exe86, msi, exe64]);

    // Installed with the exe installer: the exe for its architecture, never the MSI.
    let w = Want::for_entry(&entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", "26.04", "Example Pavlov"), None);
    assert_eq!(choose(&m, &w).unwrap().url, "https://example.com/x64-exe");
    // Installed from the MSI: the MSI, found by its upgrade code, and the version it will show.
    let mut e = entry("{AAAA0000-0000-0000-0000-000000002301}", "HKLM64", "Zipper 23.01 (x64 edition)", "23.01.00.0", "Example Pavlov");
    e.windows_installer = true;
    let w = Want::for_entry(&e, Some("{AAAA0000-0000-0000-0000-00000000FFFF}"));
    let i = choose(&m, &w).unwrap();
    assert_eq!(i.kind, "wix");
    assert_eq!(expected_version(&m, i), "26.05.00.0");
    // A 32 bit copy gets the 32 bit installer.
    let w = Want::for_entry(&entry("Zipper", "HKLM32", "Zipper 26.04 (x86)", "26.04", "Example Pavlov"), None);
    assert_eq!(choose(&m, &w).unwrap().arch, "x86");
}

#[test]
fn refuses_installers_that_would_install_a_second_copy() {
    let user = entry("App", "HKCU", "App", "1.0", "Maker");
    let machine_only = manifest(vec![inst("x64", "inno", Some("machine"))]);
    assert!(choose(&machine_only, &Want::for_entry(&user, None)).unwrap_err().contains("per user"));
    let msi_only = manifest(vec![inst("x64", "msi", None)]);
    assert!(choose(&msi_only, &Want::for_entry(&user, None)).unwrap_err().contains("MSI"));
    let mut quiet_less = inst("x64", "exe", None);
    quiet_less.silent = None;
    assert_eq!(choose(&manifest(vec![quiet_less]), &Want::fresh()).unwrap_err(), "its installer has no silent mode");
    assert_eq!(choose(&manifest(vec![inst("x64", "portable", None)]), &Want::fresh()).unwrap_err(), "only a portable copy");
    let mut x86 = entry("App", "HKLM32", "App (x86)", "1.0", "Maker");
    x86.install_location = "C:\\Program Files (x86)\\App".into();
    assert!(choose(&manifest(vec![inst("x64", "nsis", None)]), &Want::for_entry(&x86, None)).is_err());
    // A new install takes this computer's architecture and installs for all users.
    let m = manifest(vec![inst("x86", "nsis", Some("user")), inst("x64", "nsis", Some("machine"))]);
    let i = choose(&m, &Want::fresh()).unwrap();
    assert_eq!((i.arch.as_str(), i.scope.as_deref()), ("x64", Some("machine")));
}

#[test]
fn a_32_bit_copy_in_its_own_folder_stays_32_bit() {
    // A 32 bit copy in its own folder (registered in the 32 bit view, name without an
    // architecture) is updated with the 32 bit installer, not given a second 64 bit copy.
    let mut x86 = entry("7-Zip", "HKLM32", "7-Zip 26.04", "26.04", "Igor Pavlov");
    x86.install_location = "D:\\Tools\\7-Zip".into();
    let mut e64 = inst("x64", "exe", None);
    e64.silent = Some("/S".into());
    let mut e86 = inst("x86", "exe", None);
    e86.silent = Some("/S".into());
    let m = manifest(vec![e64, e86]);
    assert_eq!(choose(&m, &Want::for_entry(&x86, None)).unwrap().arch, "x86");
}

#[test]
fn an_exe_without_a_silent_switch_is_not_run() {
    // An exe whose manifest only says "InstallModes: silent", with no switch, is not run.
    let mut boiler = inst("x64", "exe", None);
    boiler.install_modes = vec!["interactive".into(), "silent".into()];
    assert_eq!(choose(&manifest(vec![boiler]), &Want::fresh()).unwrap_err(), "its installer has no silent mode");
}

#[test]
fn the_admin_retry_does_not_uninstall_again() {
    let idx = index();
    let p = idx.by_id("Example.Zipper").unwrap();
    // A per-user copy whose installer removes the old one first: the admin retry only installs.
    let mut user = installed_from(entry("Zipper", "HKCU", "Zipper", "26.04", "Example Pavlov"), "Example.Zipper");
    user.uninstall = Some(vec!["C:\\Users\\Someone\\AppData\\Local\\Zipper\\uninstall.exe".into(), "/S".into()]);
    let mut nsis = inst("x64", "nsis", Some("user"));
    nsis.upgrade_behavior = Some("uninstallPrevious".into());
    let plan = build_plan(&p, &manifest(vec![nsis.clone()]), &nsis, JobKind::Update, Some(&user), None).unwrap();
    assert_eq!(plan.steps.len(), 2, "removes the old copy, then installs");
    let fb = plan.fallback.as_ref().expect("retry with admin rights");
    assert_eq!(fb.steps.len(), 1, "the retry does not run the old uninstaller again");
    assert!(fb.steps[0].elevated());
}

#[test]
fn msi_logs_are_cleaned_up_and_zip_paths_stay_inside() {
    let idx = index();
    let p = idx.by_id("Example.Zipper").unwrap();
    // The Windows Installer log goes once the update worked.
    let msi = inst("x64", "msi", None);
    let plan = build_plan(&p, &manifest(vec![msi.clone()]), &msi, JobKind::Install, None, None).unwrap();
    let Step::Run { log: Some(log), .. } = &plan.steps[0] else { panic!() };
    assert!(plan.cleanup.contains(log));

    // A rooted or absolute installer path in a zip is refused like "..".
    let mut zip = inst("x64", "zip", None);
    zip.nested_kind = Some("inno".into());
    for bad in ["C:\\Windows\\evil.exe", "\\evil.exe", "/evil.exe"] {
        zip.nested_path = Some(bad.into());
        assert!(build_plan(&p, &manifest(vec![zip.clone()]), &zip, JobKind::Install, None, None).is_err(), "{bad}");
    }
}

#[test]
fn compares_versions_carefully() {
    assert!(is_newer("26.05", "26.04"));
    assert!(is_newer("3.0.24", "3.0.20.0"));
    assert!(!is_newer("3.0.24", "3.0.24.0"));
    assert!(!is_newer("8.3", "2026.1"), "a year version is another numbering");
    assert!(!is_newer("2026.2", "8.3"));
    assert!(!is_newer("1.2", "Drv_3.00.0045"));
    assert!(is_newer("1.0.0", "1.0.0-beta.2"));
    assert!(is_newer("V10.3.3", "V10.3.2"));
}

#[test]
fn uninstalls_without_questions() {
    let mut e = entry("{ABCDEF00-0000-0000-0000-000000000000}", "HKLM64", "A", "1", "P");
    e.uninstall_string = "MsiExec.exe /X{ABCDEF00-0000-0000-0000-000000000000}".into();
    e.windows_installer = true;
    assert_eq!(uninstall_cmd(&e, None).unwrap(), vec!["msiexec.exe", "/x", "{ABCDEF00-0000-0000-0000-000000000000}", "/qn", "/norestart"]);
    let mut q = entry("X", "HKCU", "A", "1", "P");
    q.quiet_uninstall_string = "\"C:\\Users\\Someone\\AppData\\Local\\A\\Update.exe\" --uninstall -s".into();
    assert_eq!(uninstall_cmd(&q, None).unwrap(), vec!["C:\\Users\\Someone\\AppData\\Local\\A\\Update.exe", "--uninstall", "-s"]);
    let mut quiet_inno = entry("B_is1", "HKCU", "B", "1", "P");
    quiet_inno.quiet_uninstall_string = "\"C:\\Users\\Someone\\AppData\\Local\\Programs\\B\\unins000.exe\" /SILENT".into();
    assert_eq!(uninstall_cmd(&quiet_inno, None).unwrap(), vec!["C:\\Users\\Someone\\AppData\\Local\\Programs\\B\\unins000.exe", "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
    let mut inno = entry("A_is1", "HKLM32", "A", "1", "P");
    inno.uninstall_string = "\"C:\\Program Files\\A\\unins000.exe\"".into();
    assert_eq!(uninstall_cmd(&inno, None).unwrap(), vec!["C:\\Program Files\\A\\unins000.exe", "/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]);
    let nsis = entry("A", "HKLM64", "A", "1", "P");
    assert_eq!(uninstall_cmd(&nsis, Some("nsis")).unwrap(), vec!["C:\\Program Files\\A\\uninstall.exe", "/S", "_?=C:\\Program Files\\A"]);
    // An unknown uninstaller is never run blind.
    assert!(uninstall_cmd(&nsis, Some("exe")).is_none());
    assert!(uninstall_cmd(&nsis, None).is_none());
}

pub(super) fn installed_from(e: winutil::UninstallEntry, pkg: &str) -> Installed {
    Installed {
        id: format!("other:{pkg}"),
        source: Source::Other,
        name: "Zipper".into(),
        code: "Zi".into(),
        fg: String::new(),
        bg: String::new(),
        version: e.display_version.clone(),
        install_dir: None,
        exe: None,
        uninstall: uninstall_cmd(&e, Some("nsis")),
        updated_at: None,
        size_hint: None,
        tagline: None,
        channel: None,
        missing: false,
        read_only: false,
        store: None,
        icon_remote: None,
        data: SourceData::Other { pkg: pkg.into(), how: How::ProductCode, entry: Box::new(e), upgrade_code: None },
    }
}

#[test]
fn plans_silent_installs_by_type() {
    let idx = index();
    let p = idx.by_id("Example.Zipper").unwrap();
    let cur = installed_from(entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", "26.04", "Example Pavlov"), "Example.Zipper");

    let mut nsis = inst("x64", "nsis", None);
    nsis.url = "https://example.com/dl/zipper-26.05-x64.exe?from=site".into();
    nsis.custom = Some("/NOICONS".into());
    nsis.success_codes = vec![2];
    let m = manifest(vec![nsis.clone()]);
    let plan = build_plan(&p, &m, &nsis, JobKind::Update, Some(&cur), Some(1234)).unwrap();
    let Step::Run { exe, args, elevated, ok_codes, .. } = plan.steps.last().unwrap() else { panic!() };
    assert!(exe.ends_with("zipper-26.05-x64.exe"));
    assert_eq!(args, &vec!["/S".to_string(), "/NOICONS".into()]);
    assert!(*elevated, "a copy for all users needs admin rights");
    assert_eq!(ok_codes, &vec![0, 2]);
    assert_eq!(plan.downloads[0].sha256.as_deref(), Some("a".repeat(64).as_str()));
    assert!(matches!(&plan.verify, Verify::Version { app_id, version } if app_id == "other:Example.Zipper" && version == "26.05"));
    assert!(plan.fallback.is_none());

    // MSI through Windows Installer with a log; a per-user copy runs without admin rights first.
    let user = installed_from(entry("{AAAA0000-0000-0000-0000-000000002301}", "HKCU", "Zipper", "23.01", "Example Pavlov"), "Example.Zipper");
    let msi = inst("x64", "msi", None);
    let plan = build_plan(&p, &manifest(vec![msi.clone()]), &msi, JobKind::Update, Some(&user), None).unwrap();
    let Step::Run { exe, args, elevated, ok_codes, log, .. } = &plan.steps[0] else { panic!() };
    assert_eq!(exe, &PathBuf::from("msiexec.exe"));
    assert_eq!(&args[..1], &["/i".to_string()]);
    assert!(args.contains(&"/qn".to_string()) && args.contains(&"/l*".to_string()) && args.contains(&"MSIFASTINSTALL=7".to_string()) && log.is_some());
    assert!(!*elevated);
    assert!(ok_codes.contains(&3010));
    let fb = plan.fallback.as_ref().expect("retry with admin rights");
    assert!(fb.steps[0].elevated());

    // An installer inside a zip: unpacked first, then run by its own type.
    let mut zip = inst("x64", "zip", None);
    zip.nested_kind = Some("inno".into());
    zip.nested_path = Some("setup/zipper-setup.exe".into());
    let plan = build_plan(&p, &manifest(vec![zip.clone()]), &zip, JobKind::Install, None, None).unwrap();
    assert!(matches!(&plan.steps[0], Step::ExtractZip { .. }));
    let Step::Run { exe, args, .. } = &plan.steps[1] else { panic!() };
    assert!(exe.ends_with("setup\\zipper-setup.exe"));
    assert_eq!(args[0], "/VERYSILENT");
    // The zip, the unpacked folder and the Inno Setup log.
    assert_eq!(plan.cleanup.len(), 3);
    zip.nested_path = Some("..\\..\\evil.exe".into());
    assert!(build_plan(&p, &manifest(vec![zip.clone()]), &zip, JobKind::Install, None, None).is_err());
}

#[test]
fn only_a_refused_start_is_retried_with_admin_rights() {
    assert!(needs_elevation("Could not start C:\\x\\setup.exe: The requested operation requires elevation. (os error 740)"));
    assert!(!needs_elevation("VERIFY|The installer finished, but Zipper reports 26.04 instead of 26.05"));
    assert!(!needs_elevation("STEP|1603|Installing Zipper 26.05|exit code 1603||"));
}

#[test]
fn uninstall_plan_uses_the_registered_command() {
    let cur = installed_from(entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", "26.04", "Example Pavlov"), "Example.Zipper");
    let plan = plan_uninstall(&cur).unwrap();
    let Step::Run { exe, args, elevated, .. } = &plan.steps[0] else { panic!() };
    assert!(exe.ends_with("uninstall.exe"));
    assert_eq!(args[0], "/S");
    assert!(*elevated);
    assert!(matches!(plan.verify, Verify::Removed { .. }));
    // No silent command known (WinRAR, Brave, Docker Desktop were seen like this): its own
    // uninstaller still runs, in its window, instead of the app not being removable here.
    let mut loud = cur.clone();
    loud.uninstall = None;
    let plan = plan_uninstall(&loud).expect("its own uninstaller");
    let Step::Run { exe, .. } = &plan.steps[0] else { panic!() };
    assert!(exe.ends_with("uninstall.exe"));
    assert!(matches!(plan.verify, Verify::Removed { .. }));
    // An entry that names no uninstaller at all is not removable.
    let mut none = loud.clone();
    if let SourceData::Other { entry, .. } = &mut none.data {
        entry.uninstall_string.clear();
        entry.quiet_uninstall_string.clear();
    }
    assert!(plan_uninstall(&none).is_err());
}

#[test]
fn finds_github_repositories_and_tags() {
    assert_eq!(github_repo("https://github.com/ip7z/7zip/releases/download/26.05/7z2605-x64.exe"), Some(("ip7z".into(), "7zip".into())));
    assert_eq!(github_repo("https://github.com/microsoft"), None);
    assert_eq!(github_repo("https://example.com/a/b"), None);
    assert_eq!(github_tag("https://github.com/o/r/releases/download/v1.2.3/app.exe").as_deref(), Some("v1.2.3"));
    assert_eq!(github_tag("https://github.com/o/r/releases/tag/release%2F2.0").as_deref(), Some("release/2.0"));
    assert_eq!(github_raw("https://github.com/o/r/blob/main/docs/CHANGELOG.md").as_deref(), Some("https://raw.githubusercontent.com/o/r/main/docs/CHANGELOG.md"));
    let rels = vec![
        GhRel { tag: "v1.2.30".into(), name: String::new(), body: String::new(), url: "a".into() },
        GhRel { tag: "v1.2.3".into(), name: String::new(), body: String::new(), url: "b".into() },
        GhRel { tag: "nightly".into(), name: "Build 2.0".into(), body: String::new(), url: "c".into() },
    ];
    assert_eq!(find_release(&rels, "1.2.3", None).map(|r| r.url.as_str()), Some("b"));
    assert_eq!(find_release(&rels, "2.0", None).map(|r| r.url.as_str()), Some("c"));
    assert_eq!(find_release(&rels, "9.9", Some("nightly")).map(|r| r.url.as_str()), Some("c"));
    assert!(find_release(&rels, "9.9", None).is_none());
}

#[test]
fn file_names_and_marks() {
    let mut i = inst("x64", "nsis", None);
    i.url = "https://example.com/get?id=7".into();
    assert_eq!(file_name(&i, "Example.Zipper", "26.05"), "Example.Zipper-26.05.exe");
    i.url = "https://example.com/files/Zipper%20Setup%2026.05.exe".into();
    assert_eq!(file_name(&i, "Example.Zipper", "26.05"), "Zipper_Setup_26.05.exe");
    let (code, fg, bg) = mark_for("Media Player", "Example.Player");
    assert_eq!(code, "Mp");
    assert!(fg.starts_with('#') && bg.starts_with('#'));
    assert_eq!(mark_for("7-Zip", "7zip.7zip").0, "7z");
}

#[test]
fn the_epic_preview_is_not_claimed_as_the_old_launcher() {
    let dir = crate::paths::local_app_data().join("Epic Games").join("Launcher");
    let setup = dir.join("EpicGamesLauncherSetup.exe").display().to_string();
    let mut e = entry("{D0769F44-D459-450F-B084-CAE38062C75B}", "HKCU", "Epic Games Launcher", "0.97.0", "Epic Games, Inc.");
    e.install_location = dir.display().to_string();
    e.uninstall_string = format!("\"{setup}\" --uninstall");
    e.quiet_uninstall_string = format!("\"{setup}\" --silent --uninstall");
    let mut un = Vec::new();
    un.extend(crate::sources::tools::scan(&[e.clone()], std::path::Path::new("godot")));
    assert_eq!(un.len(), 1);
    assert!(handled_entry(&e, &un));
    // The old launcher lives in Program Files and stays an Other apps entry.
    let mut old = entry("{A1B2C3D4-0000-0000-0000-000000000000}", "HKLM32", "Epic Games Launcher", "1.3.150.0", "Epic Games, Inc.");
    old.install_location = r"C:\Program Files (x86)\Epic Games\Launcher".into();
    old.windows_installer = true;
    assert!(!handled_entry(&old, &un));
}

/// Firefox, Slack, PowerToys and Notion are also in the Store, so the catalog gives them a
/// package family name; they were dropped for that although they have normal installers.
#[test]
fn starter_apps_that_are_also_in_the_store_stay() {
    let idx = test_index();
    let fx = idx.add_test_package("Mozilla.Firefox", "Mozilla Firefox", "150.0", &[], &[], &["Mozilla Firefox (x64 en-US)"], "Mozilla");
    idx.add_test_package("Microsoft.PowerToys", "PowerToys", "0.101", &[], &[], &["PowerToys (Preview) x64"], "Microsoft Corporation");
    idx.add_test_pfn(fx, "Mozilla.Firefox_n80bbvh6b1yt2");
    assert!(idx.is_msix(fx));
    let got: Vec<String> = popular_candidates(&idx).into_iter().map(|(p, _)| p.id).collect();
    assert!(got.contains(&"Mozilla.Firefox".to_string()), "{got:?}");
    assert!(got.contains(&"Microsoft.PowerToys".to_string()), "{got:?}");
    // The manifest decides: a normal installer next to the MSIX is used, an MSIX alone is
    // installed as an app package when the catalog names its family.
    let m = manifest(vec![pkg_inst("x64", "msix", "Example.Zipper_8a0j1tnjnt4a4"), inst("x64", "nsis", None)]);
    assert_eq!(kind_of(choose(&m, &Want::fresh()).unwrap()), "nsis");
    let msix_only = manifest(vec![pkg_inst("x64", "msix", "Example.Zipper_8a0j1tnjnt4a4")]);
    assert_eq!(kind_of(choose(&msix_only, &Want::fresh()).unwrap()), "msix");
    let no_family = manifest(vec![inst("x64", "msix", None)]);
    assert_eq!(choose(&no_family, &Want::fresh()).unwrap_err(), "an MSIX package without its package family name");
    let store_only = manifest(vec![inst("x64", "msstore", None)]);
    assert_eq!(choose(&store_only, &Want::fresh()).unwrap_err(), STORE_ONLY);
}

#[test]
fn the_starter_list_is_clean() {
    let mut seen = std::collections::HashSet::new();
    for (id, line) in POPULAR {
        assert!(seen.insert(id.to_ascii_lowercase()), "{id} is listed twice");
        assert!(!handled_package(id), "{id} has a source of its own");
        assert!(excluded_package(id).is_none(), "{id} is excluded");
        assert!(!line.is_empty() && line.chars().count() <= 40, "{id}: description");
        assert!(!line.contains('\u{2013}') && !line.contains('\u{2014}'), "{id}: dash in the description");
    }
    assert!(POPULAR.len() >= 250);
}

#[test]
fn package_icon_files_are_named_by_id() {
    assert_eq!(super::super::icons::file_stem("Mozilla.Firefox"), "other-mozilla.firefox");
}

#[test]
fn a_copy_for_this_user_and_one_for_all_users_are_both_listed() {
    let idx = index();
    let mut machine = entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", "26.04", "Example Pavlov");
    machine.install_location = r"C:\Program Files\Zipper".into();
    let mut user = entry("Zipper", "HKCU", "Zipper 26.04 (x64)", "26.01", "Example Pavlov");
    user.install_location = r"C:\Users\A\AppData\Local\Programs\Zipper".into();
    // The same copy for all users in both registry views stays one copy.
    let mut again = machine.clone();
    again.hive = "HKLM32";
    // A copy for all users that also wrote an entry for this user (same folder) is that copy.
    let mut mirror = machine.clone();
    mirror.hive = "HKCU";
    let r = match_all(&[machine, again, mirror, user], &idx, &[], &HashMap::new());
    assert_eq!(r.matches.len(), 2, "{:?}", reasons(&r));
    assert_eq!(r.misses.iter().filter(|(_, m)| *m == Miss::Duplicate("Example.Zipper".into())).count(), 2);

    let list = installed_copies(&r.matches, None);
    let ids: Vec<&str> = list.iter().map(|i| i.id.as_str()).collect();
    assert_eq!(ids, vec!["other:Example.Zipper", "other:Example.Zipper:user"]);
    assert_eq!(list[0].name, "Zipper (all users)");
    assert_eq!(list[1].name, "Zipper (this user)");
    assert_eq!(list[1].version, "26.01");
    assert_eq!(Want::for_installed(&list[0]).scope, Some("machine"));
    assert_eq!(Want::for_installed(&list[1]).scope, Some("user"));

    // Each copy is updated by the installer for its own scope, verified by its own id, and
    // downloads into its own folder.
    let m = manifest(vec![inst("x64", "nsis", Some("machine")), inst("x64", "inno", Some("user"))]);
    assert_eq!(choose(&m, &Want::for_installed(&list[0])).unwrap().kind, "nsis");
    assert_eq!(choose(&m, &Want::for_installed(&list[1])).unwrap().kind, "inno");
    let p = idx.by_id("Example.Zipper").unwrap();
    let plans: Vec<Plan> = list.iter().map(|c| build_plan(&p, &m, choose(&m, &Want::for_installed(c)).unwrap(), JobKind::Update, Some(c), None).unwrap()).collect();
    assert_eq!(plans[0].app_id, "other:Example.Zipper");
    assert_eq!(plans[1].app_id, "other:Example.Zipper:user");
    assert!(matches!(&plans[1].verify, Verify::Version { app_id, .. } if app_id == "other:Example.Zipper:user"));
    assert!(plans[0].steps.last().unwrap().elevated());
    assert!(!plans[1].steps.last().unwrap().elevated());
    assert_ne!(plans[0].downloads[0].dest.parent(), plans[1].downloads[0].dest.parent());

    // Get apps shows the package once.
    let feed = Feed { pkgs: HashMap::from([("example.zipper".to_string(), PkgFeed { pkg: p.clone(), versions: vec![], latest: Some(m.clone()) })]), popular: vec![], unfinished: false };
    let cat = catalog_entries(&feed, &list);
    assert_eq!(cat.len(), 1);
    assert_eq!(cat[0].name, "Zipper");

    // A single copy for this user keeps the plain id.
    let mut solo = entry("Zipper", "HKCU", "Zipper 26.04 (x64)", "26.01", "Example Pavlov");
    solo.install_location = r"C:\Users\A\AppData\Local\Programs\Zipper".into();
    let r = match_all(&[solo], &idx, &[], &HashMap::new());
    let list = installed_copies(&r.matches, None);
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "other:Example.Zipper");
    assert_eq!(list[0].name, "Zipper");
}

/// Read-only check of every Other app matched on this PC: apps the catalog can not show after
/// matching, and updates that can never finish because the version the app writes to the
/// uninstall list can not equal the one offered (an update loop). Works on a copy of the
/// winget files in a temp folder, so the app's own data is not touched.
/// cargo test --release --lib live_version_loop_probe -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn live_version_loop_probe() {
    use crate::sources::{adobe, blender, craft, tools};
    let real = std::path::PathBuf::from(std::env::var("LOCALAPPDATA").unwrap());
    let pf = std::path::PathBuf::from(std::env::var("ProgramFiles").unwrap());
    let src = real.join(paths::APP_DIR).join("data").join("winget");
    let base = std::env::temp_dir().join("updater3d-loop-probe");
    let _ = std::fs::remove_dir_all(&base);
    let dst = base.join(paths::APP_DIR).join("data").join("winget");
    std::fs::create_dir_all(dst.join("files")).unwrap();
    std::fs::copy(src.join("index.db"), dst.join("index.db")).unwrap();
    for f in std::fs::read_dir(src.join("files")).unwrap().flatten() {
        let _ = std::fs::copy(f.path(), dst.join("files").join(f.file_name()));
    }
    std::env::set_var("LOCALAPPDATA", &base);
    let un = winutil::uninstall_entries();
    let mut handled = adobe::scan(None, &un);
    handled.extend(craft::scan(&un));
    handled.extend(blender::scan(&un, &pf.join("Blender Foundation")));
    handled.extend(tools::scan(&un, &real.join("Programs").join("Godot")));
    let idx = Index::open(&winget::index_path()).unwrap();
    let r = match_all(&un, &idx, &handled, &winutil::msi_upgrade_codes());
    let mut feed = Feed::default();
    for m in &r.matches {
        if let (p, Ok((versions, man))) = load_pkg(m.pkg.clone()).await {
            feed.pkgs.insert(p.id.to_ascii_lowercase(), PkgFeed { pkg: p, versions, latest: Some(man) });
        }
    }
    // The newest version's range of uninstall list versions, from the raw version list.
    let range = |p: &Pkg| -> Option<(String, String)> {
        let b = std::fs::read(winget::dir().join("files").join(&p.hash)).ok()?;
        let y = String::from_utf8(winget::mszip(&b).ok()?).ok()?;
        let d = yaml_rust2::YamlLoader::load_from_str(&y).ok()?.into_iter().next()?;
        let first = d["vD"].as_vec()?.iter().max_by(|a, b| crate::sources::tools::cmp_ver(&winget::text(&a["v"]).unwrap_or_default(), &winget::text(&b["v"]).unwrap_or_default()))?.clone();
        Some((winget::text(&first["aMiV"])?, winget::text(&first["aMaV"])?))
    };
    let mut hidden: Vec<String> = vec![];
    for m in &r.matches {
        if make(m, Some(&feed)).is_none() {
            let why = feed.get(&m.pkg.id).and_then(|f| f.latest.as_ref()).and_then(|man| choose(man, &Want::for_entry(&m.entry, m.upgrade_code.as_deref())).err()).unwrap_or_default();
            hidden.push(format!("{} {} [{}] {}", m.pkg.id, m.entry.display_version, m.entry.hive, why));
            // After an update that left it like this: does the check after the install find it,
            // and would the next check offer the same update again?
            let copies = copies_of(std::slice::from_ref(m), std::time::UNIX_EPOCH);
            if let (Some(c), Some(man)) = (copies.first(), feed.get(&m.pkg.id).and_then(|f| f.latest.as_ref())) {
                let declared = man.installers.iter().find_map(|i| i.scope.clone());
                let offered = man.installers.first().map(|i| expected_version(man, i)).unwrap_or_default();
                let x = Expect { app_id: c.id.clone(), pkg: m.pkg.id.clone(), pkg_version: man.version.clone(), expected: offered.clone(), declared_scope: declared, ..Default::default() };
                let check = check_install(&x, &copies);
                let mut l = Learned::default();
                if let Check::Done { shows, scope } = &check {
                    if let Some(v) = shows {
                        l.set_shows(&m.pkg.id, &man.version, v);
                    }
                    if let Some((a, b)) = scope {
                        l.set_scope(&m.pkg.id, a, b);
                    }
                }
                let listed = choose_with(man, &Want::for_entry(&m.entry, m.upgrade_code.as_deref()), &l).is_ok();
                let again = listed && is_newer(&offered, &c.version) && !at_version(&c.version, &offered, None, l.shows(&m.pkg.id, &man.version));
                println!("  after an update to {offered}: check {check:?}, listed again {listed}, offered again {again}");
            }
        }
    }
    let list = installed_copies(&r.matches, Some(&feed));
    let (mut offered, mut in_range, mut extra, mut shape) = (0, vec![], vec![], vec![]);
    for i in &list {
        let Some(pfeed) = feed.get(pkg_of(i)) else { continue };
        let Some(l) = latest(i, &feed) else { continue };
        if l.version == i.version {
            continue;
        }
        offered += 1;
        let (d, e) = (version::parts(&i.version), version::parts(&l.version));
        let line = format!("{} {} -> {}", pfeed.pkg.id, i.version, l.version);
        println!("  offered: {line}");
        if let Some((lo, hi)) = range(&pfeed.pkg) {
            if !version::newer(&lo, &i.version) && !version::newer(&i.version, &hi) {
                in_range.push(format!("{line} (newest writes {lo}..{hi})"));
                continue;
            }
        }
        if d.len() < e.len() && e[d.len()..].iter().any(|x| *x != 0) {
            extra.push(line);
        } else if i.version.trim().chars().next().is_some_and(|c| !c.is_ascii_digit()) != l.version.trim().chars().next().is_some_and(|c| !c.is_ascii_digit()) {
            shape.push(line);
        }
    }
    println!("matched {} entries, listed {}, hidden {}", r.matches.len(), list.len(), hidden.len());
    for h in &hidden {
        println!("  hidden: {h}");
    }
    println!("updates offered: {offered}");
    println!("loops, the newest version is already here (uninstall list range): {}", in_range.len());
    for l in &in_range {
        println!("  {l}");
    }
    println!("loops, the app writes fewer version parts than offered: {}", extra.len());
    for l in &extra {
        println!("  {l}");
    }
    println!("loops, prefix or format differs: {}", shape.len());
    for l in &shape {
        println!("  {l}");
    }
    let _ = std::fs::remove_dir_all(&base);
}

// MEGAsync 6.7.0.3 (owner's PC): the catalog says its installer is for all users, it installed
// for this user only, and it writes "6.7.0" to the uninstall list. The update was reported as
// failed ("reports not installed") and would have been offered again after every update.
#[test]
fn an_installer_that_really_installs_per_user_updates_the_per_user_copy_once_learned() {
    let mut i = inst("x64", "nsis", Some("machine"));
    i.product_code = Some("Zipper".into());
    let m = manifest(vec![i]);
    let mut e = entry("Zipper", "HKCU", "Zipper 26.04 (x64)", "26.04", "Example Pavlov");
    e.install_location = r"C:\Users\A\AppData\Local\Zipper".into();
    let w = Want::for_entry(&e, None);
    assert!(choose_with(&m, &w, &Learned::default()).is_err(), "the catalog alone rules it out");
    let mut l = Learned::default();
    l.set_scope("Example.Zipper", "machine", "user");
    assert!(choose_with(&m, &w, &l).is_ok());
    // Only installers declared with the scope that was wrong are corrected.
    let mut l2 = Learned::default();
    l2.set_scope("Example.Zipper", "user", "machine");
    assert!(choose_with(&m, &w, &l2).is_err());
}

#[test]
fn an_install_that_moved_the_app_or_shortened_its_version_counts_as_done_and_is_learned() {
    let x = Expect { app_id: "other:Mega.MEGASync".into(), pkg: "Mega.MEGASync".into(), pkg_version: "6.7.0.3".into(), expected: "6.7.0.3".into(), declared_scope: Some("machine".into()), before: Some("6.6.2".into()), ..Default::default() };
    let copy = |scope: &'static str, v: &str, rewritten: bool| PkgCopy { id: "other:Mega.MEGASync".into(), scope, version: v.into(), rewritten };
    assert_eq!(check_install(&x, &[copy("user", "6.7.0", true)]), Check::Done { shows: Some("6.7.0".into()), scope: Some(("machine".into(), "user".into())) });
    assert_eq!(check_install(&x, &[copy("machine", "6.7.0.3", true)]), Check::Done { shows: None, scope: None });
    assert_eq!(check_install(&x, &[]), Check::Missing);
    // The old copy, untouched, or an unrelated version: not done.
    assert_eq!(check_install(&x, &[copy("machine", "6.6.2", false)]), Check::Shows("6.6.2".into()));
    assert_eq!(check_install(&x, &[copy("machine", "6.7", true)]), Check::Done { shows: Some("6.7".into()), scope: None });
    assert_eq!(check_install(&x, &[copy("machine", "6.8.0", true)]), Check::Done { shows: None, scope: None });
    assert_eq!(check_install(&x, &[copy("machine", "5.0", true)]), Check::Shows("5.0".into()));
    // Shortened but neither rewritten nor changed: the installer may have done nothing.
    let same = Expect { before: Some("6.7.0".into()), ..x.clone() };
    assert_eq!(check_install(&same, &[copy("machine", "6.7.0", false)]), Check::Shows("6.7.0".into()));
    // The copy is gone and one with another id was written: that is it. While the old copy is
    // still there it is the one checked (a second copy is not an update of it).
    let other = PkgCopy { id: "other:Mega.MEGASync:user".into(), ..copy("user", "6.7.0", true) };
    assert!(matches!(check_install(&x, std::slice::from_ref(&other)), Check::Done { .. }));
    assert_eq!(check_install(&x, &[copy("machine", "6.6.2", false), other]), Check::Shows("6.6.2".into()));
    // What was learned keeps the next check from offering the same update again.
    assert!(!at_version("6.7.0", "6.7.0.3", None, None));
    assert!(at_version("6.7.0", "6.7.0.3", None, Some("6.7.0")));
}

#[test]
fn an_app_already_in_the_newest_versions_uninstall_list_range_is_not_offered_it() {
    let idx = index();
    let p = idx.by_id("Example.Zipper").unwrap();
    let mut i = inst("x64", "nsis", None);
    i.product_code = Some("Zipper".into());
    let m = manifest(vec![i]);
    let versions = vec![VerRef { version: "26.05".into(), path: "p".into(), sha256: "s".into(), arp_min: Some("26.4.90".into()), arp_max: Some("26.4.99".into()) }];
    let feed = Feed { pkgs: HashMap::from([("example.zipper".to_string(), PkgFeed { pkg: p, versions, latest: Some(m) })]), popular: vec![], unfinished: false };
    let at = |v: &str| {
        let r = match_all(&[entry("Zipper", "HKLM64", "Zipper 26.04 (x64)", v, "Example Pavlov")], &idx, &[], &HashMap::new());
        let list = installed_copies(&r.matches, Some(&feed));
        latest(&list[0], &feed).unwrap().version
    };
    assert_eq!(at("26.4.95"), "26.4.95", "26.05 writes 26.4.90 to 26.4.99: already there");
    assert_eq!(at("26.4.80"), "26.05");
}

/// A notes site that never answers must not keep the version list from showing: the list
/// comes back after the notes time limit with a link to the notes (it used to wait up to 90 s
/// per version, one version after another).
#[tokio::test]
async fn a_slow_notes_site_does_not_hold_up_the_version_list() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // Accepts the connection and never answers.
    std::thread::spawn(move || {
        let held: Vec<_> = listener.incoming().take(4).filter_map(|c| c.ok()).collect();
        std::thread::sleep(std::time::Duration::from_secs(120));
        drop(held);
    });
    let url = format!("http://127.0.0.1:{port}/notes");
    let m = winget::Manifest { id: "Slow.Notes".into(), version: "2.0".into(), name: "Slow".into(), release_notes_url: Some(url.clone()), ..Default::default() };
    let list = vec![winget::VerRef { version: "2.0".into(), path: String::new(), sha256: String::new(), arp_min: None, arp_max: None }];
    let t0 = std::time::Instant::now();
    let infos = version_infos(&list, None, &Want::fresh(), Some(&m)).await;
    assert!(t0.elapsed() < NOTES_WAIT + std::time::Duration::from_secs(4), "took {:?}", t0.elapsed());
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].notes_url.as_deref(), Some(url.as_str()));
}

#[test]
fn inno_installers_write_a_log_the_job_can_read() {
    let idx = index();
    let p = idx.by_id("Example.Zipper").unwrap();
    let inno = inst("x64", "inno", None);
    let plan = build_plan(&p, &manifest(vec![inno.clone()]), &inno, JobKind::Install, None, None).unwrap();
    let Step::Run { args, log: Some(log), .. } = &plan.steps[0] else { panic!("an Inno install has a log") };
    assert_eq!(&args[..4], &["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"]);
    assert_eq!(args.last().unwrap(), &format!("/LOG={}", log.display()));
    assert!(plan.cleanup.contains(log));
    // The catalog's own switches stay as they are; only the log is added.
    let mut own = inst("x64", "inno", None);
    own.silent = Some(r#"/SILENT /DIR="C:\Apps\Zipper""#.into());
    let plan = build_plan(&p, &manifest(vec![own.clone()]), &own, JobKind::Install, None, None).unwrap();
    let Step::Run { args, log: Some(log), .. } = &plan.steps[0] else { panic!() };
    assert_eq!(args, &vec!["/SILENT".to_string(), r"/DIR=C:\Apps\Zipper".to_string(), format!("/LOG={}", log.display())]);
    // A log the catalog asks for is kept, and not read as ours.
    own.silent = Some(r"/VERYSILENT /LOG=C:\x.log".into());
    let plan = build_plan(&p, &manifest(vec![own.clone()]), &own, JobKind::Install, None, None).unwrap();
    let Step::Run { args, log, .. } = &plan.steps[0] else { panic!() };
    assert_eq!(args, &vec!["/VERYSILENT".to_string(), r"/LOG=C:\x.log".to_string()]);
    assert!(log.is_none());
    // Other installer types get no log switch.
    let nsis = inst("x64", "nsis", None);
    let plan = build_plan(&p, &manifest(vec![nsis.clone()]), &nsis, JobKind::Install, None, None).unwrap();
    let Step::Run { args, log, .. } = &plan.steps[0] else { panic!() };
    assert!(log.is_none() && !args.iter().any(|a| a.to_ascii_lowercase().starts_with("/log")));
}

#[test]
fn a_check_reads_again_only_the_packages_whose_version_list_changed() {
    let row = |id: &str, hash: &str, latest: &str| Pkg { row: 1, id: id.into(), name: id.into(), moniker: None, latest: latest.into(), arp_min: None, arp_max: None, hash: hash.into() };
    let read = |p: Pkg, v: &str| PkgFeed { versions: vec![VerRef { version: v.into(), path: format!("manifests/{v}"), sha256: "ab".repeat(32), arp_min: None, arp_max: None }], latest: Some(Manifest { id: p.id.clone(), version: v.into(), ..Default::default() }), pkg: p };
    let mut prev = Feed::default();
    prev.pkgs.insert("7zip.7zip".into(), read(row("7zip.7zip", "aaa", "26.0"), "26.0"));
    prev.popular.push((read(row("Mozilla.Firefox", "fff", "140.0"), "140.0"), "Web browser"));
    let before = read_before(Some(&prev));
    // Same version list: kept as read, with the new index row (its name or range may change).
    let mut now = row("7zip.7zip", "aaa", "26.0");
    now.name = "7-Zip".into();
    let (p, r) = reuse(&before, &now).expect("unchanged package is kept");
    assert_eq!(p.name, "7-Zip");
    assert_eq!(r.unwrap().1.version, "26.0");
    // Popular apps are kept too.
    assert!(reuse(&before, &row("mozilla.firefox", "fff", "140.0")).is_some());
    // A new version list (a new version came out) or an unknown package: read again.
    assert!(reuse(&before, &row("7zip.7zip", "bbb", "26.1")).is_none());
    assert!(reuse(&before, &row("Git.Git", "ggg", "2.0")).is_none());
    // No checksum, or a package read without its manifest: read again.
    assert!(reuse(&before, &row("7zip.7zip", "", "26.0")).is_none());
    let mut half = read(row("Git.Git", "ggg", "2.0"), "2.0");
    half.latest = None;
    prev.pkgs.insert("git.git".into(), half);
    assert!(reuse(&read_before(Some(&prev)), &row("Git.Git", "ggg", "2.0")).is_none());
    assert!(reuse(&read_before(None), &now).is_none());
}

#[test]
fn a_github_release_page_gives_its_release_text() {
    // Seen with WinMerge, Notepad++ and PrusaSlicer: the catalog links the release page, and
    // GitHub's API was out of requests, so only a link showed.
    let html = r#"<html><body><nav>Sign in</nav><div class="Box-body"><div data-pjax="true" data-test-selector="body-content" data-view-component="true" class="markdown-body my-3"><ul><li><a href="/c">What's Changed</a></li></ul><h2>What's Changed</h2>
        <ul><li>Fix a crash when comparing large folders (<a href="/x/pull/1">#2715</a>)</li><li>Add Japanese translation updates for the new options</li></ul>
        <h2>New Contributors</h2><ul><li>@someone made their first contribution in the release</li></ul></div></div>
        <details><summary>Assets 4</summary><ul><li>WinMerge-2.16.58-x64-Setup.exe 9.1 MB</li></ul></details></body></html>"#;
    let lines = github_release_body(html);
    // "New Contributors" is not a change, and a lone "What's Changed" heading says nothing.
    assert_eq!(lines, vec!["Fix a crash when comparing large folders".to_string(), "Add Japanese translation updates for the new options".to_string()]);
    assert!(lines.iter().any(|l| l == "Fix a crash when comparing large folders"), "{lines:?}");
    assert!(!lines.iter().any(|l| l.contains("Setup.exe") || l.contains("Sign in")), "assets and page chrome stay out: {lines:?}");
    assert!(github_release_body("<html>no release here</html>").is_empty());
}
