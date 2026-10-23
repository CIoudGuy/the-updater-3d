use super::*;
use crate::model::Source;
use crate::sources::BlenderKind;

fn app(id: &str, name: &str, version: &str, dir: Option<PathBuf>, data: SourceData) -> Installed {
    Installed {
        id: id.into(),
        source: Source::Tools,
        name: name.into(),
        code: String::new(),
        fg: String::new(),
        bg: String::new(),
        version: version.into(),
        install_dir: dir,
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
        data,
    }
}
fn tools(key: &str, ver: &str) -> SourceData {
    SourceData::Tools { key: key.into(), series: crate::version::series(ver) }
}
fn blender(series: &str, kind: BlenderKind) -> SourceData {
    SourceData::Blender { series: series.into(), kind, msi_product_code: None }
}
fn roots(tag: &str) -> Roots {
    let base = std::env::temp_dir().join(format!("u3d-prefs-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    Roots {
        roaming: base.join("Roaming"),
        local: base.join("Local"),
        program_data: base.join("ProgramData"),
        local_low: base.join("LocalLow"),
        home: base.join("Home"),
        reg: format!("Software\\u3d-prefs-test-{tag}-{}", std::process::id()),
        install_dirs: vec![],
        path_dirs: vec![],
        moniker: Box::new(|_| None),
    }
}
fn mk(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(&p).unwrap();
    p
}
fn touch(p: PathBuf) -> PathBuf {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"x").unwrap();
    p
}
fn s(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}
fn targets(steps: &[Step]) -> Vec<String> {
    steps
        .iter()
        .map(|st| match st {
            Step::RemoveDir { path, .. } | Step::RemoveFile { path } => s(path),
            Step::RemoveUserRegKey { path, .. } => format!("HKCU/{path}"),
            _ => String::new(),
        })
        .collect()
}
fn cleanup(r: &Roots) {
    let _ = std::fs::remove_dir_all(r.roaming.parent().unwrap());
    if r.reg.starts_with("Software\\u3d-prefs-test-") {
        let _ = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(&r.reg);
    }
}

#[test]
fn blender_settings_go_per_series_and_stay_while_that_series_is_installed() {
    let r = roots("blender");
    let b = r.roaming.join("Blender Foundation").join("Blender");
    let s45 = mk(b.join("4.5"));
    mk(b.join("5.2"));
    let one = app("blender:4.5", "Blender 4.5", "4.5.14", None, blender("4.5", BlenderKind::Msi));
    let other = app("blender:5.2", "Blender 5.2", "5.2.2", None, blender("5.2", BlenderKind::Msi));
    assert_eq!(targets(&steps(&one, &[other.clone()], &r)), vec![s(&s45)]);
    // A 4.5 beta copy still uses the 4.5 settings.
    let beta = app("blender:4.5-beta", "Blender 4.5 Beta", "4.5.15", None, blender("4.5", BlenderKind::Beta));
    assert!(steps(&one, &[other, beta], &r).is_empty());
    cleanup(&r);
}

#[test]
fn godot_never_touches_keystores_or_game_data() {
    let r = roots("godot");
    let g = r.roaming.join("Godot");
    let settings = touch(g.join("editor_settings-4.7.tres"));
    touch(g.join("editor_settings-4.6.tres"));
    let tpl = mk(g.join("export_templates").join("4.7.2.stable"));
    mk(g.join("export_templates").join("4.6.1.stable"));
    mk(g.join("keystores"));
    mk(g.join("app_userdata").join("My Game"));
    let profiles = mk(g.join("feature_profiles"));
    let projects = touch(g.join("projects.cfg"));
    let cache = mk(r.local.join("Godot"));
    let me = app("tools:godot:4.7.2", "Godot 4.7.2", "4.7.2", Some(PathBuf::from("C:/x/Godot 4.7.2")), tools("godot", "4.7.2"));
    let older = app("tools:godot:4.6.1", "Godot 4.6.1", "4.6.1", None, tools("godot", "4.6.1"));
    // Another Godot stays: only this version's settings and export templates go.
    let t = targets(&steps(&me, &[older], &r));
    assert_eq!(t.len(), 2, "{t:?}");
    assert!(t.contains(&s(&tpl)) && t.contains(&s(&settings)), "{t:?}");
    // The last Godot: the shared editor folders too, still never keystores or game data.
    let t = targets(&steps(&me, &[], &r));
    for p in [&tpl, &settings, &profiles, &projects, &cache] {
        assert!(t.contains(&s(p)), "{} missing from {t:?}", s(p));
    }
    assert!(!t.iter().any(|x| x.contains("keystores") || x.contains("app_userdata") || x.ends_with("4.6.tres") || x.contains("4.6.1")), "{t:?}");
    cleanup(&r);
}

#[test]
fn unity_preferences_go_only_with_the_last_editor() {
    let r = roots("unity");
    mk(r.roaming.join("Unity").join("Asset Store-5.x"));
    let me = app("tools:unity:6000.3.27f1", "Unity 6000.3.27f1", "6000.3.27f1", None, tools("unity", "6000.3.27f1"));
    let other = app("tools:unity:2022.3.62f3", "Unity 2022.3.62f3", "2022.3.62f3", None, tools("unity", "2022.3.62f3"));
    assert!(steps(&me, &[other], &r).is_empty());
    let t = targets(&steps(&me, &[], &r));
    assert!(t.iter().any(|x| x.ends_with("Asset Store-5.x")), "{t:?}");
    assert!(t.contains(&"HKCU/Software\\Unity Technologies\\Unity Editor 5.x".to_string()), "{t:?}");
    cleanup(&r);
}

#[test]
fn adobe_settings_follow_the_install_folder_name_and_creative_cloud_has_none() {
    let r = roots("adobe");
    let ps = mk(r.roaming.join("Adobe").join("Adobe Photoshop 2026"));
    mk(r.roaming.join("Adobe").join("Adobe Photoshop 2021"));
    mk(r.roaming.join("Adobe").join("CameraRaw"));
    let adobe = |sap: &str| SourceData::Adobe { sap: sap.into(), codex: String::new(), platform: "win64".into(), product_name: String::new() };
    let me = app("adobe:PHSP:27", "Photoshop 2026", "27.1", Some(PathBuf::from("C:/Program Files/Adobe/Adobe Photoshop 2026")), adobe("PHSP"));
    assert_eq!(targets(&steps(&me, &[], &r)), vec![s(&ps)]);
    let cc = app("adobe:KCCC:6", "Creative Cloud", "6.11", Some(PathBuf::from("C:/Program Files/Adobe/Adobe Creative Cloud")), adobe("KCCC"));
    assert!(steps(&cc, &[], &r).is_empty());
    cleanup(&r);
}

#[test]
fn craft_and_obs_settings_and_add_ons() {
    let r = roots("craft");
    let pc = mk(r.roaming.join("Photocraft"));
    let launcher = mk(r.roaming.join("ai.storyteller.artcraft-launcher"));
    let me = app("craft:photocraft", "PhotoCraft", "0.12.0", None, SourceData::Craft { key: "photocraft".into(), msi_product_code: None });
    // Windows paths ignore case: "PhotoCraft" is the "Photocraft" folder.
    assert_eq!(targets(&steps(&me, &[], &r)).iter().map(|t| t.to_lowercase()).collect::<Vec<_>>(), vec![s(&pc).to_lowercase()]);
    let la = app("craft:artcraft-launcher", "ArtCraft Launcher", "0.6.1", None, SourceData::Craft { key: "artcraft-launcher".into(), msi_product_code: None });
    assert_eq!(targets(&steps(&la, &[], &r)), vec![s(&launcher)]);
    let obs_cfg = mk(r.roaming.join("obs-studio"));
    let obs_plugins = mk(r.program_data.join("obs-studio"));
    let obs = app("tools:obs", "OBS Studio", "32.2.2", None, tools("obs", "32.2.2"));
    let st = steps(&obs, &[], &r);
    assert_eq!(targets(&st), vec![s(&obs_cfg), s(&obs_plugins)]);
    assert!(matches!(&st[1], Step::RemoveDir { elevated: true, .. }));
    cleanup(&r);
}

