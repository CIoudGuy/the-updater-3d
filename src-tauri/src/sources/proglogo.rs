//! Icons for installed programs and games whose own program file gives none (a game that is
//! gone but still listed, a repack with no icon at all). In order: the icon the entry names, as
//! Windows' own list shows it; for a Steam game Steam's own icon in its library cache, then its
//! store picture; for a game, the Steam store's picture found by its exact name; the icon of the
//! publisher's own site. This file only decides what to look at; reading and the network are in
//! `core`. Only a public app id or app name goes into a request, never anything about this PC.

use crate::winutil::UninstallEntry;
use std::path::{Path, PathBuf};

/// The file and icon number an entry's "DisplayIcon" names: `"C:\a\app.exe",0`, `C:\a\app.ico`,
/// `C:\a\app.dll,-101`. Only programs, libraries and icon files; the file is not checked here.
pub fn display_icon(spec: &str) -> Option<(PathBuf, i32)> {
    let s = spec.trim();
    if s.is_empty() {
        return None;
    }
    // "path",index or path,index: the number after the last comma
    let (file, index) = match s.rsplit_once(',') {
        Some((f, i)) if i.trim().parse::<i32>().is_ok() => (f, i.trim().parse::<i32>().unwrap_or(0)),
        _ => (s, 0),
    };
    let file = file.trim().trim_matches('"').trim();
    let low = file.to_ascii_lowercase();
    if !(low.ends_with(".exe") || low.ends_with(".dll") || low.ends_with(".ico")) || low.contains("\\temp\\") || low.contains('%') {
        return None;
    }
    Some((PathBuf::from(file), index))
}

/// The app id of a Steam game's uninstall key: "Steam App 1912410".
pub fn steam_app_id(key: &str) -> Option<u32> {
    key.trim().strip_prefix("Steam App ")?.trim().parse().ok().filter(|n| *n > 0)
}

/// Steam's folder, from the program its uninstall line runs (`"C:\...\Steam\steam.exe"
/// steam://uninstall/1912410`), else from where its icon files are.
pub fn steam_dir(e: &UninstallEntry) -> Option<PathBuf> {
    let exe = crate::winutil::split_program_line(&e.uninstall_string).into_iter().next().unwrap_or_default();
    let from_exe = Path::new(&exe).file_name().is_some_and(|n| n.eq_ignore_ascii_case("steam.exe")).then(|| Path::new(&exe).parent().map(Path::to_path_buf)).flatten();
    let from_icon = || {
        let (p, _) = display_icon(&e.display_icon)?;
        // "...\Steam\steam\games\<hash>.ico"
        let games = p.parent()?;
        let steam = games.parent()?;
        (games.file_name()?.eq_ignore_ascii_case("games") && steam.file_name()?.eq_ignore_ascii_case("steam")).then(|| steam.parent().map(Path::to_path_buf)).flatten()
    };
    from_exe.or_else(from_icon)
}

/// The icon Steam keeps for a game in its library cache: a small picture in the game's folder
/// (named by its hash), or `<id>_icon.jpg` as older Steam versions kept it.
pub fn steam_cached_icon(steam: &Path, app: u32) -> Option<PathBuf> {
    let cache = steam.join("appcache").join("librarycache");
    let mut loose: Vec<PathBuf> = std::fs::read_dir(cache.join(app.to_string()))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("jpg") || x.eq_ignore_ascii_case("png")))
        .collect();
    loose.sort();
    loose.into_iter().next().or_else(|| Some(cache.join(format!("{app}_icon.jpg"))).filter(|p| p.is_file()))
}

/// Where Steam's store keeps the game's tall picture, which is cut to a square for an icon.
pub fn steam_art_urls(app: u32) -> Vec<String> {
    vec![format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{app}/library_600x900.jpg"), format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{app}/capsule_231x87.jpg")]
}

