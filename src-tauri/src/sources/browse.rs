//! Browsing the whole winget catalog in Get apps, by category and A to Z, a page at a time.
//!
//! The index has ids, names and tags but no descriptions; those are in each package's newest
//! manifest. They are read only for the entries shown, a few at a time, and kept in
//! `winget/details.json` with the "can be installed silently with a checksum" verdict, so a
//! page seen once fills in at once the next time.

use super::other;
use super::winget::{self, Index, Pkg};
use crate::model::CatalogEntry;
use anyhow::Result;
use futures_util::StreamExt;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

// ---------------------------------------------------------------- categories

pub const CATEGORIES: &[&str] = &["Browsers", "Communication", "Media", "Graphics and 3D", "Development", "Games", "Utilities", "Office", "Security", "Networking", "System", "Other"];
const OTHER: usize = 11;

/// Tags (and name words) that put a package in a category. Some catalogs tag in Chinese too.
const WORDS: &[(usize, &[&str])] = &[
    (0, &["browser", "web-browser", "webbrowser", "chromium", "gecko", "firefox", "chrome", "浏览器", "网页浏览器", "navegador", "navigateur", "браузер", "nettleser"]),
    (1, &["chat", "messaging", "messenger", "email", "mail", "e-mail", "email-client", "meeting", "meetings", "conference", "conferencing", "video-conferencing", "voice-conferencing", "voip", "discord", "slack", "telegram", "irc", "sms", "talk", "call", "calls", "phone", "matrix", "xmpp", "聊天", "邮件", "电子邮件", "会议", "视频会议", "沟通", "电话", "语音会议", "开会"]),
    (2, &["audio", "video", "music", "player", "media", "multimedia", "media-player", "music-player", "video-player", "audio-player", "streaming", "podcast", "podcasts", "radio", "movie", "movies", "youtube", "codec", "ffmpeg", "recording", "recorder", "transcode", "sound", "mp3", "subtitles", "dvd", "iptv", "视频", "音频", "音乐", "播放器", "媒体", "多媒体", "流媒体", "录制", "音视频", "转码", "推流", "声音"]),
    (3, &["image", "images", "photo", "photos", "picture", "pictures", "graphics", "design", "drawing", "draw", "paint", "painting", "3d", "cad", "svg", "vector", "illustration", "photography", "gif", "icon", "icons", "modeling", "modelling", "rendering", "render", "animation", "image-editor", "photo-editor", "图像", "图片", "照片", "设计", "绘图", "图形"]),
    (4, &["development", "developer", "developer-tools", "devtools", "programming", "coding", "code", "code-editor", "ide", "git", "github", "sdk", "compiler", "debug", "debugger", "debugging", "api", "json", "database", "sql", "mysql", "postgresql", "sqlite", "python", "java", "jdk", "jre", "openjdk", "jvm", "dotnet", ".net", "nodejs", "node", "javascript", "typescript", "rust", "golang", "go", "c++", "cpp", "csharp", "c#", "php", "docker", "kubernetes", "k8s", "devops", "terminal", "shell", "console", "command-line", "powershell", "wsl", "programming-language", "runtime", "claude-code", "codex", "mcp", "agentic", "编程", "开发", "代码", "编程语言", "集成开发环境", "调试", "终端", "数据库"]),
    (5, &["game", "games", "gaming", "videogame", "video-game", "emulator", "emulation", "minecraft", "steam", "游戏"]),
    (6, &["utility", "utilities", "tool", "tools", "file-manager", "archive", "archiver", "compression", "zip", "7z", "rar", "clipboard", "screenshot", "screen-capture", "capture", "backup", "sync", "download", "downloader", "converter", "convert", "conversion", "cleaner", "calculator", "keyboard", "mouse", "search", "rename", "launcher", "工具", "备份", "下载", "转换", "同步"]),
    (7, &["office", "productivity", "pdf", "document", "documents", "docs", "ebook", "e-book", "reader", "notes", "note-taking", "markdown", "writing", "spreadsheet", "excel", "word", "presentation", "calendar", "todo", "task", "tasks", "mindmap", "mind-map", "finance", "accounting", "education", "translation", "translate", "dictionary", "ocr", "办公", "文档", "笔记", "电子书", "阅读器", "教育", "翻译", "任务"]),
    (8, &["security", "password", "password-manager", "passwords", "encryption", "encrypt", "antivirus", "anti-virus", "malware", "privacy", "2fa", "authenticator", "firewall", "protection", "安全", "隐私", "加密"]),
    (9, &["network", "networking", "vpn", "proxy", "ssh", "sftp", "ftp", "remote", "remote-desktop", "remote-access", "remote-control", "p2p", "torrent", "bittorrent", "dns", "http", "server", "wifi", "cloud", "cloud-drive", "cloud-storage", "netdisk", "网络", "代理", "远程", "远程桌面", "网盘", "云盘"]),
    (10, &["system", "driver", "drivers", "hardware", "monitoring", "benchmark", "disk", "partition", "registry", "startup", "performance", "gpu", "cpu", "usb", "boot", "uninstaller", "task-manager", "process", "系统", "驱动", "监控"]),
];

