//! Epic Games Launcher: the games it installed, read from its own manifests (one `*.item` JSON file
//! per install in `ProgramData\Epic\EpicGamesLauncher\Data\Manifests`) and the Unreal Engine
//! versions from `UnrealEngineLauncher\LauncherInstalled.dat`. Epic keeps no marker for a waiting
//! update in these files, so none is reported; the launcher updates its games itself. Nothing that
//! names the Epic account (the ownership token in a manifest) is read or kept.

use super::{decode_text, own_uri, Game, Launcher, NotesFrom};
use std::path::{Path, PathBuf};

/// What one `*.item` manifest says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpicItem {
    pub app_name: String,
    pub name: String,
    pub version: String,
    pub install_location: String,
    pub install_size: u64,
    pub namespace: String,
    pub item_id: String,
    /// The AppName of the game this entry belongs to (an add-on names its game here)
    pub main_game: String,
    pub incomplete: bool,
}

/// A manifest. None when it is not JSON or names no app.
pub fn parse_item(json: &str) -> Option<EpicItem> {
    let v: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()?;
    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
    let app_name = s("AppName");
    if app_name.is_empty() {
        return None;
    }
    Some(EpicItem {
        app_name,
        name: s("DisplayName"),
        version: s("AppVersionString"),
        install_location: s("InstallLocation"),
        install_size: v.get("InstallSize").and_then(|x| x.as_u64()).unwrap_or(0),
        namespace: s("CatalogNamespace"),
        item_id: s("CatalogItemId"),
        main_game: s("MainGameAppName"),
        incomplete: v.get("bIsIncompleteInstall").and_then(|x| x.as_bool()).unwrap_or(false),
    })
}

/// Percent-encodes everything but letters, digits and "-_.~".
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// Where Epic's own library shows the game and its update.
const LIBRARY_URI: &str = "com.epicgames.launcher://library";

/// The game a manifest stands for. None for an unfinished install, an add-on (its main game is
/// another app) and an entry with no name or folder.
pub fn game_of(item: &EpicItem) -> Option<Game> {
    if item.incomplete || item.name.is_empty() || item.install_location.is_empty() {
        return None;
    }
    if !item.main_game.is_empty() && !item.main_game.eq_ignore_ascii_case(&item.app_name) {
        return None;
    }
    let launch = (!item.namespace.is_empty() && !item.item_id.is_empty())
        .then(|| format!("com.epicgames.launcher://apps/{}%3A{}%3A{}?action=launch&silent=true", enc(&item.namespace), enc(&item.item_id), enc(&item.app_name)))
        .filter(|u| own_uri(Launcher::Epic, u));
    Some(Game {
        launcher: Launcher::Epic,
        key: item.app_name.clone(),
        name: item.name.clone(),
        version: item.version.clone(),
        update: None,
        install_dir: super::dir_of(&item.install_location),
        size: Some(item.install_size).filter(|s| *s > 0),
        updated_at: None,
        launch_uri: launch,
        show_uri: Some(LIBRARY_URI.to_string()),
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
    })
}

/// One Unreal Engine install in `LauncherInstalled.dat`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineInstall {
    /// "UE_5.4"
    pub app_name: String,
    pub version: String,
    pub location: String,
}

/// The Unreal Engine installs `LauncherInstalled.dat` lists ("InstallationList"). The games it
/// lists too are read from their manifests instead.
pub fn parse_installed_dat(json: &str) -> Vec<EngineInstall> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json.trim_start_matches('\u{feff}')) else { return vec![] };
    let Some(list) = v.get("InstallationList").and_then(|l| l.as_array()) else { return vec![] };
    list.iter()
        .filter_map(|e| {
            let s = |k: &str| e.get(k).and_then(|x| x.as_str()).unwrap_or("").trim().to_string();
            let app_name = s("AppName");
            (app_name.to_ascii_uppercase().starts_with("UE_")).then(|| EngineInstall { app_name, version: s("AppVersion"), location: s("InstallLocation") })
        })
        .collect()
}

/// The game entry of an engine install ("Unreal Engine 5.4").
pub fn engine_of(e: &EngineInstall) -> Option<Game> {
    let dir = super::dir_of(&e.location)?;
    let number = e.app_name.get(3..).unwrap_or("").trim();
    Some(Game {
        launcher: Launcher::Epic,
        key: e.app_name.clone(),
        name: if number.is_empty() { "Unreal Engine".to_string() } else { format!("Unreal Engine {number}") },
        version: e.version.clone(),
        update: None,
        install_dir: Some(dir),
        size: None,
        updated_at: None,
        launch_uri: None,
        show_uri: Some(LIBRARY_URI.to_string()),
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
    })
}

