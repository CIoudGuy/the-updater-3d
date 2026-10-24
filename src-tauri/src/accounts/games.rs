//! The games the user owns on the platforms they signed in to, as this app shows and installs
//! them: Get apps entries ("Your games"), the newest Epic build of an installed game, and the
//! plans that install, update, repair and uninstall them. Epic and GOG games install here (see
//! `sources::games::sync`); Steam games, and Epic games another company's launcher manages,
//! install in that launcher.

use super::{library, signed_in, Owned, Platform};
use crate::model::{CatalogEntry, JobKind, Source};
use crate::sources::games::sync::{read_state, Mode};
use crate::sources::games::{epic::version_of_build, here, Game, Launcher, NotesFrom};
use crate::sources::{Installed, Plan, SourceData, Step, Verify, WAITING_FOR_YOU};
use anyhow::{anyhow, bail, Result};
use std::path::{Path, PathBuf};

/// "game:<platform>:<id>": the id the game has once installed, too.
pub fn game_id(p: Platform, id: &str) -> String {
    format!("game:{}:{}", p.key(), id).to_ascii_lowercase()
}

/// The owned game a Get apps id stands for (signed in only).
pub fn owned(id: &str) -> Option<Owned> {
    let mut parts = id.splitn(3, ':');
    if parts.next()? != "game" {
        return None;
    }
    let p = Platform::from_key(parts.next()?)?;
    let key = parts.next()?;
    if !signed_in(p) {
        return None;
    }
    library(p).games.into_iter().find(|g| g.id.eq_ignore_ascii_case(key))
}

/// What the card says of where it installs.
fn tagline(o: &Owned) -> String {
    match (o.platform, o.hand_off) {
        (Platform::Steam, _) => "Your Steam game. Installs in Steam".into(),
        (Platform::Epic, true) => "Your Epic Games game. Installs in the Epic Games launcher".into(),
        (Platform::Epic, false) => "Your Epic Games game. Installs and updates here".into(),
        (Platform::Gog, _) => "Your GOG game. Installs and updates here".into(),
    }
}

/// Every owned game of the platforms signed in to, as Get apps entries.
pub fn catalog_entries(installed: &[Installed]) -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    for p in Platform::ALL {
        if !signed_in(p) {
            continue;
        }
        for o in library(p).games {
            let id = game_id(p, &o.id);
            let (code, fg, bg) = crate::sources::games::mark(&o.name, p.launcher());
            out.push(CatalogEntry {
                installed: installed.iter().any(|i| i.id == id),
                id,
                source: Source::Games,
                name: o.name.clone(),
                code,
                fg,
                bg,
                icon_url: None,
                icon_miss: o.image.is_none(),
                version: o.latest.as_deref().map(version_of_build).unwrap_or_default(),
                size: None,
                tagline: Some(tagline(&o)),
                description: None,
                blocked: None,
                group: "Games".into(),
                prerelease: false,
                pick_version: false,
            });
        }
    }
    out
}

/// Where an owned game's patch notes come from.
pub fn notes_of(o: &Owned) -> NotesFrom {
    match o.platform {
        Platform::Steam => o.id.parse().map(NotesFrom::SteamNews).unwrap_or(NotesFrom::None),
        Platform::Gog => o.id.parse().map(NotesFrom::Gog).unwrap_or(NotesFrom::None),
        Platform::Epic => NotesFrom::None,
    }
}

/// The owned game as a launcher's game, for its patch notes on its Get apps page.
pub fn as_game(o: &Owned) -> Game {
    Game {
        launcher: o.platform.launcher(),
        key: o.id.clone(),
        name: o.name.clone(),
        version: String::new(),
        update: None,
        install_dir: None,
        size: None,
        updated_at: None,
        launch_uri: None,
        show_uri: None,
        uninstall_uri: None,
        page: None,
        notes: notes_of(o),
        icon: None,
        program: None,
    }
}

fn game(i: &Installed) -> Option<&Game> {
    match &i.data {
        SourceData::Game(g) => Some(g),
        _ => None,
    }
}

/// Is the build `latest` the one installed (`version`, or a game installed here, its build)?
fn same_build(installed: &str, here_build: Option<&str>, latest: &str) -> bool {
    let l = latest.trim();
    l.is_empty() || here_build.is_some_and(|b| b.trim() == l) || installed.trim() == l || version_of_build(installed) == version_of_build(l)
}

