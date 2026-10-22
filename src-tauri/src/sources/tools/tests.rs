use super::*;
use serde_json::json;

fn rel(v: &str, pre: bool) -> Rel {
    Rel { version: v.into(), prerelease: pre, size: Some(10), file: format!("{v}.exe"), url: format!("https://x/{v}.exe"), sha256: Some("ab".repeat(32)), ..Default::default() }
}

fn inst(key: &str, id: &str, ver: &str, series: &str) -> Installed {
    let d = def(key).unwrap();
    let mut i = make(d, id.into(), d.name.into(), ver.into(), series.into(), Some(PathBuf::from(r"C:\Apps\x\y")), Some(PathBuf::from(r"C:\Apps\x\y\app.exe")));
    i.missing = false;
    i
}

fn feed_with(key: &str, list: Vec<Rel>) -> Feed {
    let mut f = Feed::default();
    f.apps.insert(key.into(), list);
    f
}

fn gh_asset(name: &str, size: u64, sha: Option<&str>) -> Value {
    json!({ "name": name, "browser_download_url": format!("https://github.com/dl/{name}"), "size": size, "digest": sha.map(|s| format!("sha256:{s}")) })
}

#[test]
fn versions_order_across_release_stages() {
    assert_eq!(cmp_ver("26.3rc2", "26.3"), Ordering::Less);
    assert_eq!(cmp_ver("26.3rc2", "26.3rc1"), Ordering::Greater);
    assert_eq!(cmp_ver("26.3", "1.1.4"), Ordering::Greater);
    assert_eq!(cmp_ver("4.8-beta1", "4.8-rc1"), Ordering::Less);
    assert_eq!(cmp_ver("4.8-rc1", "4.8-stable"), Ordering::Less);
    assert_eq!(cmp_ver("4.8", "4.7.2"), Ordering::Greater);
    assert_eq!(cmp_ver("4.8-dev7", "4.8-beta1"), Ordering::Less);
    assert_eq!(cmp_ver("33.0.0-rc3", "32.2.2"), Ordering::Greater);
    assert_eq!(cmp_ver("33.0.0-beta5", "33.0.0-beta6"), Ordering::Less);
    assert_eq!(cmp_ver("6000.3.27f1", "6000.3.26f1"), Ordering::Greater);
    assert_eq!(cmp_ver("2022.3.62f3", "2022.3.62f1"), Ordering::Greater);
    assert_eq!(cmp_ver("6000.7.0b6", "6000.7.0f1"), Ordering::Less);
    assert!(is_pre("26.3rc2") && is_pre("4.8-beta1") && is_pre("6000.7.0b6") && is_pre("33.0.0-rc3"));
    assert!(!is_pre("1.1.4") && !is_pre("6000.3.27f1") && !is_pre("32.2.2"));
}

#[test]
fn obs_installer_is_picked_with_its_digest() {
    let gh = json!([
        { "tag_name": "33.0.0-rc3", "prerelease": true, "draft": false, "published_at": "2026-10-20T23:50:59Z", "html_url": "https://github.com/obsproject/obs-studio/releases/tag/33.0.0-rc3", "body": "* Fixed a crash [notr1ch]\n* Another fix\n  + nested detail",
          "assets": [gh_asset("OBS-Studio-33.0.0-rc3-Windows-arm64.zip", 5, Some("aa")), gh_asset("OBS-Studio-33.0.0-rc3-Windows-x64-Installer.exe", 186624848, Some("f43e"))] },
        { "tag_name": "32.2.2", "prerelease": false, "draft": false, "published_at": "2026-08-14T00:00:00Z", "html_url": "u", "body": "32.2.2 Hotfix Changes\n---------------------\n\n* Fixed plugins not loading after updating OBS on Windows [notr1ch]\n* Blocked OBS Studio from running on macOS 12 [RytoEX]",
          "assets": [gh_asset("OBS-Studio-32.2.2-Windows-x64-Installer.exe", 159809104, Some("c3a0"))] },
        { "tag_name": "32.2.1", "prerelease": false, "draft": true, "assets": [gh_asset("OBS-Studio-32.2.1-Windows-x64-Installer.exe", 1, None)] },
        { "tag_name": "31.0.0", "prerelease": false, "draft": false, "assets": [gh_asset("OBS-Studio-31.0.0-macOS-Apple.dmg", 1, None)] },
        { "tag_name": "30.0.0", "prerelease": false, "draft": false, "assets": [gh_asset("OBS-Studio-30.0.0-Windows-x64-Installer.exe", 157456592, None)] }
    ]);
    let v = parse_obs(&gh);
    assert_eq!(v.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), vec!["33.0.0-rc3", "32.2.2", "30.0.0"]);
    assert!(v[0].prerelease && !v[1].prerelease);
    assert_eq!(v[1].file, "OBS-Studio-32.2.2-Windows-x64-Installer.exe");
    assert_eq!(v[1].size, Some(159809104));
    assert_eq!(v[1].sha256.as_deref(), Some("c3a0"));
    // Older releases have no digest: the exact size is all there is to check.
    assert_eq!(v[2].sha256, None);
    assert_eq!(v[0].notes, vec!["Fixed a crash", "Another fix"]);
    assert_eq!(v[1].notes, vec!["Fixed plugins not loading after updating OBS on Windows", "Blocked OBS Studio from running on macOS 12"]);
}

#[test]
fn freecad_hides_weeklies_and_picks_the_x64_installer() {
    let gh = json!([
        { "tag_name": "weekly-2026.10.21", "prerelease": true, "assets": [gh_asset("FreeCAD_weekly-2026.10.21-Windows-x86_64-installer.exe", 1, None)] },
        { "tag_name": "26.3rc2", "prerelease": true, "published_at": "2026-10-18T00:00:00Z", "body": "intro\n\n- Report anything new: https://github.com/x",
          "assets": [gh_asset("FreeCAD_26.3rc2-Windows-arm64-experimental.7z", 1, None), gh_asset("FreeCAD_26.3rc2-Windows-x86_64-installer.exe", 436663424, Some("c70c")), gh_asset("FreeCAD_26.3rc2-Windows-x86_64.7z", 3, None)] },
        { "tag_name": "1.1.4", "prerelease": false, "published_at": "2026-09-28T00:00:00Z", "body": "## What's Changed\n* fix COIN3D_MICRO_VERSION regex for coin 4.0.10 by @moench in #30899 (backported by PR [#31571](https://github.com/x))\n* core: faster open by @dev in #1",
          "assets": [gh_asset("FreeCAD_1.1.4-Windows-x86_64-py311-installer.exe", 519314976, None), gh_asset("FreeCAD_1.1.4-Windows-x86_64-py311-installer.exe-SHA256.txt", 115, None), gh_asset("FreeCAD_1.1.4-Windows-x86_64-py311.7z", 9, None)] },
        { "tag_name": "weekly-2026.09.30", "prerelease": true, "assets": [gh_asset("FreeCAD_weekly-2026.09.30-Windows-x86_64-experimental-installer.exe", 1, None)] }
    ]);
    let v = parse_freecad(&gh);
    assert_eq!(v.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), vec!["26.3rc2", "1.1.4"]);
    assert!(v[0].prerelease && !v[1].prerelease);
    assert_eq!(v[0].sha256.as_deref(), Some("c70c"));
    assert_eq!(v[1].file, "FreeCAD_1.1.4-Windows-x86_64-py311-installer.exe");
    // No digest: the checksum file next to the installer is used at install time.
    assert_eq!(v[1].sums_url.as_deref(), Some("https://github.com/dl/FreeCAD_1.1.4-Windows-x86_64-py311-installer.exe-SHA256.txt"));
    assert_eq!(v[1].notes, vec!["fix COIN3D_MICRO_VERSION regex for coin 4.0.10", "core: faster open"]);
    // A release candidate says what it is instead of listing boilerplate links.
    assert_eq!(v[0].notes, vec!["intro"]);
}

