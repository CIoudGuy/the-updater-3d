//! Steam: the games and apps in every Steam library on this PC, read from Steam's own files
//! (`libraryfolders.vdf` lists the libraries, each `appmanifest_<id>.acf` one app). Steam marks a
//! waiting update there (a build it has not installed yet); patch notes are Steam's public news
//! for the app. Nothing that names the Steam account (the manifest's owner id) is read or kept.

use super::{iso, Game, Launcher, NotesFrom};
use std::path::{Path, PathBuf};

/// A value in Valve's KeyValues text format: a string, or a list of named values.
#[derive(Clone, Debug, PartialEq)]
pub enum Kv {
    Str(String),
    Obj(Vec<(String, Kv)>),
}

impl Kv {
    /// The value named `key` (Valve's names are not case sensitive).
    pub fn get(&self, key: &str) -> Option<&Kv> {
        match self {
            Kv::Obj(v) => v.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v),
            Kv::Str(_) => None,
        }
    }
    pub fn str(&self, key: &str) -> Option<&str> {
        match self.get(key)? {
            Kv::Str(s) => Some(s),
            Kv::Obj(_) => None,
        }
    }
    pub fn num(&self, key: &str) -> Option<u64> {
        self.str(key)?.trim().parse().ok()
    }
    pub fn entries(&self) -> &[(String, Kv)] {
        match self {
            Kv::Obj(v) => v,
            Kv::Str(_) => &[],
        }
    }
}

/// Valve's KeyValues text ("name" "value", "name" { ... }) as a tree. Unquoted words and
/// "//" comments are read too; a file cut short gives what was read.
pub fn parse_kv(text: &str) -> Kv {
    let mut toks = Vec::new();
    let mut it = text.chars().peekable();
    while let Some(&c) = it.peek() {
        match c {
            '{' | '}' => {
                toks.push(c.to_string());
                it.next();
            }
            '"' => {
                it.next();
                let mut s = String::new();
                while let Some(c) = it.next() {
                    match c {
                        '\\' => match it.next() {
                            Some('n') => s.push('\n'),
                            Some('t') => s.push('\t'),
                            Some(o) => s.push(o),
                            None => break,
                        },
                        '"' => break,
                        o => s.push(o),
                    }
                }
                toks.push(format!("\u{1}{s}"));
            }
            '/' => {
                it.next();
                if it.peek() == Some(&'/') {
                    for c in it.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                } else {
                    // A word that starts with "/".
                    let mut s = String::from("/");
                    while let Some(&c) = it.peek() {
                        if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                            break;
                        }
                        s.push(c);
                        it.next();
                    }
                    toks.push(format!("\u{1}{s}"));
                }
            }
            c if c.is_whitespace() => {
                it.next();
            }
            _ => {
                let mut s = String::new();
                while let Some(&c) = it.peek() {
                    if c.is_whitespace() || c == '{' || c == '}' || c == '"' {
                        break;
                    }
                    s.push(c);
                    it.next();
                }
                toks.push(format!("\u{1}{s}"));
            }
        }
    }
    fn obj(toks: &[String], i: &mut usize) -> Vec<(String, Kv)> {
        let mut out = Vec::new();
        while *i < toks.len() {
            let t = &toks[*i];
            if t == "}" {
                *i += 1;
                break;
            }
            let Some(key) = t.strip_prefix('\u{1}') else {
                *i += 1;
                continue;
            };
            *i += 1;
            match toks.get(*i).map(String::as_str) {
                Some("{") => {
                    *i += 1;
                    out.push((key.to_string(), Kv::Obj(obj(toks, i))));
                }
                Some(v) if v.starts_with('\u{1}') => {
                    out.push((key.to_string(), Kv::Str(v[1..].to_string())));
                    *i += 1;
                }
                _ => {}
            }
        }
        out
    }
    let mut i = 0;
    Kv::Obj(obj(&toks, &mut i))
}

/// The library folders `libraryfolders.vdf` lists (each holds a "steamapps" folder).
pub fn library_paths(vdf: &str) -> Vec<PathBuf> {
    let kv = parse_kv(vdf);
    let Some(root) = kv.get("libraryfolders") else { return vec![] };
    let mut out: Vec<PathBuf> = Vec::new();
    for (_, v) in root.entries() {
        // Newer files: "0" { "path" "C:\\..." }; older ones: "1" "D:\\SteamLibrary".
        let p = match v {
            Kv::Obj(_) => v.str("path").map(str::to_string),
            Kv::Str(s) if s.contains(':') => Some(s.clone()),
            Kv::Str(_) => None,
        };
        if let Some(p) = p.filter(|p| !p.trim().is_empty()) {
            let pb = PathBuf::from(p.replace('/', "\\"));
            if !out.iter().any(|o| o.to_string_lossy().eq_ignore_ascii_case(&pb.to_string_lossy())) {
                out.push(pb);
            }
        }
    }
    out
}

