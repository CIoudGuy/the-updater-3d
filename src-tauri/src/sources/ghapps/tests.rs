use super::*;
use serde_json::json;

const SHA_A: &str = "13c8ace0a3ddb33c1b720a4378dfa5c837c8bb3e2f55226bcfb8895c63fd1b8a";
const SHA_B: &str = "ee659b567c95b9a524ceddcb20560fc56ebbbe6f466fbeab0285f803a6165a31";

/// Tidalfast's entry as its Inno Setup installer writes it (a copy for this user).
fn tidalfast() -> UninstallEntry {
    UninstallEntry {
        key: "{2BC3C20F-7CC4-4BD0-A51C-0A5415B0189B}_is1".into(),
        hive: "HKCU",
        display_name: "Tidalfast 1.0.2-alpha.3".into(),
        display_version: "1.0.2-alpha.3".into(),
        publisher: "Tidalfast contributors".into(),
        install_location: r"C:\Users\Someone\AppData\Local\Programs\Tidalfast\".into(),
        uninstall_string: r#""C:\Users\Someone\AppData\Local\Programs\Tidalfast\unins000.exe""#.into(),
        quiet_uninstall_string: r#""C:\Users\Someone\AppData\Local\Programs\Tidalfast\unins000.exe" /SILENT"#.into(),
        url_info_about: "https://cioudguy.github.io/tidalfast".into(),
        help_link: "https://github.com/CIoudGuy/tidalfast/issues".into(),
        url_update_info: "https://cioudguy.github.io/tidalfast/download/".into(),
        ..Default::default()
    }
}

fn asset(name: &str, sha: Option<&str>) -> Value {
    json!({
        "name": name,
        "size": 14356377,
        "digest": sha.map(|s| format!("sha256:{s}")),
        "browser_download_url": format!("https://github.com/CIoudGuy/tidalfast/releases/download/v/{name}"),
    })
}

fn release(tag: &str, pre: bool, assets: Vec<Value>) -> Value {
    json!({
        "tag_name": tag,
        "draft": false,
        "prerelease": pre,
        "html_url": format!("https://github.com/CIoudGuy/tidalfast/releases/tag/{tag}"),
        "published_at": "2026-10-22T22:34:16Z",
        "body": "**New**\r\n- **Plugins work like in TidaLuna.** More plugins and themes match TIDAL.\r\n- Playback is steadier.",
        "assets": assets,
    })
}

/// What GitHub answers for Tidalfast: two releases with a setup, a zip and Linux builds.
fn answer() -> Value {
    let files = |v: &str, sha: &str| {
        vec![
            asset(&format!("tidalfast-{v}-x86_64-pc-windows-msvc-setup.exe"), Some(sha)),
            asset(&format!("tidalfast-{v}-x86_64-pc-windows-msvc.zip"), Some(sha)),
            asset(&format!("tidalfast-{v}-x86_64.AppImage"), Some(sha)),
            asset(&format!("tidalfast-{v}-x86_64.deb"), Some(sha)),
        ]
    };
    json!([release("v1.0.1", false, files("v1.0.1", SHA_B)), release("v1.1.0", false, files("v1.1.0", SHA_A))])
}

fn program(e: &UninstallEntry) -> Installed {
    crate::uninstall::programs::make(e, None, Some(PathBuf::from(r"C:\Users\Someone\AppData\Local\Programs\Tidalfast")), &|_| true, None)
}

fn feed_for(i: &Installed, kind: Kind, v: &Value) -> Feed {
    let mut f = Feed::default();
    f.apps.insert(i.id.clone(), Tracked { repo: "CIoudGuy/tidalfast".into(), kind });
    f.repos.insert("cioudguy/tidalfast".into(), parse_releases(v));
    f
}

