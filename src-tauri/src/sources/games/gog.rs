//! GOG Galaxy: each installed game has a key under `HKLM\SOFTWARE\GOG.com\Games` (the 32 bit view;
//! the key is the product id) with its `gameName`, `path`, version (`ver`) and build (`buildId`).
//! A game's folder also holds `goggame-<id>.info` (JSON with the name) and the game's icon
//! `goggame-<id>.ico`. Galaxy keeps no marker for a waiting update there, so none is reported.
//! Patch notes are GOG's public product changelog, which is HTML; `changelog_items` splits it into
//! one item per version.

use super::{dir_of, own_uri, reg_text, Game, Launcher, NotesFrom};
use std::path::{Path, PathBuf};

/// What a game's registry key says.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reg {
    /// The product id (the key's name, or its `gameID`)
    pub id: u64,
    pub name: String,
    pub path: String,
    pub ver: String,
    pub build: String,
    /// The product id of the game an add-on belongs to (`dependsOn`)
    pub depends_on: String,
}

/// The name in a game's `goggame-<id>.info`.
pub fn info_name(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json.trim_start_matches('\u{feff}')).ok()?;
    let n = v.get("name")?.as_str()?.trim();
    (!n.is_empty()).then(|| n.to_string())
}

/// The game a registry key stands for. `info` is the name from the game's `.info` file, `icon` its
/// icon file. None for an add-on (it depends on another product), a game with no folder, or one
/// with no name at all.
pub fn game_of(r: &Reg, info: Option<&str>, icon: Option<PathBuf>) -> Option<Game> {
    if r.id == 0 || (!r.depends_on.trim().is_empty() && r.depends_on.trim() != r.id.to_string()) {
        return None;
    }
    let dir = dir_of(&r.path)?;
    let name = [r.name.trim(), info.unwrap_or("").trim()].into_iter().find(|n| !n.is_empty()).map(str::to_string).or_else(|| dir.file_name().map(|n| n.to_string_lossy().to_string()))?;
    let id = r.id;
    // The version GOG names, else the build it installed.
    let version = if !r.ver.trim().is_empty() { r.ver.trim().to_string() } else { r.build.trim().to_string() };
    Some(Game {
        launcher: Launcher::Gog,
        key: id.to_string(),
        name,
        version,
        update: None,
        install_dir: Some(dir),
        size: None,
        updated_at: None,
        launch_uri: None,
        show_uri: Some(format!("goggalaxy://openGameView/{id}")).filter(|u| own_uri(Launcher::Gog, u)),
        uninstall_uri: None,
        page: None,
        notes: NotesFrom::Gog(id),
        icon,
    })
}

/// GOG's public product API with the changelog.
pub fn changelog_url(id: u64) -> String {
    format!("https://api.gog.com/products/{id}?expand=changelog")
}

fn month_number(word: &str) -> Option<u32> {
    const MONTHS: [&str; 12] = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"];
    let w = word.trim_end_matches('.').to_ascii_lowercase();
    (w.len() >= 3).then(|| MONTHS.iter().position(|m| m.starts_with(&w)).map(|i| i as u32 + 1)).flatten()
}

/// The date written in a version heading ("1.6.3 update (March 27th, 2024)", "Patch 2.5 Oct 22nd
/// 2026", "v1.2 - 2024-03-27"): as an ISO 8601 date, and the heading without it.
fn split_date(title: &str) -> (String, Option<String>) {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static ISO: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(\d{4})-(\d{2})-(\d{2})\b").unwrap());
    static MDY: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b([A-Za-z]{3,9})\.?,? (\d{1,2})(?:st|nd|rd|th)?,? (\d{4})\b").unwrap());
    static LOOSE_END: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*[,;:|\-]\s*([)\]])").unwrap());
    static LOOSE_START: Lazy<Regex> = Lazy::new(|| Regex::new(r"([(\[])\s*[,;:|\-]\s*").unwrap());
    static DMY: Lazy<Regex> = Lazy::new(|| Regex::new(r"\b(\d{1,2})(?:st|nd|rd|th)? ([A-Za-z]{3,9})\.?,? (\d{4})\b").unwrap());
    let found = ISO
        .captures(title)
        .and_then(|c| Some((c.get(0)?, c[1].parse::<i32>().ok()?, c[2].parse::<u32>().ok()?, c[3].parse::<u32>().ok()?)))
        .or_else(|| MDY.captures_iter(title).find_map(|c| Some((c.get(0)?, c[3].parse::<i32>().ok()?, month_number(&c[1])?, c[2].parse::<u32>().ok()?))))
        .or_else(|| DMY.captures_iter(title).find_map(|c| Some((c.get(0)?, c[3].parse::<i32>().ok()?, month_number(&c[2])?, c[1].parse::<u32>().ok()?))));
    let Some((m, y, mo, d)) = found else { return (title.trim().to_string(), None) };
    let Some(date) = chrono::NaiveDate::from_ymd_opt(y, mo, d) else { return (title.trim().to_string(), None) };
    let rest = format!("{}{}", &title[..m.start()], &title[m.end()..]);
    // What the date leaves behind: empty brackets, loose separators, a bracket left open.
    let rest = LOOSE_START.replace_all(&LOOSE_END.replace_all(&rest, "$1"), "$1").replace("()", "").replace("[]", "");
    let rest = rest.trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | ',' | ':' | '|')).trim_end_matches(['(', '[', ' ']).trim_start_matches([')', ']', ' ']).to_string();
    (if rest.is_empty() { title.trim().to_string() } else { rest }, Some(format!("{date}T00:00:00Z")))
}