/// The newest Epic build of an installed Epic game the user owns, when it is not the one
/// installed. Epic's launcher keeps no word of updates on disk; the library has the newest build.
pub fn epic_update(i: &Installed) -> Option<String> {
    let g = game(i).filter(|g| g.launcher == Launcher::Epic)?;
    if !signed_in(Platform::Epic) {
        return None;
    }
    let latest = library(Platform::Epic).games.into_iter().find(|o| o.id.eq_ignore_ascii_case(&g.key))?.latest?;
    let here_build = g.install_dir.as_deref().and_then(read_state).map(|s| s.build);
    (!same_build(&g.version, here_build.as_deref(), &latest)).then(|| version_of_build(&latest))
}

fn plain(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The library entry of an Epic game found by its folder alone: its build names it by the
/// build's own app name ("FortniteReleaseBuilds"), the library by the store's ("Fortnite"). The
/// one entry of the same name, or whose app name starts the build's.
fn epic_match<'a>(g: &Game, lib: &'a [Owned]) -> Option<&'a Owned> {
    if lib.iter().any(|o| o.id.eq_ignore_ascii_case(&g.key)) {
        return None;
    }
    let key = g.key.to_ascii_lowercase();
    let name = plain(&g.name);
    let hits: Vec<&Owned> = lib.iter().filter(|o| (!name.is_empty() && plain(&o.name) == name) || (o.id.len() >= 4 && key.starts_with(&o.id.to_ascii_lowercase()))).collect();
    match hits[..] {
        [one] => Some(one),
        _ => None,
    }
}

/// Signed in to Epic Games: a game found by its folder alone gets the store's id (so it is the
/// library's game, with its updates) and starts through the launcher.
pub fn link_epic(games: &mut [Game]) {
    if !signed_in(Platform::Epic) {
        return;
    }
    let lib = library(Platform::Epic).games;
    for g in games.iter_mut().filter(|g| g.launcher == Launcher::Epic) {
        let Some(o) = epic_match(g, &lib) else { continue };
        g.key = o.id.clone();
        if g.launch_uri.is_none() && !o.namespace.is_empty() && !o.catalog_item.is_empty() {
            let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
            g.launch_uri = Some(format!("com.epicgames.launcher://apps/{}%3A{}%3A{}?action=launch&silent=true", enc(&o.namespace), enc(&o.catalog_item), enc(&o.id)));
        }
    }
}

/// At least two folders below a drive ("D:\Games\Hades"): a game folder that may be removed.
fn deep_enough(d: &Path) -> bool {
    d.is_absolute() && d.components().filter(|c| matches!(c, std::path::Component::Normal(_))).count() >= 2
}

/// A folder name for a game: its name without characters Windows does not allow.
pub fn folder_name(name: &str) -> String {
    let n: String = name.chars().map(|c| if c.is_control() || r#"<>:"/\|?*"#.contains(c) { ' ' } else { c }).collect();
    let n = n.split_whitespace().collect::<Vec<_>>().join(" ");
    let n = n.trim_end_matches(['.', ' ']).to_string();
    if n.is_empty() {
        "Game".into()
    } else {
        n
    }
}

/// The games folder: the one chosen in Settings, else "Games" on the Windows drive.
pub fn games_root(chosen: &str) -> PathBuf {
    let c = chosen.trim();
    if !c.is_empty() {
        return PathBuf::from(c);
    }
    let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into());
    PathBuf::from(format!("{drive}\\Games"))
}

/// A new folder for the game in `root`: its name, or its name with a number when a folder of
/// that name already holds something else.
pub fn new_folder(root: &Path, name: &str) -> PathBuf {
    let base = folder_name(name);
    let free = |p: &Path| !p.exists() || std::fs::read_dir(p).map(|mut d| d.next().is_none()).unwrap_or(false);
    let first = root.join(&base);
    if free(&first) {
        return first;
    }
    (2..).map(|n| root.join(format!("{base} ({n})"))).find(|p| free(p)).unwrap_or(first)
}

fn plan(app_id: String, kind: JobKind, name: &str, to_version: String, steps: Vec<Step>, block: Option<PathBuf>, verify: Verify) -> Plan {
    Plan { app_id, kind, name: name.to_string(), to_version, downloads: vec![], steps, block_dirs: block.into_iter().collect(), block_names: vec![], verify, fallback: None, cleanup: vec![] }
}

