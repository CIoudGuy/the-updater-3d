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
    Roots { roaming: base.join("Roaming"), local: base.join("Local"), program_data: base.join("ProgramData") }
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