/// A name for comparing: letters and digits only, lower case, without the trademark marks.
pub fn plain(name: &str) -> String {
    name.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

/// Edition words Steam adds to a game's name that an installer leaves out.
const EDITIONS: &[&str] = &["completeedition", "deluxeedition", "definitiveedition", "gameoftheyearedition", "goty", "goldedition", "ultimateedition", "standardedition", "specialedition", "digitaldeluxeedition", "remastered", "directorscut"];

/// Is this Steam title the game the entry is? The same name, or the same name and an edition.
pub fn same_game(entry_name: &str, steam_title: &str) -> bool {
    let (a, b) = (plain(entry_name), plain(steam_title));
    if a.len() < 3 || b.len() < 3 {
        return false;
    }
    a == b || b.strip_prefix(a.as_str()).is_some_and(|rest| EDITIONS.contains(&rest))
}

/// The address of Steam's store search for a game name (a public page of Steam's store).
pub fn steam_search_url(name: &str) -> String {
    let enc: String = name
        .trim()
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("https://store.steampowered.com/api/storesearch/?term={enc}&l=english&cc=US")
}

/// The Steam app id of the game the entry is, from Steam's search answer: an app (not music or an
/// add-on) whose title is the game's name, the plainest title first.
pub fn steam_pick(json: &str, entry_name: &str) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut best: Option<(usize, u32)> = None;
    for i in v["items"].as_array()? {
        let (Some(id), Some(title)) = (i["id"].as_u64(), i["name"].as_str()) else { continue };
        if i["type"].as_str().is_some_and(|t| t != "app") || !same_game(entry_name, title) {
            continue;
        }
        let rank = plain(title).len();
        if best.is_none_or(|(r, _)| rank < r) {
            best = Some((rank, id as u32));
        }
    }
    best.map(|(_, id)| id)
}

/// Words after a game's name that make a title something else than the game itself.
const NOT_THE_GAME: &[&str] = &["soundtrack", "dlc", "demo", "benchmark", "tool", "pack", "bundle", "upgrade", "pass", "artbook", "mod", "server", "editor", "sdk"];

/// The Steam app of a game that Steam's store calls by a longer name ("The Last of Us" is "The
/// Last of Us Part I"): the first app in the answer whose title starts with the game's name and
/// has only a short ending that is no add-on. Used only when no title is the game's own name.
pub fn steam_pick_longer(json: &str, entry_name: &str) -> Option<u32> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let name = plain(entry_name);
    if name.len() < 5 {
        return None;
    }
    v["items"].as_array()?.iter().find_map(|i| {
        let (id, title) = (i["id"].as_u64()?, plain(i["name"].as_str()?));
        let rest = title.strip_prefix(name.as_str())?;
        let is_app = i["type"].as_str().is_none_or(|t| t == "app");
        (is_app && !rest.is_empty() && rest.len() <= 10 && !NOT_THE_GAME.iter().any(|w| rest.contains(w))).then_some(id as u32)
    })
}

/// Does the entry look like a game? Only games are looked up by name in Steam's store, so the
/// names of other programs never leave this PC. Told by the folders its entry names.
pub fn looks_like_game(e: &UninstallEntry) -> bool {
    let at = [e.install_location.as_str(), e.uninstall_string.as_str(), e.display_icon.as_str()].join("|").to_lowercase().replace('/', "\\");
    ["\\games\\", "\\steamapps\\", "\\steamlibrary\\", "\\epic games\\", "\\gog games\\", "\\ubisoft game launcher\\games\\", "\\riot games\\", "\\battle.net\\", "\\ea games\\", "\\origin games\\", "\\xboxgames\\"].iter().any(|k| at.contains(k))
}

/// The label of a site's name that says whose it is: "ubisoft" for www.ubisoft.com, "dodi-repacks"
/// for www.dodi-repacks.site, "bbc" for www.bbc.co.uk.
fn site_label(host: &str) -> String {
    let parts: Vec<&str> = host.trim_end_matches('.').split('.').collect();
    let n = parts.len();
    let pick = match n {
        0 | 1 => 0,
        // co.uk, com.au, org.uk
        _ if n >= 3 && parts[n - 1].len() == 2 && matches!(parts[n - 2], "co" | "com" | "org" | "net" | "gov" | "ac") => n - 3,
        _ => n - 2,
    };
    parts.get(pick).copied().unwrap_or("").to_lowercase()
}

/// Is this site the publisher's or the program's own? Its name appears in theirs. A site that
/// belongs to somebody else (a support page of Microsoft, a store) is no source for the icon.
pub fn site_fits(host: &str, publisher: &str, name: &str) -> bool {
    let label = plain(&site_label(host));
    if label.len() < 3 || matches!(label.as_str(), "microsoft" | "windows" | "google" | "apple" | "github" | "steampowered" | "wordpress" | "blogspot" | "sourceforge" | "facebook" | "twitter" | "youtube") {
        return false;
    }
    plain(publisher).contains(&label) || plain(name).contains(&label)
}