/// Installing an owned game: into a new folder of `root` here (Epic, GOG), or in its launcher.
pub fn plan_install(o: &Owned, root: &Path) -> Result<Plan> {
    let id = game_id(o.platform, &o.id);
    let version = o.latest.as_deref().map(version_of_build).unwrap_or_default();
    if o.platform == Platform::Steam {
        let appid: u32 = o.id.parse().map_err(|_| anyhow!("{} has no Steam app id", o.name))?;
        let step = Step::OpenUri { uri: format!("steam://install/{appid}"), label: format!("{WAITING_FOR_YOU}: choose where Steam installs {}", o.name) };
        return Ok(plan(id, JobKind::Install, &o.name, version, vec![step], None, Verify::None));
    }
    if o.hand_off {
        let step = Step::OpenUri { uri: "com.epicgames.launcher://library".into(), label: format!("{WAITING_FOR_YOU}: install {} from your library in Epic Games", o.name) };
        return Ok(plan(id, JobKind::Install, &o.name, version, vec![step], None, Verify::None));
    }
    // An install of it that failed halfway goes on where it stopped.
    let dest = here::unfinished(o.platform.key(), &o.id).unwrap_or_else(|| new_folder(root, &o.name));
    let step = Step::GameSync { platform: o.platform.key().into(), game: o.id.clone(), name: o.name.clone(), dest, mode: Mode::Install, label: format!("Installing {}", o.name) };
    Ok(plan(id, JobKind::Install, &o.name, version, vec![step], None, Verify::None))
}

/// Updating or repairing a game installed here.
pub fn plan_sync(i: &Installed, mode: Mode) -> Result<Plan> {
    let g = game(i).ok_or_else(|| anyhow!("{} is not a game", i.name))?;
    let dir = g.install_dir.clone().filter(|_| here::is_here(g)).ok_or_else(|| anyhow!("{} updates {}: open it there.", i.name, crate::sources::games::through(g.launcher)))?;
    let p = Platform::of_launcher(g.launcher).filter(|p| p.installs_here()).ok_or_else(|| anyhow!("{} is not installed from Epic Games or GOG", i.name))?;
    if !signed_in(p) {
        bail!("Sign in to {} in Settings to {} {}", p.name(), if mode == Mode::Repair { "repair" } else { "update" }, i.name);
    }
    let (kind, label) = match mode {
        Mode::Repair => (JobKind::Repair, format!("Checking and repairing {}", i.name)),
        _ => (JobKind::Update, format!("Updating {}", i.name)),
    };
    let to = if mode == Mode::Repair { i.version.clone() } else { epic_update(i).unwrap_or_default() };
    let step = Step::GameSync { platform: p.key().into(), game: g.key.clone(), name: i.name.clone(), dest: dir.clone(), mode, label };
    Ok(plan(i.id.clone(), kind, &i.name, to, vec![step], Some(dir), Verify::None))
}

/// What starts a game installed here: its program with the arguments its platform names, and an
/// Epic game signed in (a one-time code in its arguments, as Epic's launcher starts it: these are
/// never written to the log). None when it is not a game installed here or its program is gone.
pub async fn launch_of(g: &Game) -> Option<(PathBuf, Vec<String>)> {
    let dir = g.install_dir.as_deref().filter(|_| here::is_here(g))?;
    let state = read_state(dir)?;
    let exe = g.program.clone()?;
    if g.launcher != Launcher::Epic {
        return Some((exe, state.launch_args.clone()));
    }
    // Signed out or offline: started without, as the launcher's offline mode does.
    match (super::epic::exchange_code().await, super::vault::load::<super::epic::Session>(Platform::Epic.key())) {
        (Ok(code), Some(s)) => {
            let mut args = crate::sources::games::epic_dl::launch_args(&state, Some(&code), &s.account_id, &s.display_name);
            let ns = library(Platform::Epic).games.into_iter().find(|o| o.id.eq_ignore_ascii_case(&g.key)).map(|o| o.namespace).unwrap_or_default();
            if !ns.is_empty() {
                args.push(format!("-epicsandboxid={ns}"));
            }
            Some((exe, args))
        }
        (Err(e), _) => {
            log::warn!("{}: started without Epic sign-in ({})", g.name, super::problem_text(&e));
            Some((exe, state.launch_args.clone()))
        }
        _ => Some((exe, state.launch_args.clone())),
    }
}

