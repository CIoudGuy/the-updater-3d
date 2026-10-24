//! A launcher's game as an app of this app: listed with the others, updated and uninstalled in
//! its launcher, with its patch notes.

use super::{own_uri, Game, Launcher, NotesFrom};
use crate::model::{JobKind, Source, VersionInfo};
use crate::sources::{Installed, Latest, Plan, SourceData, Step, Verify, WAITING_FOR_YOU};
use crate::winutil::UninstallEntry;
use anyhow::{anyhow, bail, Result};
use std::time::Duration;

/// Two letters and colors for a game without a picture.
fn mark(name: &str, l: Launcher) -> (String, String, String) {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let mut code = String::new();
    if let Some(w) = words.first() {
        code.extend(w.chars().next().map(|c| c.to_ascii_uppercase()));
        match words.get(1) {
            Some(w2) => code.extend(w2.chars().next().map(|c| c.to_ascii_lowercase())),
            None => code.extend(w.chars().nth(1).map(|c| c.to_ascii_lowercase())),
        }
    }
    if code.is_empty() {
        code = "G".into();
    }
    let bg = match l {
        Launcher::Steam => "#1B2838",
        Launcher::Epic => "#2A2A2A",
        Launcher::Ubisoft => "#0B2540",
        Launcher::Gog => "#2D1B3D",
        Launcher::Ea => "#3A1414",
        Launcher::BattleNet => "#0E2A4A",
        Launcher::Amazon => "#2B2410",
        Launcher::Itch => "#3A1F1F",
        Launcher::Rockstar => "#3A2A0A",
    };
    (code, "#EDEDED".into(), bg.into())
}

/// "through Steam", for what the app says about a game.
pub fn through(l: Launcher) -> String {
    format!("through {}", l.name())
}

/// The game as an installed app.
pub fn installed(g: Game) -> Installed {
    let (code, fg, bg) = mark(&g.name, g.launcher);
    Installed {
        id: g.id(),
        source: Source::Games,
        name: g.name.clone(),
        code,
        fg,
        bg,
        // A launcher that keeps no version (Ubisoft Connect) still has it installed.
        version: if g.version.trim().is_empty() { "Installed".into() } else { g.version.clone() },
        install_dir: g.install_dir.clone(),
        exe: None,
        uninstall: None,
        updated_at: g.updated_at.clone(),
        size_hint: g.size,
        tagline: Some(format!("Installed and updated {}", through(g.launcher))),
        channel: None,
        missing: false,
        read_only: false,
        store: None,
        // The launcher's own picture of it (a file on this PC), copied into the icon cache.
        icon_remote: g.icon.as_ref().map(|p| p.to_string_lossy().into_owned()),
        data: SourceData::Game(Box::new(g)),
    }
}

fn norm(p: &str) -> String {
    p.trim().trim_matches('"').replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

/// The game's own entry in Windows' list: the one installed in the game's folder.
fn entry_of<'a>(g: &Game, entries: &'a [UninstallEntry]) -> Option<&'a UninstallEntry> {
    let dir = norm(&g.install_dir.as_ref()?.to_string_lossy());
    entries.iter().find(|e| !e.install_location.trim().is_empty() && norm(&e.install_location) == dir)
}

/// The game as an installed app. A launcher with no uninstall address (Rockstar, EA, Battle.net,
/// GOG) leaves the game's own uninstaller in Windows' list: that one is used.
pub fn installed_with(g: Game, entries: &[UninstallEntry]) -> Installed {
    let own = g.uninstall_uri.as_deref().is_some_and(|u| own_uri(g.launcher, u));
    let no_launch = g.launch_uri.as_deref().is_none_or(|u| !own_uri(g.launcher, u));
    let entry = entry_of(&g, entries).cloned();
    let mut i = installed(g);
    if let Some(e) = entry {
        if !own {
            i.uninstall = crate::uninstall::switches::choose(&e, crate::uninstall::switches::detect(&e, &[])).map(|c| c.args);
        }
        // The game's program, as its entry names it: Open starts it when the launcher has no
        // address for that, and its icon is read from it when the launcher keeps no picture.
        if no_launch || i.icon_remote.is_none() {
            i.exe = crate::sources::proglogo::display_icon(&e.display_icon).map(|(p, _)| p).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) && p.is_file());
        }
    }
    i
}

