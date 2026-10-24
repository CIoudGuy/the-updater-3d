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
        program: None,
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
        program: None,
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
    let mut out = scan_in(&base.join("Epic"));
    // The per-user launcher keeps no manifests there: its games are found by their own folders.
    for g in scan_folders(&crate::winutil::fixed_drives()) {
        let same = |o: &Game| o.key.eq_ignore_ascii_case(&g.key) || o.install_dir.as_deref().zip(g.install_dir.as_deref()).is_some_and(|(a, b)| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy()));
        if !out.iter().any(same) {
            out.push(g);
        }
    }
    out
}

/// What the build manifest in a game's `.egstore` folder says (Epic's own file, written by the
/// launcher into every game it installs): its app name, its build and the program it starts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildMeta {
    pub app_name: String,
    pub build: String,
    pub launch_exe: String,
}

/// Reads an Unreal `FString` (a signed length; negative means UTF-16) at `*at`.
fn fstring(b: &[u8], at: &mut usize) -> Option<String> {
    let n = i32::from_le_bytes(b.get(*at..*at + 4)?.try_into().ok()?);
    *at += 4;
    let s = if n < 0 {
        let len = n.unsigned_abs() as usize * 2;
        let units: Vec<u16> = b.get(*at..*at + len)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        *at += len;
        String::from_utf16_lossy(&units)
    } else {
        let len = n as usize;
        let s = String::from_utf8_lossy(b.get(*at..*at + len)?).into_owned();
        *at += len;
        s
    };
    Some(s.trim_end_matches('\0').to_string())
}

/// The start of a build manifest: binary (zlib packed, Epic's usual form) or JSON (older builds).
/// Only its first part is needed, so a cut-off file still gives its name and build. The launch
/// command that follows (it can hold an id of the account) is not read.
pub fn build_meta(bytes: &[u8]) -> Option<BuildMeta> {
    if bytes.first() == Some(&b'{') {
        let v: serde_json::Value = serde_json::from_str(&String::from_utf8_lossy(bytes)).ok()?;
        let s = |k: &str| v[k].as_str().unwrap_or("").to_string();
        let m = BuildMeta { app_name: s("AppNameString"), build: s("BuildVersionString"), launch_exe: s("LaunchExeString") };
        return (!m.app_name.is_empty()).then_some(m);
    }
    const MAGIC: u32 = 0x44BE_C00C;
    if u32::from_le_bytes(bytes.get(0..4)?.try_into().ok()?) != MAGIC {
        return None;
    }
    let header = u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?) as usize;
    let stored = *bytes.get(36)?;
    let body = bytes.get(header..)?;
    let data: Vec<u8> = if stored & 1 != 0 {
        // The meta block is at the start: unpack only that much, from what was read.
        match miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(body, 64 * 1024) {
            Ok(v) => v,
            Err(e) => e.output,
        }
    } else {
        body.to_vec()
    };
    // Meta: size, data version, feature level, file data flag, app id, then the strings.
    let mut at = 4 + 1 + 4 + 1 + 4;
    let app_name = fstring(&data, &mut at)?;
    let build = fstring(&data, &mut at)?;
    let launch_exe = fstring(&data, &mut at)?;
    (!app_name.is_empty()).then_some(BuildMeta { app_name, build, launch_exe })
}

/// The version people know from a build ("++Fortnite+Release-42.40-CL-59086525-Windows" gives
/// "42.40"); else the build as written.
pub fn version_of_build(build: &str) -> String {
    let b = build.trim();
    if let Some(i) = b.find("Release-") {
        let v: String = b[i + 8..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        if v.chars().any(|c| c.is_ascii_digit()) {
            return v.trim_end_matches('.').to_string();
        }
    }
    b.to_string()
}

/// The game installed in `dir` (a folder with Epic's `.egstore`), from its build manifest.
pub fn game_of_folder(dir: &Path, meta: &BuildMeta) -> Option<Game> {
    let name = dir.file_name()?.to_string_lossy().trim().to_string();
    if name.is_empty() {
        return None;
    }
    let exe = meta.launch_exe.trim();
    let program = (!exe.is_empty() && !exe.contains("..")).then(|| dir.join(exe.replace('/', "\\"))).filter(|p| p.is_file());
    Some(Game {
        launcher: Launcher::Epic,
        key: meta.app_name.clone(),
        name,
        version: version_of_build(&meta.build),
        update: None,
        install_dir: Some(dir.to_path_buf()),
        size: None,
        updated_at: None,
        launch_uri: None,
        show_uri: Some(LIBRARY_URI.to_string()),
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
        program,
    })
}

/// The folders games are usually installed in, on each drive.
const GAME_ROOTS: &[&str] = &["Games", "Epic Games", "EpicGames", r"Program Files\Epic Games", r"Program Files (x86)\Epic Games", "SteamLibrary"];

/// Epic games found by their own folders: every folder in a usual games folder (`GAME_ROOTS`) on
/// each of `drives` that has Epic's `.egstore` with a build manifest. Two levels at most are
/// read, never the whole drive.
pub fn scan_folders(drives: &[PathBuf]) -> Vec<Game> {
    let mut out = Vec::new();
    for d in drives {
        for root in GAME_ROOTS {
            let Ok(rd) = std::fs::read_dir(d.join(root)) else { continue };
            for e in rd.flatten().take(500) {
                let dir = e.path();
                let Ok(files) = std::fs::read_dir(dir.join(".egstore")) else { continue };
                let manifest = files.flatten().map(|f| f.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("manifest"))).max_by_key(|p| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0));
                let Some(m) = manifest else { continue };
                let head = read_head(&m, 512 * 1024);
                if let Some(g) = build_meta(&head).and_then(|meta| game_of_folder(&dir, &meta)) {
                    out.push(g);
                }
            }
        }
    }
    out
}