#[test]
fn a_program_names_its_github_project_in_its_entry() {
    assert_eq!(repo_of(&tidalfast()), Some(("CIoudGuy".into(), "tidalfast".into())));
    // Only its project's site on GitHub Pages.
    let e = UninstallEntry { help_link: String::new(), ..tidalfast() };
    assert_eq!(repo_of(&e), Some(("cioudguy".into(), "tidalfast".into())));
    // A name that differs a little from the project's.
    let tool = UninstallEntry { display_name: "PhotoSortr".into(), url_info_about: "https://github.com/someone/PhotoSort".into(), ..Default::default() };
    assert_eq!(repo_of(&tool), Some(("someone".into(), "PhotoSort".into())));
    // TidaLunar links the mod it is built on; its own project is a fork.
    let luna = UninstallEntry { display_name: "TidaLunar".into(), url_info_about: "https://github.com/Inrixia/TidaLuna".into(), ..Default::default() };
    assert_eq!(repo_of(&luna), Some(("Brskt".into(), "TidaLuna".into())));
}

#[test]
fn a_link_to_another_project_is_not_the_programs_own() {
    // A tool that links the library it is built on: its releases are not the tool's.
    let e = UninstallEntry { display_name: "Photo Sorter 2.1".into(), url_info_about: "https://github.com/electron/electron".into(), ..Default::default() };
    assert_eq!(repo_of(&e), None);
    // A user's own site (no project in the address) and odd names name nothing.
    let e = UninstallEntry { display_name: "Tidalfast".into(), url_info_about: "https://cioudguy.github.io/".into(), help_link: "https://github.com/CIoud%2FGuy/tidalfast".into(), ..Default::default() };
    assert_eq!(repo_of(&e), None);
    assert_eq!(repo_of(&UninstallEntry { display_name: "Tidalfast".into(), ..Default::default() }), None);
}

#[test]
fn releases_of_a_sibling_project_are_not_the_programs() {
    // TidaLunar 0.0.18-alpha links TidaLuna, whose releases are its 1.17 plugin builds.
    let luna = parse_releases(&json!([release("1.17.1-beta", false, vec![]), release("1.17.0-beta", false, vec![])]));
    assert!(!fits("0.0.18-alpha", &luna));
    // Tidalfast's own pre-release, which is not on GitHub, is of the same series.
    assert!(fits("1.0.2-alpha.3", &parse_releases(&answer())));
    // The installed version among them, and versions numbered by year.
    assert!(fits("0.4.1", &parse_releases(&json!([release("v0.5.0", false, vec![]), release("v0.4.1", false, vec![])]))));
    assert!(fits("2024.10", &parse_releases(&json!([release("2026.1", false, vec![])]))));
    assert!(!fits("2.0", &parse_releases(&json!([release("v3.0.0", false, vec![])]))));
}

/// TidaLunar's releases: every normal one named "-alpha", its dev builds flagged as pre-releases.
fn tidalunar_answer() -> Value {
    let files = |v: &str| vec![asset(&format!("tidalunar_{v}_x64.exe"), Some(SHA_A)), asset(&format!("tidalunar_{v}_arm64.exe"), Some(SHA_B)), asset(&format!("tidalunar_{v}_win32_x64.zip"), Some(SHA_A))];
    json!([release("0.0.19-alpha", false, files("0.0.19-alpha")), release("0.0.19-alpha.dev.3", true, files("0.0.19-alpha.dev.3")), release("0.0.18-alpha", false, files("0.0.18-alpha")), release("0.0.18-alpha.dev.4", true, files("0.0.18-alpha.dev.4"))])
}