/// What one app manifest says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteamApp {
    pub appid: u32,
    pub name: String,
    pub state_flags: u64,
    pub installdir: String,
    pub size_on_disk: u64,
    pub build: u64,
    pub target_build: u64,
    pub bytes_to_download: u64,
    pub bytes_downloaded: u64,
    pub last_updated: i64,
}

/// An app manifest ("AppState" { ... }).
pub fn parse_manifest(text: &str) -> Option<SteamApp> {
    let kv = parse_kv(text);
    let a = kv.get("AppState")?;
    Some(SteamApp {
        appid: a.num("appid")?.try_into().ok()?,
        name: a.str("name").unwrap_or("").trim().to_string(),
        state_flags: a.num("StateFlags").unwrap_or(0),
        installdir: a.str("installdir").unwrap_or("").to_string(),
        size_on_disk: a.num("SizeOnDisk").unwrap_or(0),
        build: a.num("buildid").unwrap_or(0),
        target_build: a.num("TargetBuildID").unwrap_or(0),
        bytes_to_download: a.num("BytesToDownload").unwrap_or(0),
        bytes_downloaded: a.num("BytesDownloaded").unwrap_or(0),
        last_updated: a.num("LastUpdated").unwrap_or(0) as i64,
    })
}

/// Steam's own parts, not games: the shared redistributables every game installs.
const NOT_GAMES: &[u32] = &[228980];

/// StateFlags: the app needs an update.
const UPDATE_REQUIRED: u64 = 2;
/// StateFlags: fully installed.
const FULLY_INSTALLED: u64 = 4;

impl SteamApp {
    /// Is it on the PC (installed, or installed with an update waiting)?
    pub fn installed(&self) -> bool {
        self.state_flags & FULLY_INSTALLED != 0 || (self.build != 0 && self.state_flags & UPDATE_REQUIRED != 0)
    }

    /// The build Steam has found and not installed yet: Some(build), Some(0) when it only marked
    /// that one is needed, None when the app is current.
    pub fn waiting(&self) -> Option<u64> {
        if self.target_build != 0 && self.build != 0 && self.target_build != self.build {
            return Some(self.target_build);
        }
        (self.state_flags & UPDATE_REQUIRED != 0 || self.bytes_to_download > self.bytes_downloaded).then_some(0)
    }
}

/// The game a manifest in `library` stands for. `steam` is Steam's own folder (its picture cache).
pub fn game_of(app: &SteamApp, library: &Path, steam: Option<&Path>) -> Option<Game> {
    if NOT_GAMES.contains(&app.appid) || app.name.is_empty() || !app.installed() {
        return None;
    }
    let id = app.appid;
    let dir = (!app.installdir.is_empty()).then(|| library.join("steamapps").join("common").join(&app.installdir));
    Some(Game {
        launcher: Launcher::Steam,
        key: id.to_string(),
        name: app.name.clone(),
        version: if app.build != 0 { app.build.to_string() } else { String::new() },
        update: app.waiting().map(|b| if b == 0 { String::new() } else { b.to_string() }),
        install_dir: dir,
        size: Some(app.size_on_disk).filter(|s| *s > 0),
        updated_at: iso(app.last_updated),
        launch_uri: Some(format!("steam://rungameid/{id}")),
        show_uri: Some(format!("steam://nav/games/details/{id}")),
        uninstall_uri: Some(format!("steam://uninstall/{id}")),
        page: Some(format!("https://store.steampowered.com/app/{id}/")),
        notes: NotesFrom::SteamNews(id),
        icon: steam.and_then(|s| crate::sources::proglogo::steam_cached_icon(s, id)),
    })
}

/// Steam's folder on this PC, from where Steam itself says it is.
fn steam_dir() -> Option<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;
    let cu = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Valve\\Steam").ok().and_then(|k| k.get_value::<String, _>("SteamPath").ok());
    let lm = || RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("SOFTWARE\\Valve\\Steam", KEY_READ | KEY_WOW64_32KEY).ok().and_then(|k| k.get_value::<String, _>("InstallPath").ok());
    cu.or_else(lm).map(|p| PathBuf::from(p.replace('/', "\\"))).filter(|p| p.join("steamapps").is_dir())
}