/// When categories get as many votes, the one first here wins: a browser that is also tagged
/// "privacy" stays a browser.
const PRIORITY: &[usize] = &[0, 5, 8, 3, 2, 1, 7, 4, 9, 10, 6];

static WORD_MAP: Lazy<HashMap<&'static str, usize>> = Lazy::new(|| WORDS.iter().flat_map(|(c, ws)| ws.iter().map(move |w| (*w, *c))).collect());

fn norm_tag(t: &str) -> String {
    t.trim().to_lowercase().replace(['_', ' '], "-")
}

fn votes<'a>(words: impl Iterator<Item = &'a str>) -> [u32; 12] {
    let mut v = [0u32; 12];
    let mut seen = HashSet::new();
    for w in words {
        let n = norm_tag(w);
        if let Some(c) = WORD_MAP.get(n.as_str()) {
            if seen.insert(n) {
                v[*c] += 1;
            }
        }
    }
    v
}

fn pick(v: &[u32; 12]) -> Option<usize> {
    let best = *v.iter().max()?;
    (best > 0).then(|| PRIORITY.iter().copied().find(|c| v[*c] == best)).flatten()
}

/// The category of a package from its tags; with no tag that says, the words of its name and id.
pub fn category_of(tags: &[String], name: &str, id: &str) -> usize {
    if let Some(c) = pick(&votes(tags.iter().map(String::as_str))) {
        return c;
    }
    let lower = format!("{} {}", name, id).to_lowercase();
    let words = lower.split(|c: char| !(c.is_alphanumeric() || c == '+' || c == '#')).filter(|w| !w.is_empty());
    pick(&votes(words)).unwrap_or(OTHER)
}

/// "A" to "Z" for names that start with that letter (after any punctuation), "#" for the rest.
pub fn letter_of(name: &str) -> char {
    match name.chars().find(|c| c.is_alphanumeric()) {
        Some(c) if c.is_ascii_alphabetic() => c.to_ascii_uppercase(),
        _ => '#',
    }
}

// ---------------------------------------------------------------- the list

pub struct Item {
    pub pkg: Pkg,
    pub category: usize,
    pub letter: char,
}

pub struct BrowseList {
    /// By name, without the apps another source handles or that are left out on purpose
    pub items: Vec<Item>,
    /// Packages in the whole index
    pub packages: usize,
}

pub fn build(idx: &Index) -> BrowseList {
    let tags = idx.all_tags();
    let none = Vec::new();
    let mut items: Vec<Item> = idx
        .all()
        .into_iter()
        .filter(|p| !other::handled_package(&p.id) && other::excluded_package(&p.id).is_none())
        .map(|p| {
            let category = category_of(tags.get(&p.row).unwrap_or(&none), &p.name, &p.id);
            let letter = letter_of(&p.name);
            Item { pkg: p, category, letter }
        })
        .collect();
    items.sort_by_cached_key(|i| (i.pkg.name.to_lowercase(), i.pkg.id.to_lowercase()));
    BrowseList { items, packages: idx.count() }
}

/// Entries of a category and starting letter, `offset` on, and how many there are in all.
pub fn page<'a>(list: &'a BrowseList, category: Option<usize>, letter: Option<char>, offset: usize, limit: usize, keep: impl Fn(&Item) -> bool) -> (usize, Vec<&'a Item>) {
    let mut total = 0;
    let mut out = Vec::new();
    for i in list.items.iter().filter(|i| category.is_none_or(|c| i.category == c) && letter.is_none_or(|l| i.letter == l) && keep(i)) {
        if total >= offset && out.len() < limit {
            out.push(i);
        }
        total += 1;
    }
    (total, out)
}

