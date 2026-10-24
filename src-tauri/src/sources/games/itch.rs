//! itch: the games the itch app installed are the rows of the table `caves` in
//! `%APPDATA%\itch\db\butler.db`. A cave's folder is, in itch's own order: its
//! `custom_install_folder`, else the path of its `install_locations` row joined with its
//! `install_folder_name`, else the `basePath` of its `verdict` (JSON). Titles come from the
//! `games` table. itch documents the address `itch://install?game_id=<id>&launch`, which starts an
//! installed game (the app asks to confirm). It keeps no marker for a waiting update there, so
//! none is reported. The columns are read by name, so a column itch drops costs that one field.

use super::{db_rows, dir_of, own_uri, with_db, Game, Launcher, NotesFrom};
use std::collections::HashMap;
use std::path::PathBuf;

/// A row by lower case column name.
pub type Row = HashMap<String, String>;

fn get<'a>(r: &'a Row, k: &str) -> &'a str {
    r.get(k).map(|v| v.trim()).unwrap_or("")
}

/// The `basePath` in a cave's verdict.
pub fn verdict_base_path(verdict: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(verdict).ok()?;
    let p = v.get("basePath")?.as_str()?.trim();
    (!p.is_empty()).then(|| p.to_string())
}

/// The folder a cave is installed in.
pub fn cave_folder(cave: &Row, locations: &[Row]) -> Option<PathBuf> {
    if let Some(d) = dir_of(get(cave, "custom_install_folder")) {
        return Some(d);
    }
    let (loc_id, folder) = (get(cave, "install_location_id"), get(cave, "install_folder_name"));
    if !loc_id.is_empty() && !folder.is_empty() {
        if let Some(base) = locations.iter().find(|l| get(l, "id") == loc_id).and_then(|l| dir_of(get(l, "path"))) {
            return Some(base.join(folder));
        }
    }
    dir_of(&verdict_base_path(get(cave, "verdict"))?)
}

/// The game a cave stands for. `games` are the rows of the `games` table (for the title).
pub fn game_of(cave: &Row, locations: &[Row], games: &[Row]) -> Option<Game> {
    let dir = cave_folder(cave, locations)?;
    let game_id = get(cave, "game_id");
    let title = games.iter().find(|g| !game_id.is_empty() && get(g, "id") == game_id).map(|g| get(g, "title")).filter(|t| !t.is_empty());
    let name = title.map(str::to_string).or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()))?;
    let key = [get(cave, "id"), game_id].into_iter().find(|k| !k.is_empty())?.to_string();
    let numeric = !game_id.is_empty() && game_id.len() <= 18 && game_id.chars().all(|c| c.is_ascii_digit());
    Some(Game {
        launcher: Launcher::Itch,
        key,
        name,
        version: String::new(),
        update: None,
        install_dir: Some(dir),
        size: get(cave, "installed_size").parse::<u64>().ok().filter(|s| *s > 0),
        updated_at: None,
        launch_uri: numeric.then(|| format!("itch://install?game_id={game_id}&launch")).filter(|u| own_uri(Launcher::Itch, u)),
        show_uri: None,
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
        program: None,
    })
}

