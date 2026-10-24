//! Ubisoft Connect: each installed game has a key under `HKLM\SOFTWARE\Ubisoft\Launcher\Installs`
//! (the 32 bit view; the key is the game's install id and `InstallDir` its folder). Its name comes
//! from the launcher's uninstall entry "Uplay Install <id>". The launcher keeps no installed
//! version and no waiting update in these places, and it updates games itself before starting
//! them, so only the name, folder, size and picture are known. The game's picture is the icon
//! named in the launcher's game list (`cache\configuration\configurations`) and kept in the
//! launcher's `data` folder.

use super::{dir_of, own_uri, reg_text, Game, Launcher, NotesFrom};
use std::path::{Path, PathBuf};

/// The text of a game in the launcher's list is a YAML document that starts with this line.
fn doc_starts(text: &str) -> Vec<usize> {
    static START: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"version: \d+(?:\.\d+)*\r?\nroot:").unwrap());
    START.find_iter(text).map(|m| m.start()).collect()
}

/// The icon file name in the launcher's game list for install `id`. The list holds one document
/// per game; the one whose start command reads `Installs\<id>\InstallDir` names its icon
/// (`icon_image: <hash>.ico`).
pub fn icon_name(configurations: &str, id: &str) -> Option<String> {
    static ICON: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"(?m)^\s*icon_image:\s*([A-Za-z0-9_.\-]+)\s*$").unwrap());
    let needle = format!("Installs\\{id}\\InstallDir");
    let at = configurations.find(&needle)?;
    let starts = doc_starts(configurations);
    let from = *starts.iter().rev().find(|s| **s <= at)?;
    let to = starts.iter().find(|s| **s > at).copied().unwrap_or(configurations.len());
    let name = ICON.captures(&configurations[from..to])?[1].to_string();
    let low = name.to_ascii_lowercase();
    (!name.contains("..") && (low.ends_with(".ico") || low.ends_with(".png") || low.ends_with(".jpg"))).then_some(name)
}

/// An install id is a number.
fn is_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && s.chars().all(|c| c.is_ascii_digit())
}

/// What the registry says about one installed game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Install {
    pub id: String,
    /// `InstallDir` as the launcher wrote it ("D:/Games/Some Game/")
    pub dir: String,
    /// The uninstall entry's `DisplayName` and `DisplayVersion`
    pub name: String,
    pub version: String,
    pub size_kb: u64,
}

/// The game an install stands for. `icon` is the launcher's picture of it when it has one.
pub fn game_of(i: &Install, icon: Option<PathBuf>) -> Option<Game> {
    if !is_id(&i.id) {
        return None;
    }
    let dir = dir_of(&i.dir)?;
    let name = if i.name.trim().is_empty() { dir.file_name()?.to_string_lossy().to_string() } else { i.name.trim().to_string() };
    let id = &i.id;
    Some(Game {
        launcher: Launcher::Ubisoft,
        key: id.clone(),
        name,
        version: i.version.trim().to_string(),
        update: None,
        install_dir: Some(dir),
        size: Some(i.size_kb.saturating_mul(1024)).filter(|s| *s > 0),
        updated_at: None,
        launch_uri: Some(format!("uplay://launch/{id}/0")).filter(|u| own_uri(Launcher::Ubisoft, u)),
        show_uri: None,
        uninstall_uri: Some(format!("uplay://uninstall/{id}")).filter(|u| own_uri(Launcher::Ubisoft, u)),
        page: None,
        notes: NotesFrom::None,
        icon,
        program: None,
    })
}

/// The launcher's own folder, from where it says it is installed.
fn launcher_dir() -> Option<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;
    let k = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("SOFTWARE\\Ubisoft\\Launcher", KEY_READ | KEY_WOW64_32KEY).ok()?;
    dir_of(&k.get_value::<String, _>("InstallDir").ok()?)
}

/// The launcher's game list, when it is a sensible size.
fn read_configurations() -> Option<String> {
    let p = PathBuf::from(std::env::var_os("LOCALAPPDATA")?).join("Ubisoft Game Launcher").join("cache").join("configuration").join("configurations");
    let meta = std::fs::metadata(&p).ok()?;
    if meta.len() > 32 * 1024 * 1024 {
        return None;
    }
    Some(String::from_utf8_lossy(&std::fs::read(p).ok()?).into_owned())
}

/// The picture of install `id`: its icon file in the launcher's `data` folder, when that exists.
pub fn icon_file(launcher: &Path, configurations: &str, id: &str) -> Option<PathBuf> {
    let f = launcher.join("data").join(icon_name(configurations, id)?);
    f.is_file().then_some(f)
}