#[test]
fn a_project_that_flags_its_dev_builds_offers_its_normal_releases() {
    let rels = parse_releases(&tidalunar_answer());
    // Its normal releases are all named "-alpha": only GitHub's flag tells a dev build.
    assert_eq!(newest(&rels, "0.0.18-alpha").map(|r| r.version.as_str()), Some("0.0.19-alpha"));
    assert!(fits("0.0.18-alpha", &rels));
    let r = newest(&rels, "0.0.18-alpha").unwrap();
    assert_eq!(installer_for(r, Kind::Nsis, "x64").map(|a| a.name.as_str()), Some("tidalunar_0.0.19-alpha_x64.exe"));
    let e = UninstallEntry { key: "TidaLunar".into(), hive: "HKCU", display_name: "TidaLunar".into(), display_version: "0.0.18-alpha".into(), url_info_about: "https://github.com/Inrixia/TidaLuna".into(), ..Default::default() };
    let i = program(&e);
    let mut f = Feed::default();
    f.apps.insert(i.id.clone(), Tracked { repo: "Brskt/TidaLuna".into(), kind: Kind::Nsis });
    f.repos.insert("brskt/tidaluna".into(), rels.clone());
    let list = versions(&i, &f);
    // Dev builds older than the newest normal release are left out; the normal ones are not pre-releases.
    assert_eq!(list.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["0.0.19-alpha", "0.0.18-alpha"]);
    assert!(list.iter().all(|v| !v.prerelease));
    assert!(list[0].latest && list[1].installed);
    // A copy on a dev build moves on to the next normal release too.
    assert_eq!(newest(&rels, "0.0.19-alpha.dev.3").map(|r| r.version.as_str()), Some("0.0.19-alpha"));
}

#[test]
fn a_tag_names_its_version() {
    assert_eq!(version_of_tag("v1.1.0").as_deref(), Some("1.1.0"));
    assert_eq!(version_of_tag("tidalfast-v1.1.0").as_deref(), Some("1.1.0"));
    assert_eq!(version_of_tag("2.0.0-beta.2").as_deref(), Some("2.0.0-beta.2"));
    assert_eq!(version_of_tag("nightly"), None);
}

#[test]
fn releases_are_read_newest_first_without_drafts_or_foreign_files() {
    let mut v = answer();
    v.as_array_mut().unwrap().push(json!({ "tag_name": "v9.0.0", "draft": true, "assets": [] }));
    v[0]["assets"].as_array_mut().unwrap().push(json!({ "name": "evil-setup.exe", "browser_download_url": "https://example.com/evil-setup.exe", "digest": format!("sha256:{SHA_A}") }));
    v[0]["assets"].as_array_mut().unwrap().push(json!({ "name": "short-setup.exe", "browser_download_url": "https://github.com/CIoudGuy/tidalfast/releases/download/v1.0.1/short-setup.exe", "digest": "sha256:abc" }));
    let rels = parse_releases(&v);
    assert_eq!(rels.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), ["1.1.0", "1.0.1"]);
    let old = &rels[1];
    assert!(!old.assets.iter().any(|a| a.name == "evil-setup.exe"));
    // A digest that is not a SHA-256 is no checksum.
    assert_eq!(old.assets.iter().find(|a| a.name == "short-setup.exe").unwrap().sha256, None);
    assert_eq!(rels[0].assets[0].sha256.as_deref(), Some(SHA_A));
    assert_eq!(rels[0].url, "https://github.com/CIoudGuy/tidalfast/releases/tag/v1.1.0");
}

#[test]
fn a_pre_release_copy_moves_to_the_newer_final_release() {
    let rels = parse_releases(&answer());
    assert_eq!(newest(&rels, "1.0.2-alpha.3").map(|r| r.version.as_str()), Some("1.1.0"));
    // A final copy is never offered a pre-release; a pre-release copy is.
    let mut v = answer();
    v.as_array_mut().unwrap().push(release("v1.2.0-beta.1", true, vec![]));
    let rels = parse_releases(&v);
    assert_eq!(newest(&rels, "1.1.0").map(|r| r.version.as_str()), Some("1.1.0"));
    assert_eq!(newest(&rels, "1.0.2-alpha.3").map(|r| r.version.as_str()), Some("1.2.0-beta.1"));
}