/// How many entries each category has, in `CATEGORIES` order.
pub fn counts(list: &BrowseList, keep: impl Fn(&Item) -> bool) -> Vec<usize> {
    let mut v = vec![0; CATEGORIES.len()];
    for i in list.items.iter().filter(|i| keep(i)) {
        v[i.category] += 1;
    }
    v
}

static LIST: Lazy<Mutex<Option<(std::time::SystemTime, Arc<BrowseList>)>>> = Lazy::new(|| Mutex::new(None));

/// The list for the index on disk, built again when the index file changes.
pub fn list() -> Result<Arc<BrowseList>> {
    let path = winget::index_path();
    let stamp = std::fs::metadata(&path)?.modified()?;
    if let Some((t, l)) = LIST.lock().as_ref() {
        if *t == stamp {
            return Ok(l.clone());
        }
    }
    let l = Arc::new(build(&Index::open(&path)?));
    *LIST.lock() = Some((stamp, l.clone()));
    Ok(l)
}

/// What Get apps gets for one page.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BrowsePage {
    /// Entries of this category and letter in all
    pub total: usize,
    pub entries: Vec<CatalogEntry>,
    /// Packages in the whole winget catalog
    pub packages: usize,
    pub categories: Vec<CategoryCount>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CategoryCount {
    pub name: &'static str,
    pub count: usize,
}

pub fn category_index(name: &str) -> Option<usize> {
    CATEGORIES.iter().position(|c| c.eq_ignore_ascii_case(name))
}

// ---------------------------------------------------------------- descriptions

/// The short line on a card: the manifest's ShortDescription, else our line for a well-known
/// app, else the publisher.
pub fn tagline_for(short: Option<&str>, popular: Option<&str>, publisher: Option<&str>) -> Option<String> {
    [short, popular, publisher].into_iter().flatten().map(str::trim).find(|s| !s.is_empty()).map(|s| one_line(s, 160))
}

/// Our line for a package of the starter list (`other::POPULAR`).
pub fn popular_line(id: &str) -> Option<&'static str> {
    other::POPULAR.iter().find(|(p, _)| p.eq_ignore_ascii_case(id)).map(|(_, l)| *l).filter(|l| !l.is_empty())
}

fn one_line(s: &str, max: usize) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    cut(&flat, max)
}

fn cut(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut t: String = s.chars().take(max).collect();
    if let Some(sp) = t.rfind(' ') {
        t.truncate(sp);
    }
    format!("{}...", t.trim_end_matches([',', '.', ';', ':']))
}

/// What the newest manifest of a package says about it, kept between runs.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Detail {
    /// The version list hash of the index row it was read for: a new version gives a new hash
    pub hash: String,
    pub short: Option<String>,
    pub long: Option<String>,
    pub publisher: Option<String>,
    /// Why the newest version can not be installed silently here; None when it can
    pub blocked: Option<String>,
}

impl Detail {
    pub fn from_manifest(hash: &str, m: &winget::Manifest) -> Detail {
        Detail {
            hash: hash.to_string(),
            short: m.description.clone(),
            long: m.long_description.as_deref().map(|d| cut(d.trim(), 2000)).filter(|d| !d.is_empty() && Some(d.as_str()) != m.description.as_deref()),
            publisher: Some(m.publisher.clone()).filter(|p| !p.is_empty()),
            blocked: other::choose(m, &other::Want::fresh()).err(),
        }
    }
}

/// A change to show on an Other apps card once its manifest was read.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DetailPatch {
    /// "other:<package id>"
    pub id: String,
    pub tagline: Option<String>,
    pub description: Option<String>,
    pub blocked: Option<String>,
}

fn patch_for(pkg_id: &str, d: &Detail) -> DetailPatch {
    DetailPatch {
        id: format!("other:{pkg_id}"),
        tagline: tagline_for(d.short.as_deref(), popular_line(pkg_id), d.publisher.as_deref()),
        description: d.long.clone(),
        blocked: d.blocked.clone(),
    }
}

fn details_path() -> PathBuf {
    winget::dir().join("details.json")
}

/// Lowercase package id -> what its manifest says.
static DETAILS: Lazy<Mutex<HashMap<String, Detail>>> = Lazy::new(|| Mutex::new(std::fs::read(details_path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()));
/// Packages being read now, and those whose manifest could not be read this run.
static BUSY: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));
static FAILED: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