/// HTML as lines for `notes::text_lines`: list items as "- " lines, sub headings as "### " lines.
fn html_lines(html: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static BOLD_PARA: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<p[^>]*>\s*<(?:strong|b)>(.*?)</(?:strong|b)>\s*:?\s*</p>").unwrap());
    static SUB: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<h[1-6][^>]*>(.*?)</h[1-6]>").unwrap());
    static LI: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<li[^>]*>").unwrap());
    static BREAKS: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)</li>|</?(?:p|ul|ol|div|br|tr|table)[^>]*>").unwrap());
    let t = BOLD_PARA.replace_all(html, "\n### $1\n");
    let t = SUB.replace_all(&t, "\n### $1\n");
    let t = LI.replace_all(&t, "\n- ");
    let t = BREAKS.replace_all(&t, "\n");
    let mut out = Vec::new();
    for raw in t.split('\n') {
        let line = crate::notes::strip_tags(raw);
        let line = line.trim();
        if let Some(h) = line.strip_prefix("### ") {
            // Headings come as "[New content & features]".
            let h = h.trim().trim_start_matches('[').trim_end_matches(']').trim_end_matches(':').trim();
            if !h.is_empty() {
                out.push(format!("### {h}"));
            }
        } else if let Some(item) = line.strip_prefix("- ") {
            if !item.trim().is_empty() {
                out.push(format!("- {}", item.trim()));
            }
        } else if line != "-" && !line.is_empty() {
            out.push(line.to_string());
        }
    }
    out.join("\n")
}

/// The versions in the HTML of a product changelog, newest first as GOG lists them: each with its
/// heading (without the date), the date when the heading has one, and the change lines. The page
/// is a list of `<h4>1.6.3 update (March 27th, 2024)</h4>` headings, each followed by the changes
/// as paragraphs, sub headings and lists; the level of the first heading is the version level.
pub fn changelog_items(json: &str) -> Vec<(String, Option<String>, Vec<String>)> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static HEAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<h([1-6])[^>]*>(.*?)</h[1-6]>").unwrap());
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let Some(html) = v.get("changelog").and_then(|c| c.as_str()) else { return vec![] };
    let heads: Vec<(usize, usize, u32, String)> = HEAD.captures_iter(html).filter_map(|c| Some((c.get(0)?.start(), c.get(0)?.end(), c[1].parse().ok()?, crate::notes::strip_tags(&c[2])))).collect();
    let Some(level) = heads.first().map(|h| h.2) else { return vec![] };
    let versions: Vec<&(usize, usize, u32, String)> = heads.iter().filter(|h| h.2 <= level).collect();
    let mut out = Vec::new();
    for (i, (_, end, _, title)) in versions.iter().enumerate() {
        let stop = versions.get(i + 1).map(|h| h.0).unwrap_or(html.len());
        let title = title.trim();
        if title.is_empty() {
            continue;
        }
        let (name, date) = split_date(title);
        out.push((name, date, crate::notes::text_lines(&html_lines(&html[*end..stop]))));
    }
    out
}