/// The games in an open `butler.db`.
pub fn games_in(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<Game>> {
    let caves = db_rows(conn, "SELECT * FROM caves")?;
    let locations = db_rows(conn, "SELECT * FROM install_locations").unwrap_or_default();
    // Only the games the caves are of: numbers, so the list is safe to put in the query.
    let ids: Vec<&str> = caves.iter().map(|c| get(c, "game_id")).filter(|g| !g.is_empty() && g.len() <= 18 && g.chars().all(|c| c.is_ascii_digit())).collect();
    let games = if ids.is_empty() { vec![] } else { db_rows(conn, &format!("SELECT * FROM games WHERE id IN ({})", ids.join(","))).unwrap_or_default() };
    let mut out: Vec<Game> = Vec::new();
    for g in caves.iter().filter_map(|c| game_of(c, &locations, &games)) {
        if g.install_dir.as_deref().is_some_and(std::path::Path::is_dir) && !out.iter().any(|o| o.key == g.key) {
            out.push(g);
        }
    }
    Ok(out)
}

/// Every game the itch app installed on this PC.
pub fn scan() -> Vec<Game> {
    let Some(roaming) = std::env::var_os("APPDATA") else { return vec![] };
    let db = PathBuf::from(roaming).join("itch").join("db").join("butler.db");
    if !db.is_file() {
        return vec![];
    }
    with_db(&db, games_in).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use std::path::Path;

    fn row(pairs: &[(&str, &str)]) -> Row {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn a_caves_folder_follows_itchs_own_order() {
        let locs = [row(&[("id", "loc-1"), ("path", "D:\\itch\\apps")])];
        // A custom folder wins.
        let c = row(&[("custom_install_folder", "E:\\Mine\\Game"), ("install_location_id", "loc-1"), ("install_folder_name", "game")]);
        assert_eq!(cave_folder(&c, &locs).as_deref(), Some(Path::new(r"E:\Mine\Game")));
        // Else the location's path and the folder name.
        let c = row(&[("install_location_id", "loc-1"), ("install_folder_name", "game")]);
        assert_eq!(cave_folder(&c, &locs).as_deref(), Some(Path::new(r"D:\itch\apps\game")));
        // Else the verdict's base path, when itch no longer knows the location.
        let c = row(&[("install_location_id", "gone"), ("install_folder_name", "game"), ("verdict", r#"{"basePath":"C:\\Users\\Someone\\itch\\apps\\game","totalSize":12}"#)]);
        assert_eq!(cave_folder(&c, &locs).as_deref(), Some(Path::new(r"C:\Users\Someone\itch\apps\game")));
        assert_eq!(cave_folder(&row(&[("verdict", "not json")]), &locs), None);
        assert_eq!(cave_folder(&row(&[]), &locs), None);
    }

    #[test]
    fn a_cave_gives_the_game_with_its_title_and_launch_address() {
        let locs = [row(&[("id", "loc-1"), ("path", "D:\\itch\\apps")])];
        let games = [row(&[("id", "123456"), ("title", "Some Itch Game")]), row(&[("id", "9"), ("title", "Other")])];
        let c = row(&[("id", "cave-uuid-1"), ("game_id", "123456"), ("install_location_id", "loc-1"), ("install_folder_name", "some-itch-game"), ("installed_size", "4096")]);
        let g = game_of(&c, &locs, &games).unwrap();
        assert_eq!(g.id(), "game:itch:cave-uuid-1");
        assert_eq!(g.name, "Some Itch Game");
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"D:\itch\apps\some-itch-game")));
        assert_eq!(g.size, Some(4096));
        assert_eq!(g.launch_uri.as_deref(), Some("itch://install?game_id=123456&launch"));
        assert!(g.launch_uri.as_deref().is_some_and(|u| own_uri(Launcher::Itch, u)));
        assert_eq!((g.show_uri, g.update, g.version.as_str()), (None, None, ""));
        // No title known: the folder names it. No numeric game id: no launch address.
        let c = row(&[("id", "cave-uuid-2"), ("game_id", "x1"), ("install_location_id", "loc-1"), ("install_folder_name", "mystery")]);
        let g = game_of(&c, &locs, &games).unwrap();
        assert_eq!(g.name, "mystery");
        assert!(g.launch_uri.is_none());
    }

    #[test]
    fn the_database_is_read_with_whatever_columns_it_has() {
        let here = std::env::temp_dir();
        let c = Connection::open_in_memory().unwrap();
        c.execute_batch(
            "CREATE TABLE install_locations (id TEXT PRIMARY KEY, path TEXT);
             CREATE TABLE games (id INTEGER PRIMARY KEY, title TEXT, short_text TEXT);
             CREATE TABLE caves (id TEXT PRIMARY KEY, game_id INTEGER, install_location_id TEXT, install_folder_name TEXT, custom_install_folder TEXT, verdict TEXT, installed_at TEXT, seconds_run INTEGER);",
        )
        .unwrap();
        c.execute("INSERT INTO install_locations VALUES ('loc', ?1)", [here.parent().unwrap_or(&here).to_string_lossy().to_string()]).unwrap();
        c.execute("INSERT INTO games VALUES (7, 'Kept Game', 'x'), (8, 'Gone Game', 'y')", []).unwrap();
        let folder = here.file_name().unwrap().to_string_lossy().to_string();
        c.execute("INSERT INTO caves VALUES ('c1', 7, 'loc', ?1, NULL, NULL, '2026-01-01', 10)", [folder]).unwrap();
        c.execute("INSERT INTO caves VALUES ('c2', 8, 'loc', 'no-such-folder-here', NULL, NULL, '2026-01-01', 10)", []).unwrap();
        let games = games_in(&c).unwrap();
        assert_eq!(games.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Kept Game"]);
        assert_eq!(games[0].launch_uri.as_deref(), Some("itch://install?game_id=7&launch"));
        // Another launcher's file has no caves table.
        assert!(games_in(&Connection::open_in_memory().unwrap()).is_err());
    }
}
