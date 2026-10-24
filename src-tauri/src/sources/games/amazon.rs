//! Amazon Games: the games it installed are rows of the table `DbSet` in
//! `%LOCALAPPDATA%\Amazon Games\Data\Games\Sql\GameInstallInfo.sqlite` (`Id`, `ProductTitle`,
//! `InstallDirectory`, and `Installed` = 1 for the ones on the PC). The columns are read by name, so
//! a column the launcher drops costs that one field. Amazon Games keeps no marker for a waiting
//! update in this file, so none is reported.

use super::{db_rows, dir_of, own_uri, with_db, Game, Launcher, NotesFrom};
use std::collections::HashMap;
use std::path::PathBuf;

/// A row of `DbSet` by lower case column name.
pub type Row = HashMap<String, String>;

/// Is `s` safe to put in an address (letters, digits and ". _ : -")?
fn plain(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// The game a row stands for. None when the row is not installed or has no id, title or folder.
pub fn game_of(r: &Row) -> Option<Game> {
    let get = |k: &str| r.get(k).map(|v| v.trim()).unwrap_or("");
    // A table without the column lists installed games only.
    let installed = r.get("installed").is_none_or(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    let (id, title) = (get("id"), get("producttitle"));
    if !installed || id.is_empty() || title.is_empty() {
        return None;
    }
    let dir = dir_of(get("installdirectory"))?;
    Some(Game {
        launcher: Launcher::Amazon,
        key: id.to_string(),
        name: title.to_string(),
        version: get("productversion").to_string(),
        update: None,
        install_dir: Some(dir),
        size: None,
        updated_at: None,
        launch_uri: Some(format!("amazon-games://play/{id}")).filter(|u| plain(id) && own_uri(Launcher::Amazon, u)),
        show_uri: None,
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
        program: None,
    })
}

/// The games in an open `GameInstallInfo.sqlite`.
pub fn games_in(conn: &rusqlite::Connection) -> rusqlite::Result<Vec<Game>> {
    let mut out: Vec<Game> = Vec::new();
    for g in db_rows(conn, "SELECT * FROM DbSet")?.iter().filter_map(game_of) {
        if g.install_dir.as_deref().is_some_and(std::path::Path::is_dir) && !out.iter().any(|o| o.key == g.key) {
            out.push(g);
        }
    }
    Ok(out)
}

/// Every game Amazon Games installed on this PC.
pub fn scan() -> Vec<Game> {
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return vec![] };
    let db = PathBuf::from(local).join("Amazon Games").join("Data").join("Games").join("Sql").join("GameInstallInfo.sqlite");
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
    fn an_installed_row_gives_the_game() {
        let r = row(&[("id", "amzn1.adg.product.11111111-2222-3333-4444-555555555555"), ("producttitle", "Some Amazon Game"), ("installdirectory", "D:\\Amazon Games\\Library\\Some Amazon Game"), ("installed", "1"), ("productversion", "1.0.7")]);
        let g = game_of(&r).unwrap();
        assert_eq!(g.id(), "game:amazon:amzn1.adg.product.11111111-2222-3333-4444-555555555555");
        assert_eq!(g.name, "Some Amazon Game");
        assert_eq!(g.version, "1.0.7");
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"D:\Amazon Games\Library\Some Amazon Game")));
        assert_eq!(g.launch_uri.as_deref(), Some("amazon-games://play/amzn1.adg.product.11111111-2222-3333-4444-555555555555"));
        assert!(g.launch_uri.as_deref().is_some_and(|u| own_uri(Launcher::Amazon, u)));
        assert_eq!(g.update, None);
    }

    #[test]
    fn rows_that_are_not_installed_games_give_nothing() {
        let ok = [("id", "x1"), ("producttitle", "T"), ("installdirectory", "D:\\G"), ("installed", "1")];
        assert!(game_of(&row(&ok)).is_some());
        let mut not = ok.to_vec();
        not[3] = ("installed", "0");
        assert!(game_of(&row(&not)).is_none());
        for missing in ["id", "producttitle", "installdirectory"] {
            let r: Vec<(&str, &str)> = ok.iter().copied().filter(|(k, _)| *k != missing).collect();
            assert!(game_of(&row(&r)).is_none(), "{missing}");
        }
        // No Installed column: the table lists installed games only.
        let r: Vec<(&str, &str)> = ok.iter().copied().filter(|(k, _)| *k != "installed").collect();
        assert!(game_of(&row(&r)).is_some());
    }

    #[test]
    fn an_id_that_is_not_safe_in_an_address_has_no_launch_address() {
        let r = row(&[("id", "a b/c"), ("producttitle", "T"), ("installdirectory", "D:\\G"), ("installed", "1")]);
        assert!(game_of(&r).unwrap().launch_uri.is_none());
    }

    #[test]
    fn the_table_is_read_with_whatever_columns_it_has() {
        let here = std::env::temp_dir();
        let c = Connection::open_in_memory().unwrap();
        // The launcher's columns, plus ones this reader does not know, and no ProductVersion.
        c.execute("CREATE TABLE DbSet (Id TEXT PRIMARY KEY, ProductTitle TEXT, InstallDirectory TEXT, Installed INTEGER, ProductDescription TEXT, Blob BLOB)", []).unwrap();
        let add = |id: &str, t: &str, dir: &str, installed: i64| c.execute("INSERT INTO DbSet VALUES (?1, ?2, ?3, ?4, 'desc', x'00')", rusqlite::params![id, t, dir, installed]).unwrap();
        add("g1", "Kept Game", &here.to_string_lossy(), 1);
        add("g2", "Uninstalled Game", &here.to_string_lossy(), 0);
        add("g3", "Game With Gone Folder", "Z:\\No\\Such\\Folder", 1);
        let games = games_in(&c).unwrap();
        assert_eq!(games.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["Kept Game"]);
        assert_eq!(games[0].version, "");
        // Another launcher's file is not this table.
        let other = Connection::open_in_memory().unwrap();
        assert!(games_in(&other).is_err());
    }
}