/// Uninstalling a game installed here: its folder goes (the app wrote everything in it).
pub fn plan_uninstall_here(i: &Installed) -> Option<Plan> {
    let g = game(i).filter(|g| here::is_here(g))?;
    // A game's own folder, never a drive or a folder right under one ("D:\Games").
    let dir = g.install_dir.clone().filter(|d| deep_enough(d))?;
    let step = Step::RemoveDir { path: dir.clone(), elevated: false, label: format!("Removing {}", i.name) };
    Some(plan(i.id.clone(), JobKind::Uninstall, &i.name, String::new(), vec![step], Some(dir), Verify::Removed { app_id: i.id.clone() }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_games_folder_name_is_safe_and_new() {
        assert_eq!(folder_name("Half-Life: Alyx"), "Half-Life Alyx");
        assert_eq!(folder_name("What? / Why*"), "What Why");
        assert_eq!(folder_name("Trailing dots..."), "Trailing dots");
        assert_eq!(folder_name("<>"), "Game");
        let root = std::env::temp_dir().join(format!("u3d-games-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Some Game")).unwrap();
        // An empty folder of that name is used.
        assert_eq!(new_folder(&root, "Some Game"), root.join("Some Game"));
        std::fs::write(root.join("Some Game").join("x.txt"), b"x").unwrap();
        assert_eq!(new_folder(&root, "Some Game"), root.join("Some Game (2)"));
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(games_root(r"E:\My Games"), PathBuf::from(r"E:\My Games"));
        assert!(games_root("").ends_with("Games"));
    }

    #[test]
    fn an_epic_build_is_new_only_when_it_differs() {
        assert!(same_build("42.40", None, "++Fortnite+Release-42.40-CL-123-Windows"));
        assert!(!same_build("42.40", None, "++Fortnite+Release-42.50-CL-130-Windows"));
        assert!(same_build("1.0.3", Some("1.0.3+987"), "1.0.3+987"));
        assert!(!same_build("1.0.3", Some("1.0.3+987"), "1.0.4+990"));
        assert!(same_build("anything", None, ""));
    }

    #[test]
    fn an_epic_game_found_by_its_folder_is_the_librarys_game() {
        let o = |id: &str, name: &str| Owned { platform: Platform::Epic, id: id.into(), name: name.into(), image: None, namespace: "fn".into(), catalog_item: "4fe7".into(), latest: None, hand_off: false };
        let lib = vec![o("Fortnite", "Fortnite"), o("Sugar", "Hades"), o("Kinglet", "Kingdom Come")];
        let folder = |key: &str, name: &str| Game { key: key.into(), name: name.into(), ..as_game(&o("x", "x")) };
        // The build's own app name, or the folder's name, finds it.
        assert_eq!(epic_match(&folder("FortniteReleaseBuilds", "Fortnite"), &lib).map(|o| o.id.as_str()), Some("Fortnite"));
        assert_eq!(epic_match(&folder("SugarRelease", "Some Folder"), &lib).map(|o| o.id.as_str()), Some("Sugar"));
        assert_eq!(epic_match(&folder("Unknown", "HADES"), &lib).map(|o| o.id.as_str()), Some("Sugar"));
        // Already the library's id, nothing alike, or two alike: left as it is.
        assert!(epic_match(&folder("Fortnite", "Fortnite"), &lib).is_none());
        assert!(epic_match(&folder("Other", "Other game"), &lib).is_none());
        let two = vec![o("Fortnite", "Fortnite"), o("FortniteDemo", "Fortnite")];
        assert!(epic_match(&folder("FortniteReleaseBuilds", "Fortnite"), &two).is_none());
    }

    #[test]
    fn only_a_games_own_folder_is_removed() {
        assert!(deep_enough(Path::new(r"D:\Games\Hades")));
        assert!(!deep_enough(Path::new(r"D:\Games")));
        assert!(!deep_enough(Path::new(r"D:\")));
        assert!(!deep_enough(Path::new(r"Games\Hades")));
    }

    #[test]
    fn steam_and_launcher_managed_games_install_in_their_launcher() {
        let o = |p: Platform, id: &str, hand_off: bool| Owned { platform: p, id: id.into(), name: "A Game".into(), image: None, namespace: "ns".into(), catalog_item: "ci".into(), latest: Some("1.2".into()), hand_off };
        let root = Path::new(r"C:\Games");
        let s = plan_install(&o(Platform::Steam, "1174180", true), root).unwrap();
        assert!(matches!(&s.steps[..], [Step::OpenUri { uri, .. }] if uri == "steam://install/1174180"));
        assert_eq!(s.app_id, "game:steam:1174180");
        let e = plan_install(&o(Platform::Epic, "Ea_Game", true), root).unwrap();
        assert!(matches!(&e.steps[..], [Step::OpenUri { uri, .. }] if uri.starts_with("com.epicgames.launcher://")));
        let here = plan_install(&o(Platform::Gog, "1207658924", false), root).unwrap();
        assert!(matches!(&here.steps[..], [Step::GameSync { platform, dest, mode: Mode::Install, .. }] if platform == "gog" && dest.starts_with(root)));
        assert_eq!(here.app_id, "game:gog:1207658924");
        assert!(plan_install(&o(Platform::Steam, "not a number", true), root).is_err());
    }
}