/// The account a github.com address is of: "Inrixia" for https://github.com/Inrixia/TidaLuna.
fn github_owner(url: &url::Url) -> Option<String> {
    let host = url.host_str()?.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    url.path_segments()?.next().filter(|o| !o.is_empty()).map(String::from)
}

/// Is the account behind a GitHub address the program's own: named like its publisher or itself?
fn owner_fits(owner: &str, publisher: &str, name: &str) -> bool {
    let (o, p, n) = (plain(owner), plain(publisher), plain(name));
    o.len() >= 3 && (p.contains(&o) || n.contains(&o) || (p.len() >= 4 && o.contains(&p)))
}

/// The publisher's own pages an entry names, for the icon finder: the sites that fit. A GitHub
/// address fits when its account is the publisher's (the finder reads that account's picture).
pub fn site_pages(e: &UninstallEntry) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in [&e.url_info_about, &e.help_link, &e.url_update_info] {
        let u = raw.trim().replace("&amp;", "&");
        let Ok(url) = url::Url::parse(&u) else { continue };
        let Some(host) = url.host_str() else { continue };
        let fits = match github_owner(&url) {
            Some(owner) => owner_fits(&owner, &e.publisher, &e.display_name),
            None => site_fits(host, &e.publisher, &e.display_name),
        };
        if !matches!(url.scheme(), "http" | "https") || !fits || out.iter().any(|o| o == &u) {
            continue;
        }
        out.push(u);
    }
    out
}

