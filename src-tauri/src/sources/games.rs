//! Games and apps installed through game launchers (Steam, Epic Games, Ubisoft Connect, GOG
//! Galaxy, EA app, Battle.net, Amazon Games, itch, Rockstar). Each launcher keeps its own list of
//! what it installed on this PC; those files are read here, nothing is signed in to and nothing
//! about the PC or its accounts is sent anywhere. A launcher downloads and installs its own
//! updates: when it has marked one, Update opens it there. Patch notes come from public sources
//! where a launcher has them (Steam's news, GOG's changelogs).

use std::path::{Path, PathBuf};

pub mod amazon;
pub mod battlenet;
pub mod ea;
pub mod epic;
pub mod gog;
pub mod itch;
pub mod rockstar;
pub mod steam;
pub mod ubisoft;

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
    // The launchers that leave only an uninstall entry share one read of the registry list.
    let uninstall = crate::winutil::uninstall_entries();
    let mut out = steam::scan();
    out.extend(epic::scan());
    out.extend(ubisoft::scan());
    out.extend(gog::scan());
    out.extend(ea::scan_entries(&uninstall));
    out.extend(battlenet::scan_entries(&uninstall));
    out.extend(amazon::scan());
    out.extend(itch::scan());
    out.extend(rockstar::scan_entries(&uninstall));
    out.retain(|g| !g.name.trim().is_empty());
    out
}

/// A text file's contents whatever its encoding: UTF-8 (with or without a byte order mark) or
/// UTF-16. Anything that is not valid text comes out with replacement characters.
pub(crate) fn decode_text(b: &[u8]) -> String {
    let utf16 = |bytes: &[u8], le: bool| {
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| if le { u16::from_le_bytes([c[0], c[1]]) } else { u16::from_be_bytes([c[0], c[1]]) }).collect();
        String::from_utf16_lossy(&units)
    };
    match b {
        [0xFF, 0xFE, rest @ ..] => utf16(rest, true),
        [0xFE, 0xFF, rest @ ..] => utf16(rest, false),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        // No mark: an ASCII character followed by a zero byte is UTF-16 too.
        [a, 0, ..] if *a != 0 => utf16(b, true),
        [0, a, ..] if *a != 0 => utf16(b, false),
        _ => String::from_utf8_lossy(b).into_owned(),
    }
}

/// A folder a launcher wrote down ("D:/Games/Some Game/", `"C:\Games\X"`): quotes and a trailing
/// separator removed, slashes turned to backslashes. None when it is empty.
pub(crate) fn dir_of(s: &str) -> Option<PathBuf> {
    let t = s.trim().trim_matches('"').trim();
    if t.is_empty() {
        return None;
    }
    let mut t = t.replace('/', "\\");
    while t.len() > 3 && t.ends_with('\\') {
        t.pop();
    }
    Some(PathBuf::from(t))
}

/// A registry value as text whatever its type: "1234" as a string or as a number.
pub(crate) fn reg_text(key: &winreg::RegKey, name: &str) -> String {
    if let Ok(s) = key.get_value::<String, _>(name) {
        return s.trim().to_string();
    }
    if let Ok(n) = key.get_value::<u32, _>(name) {
        return n.to_string();
    }
    key.get_value::<u64, _>(name).map(|n| n.to_string()).unwrap_or_default()
}

