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

/// The finder replayed on what test machines recorded. U3D_REPLAY is a folder of batches, each
/// with "final.json" (every folder and HKCU key two levels down, the uninstall list), the
/// "installed.json" of `sandbox_dump` and one file per app with what appeared while it was
/// installed and first run. Prints what was found and missed per app; writes U3D_REPLAY_OUT.
/// cargo test --release --lib sandbox_replay -- --ignored --nocapture
#[test]
#[ignore]
fn sandbox_replay() {
    use serde_json::Value;
    let dir = PathBuf::from(std::env::var("U3D_REPLAY").unwrap());
    let idx = std::sync::Arc::new(crate::sources::winget::Index::open(&crate::sources::winget::index_path()).ok());
    let read = |p: &Path| -> Option<Value> { serde_json::from_str(std::fs::read_to_string(p).ok()?.trim_start_matches('\u{feff}')).ok() };
    let mut batches: Vec<PathBuf> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.path()).filter(|p| p.join("final.json").is_file() && p.join("installed.json").is_file()).collect();
    batches.sort();
    let mut per_app: Vec<(PathBuf, Value)> = vec![];
    for b in &batches {
        for e in std::fs::read_dir(b).unwrap().flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.ends_with(".json") && !["final.json", "installed.json"].contains(&n.as_str()) {
                if let Some(v) = read(&e.path()) {
                    per_app.push((b.clone(), v));
                }
            }
        }
    }
    // What shows up for many apps is Windows' own doing, not theirs.
    let mut freq: std::collections::HashMap<String, usize> = Default::default();
    for (_, v) in &per_app {
        for x in v["new"].as_array().into_iter().flatten().filter_map(Value::as_str) {
            *freq.entry(x.to_ascii_lowercase()).or_default() += 1;
        }
    }
    let mut report = Vec::new();
    let (mut full, mut part, mut none, mut empty, mut undetected) = (0, 0, 0, 0, 0);
    for (bi, b) in batches.iter().enumerate() {
        let (Some(fin), Some(inst)) = (read(&b.join("final.json")), read(&b.join("installed.json"))) else { continue };
        let r0 = roots(&format!("replay{bi}"));
        let base = r0.roaming.parent().unwrap().to_path_buf();
        let tag_dir = |t: &str| match t {
            "R" => Some(base.join("Roaming")),
            "L" => Some(base.join("Local")),
            "LL" => Some(base.join("LocalLow")),
            "PD" => Some(base.join("ProgramData")),
            "H" => Some(base.join("Home")),
            _ => None,
        };
        let cu = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER);
        // A list, or a list holding the list.
        let snap: Vec<&Value> = fin["snap"].as_array().unwrap().iter().flat_map(|v| v.as_array().map(|a| a.iter().collect()).unwrap_or_else(|| vec![v])).collect();
        for x in snap.into_iter().filter_map(Value::as_str) {
            let Some((tag, rest)) = x.split_once('\\') else { continue };
            if tag == "HKCU" {
                let _ = cu.create_subkey(format!("{}\\{rest}", r0.reg));
            } else if let Some(t) = tag_dir(tag) {
                if let Some(d) = rest.strip_suffix('\\') {
                    let _ = std::fs::create_dir_all(t.join(d));
                } else {
                    let _ = std::fs::create_dir_all(t.join(rest).parent().unwrap());
                    let _ = std::fs::write(t.join(rest), b"x");
                }
            }
        }
        let s = |k: &str| fin[k].as_str().unwrap_or("").to_ascii_lowercase();
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
        for e in fin["arp"].as_array().unwrap() {
            let g = |k: &str| e[k].as_str().unwrap_or("").to_string();
            if !g("InstallLocation").trim().is_empty() {
                install_dirs.push(map(g("InstallLocation").trim().trim_end_matches('\\')));
            }
            let icon = g("DisplayIcon").split(',').next().unwrap_or("").trim().trim_matches('"').to_string();
            let un = crate::winutil::split_program_line(&g("UninstallString")).into_iter().next().unwrap_or_default();
            for prog in [icon, un] {
                if Path::new(&prog).is_absolute() {
                    install_dirs.extend(map(&prog).parent().map(Path::to_path_buf));
                }
            }
        }
        let path_dirs = fin["path"].as_str().unwrap_or("").split(';').filter(|x| !x.is_empty()).map(map).collect();
        let ix = idx.clone();
        let r = Roots { install_dirs, path_dirs, moniker: Box::new(move |id: &str| ix.as_ref().as_ref()?.by_id(id)?.moniker.filter(|m| !m.is_empty())), ..r0 };
        let apps: Vec<Installed> = inst
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                let g = |k: &str| v[k].as_str().map(str::to_string);
                let data = match (g("pkg"), g("tool")) {
                    (_, Some(key)) => SourceData::Tools { key, series: g("series").unwrap_or_default() },
                    (pkg, None) => {
                        let mut e = crate::winutil::UninstallEntry::default();
                        e.display_name = g("display_name").unwrap_or_default();
                        e.publisher = g("publisher").unwrap_or_default();
                        SourceData::Other { pkg: pkg.unwrap_or_default(), how: crate::sources::other::How::ProductCode, entry: Box::new(e), upgrade_code: None }
                    }
                };
                let mut a = app(&g("id").unwrap_or_default(), &g("name").unwrap_or_default(), &g("version").unwrap_or_default(), g("install_dir").map(|d| map(&d)), data);
                a.source = if g("tool").is_some() { Source::Tools } else { Source::Other };
                a.exe = g("exe").map(|d| map(&d));
                a.uninstall = v["uninstall"].as_array().map(|u| u.iter().filter_map(Value::as_str).map(|x| map(x).to_string_lossy().to_string()).collect());
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
        for (_, v) in per_app.iter().filter(|(pb, _)| pb == b) {
            let id = v["id"].as_str().unwrap_or("").to_string();
            let lid = format!("other:{}", id.to_ascii_lowercase());
            let me = apps.iter().find(|a| a.id.to_ascii_lowercase() == lid || a.id.to_ascii_lowercase().starts_with(&format!("{lid}:")));
            // What the app made: its top-most new folders, files and keys, without install
            // folders, Windows' own changes and the user's own folders.
            let new: Vec<String> = v["new"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_string).collect();
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
                    let windows = ["microsoft", "packages", "temp", "classes", "policies", "wow6432node", "appdatalow", "nvidia", "amd", "intel", "d3dscache"].iter().any(|w| rest == *w || rest.starts_with(&format!("{w}\\"))) || rest.ends_with("user data\\");
                    keep_tag && !parent_new && !holds && !windows && freq.get(&l).copied().unwrap_or(0) < 4
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
            let missed: Vec<&String> = made.iter().filter(|m| !covered(m)).collect();
            // Found but not made by this app (made before it, by Windows or by another app).
            let extra: Vec<&String> = got.iter().filter(|g| {
                let g = g.to_ascii_lowercase();
                !low_new.iter().any(|n| *n == g || n.starts_with(&g) || g.starts_with(n.as_str()) || n.starts_with(&format!("{g}\\")))
            }).collect();
            match (made.is_empty(), missed.len()) {
                (true, _) => empty += 1,
                (false, 0) => full += 1,
                (false, n) if n == made.len() => none += 1,
                _ => part += 1,
            }
            println!("{id} [{}] made={} found={got:?} missed={missed:?} extra={extra:?}", me.name, made.len());
            report.push(serde_json::json!({ "id": id, "name": me.name, "detected": true, "made": made, "found": got, "missed": missed, "extra": extra, "exe": me.exe.as_ref().map(|e| e.to_string_lossy().to_string()), "install_dir": me.install_dir.as_ref().map(|e| e.to_string_lossy().to_string()) }));
        }
        cleanup(&r);
    }
    println!("== apps {} | settings fully found {full}, partly {part}, not found {none}, made nothing {empty}, not detected {undetected}", report.len());
    if let Ok(out) = std::env::var("U3D_REPLAY_OUT") {
        std::fs::write(out, serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }
}