fn save() {
    let body = serde_json::to_vec(&*DETAILS.lock()).unwrap_or_default();
    let path = details_path();
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, body).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// Fill an entry with what is known about its package: description, short line, verdict.
pub fn apply(e: &mut CatalogEntry) {
    let Some(id) = e.id.strip_prefix("other:") else { return };
    let d = DETAILS.lock().get(&id.to_ascii_lowercase()).cloned();
    match d {
        Some(d) => {
            let p = patch_for(id, &d);
            if p.tagline.is_some() {
                e.tagline = p.tagline;
            }
            if e.description.is_none() {
                e.description = p.description;
            }
            e.blocked = p.blocked;
        }
        None => {
            if let Some(l) = popular_line(id) {
                e.tagline = Some(l.to_string());
            }
        }
    }
}

/// Of these packages, the ones with nothing kept for their current version.
pub fn stale(pkgs: &[Pkg]) -> Vec<Pkg> {
    let d = DETAILS.lock();
    let busy = BUSY.lock();
    let failed = FAILED.lock();
    pkgs.iter()
        .filter(|p| {
            let k = p.id.to_ascii_lowercase();
            d.get(&k).is_none_or(|x| x.hash != p.hash) && !busy.contains(&k) && !failed.contains(&k)
        })
        .cloned()
        .collect()
}

/// Packages of the index by id (search results and single pages have only ids).
pub fn pkgs_by_id(ids: &[String]) -> Vec<Pkg> {
    let Ok(idx) = Index::open(&winget::index_path()) else { return vec![] };
    ids.iter().filter(|id| !other::handled_package(id) && other::excluded_package(id).is_none()).filter_map(|id| idx.by_id(id)).collect()
}

/// How many manifests are read at the same time.
const PARALLEL: usize = 6;