/// Every game Ubisoft Connect has installed on this PC.
pub fn scan() -> Vec<Game> {
    use winreg::enums::*;
    use winreg::RegKey;
    let lm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let flags = KEY_READ | KEY_WOW64_32KEY;
    let Ok(installs) = lm.open_subkey_with_flags("SOFTWARE\\Ubisoft\\Launcher\\Installs", flags) else { return vec![] };
    let uninstall = lm.open_subkey_with_flags("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall", flags).ok();
    let mut found: Vec<Install> = Vec::new();
    for id in installs.enum_keys().flatten().filter(|k| is_id(k)) {
        let Ok(k) = installs.open_subkey_with_flags(&id, flags) else { continue };
        let mut i = Install { id: id.clone(), dir: reg_text(&k, "InstallDir"), ..Default::default() };
        if let Some(u) = uninstall.as_ref().and_then(|u| u.open_subkey_with_flags(format!("Uplay Install {id}"), flags).ok()) {
            i.name = reg_text(&u, "DisplayName");
            i.version = reg_text(&u, "DisplayVersion");
            i.size_kb = reg_text(&u, "EstimatedSize").parse().unwrap_or(0);
            if i.dir.trim().is_empty() {
                i.dir = reg_text(&u, "InstallLocation");
            }
        }
        // A game whose folder is gone is no longer installed.
        if dir_of(&i.dir).is_some_and(|d| d.is_dir()) {
            found.push(i);
        }
    }
    if found.is_empty() {
        return vec![];
    }
    let (launcher, config) = (launcher_dir(), read_configurations());
    found.iter().filter_map(|i| game_of(i, launcher.as_deref().zip(config.as_deref()).and_then(|(l, c)| icon_file(l, c, &i.id)))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two games of the launcher's list, as it is stored: YAML documents with binary between them.
    fn configurations() -> String {
        let doc = |name: &str, icon: &str, id: &str| {
            format!(
                "version: 2.0\nroot:\n  name: {name}\n  background_image: 0200468a9d7f35719a8aa7639e1660f6.jpg\n  thumb_image: f20f8e1c4291327f40839533e3d89bed.jpg\n  icon_image: {icon}\n  start_game:\n    online:\n      executables:\n      - shortcut_name: {name}\n        path:\n          relative: Game.exe\n        working_directory:\n          register: HKEY_LOCAL_MACHINE\\SOFTWARE\\Ubisoft\\Launcher\\Installs\\{id}\\InstallDir\n    offline:\n      executables:\n      - shortcut_name: {name}\n        working_directory:\n          register: HKEY_LOCAL_MACHINE\\SOFTWARE\\Ubisoft\\Launcher\\Installs\\{id}\\InstallDir\n  installer:\n    game_identifier: {name}\n    publisher: Ubisoft\nlocalizations: {{}}\n"
            )
        };
        format!("\u{0}\u{1}\u{fffd}\u{7f}\u{fffd}]{}\u{fffd}\u{0}E\u{fffd}=\u{fffd}{}\u{fffd}", doc("First Game", "aaaa0000aaaa0000aaaa0000aaaa0000.ico", "11"), doc("Second Game", "bbbb1111bbbb1111bbbb1111bbbb1111.ico", "16732"))
    }

    #[test]
    fn the_icon_is_the_one_in_the_games_own_document() {
        let c = configurations();
        assert_eq!(icon_name(&c, "16732").as_deref(), Some("bbbb1111bbbb1111bbbb1111bbbb1111.ico"));
        assert_eq!(icon_name(&c, "11").as_deref(), Some("aaaa0000aaaa0000aaaa0000aaaa0000.ico"));
        // An id that only starts like another one is not that game.
        assert_eq!(icon_name(&c, "1673"), None);
        assert_eq!(icon_name(&c, "999"), None);
        assert_eq!(icon_name("", "11"), None);
        assert_eq!(icon_name("no documents here Installs\\11\\InstallDir", "11"), None);
    }

    #[test]
    fn an_icon_name_that_is_not_a_plain_picture_file_is_refused() {
        let c = configurations().replace("bbbb1111bbbb1111bbbb1111bbbb1111.ico", "..\\..\\evil.dll");
        assert_eq!(icon_name(&c, "16732"), None);
    }

    fn install() -> Install {
        Install { id: "16732".into(), dir: "D:/Games/Some Game/".into(), name: "Some Game".into(), version: String::new(), size_kb: 0 }
    }

    #[test]
    fn an_install_gives_the_game_with_ubisofts_addresses() {
        let g = game_of(&install(), None).unwrap();
        assert_eq!(g.id(), "game:ubisoft:16732");
        assert_eq!(g.name, "Some Game");
        assert_eq!(g.version, "");
        assert_eq!(g.update, None);
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"D:\Games\Some Game")));
        assert_eq!(g.size, None);
        assert_eq!(g.launch_uri.as_deref(), Some("uplay://launch/16732/0"));
        assert_eq!(g.uninstall_uri.as_deref(), Some("uplay://uninstall/16732"));
        assert!(g.show_uri.is_none());
        assert!(g.launch_uri.as_deref().is_some_and(|u| own_uri(Launcher::Ubisoft, u)));
    }

    #[test]
    fn the_folder_name_stands_in_for_a_missing_uninstall_entry() {
        let mut i = install();
        i.name.clear();
        assert_eq!(game_of(&i, None).unwrap().name, "Some Game");
        i.size_kb = 1000;
        assert_eq!(game_of(&i, None).unwrap().size, Some(1_024_000));
    }

    #[test]
    fn an_id_that_is_not_a_number_or_no_folder_gives_nothing() {
        let mut i = install();
        i.id = "16732/../x".into();
        assert!(game_of(&i, None).is_none());
        let mut i = install();
        i.dir = "  ".into();
        assert!(game_of(&i, None).is_none());
    }
}