#[test]
fn godot_stable_from_the_main_repo_and_betas_from_builds() {
    let main = json!([
        { "tag_name": "4.7.2-stable", "prerelease": false, "published_at": "2026-08-18T00:00:00Z", "html_url": "https://github.com/godotengine/godot/releases/tag/4.7.2-stable",
          "body": "**Godot 4.7.2** is a maintenance release addressing stability and usability issues. Maintenance releases are recommended.\r\n\r\nReport bugs on GitHub:\r\n- https://github.com/godotengine/godot/issues\r\n\r\n- [Release notes](https://godotengine.org/article/maintenance-release-godot-4-7-2/)\r\n",
          "assets": [gh_asset("Godot_v4.7.2-stable_mono_win64.zip", 1, None), gh_asset("Godot_v4.7.2-stable_win64.exe.zip", 86013866, Some("7319"))] },
        { "tag_name": "3.6.3-stable", "prerelease": false, "assets": [gh_asset("Godot_v3.6.3-stable_win64.exe.zip", 35756983, Some("8061"))] },
        { "tag_name": "4.7-stable", "prerelease": false, "assets": [gh_asset("Godot_v4.7-stable_win64.exe.zip", 80000000, None)] }
    ]);
    let v = parse_godot(&main, false);
    assert_eq!(v.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), vec!["4.7.2", "4.7", "3.6.3"]);
    assert_eq!(v[0].file, "Godot_v4.7.2-stable_win64.exe.zip");
    assert_eq!(v[0].sha256.as_deref(), Some("7319"));
    assert_eq!(v[0].notes, vec!["Godot 4.7.2 is a maintenance release addressing stability and usability issues"]);
    assert_eq!(v[0].notes_url.as_deref(), Some("https://godotengine.org/article/maintenance-release-godot-4-7-2/"));

    let builds = json!([
        { "tag_name": "4.8-beta1", "prerelease": true, "body": "x", "assets": [gh_asset("Godot_v4.8-beta1_win64.exe.zip", 64756504, Some("d131")), gh_asset("Godot_v4.8-beta1_win64.exe.debugsymbols.zip", 5, None)] },
        { "tag_name": "4.8-dev7", "prerelease": true, "assets": [gh_asset("Godot_v4.8-dev7_win64.exe.zip", 1, None)] },
        { "tag_name": "4.7.2-rc1", "prerelease": true, "assets": [gh_asset("Godot_v4.7.2-rc1_win64.exe.zip", 2, None)] },
        { "tag_name": "4.7.2-stable", "prerelease": false, "assets": [gh_asset("Godot_v4.7.2-stable_win64.exe.zip", 86013866, None)] }
    ]);
    let b = parse_godot(&builds, true);
    assert_eq!(b.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), vec!["4.8-beta1", "4.7.2-rc1"]);
    assert!(b.iter().all(|r| r.prerelease));
    assert_eq!(b[0].file, "Godot_v4.8-beta1_win64.exe.zip");
}

#[test]
fn krita_index_and_folder_are_parsed() {
    let index = r#"<tr><td valign="top"><a href="5.3.4.1/"><img src="/icons/folder.gif" alt="[DIR]" /></a></td><td><a href="5.3.4.1/">5.3.4.1/</a>    </td><td align="right">2026-09-20 10:00  </td><td align="right">  - </td><td>&nbsp;</td></tr>
<tr><td valign="top"><a href="6.0.4.1/"><img src="/icons/folder.gif" alt="[DIR]" /></a></td><td><a href="6.0.4.1/">6.0.4.1/</a>   </td><td align="right">2026-10-07 07:21  </td><td align="right">  - </td><td>&nbsp;</td></tr>
<tr><td valign="top"><a href="6.0.4/"><img src="/icons/folder.gif" alt="[DIR]" /></a></td><td><a href="6.0.4/">6.0.4/</a>   </td><td align="right">2026-10-01 07:21  </td><td align="right">  - </td><td>&nbsp;</td></tr>
<tr><td valign="top"><a href="6.0.2/"><img src="/icons/folder.gif" alt="[DIR]" /></a></td><td><a href="6.0.2/">6.0.2/</a>   </td><td align="right">2026-05-26 14:33  </td><td align="right">  - </td><td>&nbsp;</td></tr>
<tr><td valign="top"><a href="FastSketchPlugin-1.0.2/"><img src="/icons/folder.gif" alt="[DIR]" /></a></td><td><a href="FastSketchPlugin-1.0.2/">FastSketchPlugin-1.0.2/</a></td><td align="right">2026-01-01 00:00  </td><td align="right">  - </td><td>&nbsp;</td></tr>
<tr><td valign="top"><a href="5.2.12-android/"><img src="/icons/folder.gif" alt="[DIR]" /></a></td><td><a href="5.2.12-android/">5.2.12-android/</a></td><td align="right">2026-01-01 00:00  </td><td align="right">  - </td><td>&nbsp;</td></tr>"#;
    // Newest first; plugin folders and Android builds are not Krita releases for Windows.
    assert_eq!(krita_versions(index), vec!["6.0.4.1", "6.0.4", "6.0.2", "5.3.4.1"]);

    let folder = r#"<tr><td valign="top"><a href="krita-6.0.4.1-signed.dmg"><img src="/icons/binary.gif" alt="[   ]" /></a></td><td><a href="krita-6.0.4.1-signed.dmg">krita-6.0.4.1-signed.dmg</a>   </td><td align="right">2026-10-06 15:00  </td><td align="right">384M</td><td>&nbsp;</td></tr>
<tr><td valign="top"><a href="krita-x64-6.0.4.1-setup.exe"><img src="/icons/binary.gif" alt="[   ]" /></a></td><td><a href="krita-x64-6.0.4.1-setup.exe">krita-x64-6.0.4.1-setup.exe</a>      </td><td align="right">2026-10-01 07:29  </td><td align="right">167M</td><td>&nbsp;</td></tr>"#;
    let (file, date, size) = krita_installer(folder, "6.0.4.1").unwrap();
    assert_eq!(file, "krita-x64-6.0.4.1-setup.exe");
    assert_eq!(date.as_deref(), Some("2026-10-01T00:00:00Z"));
    assert_eq!(size, Some(167 * 1024 * 1024));
    // A release folder without a Windows installer (an Android-only release) is skipped.
    assert!(krita_installer(folder, "5.3.4.1").is_none());
}