/// Uninstalling a game: through its launcher when it has an address for it, else with the
/// game's own uninstaller from Windows' list.
pub fn plan_uninstall_with(i: &Installed, entries: &[UninstallEntry]) -> Result<Plan> {
    if can_uninstall(i) {
        return plan_uninstall(i);
    }
    let g = game(i).ok_or_else(|| anyhow!("{} is not a launcher's game", i.name))?;
    let e = entry_of(g, entries).ok_or_else(|| anyhow!("{} is uninstalled in {}", i.name, g.launcher.name()))?;
    let prog = crate::uninstall::programs::make(e, None, g.install_dir.clone(), &|p| p.exists(), None);
    let mut p = crate::uninstall::run::plan(&prog)?;
    p.app_id = i.id.clone();
    p.name = i.name.clone();
    p.verify = Verify::Removed { app_id: i.id.clone() };
    Ok(p)
}

fn game(i: &Installed) -> Option<&Game> {
    match &i.data {
        SourceData::Game(g) => Some(g),
        _ => None,
    }
}

/// The update the launcher has marked.
pub fn latest(i: &Installed) -> Option<Latest> {
    let g = game(i)?;
    let u = g.update.as_ref()?;
    // No note: the page already says the launcher installs it.
    Some(Latest { version: if u.is_empty() { "New build".into() } else { u.clone() }, size: None, note: None })
}

/// Shows the game, and its waiting update, in its launcher.
pub fn show_uri(i: &Installed) -> Option<String> {
    let g = game(i)?;
    g.show_uri.clone().or_else(|| g.launch_uri.clone()).filter(|u| own_uri(g.launcher, u))
}

/// Starts the game through its launcher.
pub fn launch_uri(i: &Installed) -> Option<String> {
    let g = game(i)?;
    g.launch_uri.clone().or_else(|| g.show_uri.clone()).filter(|u| own_uri(g.launcher, u))
}

/// Can the launcher be asked to uninstall it?
pub fn can_uninstall(i: &Installed) -> bool {
    game(i).is_some_and(|g| g.uninstall_uri.as_deref().is_some_and(|u| own_uri(g.launcher, u)))
}

/// Uninstalling a game: its launcher asks the user to confirm and removes it with its saves
/// handled as the launcher does. The list shows it gone after the next scan.
pub fn plan_uninstall(i: &Installed) -> Result<Plan> {
    let Some(g) = game(i) else { bail!("{} is not a launcher's game", i.name) };
    let Some(uri) = g.uninstall_uri.clone().filter(|u| own_uri(g.launcher, u)) else { bail!("{} is uninstalled in {}", i.name, g.launcher.name()) };
    Ok(Plan {
        app_id: i.id.clone(),
        kind: JobKind::Uninstall,
        name: i.name.clone(),
        to_version: String::new(),
        downloads: vec![],
        steps: vec![Step::OpenUri { uri, label: format!("{WAITING_FOR_YOU}: confirm the uninstall in {}", g.launcher.name()) }],
        block_dirs: vec![],
        block_names: vec![],
        verify: Verify::None,
        fallback: None,
        cleanup: vec![],
    })
}

/// Unique titles for the news list (two posts can share one, "Patch notes").
fn unique(title: &str, seen: &mut Vec<String>) -> String {
    let mut t: String = title.chars().take(90).collect();
    let base = t.clone();
    let mut n = 2;
    while seen.contains(&t) {
        t = format!("{base} ({n})");
        n += 1;
    }
    seen.push(t.clone());
    t
}

