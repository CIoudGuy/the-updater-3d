use super::*;
use crate::sources::winget::{test_index, ArpEntry};

fn entry(key: &str, hive: &'static str, name: &str, ver: &str, publisher: &str) -> winutil::UninstallEntry {
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
        icon_remote: None,
        data: SourceData::Tools { key: "zipper".into(), series: String::new() },
    };
    let r = match_all(&[blender, adobe, zipper], &idx, &[listed], &HashMap::new());
    assert!(r.matches.is_empty());
    assert!(r.misses.iter().all(|(_, m)| *m == Miss::Handled));
    assert!(handled_package("BlenderFoundation.Blender.LTS.4.5") && handled_package("Unity.Unity.6000") && !handled_package("Unity.UnityHub"));
    assert_eq!(excluded_package("Microsoft.VisualStudio.2022.Community"), Some("updated by the Visual Studio Installer"));
    assert_eq!(excluded_package("Microsoft.VisualStudioCode"), None);
}

fn inst(arch: &str, kind: &str, scope: Option<&str>) -> Installer {
    Installer { arch: arch.into(), kind: kind.into(), url: format!("https://example.com/{arch}-{kind}"), sha256: "a".repeat(64), scope: scope.map(String::from), ..Default::default() }
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

fn installed_from(e: winutil::UninstallEntry, pkg: &str) -> Installed {
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
    assert!(args.contains(&"/qn".to_string()) && args.contains(&"/l*v".to_string()) && log.is_some());
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
    assert_eq!(plan.cleanup.len(), 2);
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
    let mut none = cur.clone();
    none.uninstall = None;
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
