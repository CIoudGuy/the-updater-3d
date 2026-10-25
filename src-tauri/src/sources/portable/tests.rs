use super::*;
use crate::sources::locate::Shortcut;

/// A folder next to the test program (Temp never counts as an install folder). Removed on drop.
struct Fx(PathBuf);

impl Fx {
    fn new(name: &str) -> Fx {
        let d = std::env::current_exe().unwrap().parent().unwrap().join(format!("portable-fixture-{name}-{}", std::process::id()));
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
    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.0.join(rel)).ok()
    }
}

impl Drop for Fx {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn shortcut(name: &str, target: &Path) -> Shortcut {
    Shortcut { name: name.into(), target: target.to_path_buf(), lnk: PathBuf::from(format!("C:\\Links\\{name}.lnk")) }
}

/// What the program files of the tests say about themselves.
fn thorium_like(p: &Path) -> Option<Program> {
    let stem = p.file_stem()?.to_string_lossy().to_ascii_lowercase();
    let (name, company) = match stem.as_str() {
        "thorium" | "chrome_proxy" => ("Thorium", "Alex313031"),
        "zapster" => ("Zapster", "Zap Ltd"),
        "tool" => ("Tool", "Tool Co"),
        "named" => ("Named App", "Named Co"),
        _ => return None,
    };
    Some(Program { exe: p.to_path_buf(), name: name.into(), company: company.into(), version: "156.0.8078.28".into() })
}

#[test]
fn a_portable_browser_is_found_by_its_shortcut_once_per_folder() {
    let fx = Fx::new("found");
    // Thorium's portable build: BIN holds the program, USER_DATA the profile next to it.
    let exe = fx.file("Browsers\\Thorium\\BIN\\thorium.exe", &"MZ".repeat(400));
    let proxy = fx.file("Browsers\\Thorium\\BIN\\chrome_proxy.exe", "MZ");
    fx.file("Browsers\\Thorium\\USER_DATA\\Local State", "{}");
    // Its own shortcut and a web app's shortcut, which starts the browser's helper.
    let links = vec![shortcut("Thorium", &exe), shortcut("Paper Planner", &proxy)];
    let found = candidates(&links, &[], &[], &thorium_like);
    assert_eq!(found.len(), 1);
    let e = &found[0];
    assert_eq!((e.hive, e.display_name.as_str(), e.publisher.as_str(), e.display_version.as_str()), (HIVE, "Thorium", "Alex313031", "156.0.8078.28"));
    assert!(same_path(Path::new(&e.key), &exe));
    assert!(same_path(Path::new(&e.install_location), &fx.0.join("Browsers\\Thorium")));
    // A web app it installed names it too, with no shortcut to the browser at all.
    let mut web = UninstallEntry { key: "3c9e1f0a7b2d4e6f8a1c3e5b7d9f0a2c".into(), hive: "HKCU", display_name: "Paper Planner".into(), display_version: "1.0".into(), publisher: "Thorium".into(), ..Default::default() };
    web.uninstall_string = format!(r#""{}" --uninstall-app-id=kmpdfnlgcoapkiibcehndbgjmnihblme"#, exe.display());
    let found = candidates(&[], &[web], &[], &thorium_like);
    assert_eq!(found.len(), 1);
    assert!(same_path(Path::new(&found[0].key), &exe));
}

#[test]
fn a_program_an_installed_app_covers_is_not_a_portable_copy() {
    let fx = Fx::new("covered");
    // In the folder an entry of Windows' list names.
    let zap = fx.file("Apps\\Zapster\\zapster.exe", "MZ");
    let mut listed = UninstallEntry { key: "Zapster".into(), hive: "HKLM64", display_name: "Zapster".into(), display_version: "2.0".into(), ..Default::default() };
    listed.install_location = fx.0.join("Apps\\Zapster").to_string_lossy().to_string();
    // An uninstaller next to it: installed after all.
    let tool = fx.file("Apps\\Tool\\tool.exe", "MZ");
    fx.file("Apps\\Tool\\unins000.exe", "MZ");
    // Named like an app in Windows' list.
    let named = fx.file("Apps\\Named\\named.exe", "MZ");
    let other = UninstallEntry { key: "{1}".into(), hive: "HKLM64", display_name: "Named App".into(), display_version: "1.0".into(), ..Default::default() };
    // In the folder of an app another source lists.
    let blender = fx.file("Apps\\Thorium\\thorium.exe", "MZ");
    let handled = crate::sources::Installed {
        id: "blender:5.2".into(),
        source: crate::model::Source::Blender,
        name: "Blender".into(),
        code: String::new(),
        fg: String::new(),
        bg: String::new(),
        version: "5.2".into(),
        install_dir: Some(fx.0.join("Apps\\Thorium")),
        exe: None,
        uninstall: None,
        updated_at: None,
        size_hint: None,
        tagline: None,
        channel: None,
        missing: false,
        read_only: false,
        icon_remote: None,
        data: SourceData::Tools { key: "x".into(), series: String::new() },
        store: None,
    };
    let links = vec![shortcut("Zapster", &zap), shortcut("Tool", &tool), shortcut("Named", &named), shortcut("Thorium", &blender)];
    assert!(candidates(&links, &[listed, other], &[handled], &thorium_like).is_empty());
}

#[test]
fn the_build_a_copy_is_named_in_its_folders_or_a_marker_next_to_its_program() {
    let fx = Fx::new("words");
    let exe = fx.file("Browsers\\Thorium\\BIN\\thorium.exe", "MZ");
    fx.file("Browsers\\Thorium\\BIN\\thor_ver", "Win\r\nAVX2\r\n");
    // A big file is not a marker.
    fx.file("Browsers\\Thorium\\BIN\\icudtl", &"SSE4 ".repeat(40));
    let e = entry_for(&fx.0.join("Browsers\\Thorium"), &Program { exe, name: "Thorium".into(), company: "Alex313031".into(), version: "156.0.8078.28".into() });
    let w = build_words(&e);
    assert!(w.contains(&"avx2".to_string()) && w.contains(&"thorium".to_string()));
    assert!(!w.contains(&"sse4".to_string()));
}

/// Files of Thorium's GitHub release M156.0.8078.28 (Windows, Linux, macOS and Android).
fn thorium_release() -> Vec<Asset> {
    [
        "SystemWebView_arm64.apk",
        "thorium-browser_156.0.8078.28_arm64.zip",
        "thorium-browser_156.0.8078.28_AVX.zip",
        "thorium-browser_156.0.8078.28_AVX2.zip",
        "thorium-browser_156.0.8078.28_AVX512.zip",
        "thorium-browser_156.0.8078.28_i386.zip",
        "thorium-browser_156.0.8078.28_SSE3.zip",
        "thorium-browser_156.0.8078.28_SSE4.zip",
        "thorium-browser_156.0.8078.28_AVX2.deb",
        "Thorium_ARM64_156.0.8078.28.zip",
        "thorium_ARM64_installer.exe",
        "Thorium_AVX2_156.0.8078.28.zip",
        "thorium_AVX2_mini_installer.exe",
        "Thorium_AVX512_156.0.8078.28.zip",
        "thorium_AVX512_mini_installer.exe",
        "Thorium_AVX_156.0.8078.28.zip",
        "thorium_AVX_mini_installer.exe",
        "Thorium_Browser_156.0.8078.28_AVX2.AppImage",
        "Thorium_MacOS_x64.dmg",
        "Thorium_SSE3_156.0.8078.28.zip",
        "thorium_SSE3_mini_installer.exe",
        "Thorium_SSE4_156.0.8078.28.zip",
        "thorium_SSE4_mini_installer.exe",
        "Thorium_WIN32_SSE2_156.0.8078.28.zip",
        "thorium_WIN32_SSE2_mini_installer.exe",
    ]
    .iter()
    .map(|n| Asset { name: n.to_string(), url: format!("https://github.com/gz83/thorium/releases/download/M156.0.8078.28/{n}"), size: Some(1), sha256: Some("c".repeat(64)) })
    .collect()
}

#[test]
fn the_portable_zip_is_the_one_that_goes_with_the_catalogs_installer() {
    let rel = thorium_release();
    let pick = |inst: &str, arch: &str| pick_zip(&rel, inst, arch).map(|a| a.name.clone());
    // Not the Linux zip of the same build, not another build.
    assert_eq!(pick("thorium_AVX2_mini_installer.exe", "x64").as_deref(), Some("Thorium_AVX2_156.0.8078.28.zip"));
    assert_eq!(pick("thorium_AVX_mini_installer.exe", "x64").as_deref(), Some("Thorium_AVX_156.0.8078.28.zip"));
    assert_eq!(pick("thorium_SSE4_mini_installer.exe", "x64").as_deref(), Some("Thorium_SSE4_156.0.8078.28.zip"));
    assert_eq!(pick("thorium_ARM64_installer.exe", "arm64").as_deref(), Some("Thorium_ARM64_156.0.8078.28.zip"));
    // Nothing that starts like the installer: no zip.
    assert_eq!(pick("ChromeSetup.exe", "x64"), None);
    // Two that fit equally well: no guess.
    let twins: Vec<Asset> = ["App_1.0_a.zip", "App_1.0_b.zip"].iter().map(|n| Asset { name: n.to_string(), url: String::new(), size: None, sha256: None }).collect();
    assert_eq!(pick_zip(&twins, "App_1.0_setup.exe", "x64").map(|a| a.name.clone()), None);
}

/// A zip of these files ("" makes a folder).
fn zip_of(path: &Path, files: &[(&str, &str)]) {
    let mut w = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    let o = zip::write::SimpleFileOptions::default();
    for (name, body) in files {
        if body.is_empty() && name.ends_with('/') {
            w.add_directory(*name, o).unwrap();
        } else {
            w.start_file(*name, o).unwrap();
            std::io::Write::write_all(&mut w, body.as_bytes()).unwrap();
        }
    }
    w.finish().unwrap();
}

/// A portable Thorium 155 as it is on disk, with a profile and a note of the user's.
fn old_copy(fx: &Fx) -> PathBuf {
    let exe = fx.file("Thorium\\BIN\\thorium.exe", "old exe");
    fx.file("Thorium\\BIN\\chrome_proxy.exe", "old proxy");
    fx.file("Thorium\\BIN\\thor_ver", "Win\nAVX2");
    fx.file("Thorium\\BIN\\155.0.1.1\\chrome.dll", "old dll");
    fx.file("Thorium\\BIN\\155.0.1.1\\Locales\\en-US.pak", "old pak");
    fx.file("Thorium\\USER_DATA\\Default\\Preferences", "mine");
    fx.file("Thorium\\THORIUM.BAT", "old launcher");
    fx.file("Thorium\\README.txt", "my notes");
    exe
}

const NEW_ZIP: &[(&str, &str)] = &[
    ("BIN/", ""),
    ("BIN/thorium.exe", "new exe"),
    ("BIN/chrome_proxy.exe", "new proxy"),
    ("BIN/thor_ver", "Win\nAVX2"),
    ("BIN/156.0.0.1/chrome.dll", "new dll"),
    ("BIN/156.0.0.1/Locales/en-US.pak", "new pak"),
    ("USER_DATA/", ""),
    ("USER_DATA/Default/Preferences", "fresh profile"),
    ("THORIUM.BAT", "new launcher"),
    ("README.txt", "readme"),
    ("THORIUM_SHELL.BAT", "shell launcher"),
];

#[test]
fn an_update_replaces_the_program_and_keeps_the_data_next_to_it() {
    let fx = Fx::new("replace");
    let exe = old_copy(&fx);
    let zip = fx.0.join("new.zip");
    zip_of(&zip, NEW_ZIP);
    let done = replace(&zip, &exe, None, &crate::elevate::extract_zip).unwrap();
    assert!(done.contains("155.0.1.1"), "{done}");
    // The program and launchers are the new ones; the old version's folder is gone.
    assert_eq!(fx.read("Thorium\\BIN\\thorium.exe").as_deref(), Some("new exe"));
    assert_eq!(fx.read("Thorium\\BIN\\chrome_proxy.exe").as_deref(), Some("new proxy"));
    assert_eq!(fx.read("Thorium\\BIN\\156.0.0.1\\Locales\\en-US.pak").as_deref(), Some("new pak"));
    assert!(!fx.0.join("Thorium\\BIN\\155.0.1.1").exists());
    assert_eq!(fx.read("Thorium\\THORIUM.BAT").as_deref(), Some("new launcher"));
    assert_eq!(fx.read("Thorium\\THORIUM_SHELL.BAT").as_deref(), Some("shell launcher"));
    // The profile and the user's own files stay as they are.
    assert_eq!(fx.read("Thorium\\USER_DATA\\Default\\Preferences").as_deref(), Some("mine"));
    assert_eq!(fx.read("Thorium\\README.txt").as_deref(), Some("my notes"));
    // Nothing of the update is left behind.
    assert!(!fx.0.join("Thorium").join(STAGING).exists() && !fx.0.join("Thorium").join(BACKUP).exists());
}

#[test]
fn an_update_that_fails_halfway_leaves_the_copy_as_it_was() {
    use std::os::windows::fs::OpenOptionsExt;
    let fx = Fx::new("undo");
    let exe = old_copy(&fx);
    let zip = fx.0.join("new.zip");
    zip_of(&zip, NEW_ZIP);
    // The program file is held open by something that shares it with no one: it can not move.
    let held = std::fs::OpenOptions::new().read(true).share_mode(0).open(&exe).unwrap();
    let err = replace(&zip, &exe, None, &crate::elevate::extract_zip).unwrap_err();
    drop(held);
    assert!(format!("{err:#}").contains("left as it was"), "{err:#}");
    assert_eq!(fx.read("Thorium\\BIN\\thorium.exe").as_deref(), Some("old exe"));
    assert_eq!(fx.read("Thorium\\BIN\\chrome_proxy.exe").as_deref(), Some("old proxy"));
    assert_eq!(fx.read("Thorium\\BIN\\155.0.1.1\\chrome.dll").as_deref(), Some("old dll"));
    assert!(!fx.0.join("Thorium\\BIN\\156.0.0.1").exists());
    assert_eq!(fx.read("Thorium\\THORIUM.BAT").as_deref(), Some("old launcher"));
    assert!(!fx.0.join("Thorium\\THORIUM_SHELL.BAT").exists());
    assert!(!fx.0.join("Thorium").join(STAGING).exists() && !fx.0.join("Thorium").join(BACKUP).exists());
    // A download without the program changes nothing.
    let other = fx.0.join("other.zip");
    zip_of(&other, &[("BIN/other.exe", "x")]);
    assert!(replace(&other, &exe, None, &crate::elevate::extract_zip).is_err());
    assert_eq!(fx.read("Thorium\\BIN\\thorium.exe").as_deref(), Some("old exe"));
    assert!(!fx.0.join("Thorium\\BIN\\other.exe").exists());
}

#[test]
fn a_portable_copy_is_uninstalled_by_removing_its_program_folder() {
    let fx = Fx::new("uninstall");
    let exe = old_copy(&fx);
    let e = entry_for(&fx.0.join("Thorium"), &Program { exe: exe.clone(), name: "Thorium".into(), company: "Alex313031".into(), version: "155.0.1.1".into() });
    let inst = crate::sources::Installed {
        id: "other:Alex313031.Thorium.AVX2".into(),
        source: crate::model::Source::Other,
        name: "Thorium (AVX2)".into(),
        code: String::new(),
        fg: String::new(),
        bg: String::new(),
        version: "155.0.1.1".into(),
        install_dir: Some(fx.0.join("Thorium")),
        exe: Some(exe.clone()),
        uninstall: None,
        updated_at: None,
        size_hint: None,
        tagline: None,
        channel: None,
        missing: false,
        read_only: false,
        icon_remote: None,
        data: SourceData::Other { pkg: "Alex313031.Thorium.AVX2".into(), how: crate::sources::other::How::Portable, entry: Box::new(e), upgrade_code: None },
        store: None,
    };
    assert!(is_portable(&inst));
    let plan = plan_uninstall(&inst).unwrap();
    // The program folder goes; the profile next to it is left for the leftovers step.
    assert!(matches!(&plan.steps[0], Step::RemoveDir { path, .. } if same_path(path, &fx.0.join("Thorium\\BIN"))));
    assert!(matches!(&plan.verify, Verify::Removed { app_id } if app_id == "other:Alex313031.Thorium.AVX2"));
    assert!(same_path(&plan.block_dirs[0], &fx.0.join("Thorium\\BIN")));
    // The leftovers step after it looks at its folder; it has no uninstall entry to look for.
    let s = crate::uninstall::run::subject(&inst);
    assert!(s.entry.is_none() && s.runner.is_none());
    assert!(same_path(s.install_dir.as_deref().unwrap(), &fx.0.join("Thorium")));
}