/// The game's news and patch notes, newest first, as its version history (each item's title
/// in place of a version). None for launchers that publish nothing.
pub async fn news(g: &Game) -> Vec<VersionInfo> {
    match g.notes {
        NotesFrom::SteamNews(id) => {
            let url = format!("https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={id}&count=15&maxlength=0&format=json&feeds=steam_community_announcements");
            let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(15), crate::http::get_cached(&url, &[], Duration::from_secs(3600))).await else { return vec![] };
            steam_history(id, &String::from_utf8_lossy(&f.body))
        }
        NotesFrom::Gog(id) => {
            let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(15), crate::http::get_cached(&super::gog::changelog_url(id), &[], Duration::from_secs(12 * 3600))).await else { return vec![] };
            let mut seen = Vec::new();
            super::gog::changelog_items(&String::from_utf8_lossy(&f.body))
                .into_iter()
                .take(15)
                .enumerate()
                .map(|(n, (title, date, lines))| VersionInfo { version: unique(&title, &mut seen), date, notes: lines, notes_url: None, size: None, installed: false, latest: n == 0, prerelease: false })
                .collect()
        }
        NotesFrom::None => vec![],
    }
}

/// Steam's news answer for app `id` as a version history.
pub fn steam_history(id: u32, json: &str) -> Vec<VersionInfo> {
    let mut seen = Vec::new();
    crate::notes::steam_news_items(json)
        .into_iter()
        .take(15)
        .enumerate()
        .map(|(n, it)| VersionInfo {
            version: unique(&it.title, &mut seen),
            date: it.date,
            notes: it.lines,
            notes_url: Some(if it.gid.is_empty() { format!("https://store.steampowered.com/news/app/{id}") } else { format!("https://store.steampowered.com/news/app/{id}/view/{}", it.gid) }),
            size: None,
            installed: false,
            latest: n == 0,
            prerelease: false,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn starfield(update: Option<&str>) -> Game {
        Game {
            launcher: Launcher::Steam,
            key: "1912410".into(),
            name: "Starfield".into(),
            version: "24098430".into(),
            update: update.map(String::from),
            install_dir: Some(PathBuf::from(r"D:\SteamLibrary\steamapps\common\Starfield")),
            size: Some(140_000_000_000),
            updated_at: Some("2026-07-01T10:00:00Z".into()),
            launch_uri: Some("steam://rungameid/1912410".into()),
            show_uri: Some("steam://nav/games/details/1912410".into()),
            uninstall_uri: Some("steam://uninstall/1912410".into()),
            page: Some("https://store.steampowered.com/app/1912410/".into()),
            notes: NotesFrom::SteamNews(1912410),
            icon: None,
        }
    }

    #[test]
    fn a_launchers_game_is_an_app_that_updates_there() {
        let i = installed(starfield(Some("24555555")));
        assert_eq!(i.id, "game:steam:1912410");
        assert_eq!(i.source, Source::Games);
        assert_eq!(i.version, "24098430");
        assert!(!i.read_only);
        let l = latest(&i).unwrap();
        assert_eq!(l.version, "24555555");
        assert_eq!(show_uri(&i).as_deref(), Some("steam://nav/games/details/1912410"));
        assert_eq!(launch_uri(&i).as_deref(), Some("steam://rungameid/1912410"));
        assert!(can_uninstall(&i));
        // An update marked without a build still shows.
        assert_eq!(latest(&installed(starfield(Some("")))).unwrap().version, "New build");
        assert!(latest(&installed(starfield(None))).is_none());
    }

    #[test]
    fn a_game_is_uninstalled_by_its_launcher_only() {
        let p = plan_uninstall(&installed(starfield(None))).unwrap();
        assert!(matches!(&p.steps[..], [Step::OpenUri { uri, .. }] if uri == "steam://uninstall/1912410"));
        // An address of another kind is never opened.
        let mut g = starfield(None);
        g.uninstall_uri = Some("https://example.com/remove".into());
        g.show_uri = Some("cmd://evil".into());
        g.launch_uri = None;
        let i = installed(g);
        assert!(plan_uninstall(&i).is_err());
        assert!(!can_uninstall(&i));
        assert_eq!(show_uri(&i), None);
    }

    #[test]
    fn a_launcher_without_an_uninstall_address_uses_the_games_own_uninstaller() {
        let mut g = starfield(None);
        g.launcher = Launcher::Rockstar;
        g.key = "rdr2".into();
        g.uninstall_uri = None;
        g.install_dir = Some(PathBuf::from(r"D:\Games\Red Dead Redemption 2"));
        let e = UninstallEntry {
            key: "Rockstar Games Red Dead Redemption 2".into(),
            hive: "HKLM32",
            display_name: "Red Dead Redemption 2".into(),
            install_location: r"D:\Games\Red Dead Redemption 2\".into(),
            uninstall_string: r#""C:\Program Files\Rockstar Games\Launcher\uninstall.exe" -uninstall=rdr2"#.into(),
            ..Default::default()
        };
        let other = UninstallEntry { key: "Other".into(), display_name: "Other".into(), install_location: r"D:\Games\Other".into(), uninstall_string: r#""D:\Games\Other\unins.exe""#.into(), ..Default::default() };
        let i = installed_with(g.clone(), &[other.clone(), e.clone()]);
        assert!(i.uninstall.is_some(), "its entry's uninstaller is used");
        let p = plan_uninstall_with(&i, &[other.clone(), e]).unwrap();
        assert_eq!(p.app_id, i.id);
        assert!(matches!(&p.verify, Verify::Removed { app_id } if app_id == &i.id));
        assert!(matches!(&p.steps[..], [Step::Run { exe, .. }] if exe.to_string_lossy().ends_with("uninstall.exe")));
        // No entry in its folder: nothing to run.
        let lone = installed_with(g, &[other.clone()]);
        assert!(lone.uninstall.is_none());
        assert!(plan_uninstall_with(&lone, &[other]).is_err());
        // A launcher with its own address keeps it.
        let s = installed_with(starfield(None), &[]);
        assert!(matches!(&plan_uninstall_with(&s, &[]).unwrap().steps[..], [Step::OpenUri { .. }]));
    }

    #[test]
    fn steams_announcements_are_the_games_patch_notes() {
        let json = r#"{"appnews":{"appid":1912410,"newsitems":[
            {"gid":"501","title":"Patch notes","url":"https://steamstore-a.akamaihd.net/news/externalpost/x","contents":"[h2]Fixes[/h2][list][*]Fixed a crash when landing[*]Better frame pacing[/list]","feedname":"steam_community_announcements","feed_type":1,"date":1790000000},
            {"gid":"502","title":"Patch notes","contents":"[list][*]Ship builder fixes[/list]","feedname":"steam_community_announcements","feed_type":1,"date":1789000000},
            {"gid":"503","title":"A review elsewhere","contents":"Press text","feedname":"pcgamer","feed_type":0,"date":1791000000}
        ]}}"#;
        let list = steam_history(1912410, json);
        assert_eq!(list.iter().map(|v| v.version.as_str()).collect::<Vec<_>>(), ["Patch notes", "Patch notes (2)"]);
        assert!(list[0].latest && !list[1].latest);
        // A lone heading over everything says nothing and is dropped (see `notes::tidy_headings`).
        assert_eq!(list[0].notes, ["Fixed a crash when landing", "Better frame pacing"]);
        assert_eq!(list[0].notes_url.as_deref(), Some("https://store.steampowered.com/news/app/1912410/view/501"));
        assert_eq!(steam_history(1, "not json"), vec![]);
    }
}
