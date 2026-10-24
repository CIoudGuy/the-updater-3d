//! Games and apps installed through game launchers (Steam, Epic Games, Ubisoft Connect, GOG
//! Galaxy, EA app, Battle.net, Amazon Games, itch, Rockstar). Each launcher keeps its own list of
//! what it installed on this PC; those files are read here, nothing is signed in to and nothing
//! about the PC or its accounts is sent anywhere. A launcher downloads and installs its own
//! updates: when it has marked one, Update opens it there. Patch notes come from public sources
//! where a launcher has them (Steam's news, GOG's changelogs).

use std::path::PathBuf;

pub mod steam;

mod app;
pub use app::{can_uninstall, installed, latest, launch_uri, news, plan_uninstall, show_uri, steam_history, through};

/// A game launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Launcher {
    Steam,
    Epic,
    Ubisoft,
    Gog,
    Ea,
    BattleNet,
    Amazon,
    Itch,
    Rockstar,
}

impl Launcher {
    /// Its name as people know it.
    pub fn name(self) -> &'static str {
        match self {
            Launcher::Steam => "Steam",
            Launcher::Epic => "Epic Games Launcher",
            Launcher::Ubisoft => "Ubisoft Connect",
            Launcher::Gog => "GOG Galaxy",
            Launcher::Ea => "EA app",
            Launcher::BattleNet => "Battle.net",
            Launcher::Amazon => "Amazon Games",
            Launcher::Itch => "itch",
            Launcher::Rockstar => "Rockstar Games Launcher",
        }
    }

    /// The short word in ids: "game:steam:1912410".
    pub fn key(self) -> &'static str {
        match self {
            Launcher::Steam => "steam",
            Launcher::Epic => "epic",
            Launcher::Ubisoft => "ubisoft",
            Launcher::Gog => "gog",
            Launcher::Ea => "ea",
            Launcher::BattleNet => "battlenet",
            Launcher::Amazon => "amazon",
            Launcher::Itch => "itch",
            Launcher::Rockstar => "rockstar",
        }
    }

    /// The address schemes the launcher answers. Only these are ever opened for its games.
    pub fn schemes(self) -> &'static [&'static str] {
        match self {
            Launcher::Steam => &["steam"],
            Launcher::Epic => &["com.epicgames.launcher"],
            Launcher::Ubisoft => &["uplay"],
            Launcher::Gog => &["goggalaxy"],
            Launcher::Ea => &["link2ea", "origin2"],
            Launcher::BattleNet => &["battlenet"],
            Launcher::Amazon => &["amazon-games"],
            Launcher::Itch => &["itch"],
            Launcher::Rockstar => &["rockstar"],
        }
    }
}

/// Where a game's patch notes are read from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotesFrom {
    None,
    /// Steam's public news for the app id (its official announcements)
    SteamNews(u32),
    /// GOG's public product changelog for the product id
    Gog(u64),
}

/// A game (or an app) a launcher installed on this PC.
#[derive(Clone, Debug, PartialEq)]
pub struct Game {
    pub launcher: Launcher,
    /// The launcher's own id: the app id on Steam, the AppName on Epic, the install id on Ubisoft.
    pub key: String,
    pub name: String,
    /// The installed version as the launcher names it ("1.2.3"), or its build ("15790123") when it
    /// has no version. Empty when the launcher keeps neither.
    pub version: String,
    /// The newer version or build the launcher has found and not installed yet. Some("") when it
    /// has marked an update without saying which.
    pub update: Option<String>,
    pub install_dir: Option<PathBuf>,
    pub size: Option<u64>,
    /// When the launcher last installed or updated it (ISO 8601)
    pub updated_at: Option<String>,
    /// Starts the game through its launcher
    pub launch_uri: Option<String>,
    /// Shows the game (and its update) in the launcher
    pub show_uri: Option<String>,
    /// Uninstalls it through the launcher
    pub uninstall_uri: Option<String>,
    /// Its store page (https)
    pub page: Option<String>,
    pub notes: NotesFrom,
    /// A picture of it the launcher keeps on this PC (its icon), when there is one
    pub icon: Option<PathBuf>,
}

impl Game {
    /// "game:<launcher>:<key>", lowercase.
    pub fn id(&self) -> String {
        format!("game:{}:{}", self.launcher.key(), self.key).to_ascii_lowercase()
    }
}

/// Is `uri` an address of `launcher`'s own scheme? Nothing else is opened for its games.
pub fn own_uri(launcher: Launcher, uri: &str) -> bool {
    let Some((scheme, rest)) = uri.split_once("://") else { return false };
    !rest.is_empty() && !uri.chars().any(|c| c.is_control() || c == '"' || c == ' ') && launcher.schemes().iter().any(|s| s.eq_ignore_ascii_case(scheme))
}

/// Every game the launchers on this PC list. A launcher that is not installed gives none.
pub fn scan() -> Vec<Game> {
    let mut out = steam::scan();
    out.retain(|g| !g.name.trim().is_empty());
    out
}

/// A Unix time as ISO 8601.
pub(crate) fn iso(unix: i64) -> Option<String> {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0).filter(|_| unix > 0).map(|t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_launchers_own_addresses_are_opened() {
        assert!(own_uri(Launcher::Steam, "steam://nav/games/details/1912410"));
        assert!(!own_uri(Launcher::Steam, "https://example.com"));
        assert!(!own_uri(Launcher::Steam, "uplay://launch/16732/0"));
        assert!(!own_uri(Launcher::Steam, "steam://"));
        assert!(!own_uri(Launcher::Steam, "steam://run/1\" --evil"));
        assert!(own_uri(Launcher::Ubisoft, "uplay://launch/16732/0"));
    }

    #[test]
    fn a_game_id_names_its_launcher() {
        let g = Game { launcher: Launcher::Epic, key: "Fortnite".into(), name: "Fortnite".into(), version: String::new(), update: None, install_dir: None, size: None, updated_at: None, launch_uri: None, show_uri: None, uninstall_uri: None, page: None, notes: NotesFrom::None, icon: None };
        assert_eq!(g.id(), "game:epic:fortnite");
    }
}