/// The first `max` bytes of a file.
fn read_head(p: &Path, max: u64) -> Vec<u8> {
    use std::io::Read;
    let mut b = Vec::new();
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.take(max).read_to_end(&mut b);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An Unreal FString as written in a manifest: its length with the end zero, then the text.
    fn fs(s: &str) -> Vec<u8> {
        let mut v = ((s.len() + 1) as i32).to_le_bytes().to_vec();
        v.extend_from_slice(s.as_bytes());
        v.push(0);
        v
    }

    /// A build manifest packed the way the launcher writes it: header, then the zlib packed body
    /// whose meta block holds the app name, build, program and launch command.
    fn binary_manifest(app: &str, build: &str, exe: &str, command: &str) -> Vec<u8> {
        let mut meta = Vec::new();
        meta.extend_from_slice(&448u32.to_le_bytes());
        meta.push(2);
        meta.extend_from_slice(&21u32.to_le_bytes());
        meta.push(0);
        meta.extend_from_slice(&0u32.to_le_bytes());
        for s in [app, build, exe, command] {
            meta.extend(fs(s));
        }
        meta.extend(vec![0u8; 4096]);
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&meta, 6);
        let mut out = Vec::new();
        out.extend_from_slice(&0x44BE_C00Cu32.to_le_bytes());
        out.extend_from_slice(&41u32.to_le_bytes());
        out.extend_from_slice(&(meta.len() as u32).to_le_bytes());
        out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
        out.extend(vec![0u8; 20]);
        out.push(1);
        out.extend_from_slice(&21u32.to_le_bytes());
        out.extend(packed);
        out
    }

    #[test]
    fn a_games_own_build_manifest_names_it_and_its_build() {
        let m = binary_manifest("FortniteReleaseBuilds", "++Fortnite+Release-42.40-CL-59086525-Windows", "FortniteGame/Binaries/Win64/FortniteBootstrapper.exe", "-obfuscationid=SOMEID");
        let meta = build_meta(&m).unwrap();
        assert_eq!(meta.app_name, "FortniteReleaseBuilds");
        assert_eq!(meta.build, "++Fortnite+Release-42.40-CL-59086525-Windows");
        assert_eq!(meta.launch_exe, "FortniteGame/Binaries/Win64/FortniteBootstrapper.exe");
        // The launch command (it can hold an account id) is not kept.
        assert!(!format!("{meta:?}").contains("SOMEID"));
        // A file read only in part still gives them.
        assert_eq!(build_meta(&m[..m.len().min(200)]).map(|m| m.app_name).as_deref(), Some("FortniteReleaseBuilds"));
        // The older JSON form, and things that are not a manifest.
        assert_eq!(build_meta(br#"{"AppNameString":"Sugar","BuildVersionString":"++Prod+2.43","LaunchExeString":"Binaries/Win64/RocketLeague.exe"}"#).map(|m| m.app_name).as_deref(), Some("Sugar"));
        assert_eq!(build_meta(b"not a manifest"), None);
        assert_eq!(build_meta(&[]), None);
    }

    #[test]
    fn a_build_reads_as_the_version_people_know() {
        assert_eq!(version_of_build("++Fortnite+Release-42.40-CL-59086525-Windows"), "42.40");
        assert_eq!(version_of_build("1.2.3"), "1.2.3");
        assert_eq!(version_of_build("++Prod+CL-123"), "++Prod+CL-123");
    }

    #[test]
    fn games_are_found_by_their_folders_on_every_drive() {
        let root = std::env::temp_dir().join(format!("u3d-epic-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let game = root.join("Games").join("Fortnite");
        std::fs::create_dir_all(game.join(".egstore")).unwrap();
        std::fs::create_dir_all(game.join(r"FortniteGame\Binaries\Win64")).unwrap();
        std::fs::write(game.join(r"FortniteGame\Binaries\Win64\FortniteBootstrapper.exe"), b"MZ").unwrap();
        std::fs::write(game.join(".egstore").join("D433D18D04C83CBC310043BDC5E3087B.manifest"), binary_manifest("FortniteReleaseBuilds", "++Fortnite+Release-42.40-CL-59086525-Windows", "FortniteGame/Binaries/Win64/FortniteBootstrapper.exe", "")).unwrap();
        // A folder without Epic's store data is not an Epic game.
        std::fs::create_dir_all(root.join("Games").join("Other Game")).unwrap();
        let found = scan_folders(&[root.clone()]);
        assert_eq!(found.len(), 1);
        let g = &found[0];
        assert_eq!((g.name.as_str(), g.version.as_str(), g.key.as_str()), ("Fortnite", "42.40", "FortniteReleaseBuilds"));
        assert_eq!(g.install_dir.as_deref(), Some(game.as_path()));
        assert_eq!(g.program.as_deref(), Some(game.join(r"FortniteGame\Binaries\Win64\FortniteBootstrapper.exe").as_path()));
        assert!(g.show_uri.as_deref().is_some_and(|u| own_uri(Launcher::Epic, u)));
        let _ = std::fs::remove_dir_all(&root);
    }

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
