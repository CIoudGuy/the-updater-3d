//! Games this app installed itself (from Epic Games or GOG, signed in): their folders are kept
//! in a list, and each folder's `.u3d\game.json` (`sync::State`) says which game and build it
//! holds. They update, repair and undo here, not in a launcher.

use super::sync::{read_state, safe_rel, state_file, State};
use super::{Game, Launcher, NotesFrom};
use std::path::{Path, PathBuf};

fn list_file() -> PathBuf {
    let d = crate::paths::data_dir().join("accounts");
    let _ = std::fs::create_dir_all(&d);
    d.join("games-here.json")
}

fn same(a: &Path, b: &Path) -> bool {
    let n = |p: &Path| p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase();
    n(a) == n(b)
}

/// The folders games were installed into here.
pub fn folders() -> Vec<PathBuf> {
    std::fs::read(list_file()).ok().and_then(|b| serde_json::from_slice::<Vec<PathBuf>>(&b).ok()).unwrap_or_default()
}

fn save(v: &[PathBuf]) {
    if let Ok(b) = serde_json::to_vec_pretty(v) {
        let f = list_file();
        let tmp = f.with_extension("json.part");
        if std::fs::write(&tmp, b).is_ok() {
            let _ = std::fs::rename(&tmp, &f);
        }
    }
}

/// Keep `dir` in the list (before its install starts, so a half-done install is found again).
pub fn remember(dir: &Path) {
    let mut v = folders();
    if !v.iter().any(|d| same(d, dir)) {
        v.push(dir.to_path_buf());
        save(&v);
    }
}

pub fn forget(dir: &Path) {
    let mut v = folders();
    let n = v.len();
    v.retain(|d| !same(d, dir));
    if v.len() != n {
        save(&v);
    }
}

fn installing_file(dir: &Path) -> PathBuf {
    dir.join(super::sync::OWN_DIR).join("installing.json")
}

#[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
struct Installing {
    platform: String,
    game: String,
}

/// A new install starts in `dir`: the folder is made and says which game is being installed, so
/// an install that stops halfway is continued there (see `unfinished`).
pub fn begin_install(dir: &Path, platform: &str, game: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join(super::sync::OWN_DIR))?;
    let b = serde_json::to_vec(&Installing { platform: platform.into(), game: game.into() }).unwrap_or_default();
    std::fs::write(installing_file(dir), b)
}

/// The install in `dir` finished.
pub fn end_install(dir: &Path) {
    let _ = std::fs::remove_file(installing_file(dir));
}

/// The folder of an install of this game that stopped halfway (it failed), to go on in.
pub fn unfinished(platform: &str, game: &str) -> Option<PathBuf> {
    folders().into_iter().find(|d| {
        !state_file(d).is_file()
            && std::fs::read(installing_file(d)).ok().and_then(|b| serde_json::from_slice::<Installing>(&b).ok()).is_some_and(|i| i.platform == platform && i.game.eq_ignore_ascii_case(game))
    })
}

pub fn launcher_of(platform: &str) -> Option<Launcher> {
    match platform {
        "epic" => Some(Launcher::Epic),
        "gog" => Some(Launcher::Gog),
        _ => None,
    }
}

/// Is this game one the app installed (and so updates here)?
pub fn is_here(g: &Game) -> bool {
    g.install_dir.as_deref().is_some_and(|d| state_file(d).is_file())
}

/// The game in `dir`, from its state.
pub fn game_of(dir: &Path, s: &State) -> Option<Game> {
    let launcher = launcher_of(&s.platform)?;
    if s.game.trim().is_empty() || s.name.trim().is_empty() {
        return None;
    }
    let updated_at = std::fs::metadata(state_file(dir)).and_then(|m| m.modified()).ok().map(|t| chrono::DateTime::<chrono::Utc>::from(t).format("%Y-%m-%dT%H:%M:%SZ").to_string());
    let program = safe_rel(&s.launch).map(|r| dir.join(r)).filter(|p| p.is_file());
    Some(Game {
        launcher,
        key: s.game.clone(),
        name: s.name.clone(),
        version: if s.version.trim().is_empty() { s.build.clone() } else { s.version.clone() },
        update: None,
        install_dir: Some(dir.to_path_buf()),
        size: None,
        updated_at,
        launch_uri: None,
        show_uri: None,
        uninstall_uri: None,
        page: None,
        notes: match launcher {
            Launcher::Gog => s.game.parse().map(NotesFrom::Gog).unwrap_or(NotesFrom::None),
            _ => NotesFrom::None,
        },
        icon: None,
        program,
    })
}

/// Every game installed here whose folder is still there. Folders that are gone leave the list.
pub fn scan() -> Vec<Game> {
    let dirs = folders();
    let mut out = Vec::new();
    let mut keep = Vec::new();
    for d in dirs {
        if !d.is_dir() {
            continue;
        }
        keep.push(d.clone());
        if let Some(g) = read_state(&d).and_then(|s| game_of(&d, &s)) {
            out.push(g);
        }
    }
    if keep.len() != folders().len() {
        save(&keep);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_with_its_state_is_a_game_installed_here() {
        let dir = std::env::temp_dir().join(format!("u3d-here-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Bin")).unwrap();
        std::fs::write(dir.join("Bin").join("Game.exe"), b"MZ").unwrap();
        let s = State { platform: "gog".into(), game: "1207658924".into(), name: "Unreal Tournament 2004".into(), version: "3369".into(), build: "5512".into(), launch: "Bin/Game.exe".into(), launch_args: vec![] };
        super::super::sync::write_state(&dir, &s).unwrap();
        let g = game_of(&dir, &read_state(&dir).unwrap()).unwrap();
        assert_eq!((g.launcher, g.key.as_str(), g.version.as_str()), (Launcher::Gog, "1207658924", "3369"));
        assert_eq!(g.id(), "game:gog:1207658924");
        assert_eq!(g.program.as_deref(), Some(dir.join("Bin").join("Game.exe").as_path()));
        assert!(matches!(g.notes, NotesFrom::Gog(1207658924)));
        assert!(is_here(&g));
        // A launch path that leaves the folder is not started.
        let bad = State { launch: "../../Windows/notepad.exe".into(), ..s.clone() };
        assert_eq!(game_of(&dir, &bad).unwrap().program, None);
        // Not a platform the app installs from.
        assert!(game_of(&dir, &State { platform: "steam".into(), ..s }).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_install_that_stopped_halfway_is_found_again() {
        let dir = std::env::temp_dir().join(format!("u3d-half-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        begin_install(&dir, "gog", "1207658924").unwrap();
        // Only listed folders are looked at; the test checks the marker itself.
        let b = std::fs::read(installing_file(&dir)).unwrap();
        assert_eq!(serde_json::from_slice::<Installing>(&b).unwrap(), Installing { platform: "gog".into(), game: "1207658924".into() });
        assert!(!state_file(&dir).is_file());
        end_install(&dir);
        assert!(!installing_file(&dir).is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
