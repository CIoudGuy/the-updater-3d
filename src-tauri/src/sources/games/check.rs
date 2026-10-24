//! What the launchers' own files do not say, read with every check for updates: the newest build
//! GOG lists for a GOG game (its public build list), and for a game from any other launcher its
//! Steam page, whose official announcements are its patch notes when it is sold on Steam too
//! (found by its exact name in Steam's public store search). Only a public product id or the
//! game's name is sent; nothing about the PC or its accounts.

use super::{Game, Launcher, NotesFrom};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// What the check found.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    /// Game id -> the newer version its launcher's public list has
    pub updates: HashMap<String, String>,
    /// Game id -> the Steam app with its patch notes, for a game not installed through Steam
    pub steam: HashMap<String, u32>,
}

/// Most games looked up in one check.
const MAX_GAMES: usize = 60;

/// The newer build in GOG's public build list (`generation=2` answer) for a game at `installed`
/// (its version, or its build id). None when it is the newest, or when it matches no build at
/// all (a version written another way is not taken for an update).
pub fn gog_update(installed: &str, json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    let mut builds: Vec<(String, String, String)> = v["items"]
        .as_array()?
        .iter()
        // The default branch only: a beta branch is not what an installed copy moves to.
        .filter(|b| b["branch"].is_null() && b["public"].as_bool() != Some(false))
        .map(|b| (b["date_published"].as_str().unwrap_or("").to_string(), b["build_id"].as_str().unwrap_or("").to_string(), b["version_name"].as_str().unwrap_or("").trim().to_string()))
        .collect();
    builds.sort_by(|a, b| b.0.cmp(&a.0));
    let (_, newest_id, newest_name) = builds.first()?.clone();
    let me = installed.trim();
    if me.is_empty() {
        return None;
    }
    let is = |id: &str, name: &str| me == id || me.eq_ignore_ascii_case(name);
    if is(&newest_id, &newest_name) {
        return None;
    }
    builds[1..].iter().any(|(_, id, name)| is(id, name)).then(|| if newest_name.is_empty() { newest_id } else { newest_name })
}

async fn get(url: &str, max_age: Duration) -> Option<String> {
    let f = tokio::time::timeout(Duration::from_secs(15), crate::http::get_cached(url, &[], max_age)).await.ok()?.ok()?;
    Some(String::from_utf8_lossy(&f.body).into_owned())
}

async fn gog(g: &Game) -> Option<String> {
    let id: u64 = g.key.parse().ok()?;
    for generation in [2, 1] {
        let url = format!("https://content-system.gog.com/products/{id}/os/windows/builds?generation={generation}");
        let Some(json) = get(&url, Duration::from_secs(6 * 3600)).await else { continue };
        let has_builds = serde_json::from_str::<Value>(&json).ok().is_some_and(|v| v["items"].as_array().is_some_and(|a| !a.is_empty()));
        if has_builds {
            return gog_update(&g.version, &json);
        }
    }
    None
}

/// The Steam app that is the same game, by its exact name (an edition word may follow it).
async fn steam_twin(g: &Game) -> Option<u32> {
    let json = get(&crate::sources::proglogo::steam_search_url(&g.name), Duration::from_secs(7 * 24 * 3600)).await?;
    crate::sources::proglogo::steam_pick(&json, &g.name)
}