#[test]
fn version_lists_only_keep_pre_releases_of_unreleased_versions() {
    let mut l = vec![rel("33.0.0-rc3", true), rel("33.0.0-beta1", true), rel("32.2.2", false), rel("32.2.0-rc2", true), rel("32.2.0", false)];
    sort_newest_first(&mut l);
    trim_pre(&mut l);
    assert_eq!(l.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), vec!["33.0.0-rc3", "33.0.0-beta1", "32.2.2", "32.2.0"]);
    // Nothing stable yet: everything stays.
    let mut only_pre = vec![rel("1.0-rc1", true), rel("1.0-beta1", true)];
    trim_pre(&mut only_pre);
    assert_eq!(only_pre.len(), 2);
}

#[test]
fn checksum_files_are_read() {
    let h = "f4b18257db3efb81cdc1563db3e813c8f6ec4150d4d89b444c897a661c99b94b";
    assert_eq!(sum_from(&format!("{h}  krita-x64-6.0.4.1-setup.exe\n"), "krita-x64-6.0.4.1-setup.exe").as_deref(), Some(h));
    assert_eq!(sum_from(&format!("{}\n", h.to_ascii_uppercase()), "anything").as_deref(), Some(h));
    assert_eq!(sum_from("not a checksum file", "x"), None);
}

#[test]
fn unity_release_api_is_parsed() {
    let api = json!({ "offset": 0, "limit": 25, "total": 2, "results": [
        { "version": "6000.3.27f1", "releaseDate": "2026-10-20T13:42:16.544Z", "stream": "LTS", "releaseNotes": { "url": "https://x/6000_3_27f1.md", "type": "MD" },
          "downloads": [ { "url": "https://download.unity3d.com/download_unity/672d562a42a7/Windows64EditorInstaller/UnitySetup64-6000.3.27f1.exe", "type": "EXE", "platform": "WINDOWS",
            "downloadSize": { "value": 4235893440u64, "unit": "BYTE" }, "installedSize": { "value": 8464143666u64, "unit": "BYTE" }, "integrity": "md5-e5QStPCXCBTa8atkaZRINg==" } ] },
        { "version": "6000.7.0b6", "releaseDate": "2026-10-21T00:00:00Z", "stream": "BETA",
          "downloads": [ { "url": "https://download.unity3d.com/download_unity/aaa/Windows64EditorInstaller/UnitySetup64-6000.7.0b6.exe", "downloadSize": { "value": 100 } } ] },
        { "version": "1.0.0f1", "stream": "LTS", "downloads": [] }
    ]});
    let v = parse_unity(&api);
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].version, "6000.3.27f1");
    assert!(!v[0].prerelease && v[1].prerelease);
    assert_eq!(v[0].size, Some(4235893440));
    assert_eq!(v[0].file, "UnitySetup64-6000.3.27f1.exe");
    assert_eq!(v[0].md5.as_deref(), Some("7b9412b4f0970814daf1ab6469944836"));
    assert_eq!(v[0].notes_src.as_deref(), Some("https://x/6000_3_27f1.md"));
    assert_eq!(v[0].notes_url.as_deref(), Some("https://unity.com/releases/editor/whats-new/6000.3.27f1"));
    assert_eq!(v[1].md5, None);
}

#[test]
fn unity_notes_skip_known_issues() {
    let md = "### Known Issues in 6000.3.27f1\n\n- Content Build: Player stops responding\n    ([UUM-1](https://x))\n\n### 6000.3.27f1 Release Notes\n\n#### Improvements\n\n- Asset Import: Improved the security of model import [details](https://x) \u{2014} two fixes.\n    (UUM-155199)\n\n#### API Changes\n\n- Graphics: Added Graphics.disableDepthAndStencilBuffers\n";
    let v = unity_bullets(md);
    // Unity's sections come along as headings.
    assert_eq!(v, vec!["## Improvements", "Asset Import: Improved the security of model import details, two fixes", "## API Changes", "Graphics: Added Graphics.disableDepthAndStencilBuffers"]);
    // Without a release notes heading every bullet counts, all of them.
    let many: String = (0..30).map(|i| format!("- item {i}\n")).collect();
    assert_eq!(unity_bullets(&many).len(), 30);
}

#[test]
fn offers_the_newest_stable_release_unless_a_pre_release_is_installed() {
    let feed = feed_with("obs", vec![rel("33.0.0-rc3", true), rel("32.2.2", false), rel("32.2.1", false)]);
    let stable = inst("obs", "tools:obs", "32.2.1", "");
    assert_eq!(latest(&stable, std::slice::from_ref(&stable), &feed).unwrap().version, "32.2.2");
    let current = inst("obs", "tools:obs", "32.2.2", "");
    assert_eq!(latest(&current, std::slice::from_ref(&current), &feed).unwrap().version, "32.2.2");
    let testing = inst("obs", "tools:obs", "33.0.0-rc2", "");
    assert!(testing.channel.as_deref() == Some("beta"));
    assert_eq!(latest(&testing, std::slice::from_ref(&testing), &feed).unwrap().version, "33.0.0-rc3");
}

#[test]
fn freecad_series_update_in_place_and_the_newest_series_is_offered_the_next() {
    let feed = feed_with("freecad", vec![rel("26.3", false), rel("1.1.4", false), rel("1.1.3", false), rel("1.0.2", false)]);
    let old = inst("freecad", "tools:freecad:1.0", "1.0.1", "1.0");
    let new = inst("freecad", "tools:freecad:1.1", "1.1.4", "1.1");
    let all = vec![new.clone(), old.clone()];
    // 1.0 gets its own bug-fix, in place, not 26.3.
    let l = latest(&old, &all, &feed).unwrap();
    assert_eq!((l.version.as_str(), l.note), ("1.0.2", None));
    // 1.1 is the newest installed series and up to date: it is offered 26.3 next to it.
    let l = latest(&new, &all, &feed).unwrap();
    assert_eq!(l.version, "26.3");
    assert!(l.note.unwrap().contains("installs next to 1.1.4"));
}

