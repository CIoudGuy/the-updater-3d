//! EA app: its own list of installed games (`ProgramData\EA Desktop\...\IS`) is encrypted with a key
//! made from this PC's hardware, so it is not read. What is documented and plain: every game folder
//! the EA installer made holds `__Installer\installerdata.xml` (the game's title and content id),
//! the games are in the EA app's library folder (`Program Files\EA Games\<game>`) or have an
//! uninstall entry whose command is the EA installer's cleanup
//! (`...\EAInstaller\<game>\Cleanup.exe" uninstall_game ...`) naming their folder. The content id
//! is what `origin2://game/launch?offerIds=<id>` takes to start the game through the EA app.
//! The EA app keeps no readable marker for a waiting update, so none is reported.

use super::{decode_text, dir_of, own_uri, Game, Launcher, NotesFrom};
use crate::winutil::UninstallEntry;
use std::path::{Path, PathBuf};

/// What a game's `installerdata.xml` says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EaMeta {
    pub content_id: String,
    pub title: Option<String>,
}

/// A content id is 1 to 64 letters, digits and ". _ : -", starting with a letter or digit. Anything
/// else is not an id (and would not be safe in an address).
pub fn valid_content_id(s: &str) -> bool {
    let mut it = s.chars();
    it.next().is_some_and(|c| c.is_ascii_alphanumeric()) && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '-'))
}

/// `installerdata.xml`: the first `contentIDs/contentID` and the `gameTitles/gameTitle` (English
/// first). None when the XML is broken or holds no valid content id.
pub fn parse_installer_data(text: &str) -> Option<EaMeta> {
    let doc = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')).ok()?;
    let under = |n: &roxmltree::Node, parent: &str, tag: &str| n.has_tag_name(tag) && n.parent().is_some_and(|p| p.has_tag_name(parent));
    let content_id = doc.descendants().filter(|n| under(n, "contentIDs", "contentID")).filter_map(|n| n.text()).map(str::trim).find(|t| valid_content_id(t))?.to_string();
    let titles: Vec<(bool, String)> = doc
        .descendants()
        .filter(|n| under(n, "gameTitles", "gameTitle"))
        .filter_map(|n| Some((n.attribute("locale").is_some_and(|l| l.eq_ignore_ascii_case("en_US")), n.text()?.trim().to_string())))
        .filter(|(_, t)| !t.is_empty())
        .collect();
    let title = titles.iter().find(|(en, _)| *en).or_else(|| titles.first()).map(|(_, t)| t.clone());
    Some(EaMeta { content_id, title })
}

/// Is this uninstall command the EA installer's cleanup of a game?
pub fn is_cleanup(cmd: &str) -> bool {
    let low = cmd.to_ascii_lowercase();
    low.contains("eainstaller") && low.contains("cleanup.exe") && low.contains("uninstall_game")
}

/// The game in folder `dir`. `entry` is its uninstall entry when it has one (version, size, name).
pub fn game_of(dir: &Path, meta: &EaMeta, entry: Option<&UninstallEntry>) -> Option<Game> {
    let name = [meta.title.clone().unwrap_or_default(), entry.map(|e| e.display_name.trim().to_string()).unwrap_or_default(), dir.file_name()?.to_string_lossy().to_string()].into_iter().find(|n| !n.trim().is_empty())?;
    let id = &meta.content_id;
    Some(Game {
        launcher: Launcher::Ea,
        key: id.clone(),
        name: name.trim().to_string(),
        version: entry.map(|e| e.display_version.trim().to_string()).unwrap_or_default(),
        update: None,
        install_dir: Some(dir.to_path_buf()),
        size: entry.map(|e| e.estimated_size_kb.saturating_mul(1024)).filter(|s| *s > 0),
        updated_at: entry.and_then(|e| crate::winutil::install_date_iso(&e.install_date)),
        launch_uri: Some(format!("origin2://game/launch?offerIds={id}")).filter(|u| own_uri(Launcher::Ea, u)),
        show_uri: None,
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::None,
        icon: None,
        program: None,
    })
}

/// The folder of every EA library (`Program Files\EA Games`).
fn library_folders() -> Vec<PathBuf> {
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().filter_map(|v| std::env::var_os(v)).map(|p| PathBuf::from(p).join("EA Games")).collect()
}