#[test]
fn the_dialog_lists_what_goes_with_sizes() {
    let r = roots("preview");
    let cfg = mk(r.roaming.join("obs-studio"));
    std::fs::write(cfg.join("global.ini"), vec![b'x'; 1000]).unwrap();
    std::fs::create_dir_all(cfg.join("plugin_config")).unwrap();
    std::fs::write(cfg.join("plugin_config").join("a.json"), vec![b'x'; 500]).unwrap();
    let obs = app("tools:obs", "OBS Studio", "32.2.2", None, tools("obs", "32.2.2"));
    let items = preview(&steps(&obs, &[], &r));
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].size, Some(1500));
    assert!(items[0].path.ends_with("obs-studio") && !items[0].registry);
    // A registry key is listed without a size.
    let reg = preview(&[Step::RemoveUserRegKey { path: r"Software\Unity Technologies\Unity Editor 5.x".into(), label: String::new() }]);
    assert_eq!(reg[0].path, r"HKCU\Software\Unity Technologies\Unity Editor 5.x");
    assert!(reg[0].registry && reg[0].size.is_none());
    cleanup(&r);
}

#[test]
fn other_app_folder_names_are_the_apps_own() {
    assert_eq!(other_names("7-Zip 26.04 (x64)", "7zip.7zip"), vec!["7-Zip", "7zip"]);
    assert_eq!(other_names("LM Studio 0.4.14+4", "ElementLabs.LMStudio"), vec!["LM Studio", "LMStudio", "lm-studio"]);
    assert_eq!(other_names("Cursor (User)", "Anysphere.Cursor"), vec!["Cursor"]);
    // Generic or too short names are never used.
    assert!(other_names("Google Chrome", "Google.Chrome.EXE").iter().all(|n| !n.eq_ignore_ascii_case("google")));
    assert!(other_names("Go 1.23", "GoLang.Go").iter().all(|n| n.chars().count() >= 3));
    assert!(other_names("Microsoft Teams", "Microsoft.Teams").iter().all(|n| !n.eq_ignore_ascii_case("microsoft")));
}

#[test]
fn other_apps_delete_their_own_folders_only() {
    let r = roots("other");
    let roaming_discord = mk(r.roaming.join("discord"));
    let install = mk(r.local.join("Discord"));
    mk(r.local.join("Google"));
    let mut e = crate::winutil::UninstallEntry::default();
    e.publisher = "Discord Inc.".into();
    let mut d = app("other:Discord.Discord", "Discord", "1.0.9", Some(install.clone()), SourceData::Other { pkg: "Discord.Discord".into(), how: crate::sources::other::How::ProductCode, entry: Box::new(e), upgrade_code: None });
    d.source = Source::Other;
    let t = targets(&steps(&d, &[], &r));
    // Its settings folder goes; the folder its uninstaller handles (it holds the app) and other
    // publishers' folders stay.
    assert_eq!(t.iter().map(|x| x.to_lowercase()).collect::<Vec<_>>(), vec![s(&roaming_discord).to_lowercase()]);
    cleanup(&r);
}

#[test]
fn deleting_really_removes_them_and_keeps_the_rest() {
    let r = roots("run");
    let g = r.roaming.join("Godot");
    touch(g.join("editor_settings-4.7.tres"));
    mk(g.join("export_templates").join("4.7.2.stable"));
    let keys = touch(g.join("keystores").join("release.keystore"));
    let save = touch(g.join("app_userdata").join("My Game").join("save.dat"));
    let me = app("tools:godot:4.7.2", "Godot 4.7.2", "4.7.2", Some(PathBuf::from("C:/x/Godot 4.7.2")), tools("godot", "4.7.2"));
    let st = steps(&me, &[], &r);
    assert!(!st.is_empty());
    for step in &st {
        let res = crate::elevate::execute(step);
        assert!(res.ok, "{}", res.message);
    }
    assert!(!g.join("editor_settings-4.7.tres").exists() && !g.join("export_templates").join("4.7.2.stable").exists());
    assert!(keys.exists() && save.exists(), "keystores and game data are never deleted");
    assert!(steps(&me, &[], &r).iter().all(|st| !matches!(st, Step::RemoveFile { path } if path.ends_with("editor_settings-4.7.tres"))), "nothing left to delete");
    cleanup(&r);
}

#[test]
fn a_folder_two_apps_share_is_not_deleted() {
    let r = roaming_python();
    let py = |ver: &str| {
        let mut e = crate::winutil::UninstallEntry::default();
        e.publisher = "Python Software Foundation".into();
        let mut a = app(&format!("other:Python.Python.{ver}"), &format!("Python {ver}.11 (64-bit)"), ver, None, SourceData::Other { pkg: format!("Python.Python.{ver}"), how: crate::sources::other::How::ProductCode, entry: Box::new(e), upgrade_code: None });
        a.source = Source::Other;
        a
    };
    let (p310, p312) = (py("3.10"), py("3.12"));
    // "AppData\Roaming\Python" holds every version's packages: removing 3.10 leaves it.
    assert!(steps(&p310, &[p312.clone()], &r.0).is_empty());
    // With only one Python left, it is that one's own.
    assert_eq!(targets(&steps(&p312, &[], &r.0)).len(), 1);
    cleanup(&r.0);
}

fn roaming_python() -> (Roots,) {
    let r = roots("python");
    mk(r.roaming.join("Python").join("Python310"));
    (r,)
}

/// An Other app with this package id, name, publisher, install folder and program.
fn other_app(pkg: &str, name: &str, publisher: &str, dir: Option<PathBuf>, exe: Option<PathBuf>) -> Installed {
    let mut e = crate::winutil::UninstallEntry::default();
    e.publisher = publisher.into();
    e.display_name = name.into();
    let mut a = app(&format!("other:{pkg}"), name, "1.0", dir, SourceData::Other { pkg: pkg.into(), how: crate::sources::other::How::ProductCode, entry: Box::new(e), upgrade_code: None });
    a.source = Source::Other;
    a.exe = exe;
    a
}
fn lower(t: Vec<String>) -> Vec<String> {
    t.into_iter().map(|x| x.to_lowercase()).collect()
}
fn reg_key(path: &str) {
    winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).create_subkey(path).unwrap();
}

#[test]
fn cline_keeps_settings_in_a_home_dot_folder_an_app_id_folder_and_the_registry() {
    let r = roots("cline");
    let install = mk(r.local.join("Cline"));
    let dot = mk(r.home.join(".cline").join("data"));
    let id_dir = mk(r.local.join("bot.cline.app").join("EBWebView"));
    mk(r.home.join(".clinex"));
    reg_key(&format!("{}\\cline\\Cline", r.reg));
    let c = other_app("Cline.ClineDesktop", "Cline", "cline", Some(install.clone()), Some(install.join("cline-app.exe")));
    let t = lower(targets(&steps(&c, &[], &r)));
    let want = [s(id_dir.parent().unwrap()), s(dot.parent().unwrap()), format!("HKCU/{}\\cline", r.reg)];
    assert_eq!(t, want.iter().map(|x| x.to_lowercase()).collect::<Vec<_>>());
    // The install folder is the uninstaller's to remove.
    assert!(!t.contains(&s(&install).to_lowercase()));
    cleanup(&r);
}