#[test]
fn side_by_side_copies_do_not_nag_about_a_build_that_is_already_installed() {
    let feed = feed_with("godot", vec![rel("4.7.2", false), rel("4.7.1", false), rel("4.8-beta1", true)]);
    let a = inst("godot", "tools:godot:4.7.1", "4.7.1", "4.7.1");
    let l = latest(&a, std::slice::from_ref(&a), &feed).unwrap();
    assert_eq!(l.version, "4.7.2");
    assert!(l.note.unwrap().contains("installs next to 4.7.1"));
    // Once 4.7.2 is installed too, the old copy is no longer offered it.
    let b = inst("godot", "tools:godot:4.7.2", "4.7.2", "4.7.2");
    let all = vec![a.clone(), b.clone()];
    assert_eq!(latest(&a, &all, &feed).unwrap().version, "4.7.1");
    assert_eq!(latest(&b, &all, &feed).unwrap().version, "4.7.2");
}

#[test]
fn unity_updates_stay_in_their_release_series() {
    let mut feed = Feed::default();
    feed.unity_series.insert("2022.3".into(), vec![rel("2022.3.62f3", false), rel("2022.3.62f1", false)]);
    feed.unity_series.insert("6000.3".into(), vec![rel("6000.3.27f1", false), rel("6000.3.26f1", false)]);
    let a = inst("unity", "tools:unity:2022.3.62f1", "2022.3.62f1", "2022.3.62f1");
    let b = inst("unity", "tools:unity:6000.3.27f1", "6000.3.27f1", "6000.3.27f1");
    let all = vec![a.clone(), b.clone()];
    // The build number after the letter decides: f3 is newer than f1.
    assert_eq!(latest(&a, &all, &feed).unwrap().version, "2022.3.62f3");
    assert_eq!(latest(&b, &all, &feed).unwrap().version, "6000.3.27f1");
    // A series that was not looked up gives no answer (never another series' build).
    let c = inst("unity", "tools:unity:6000.0.1f1", "6000.0.1f1", "6000.0.1f1");
    assert!(latest(&c, std::slice::from_ref(&c), &feed).is_none());
}

#[test]
fn catalog_has_one_entry_per_app_and_knows_what_is_installed() {
    let mut feed = Feed::default();
    feed.apps.insert("obs".into(), vec![rel("33.0.0-rc3", true), rel("32.2.2", false)]);
    feed.apps.insert("freecad".into(), vec![rel("1.1.4", false)]);
    feed.apps.insert("unity".into(), vec![rel("6000.3.27f1", false)]);
    let have = vec![inst("obs", "tools:obs", "32.0.0", ""), inst("freecad", "tools:freecad:1.0", "1.0.2", "1.0"), inst("unity", "tools:unity:6000.3.27f1", "6000.3.27f1", "6000.3.27f1")];
    let c = catalog_entries(&feed, &have);
    assert_eq!(c.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(), vec!["tools:obs", "tools:freecad", "tools:unity"]);
    assert!(c.iter().all(|e| e.pick_version && e.group == "More apps"));
    assert_eq!(c[0].version, "32.2.2");
    assert!(c[0].installed);
    // FreeCAD 1.0 is installed but the offered series is 1.1: still installable.
    assert!(!c[1].installed);
    assert!(c[2].installed);
}

fn ctx() -> Ctx<'static> {
    Ctx { godot_root: Path::new(r"D:\Games\Godot") }
}

fn run_of(step: &Step) -> (&PathBuf, &Vec<String>, bool) {
    match step {
        Step::Run { exe, args, elevated, .. } => (exe, args, *elevated),
        other => panic!("not a Run step: {other:?}"),
    }
}

#[test]
fn obs_and_krita_install_silently_with_admin_rights() {
    let mut r = rel("32.2.2", false);
    r.file = "OBS-Studio-32.2.2-Windows-x64-Installer.exe".into();
    r.sha256 = Some("c3a0".into());
    r.size = Some(159809104);
    let cur = inst("obs", "tools:obs", "32.2.1", "");
    let p = build_plan("obs", &r, JobKind::Update, Some(&cur), &ctx()).unwrap();
    assert_eq!(p.app_id, "tools:obs");
    let (exe, args, elevated) = run_of(&p.steps[0]);
    assert!(exe.ends_with("OBS-Studio-32.2.2-Windows-x64-Installer.exe"));
    assert_eq!(args, &vec!["/S".to_string()]);
    assert!(elevated);
    assert_eq!(p.downloads[0].size, Some(159809104));
    assert_eq!(p.downloads[0].sha256.as_deref(), Some("c3a0"));
    assert_eq!(p.block_names, vec!["obs64.exe"]);
    assert_eq!(p.block_dirs, vec![PathBuf::from(r"C:\Apps\x\y")]);
    assert!(matches!(&p.verify, Verify::Version { app_id, version } if app_id == "tools:obs" && version == "32.2.2"));
    assert_eq!(p.cleanup, vec![p.downloads[0].dest.clone()]);

    let mut k = rel("6.0.4.1", false);
    k.file = "krita-x64-6.0.4.1-setup.exe".into();
    let p = build_plan("krita", &k, JobKind::Install, None, &ctx()).unwrap();
    let (_, args, elevated) = run_of(&p.steps[0]);
    assert_eq!((args.as_slice(), elevated), (&["/S".to_string()][..], true));
    assert_eq!(p.block_names, vec!["krita.exe"]);
}

#[test]
fn freecad_installs_per_user_into_its_series_folder() {
    let mut r = rel("26.3", false);
    r.file = "FreeCAD_26.3-Windows-x86_64-installer.exe".into();
    let new = build_plan("freecad", &r, JobKind::Install, None, &ctx()).unwrap();
    assert_eq!(new.app_id, "tools:freecad:26.3");
    let (_, args, elevated) = run_of(&new.steps[0]);
    assert!(!elevated, "a per-user install needs no admin rights");
    // The folder is the last argument and has no quotes, whatever the user name looks like.
    let dir = paths::local_app_data().join("Programs").join("FreeCAD 26.3");
    assert_eq!(args, &vec!["/S".to_string(), format!("/D={}", dir.display())]);
    assert!(new.block_dirs.is_empty());

    // A bug-fix goes where the series already is, and elevates only for a folder that needs it.
    let mut cur = inst("freecad", "tools:freecad:1.1", "1.1.3", "1.1");
    cur.install_dir = Some(paths::program_files().join("FreeCAD 1.1"));
    let mut f = rel("1.1.4", false);
    f.file = "FreeCAD_1.1.4-Windows-x86_64-py311-installer.exe".into();
    let upd = build_plan("freecad", &f, JobKind::Update, Some(&cur), &ctx()).unwrap();
    let (_, args, elevated) = run_of(&upd.steps[0]);
    assert_eq!(args.last().unwrap(), &format!("/D={}", paths::program_files().join("FreeCAD 1.1").display()));
    assert!(elevated);
    assert_eq!(upd.app_id, "tools:freecad:1.1");
}