#[test]
fn an_old_tag_numbered_another_way_is_not_the_newest() {
    // ninja's releases: its 1.13 line and a 2012 tag named by date.
    let at = |tag: &str, date: &str| {
        let mut r = release(tag, false, vec![]);
        r["published_at"] = json!(date);
        r
    };
    let rels = parse_releases(&json!([at("v1.13.2", "2025-11-20T10:00:00Z"), at("v1.13.1", "2025-07-10T10:00:00Z"), at("release-120715", "2012-07-15T10:00:00Z")]));
    assert_eq!(newest(&rels, "1.13.1").map(|r| r.version.as_str()), Some("1.13.2"));
    let i = program(&UninstallEntry { display_name: "ninja".into(), display_version: "1.13.1".into(), ..tidalfast() });
    let f = feed_for(&i, Kind::Inno, &json!([at("v1.13.2", "2025-11-20T10:00:00Z"), at("v1.13.1", "2025-07-10T10:00:00Z"), at("release-120715", "2012-07-15T10:00:00Z")]));
    assert_eq!(latest(&i, &f).map(|l| l.version), Some("1.13.2".into()));
    assert_eq!(versions(&i, &f).iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["1.13.2", "1.13.1"]);
    // A new first number, once it is out, is the newest.
    let rels = parse_releases(&json!([at("v2.0.0", "2026-01-05T10:00:00Z"), at("v1.13.2", "2025-11-20T10:00:00Z")]));
    assert_eq!(newest(&rels, "1.13.2").map(|r| r.version.as_str()), Some("2.0.0"));
}

#[test]
fn the_setup_of_the_installed_type_for_this_processor_is_picked() {
    let rels = parse_releases(&answer());
    let r = &rels[0];
    assert_eq!(installer_for(r, Kind::Inno, "x64").map(|a| a.name.as_str()), Some("tidalfast-v1.1.0-x86_64-pc-windows-msvc-setup.exe"));
    // No ARM64 build, no MSI, and an uninstaller of unknown type: nothing to run silently.
    assert_eq!(installer_for(r, Kind::Inno, "arm64"), None);
    assert_eq!(installer_for(r, Kind::Msi, "x64"), None);
    assert_eq!(installer_for(r, Kind::Unknown, "x64"), None);
    assert_eq!(installer_for(r, Kind::Squirrel, "x64"), None);
}

#[test]
fn an_installer_without_a_checksum_or_one_of_several_is_not_run() {
    let no_sum = parse_releases(&json!([release("v2.0.0", false, vec![asset("app-2.0.0-setup.exe", None)])]));
    assert_eq!(installer_for(&no_sum[0], Kind::Nsis, "x64"), None);
    let two = parse_releases(&json!([release("v2.0.0", false, vec![asset("app-2.0.0-setup.exe", Some(SHA_A)), asset("app-2.0.0-full-setup.exe", Some(SHA_B))])]));
    assert_eq!(installer_for(&two[0], Kind::Nsis, "x64"), None);
    // The build for this processor over one that does not say; 32-bit and portable ones are not it.
    let mixed = parse_releases(&json!([release("v2.0.0", false, vec![asset("app-setup.exe", Some(SHA_A)), asset("app-setup-x64.exe", Some(SHA_B)), asset("app-setup-win32.exe", Some(SHA_A)), asset("app-portable.exe", Some(SHA_A))])]));
    assert_eq!(installer_for(&mixed[0], Kind::Nsis, "x64").map(|a| a.name.as_str()), Some("app-setup-x64.exe"));
    let msi = parse_releases(&json!([release("v2.0.0", false, vec![asset("App-2.0.0-x64.msi", Some(SHA_A)), asset("App-2.0.0-arm64.msi", Some(SHA_B))])]));
    assert_eq!(installer_for(&msi[0], Kind::Msi, "arm64").map(|a| a.name.as_str()), Some("App-2.0.0-arm64.msi"));
}