/// Every game GOG Galaxy has installed on this PC.
pub fn scan() -> Vec<Game> {
    use winreg::enums::*;
    use winreg::RegKey;
    let flags = KEY_READ | KEY_WOW64_32KEY;
    let Ok(games) = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey_with_flags("SOFTWARE\\GOG.com\\Games", flags) else { return vec![] };
    let mut out: Vec<Game> = Vec::new();
    for name in games.enum_keys().flatten() {
        let Ok(k) = games.open_subkey_with_flags(&name, flags) else { continue };
        let id = [reg_text(&k, "gameID"), name.clone()].iter().find_map(|s| s.trim().parse::<u64>().ok()).unwrap_or(0);
        let r = Reg { id, name: reg_text(&k, "gameName"), path: reg_text(&k, "path"), ver: reg_text(&k, "ver"), build: reg_text(&k, "buildId"), depends_on: reg_text(&k, "dependsOn") };
        let Some(dir) = dir_of(&r.path).filter(|d| d.is_dir()) else { continue };
        let info = std::fs::read(dir.join(format!("goggame-{id}.info"))).ok().and_then(|b| info_name(&super::decode_text(&b)));
        let ico: &Path = &dir.join(format!("goggame-{id}.ico"));
        let icon = ico.is_file().then(|| ico.to_path_buf());
        if let Some(g) = game_of(&r, info.as_deref(), icon) {
            if !out.iter().any(|o| o.key == g.key) {
                out.push(g);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Reg {
        Reg { id: 1453375253, name: "Some Farm".into(), path: r"D:\GOG Games\Some Farm".into(), ver: "1.6.14".into(), build: "58364182".into(), depends_on: String::new() }
    }

    #[test]
    fn a_registry_key_gives_the_game() {
        let g = game_of(&reg(), None, None).unwrap();
        assert_eq!(g.id(), "game:gog:1453375253");
        assert_eq!(g.name, "Some Farm");
        assert_eq!(g.version, "1.6.14");
        assert_eq!(g.install_dir.as_deref(), Some(Path::new(r"D:\GOG Games\Some Farm")));
        assert_eq!(g.notes, NotesFrom::Gog(1453375253));
        assert_eq!(g.show_uri.as_deref(), Some("goggalaxy://openGameView/1453375253"));
        assert!(g.show_uri.as_deref().is_some_and(|u| own_uri(Launcher::Gog, u)));
        assert_eq!(g.update, None);
    }

    #[test]
    fn the_build_stands_in_for_a_missing_version_and_the_info_file_for_a_missing_name() {
        let mut r = reg();
        r.ver.clear();
        r.name.clear();
        let g = game_of(&r, Some("Some Farm (info)"), None).unwrap();
        assert_eq!(g.version, "58364182");
        assert_eq!(g.name, "Some Farm (info)");
        // With neither, the folder names it.
        assert_eq!(game_of(&r, None, None).unwrap().name, "Some Farm");
    }

    #[test]
    fn add_ons_and_unknown_products_are_not_games() {
        let mut dlc = reg();
        dlc.depends_on = "1207658930".into();
        assert!(game_of(&dlc, None, None).is_none());
        // A product that names itself is the game.
        let mut own = reg();
        own.depends_on = "1453375253".into();
        assert!(game_of(&own, None, None).is_some());
        let mut none = reg();
        none.id = 0;
        assert!(game_of(&none, None, None).is_none());
        let mut nopath = reg();
        nopath.path.clear();
        assert!(game_of(&nopath, None, None).is_none());
    }

    #[test]
    fn the_info_files_name_is_read() {
        let info = r#"{"buildId":"58364182","clientId":"11112222333344445","gameId":"1453375253","language":"English","languages":["en-US"],"name":"Some Farm","playTasks":[{"category":"game","isPrimary":true,"name":"Some Farm","path":"Game.exe","type":"FileTask"}],"rootGameId":"1453375253","version":1}"#;
        assert_eq!(info_name(info).as_deref(), Some("Some Farm"));
        assert_eq!(info_name("{}"), None);
        assert_eq!(info_name("not json"), None);
    }

    #[test]
    fn the_changelog_address_is_the_public_product_api() {
        assert_eq!(changelog_url(1453375253), "https://api.gog.com/products/1453375253?expand=changelog");
    }

    /// The shape of GOG's changelog field: an `<h4>` per version (the date is in its text), `<h5>`
    /// or a bold paragraph for a group of changes, and lists.
    fn changelog_json() -> String {
        let html = "<h4>1.6.3 update (March 27th, 2024)</h4><h5>[New content &amp; features]</h5><ul><li>Added a new <b>farm</b> map</li><li>Pets can now sleep &mdash; on beds</li></ul><h5>[Bug fixes]</h5><ul><li>Fixed a crash when loading old saves</li></ul>\
<h4>Patch 2.5 Oct 22nd 2026</h4><p><strong>Extra Content</strong></p><p>A new side quest.</p><ul><li>New clothing sets<ul><li>Jackets</li></ul></li></ul>\
<h4>1.0 (2016-02-26)</h4><ul><li>Initial release</li></ul><p>&nbsp;</p>";
        serde_json::json!({ "id": 1453375253u64, "title": "Some Farm", "changelog": html }).to_string()
    }

    #[test]
    fn the_changelog_is_split_into_versions_with_dates_and_lines() {
        let items = changelog_items(&changelog_json());
        assert_eq!(items.len(), 3);
        let (title, date, lines) = &items[0];
        assert_eq!(title, "1.6.3 update");
        assert_eq!(date.as_deref(), Some("2024-03-27T00:00:00Z"));
        assert_eq!(lines[0], "## New content & features");
        assert!(lines.contains(&"Added a new farm map".to_string()), "{lines:?}");
        assert!(lines.contains(&"Fixed a crash when loading old saves".to_string()), "{lines:?}");
        assert!(lines.contains(&"## Bug fixes".to_string()), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains('<') || l.contains("&amp;")), "{lines:?}");
        let (title, date, lines) = &items[1];
        assert_eq!(title, "Patch 2.5");
        assert_eq!(date.as_deref(), Some("2026-10-22T00:00:00Z"));
        // A single heading on top of everything says nothing, so the notes' own tidying drops it.
        assert_eq!(lines[0], "A new side quest");
        assert!(lines.contains(&"Jackets".to_string()) && lines.contains(&"New clothing sets".to_string()), "{lines:?}");
        let (title, date, lines) = &items[2];
        assert_eq!(title, "1.0");
        assert_eq!(date.as_deref(), Some("2016-02-26T00:00:00Z"));
        assert_eq!(lines, &["Initial release".to_string()]);
    }

    #[test]
    fn no_dashes_come_out_of_a_changelog() {
        let items = changelog_items(&changelog_json());
        let all = items.iter().flat_map(|(t, _, l)| std::iter::once(t).chain(l.iter())).cloned().collect::<Vec<_>>().join("\n");
        assert!(!all.contains('\u{2014}') && !all.contains('\u{2013}'), "{all}");
    }

    #[test]
    fn a_changelog_that_is_missing_or_not_split_gives_nothing() {
        assert!(changelog_items("").is_empty());
        assert!(changelog_items("{}").is_empty());
        assert!(changelog_items(r#"{"changelog":null}"#).is_empty());
        assert!(changelog_items(r#"{"changelog":"Just a plain note with no headings."}"#).is_empty());
    }

    #[test]
    fn dates_in_headings_are_read_in_the_common_forms_only() {
        assert_eq!(split_date("Hotfix 1.2 (28 May 2018)"), ("Hotfix 1.2".to_string(), Some("2018-05-28T00:00:00Z".to_string())));
        assert_eq!(split_date("Version 3.0 - Sept 3, 2021").1.as_deref(), Some("2021-09-03T00:00:00Z"));
        // A day and month that do not exist, or no date: the heading as it is.
        assert_eq!(split_date("Update February 31st 2020"), ("Update February 31st 2020".to_string(), None));
        assert_eq!(split_date("Version 1.2.3"), ("Version 1.2.3".to_string(), None));
        // Brackets that belong to the heading stay; the ones around the date go.
        assert_eq!(split_date("The Big Update (Patch 1.6, September 6th 2022)"), ("The Big Update (Patch 1.6)".to_string(), Some("2022-09-06T00:00:00Z".to_string())));
        assert_eq!(split_date("Patch 1.5.1 (December, 24th 2020)"), ("Patch 1.5.1".to_string(), Some("2020-12-24T00:00:00Z".to_string())));
        assert_eq!(split_date("Update 1.11(A) (March 22nd, 2017)").0, "Update 1.11(A)");
        // A date alone keeps the whole heading as its name.
        assert_eq!(split_date("2020-05-01"), ("2020-05-01".to_string(), Some("2020-05-01T00:00:00Z".to_string())));
    }

    /// Run with `cargo test gog -- --ignored`: GOG's real changelog for a public product (nothing
    /// about this PC is sent), the live shape and not a fixture.
    #[tokio::test]
    #[ignore]
    async fn live_changelog_of_a_public_product() {
        // Two public products with different markup.
        for id in [1453375253u64, 1423049311] {
            let body = reqwest::Client::new().get(changelog_url(id)).send().await.unwrap().text().await.unwrap();
            let items = changelog_items(&body);
            assert!(!items.is_empty());
            assert!(items.iter().any(|(_, d, l)| d.is_some() && !l.is_empty()));
            eprintln!("{id}: {} versions", items.len());
            for (t, d, l) in items.iter().take(40) {
                eprintln!("  {t:?} {d:?} {} lines", l.len());
            }
            for l in items[0].2.iter().take(14) {
                eprintln!("    | {l}");
            }
        }
    }
}
