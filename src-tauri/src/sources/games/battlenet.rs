//! Battle.net: each game it installed has an uninstall entry whose command is the Battle.net
//! Agent's uninstaller (`"...\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS --uid=<uid>
//! --displayname="<name>"`). The uid names the game. Blizzard documents no list of the codes
//! `battlenet://` takes, so only the codes seen in Blizzard's own forum answers are used (`D3`,
//! `WoW`); any other game has no launch address and the app opens Battle.net itself. Battle.net
//! shows and installs updates itself; nothing local marks one.

use super::{dir_of, own_uri, Game, Launcher, NotesFrom};
use crate::winutil::UninstallEntry;

/// The `battlenet://` codes that are documented, by the uid in the uninstall command.
const LAUNCH_CODES: &[(&str, &str)] = &[("wow", "WoW"), ("d3", "D3")];

/// The value of `--<name>=` in an uninstall command: up to the next space or quote, or the
/// quoted text when it is quoted (`--displayname="World of Warcraft"`).
fn flag(cmd: &str, name: &str) -> Option<String> {
    let key = format!("--{name}=");
    let rest = &cmd[cmd.to_ascii_lowercase().find(&key)? + key.len()..];
    let v = match rest.strip_prefix('"') {
        Some(q) => q.split('"').next().unwrap_or(""),
        None => rest.split(|c: char| c.is_whitespace() || c == '"').next().unwrap_or(""),
    };
    let v = v.trim();
    (!v.is_empty()).then(|| v.to_string())
}

/// The game uid in a Battle.net uninstall command.
pub fn uid_of(cmd: &str) -> Option<String> {
    if !cmd.to_ascii_lowercase().contains("battle.net") {
        return None;
    }
    flag(cmd, "uid").map(|u| u.to_ascii_lowercase()).filter(|u| u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
}

/// The game an uninstall entry stands for. None for anything that is not run by the Battle.net
/// Agent's uninstaller and for Battle.net itself (uid "battle.net").
pub fn game_of(e: &UninstallEntry) -> Option<Game> {
    let uid = uid_of(&e.uninstall_string).or_else(|| uid_of(&e.quiet_uninstall_string))?;
    if uid == "battle.net" || e.system_component || !e.parent_key.is_empty() {
        return None;
    }
    let name = [e.display_name.trim().to_string(), flag(&e.uninstall_string, "displayname").unwrap_or_default()].into_iter().find(|n| !n.is_empty())?;
    if name.eq_ignore_ascii_case("battle.net") {
        return None;
    }
    let code = LAUNCH_CODES.iter().find(|(u, _)| *u == uid).map(|(_, c)| *c);
    Some(Game {
        launcher: Launcher::BattleNet,
        key: uid,
        name,
        version: e.display_version.trim().to_string(),
        update: None,
        install_dir: dir_of(&e.install_location),
        size: Some(e.estimated_size_kb.saturating_mul(1024)).filter(|s| *s > 0),
        updated_at: crate::winutil::install_date_iso(&e.install_date),
        launch_uri: code.map(|c| format!("battlenet://{c}")).filter(|u| own_uri(Launcher::BattleNet, u)),
        show_uri: None,
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
    })
}

/// Every Battle.net game among `entries` (the uninstall entries of this PC). A game whose folder
/// is known and gone is left out.
pub fn scan_entries(entries: &[UninstallEntry]) -> Vec<Game> {
    let mut out: Vec<Game> = Vec::new();
    for g in entries.iter().filter_map(game_of) {
        let gone = g.install_dir.as_deref().is_some_and(|d| !d.is_dir());
        if !gone && !out.iter().any(|o| o.key == g.key) {
            out.push(g);
        }
    }
    out
}

/// Every game Battle.net installed on this PC.
pub fn scan() -> Vec<Game> {
    scan_entries(&crate::winutil::uninstall_entries())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn entry(uid: &str, name: &str) -> UninstallEntry {
        UninstallEntry {
            key: format!("Battle.net_{uid}"),
            display_name: name.into(),
            display_version: "11.0.2.54000".into(),
            publisher: "Blizzard Entertainment".into(),
            install_location: r"C:\Games\Some Blizzard Game".into(),
            uninstall_string: format!(r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS --uid={uid} --displayname="{name}""#),
            estimated_size_kb: 100_000,
            ..Default::default()
        }
    }

    #[test]
    fn the_uid_and_name_come_from_the_uninstall_command() {
        let c = r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS --uid=wow --displayname="World of Warcraft""#;
        assert_eq!(uid_of(c).as_deref(), Some("wow"));
        assert_eq!(flag(c, "displayname").as_deref(), Some("World of Warcraft"));
        assert_eq!(uid_of(r#""C:\Other\unins000.exe" --uid=wow"#), None);
        assert_eq!(uid_of(r#""C:\ProgramData\Battle.net\Agent\Blizzard Uninstaller.exe" --lang=enUS"#), None);
    }

    #[test]
    fn a_game_with_a_documented_code_can_be_launched_through_battle_net() {
        let g = game_of(&entry("wow", "World of Warcraft")).unwrap();
        assert_eq!(g.id(), "game:battlenet:wow");
        assert_eq!(g.name, "World of Warcraft");
        assert_eq!(g.version, "11.0.2.54000");
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"C:\Games\Some Blizzard Game")));
        assert_eq!(g.size, Some(100_000 * 1024));
        assert_eq!(g.launch_uri.as_deref(), Some("battlenet://WoW"));
        assert!(g.launch_uri.as_deref().is_some_and(|u| own_uri(Launcher::BattleNet, u)));
        assert_eq!(game_of(&entry("d3", "Diablo III")).unwrap().launch_uri.as_deref(), Some("battlenet://D3"));
    }

    #[test]
    fn a_game_without_a_documented_code_has_no_launch_address() {
        let g = game_of(&entry("s2", "StarCraft II")).unwrap();
        assert_eq!(g.key, "s2");
        assert_eq!((g.launch_uri, g.show_uri, g.uninstall_uri), (None, None, None));
    }

    #[test]
    fn battle_net_itself_and_other_programs_are_not_games() {
        assert!(game_of(&entry("battle.net", "Battle.net")).is_none());
        let mut other = entry("wow", "World of Warcraft");
        other.uninstall_string = r#""C:\Program Files\Other\unins000.exe""#.into();
        assert!(game_of(&other).is_none());
        let mut hidden = entry("wow", "World of Warcraft");
        hidden.system_component = true;
        assert!(game_of(&hidden).is_none());
    }

    #[test]
    fn the_name_in_the_command_stands_in_for_a_missing_display_name() {
        let mut e = entry("wow", "World of Warcraft");
        e.display_name.clear();
        assert_eq!(game_of(&e).unwrap().name, "World of Warcraft");
    }

    #[test]
    fn a_game_whose_known_folder_is_gone_is_left_out() {
        let mut gone = entry("wow", "World of Warcraft");
        gone.install_location = r"Z:\No\Such\Folder".into();
        assert!(scan_entries(&[gone]).is_empty());
        // A game with no folder in its entry is kept.
        let mut nodir = entry("d3", "Diablo III");
        nodir.install_location.clear();
        assert_eq!(scan_entries(&[nodir]).len(), 1);
    }
}