/// Every game and app in this PC's Steam libraries.
pub fn scan() -> Vec<Game> {
    let Some(steam) = steam_dir() else { return vec![] };
    let mut libs = std::fs::read_to_string(steam.join("steamapps").join("libraryfolders.vdf")).map(|t| library_paths(&t)).unwrap_or_default();
    if !libs.iter().any(|l| l.to_string_lossy().eq_ignore_ascii_case(&steam.to_string_lossy())) {
        libs.insert(0, steam.clone());
    }
    let mut out = Vec::new();
    for lib in libs {
        let Ok(rd) = std::fs::read_dir(lib.join("steamapps")) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_ascii_lowercase();
            if !(name.starts_with("appmanifest_") && name.ends_with(".acf")) {
                continue;
            }
            let Some(app) = std::fs::read_to_string(e.path()).ok().and_then(|t| parse_manifest(&t)) else { continue };
            if let Some(g) = game_of(&app, &lib, Some(&steam)) {
                if !out.iter().any(|o: &Game| o.key == g.key) {
                    out.push(g);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBS: &str = r#""libraryfolders"
{
	"0"
	{
		"path"		"C:\\Program Files (x86)\\Steam"
		"label"		""
		"apps"
		{
			"228980"		"329931729"
		}
	}
	"1"
	{
		"path"		"D:\\SteamLibrary"
		"apps"
		{
			"1912410"		"140000000000"
		}
	}
}"#;

    fn manifest(flags: u64, build: u64, target: u64) -> String {
        format!(
            r#""AppState"
{{
	"appid"		"1912410"
	"Universe"		"1"
	"name"		"Starfield"
	"StateFlags"		"{flags}"
	"installdir"		"Starfield"
	"LastUpdated"		"1783497936"
	"SizeOnDisk"		"140000000000"
	"buildid"		"{build}"
	"LastOwner"		"76561190000000000"
	"BytesToDownload"		"0"
	"BytesDownloaded"		"0"
	"TargetBuildID"		"{target}"
	"InstalledDepots"
	{{
		"1716741"
		{{
			"manifest"		"5753583882400741046"
			"size"		"25674515"
		}}
	}}
}}"#
        )
    }

    #[test]
    fn the_libraries_are_read_from_steams_list() {
        assert_eq!(library_paths(LIBS), [PathBuf::from(r"C:\Program Files (x86)\Steam"), PathBuf::from(r"D:\SteamLibrary")]);
        // The older form: the number names the path itself.
        assert_eq!(library_paths("\"LibraryFolders\" { \"TimeNextStatsReport\" \"1\" \"1\" \"E:\\\\Games\" }"), [PathBuf::from(r"E:\Games")]);
    }

    #[test]
    fn a_current_game_is_listed_with_its_build_and_steams_addresses() {
        let app = parse_manifest(&manifest(4, 24098430, 24098430)).unwrap();
        assert_eq!(app.appid, 1912410);
        let g = game_of(&app, Path::new(r"D:\SteamLibrary"), None).unwrap();
        assert_eq!(g.id(), "game:steam:1912410");
        assert_eq!(g.name, "Starfield");
        assert_eq!(g.version, "24098430");
        assert_eq!(g.update, None);
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"D:\SteamLibrary\steamapps\common\Starfield")));
        assert_eq!(g.size, Some(140000000000));
        assert_eq!(g.launch_uri.as_deref(), Some("steam://rungameid/1912410"));
        assert_eq!(g.show_uri.as_deref(), Some("steam://nav/games/details/1912410"));
        assert_eq!(g.notes, NotesFrom::SteamNews(1912410));
        assert!(g.updated_at.as_deref().is_some_and(|d| d.starts_with("2026-")));
    }

    #[test]
    fn a_waiting_update_is_the_build_steam_found() {
        let app = parse_manifest(&manifest(6, 24098430, 24555555)).unwrap();
        assert_eq!(game_of(&app, Path::new("D:\\L"), None).unwrap().update.as_deref(), Some("24555555"));
        // Marked as needing an update, build not known yet.
        let app = parse_manifest(&manifest(6, 24098430, 0)).unwrap();
        assert_eq!(game_of(&app, Path::new("D:\\L"), None).unwrap().update.as_deref(), Some(""));
    }

    #[test]
    fn steams_own_parts_and_unfinished_installs_are_not_games() {
        let redist = parse_manifest(&manifest(4, 1, 1).replace("1912410", "228980")).unwrap();
        assert!(game_of(&redist, Path::new("C:\\S"), None).is_none());
        // Still downloading for the first time: no build installed yet.
        let fresh = parse_manifest(&manifest(1026, 0, 24098430)).unwrap();
        assert!(game_of(&fresh, Path::new("C:\\S"), None).is_none());
    }

    #[test]
    fn the_owner_id_is_not_kept() {
        let app = parse_manifest(&manifest(4, 1, 1)).unwrap();
        let g = game_of(&app, Path::new("C:\\S"), None).unwrap();
        assert!(!format!("{g:?}").contains("76561190000000000"));
    }
}