/// The EA games among the uninstall `entries` of this PC and in the library folders `libs`.
pub fn scan_in(entries: &[UninstallEntry], libs: &[PathBuf]) -> Vec<Game> {
    fn add<'a>(list: &mut Vec<(PathBuf, Option<&'a UninstallEntry>)>, dir: PathBuf, e: Option<&'a UninstallEntry>) {
        if !list.iter().any(|(d, _)| d.to_string_lossy().eq_ignore_ascii_case(&dir.to_string_lossy())) {
            list.push((dir, e));
        }
    }
    let mut candidates: Vec<(PathBuf, Option<&UninstallEntry>)> = Vec::new();
    for e in entries.iter().filter(|e| is_cleanup(&e.uninstall_string) || is_cleanup(&e.quiet_uninstall_string)) {
        if let Some(d) = dir_of(&e.install_location) {
            add(&mut candidates, d, Some(e));
        }
    }
    for lib in libs {
        for sub in std::fs::read_dir(lib).into_iter().flatten().flatten().map(|d| d.path()).filter(|p| p.is_dir()) {
            add(&mut candidates, sub, None);
        }
    }
    let mut out: Vec<Game> = Vec::new();
    for (dir, e) in candidates {
        let Some(meta) = std::fs::read(dir.join("__Installer").join("installerdata.xml")).ok().and_then(|b| parse_installer_data(&decode_text(&b))) else { continue };
        if let Some(g) = game_of(&dir, &meta, e) {
            if !out.iter().any(|o| o.key == g.key) {
                out.push(g);
            }
        }
    }
    out
}

/// Every EA game among `entries` (the uninstall entries of this PC) and in the EA app's library
/// folders.
pub fn scan_entries(entries: &[UninstallEntry]) -> Vec<Game> {
    scan_in(entries, &library_folders())
}