#[test]
fn updater_major_version_vendor_and_program_data_folders() {
    let r = roots("kinds");
    let updater = mk(r.local.join("lm-studio-updater"));
    let lm = other_app("ElementLabs.LMStudio", "LM Studio 0.4.14", "Element Labs Inc.", None, None);
    assert_eq!(targets(&steps(&lm, &[], &r)), vec![s(&updater)]);
    let wiz3 = mk(r.roaming.join("WizTree3"));
    let wiz = other_app("AntibodySoftware.WizTree", "WizTree", "Antibody Software", None, None);
    assert_eq!(targets(&steps(&wiz, &[], &r)), vec![s(&wiz3)]);
    // Under the publisher's folder, including a shared vendor's one, only the app's subfolder.
    let speccy = mk(r.roaming.join("Piriform").join("Speccy"));
    mk(r.roaming.join("Piriform").join("CCleaner"));
    let sp = other_app("Piriform.Speccy", "Speccy", "Piriform Software Ltd", None, None);
    assert_eq!(targets(&steps(&sp, &[], &r)), vec![s(&speccy)]);
    let teams = mk(r.local.join("Microsoft").join("PowerToys"));
    let pt = other_app("Microsoft.PowerToys", "PowerToys (Preview)", "Microsoft Corporation", None, None);
    assert_eq!(targets(&steps(&pt, &[], &r)), vec![s(&teams)]);
    // ProgramData needs admin rights; LocalLow is per user.
    let pd = mk(r.program_data.join("Windhawk"));
    let low = mk(r.local_low.join("Windhawk"));
    let wh = other_app("RamenSoftware.Windhawk", "Windhawk", "Ramen Software", None, None);
    let st = steps(&wh, &[], &r);
    assert_eq!(targets(&st), vec![s(&low), s(&pd)]);
    assert!(matches!(&st[1], Step::RemoveDir { elevated: true, .. }));
    cleanup(&r);
}

#[test]
fn short_and_generic_names_match_only_whole_folder_names() {
    let r = roots("short");
    let code = mk(r.roaming.join("Code"));
    for other in ["Codex", "CodeBlocks", "Code Cache", "vscode-shared", "com.code.helper"] {
        mk(r.roaming.join(other));
    }
    mk(r.home.join(".codex"));
    mk(r.roaming.join("Microsoft"));
    let vs = other_app("Microsoft.VisualStudioCode", "Microsoft Visual Studio Code", "Microsoft Corporation", Some(PathBuf::from("C:/Programs/Microsoft VS Code")), Some(PathBuf::from("C:/Programs/Microsoft VS Code/Code.exe")));
    assert_eq!(targets(&steps(&vs, &[], &r)), vec![s(&code)]);
    // Names under three letters, and words such as "Launcher" or "Server", are never used.
    mk(r.roaming.join("Go"));
    mk(r.roaming.join("Launcher"));
    mk(r.roaming.join("Server"));
    let go = other_app("GoLang.Go", "Go 1.23", "Google", None, None);
    let launcher = other_app("RockstarGames.Launcher", "Launcher", "Example", None, None);
    let server = other_app("Jellyfin.Server", "Server", "Example", None, None);
    for a in [go, launcher, server] {
        assert!(steps(&a, &[], &r).is_empty(), "{}", a.name);
    }
    // An app id folder matches only when the name is its last real part.
    mk(r.local.join("com.epicgames.eglv2"));
    let epic = other_app("Epic.Games", "Epic Games", "Epic", None, None);
    assert!(steps(&epic, &[], &r).is_empty());
    cleanup(&r);
}

#[test]
fn a_program_or_install_folder_named_after_the_vendor_never_takes_the_vendors_folder() {
    let r = roots("vendor");
    // "Maxon.exe" of Maxon's app manager: "AppData\Roaming\Maxon" holds every Maxon product.
    mk(r.roaming.join("Maxon").join("Cinema 4D"));
    let own = mk(r.local.join("MaxonApp"));
    let mx = other_app("Maxon.MaxonApp", "Maxon App Installer", "Maxon Computer GmbH", None, Some(PathBuf::from("C:/Program Files/Maxon/App Manager/Maxon.exe")));
    assert_eq!(targets(&steps(&mx, &[], &r)), vec![s(&own)]);
    // "Jellyfin\Server": the server's own subfolder, not all of "Jellyfin".
    let server = mk(r.program_data.join("Jellyfin").join("Server"));
    mk(r.program_data.join("Jellyfin").join("Other"));
    let js = other_app("Jellyfin.Server", "Jellyfin Server", "Jellyfin Project", Some(PathBuf::from("C:/Program Files/Jellyfin/Server")), None);
    assert_eq!(targets(&steps(&js, &[], &r)), vec![s(&server)]);
    cleanup(&r);
}

#[test]
fn never_another_apps_folder_a_program_folder_or_a_browser_profile() {
    let mut r = roots("guards");
    // Two installed apps with the same name keep the folder.
    mk(r.roaming.join("Jelly"));
    let a = other_app("Jelly.Player", "Jelly", "Jelly", None, None);
    let b = other_app("Jelly.Server", "Jelly", "Jelly", None, None);
    assert!(steps(&a, &[b], &r).is_empty());
    // A folder holding any program in the uninstall list (not only the apps shown here) stays.
    let held = mk(r.local.join("Tidy"));
    r.install_dirs.push(held.join("app-1.2"));
    let tidy = other_app("Tidy.Tidy", "Tidy", "Tidy", None, None);
    assert!(steps(&tidy, &[], &r).is_empty());
    // Browser profiles (passwords, bookmarks) are the user's own.
    let ff = mk(r.roaming.join("Mozilla").join("Firefox"));
    touch(ff.join("profiles.ini"));
    let cache = mk(r.local.join("Mozilla").join("Firefox"));
    let fox = other_app("Mozilla.Firefox", "Mozilla Firefox (en-US)", "Mozilla", None, None);
    assert_eq!(targets(&steps(&fox, &[], &r)), vec![s(&cache)]);
    mk(r.local.join("Google").join("Chrome").join("User Data").join("Default"));
    let chrome = other_app("Google.Chrome.EXE", "Google Chrome", "Google LLC", None, None);
    assert!(steps(&chrome, &[], &r).is_empty());
    cleanup(&r);
}

#[test]
fn opera_keeps_its_profile_in_its_own_folder_and_that_stays() {
    // Seen in a test install: Opera's passwords and bookmarks are in "Roaming\Opera Software\
    // Opera Stable" itself (a Chromium profile: "Local State" and "Default"), its cache in Local.
    let r = roots("opera");
    let profile = mk(r.roaming.join("Opera Software").join("Opera Stable"));
    touch(profile.join("Local State"));
    touch(profile.join("Default").join("Bookmarks"));
    let cache = mk(r.local.join("Opera Software").join("Opera Stable"));
    // An Electron app's folder has "Local State" too, but no "Default" profile.
    let opera = other_app("Opera.Opera", "Opera Stable", "Opera Software", None, None);
    assert_eq!(targets(&steps(&opera, &[], &r)), vec![s(&cache)]);
    let electron = mk(r.roaming.join("obsidian"));
    touch(electron.join("Local State"));
    mk(electron.join("Local Storage"));
    let obs = other_app("Obsidian.Obsidian", "Obsidian", "Obsidian", None, None);
    assert_eq!(targets(&steps(&obs, &[], &r)), vec![s(&electron)]);
    cleanup(&r);
}