/// What an icon from these pages is kept under, so the programs that share a site share its icon:
/// the site, or for GitHub the account.
pub fn site_key(pages: &[String]) -> Option<String> {
    let url = url::Url::parse(pages.first()?).ok()?;
    match github_owner(&url) {
        Some(owner) => Some(format!("github.com-{}", owner.to_ascii_lowercase())),
        None => url.host_str().map(|h| h.to_ascii_lowercase()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(f: impl FnOnce(&mut UninstallEntry)) -> UninstallEntry {
        let mut e = UninstallEntry::default();
        f(&mut e);
        e
    }

    #[test]
    fn the_icon_an_entry_names_is_a_file_and_a_number() {
        assert_eq!(display_icon(r#""C:\Games\Far Cry 6\FarCry6.exe",0"#), Some((PathBuf::from(r"C:\Games\Far Cry 6\FarCry6.exe"), 0)));
        assert_eq!(display_icon(r"C:\Program Files\App\app.exe,-101"), Some((PathBuf::from(r"C:\Program Files\App\app.exe"), -101)));
        assert_eq!(display_icon(r"C:\Program Files (x86)\Steam\steam\games\5d74a49b.ico"), Some((PathBuf::from(r"C:\Program Files (x86)\Steam\steam\games\5d74a49b.ico"), 0)));
        // a comma inside the folder name is not an icon number
        assert_eq!(display_icon(r"C:\Fun, Games\app.exe"), Some((PathBuf::from(r"C:\Fun, Games\app.exe"), 0)));
        assert_eq!(display_icon(""), None);
        assert_eq!(display_icon(r"C:\a\readme.txt"), None);
        assert_eq!(display_icon(r"%SystemRoot%\system32\shell32.dll,3"), None);
        assert_eq!(display_icon(r"C:\Users\x\AppData\Local\Temp\setup.exe"), None);
    }

    #[test]
    fn a_steam_game_is_told_by_its_key() {
        assert_eq!(steam_app_id("Steam App 1912410"), Some(1912410));
        assert_eq!(steam_app_id("Steam App 0"), None);
        assert_eq!(steam_app_id("Steam App x"), None);
        assert_eq!(steam_app_id("Black Myth: Wukong_is1"), None);
    }

    #[test]
    fn steams_folder_comes_from_its_uninstall_line_or_its_icon_files() {
        let a = entry(|e| e.uninstall_string = r#""C:\Program Files (x86)\Steam\steam.exe" steam://uninstall/1912410"#.into());
        assert_eq!(steam_dir(&a), Some(PathBuf::from(r"C:\Program Files (x86)\Steam")));
        let b = entry(|e| e.display_icon = r"D:\Steam\steam\games\537a773b.ico".into());
        assert_eq!(steam_dir(&b), Some(PathBuf::from(r"D:\Steam")));
        assert_eq!(steam_dir(&entry(|e| e.uninstall_string = r#""D:\Games\Far Cry 6\unins000.exe""#.into())), None);
    }

    #[test]
    fn steams_library_cache_gives_the_small_icon_of_a_game() {
        let d = std::env::temp_dir().join(format!("tu3d-steam-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let app = d.join("appcache").join("librarycache").join("1912410");
        std::fs::create_dir_all(app.join("47d3b04112a8")).unwrap();
        std::fs::write(app.join("47d3b04112a8").join("library_capsule.jpg"), b"art").unwrap();
        assert_eq!(steam_cached_icon(&d, 1912410), None, "a picture in a sub folder is not the icon");
        std::fs::write(app.join("da6f7ad684d2.jpg"), b"icon").unwrap();
        assert_eq!(steam_cached_icon(&d, 1912410), Some(app.join("da6f7ad684d2.jpg")));
        // an older Steam kept it next to the folders
        std::fs::write(d.join("appcache").join("librarycache").join("440_icon.jpg"), b"icon").unwrap();
        assert_eq!(steam_cached_icon(&d, 440), Some(d.join("appcache").join("librarycache").join("440_icon.jpg")));
        assert_eq!(steam_cached_icon(&d, 999), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_steam_title_is_the_game_when_the_name_or_an_edition_of_it() {
        assert!(same_game("Far Cry 6", "Far Cry® 6"));
        assert!(same_game("Horizon Forbidden West", "Horizon Forbidden West™ Complete Edition"));
        assert!(same_game("Black Myth: Wukong", "Black Myth Wukong"));
        assert!(!same_game("Black Myth: Wukong", "Black Myth: Wukong Soundtrack Selection"));
        assert!(!same_game("Palworld", "Palworld: Palfarm"));
        assert!(!same_game("Far Cry", "Far Cry 6"));
        assert!(!same_game("X", "X"));
    }

    #[test]
    fn the_games_store_answer_names_the_app_of_the_game() {
        let json = r#"{"total":4,"items":[
            {"type":"app","name":"Palworld: Palfarm","id":4031890},
            {"type":"app","name":"Palworld - Soundtrack","id":2771110},
            {"type":"app","name":"Palworld","id":1623730},
            {"type":"app","name":"Palworld Deluxe Edition","id":1}]}"#;
        assert_eq!(steam_pick(json, "Palworld"), Some(1623730));
        assert_eq!(steam_pick(json, "Pal"), None);
        let two = r#"{"items":[{"type":"app","name":"Horizon Forbidden West™ Complete Edition","id":2420110},{"type":"sub","name":"Horizon Forbidden West","id":9}]}"#;
        assert_eq!(steam_pick(two, "Horizon Forbidden West"), Some(2420110));
        assert_eq!(steam_pick("not json", "x"), None);
        assert_eq!(steam_pick(r#"{"items":[]}"#, "Palworld"), None);
    }

    #[test]
    fn only_a_game_is_looked_up_by_name() {
        let g = |loc: &str, un: &str| entry(|e| {
            e.install_location = loc.into();
            e.uninstall_string = un.into();
        });
        assert!(looks_like_game(&g(r"D:\Games\Far Cry 6\", r#""D:\Games\Far Cry 6\unins000.exe""#)));
        assert!(looks_like_game(&g("", r#""C:\Program Files (x86)\Ubisoft\Ubisoft Game Launcher\games\Far Cry 6\unins000.exe""#)));
        assert!(looks_like_game(&g(r"D:/SteamLibrary/steamapps/common/Palworld", "")));
        assert!(!looks_like_game(&g(r"C:\Program Files\7-Zip\", r#""C:\Program Files\7-Zip\Uninstall.exe""#)));
        assert!(!looks_like_game(&g("", "")));
    }

    #[test]
    fn a_site_is_a_source_only_when_it_is_the_publishers_own() {
        assert!(site_fits("www.ubisoft.com", "Ubisoft Entertainment", "Far Cry 6"));
        assert!(site_fits("www.dodi-repacks.site", "DODI-Repacks", "Horizon Forbidden West"));
        assert!(site_fits("rhythia.com", "Capo Games", "Rhythia"));
        assert!(site_fits("www.bbc.co.uk", "BBC", "iPlayer"));
        assert!(!site_fits("support.microsoft.com", "Some Maker", "Some App"));
        assert!(!site_fits("help.steampowered.com", "Valve", "Steam"));
        assert!(!site_fits("www.example.com", "Other Corp", "Other App"));
        assert!(!site_fits("ea.com", "Electronic Arts", "FC 26"));
        assert_eq!(site_label("www.bbc.co.uk"), "bbc");
        assert_eq!(site_label("getsharex.com"), "getsharex");
    }

    #[test]
    fn the_pages_an_entry_names_are_cleaned_and_only_its_own_kept() {
        let e = entry(|e| {
            e.publisher = "DODI-Repacks".into();
            e.display_name = "Horizon Forbidden West".into();
            e.url_info_about = "http://www.dodi-repacks.site/".into();
            e.help_link = "http://www.dodi-repacks.site/".into();
            e.url_update_info = "https://support.microsoft.com/help?a=1&amp;b=2".into();
        });
        assert_eq!(site_pages(&e), vec!["http://www.dodi-repacks.site/".to_string()]);
        assert_eq!(site_key(&site_pages(&e)).as_deref(), Some("www.dodi-repacks.site"));
        assert!(site_pages(&UninstallEntry::default()).is_empty());
        let ftp = entry(|e| {
            e.publisher = "Acme".into();
            e.url_info_about = "ftp://acme.com/".into();
        });
        assert!(site_pages(&ftp).is_empty());
    }

    #[test]
    fn a_github_account_is_the_publishers_when_it_is_named_like_it() {
        let e = entry(|e| {
            e.publisher = "Inrixia".into();
            e.display_name = "TidaLunar".into();
            e.url_info_about = "https://github.com/Inrixia/TidaLuna".into();
        });
        assert_eq!(site_pages(&e), vec!["https://github.com/Inrixia/TidaLuna".to_string()]);
        assert_eq!(site_key(&site_pages(&e)).as_deref(), Some("github.com-inrixia"));
        // somebody else's project is no source for the icon of a program they did not make
        let other = entry(|e| {
            e.publisher = "Some Maker".into();
            e.display_name = "Some App".into();
            e.url_info_about = "https://github.com/unrelated/project".into();
        });
        assert!(site_pages(&other).is_empty());
        // the program's own name counts too
        let named = entry(|e| {
            e.publisher = String::new();
            e.display_name = "ShareX".into();
            e.url_info_about = "https://github.com/ShareX/ShareX".into();
        });
        assert_eq!(site_pages(&named).len(), 1);
    }

    #[test]
    fn a_game_with_a_longer_name_in_steams_store_is_found_when_nothing_has_its_own_name() {
        let json = r#"{"items":[
            {"type":"app","name":"The Last of Us™ Part I","id":1888930},
            {"type":"app","name":"The Last of Us Part I - Soundtrack","id":5},
            {"type":"app","name":"The Last of Us™ Part II Remastered","id":2531310}]}"#;
        assert_eq!(steam_pick(json, "The Last of Us"), None);
        assert_eq!(steam_pick_longer(json, "The Last of Us"), Some(1888930));
        // an add-on or a soundtrack is not the game, and a short name is too loose
        assert_eq!(steam_pick_longer(r#"{"items":[{"type":"app","name":"Starfield Soundtrack","id":1},{"type":"app","name":"Starfield Creation Kit Tool","id":2}]}"#, "Starfield"), None);
        assert_eq!(steam_pick_longer(json, "The"), None);
        assert_eq!(steam_pick_longer(r#"{"items":[{"type":"dlc","name":"Far Cry 6 Pack","id":1}]}"#, "Far Cry"), None);
    }

    #[test]
    fn the_search_address_holds_only_the_name() {
        assert_eq!(steam_search_url("Far Cry 6"), "https://store.steampowered.com/api/storesearch/?term=Far%20Cry%206&l=english&cc=US");
        assert_eq!(steam_search_url("Black Myth: Wukong"), "https://store.steampowered.com/api/storesearch/?term=Black%20Myth%3A%20Wukong&l=english&cc=US");
    }
}