/// Runs `f` on a launcher's SQLite file, opened read-only. When that fails (the launcher has the
/// file locked), a private copy of the file and its journal is read instead. None when neither
/// works or `f` fails.
pub(crate) fn with_db<T>(path: &Path, f: impl Fn(&rusqlite::Connection) -> rusqlite::Result<T>) -> Option<T> {
    use rusqlite::{Connection, OpenFlags};
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    if let Ok(c) = Connection::open_with_flags(path, flags) {
        if let Ok(v) = f(&c) {
            return Some(v);
        }
    }
    let name = path.file_name()?;
    let dir = std::env::temp_dir().join(format!("updater3d-db-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).ok()?;
    let copy = dir.join(name);
    let result = (|| {
        std::fs::copy(path, &copy).ok()?;
        for ext in ["-wal", "-shm", "-journal"] {
            let (mut from, mut to) = (path.as_os_str().to_owned(), copy.as_os_str().to_owned());
            from.push(ext);
            to.push(ext);
            if Path::new(&from).is_file() {
                let _ = std::fs::copy(&from, &to);
            }
        }
        let c = Connection::open_with_flags(&copy, flags).ok()?;
        f(&c).ok()
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// A SQLite value as text ("" for null or a blob).
pub(crate) fn db_text(v: rusqlite::types::ValueRef<'_>) -> String {
    use rusqlite::types::ValueRef;
    match v {
        ValueRef::Text(t) => String::from_utf8_lossy(t).trim().to_string(),
        ValueRef::Integer(i) => i.to_string(),
        ValueRef::Real(r) => r.to_string(),
        _ => String::new(),
    }
}

/// Every row of `sql` as its columns by lower case name, so a launcher that adds, renames or
/// drops a column costs the one field and not the whole list.
pub(crate) fn db_rows(conn: &rusqlite::Connection, sql: &str) -> rusqlite::Result<Vec<std::collections::HashMap<String, String>>> {
    let mut st = conn.prepare(sql)?;
    let names: Vec<String> = st.column_names().iter().map(|n| n.to_ascii_lowercase()).collect();
    let mut rows = st.query([])?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let mut m = std::collections::HashMap::new();
        for (i, n) in names.iter().enumerate() {
            m.insert(n.clone(), db_text(r.get_ref(i)?));
        }
        out.push(m);
    }
    Ok(out)
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
    fn text_files_are_read_in_any_of_the_encodings_launchers_write() {
        assert_eq!(decode_text(b"plain"), "plain");
        assert_eq!(decode_text(b"\xEF\xBB\xBFwith mark"), "with mark");
        let utf16 = |s: &str, mark: &[u8], le: bool| -> Vec<u8> { mark.iter().copied().chain(s.encode_utf16().flat_map(move |u| if le { u.to_le_bytes() } else { u.to_be_bytes() })).collect() };
        assert_eq!(decode_text(&utf16("<a>caf\u{e9}</a>", &[0xFF, 0xFE], true)), "<a>caf\u{e9}</a>");
        assert_eq!(decode_text(&utf16("<a/>", &[0xFE, 0xFF], false)), "<a/>");
        // No mark: still UTF-16.
        assert_eq!(decode_text(&utf16("<a/>", &[], true)), "<a/>");
        assert_eq!(decode_text(b""), "");
        assert_eq!(decode_text(b"x"), "x");
    }

    #[test]
    fn a_folder_a_launcher_wrote_down_is_made_plain() {
        assert_eq!(dir_of("D:/Games/Some Game/").as_deref(), Some(Path::new(r"D:\Games\Some Game")));
        assert_eq!(dir_of(r#" "C:\Games\X\" "#).as_deref(), Some(Path::new(r"C:\Games\X")));
        assert_eq!(dir_of(r"D:\").as_deref(), Some(Path::new(r"D:\")));
        assert_eq!(dir_of("  "), None);
        assert_eq!(dir_of(""), None);
    }

    #[test]
    fn a_database_is_read_through_a_copy_when_it_cannot_be_opened_in_place() {
        let dir = std::env::temp_dir().join(format!("updater3d-db-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("x.db");
        {
            let c = rusqlite::Connection::open(&db).unwrap();
            c.execute_batch("CREATE TABLE t (id INTEGER, name TEXT, note BLOB); INSERT INTO t VALUES (1, 'one', x'00'), (2, NULL, NULL);").unwrap();
        }
        let rows = with_db(&db, |c| db_rows(c, "SELECT * FROM t ORDER BY id")).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], "one");
        assert_eq!(rows[0]["note"], "");
        assert_eq!(rows[1]["name"], "");
        // A query that fails (another launcher's file) is no result; so is a file that is not there.
        assert!(with_db(&db, |c| db_rows(c, "SELECT * FROM missing")).is_none());
        assert!(with_db(&dir.join("absent.db"), |c| db_rows(c, "SELECT 1")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Run with `cargo test live_scan -- --ignored --nocapture`: what the launchers on this PC list.
    #[test]
    #[ignore]
    fn live_scan() {
        let games = scan();
        for g in &games {
            let dir = g.install_dir.as_deref();
            println!(
                "{} | {} | version {:?} | update {:?} | folder {} | size {:?} | launch {:?} | show {:?} | uninstall {:?} | icon {:?}",
                g.id(),
                g.name,
                g.version,
                g.update,
                dir.map(|d| format!("{} (exists: {})", d.display(), d.is_dir())).unwrap_or_default(),
                g.size,
                g.launch_uri,
                g.show_uri,
                g.uninstall_uri,
                g.icon.as_deref().map(|i| format!("{} (exists: {})", i.display(), i.is_file()))
            );
            for u in [&g.launch_uri, &g.show_uri, &g.uninstall_uri].into_iter().flatten() {
                assert!(own_uri(g.launcher, u), "{u}");
            }
        }
        let mut ids: Vec<String> = games.iter().map(Game::id).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), games.len(), "ids are unique");
        println!("{} games", games.len());
    }

    #[test]
    fn a_game_id_names_its_launcher() {
        let g = Game { launcher: Launcher::Epic, key: "Fortnite".into(), name: "Fortnite".into(), version: String::new(), update: None, install_dir: None, size: None, updated_at: None, launch_uri: None, show_uri: None, uninstall_uri: None, page: None, notes: NotesFrom::None, icon: None };
        assert_eq!(g.id(), "game:epic:fortnite");
    }
}