/// Read the newest manifest of each package (from the disk cache when it is there), keep what it
/// says and hand the changes to `send` in small batches as they come in.
pub async fn fetch(pkgs: Vec<Pkg>, send: impl Fn(Vec<DetailPatch>)) {
    let pkgs = {
        let mut busy = BUSY.lock();
        pkgs.into_iter().filter(|p| busy.insert(p.id.to_ascii_lowercase())).collect::<Vec<_>>()
    };
    if pkgs.is_empty() {
        return;
    }
    let mut got = futures_util::stream::iter(pkgs.into_iter().map(|p| async move {
        let hash = p.hash.clone();
        let id = p.id.clone();
        let r = tokio::time::timeout(Duration::from_secs(30), other::load_pkg(p)).await;
        (id, hash, r.ok().and_then(|(_, r)| r.ok()).map(|(_, m)| m))
    }))
    .buffer_unordered(PARALLEL);
    let mut batch = Vec::new();
    let mut last = std::time::Instant::now();
    let mut changed = false;
    while let Some((id, hash, m)) = got.next().await {
        let key = id.to_ascii_lowercase();
        BUSY.lock().remove(&key);
        match m {
            Some(m) => {
                let d = Detail::from_manifest(&hash, &m);
                batch.push(patch_for(&id, &d));
                DETAILS.lock().insert(key, d);
                changed = true;
            }
            None => {
                FAILED.lock().insert(key);
            }
        }
        if batch.len() >= 12 || (!batch.is_empty() && last.elapsed() > Duration::from_millis(800)) {
            send(std::mem::take(&mut batch));
            last = std::time::Instant::now();
        }
    }
    if !batch.is_empty() {
        send(batch);
    }
    if changed {
        let _ = tokio::task::spawn_blocking(save).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(t: &[&str]) -> Vec<String> {
        t.iter().map(|s| s.to_string()).collect()
    }
    fn cat(t: &[&str], name: &str, id: &str) -> &'static str {
        CATEGORIES[category_of(&tags(t), name, id)]
    }

    #[test]
    fn tags_map_to_categories() {
        assert_eq!(cat(&["browser", "web", "gecko", "privacy", "浏览器"], "Mozilla Firefox", "Mozilla.Firefox"), "Browsers");
        assert_eq!(cat(&["chat", "messaging", "voip"], "Signal", "OpenWhisperSystems.Signal"), "Communication");
        assert_eq!(cat(&["video", "player", "media", "codec"], "VLC media player", "VideoLAN.VLC"), "Media");
        assert_eq!(cat(&["3d", "modeling", "animation", "rendering"], "Some 3D Tool", "X.Y"), "Graphics and 3D");
        assert_eq!(cat(&["git", "developer-tools", "cli"], "Git", "Git.Git"), "Development");
        assert_eq!(cat(&["game", "launcher"], "Some Launcher", "X.Y"), "Games");
        assert_eq!(cat(&["archive", "compression", "zip"], "7-Zip", "7zip.7zip"), "Utilities");
        assert_eq!(cat(&["pdf", "reader"], "Sumatra PDF", "SumatraPDF.SumatraPDF"), "Office");
        assert_eq!(cat(&["password-manager", "encryption"], "Bitwarden", "Bitwarden.Bitwarden"), "Security");
        assert_eq!(cat(&["vpn", "network"], "WireGuard", "WireGuard.WireGuard"), "Networking");
        assert_eq!(cat(&["driver", "hardware"], "Some Driver", "X.Y"), "System");
        // Case, spaces and underscores in tags do not matter.
        assert_eq!(cat(&["Web Browser"], "X", "X.Y"), "Browsers");
        assert_eq!(cat(&["Password_Manager"], "X", "X.Y"), "Security");
        // A repeated tag is one vote: two Media tags beat "browser" written twice.
        assert_eq!(cat(&["browser", "Browser", "audio", "music"], "X", "X.Y"), "Media");
        // Equal votes: the priority order decides (a browser tagged "privacy" is a browser).
        assert_eq!(cat(&["privacy", "browser"], "X", "X.Y"), "Browsers");
        assert_eq!(cat(&["tool", "game"], "X", "X.Y"), "Games");
    }

    #[test]
    fn untagged_packages_use_their_name_else_other() {
        assert_eq!(cat(&[], "Waterfox Browser", "Waterfox.Waterfox"), "Browsers");
        assert_eq!(cat(&[], "Notepad Pro", "Acme.Firefox"), "Browsers");
        assert_eq!(cat(&["unknown-tag", "windows"], "Zebra", "Acme.Zebra"), "Other");
        assert_eq!(cat(&[], "", ""), "Other");
        // Tags win over the name.
        assert_eq!(cat(&["game"], "Chat Browser", "X.Y"), "Games");
    }

    #[test]
    fn every_category_but_other_has_words_and_a_place_in_the_priority() {
        for c in 0..OTHER {
            assert!(WORDS.iter().any(|(k, _)| *k == c), "{}", CATEGORIES[c]);
            assert!(PRIORITY.contains(&c), "{}", CATEGORIES[c]);
        }
        assert_eq!(PRIORITY.len(), OTHER);
        // A word in two categories would make the result depend on table order.
        let mut seen = HashMap::new();
        for (c, ws) in WORDS {
            for w in *ws {
                assert_eq!(*w, norm_tag(w), "{w} is not written as tags are compared");
                if let Some(o) = seen.insert(*w, *c) {
                    panic!("{w} is in {} and {}", CATEGORIES[o], CATEGORIES[*c]);
                }
            }
        }
    }

    #[test]
    fn letters() {
        assert_eq!(letter_of("blender"), 'B');
        assert_eq!(letter_of("\"Quoted\" App"), 'Q');
        assert_eq!(letter_of("7-Zip"), '#');
        assert_eq!(letter_of("Émile"), '#');
        assert_eq!(letter_of(""), '#');
    }

    fn test_list() -> BrowseList {
        let idx = winget::test_index();
        for (id, name, t) in [
            ("Mozilla.Firefox", "Mozilla Firefox", &["browser"][..]),
            ("Google.Chrome", "Google Chrome", &["browser"][..]),
            ("7zip.7zip", "7-Zip", &["archive"][..]),
            ("Acme.Alpha", "alpha", &[][..]),
            ("Acme.Beta", "Beta", &[][..]),
            ("BlenderFoundation.Blender", "Blender", &["3d"][..]),
            ("BlenderFoundation.Blender.LTS.4.5", "Blender 4.5 LTS", &["3d"][..]),
        ] {
            let row = idx.add_test_package(id, name, "1.0", &[], &[], &[], "Acme");
            idx.add_test_tags(row, t);
        }
        build(&idx)
    }

    #[test]
    fn the_list_is_sorted_by_name_without_handled_apps() {
        let l = test_list();
        assert_eq!(l.packages, 7);
        let names: Vec<&str> = l.items.iter().map(|i| i.pkg.name.as_str()).collect();
        assert_eq!(names, ["7-Zip", "alpha", "Beta", "Google Chrome", "Mozilla Firefox"]);
        assert_eq!(counts(&l, |_| true)[0], 2);
        assert_eq!(counts(&l, |_| true)[OTHER], 2);
    }

    #[test]
    fn pages() {
        let l = test_list();
        let ids = |v: Vec<&Item>| v.iter().map(|i| i.pkg.id.clone()).collect::<Vec<_>>();
        let (n, p) = page(&l, None, None, 0, 2, |_| true);
        assert_eq!((n, ids(p)), (5, vec!["7zip.7zip".to_string(), "Acme.Alpha".into()]));
        let (n, p) = page(&l, None, None, 2, 2, |_| true);
        assert_eq!((n, ids(p)), (5, vec!["Acme.Beta".to_string(), "Google.Chrome".into()]));
        let (n, p) = page(&l, None, None, 4, 2, |_| true);
        assert_eq!((n, ids(p)), (5, vec!["Mozilla.Firefox".to_string()]));
        let (n, p) = page(&l, None, None, 9, 2, |_| true);
        assert_eq!((n, p.len()), (5, 0));
        let (n, p) = page(&l, Some(0), None, 0, 60, |_| true);
        assert_eq!((n, ids(p)), (2, vec!["Google.Chrome".to_string(), "Mozilla.Firefox".into()]));
        let (n, p) = page(&l, None, Some('#'), 0, 60, |_| true);
        assert_eq!((n, ids(p)), (1, vec!["7zip.7zip".to_string()]));
        let (n, p) = page(&l, Some(OTHER), Some('A'), 0, 60, |_| true);
        assert_eq!((n, ids(p)), (1, vec!["Acme.Alpha".to_string()]));
        // Entries left out (installed apps when Get apps hides them) do not count.
        let (n, p) = page(&l, Some(0), None, 0, 60, |i| i.pkg.id != "Google.Chrome");
        assert_eq!((n, ids(p)), (1, vec!["Mozilla.Firefox".to_string()]));
    }

    #[test]
    fn card_line_falls_back_from_the_manifest_to_our_line_to_the_publisher() {
        assert_eq!(tagline_for(Some("Archiver"), Some("Ours"), Some("Pub")).as_deref(), Some("Archiver"));
        assert_eq!(tagline_for(None, Some("Ours"), Some("Pub")).as_deref(), Some("Ours"));
        assert_eq!(tagline_for(Some("  "), None, Some("Pub")).as_deref(), Some("Pub"));
        assert_eq!(tagline_for(None, None, None), None);
        assert_eq!(tagline_for(Some("Two\n  lines"), None, None).as_deref(), Some("Two lines"));
        let long = "word ".repeat(60);
        let t = tagline_for(Some(&long), None, None).unwrap();
        assert!(t.ends_with("...") && t.chars().count() <= 163, "{t}");
        assert_eq!(popular_line("google.chrome"), Some("Web browser"));
        assert_eq!(popular_line("Nobody.Nothing"), None);
    }

    #[test]
    fn details_keep_the_long_text_and_the_verdict() {
        let m = winget::parse_manifest(
            "PackageIdentifier: A.B\nPackageVersion: 1.0\nPackageName: AB\nPublisher: A Ltd\nShortDescription: Short\nDescription: Longer text.\nInstallers:\n- Architecture: x64\n  InstallerType: msix\n  InstallerUrl: https://a.example/x.msix\n  InstallerSha256: 0000000000000000000000000000000000000000000000000000000000000000\n",
        )
        .unwrap();
        let d = Detail::from_manifest("h1", &m);
        assert_eq!((d.short.as_deref(), d.long.as_deref(), d.publisher.as_deref()), (Some("Short"), Some("Longer text."), Some("A Ltd")));
        assert_eq!(d.blocked.as_deref(), Some("only a Store or MSIX package"));
        let p = patch_for("A.B", &d);
        assert_eq!((p.id.as_str(), p.tagline.as_deref()), ("other:A.B", Some("Short")));
        // A long text equal to the short one is not repeated on the page.
        let m2 = winget::parse_manifest("PackageIdentifier: A.B\nPackageVersion: 1.0\nPublisher: A Ltd\nShortDescription: Same\nDescription: Same\n").unwrap();
        let d2 = Detail::from_manifest("h", &m2);
        assert_eq!((d2.long, d2.blocked.as_deref()), (None, Some("no installer for Windows")));
    }
}