/// Reads what the launchers' public sources say about these games.
pub async fn check(games: &[Game]) -> Feed {
    let jobs: Vec<Game> = games.iter().filter(|g| g.launcher == Launcher::Gog || (g.launcher != Launcher::Steam && g.notes == NotesFrom::None)).take(MAX_GAMES).cloned().collect();
    let found: Vec<(String, Option<String>, Option<u32>)> = futures_util::stream::iter(jobs)
        .map(|g| async move {
            let update = if g.launcher == Launcher::Gog { gog(&g).await } else { None };
            let steam = if g.notes == NotesFrom::None { steam_twin(&g).await } else { None };
            (g.id(), update, steam)
        })
        .buffer_unordered(3)
        .collect()
        .await;
    let mut out = Feed::default();
    for (id, update, steam) in found {
        if let Some(u) = update {
            out.updates.insert(id.clone(), u);
        }
        if let Some(s) = steam {
            out.steam.insert(id, s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const CYBERPUNK: &str = r#"{"total_count":3,"count":3,"items":[
        {"build_id":"60152385347545148","product_id":1423049311,"os":"windows","branch":null,"version_name":"2.5_RedModFix","public":true,"date_published":"2026-10-22T10:00:00+0300","generation":2},
        {"build_id":"60148683633405433","product_id":1423049311,"os":"windows","branch":null,"version_name":"2.5","public":true,"date_published":"2026-10-21T10:00:00+0300","generation":2},
        {"build_id":"60200000000000000","product_id":1423049311,"os":"windows","branch":"beta","version_name":"2.6 beta","public":true,"date_published":"2026-10-23T10:00:00+0300","generation":2},
        {"build_id":"58989493373906337","product_id":1423049311,"os":"windows","branch":null,"version_name":"2.31a","public":true,"date_published":"2025-09-16T10:00:00+0300","generation":2}
    ]}"#;

    #[test]
    fn a_gog_game_on_an_older_build_has_the_newest_one_waiting() {
        assert_eq!(gog_update("2.5", CYBERPUNK).as_deref(), Some("2.5_RedModFix"));
        assert_eq!(gog_update("2.31a", CYBERPUNK).as_deref(), Some("2.5_RedModFix"));
        // By its build id too.
        assert_eq!(gog_update("60148683633405433", CYBERPUNK).as_deref(), Some("2.5_RedModFix"));
    }

    #[test]
    fn the_newest_a_beta_or_an_unknown_version_is_no_update() {
        assert_eq!(gog_update("2.5_RedModFix", CYBERPUNK), None);
        assert_eq!(gog_update("60152385347545148", CYBERPUNK), None);
        // A beta branch build is not offered, and a version that matches no build is not taken
        // for an old one.
        assert_eq!(gog_update("2.6 beta", CYBERPUNK), None);
        assert_eq!(gog_update("v2.5.0 (GOG)", CYBERPUNK), None);
        assert_eq!(gog_update("", CYBERPUNK), None);
        assert_eq!(gog_update("2.5", "not json"), None);
    }

    /// This PC's games with the public answers, and the patch notes each one gets.
    #[tokio::test]
    #[ignore]
    async fn live_this_pc() {
        let games = crate::sources::games::scan();
        let f = check(&games).await;
        for g in &games {
            let twin = f.steam.get(&g.id()).copied();
            let notes = crate::sources::games::news(g, twin).await;
            eprintln!("{} | update {:?} | steam twin {:?} | {} patch notes, first: {:?}", g.name, g.update.clone().or_else(|| f.updates.get(&g.id()).cloned()), twin, notes.len(), notes.first().map(|v| &v.version));
        }
    }

    /// GOG's real list and Steam's real search: `cargo test games::check -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_public_answers() {
        let gog_game = Game { launcher: Launcher::Gog, key: "1423049311".into(), name: "Cyberpunk 2077".into(), version: "2.31a".into(), update: None, install_dir: None, size: None, updated_at: None, launch_uri: None, show_uri: None, uninstall_uri: None, page: None, notes: NotesFrom::Gog(1423049311), icon: None, program: None };
        let rdr2 = Game { launcher: Launcher::Rockstar, key: "rdr2".into(), name: "Red Dead Redemption 2".into(), notes: NotesFrom::None, ..gog_game.clone() };
        let f = check(&[gog_game.clone(), rdr2.clone()]).await;
        eprintln!("{f:?}");
        assert!(f.updates.contains_key(&gog_game.id()));
        assert_eq!(f.steam.get(&rdr2.id()), Some(&1174180));
    }
}