#[test]
fn publisher_domains_software_folders_and_id_suffixes_seen_in_test_installs() {
    let r = roots("seen");
    // Qt apps name the key after the publisher's web domain.
    reg_key(&format!("{}\\jellyfin.org\\Jellyfin Media Player", r.reg));
    reg_key(&format!("{}\\jellyfin.org\\Jellyfin Server", r.reg));
    let jmp = other_app("Jellyfin.JellyfinMediaPlayer", "Jellyfin Media Player", "Jellyfin Project", None, None);
    assert_eq!(targets(&steps(&jmp, &[], &r)), vec![format!("HKCU/{}\\jellyfin.org\\Jellyfin Media Player", r.reg)]);
    // Flutter apps: "io.appflowy\AppFlowy", the app's name as the domain.
    let flowy = mk(r.roaming.join("io.appflowy").join("AppFlowy"));
    mk(r.roaming.join("io.appflowy").join("Other"));
    let af = other_app("AppFlowy.AppFlowy", "AppFlowy", "AppFlowy-IO", None, None);
    assert_eq!(targets(&steps(&af, &[], &r)), vec![s(&flowy)]);
    // Never a domain of a generic word, nor a name with dots that is not a domain.
    reg_key(&format!("{}\\microsoft.com\\Edge", r.reg));
    let edge = other_app("Contoso.Edge", "Edge", "Microsoft", None, None);
    assert!(targets(&steps(&edge, &[], &r)).iter().all(|t| !t.contains("microsoft.com")));
    // "CPUID" keeps its apps in "CPUID Software"; another app of it keeps its own.
    let cpuz = mk(r.program_data.join("CPUID Software").join("cpu-z"));
    let hwm = mk(r.program_data.join("CPUID Software").join("hwmonitor"));
    let z = other_app("CPUID.CPU-Z", "CPUID CPU-Z", "CPUID, Inc.", None, None);
    assert_eq!(targets(&steps(&z, &[], &r)), vec![s(&cpuz)]);
    assert!(!targets(&steps(&z, &[], &r)).contains(&s(&hwm)));
    // Waterfox's update folder: its name with an id after it.
    let wf = mk(r.program_data.join("Waterfox-1de4eec8-1241-4177-a864-e594e8d1fb38"));
    mk(r.program_data.join("Waterfoxy-1de4eec8-1241-4177-a864-e594e8d1fb38"));
    mk(r.program_data.join("Waterfox-notanid"));
    let w = other_app("Waterfox.Waterfox", "Waterfox", "BrowserWorks", None, None);
    assert_eq!(targets(&steps(&w, &[], &r)), vec![s(&wf)]);
    // Where .NET keeps a program's user settings: "<program>.exe_Url_<hash>".
    let net = mk(r.local.join("Atlassian").join("SourceTree.exe_Url_pvbhzmuknxzic1u2pbbpoooc2yeizzcm"));
    let own = mk(r.local.join("Atlassian").join("SourceTree"));
    mk(r.local.join("Atlassian").join("SourceTree.exe_Url_short"));
    mk(r.local.join("Atlassian").join("Jira.exe_Url_pvbhzmuknxzic1u2pbbpoooc2yeizzcm"));
    let st = other_app("Atlassian.Sourcetree", "Sourcetree", "Atlassian", None, Some(PathBuf::from("C:\\x\\SourceTree.exe")));
    let mut t = targets(&steps(&st, &[], &r));
    t.sort();
    assert_eq!(t, vec![s(&own), s(&net)]);
    // "calibre-cache", and a version after the name in the publisher's key.
    let cc = mk(r.local.join("calibre-cache"));
    let cal = other_app("calibre.calibre", "calibre 64bit", "Kovid Goyal", None, None);
    assert_eq!(targets(&steps(&cal, &[], &r)), vec![s(&cc)]);
    reg_key(&format!("{}\\The Document Foundation\\LibreOffice 26.8", r.reg));
    reg_key(&format!("{}\\The Document Foundation\\Other 26.8", r.reg));
    let lo = other_app("TheDocumentFoundation.LibreOffice", "LibreOffice 26.8.1.1", "The Document Foundation", None, None);
    assert_eq!(targets(&steps(&lo, &[], &r)), vec![format!("HKCU/{}\\The Document Foundation\\LibreOffice 26.8", r.reg)]);
    // A publisher named by its web site: "MediaArea.net" keeps "MediaArea".
    reg_key(&format!("{}\\MediaArea\\MediaInfo", r.reg));
    let mi = other_app("MediaArea.MediaInfo.GUI", "MediaInfo", "MediaArea.net", None, None);
    assert_eq!(targets(&steps(&mi, &[], &r)), vec![format!("HKCU/{}\\MediaArea\\MediaInfo", r.reg)]);
    cleanup(&r);
}

#[test]
fn more_folder_and_key_names_seen_in_test_installs() {
    let r = roots("seen2");
    // "foobar2000-v2": a major version after a "v".
    let fb = mk(r.roaming.join("foobar2000-v2"));
    mk(r.roaming.join("foobar2000-vista"));
    let f = other_app("PeterPawlowski.foobar2000", "foobar2000", "Peter Pawlowski", None, None);
    assert_eq!(targets(&steps(&f, &[], &r)), vec![s(&fb)]);
    // A Tauri app id from the app's web domain, and the publisher's whole name as Qt keeps it.
    let jan = mk(r.local.join("jan.ai.app"));
    mk(r.local.join("jan.ai.notes"));
    reg_key(&format!("{}\\Menlo Research Pte. Ltd.\\Jan", r.reg));
    reg_key(&format!("{}\\Menlo Research Pte. Ltd.\\Other", r.reg));
    let j = other_app("Jan.Jan", "Jan", "Menlo Research Pte. Ltd.", None, None);
    assert_eq!(targets(&steps(&j, &[], &r)), vec![s(&jan), format!("HKCU/{}\\Menlo Research Pte. Ltd.\\Jan", r.reg)]);
    // The maker in the package id: "Perplexity.Comet" of "The Comet Authors".
    reg_key(&format!("{}\\Perplexity\\Comet", r.reg));
    reg_key(&format!("{}\\Perplexity\\Update", r.reg));
    let c = other_app("Perplexity.Comet", "Comet", "The Comet Authors", None, None);
    assert_eq!(targets(&steps(&c, &[], &r)), vec![format!("HKCU/{}\\Perplexity\\Comet", r.reg)]);
    // The maker never hides a program of that name: "GeoGebra.exe" keeps "Roaming\GeoGebra".
    let gg = mk(r.roaming.join("GeoGebra"));
    let g = other_app("GeoGebra.Classic", "GeoGebra Classic", "International GeoGebra Institute", None, Some(PathBuf::from("C:\\x\\GeoGebra.exe")));
    assert_eq!(targets(&steps(&g, &[], &r)), vec![s(&gg)]);
    // An Electron updater with a word after the name, and the Roaming folder of that name.
    let upd = mk(r.local.join("tutanota-desktop-updater"));
    let data = mk(r.roaming.join("tutanota-desktop"));
    mk(r.roaming.join("tutanota-other"));
    let t = other_app("Tutanota.Tutanota", "Tuta Mail", "Tutao GmbH", None, None);
    assert_eq!(targets(&steps(&t, &[], &r)), vec![s(&upd), s(&data)]);
    let ev = mk(r.program_data.join("evernote-client-updater"));
    mk(r.program_data.join("evernote-thing-updater"));
    let e = other_app("Evernote.Evernote", "Evernote", "Bending Spoons US Inc.", None, None);
    assert_eq!(targets(&steps(&e, &[], &r)), vec![s(&ev)]);
    // More than one id word between the name and "updater".
    let th = mk(r.local.join("theia-ide-electron-app-updater"));
    mk(r.local.join("theia-ide-plugin-updater"));
    let ti = other_app("EclipseFoundation.TheiaIDE", "Theia IDE", "Eclipse Theia", None, None);
    assert_eq!(targets(&steps(&ti, &[], &r)), vec![s(&th)]);
    cleanup(&r);
}