/// The games and engines under Epic's folder in ProgramData (`<epic>\EpicGamesLauncher\...`).
pub fn scan_in(epic: &Path) -> Vec<Game> {
    let mut out: Vec<Game> = Vec::new();
    let mut push = |g: Game| {
        let there = g.install_dir.as_deref().is_some_and(Path::is_dir);
        if there && !out.iter().any(|o| o.key.eq_ignore_ascii_case(&g.key)) {
            out.push(g);
        }
    };
    if let Ok(rd) = std::fs::read_dir(epic.join("EpicGamesLauncher").join("Data").join("Manifests")) {
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("item"))).collect();
        files.sort();
        for f in files {
            if let Some(g) = std::fs::read(&f).ok().and_then(|b| parse_item(&decode_text(&b))).and_then(|i| game_of(&i)) {
                push(g);
            }
        }
    }
    if let Ok(b) = std::fs::read(epic.join("UnrealEngineLauncher").join("LauncherInstalled.dat")) {
        for e in parse_installed_dat(&decode_text(&b)) {
            if let Some(g) = engine_of(&e) {
                push(g);
            }
        }
    }
    out
}

/// Every game and engine the Epic Games Launcher installed on this PC.
pub fn scan() -> Vec<Game> {
    let base = std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"));
    scan_in(&base.join("Epic"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(app: &str, name: &str, main: &str, incomplete: bool, dir: &str) -> String {
        format!(
            r#"{{
	"FormatVersion": 0,
	"bIsIncompleteInstall": {incomplete},
	"AppVersionString": "1.4.2-18873641+++Release",
	"LaunchCommand": "",
	"LaunchExecutable": "Binaries\\Win64\\Game.exe",
	"ManifestLocation": "C:\\Users\\Someone\\Games\\Some Game\\.egstore",
	"bIsApplication": true,
	"bIsManaged": false,
	"bNeedsValidation": false,
	"bRequiresAuth": true,
	"bCanRunOffline": true,
	"AppName": "{app}",
	"BaseURLs": ["download.epicgames.com"],
	"BuildLabel": "Live-Windows",
	"CatalogItemId": "0b9c6a1de9e44f4fa5bb4b3d2a7c1e55",
	"CatalogNamespace": "4f1a2b3c4d5e6f708192a3b4c5d6e7f8",
	"AppCategories": ["public", "games", "applications"],
	"DisplayName": "{name}",
	"InstallationGuid": "AAAAAAAA-BBBB-CCCC-DDDD-EEEEEEEEEEEE",
	"InstallLocation": "{dir}",
	"InstallSessionId": "FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF",
	"InstallSize": 28123456789,
	"MainWindowProcessName": "Game.exe",
	"MainGameAppName": "{main}",
	"OwnershipToken": "TOKEN-SHOULD-NEVER-BE-KEPT"
}}"#
        )
    }

    #[test]
    fn a_manifest_gives_the_game_with_its_launch_address() {
        let i = parse_item(&item("Pumpkin", "Some Game", "Pumpkin", false, r"D:\\Games\\Some Game")).unwrap();
        assert_eq!(i.app_name, "Pumpkin");
        assert_eq!(i.install_size, 28123456789);
        let g = game_of(&i).unwrap();
        assert_eq!(g.id(), "game:epic:pumpkin");
        assert_eq!(g.name, "Some Game");
        assert_eq!(g.version, "1.4.2-18873641+++Release");
        assert_eq!(g.update, None);
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"D:\Games\Some Game")));
        assert_eq!(g.size, Some(28123456789));
        assert_eq!(g.launch_uri.as_deref(), Some("com.epicgames.launcher://apps/4f1a2b3c4d5e6f708192a3b4c5d6e7f8%3A0b9c6a1de9e44f4fa5bb4b3d2a7c1e55%3APumpkin?action=launch&silent=true"));
        assert!(g.launch_uri.as_deref().is_some_and(|u| own_uri(Launcher::Epic, u)));
        assert!(g.show_uri.as_deref().is_some_and(|u| own_uri(Launcher::Epic, u)));
        assert_eq!(g.notes, NotesFrom::None);
    }

    #[test]
    fn unfinished_installs_and_add_ons_are_not_games() {
        let unfinished = parse_item(&item("Pumpkin", "Some Game", "Pumpkin", true, r"D:\\Games\\Some Game")).unwrap();
        assert!(game_of(&unfinished).is_none());
        // An add-on names the game it belongs to.
        let dlc = parse_item(&item("PumpkinSeasonPass", "Some Game Season Pass", "Pumpkin", false, r"D:\\Games\\Some Game")).unwrap();
        assert!(game_of(&dlc).is_none());
        // No MainGameAppName at all is a game.
        let alone = parse_item(&item("Pumpkin", "Some Game", "", false, r"D:\\Games\\Some Game")).unwrap();
        assert!(game_of(&alone).is_some());
    }

    #[test]
    fn broken_or_empty_manifests_give_nothing() {
        assert!(parse_item("").is_none());
        assert!(parse_item("{ not json").is_none());
        assert!(parse_item(r#"{"DisplayName":"No app name"}"#).is_none());
        // A manifest cut short by the launcher writing it gives what was complete: nothing here.
        assert!(parse_item(r#"{"AppName":"X","DisplayName":"X","InstallLoc"#).is_none());
    }

    #[test]
    fn the_account_token_is_not_kept() {
        let i = parse_item(&item("Pumpkin", "Some Game", "Pumpkin", false, r"D:\\Games\\Some Game")).unwrap();
        let g = game_of(&i).unwrap();
        assert!(!format!("{i:?}{g:?}").contains("TOKEN-SHOULD-NEVER-BE-KEPT"));
    }

    #[test]
    fn an_address_part_with_odd_characters_is_encoded() {
        let mut i = parse_item(&item("Odd App", "Some Game", "Odd App", false, r"D:\\Games\\X")).unwrap();
        i.item_id = "a b&c".into();
        let g = game_of(&i).unwrap();
        let uri = g.launch_uri.unwrap();
        assert!(uri.contains("%3Aa%20b%26c%3AOdd%20App?"), "{uri}");
        assert!(own_uri(Launcher::Epic, &uri));
        // Without the catalog ids there is no address that is known to work.
        i.namespace.clear();
        assert!(game_of(&i).unwrap().launch_uri.is_none());
    }

    const DAT: &str = r#"{
	"InstallationList": [
		{
			"InstallLocation": "C:\\Program Files\\Epic Games\\UE_5.4",
			"NamespaceId": "ue",
			"ItemId": "11111111111111111111111111111111",
			"ArtifactId": "UE_5.4",
			"AppVersion": "5.4.4-37670630+++UE5+Release-5.4-Windows",
			"AppName": "UE_5.4"
		},
		{
			"InstallLocation": "D:\\Games\\Some Game",
			"NamespaceId": "4f1a2b3c4d5e6f708192a3b4c5d6e7f8",
			"ItemId": "0b9c6a1de9e44f4fa5bb4b3d2a7c1e55",
			"ArtifactId": "Pumpkin",
			"AppVersion": "1.4.2-18873641+++Release",
			"AppName": "Pumpkin"
		}
	]
}"#;

    #[test]
    fn unreal_engine_installs_are_read_from_the_installed_list() {
        let list = parse_installed_dat(DAT);
        assert_eq!(list.len(), 1, "the game is read from its manifest");
        let g = engine_of(&list[0]).unwrap();
        assert_eq!(g.id(), "game:epic:ue_5.4");
        assert_eq!(g.name, "Unreal Engine 5.4");
        assert_eq!(g.version, "5.4.4-37670630+++UE5+Release-5.4-Windows");
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"C:\Program Files\Epic Games\UE_5.4")));
        assert!(g.launch_uri.is_none());
        assert!(parse_installed_dat("{}").is_empty());
        assert!(parse_installed_dat("garbage").is_empty());
    }

    #[test]
    fn the_folder_scan_skips_what_is_gone_and_joins_both_files() {
        let root = std::env::temp_dir().join(format!("updater3d-epic-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let games = root.join("games");
        let (kept, gone, engine) = (games.join("Some Game"), games.join("Removed Game"), games.join("UE_5.4"));
        std::fs::create_dir_all(&kept).unwrap();
        std::fs::create_dir_all(&engine).unwrap();
        let manifests = root.join("Epic").join("EpicGamesLauncher").join("Data").join("Manifests");
        std::fs::create_dir_all(&manifests).unwrap();
        let esc = |p: &Path| p.to_string_lossy().replace('\\', "\\\\");
        std::fs::write(manifests.join("A.item"), item("Pumpkin", "Some Game", "Pumpkin", false, &esc(&kept))).unwrap();
        std::fs::write(manifests.join("B.item"), item("Gourd", "Removed Game", "Gourd", false, &esc(&gone))).unwrap();
        std::fs::write(manifests.join("C.item"), item("PumpkinPass", "Pass", "Pumpkin", false, &esc(&kept))).unwrap();
        std::fs::write(manifests.join("D.item"), "{ broken").unwrap();
        std::fs::write(manifests.join("notes.txt"), "ignored").unwrap();
        let ue = root.join("Epic").join("UnrealEngineLauncher");
        std::fs::create_dir_all(&ue).unwrap();
        let dat = format!(r#"{{"InstallationList":[{{"InstallLocation":"{}","AppName":"UE_5.4","AppVersion":"5.4.4"}}]}}"#, esc(&engine));
        std::fs::write(ue.join("LauncherInstalled.dat"), dat).unwrap();
        let got = scan_in(&root.join("Epic"));
        let names: Vec<&str> = got.iter().map(|g| g.name.as_str()).collect();
        assert_eq!(names, ["Some Game", "Unreal Engine 5.4"]);
        let _ = std::fs::remove_dir_all(&root);
        // No Epic folder at all is no games.
        assert!(scan_in(&root.join("Epic")).is_empty());
    }
}