#[test]
fn unity_installs_into_a_folder_per_version_with_md5_and_exact_size() {
    let mut r = rel("6000.3.999f1", false);
    r.file = "UnitySetup64-6000.3.999f1.exe".into();
    r.size = Some(4235893440);
    r.md5 = Some("7b9412b4f0970814daf1ab6469944836".into());
    let p = build_plan("unity", &r, JobKind::Install, None, &ctx()).unwrap();
    assert_eq!(p.app_id, "tools:unity:6000.3.999f1");
    let dir = paths::program_files().join("Unity").join("Hub").join("Editor").join("6000.3.999f1");
    let (_, args, elevated) = run_of(&p.steps[0]);
    assert_eq!(args, &vec!["/S".to_string(), format!("/D={}", dir.display())]);
    assert!(elevated);
    // The disk space check and the progress bar use the real download size.
    assert_eq!(p.downloads[0].size, Some(4235893440));
    assert_eq!(p.downloads[0].md5.as_deref(), Some("7b9412b4f0970814daf1ab6469944836"));
    assert!(matches!(&p.verify, Verify::Version { app_id, .. } if app_id == "tools:unity:6000.3.999f1"));
}

#[test]
fn godot_is_unpacked_with_a_marker_and_a_start_menu_shortcut() {
    let mut r = rel("4.7.2", false);
    r.file = "Godot_v4.7.2-stable_win64.exe.zip".into();
    let p = build_plan("godot", &r, JobKind::Install, None, &ctx()).unwrap();
    assert_eq!(p.app_id, "tools:godot:4.7.2");
    assert_eq!(p.name, "Godot 4.7.2");
    let dir = PathBuf::from(r"D:\Games\Godot\Godot 4.7.2");
    assert!(matches!(&p.steps[0], Step::ExtractZip { dest, strip_root: false, elevated: false, .. } if dest == &dir));
    match &p.steps[1] {
        Step::WriteFile { path, contents, elevated } => {
            assert_eq!(path, &dir.join(MARKER));
            assert!(!elevated);
            let m: Marker = serde_json::from_str(contents).unwrap();
            assert_eq!((m.version.as_str(), m.exe.as_str()), ("4.7.2", "Godot_v4.7.2-stable_win64.exe"));
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(&p.steps[2], Step::Shortcut { target, .. } if target == &dir.join("Godot_v4.7.2-stable_win64.exe")));
    assert_eq!(p.steps.len(), 3);
    assert!(p.steps.iter().all(|s| !s.elevated()));
}

#[test]
fn nsis_uninstall_runs_in_place_and_removes_the_folder() {
    let mut i = inst("obs", "tools:obs", "32.2.2", "");
    let dir = paths::program_files().join("obs-studio");
    i.install_dir = Some(dir.clone());
    i.uninstall = Some(vec![dir.join("uninstall.exe").to_string_lossy().to_string(), "/S".into()]);
    let p = plan_uninstall(&i).unwrap();
    let (exe, args, elevated) = run_of(&p.steps[0]);
    assert_eq!(exe, &dir.join("uninstall.exe"));
    // Without _?= the uninstaller copies itself away and returns at once.
    assert_eq!(args, &vec!["/S".to_string(), format!("_?={}", dir.display())]);
    assert!(elevated);
    assert!(matches!(&p.steps[1], Step::RemoveDir { path, .. } if path == &dir));
    assert!(matches!(&p.verify, Verify::Removed { app_id } if app_id == "tools:obs"));

    // A folder that is not an application folder is never deleted.
    let mut bad = i.clone();
    bad.install_dir = Some(paths::program_files());
    assert_eq!(plan_uninstall(&bad).unwrap().steps.len(), 1);
    bad.install_dir = Some(PathBuf::from(r"C:\Tools"));
    assert_eq!(plan_uninstall(&bad).unwrap().steps.len(), 1);

    let mut none = i.clone();
    none.uninstall = None;
    assert!(plan_uninstall(&none).is_err());
}

#[test]
fn godot_uninstall_removes_its_folder_or_just_a_loose_copy() {
    let mut g = inst("godot", "tools:godot:4.7.2", "4.7.2", "4.7.2");
    g.name = "Godot 4.7.2".into();
    g.install_dir = Some(PathBuf::from(r"D:\Games\Godot\Godot 4.7.2"));
    let p = plan_uninstall(&g).unwrap();
    assert!(matches!(&p.steps[0], Step::RemoveDir { path, .. } if path == &PathBuf::from(r"D:\Games\Godot\Godot 4.7.2")));
    assert!(matches!(&p.steps[1], Step::RemoveFile { .. }));

    let mut loose = g.clone();
    loose.install_dir = None;
    loose.exe = Some(PathBuf::from(r"D:\Games\Godot\Godot_v4.7.2-stable_win64.exe"));
    let p = plan_uninstall(&loose).unwrap();
    assert!(p.steps.iter().all(|s| !matches!(s, Step::RemoveDir { .. })));
    assert!(matches!(&p.steps[1], Step::RemoveFile { path } if path.ends_with("Godot_v4.7.2-stable_win64_console.exe")));
    assert_eq!(p.block_names, vec!["godot_v4.7.2-stable_win64.exe"]);
}

#[test]
fn godot_copies_are_found_by_marker_and_by_file_name() {
    let root = std::env::temp_dir().join(format!("u3d-godot-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let ours = root.join("Godot 4.7.2");
    std::fs::create_dir_all(&ours).unwrap();
    std::fs::write(ours.join("Godot_v4.7.2-stable_win64.exe"), b"x").unwrap();
    std::fs::write(ours.join(MARKER), r#"{"version":"4.7.2","exe":"Godot_v4.7.2-stable_win64.exe"}"#).unwrap();
    // A folder without the marker is not ours.
    std::fs::create_dir_all(root.join("Other")).unwrap();
    std::fs::write(root.join("Other").join("Godot_v3.0_win64.exe"), b"x").unwrap();
    std::fs::write(root.join("Godot_v4.6.1-stable_win64.exe"), b"x").unwrap();
    std::fs::write(root.join("Godot_v4.6.1-stable_win64_console.exe"), b"x").unwrap();
    let mut out = Vec::new();
    scan_godot(&root, &mut out);
    out.sort_by(|a, b| a.id.cmp(&b.id));
    let _ = std::fs::remove_dir_all(&root);
    assert_eq!(out.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(), vec!["tools:godot:4.6.1", "tools:godot:4.7.2"]);
    assert!(out[0].install_dir.is_none() && out[1].install_dir.is_some());
    assert!(out.iter().all(|i| !i.missing));
}

#[test]
fn installed_apps_are_read_from_the_uninstall_list() {
    let entry = |key: &str, name: &str, ver: &str, uninstall: &str| winutil::UninstallEntry {
        key: key.into(),
        display_name: name.into(),
        display_version: ver.into(),
        uninstall_string: uninstall.into(),
        ..Default::default()
    };
    let un = vec![
        entry("OBS Studio", "OBS Studio", "32.2.2", r#""C:\Program Files\obs-studio\uninstall.exe""#),
        entry("Krita_x64", "Krita (x64) 6.0.4.1", "6.0.4.1", r#""C:\Program Files\Krita (x64)\uninstall.exe""#),
        entry("FreeCAD 1.1", "FreeCAD 1.1.4", "1.1.4", r#""C:\Users\u\AppData\Local\Programs\FreeCAD 1.1\Uninstall-FreeCAD.exe""#),
        entry("Unity 6000.3.27f1", "Unity 6000.3.27f1", "", r#""C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Uninstall.exe""#),
        entry("Unity Hub", "Unity Hub", "3.0.0", ""),
    ];
    let mut out = Vec::new();
    scan_obs(&un, &mut out);
    scan_krita(&un, &mut out);
    scan_freecad(&un, &mut out);
    scan_unity(&un, &mut out);
    let ids: Vec<&str> = out.iter().map(|i| i.id.as_str()).collect();
    assert!(ids.contains(&"tools:obs") && ids.contains(&"tools:krita") && ids.contains(&"tools:freecad:1.1") && ids.contains(&"tools:unity:6000.3.27f1"), "{ids:?}");
    assert!(!ids.iter().any(|i| i.contains("hub")));
    let obs = out.iter().find(|i| i.id == "tools:obs").unwrap();
    assert_eq!(obs.install_dir.as_deref(), Some(Path::new(r"C:\Program Files\obs-studio")));
    assert_eq!(obs.uninstall.as_ref().unwrap().last().map(String::as_str), Some("/S"));
    let unity = out.iter().find(|i| i.id.starts_with("tools:unity")).unwrap();
    assert_eq!(unity.install_dir.as_deref(), Some(Path::new(r"C:\Program Files\Unity\Hub\Editor\6000.3.27f1")));
    assert_eq!(unity.version, "6000.3.27f1");
    let fc = out.iter().find(|i| i.id.starts_with("tools:freecad")).unwrap();
    assert_eq!(fc.name, "FreeCAD 1.1");
    assert_eq!(fc.install_dir.as_deref(), Some(Path::new(r"C:\Users\u\AppData\Local\Programs\FreeCAD 1.1")));
}

#[test]
fn source_has_no_long_dashes() {
    for (name, text) in [("tools.rs", include_str!("../tools.rs")), ("tools/tests.rs", include_str!("tests.rs"))] {
        assert!(!text.contains('\u{2014}') && !text.contains('\u{2013}'), "{name} has an em or en dash");
    }
}

#[test]
fn unity_hub_feed_gives_the_x64_installer_with_its_sha512() {
    // Unity Hub's latest.yml as published (shortened signature lines).
    let yml = "version: 3.22.3\nfiles:\n  - url: 3.22.3/UnityHubSetup-3.22.3-x64.exe\n    sha512: M3JBFlfmEPLel7O0iB1aUFCEqzFnY4OlpVlvyz4ckvZwF48mxgWca/AmOppd+58urHMsxPui9e8lw2jE5coFCw==\n    size: 193266120\n    isAdminRightsRequired: true\n  - url: 3.22.3/UnityHubSetup-3.22.3-arm64.exe\n    sha512: CQS+0hRW2lGch5EhUpXrD/jMNQpZLFF9bkNHs94X/e5viCPdjY3YokEHwrwkhNUtOmIUDNXSwXI7niSlucWRZA==\n    size: 183441496\nreleaseDate: 2026-10-13T20:00:51.624Z\n";
    let r = parse_hub_feed(yml).unwrap();
    assert_eq!(r.version, "3.22.3");
    assert_eq!(r.url, "https://public-cdn.cloud.unity3d.com/hub/prod/3.22.3/UnityHubSetup-3.22.3-x64.exe");
    assert_eq!(r.file, "UnityHubSetup-3.22.3-x64.exe");
    assert_eq!(r.size, Some(193266120));
    let sha = r.sha512.unwrap();
    assert_eq!(sha.len(), 128);
    assert!(sha.starts_with("33724116"));
}

#[test]
fn unity_hub_notes_page_lists_every_version() {
    let html = r#"<div><h2 class="x">3.22.3</h2></div><div class="portable-text-base"><p>October 13, 2026</p><p><strong>Bug fixes and improvements</strong></p><ul><li><strong>Projects</strong><ul><li>Fixed an issue where Add project from disk could reject projects.</li></ul></li><li>Fixed the sign-in window on high DPI screens.</li></ul></div><div><h2 class="x">3.22.2</h2></div><div><p>September 30, 2026</p><ul><li>Faster editor downloads for large modules.</li></ul></div>"#;
    let v = notes::unity_hub(html);
    assert_eq!(v.len(), 2);
    assert_eq!(v[0].0, "3.22.3");
    assert_eq!(v[0].1.as_deref(), Some("2026-10-13T00:00:00Z"));
    assert!(v[0].2.iter().any(|l| l.starts_with("Fixed an issue where Add project from disk")), "{:?}", v[0].2);
    assert!(v[0].2.iter().any(|l| l == "Fixed the sign-in window on high DPI screens"), "{:?}", v[0].2);
    assert_eq!(v[1].2, vec!["Faster editor downloads for large modules"]);
}

#[test]
fn epic_version_comes_from_the_download_address() {
    assert_eq!(epic_version("https://content-download-egs.distro.on.epicgames.com/Builds/EGLv2/0.100.0%2Bg9dfc3c3/Windows/EpicGamesLauncherSetup.exe?launcherfilename=EpicGamesLauncherSetup.exe").as_deref(), Some("0.100.0"));
    assert_eq!(epic_version("https://x/Builds/EGLv2/0.97.1+gabc/Windows/a.exe").as_deref(), Some("0.97.1"));
    assert_eq!(epic_version("https://x/Builds/EGLv2/1.2/Windows/a.exe").as_deref(), Some("1.2"));
    assert_eq!(epic_version("https://x/Builds/Launcher/Windows/a.exe"), None);
    assert_eq!(epic_version("https://x/Builds/EGLv2/latest/Windows/a.exe"), None);
    assert_eq!(cmp_ver("0.100.0", "0.97.0"), Ordering::Greater);
}

fn epic_entry(name: &str, ver: &str, uninstall: &str, quiet: &str, msi: bool) -> winutil::UninstallEntry {
    winutil::UninstallEntry {
        key: "{D0769F44-D459-450F-B084-CAE38062C75B}".into(),
        display_name: name.into(),
        display_version: ver.into(),
        publisher: "Epic Games, Inc.".into(),
        install_location: r"%LOCALAPPDATA%\Epic Games\Launcher".replace("%LOCALAPPDATA%", &paths::local_app_data().to_string_lossy()),
        uninstall_string: uninstall.into(),
        quiet_uninstall_string: quiet.into(),
        windows_installer: msi,
        ..Default::default()
    }
}

#[test]
fn the_epic_preview_is_told_apart_from_the_old_launcher() {
    let dir = paths::local_app_data().join("Epic Games").join("Launcher");
    let setup = dir.join("EpicGamesLauncherSetup.exe").display().to_string();
    let preview = epic_entry("Epic Games Launcher", "0.97.0", &format!("\"{setup}\" --uninstall"), &format!("\"{setup}\" --silent --uninstall"), false);
    let old = epic_entry("Epic Games Launcher", "1.3.150.0", "MsiExec.exe /X{A1B2C3D4-0000-0000-0000-000000000000}", "", true);
    assert!(is_epic_preview(&preview));
    assert!(!is_epic_preview(&old));

    let mut out = Vec::new();
    scan_epic(&[old.clone(), preview.clone()], &mut out);
    assert_eq!(out.len(), 1);
    let i = &out[0];
    assert_eq!((i.id.as_str(), i.name.as_str(), i.version.as_str()), ("tools:epic", "Epic Games Launcher (Preview)", "0.97.0"));
    assert_eq!(i.install_dir.as_deref(), Some(dir.as_path()));
    assert_eq!(i.exe.as_deref(), Some(dir.join("EpicGamesLauncher.exe").as_path()));
    let mut none = Vec::new();
    scan_epic(&[old], &mut none);
    assert!(none.is_empty());
}

#[test]
fn epic_update_install_and_uninstall_plans() {
    let dir = paths::local_app_data().join("Epic Games").join("Launcher");
    let setup = dir.join("EpicGamesLauncherSetup.exe").display().to_string();
    let e = epic_entry("Epic Games Launcher", "0.97.0", &format!("\"{setup}\" --uninstall"), &format!("\"{setup}\" --silent --uninstall"), false);
    let mut out = Vec::new();
    scan_epic(&[e], &mut out);
    let mut cur = out.remove(0);
    cur.missing = false;

    let r = Rel { version: "0.100.0".into(), size: Some(217226744), url: "https://x/EGLv2/0.100.0%2Bg1/Windows/EpicGamesLauncherSetup.exe".into(), file: "EpicGamesLauncherSetup.exe".into(), ..Default::default() };
    let feed = feed_with("epic", vec![r.clone()]);
    let l = latest(&cur, &[cur.clone()], &feed).unwrap();
    assert_eq!(l.version, "0.100.0");

    let ctx = Ctx { godot_root: Path::new("godot") };
    let p = build_plan("epic", &r, JobKind::Update, Some(&cur), &ctx).unwrap();
    assert_eq!(p.downloads[0].signer.as_deref(), Some("Epic Games Inc."));
    assert_eq!(p.downloads[0].size, Some(217226744));
    assert!(p.downloads[0].sha256.is_none());
    match &p.steps[0] {
        Step::Run { args, elevated, .. } => assert_eq!((args.as_slice(), *elevated), (&["--silent".to_string()][..], false)),
        _ => panic!("expected a run step"),
    }
    assert_eq!(p.block_dirs, vec![dir.clone()]);
    assert!(p.block_names.iter().any(|n| n == "EpicGamesLauncher.exe"));

    let u = plan_uninstall(&cur).unwrap();
    assert_eq!(u.steps.len(), 1);
    match &u.steps[0] {
        Step::Run { exe, args, elevated, .. } => {
            assert_eq!(exe, &dir.join("EpicGamesLauncherSetup.exe"));
            assert_eq!(args.as_slice(), ["--silent".to_string(), "--uninstall".to_string()]);
            assert!(!*elevated);
        }
        _ => panic!("expected a run step"),
    }
    // Only the plain uninstall string: the silent switch is added, and NSIS switches are not.
    cur.uninstall = Some(vec![setup.clone(), "--uninstall".into(), "/S".into()]);
    match &plan_uninstall(&cur).unwrap().steps[0] {
        Step::Run { args, .. } => assert_eq!(args.as_slice(), ["--uninstall".to_string(), "--silent".to_string()]),
        _ => panic!("expected a run step"),
    }
}

#[test]
fn unity_supported_stream_is_not_a_pre_release() {
    let dl = |v: &str| json!([{ "url": format!("https://x/UnitySetup64-{v}.exe"), "downloadSize": { "value": 1 } }]);
    let api = json!({ "results": [
        { "version": "6000.4.12f1", "stream": "SUPPORTED", "downloads": dl("6000.4.12f1") },
        { "version": "6000.4.11f1", "stream": "SUPPORTED", "downloads": dl("6000.4.11f1") },
        { "version": "6000.4.0b11", "stream": "BETA", "downloads": dl("6000.4.0b11") },
        { "version": "6000.4.0a5", "stream": "ALPHA", "downloads": dl("6000.4.0a5") },
        { "version": "6000.4.0b1", "stream": "", "downloads": dl("6000.4.0b1") }
    ]});
    let v = parse_unity(&api);
    assert_eq!(v.iter().map(|r| r.prerelease).collect::<Vec<_>>(), vec![false, false, true, true, true]);
    let mut feed = Feed::default();
    feed.unity_series.insert("6000.4".into(), v);
    let a = inst("unity", "tools:unity:6000.4.11f1", "6000.4.11f1", "6000.4.11f1");
    assert_eq!(latest(&a, std::slice::from_ref(&a), &feed).unwrap().version, "6000.4.12f1");
}

#[test]
fn unity_hub_versions_without_a_download_are_not_offered() {
    let mut newer = rel("3.23.0", false);
    newer.url = String::new();
    let mut cur = rel("3.22.3", false);
    cur.sha512 = Some("ab".into());
    let feed = feed_with("unityhub", vec![newer, cur]);
    let i = inst("unityhub", "tools:unityhub", "3.22.3", "");
    assert_eq!(latest(&i, std::slice::from_ref(&i), &feed).unwrap().version, "3.22.3");
    let c = catalog_entries(&feed, &[]);
    assert_eq!(c.iter().find(|e| e.id == "tools:unityhub").unwrap().version, "3.22.3");
}

#[test]
fn an_installer_without_a_checksum_is_never_planned() {
    let mut k = rel("6.0.4.1", false);
    k.sha256 = None;
    let e = build_plan("krita", &k, JobKind::Install, None, &ctx()).unwrap_err().to_string();
    assert_eq!(e, "Could not get the checksum for Krita 6.0.4.1. Try again later.");
    for key in ["obs", "freecad", "godot", "unity"] {
        assert!(build_plan(key, &k, JobKind::Install, None, &ctx()).is_err(), "{key}");
    }
    // Any one check is enough: MD5 (Unity), SHA-512 (Unity Hub, old Godot) or the signer (Epic).
    k.md5 = Some("7b94".into());
    assert!(build_plan("unity", &k, JobKind::Install, None, &ctx()).is_ok());
    k.md5 = None;
    k.sha512 = Some("cd".into());
    assert!(build_plan("godot", &k, JobKind::Install, None, &ctx()).is_ok());
}

#[test]
fn old_godot_releases_are_checked_with_their_sha512_sums() {
    let gh = json!([{ "tag_name": "4.3-stable", "html_url": "https://x", "assets": [
        gh_asset("Godot_v4.3-stable_win64.exe.zip", 10, None),
        gh_asset("SHA512-SUMS.txt", 1, None)
    ]}]);
    let v = parse_godot(&gh, false);
    assert_eq!(v[0].sums_url.as_deref(), Some("https://github.com/dl/SHA512-SUMS.txt"));
    let h = "ad".repeat(64);
    let text = format!("{}  Godot_v4.3-stable_linux.x86_64.zip
{h}  Godot_v4.3-stable_win64.exe.zip
", "ff".repeat(64));
    assert_eq!(sha512_from(&text, "Godot_v4.3-stable_win64.exe.zip"), Some(h));
    assert_eq!(sha512_from(&text, "other.zip"), None);
}

#[test]
fn arm64_pcs_get_native_godot_unity_and_hub_builds_when_there_are_some() {
    let gh = json!([
        { "tag_name": "4.7.2-stable", "assets": [gh_asset("Godot_v4.7.2-stable_win64.exe.zip", 1, Some("aa")), gh_asset("Godot_v4.7.2-stable_windows_arm64.exe.zip", 2, Some("bb"))] },
        { "tag_name": "3.6-stable", "assets": [gh_asset("Godot_v3.6-stable_win64.exe.zip", 3, Some("cc"))] }
    ]);
    let arm = parse_godot_for(&gh, false, "arm64");
    assert_eq!(arm[0].file, "Godot_v4.7.2-stable_windows_arm64.exe.zip");
    // No ARM64 build: the x64 one, which runs emulated.
    assert_eq!(arm[1].file, "Godot_v3.6-stable_win64.exe.zip");
    assert_eq!(parse_godot_for(&gh, false, "x64")[0].file, "Godot_v4.7.2-stable_win64.exe.zip");
    assert_eq!(unity_archs("arm64"), &["ARM64", "X86_64"]);
    assert_eq!(unity_archs("x64"), &["X86_64"]);
    let yml = "version: 3.22.3\nfiles:\n  - url: 3.22.3/UnityHubSetup-3.22.3-x64.exe\n    sha512: AAAA\n    size: 1\n  - url: 3.22.3/UnityHubSetup-3.22.3-arm64.exe\n    sha512: BBBB\n    size: 2\n";
    assert!(parse_hub_feed_for(yml, "arm64").unwrap().file.ends_with("-arm64.exe"));
    assert!(parse_hub_feed_for(yml, "x64").unwrap().file.ends_with("-x64.exe"));
}

#[test]
fn unity_editors_in_the_hubs_own_folder_and_added_by_hand_are_found() {
    let root = std::env::temp_dir().join(format!("tu3d-unity-test-{}", std::process::id()));
    let editor = |dir: &Path| {
        std::fs::create_dir_all(dir.join("Editor")).unwrap();
        std::fs::write(dir.join("Editor").join("Unity.exe"), b"x").unwrap();
    };
    let custom = root.join("Editors");
    editor(&custom.join("6000.3.1f1"));
    std::fs::create_dir_all(custom.join("notaversion")).unwrap();
    let by_hand = root.join("Other").join("My Unity");
    editor(&by_hand);
    let hub = root.join("appdata").join("UnityHub");
    std::fs::create_dir_all(&hub).unwrap();
    std::fs::write(hub.join("secondaryInstallPath.json"), serde_json::to_string(&custom.display().to_string()).unwrap()).unwrap();
    let list = json!({ "schema_version": "v2", "data": [{ "version": "2022.3.62f1", "location": [by_hand.join("Editor").join("Unity.exe").display().to_string()], "manual": true }] });
    std::fs::write(hub.join("editors-v2.json"), list.to_string()).unwrap();
    let found = hub_editors(&root.join("appdata"));
    assert!(found.contains(&("6000.3.1f1".to_string(), custom.join("6000.3.1f1"))));
    assert!(found.contains(&("2022.3.62f1".to_string(), by_hand.clone())));
    assert!(!found.iter().any(|(v, _)| v == "notaversion"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn krita_from_the_microsoft_store_is_listed_and_left_to_the_store() {
    use crate::sources::msstore::Package;
    let pkg = |name: &str, family: &str| Package { name: name.into(), family: family.into(), version: "5.2.9.0".into(), install_dir: None, installed_at: None };
    let pkgs = vec![
        pkg("49800KritaProject.Krita", "49800KritaProject.Krita_n3kgb906j1zjg"),
        // Same name from someone else.
        pkg("12345Someone.Krita", "12345Someone.Krita_aaaaaaaaaaaaa"),
    ];
    let v = store_copies(&pkgs);
    assert_eq!(v.len(), 1);
    let k = &v[0];
    assert_eq!(k.id, "tools:krita:store");
    assert_eq!(k.name, "Krita (Microsoft Store)");
    assert_eq!(k.version, "5.2.9");
    assert!(k.read_only && k.uninstall.is_none() && !k.missing);
    assert_eq!(k.tagline.as_deref(), Some("Updated by the Microsoft Store"));
    assert_eq!(k.store.as_ref().unwrap().family, "49800KritaProject.Krita_n3kgb906j1zjg");

    // Never offered an update; Get apps still offers Krita from its own site.
    let feed = feed_with("krita", vec![rel("5.3.0", false)]);
    assert!(latest(k, &v, &feed).is_none());
    let c = catalog_entries(&feed, &v);
    assert!(!c.iter().find(|e| e.id == "tools:krita").unwrap().installed);
    // Next to a copy from krita.org, that one still updates.
    let site = inst("krita", "tools:krita", "5.2.6", "");
    let all = vec![site.clone(), k.clone()];
    assert_eq!(latest(&site, &all, &feed).unwrap().version, "5.3.0");
    assert!(catalog_entries(&feed, &all).iter().find(|e| e.id == "tools:krita").unwrap().installed);
}