#[test]
fn an_apps_full_name_in_another_vendors_folder_is_its_own() {
    let r = roots("fullname");
    // Seen: Stream Deck (now published by Corsair) keeps "Elgato\StreamDeck", MEGAsync "Mega
    // Limited\MEGAsync".
    let sd = mk(r.roaming.join("Elgato").join("StreamDeck"));
    let vc = mk(r.roaming.join("Elgato").join("Volume Controller"));
    reg_key(&format!("{}\\Elgato Systems GmbH\\StreamDeck", r.reg));
    let deck = other_app("Elgato.StreamDeck", "Stream Deck", "Corsair Memory, Inc.", None, None);
    assert_eq!(targets(&steps(&deck, &[], &r)), vec![s(&sd), format!("HKCU/{}\\Elgato Systems GmbH\\StreamDeck", r.reg)]);
    assert!(vc.is_dir());
    // Short or generic names never match in a folder that is not the publisher's.
    mk(r.local.join("Vendor").join("Steam"));
    mk(r.local.join("Microsoft").join("MEGAsync"));
    let mega = mk(r.local.join("Mega Limited").join("MEGAsync"));
    let m = other_app("Mega.MEGASync", "MEGAsync", "Mega Limited", None, None);
    assert_eq!(targets(&steps(&m, &[], &r)), vec![s(&mega)]);
    let steam = other_app("Valve.Steam", "Steam", "Valve Corporation", None, None);
    assert!(steps(&steam, &[], &r).is_empty());
    // Nothing inside a folder another installed app takes whole.
    let deckpro = mk(r.roaming.join("StreamDeckPro"));
    mk(deckpro.join("StreamDeck"));
    let pro = other_app("Elgato.StreamDeckPro", "StreamDeckPro", "Elgato", None, None);
    let t = targets(&steps(&deck, &[pro.clone()], &r));
    assert!(t.iter().all(|x| !x.contains("StreamDeckPro")), "{t:?}");
    assert_eq!(targets(&steps(&pro, &[deck], &r)), vec![s(&deckpro)]);
    cleanup(&r);
}

#[test]
fn the_table_of_known_places_adds_what_names_do_not_find() {
    let r = roots("known");
    let itunes = mk(r.roaming.join("Apple Computer").join("iTunes"));
    let shared = mk(r.roaming.join("Apple Computer").join("Preferences"));
    let local = mk(r.local.join("Apple Computer").join("iTunes"));
    reg_key(&format!("{}\\Apple Computer, Inc.\\iTunes", r.reg));
    let a = other_app("Apple.iTunes", "iTunes", "Apple Inc.", None, None);
    let t = targets(&steps(&a, &[], &r));
    assert_eq!(t, vec![s(&itunes), s(&local), format!("HKCU/{}\\Apple Computer, Inc.\\iTunes", r.reg)]);
    assert!(!t.contains(&s(&shared)));
    // A place in the table that holds a program stays.
    let mut r2 = roots("known2");
    let held = mk(r2.local.join("zen"));
    r2.install_dirs.push(held.join("app"));
    let z = other_app("Zen-Team.Zen-Browser", "Zen Browser", "Zen Team", None, None);
    assert!(steps(&z, &[], &r2).is_empty());
    // "IdeaIC*" is the name with a version after it, and nothing while a second copy is installed.
    let r3 = roots("known3");
    let v1 = mk(r3.roaming.join("JetBrains").join("IdeaIC2025.2"));
    let v2 = mk(r3.local.join("JetBrains").join("IdeaIC2024.3.1"));
    mk(r3.roaming.join("JetBrains").join("IdeaICx2025"));
    mk(r3.roaming.join("JetBrains").join("IntelliJIdea2025.2"));
    let ce = other_app("JetBrains.IntelliJIDEA.Community", "IntelliJ IDEA Community Edition", "JetBrains s.r.o.", None, None);
    assert_eq!(targets(&steps(&ce, &[], &r3)), vec![s(&v1), s(&v2)]);
    let mut copy = ce.clone();
    copy.id = "other:JetBrains.IntelliJIDEA.Community:user".into();
    assert!(steps(&ce, &[copy], &r3).is_empty());
    let pce = other_app("JetBrains.PyCharm.Community", "PyCharm Community Edition", "JetBrains s.r.o.", None, None);
    let own = mk(r3.roaming.join("JetBrains").join("PyCharmCE2025.2"));
    mk(r3.roaming.join("JetBrains").join("PyCharm2025.2"));
    assert_eq!(targets(&steps(&pce, &[], &r3)), vec![s(&own)]);
    cleanup(&r3);
    // A place may be a file of its own.
    let r4 = roots("known4");
    let rc = touch(r4.local.join("kdenliverc"));
    let kd = other_app("KDE.Kdenlive", "Kdenlive", "KDE e.V.", None, None);
    assert!(targets(&steps(&kd, &[], &r4)).contains(&s(&rc)));
    cleanup(&r4);
    // Every place in the table starts with one of the known roots.
    for (_, places) in known::KNOWN {
        for p in *places {
            let root = p.split('\\').next().unwrap();
            assert!(["Roaming", "Local", "LocalLow", "ProgramData", "Home", "HKCU"].contains(&root), "{p}");
            assert!(p.split('\\').count() >= 2, "{p}");
        }
    }
    cleanup(&r);
    cleanup(&r2);
}

#[test]
fn a_dotnet_settings_folder_of_the_program_goes_whatever_company_folder_holds_it() {
    let r = roots("dotnet");
    let own = mk(r.local.join("GARMIN_Corp").join("BaseCamp.exe_Url_urbejbdz42c555ix3xqbitecusqt0tuj"));
    let other = mk(r.local.join("GARMIN_Corp").join("Express.exe_Url_urbejbdz42c555ix3xqbitecusqt0tuj"));
    let a = other_app("Garmin.BaseCamp", "BaseCamp", "Garmin Ltd or its subsidiaries", None, None);
    let t = targets(&steps(&a, &[], &r));
    assert!(t.contains(&s(&own)), "{t:?}");
    assert!(!t.contains(&s(&other)));
    assert!(!t.contains(&s(&r.local.join("GARMIN_Corp"))));
    cleanup(&r);
}

#[test]
fn an_edition_year_is_part_of_the_name() {
    let r = roots("year");
    let own = format!("{}\\SoftMaker Software GmbH\\SoftMaker FreeOffice 2024", r.reg);
    let older = format!("{}\\SoftMaker Software GmbH\\SoftMaker FreeOffice 2021", r.reg);
    reg_key(&own);
    reg_key(&older);
    let a = other_app("SoftMaker.FreeOffice.2024", "SoftMaker FreeOffice 2024", "SoftMaker Software GmbH", None, None);
    assert_eq!(targets(&steps(&a, &[], &r)), vec![format!("HKCU/{own}")]);
    // A version is not a year.
    assert_eq!(edition_year("Inno Setup 6.4.3"), None);
    assert_eq!(edition_year("7-Zip 2600"), None);
    cleanup(&r);
}

#[test]
fn a_home_dot_folder_a_separate_command_line_tool_uses_stays() {
    let mut r = roots("cli");
    mk(r.home.join(".claude"));
    let bin = mk(r.home.join(".local").join("bin"));
    touch(bin.join("claude.exe"));
    r.path_dirs.push(bin.clone());
    let desk = other_app("Anthropic.Claude", "Claude", "Anthropic", Some(r.local.join("AnthropicClaude")), None);
    assert!(steps(&desk, &[], &r).is_empty());
    // The app's own command-line tool does not count: Rustup is the "rustup" command.
    let rustup = mk(r.home.join(".rustup"));
    let cargo_bin = mk(r.home.join(".cargo").join("bin"));
    touch(cargo_bin.join("rustup.exe"));
    r.path_dirs.push(cargo_bin.clone());
    let mut ru = other_app("Rustlang.Rustup", "Rustup: the Rust toolchain installer", "The Rust Project Developers", None, None);
    ru.uninstall = Some(vec![cargo_bin.join("rustup.exe").to_string_lossy().to_string(), "self".into(), "uninstall".into()]);
    assert_eq!(targets(&steps(&ru, &[], &r)), vec![s(&rustup)]);
    cleanup(&r);
}