/// Every game the EA app installed on this PC.
pub fn scan() -> Vec<Game> {
    scan_entries(&crate::winutil::uninstall_entries())
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<DiPManifest version="4.0">
  <gameTitles>
    <gameTitle locale="de_DE">Ein Spiel</gameTitle>
    <gameTitle locale="en_US">Some EA Game</gameTitle>
  </gameTitles>
  <contentIDs>
    <contentID>1026481</contentID>
    <contentID>1026482</contentID>
  </contentIDs>
  <runtime>
    <launcher><filePath>[HKEY_LOCAL_MACHINE\SOFTWARE\Someone\Game\Install Dir]game.exe</filePath></launcher>
  </runtime>
</DiPManifest>"#;

    #[test]
    fn the_content_id_and_english_title_are_read() {
        let m = parse_installer_data(XML).unwrap();
        assert_eq!(m, EaMeta { content_id: "1026481".into(), title: Some("Some EA Game".into()) });
        // The title of another language stands in when there is no English one.
        let m = parse_installer_data(&XML.replace("en_US", "fr_FR")).unwrap();
        assert_eq!(m.title.as_deref(), Some("Ein Spiel"));
    }

    #[test]
    fn a_file_with_no_valid_content_id_gives_nothing() {
        assert!(parse_installer_data("").is_none());
        assert!(parse_installer_data("<DiPManifest><contentIDs></contentIDs></DiPManifest>").is_none());
        assert!(parse_installer_data("<DiPManifest><contentIDs><contentID>a b&amp;c</contentID></contentIDs></DiPManifest>").is_none());
        // A file cut short is broken XML.
        assert!(parse_installer_data("<DiPManifest><contentIDs><contentID>12</contentID>").is_none());
        // An id outside contentIDs (a launcher entry, say) is not the game's id.
        assert!(parse_installer_data("<DiPManifest><launcher><contentID>12</contentID></launcher></DiPManifest>").is_none());
        // The first valid one is used.
        let m = parse_installer_data("<DiPManifest><contentIDs><contentID>bad id</contentID><contentID>77</contentID></contentIDs></DiPManifest>").unwrap();
        assert_eq!(m.content_id, "77");
    }

    #[test]
    fn content_ids_are_checked() {
        for ok in ["1026481", "Origin.OFR.50.0001234", "a-b_c:d.e", "x"] {
            assert!(valid_content_id(ok), "{ok}");
        }
        for bad in ["", "-1", ".x", "a b", "a&b", "a/b", "a\"b", &"1".repeat(65)] {
            assert!(!valid_content_id(bad), "{bad}");
        }
    }

    #[test]
    fn the_ea_installers_cleanup_command_is_recognised() {
        assert!(is_cleanup(r#""C:\Program Files\Common Files\EAInstaller\Some EA Game\Cleanup.exe" uninstall_game-1026481"#));
        assert!(!is_cleanup(r#""C:\Program Files\Other\unins000.exe""#));
        assert!(!is_cleanup(r#""C:\Program Files\Common Files\EAInstaller\Some EA Game\Touchup.exe" install"#));
    }

    #[test]
    fn a_game_gets_its_launch_address_from_the_content_id() {
        let m = parse_installer_data(XML).unwrap();
        let e = UninstallEntry { key: "k".into(), display_name: "Entry Name".into(), display_version: "1.2.3".into(), estimated_size_kb: 2048, install_date: "20260514".into(), ..Default::default() };
        let g = game_of(Path::new(r"C:\Program Files\EA Games\Some EA Game"), &m, Some(&e)).unwrap();
        assert_eq!(g.id(), "game:ea:1026481");
        assert_eq!(g.name, "Some EA Game");
        assert_eq!(g.version, "1.2.3");
        assert_eq!(g.size, Some(2048 * 1024));
        assert_eq!(g.updated_at.as_deref(), Some("2026-05-14T00:00:00Z"));
        assert_eq!(g.launch_uri.as_deref(), Some("origin2://game/launch?offerIds=1026481"));
        assert!(g.launch_uri.as_deref().is_some_and(|u| own_uri(Launcher::Ea, u)));
        assert_eq!((g.show_uri, g.uninstall_uri, g.update), (None, None, None));
        // The entry's name, then the folder's, stand in for a missing title.
        let bare = EaMeta { content_id: "1".into(), title: None };
        assert_eq!(game_of(Path::new(r"C:\x\Folder Name"), &bare, Some(&e)).unwrap().name, "Entry Name");
        assert_eq!(game_of(Path::new(r"C:\x\Folder Name"), &bare, None).unwrap().name, "Folder Name");
    }

    #[test]
    fn games_are_found_in_library_folders_and_by_their_uninstall_entry() {
        let root = std::env::temp_dir().join(format!("updater3d-ea-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let lib = root.join("EA Games");
        let make = |dir: &Path, xml: &[u8]| {
            std::fs::create_dir_all(dir.join("__Installer")).unwrap();
            std::fs::write(dir.join("__Installer").join("installerdata.xml"), xml).unwrap();
        };
        // In the library, as UTF-8; elsewhere, as UTF-16 with a byte order mark.
        make(&lib.join("Some EA Game"), XML.as_bytes());
        let elsewhere = root.join("D").join("Other EA Game");
        let utf16: Vec<u8> = [0xFFu8, 0xFE].into_iter().chain(XML.replace("1026481", "5550001").replace("Some EA Game", "Other EA Game").encode_utf16().flat_map(|u| u.to_le_bytes())).collect();
        make(&elsewhere, &utf16);
        // A folder of the library with no installer data, and a file next to the games.
        std::fs::create_dir_all(lib.join("Not A Game")).unwrap();
        std::fs::write(lib.join("readme.txt"), "x").unwrap();
        let entries = [
            UninstallEntry {
                key: "{X}".into(),
                display_name: "Other EA Game".into(),
                display_version: "9.9".into(),
                install_location: elsewhere.to_string_lossy().to_string(),
                uninstall_string: r#""C:\Program Files\Common Files\EAInstaller\Other EA Game\Cleanup.exe" uninstall_game-5550001"#.into(),
                ..Default::default()
            },
            // Not the EA installer: not looked at.
            UninstallEntry { key: "{Y}".into(), display_name: "Else".into(), install_location: lib.join("Not A Game").to_string_lossy().to_string(), uninstall_string: "x.exe".into(), ..Default::default() },
        ];
        let got = scan_in(&entries, &[lib.clone(), root.join("missing library")]);
        let mut seen: Vec<(String, String, String)> = got.iter().map(|g| (g.key.clone(), g.name.clone(), g.version.clone())).collect();
        seen.sort();
        assert_eq!(seen, [("1026481".into(), "Some EA Game".into(), String::new()), ("5550001".into(), "Other EA Game".into(), "9.9".into())]);
        let _ = std::fs::remove_dir_all(&root);
    }
}