#[test]
fn a_program_from_github_gets_its_newest_version_and_history() {
    let i = program(&tidalfast());
    let f = feed_for(&i, Kind::Inno, &answer());
    let l = latest(&i, &f).unwrap();
    assert_eq!(l.version, "1.1.0");
    assert_eq!(l.size, Some(14356377));
    assert_eq!(manual_page(&i, &f), None);
    assert!(can_roll_back(&i, &f));
    let list = versions(&i, &f);
    assert_eq!(list.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["1.1.0", "1.0.1"]);
    assert!(list[0].latest && !list[1].latest);
    assert!(list[0].notes.iter().any(|n| n.contains("Plugins work like in TidaLuna")), "{:?}", list[0].notes);
    assert_eq!(list[0].notes_url.as_deref(), Some("https://github.com/CIoudGuy/tidalfast/releases/tag/v1.1.0"));
    // Another program is not this project's.
    let other = program(&UninstallEntry { key: "Other".into(), display_name: "Other".into(), ..tidalfast() });
    assert!(latest(&other, &f).is_none() && versions(&other, &f).is_empty());
}

#[test]
fn a_release_with_no_installer_to_run_opens_its_page() {
    let i = program(&tidalfast());
    // An uninstaller of unknown type: the newest version shows, its page has the download.
    let f = feed_for(&i, Kind::Unknown, &answer());
    assert_eq!(latest(&i, &f).map(|l| l.version), Some("1.1.0".into()));
    assert_eq!(manual_page(&i, &f).as_deref(), Some("https://github.com/CIoudGuy/tidalfast/releases/tag/v1.1.0"));
    assert!(!can_roll_back(&i, &f));
}

#[test]
fn older_pre_releases_are_left_out_of_the_history() {
    let i = program(&UninstallEntry { display_version: "1.1.0".into(), display_name: "Tidalfast".into(), ..tidalfast() });
    let mut v = answer();
    v.as_array_mut().unwrap().push(release("v1.1.0-rc.1", true, vec![]));
    v.as_array_mut().unwrap().push(release("v1.2.0-beta.1", true, vec![]));
    let f = feed_for(&i, Kind::Inno, &v);
    let list = versions(&i, &f);
    assert_eq!(list.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["1.2.0-beta.1", "1.1.0", "1.0.1"]);
    assert!(list[0].prerelease && !list[0].latest);
    assert!(list[1].installed && list[1].latest);
}