#[test]
fn a_home_dot_folder_another_installed_app_has_the_command_of_stays() {
    // Seen: Miniconda's "_conda.exe" finds ".conda", which Anaconda (its "Scripts\conda.exe")
    // uses as well when both are installed, though neither is on PATH.
    let r = roots("conda");
    let dot = mk(r.home.join(".conda"));
    let mini_dir = mk(r.program_data.join("miniconda3"));
    let mini = other_app("Anaconda.Miniconda3", "Miniconda3", "Anaconda, Inc.", Some(mini_dir.clone()), Some(mini_dir.join("_conda.exe")));
    assert_eq!(targets(&steps(&mini, &[], &r)), vec![s(&dot)]);
    let ana_dir = mk(r.program_data.join("anaconda3"));
    touch(ana_dir.join("Scripts").join("conda.exe"));
    let ana = other_app("Anaconda.Anaconda3", "Anaconda3", "Anaconda, Inc.", Some(ana_dir.clone()), Some(ana_dir.join("Scripts").join("anaconda.exe")));
    assert!(steps(&mini, &[ana], &r).is_empty());
    // Its own copy of the command does not count.
    touch(mini_dir.join("Scripts").join("conda.exe"));
    assert_eq!(targets(&steps(&mini, &[], &r)), vec![s(&dot)]);
    cleanup(&r);
}

#[test]
fn deleting_a_found_dot_folder_and_registry_key_really_removes_them() {
    let r = roots("realcline");
    let dot = touch(r.home.join(".cline").join("data").join("settings.json"));
    let keep = touch(r.home.join("Documents").join("cline").join("notes.txt"));
    reg_key(&format!("{}\\cline\\Cline", r.reg));
    let c = other_app("Cline.ClineDesktop", "Cline", "cline", None, None);
    let st = steps(&c, &[], &r);
    assert_eq!(st.len(), 2);
    for step in &st {
        let res = match step {
            // The registry step works on the real HKCU: only the test's own key is in it.
            Step::RemoveUserRegKey { path, .. } => {
                assert!(path.starts_with("Software\\u3d-prefs-test-"));
                crate::elevate::execute(step)
            }
            _ => crate::elevate::execute(step),
        };
        assert!(res.ok, "{}", res.message);
    }
    assert!(!dot.exists() && keep.exists());
    assert!(winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).open_subkey(format!("{}\\cline", r.reg)).is_err());
    cleanup(&r);
}

/// Read-only audit of every app installed on this PC: what "Also delete settings and add-ons"
/// would list for it, with sizes, and folders that look related but are not listed. Never
/// deletes anything.
/// cargo test --release --lib live_prefs_audit -- --ignored --nocapture
#[test]
#[ignore]
fn live_prefs_audit() {
    use crate::sources::{adobe, blender, craft, other, tools};
    let s = crate::model::Settings::default();
    let un = crate::winutil::uninstall_entries();
    let mut all = adobe::scan(None, &un);
    all.extend(craft::scan(&un));
    all.extend(blender::scan(&un, &PathBuf::from(&s.blender_install_dir)));
    all.extend(tools::scan(&un, &PathBuf::from(&s.godot_install_dir)));
    let found = other::scan(&un, &all, None);
    all.extend(found);
    all.retain(|i| !i.missing);
    let r = Roots::system();
    let user_dirs: Vec<String> = ["Documents", "Desktop", "Pictures", "Videos", "Music", "Downloads", "OneDrive"]
        .iter()
        .map(|d| format!("{}\\{d}\\",std::env::var("USERPROFILE").unwrap_or_default()).to_ascii_lowercase())
        .collect();
    let installs: Vec<String> = all.iter().filter_map(|i| i.install_dir.as_ref()).map(|d| d.to_string_lossy().to_ascii_lowercase()).collect();
    let mut claimed: Vec<(String, String)> = vec![];
    let (mut zero, mut with) = (vec![], 0);
    for inst in &all {
        let rest: Vec<Installed> = all.iter().filter(|o| o.id != inst.id).cloned().collect();
        let st = steps(inst, &rest, &r);
        let items = preview(&st);
        if items.is_empty() {
            zero.push(inst);
            continue;
        }
        with += 1;
        let total: u64 = items.iter().filter_map(|i| i.size).sum();
        println!("{} [{}] {} MB", inst.name, inst.id, total / 1_000_000);
        for st in &st {
            let p = match st {
                Step::RemoveDir { path, .. } | Step::RemoveFile { path } => path.to_string_lossy().to_ascii_lowercase(),
                Step::RemoveUserRegKey { path, .. } => format!("hkcu\\{}",path.to_ascii_lowercase()),
                _ => continue,
            };
            if user_dirs.iter().any(|u| p.starts_with(u)) {
                println!("  !! USER FOLDER {p}");
            }
            if installs.iter().any(|i| *i == p || i.starts_with(&format!("{p}\\"))) {
                println!("  !! HOLDS AN INSTALL {p}");
            }
            claimed.push((p, inst.id.clone()));
        }
        for i in &items {
            println!("  {} {}", i.path, i.size.map(|s| format!("{} KB", s / 1000)).unwrap_or_default());
        }
    }
    claimed.sort();
    for w in claimed.windows(2) {
        if w[0].0 == w[1].0 {
            println!("!! CLAIMED TWICE {} by {} and {}", w[0].0, w[0].1, w[1].1);
        }
    }
    // Folders whose name contains a word of the app's name, for the apps with nothing found.
    let norm = |t: &str| t.chars().filter(|c| c.is_ascii_alphanumeric()).collect::<String>().to_ascii_lowercase();
    let home = PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default());
    let bases = [r.roaming.clone(), r.local.clone(), r.program_data.clone(), home.join("AppData").join("LocalLow"), home.clone()];
    let listing: Vec<(PathBuf, String)> = bases.iter().flat_map(|b| std::fs::read_dir(b).into_iter().flatten().flatten()).filter(|e| e.path().is_dir()).map(|e| (e.path(), norm(&e.file_name().to_string_lossy()))).collect();
    println!("\n== {} apps, {} with findings, {} with none ==", all.len(), with, zero.len());
    for inst in &zero {
        let words: Vec<String> = inst.name.split(|c: char| !c.is_ascii_alphanumeric()).map(norm).filter(|w| w.len() >= 4 && !w.chars().all(|c| c.is_ascii_digit())).collect();
        let near: Vec<String> = listing.iter().filter(|(_, n)| words.iter().any(|w| n.contains(w.as_str()))).map(|(p, _)| p.to_string_lossy().to_string()).collect();
        println!("none: {} [{}] dir={:?} near={near:?}", inst.name, inst.id, inst.install_dir);
    }
}

/// Run inside a test machine after installing apps: the installed apps as the finder sees them,
/// as JSON in the file U3D_DUMP (read by `sandbox_replay`).
#[test]
#[ignore]
fn sandbox_dump() {
    use crate::sources::{other, tools};
    let un = crate::winutil::uninstall_entries();
    let mut all = tools::scan(&un, &PathBuf::from(&crate::model::Settings::default().godot_install_dir));
    let found = other::scan(&un, &all, None);
    println!("{} uninstall entries, index {:?} {}, {} apps", un.len(), crate::sources::winget::index_path(), crate::sources::winget::index_path().is_file(), all.len() + found.len());
    all.extend(found);
    let p = |d: &Option<PathBuf>| d.as_ref().map(|x| x.to_string_lossy().to_string());
    let rows: Vec<serde_json::Value> = all
        .iter()
        .map(|i| {
            let mut v = serde_json::json!({ "id": i.id, "name": i.name, "version": i.version, "install_dir": p(&i.install_dir), "exe": p(&i.exe), "uninstall": i.uninstall });
            match &i.data {
                SourceData::Other { pkg, entry, .. } => {
                    v["pkg"] = pkg.clone().into();
                    v["display_name"] = entry.display_name.clone().into();
                    v["publisher"] = entry.publisher.clone().into();
                }
                SourceData::Tools { key, series } => {
                    v["tool"] = key.clone().into();
                    v["series"] = series.clone().into();
                }
                _ => {}
            }
            v
        })
        .collect();
    std::fs::write(std::env::var("U3D_DUMP").unwrap(), serde_json::to_string_pretty(&rows).unwrap()).unwrap();
}

