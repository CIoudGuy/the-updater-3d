//! Rockstar Games Launcher: each game it installed has an uninstall entry published by "Rockstar
//! Games" (with its folder, version and size). The launcher's own lists (`titles.dat` and the other
//! `.dat` files in ProgramData) are encrypted, and no address that starts or opens a game is
//! documented, so a game here has no launch address; the app opens the launcher itself.

use super::{dir_of, Game, Launcher, NotesFrom};
use crate::winutil::UninstallEntry;
use std::path::PathBuf;

/// The code Rockstar's uninstall command names: `"...\uninstall.exe" -enableFullMode
/// -uninstall=rdr2` gives "rdr2".
pub fn uninstall_code(cmd: &str) -> Option<String> {
    let rest = &cmd[cmd.to_ascii_lowercase().find("-uninstall=")? + "-uninstall=".len()..];
    let code: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')).collect();
    (!code.is_empty()).then_some(code.to_ascii_lowercase())
}

/// The game an uninstall entry stands for. None for anything not published by Rockstar Games, for
/// the launcher and its parts ("Rockstar Games Launcher", "... Social Club", "... SDK"), updates
/// and hidden entries.
pub fn game_of(e: &UninstallEntry) -> Option<Game> {
    let name = e.display_name.trim();
    if !e.publisher.to_ascii_lowercase().contains("rockstar games") || name.is_empty() || name.to_ascii_lowercase().starts_with("rockstar games") || e.system_component || !e.parent_key.is_empty() {
        return None;
    }
    // The folder the entry names; else the one the game's program (its icon) is in.
    let dir = dir_of(&e.install_location).or_else(|| crate::sources::proglogo::display_icon(&e.display_icon).and_then(|(p, _)| p.parent().map(PathBuf::from)))?;
    Some(Game {
        launcher: Launcher::Rockstar,
        key: uninstall_code(&e.uninstall_string).unwrap_or_else(|| e.key.trim().to_string()),
        name: name.to_string(),
        version: e.display_version.trim().to_string(),
        update: None,
        install_dir: Some(dir),
        size: Some(e.estimated_size_kb.saturating_mul(1024)).filter(|s| *s > 0),
        updated_at: crate::winutil::install_date_iso(&e.install_date),
        launch_uri: None,
        show_uri: None,
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
        program: None,
    })
}

/// Every Rockstar game among `entries` (the uninstall entries of this PC) that is still there.
pub fn scan_entries(entries: &[UninstallEntry]) -> Vec<Game> {
    let mut out: Vec<Game> = Vec::new();
    for g in entries.iter().filter_map(game_of) {
        if g.install_dir.as_deref().is_some_and(std::path::Path::is_dir) && !out.iter().any(|o| o.key == g.key) {
            out.push(g);
        }
    }
    out
}

/// Every game the Rockstar Games Launcher installed on this PC.
pub fn scan() -> Vec<Game> {
    scan_entries(&crate::winutil::uninstall_entries())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// The uninstall entry of a Rockstar game, as the launcher writes it.
    fn entry() -> UninstallEntry {
        UninstallEntry {
            key: "Some Rockstar Game".into(),
            display_name: "Some Rockstar Game".into(),
            display_version: "2.3.4.5".into(),
            publisher: "Rockstar Games".into(),
            install_location: r"D:\Games\Some Rockstar Game".into(),
            uninstall_string: r#""D:\Games\Some Rockstar Game\uninstall.exe" -enableFullMode -uninstall=rdr2"#.into(),
            display_icon: r#""D:\Games\Some Rockstar Game\Game.exe""#.into(),
            estimated_size_kb: 12_345_678,
            ..Default::default()
        }
    }

    #[test]
    fn an_entry_gives_the_game() {
        let g = game_of(&entry()).unwrap();
        assert_eq!(g.id(), "game:rockstar:rdr2");
        assert_eq!(g.name, "Some Rockstar Game");
        assert_eq!(g.version, "2.3.4.5");
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"D:\Games\Some Rockstar Game")));
        assert_eq!(g.size, Some(12_345_678 * 1024));
        assert_eq!((g.launch_uri, g.show_uri, g.uninstall_uri, g.update), (None, None, None, None));
    }

    #[test]
    fn the_code_comes_from_the_uninstall_command_else_the_key() {
        assert_eq!(uninstall_code(r#""D:\G\uninstall.exe" -enableFullMode -uninstall=gta5 -x"#).as_deref(), Some("gta5"));
        assert_eq!(uninstall_code(r#""D:\G\uninstall.exe""#), None);
        let mut e = entry();
        e.uninstall_string = r#""D:\G\uninstall.exe""#.into();
        assert_eq!(game_of(&e).unwrap().key, "Some Rockstar Game");
    }

    #[test]
    fn the_launcher_its_parts_and_other_publishers_are_not_games() {
        for n in ["Rockstar Games Launcher", "Rockstar Games Social Club", "Rockstar Games SDK"] {
            let mut e = entry();
            e.display_name = n.into();
            assert!(game_of(&e).is_none(), "{n}");
        }
        let mut other = entry();
        other.publisher = "Someone Else".into();
        assert!(game_of(&other).is_none());
        let mut hidden = entry();
        hidden.system_component = true;
        assert!(game_of(&hidden).is_none());
        let mut update = entry();
        update.parent_key = "Some Rockstar Game".into();
        assert!(game_of(&update).is_none());
    }

    #[test]
    fn the_programs_folder_stands_in_for_a_missing_install_location() {
        let mut e = entry();
        e.install_location.clear();
        assert_eq!(game_of(&e).unwrap().install_dir.as_deref(), Some(Path::new(r"D:\Games\Some Rockstar Game")));
        e.display_icon.clear();
        assert!(game_of(&e).is_none());
    }

    #[test]
    fn a_game_whose_folder_is_gone_is_not_listed() {
        let mut gone = entry();
        gone.install_location = r"Z:\No\Such\Folder".into();
        assert!(scan_entries(&[gone]).is_empty());
        let here = std::env::temp_dir();
        let mut there = entry();
        there.install_location = here.to_string_lossy().to_string();
        assert_eq!(scan_entries(&[there.clone(), there]).len(), 1);
    }
}