#[test]
fn the_update_runs_the_setup_silently_for_this_user_with_its_checksum() {
    let i = program(&tidalfast());
    let f = feed_for(&i, Kind::Inno, &answer());
    let p = plan_install(&i, None, JobKind::Update, Some(&f)).unwrap();
    assert_eq!(p.app_id, i.id);
    assert_eq!(p.to_version, "1.1.0");
    let d = &p.downloads[0];
    assert!(d.url.starts_with("https://github.com/CIoudGuy/tidalfast/releases/download/"));
    assert_eq!(d.sha256.as_deref(), Some(SHA_A));
    let Step::Run { exe, args, elevated, ok_codes, .. } = &p.steps[0] else { panic!("{:?}", p.steps) };
    assert_eq!(exe, &d.dest);
    assert!(!elevated, "a copy for this user installs without admin rights");
    assert_eq!(&args[..4], ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/SP-"]);
    assert_eq!(ok_codes, &[0]);
    assert!(matches!(&p.verify, Verify::Version { app_id, version } if app_id == &i.id && version == "1.1.0"));
    let Some(fb) = &p.fallback else { panic!("no second try with admin rights") };
    assert!(fb.steps[0].elevated());
    assert!(p.cleanup.contains(&d.dest));
    // An older release, asked for by version.
    let back = plan_install(&i, Some("1.0.1"), JobKind::Rollback, Some(&f)).unwrap();
    assert_eq!(back.downloads[0].sha256.as_deref(), Some(SHA_B));
    assert!(plan_install(&i, Some("0.9.0"), JobKind::Rollback, Some(&f)).is_err());
    assert!(plan_install(&i, None, JobKind::Update, None).is_err());
}

#[test]
fn a_copy_for_all_users_installs_with_admin_rights() {
    let e = UninstallEntry { hive: "HKLM64", uninstall_string: r#""C:\Program Files\Tidalfast\unins000.exe""#.into(), ..tidalfast() };
    let i = program(&e);
    let f = feed_for(&i, Kind::Inno, &answer());
    let p = plan_install(&i, None, JobKind::Update, Some(&f)).unwrap();
    assert!(p.steps[0].elevated());
    assert!(p.fallback.is_none());
    // The plan reads the uninstaller again: one of unknown type that is not on the PC runs nothing.
    let e = UninstallEntry { key: "Tidalfast".into(), ..e };
    let i = program(&e);
    let f = feed_for(&i, Kind::Nsis, &answer());
    assert!(plan_install(&i, None, JobKind::Update, Some(&f)).is_err());
}

/// What this PC's programs would get: `U3D_ENTRIES` is a JSON list of uninstall entries (key,
/// hive, name, version, about, help, update, uninstall) and `U3D_GH_DIR` holds each project's
/// saved release list as "owner_repo.json" in lowercase.
#[test]
#[ignore]
fn live_this_pc() {
    let (Ok(entries), Ok(dir)) = (std::env::var("U3D_ENTRIES"), std::env::var("U3D_GH_DIR")) else { return };
    let list: Value = serde_json::from_str(std::fs::read_to_string(entries).unwrap().trim_start_matches('\u{feff}')).unwrap();
    for x in list.as_array().unwrap() {
        let s = |k: &str| x[k].as_str().unwrap_or("").to_string();
        let hive: &'static str = match s("hive").as_str() { "HKCU" => "HKCU", "HKLM32" => "HKLM32", _ => "HKLM64" };
        let e = UninstallEntry { key: s("key"), hive, display_name: s("name"), display_version: s("version"), url_info_about: s("about"), help_link: s("help"), url_update_info: s("update"), uninstall_string: s("uninstall"), ..Default::default() };
        let Some((o, r)) = repo_of(&e) else {
            eprintln!("{}: names no project of its own", e.display_name);
            continue;
        };
        let file = std::path::Path::new(&dir).join(format!("{}_{}.json", o.to_ascii_lowercase(), r.to_ascii_lowercase()));
        let rels = std::fs::read(&file).ok().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).map(|v| parse_releases(&v)).unwrap_or_default();
        let kind = kind_of(&e);
        if rels.is_empty() || !fits(&e.display_version, &rels) {
            eprintln!("{} {}: {o}/{r}, {} releases, not its own", e.display_name, e.display_version, rels.len());
            continue;
        }
        let top = newest(&rels, &e.display_version);
        let file = top.and_then(|t| installer_for(t, kind, "x64"));
        eprintln!("{} {}: {o}/{r} ({kind:?}), newest {:?}, installer {:?}", e.display_name, e.display_version, top.map(|t| &t.version), file.map(|a| &a.name));
    }
}

/// The real project: run with `cargo test ghapps -- --ignored`. `U3D_GH_JSON` names a saved
/// answer to read instead (GitHub allows 60 questions an hour without signing in).
#[tokio::test]
#[ignore]
async fn live_tidalfast_releases() {
    let rels = match std::env::var("U3D_GH_JSON") {
        Ok(p) => parse_releases(&serde_json::from_slice(&std::fs::read(p).unwrap()).unwrap()),
        Err(_) => releases("cioudguy/tidalfast").await,
    };
    let r = newest(&rels, "1.0.2-alpha.3").expect("a release");
    let a = installer_for(r, Kind::Inno, "x64").expect("its setup");
    eprintln!("{} {} {:?}", r.version, a.name, a.sha256);
    assert!(a.name.ends_with("-setup.exe"));
}

#[test]
fn a_projects_description_becomes_a_short_line() {
    assert_eq!(short_about("A client mod for the Tidal music client & successor to Neptune").as_deref(), Some("A client mod for the Tidal music client & successor to Neptune"));
    assert_eq!(short_about(":rocket: Fast TIDAL player. See https://example.com.").as_deref(), Some("Fast TIDAL player. See"));
    assert_eq!(short_about("  "), None);
    let long = "word ".repeat(60);
    let s = short_about(&long).unwrap();
    assert!(s.ends_with("...") && s.chars().count() <= 143, "{s}");
}