/// The finder replayed on what test machines recorded. U3D_REPLAY is a folder with one JSON
/// file per app (any depth): what appeared while it was installed and first run ("new"), every
/// folder and HKCU key two levels down right after ("snap"), the uninstall list ("arpAll") and
/// the installed apps as `sandbox_dump` saw them. Prints what was found and missed per app and
/// writes the list to U3D_REPLAY_OUT.
/// cargo test --release --lib sandbox_replay -- --ignored --nocapture
#[test]
#[ignore]
fn sandbox_replay() {
    use serde_json::Value;
    use std::collections::{BTreeMap, HashMap, HashSet};
    fn walk(d: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "json") {
                out.push(p);
            }
        }
    }
    let mut files = vec![];
    walk(&PathBuf::from(std::env::var("U3D_REPLAY").unwrap()), &mut files);
    let mut groups: BTreeMap<PathBuf, Vec<Value>> = BTreeMap::new();
    for f in files {
        let Ok(raw) = std::fs::read_to_string(&f) else { continue };
        let Ok(v) = serde_json::from_str::<Value>(raw.trim_start_matches('\u{feff}')) else { continue };
        if v.get("snap").is_some() && v.get("id").is_some() {
            groups.entry(f.parent().unwrap().to_path_buf()).or_default().push(v);
        }
    }
    for g in groups.values_mut() {
        g.sort_by_key(|v| v["i"].as_u64().unwrap_or(0));
    }
    // What shows up for many apps is Windows' own doing, not theirs.
    let mut freq: HashMap<String, usize> = HashMap::new();
    for v in groups.values().flatten() {
        for x in v["new"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            *freq.entry(x.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let idx = std::sync::Arc::new(crate::sources::winget::Index::open(&crate::sources::winget::index_path()).ok());
    let strs = |v: &Value| -> Vec<String> { v.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect() };
    let mut report = Vec::new();
    let (mut full, mut part, mut none, mut empty, mut undetected) = (0, 0, 0, 0, 0);
    for (gi, recs) in groups.values().enumerate() {
        let r0 = roots(&format!("replay{gi}"));
        let base = r0.roaming.parent().unwrap().to_path_buf();
        let cu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        let reg = r0.reg.clone();
        let place = |x: &str, make: bool| {
            let Some((tag, rest)) = x.split_once('\\') else { return };
            let dir = match tag {
                "R" => "Roaming",
                "L" => "Local",
                "LL" => "LocalLow",
                "PD" => "ProgramData",
                "H" => "Home",
                "HKCU" => {
                    if make {
                        let _ = cu.create_subkey(format!("{reg}\\{rest}"));
                    } else {
                        let _ = cu.delete_subkey_all(format!("{reg}\\{rest}"));
                    }
                    return;
                }
                _ => return,
            };
            let p = base.join(dir).join(rest.trim_end_matches('\\'));
            match (make, rest.ends_with('\\')) {
                (true, true) => drop(std::fs::create_dir_all(&p)),
                (true, false) => {
                    let _ = std::fs::create_dir_all(p.parent().unwrap());
                    let _ = std::fs::write(&p, b"x");
                }
                (false, true) => drop(std::fs::remove_dir_all(&p)),
                (false, false) => drop(std::fs::remove_file(&p)),
            }
        };
        let mut prev: HashSet<String> = HashSet::new();
        for v in recs {
            let snap: HashSet<String> = strs(&v["snap"]).into_iter().collect();
            for x in prev.difference(&snap) {
                place(x, false);
            }
            for x in snap.difference(&prev) {
                place(x, true);
            }
            prev = snap;
            let s = |k: &str| v[k].as_str().unwrap_or("").to_ascii_lowercase();
            let home = s("home");
            let pairs = [(format!("{home}\\appdata\\locallow"), base.join("LocalLow")), (s("roaming"), base.join("Roaming")), (s("local"), base.join("Local")), (s("pd"), base.join("ProgramData")), (home, base.join("Home"))];
            let map = |p: &str| -> PathBuf {
                let low = p.to_ascii_lowercase();
                for (from, to) in &pairs {
                    if low == *from {
                        return to.clone();
                    }
                    if low.starts_with(&format!("{from}\\")) {
                        return to.join(&p[from.len() + 1..]);
                    }
                }
                PathBuf::from(p)
            };
            let mut install_dirs = Vec::new();
            for e in v["arpAll"].as_array().into_iter().flatten() {
                let g = |k: &str| e[k].as_str().unwrap_or("").to_string();
                if !g("InstallLocation").trim().is_empty() {
                    install_dirs.push(map(g("InstallLocation").trim().trim_matches('"').trim_end_matches('\\')));
                }
                let icon = g("DisplayIcon").split(',').next().unwrap_or("").trim().trim_matches('"').to_string();
                let un = crate::winutil::split_program_line(&g("UninstallString")).into_iter().next().unwrap_or_default();
                for prog in [icon, un] {
                    if Path::new(&prog).is_absolute() {
                        install_dirs.extend(map(&prog).parent().map(Path::to_path_buf));
                    }
                }
            }
            let ix = idx.clone();
            let r = Roots {
                roaming: r0.roaming.clone(),
                local: r0.local.clone(),
                program_data: r0.program_data.clone(),
                local_low: r0.local_low.clone(),
                home: r0.home.clone(),
                reg: r0.reg.clone(),
                install_dirs,
                path_dirs: v["path"].as_str().unwrap_or("").split(';').filter(|x| !x.is_empty()).map(map).collect(),
                moniker: Box::new(move |id: &str| ix.as_ref().as_ref()?.by_id(id)?.moniker.filter(|m| !m.is_empty())),
            };
            // Windows PowerShell writes a list inside an object as {"value": [...], "Count": n}.
            let installed: Vec<Value> = match &v["installed"] {
                Value::Array(a) => a.clone(),
                Value::Object(o) => match o.get("value") {
                    Some(Value::Array(a)) => a.clone(),
                    _ => vec![v["installed"].clone()],
                },
                _ => vec![],
            };
            let apps: Vec<Installed> = installed
                .iter()
                .map(|x| {
                    let g = |k: &str| x[k].as_str().map(str::to_string);
                    let data = match g("tool") {
                        Some(key) => SourceData::Tools { key, series: g("series").unwrap_or_default() },
                        None => {
                            let mut e = crate::winutil::UninstallEntry::default();
                            e.display_name = g("display_name").unwrap_or_default();
                            e.publisher = g("publisher").unwrap_or_default();
                            SourceData::Other { pkg: g("pkg").unwrap_or_default(), how: crate::sources::other::How::ProductCode, entry: Box::new(e), upgrade_code: None }
                        }
                    };
                    let mut a = app(&g("id").unwrap_or_default(), &g("name").unwrap_or_default(), &g("version").unwrap_or_default(), g("install_dir").map(|d| map(&d)), data);
                    a.source = if g("tool").is_some() { Source::Tools } else { Source::Other };
                    a.exe = g("exe").map(|d| map(&d));
                    a.uninstall = x["uninstall"].as_array().map(|u| u.iter().filter_map(Value::as_str).map(|y| map(y).to_string_lossy().to_string()).collect());
                    a
                })
                .collect();
            let rel = |p: &str| -> String {
                let low = p.to_ascii_lowercase();
                for (t, d) in [("R", "Roaming"), ("LL", "LocalLow"), ("L", "Local"), ("PD", "ProgramData"), ("H", "Home")] {
                    let b = base.join(d).to_string_lossy().to_ascii_lowercase();
                    if low.starts_with(&format!("{b}\\")) {
                        return format!("{t}\\{}", &p[b.len() + 1..]);
                    }
                }
                p.to_string()
            };
            let inst_rel: Vec<String> = r.install_dirs.iter().chain(apps.iter().filter_map(|a| a.install_dir.as_ref())).map(|d| format!("{}\\", rel(&d.to_string_lossy()).to_ascii_lowercase())).collect();
            let id = v["id"].as_str().unwrap_or("").to_string();
            let lid = format!("other:{}", id.to_ascii_lowercase());
            let me = apps.iter().find(|a| a.id.to_ascii_lowercase() == lid || a.id.to_ascii_lowercase().starts_with(&format!("{lid}:")));
            // What the app made: its top-most new folders, files and keys, without install
            // folders, Windows' own changes and the user's own folders.
            let new = strs(&v["new"]);
            let low_new: Vec<String> = new.iter().map(|x| x.to_ascii_lowercase()).collect();
            let mut made: Vec<String> = new
                .iter()
                .filter(|x| {
                    let l = x.to_ascii_lowercase();
                    let Some((tag, rest)) = l.split_once('\\') else { return false };
                    let parent_new = rest.trim_end_matches('\\').rsplit_once('\\').is_some_and(|(p, _)| low_new.contains(&format!("{tag}\\{p}\\")) || (tag == "hkcu" && low_new.contains(&format!("{tag}\\{p}"))));
                    let keep_tag = matches!(tag, "r" | "l" | "ll" | "pd" | "hkcu") || (tag == "h" && rest.starts_with('.'));
                    let holds = inst_rel.iter().any(|i| i.starts_with(&l) || (l.ends_with('\\') && *i == l));
                    // Windows' own folders and the shader caches of the graphics driver.
                    // Also what shared frameworks and installers leave (Chromium's and Squirrel's,
                    // WiX#, the installer's own id key) and folders shared by many programs.
                    let windows = [
                        "microsoft", "packages", "temp", "classes", "policies", "wow6432node", "appdatalow", "nvidia", "amd", "intel", "d3dscache", "app.theupdater3d", "chromium",
                        "registeredapplications", "squirreltemp", "programs\\common", "peerdistrepub", "toastnotificationmanagercompat", "fontconfig", ".dbus-keyrings", "wixsharp",
                        "branchio", "google\\common", "google\\update", "crashdumps", "programs", "advinst_logs", "caphyon",
                    ]
                    .iter()
                    .any(|w| rest.trim_end_matches('\\') == *w || rest.starts_with(&format!("{w}\\")))
                        || is_guid(rest.trim_end_matches('\\'));
                    let marker = ["user data\\", "default\\", "profiles.ini", "local state", "login data", "bookmarks"].iter().any(|m| rest.ends_with(m) && rest.matches('\\').count() >= 2);
                    keep_tag && !parent_new && !holds && !windows && !marker && freq.get(&l).copied().unwrap_or(0) < 4
                })
                .cloned()
                .collect();
            made.sort();
            made.dedup();
            let Some(me) = me else {
                undetected += 1;
                println!("{id} NOT DETECTED made={made:?}");
                report.push(serde_json::json!({ "id": id, "detected": false, "made": made }));
                continue;
            };
            let others: Vec<Installed> = apps.iter().filter(|o| o.id != me.id).cloned().collect();
            let got: Vec<String> = steps(me, &others, &r)
                .iter()
                .filter_map(|st| match st {
                    Step::RemoveDir { path, .. } => Some(format!("{}\\", rel(&path.to_string_lossy()))),
                    Step::RemoveFile { path } => Some(rel(&path.to_string_lossy())),
                    Step::RemoveUserRegKey { path, .. } => Some(format!("HKCU\\{}", path.strip_prefix(&format!("{}\\", r.reg)).unwrap_or(path))),
                    _ => None,
                })
                .collect();
            let low_got: Vec<String> = got.iter().map(|g| g.to_ascii_lowercase()).collect();
            let covered = |m: &str| {
                let m = m.to_ascii_lowercase();
                low_got.iter().any(|g| m == *g || m.starts_with(g.as_str()) || m.starts_with(&format!("{g}\\")))
            };
            // A new vendor folder or key counts by what is in it (the finder takes the app's own
            // subfolder only), and a browser profile is kept on purpose.
            let low_snap: HashSet<String> = prev.iter().map(|x| x.to_ascii_lowercase()).collect();
            let profile = |m: &str| {
                let m = m.to_ascii_lowercase();
                m.ends_with('\\') && ["user data\\", "profiles.ini"].iter().any(|k| low_snap.contains(&format!("{m}{k}"))) || (low_snap.contains(&format!("{m}default\\")) && low_snap.contains(&format!("{m}local state")))
            };
            let mut kept: Vec<String> = vec![];
            let mut missed: Vec<String> = vec![];
            for m in &made {
                if covered(m) {
                    continue;
                }
                if profile(m) {
                    kept.push(m.clone());
                    continue;
                }
                let lm = m.to_ascii_lowercase();
                let pre = if lm.starts_with("hkcu") { format!("{lm}\\") } else { lm.clone() };
                let kids: Vec<&String> = new.iter().filter(|n| {
                    let ln = n.to_ascii_lowercase();
                    ln.starts_with(&pre) && ln.len() > pre.len() && !ln[pre.len()..].trim_end_matches('\\').contains('\\')
                }).collect();
                let kids: Vec<&String> = kids.into_iter().filter(|k| !["user data\\", "default\\", "profiles.ini", "local state", "login data", "bookmarks"].iter().any(|x| k.to_ascii_lowercase().ends_with(x))).collect();
                if lm.ends_with('\\') && kids.is_empty() || !lm.ends_with('\\') && !lm.starts_with("hkcu") || kids.is_empty() {
                    missed.push(m.clone());
                    continue;
                }
                for k in kids {
                    if profile(k) {
                        kept.push(k.clone());
                    } else if !covered(k) {
                        missed.push(k.clone());
                    }
                }
            }
            // Found though this app did not make it: made before it, by Windows or another app.
            let extra: Vec<&String> = got
                .iter()
                .filter(|g| {
                    let g = g.to_ascii_lowercase();
                    !low_new.iter().any(|n| *n == g || n.starts_with(&g) || g.starts_with(n.as_str()) || n.starts_with(&format!("{g}\\")))
                })
                .collect();
            let hit = low_new.iter().any(|n| covered(n));
            let status = match (made.len() == kept.len(), missed.is_empty(), hit) {
                (true, _, _) => "nothing",
                (_, true, _) => "full",
                (_, false, true) => "part",
                _ => "none",
            };
            match status {
                "nothing" => empty += 1,
                "full" => full += 1,
                "part" => part += 1,
                _ => none += 1,
            }
            println!("{id} [{}] {status} found={got:?} missed={missed:?} kept={kept:?} extra={extra:?}", me.name);
            let publisher = installed.iter().find(|x| x["id"].as_str() == Some(me.id.as_str())).map(|x| x["publisher"].clone());
            report.push(serde_json::json!({ "id": id, "name": me.name, "detected": true, "status": status, "made": made, "found": got, "missed": missed, "kept": kept, "extra": extra, "publisher": publisher, "exe": me.exe.as_ref().map(|e| rel(&e.to_string_lossy())), "install_dir": me.install_dir.as_ref().map(|e| rel(&e.to_string_lossy())) }));
        }
        cleanup(&r0);
    }
    println!("== apps {} | settings fully found {full}, partly {part}, not found {none}, made nothing {empty}, not detected {undetected}", report.len());
    if let Ok(out) = std::env::var("U3D_REPLAY_OUT") {
        std::fs::write(out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
}
