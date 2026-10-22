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

/// Words of a text for search: lower case, split at anything but letters and digits.
fn words_of(s: &str) -> Vec<String> {
    s.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect()
}

/// Letters and digits only, lower case: "7-Zip" meets "7zip", "Visual Studio Code" meets "visualstudiocode".
fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// What a search compares, worked out once when the list is built.
#[derive(Default)]
pub struct Keys {
    name: String,
    name_c: String,
    name_words: Vec<String>,
    id: String,
    id_c: String,
    id_words: Vec<String>,
    moniker: String,
    /// The first part of the id ("Google" of "Google.Chrome"): the index keeps no usable publisher names
    publisher: Vec<String>,
    tags: Vec<String>,
}

impl Keys {
    pub fn new(name: &str, id: &str, moniker: Option<&str>, tags: &[String]) -> Keys {
        let mut tags: Vec<String> = tags.iter().flat_map(|t| words_of(t)).collect();
        tags.sort();
        tags.dedup();
        Keys {
            name: name.trim().to_lowercase(),
            name_c: compact(name),
            name_words: words_of(name),
            id: id.to_lowercase(),
            id_c: compact(id),
            id_words: words_of(id),
            moniker: moniker.unwrap_or("").trim().to_lowercase(),
            publisher: words_of(id.split('.').next().unwrap_or("")),
            tags,
        }
    }
}

/// A search as it is compared.
pub struct Query {
    raw: String,
    compact: String,
    words: Vec<String>,
}

impl Query {
    /// None for a search with nothing to look for (empty, or punctuation only).
    pub fn new(q: &str) -> Option<Query> {
        let raw = q.trim().to_lowercase();
        let words = words_of(&raw);
        (!words.is_empty()).then(|| Query { compact: compact(&raw), raw, words })
    }
}

/// How well a package matches a search, best first: 0 the exact name (or moniker or id),
/// 1 the start of the name or moniker, 2 every word starts a word of the name, 3 the name, id or
/// moniker has it, 4 every word starts a word of the name, id, publisher or tags. None: no match.
pub fn rank(k: &Keys, q: &Query) -> Option<u8> {
    let starts = |ws: &[String], w: &str| ws.iter().any(|x| x.starts_with(w));
    if k.name == q.raw || k.name_c == q.compact || k.moniker == q.raw || k.id == q.raw {
        return Some(0);
    }
    if k.name.starts_with(&q.raw) || k.name_c.starts_with(&q.compact) || (!k.moniker.is_empty() && k.moniker.starts_with(&q.raw)) {
        return Some(1);
    }
    if q.words.iter().all(|w| starts(&k.name_words, w)) {
        return Some(2);
    }
    if k.name.contains(&q.raw) || k.name_c.contains(&q.compact) || k.id.contains(&q.raw) || k.id_c.contains(&q.compact) || k.moniker.contains(&q.raw) {
        return Some(3);
    }
    if q.words.iter().all(|w| starts(&k.name_words, w) || starts(&k.id_words, w) || starts(&k.publisher, w) || starts(&k.tags, w)) {
        return Some(4);
    }
    None
}

/// Not in the starter list: sorts after every place in it.
const NOT_POPULAR: u32 = u32::MAX;

pub struct Item {
    pub pkg: Pkg,
    pub category: usize,
    pub letter: char,
    /// Place in the starter list (`other::POPULAR`), or `NOT_POPULAR`
    pub popular: u32,
    pub keys: Keys,
}

impl Item {
    pub fn is_popular(&self) -> bool {
        self.popular != NOT_POPULAR
    }
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
    let popular: HashMap<String, u32> = other::POPULAR.iter().enumerate().map(|(i, (id, _))| (id.to_ascii_lowercase(), i as u32)).collect();
    let mut items: Vec<Item> = idx
        .all()
        .into_iter()
        .filter(|p| !other::handled_package(&p.id) && other::excluded_package(&p.id).is_none())
        .map(|p| {
            let t = tags.get(&p.row).unwrap_or(&none);
            let category = category_of(t, &p.name, &p.id);
            let letter = letter_of(&p.name);
            let keys = Keys::new(&p.name, &p.id, p.moniker.as_deref(), t);
            let popular = popular.get(&p.id.to_ascii_lowercase()).copied().unwrap_or(NOT_POPULAR);
            Item { pkg: p, category, letter, popular, keys }
        })
        .collect();
    items.sort_by_cached_key(|i| (i.pkg.name.to_lowercase(), i.pkg.id.to_lowercase()));
    BrowseList { items, packages: idx.count() }
}

/// The orders Get apps offers for Other apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sort {
    /// The starter list in its order, then A to Z; with a search, the best matches first
    Popular,
    NameAsc,
    NameDesc,
    /// Newest release first, as far as the manifests read so far tell; the rest after, A to Z
    Updated,
}

impl Sort {
    pub fn parse(s: Option<&str>) -> Sort {
        match s.unwrap_or("") {
            "az" => Sort::NameAsc,
            "za" => Sort::NameDesc,
            "updated" => Sort::Updated,
            _ => Sort::Popular,
        }
    }
}

/// What to list: a category, a first letter and a search, in an order.
pub struct Want<'a> {
    pub category: Option<usize>,
    pub letter: Option<char>,
    pub query: Option<&'a Query>,
    pub sort: Sort,
}

/// One page of a selection and what the filters around it show.
pub struct Selection<'a> {
    /// Entries of the whole selection
    pub total: usize,
    pub items: Vec<&'a Item>,
    /// Entries of each category for the same search and letter, in `CATEGORIES` order
    pub counts: Vec<usize>,
    /// Entries of the whole selection with a known release date
    pub dated: usize,
}

/// Entries `offset` on of what `want` asks for, leaving out those `keep` says no to. `date`
/// gives the newest release date known for a package ("2026-10-01"), for `Sort::Updated`.
pub fn select<'a, 'd>(list: &'a BrowseList, want: &Want, offset: usize, limit: usize, keep: impl Fn(&Item) -> bool, date: impl Fn(&Item) -> Option<&'d str>) -> Selection<'a> {
    let mut counts = vec![0; CATEGORIES.len()];
    let mut hits: Vec<(u8, &Item)> = Vec::new();
    for i in &list.items {
        if want.letter.is_some_and(|l| i.letter != l) || !keep(i) {
            continue;
        }
        let r = match want.query {
            Some(q) => match rank(&i.keys, q) {
                Some(r) => r,
                None => continue,
            },
            None => 0,
        };
        counts[i.category] += 1;
        if want.category.is_none_or(|c| i.category == c) {
            hits.push((r, i));
        }
    }
    match want.sort {
        // A starter list app moves up one step: "chrome" is Google Chrome before Chrome Remote Desktop.
        Sort::Popular if want.query.is_some() => hits.sort_by_key(|(r, i)| (if i.is_popular() { r.saturating_sub(1) } else { *r }, i.popular, i.keys.name.len())),
        Sort::Popular => hits.sort_by_key(|(_, i)| i.popular),
        Sort::NameAsc => {}
        Sort::NameDesc => hits.reverse(),
        // Newest first, undated last; equal dates stay A to Z.
        Sort::Updated => hits.sort_by_key(|(_, i)| std::cmp::Reverse(date(i))),
    }
    let dated = hits.iter().filter(|(_, i)| date(i).is_some()).count();
    let total = hits.len();
    let items = hits.into_iter().skip(offset).take(limit).map(|(_, i)| i).collect();
    Selection { total, items, counts, dated }
}

static LIST: Lazy<Mutex<Option<(std::time::SystemTime, Arc<BrowseList>)>>> = Lazy::new(|| Mutex::new(None));
/// Held while the list is built, so a page asked for during the warm up waits for that build
/// instead of starting a second one.
static BUILDING: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

/// The list for the index on disk, built again when the index file changes.
pub fn list() -> Result<Arc<BrowseList>> {
    let path = winget::index_path();
    let stamp = std::fs::metadata(&path)?.modified()?;
    let cached = || LIST.lock().as_ref().filter(|(t, _)| *t == stamp).map(|(_, l)| l.clone());
    if let Some(l) = cached() {
        return Ok(l);
    }
    let _building = BUILDING.lock();
    if let Some(l) = cached() {
        return Ok(l);
    }
    let l = Arc::new(build(&Index::open(&path)?));
    *LIST.lock() = Some((stamp, l.clone()));
    Ok(l)
}

/// Release dates known now, by lowercase package id: one lock for a whole page.
pub fn dates() -> HashMap<String, String> {
    DETAILS.lock().iter().filter_map(|(k, d)| d.date.clone().map(|x| (k.clone(), x))).collect()
}

/// What Get apps asks for: one page of Other apps.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Request {
    pub query: Option<String>,
    pub category: Option<String>,
    pub letter: Option<String>,
    /// "popular" (the default), "az", "za" or "updated"
    pub sort: Option<String>,
    pub offset: usize,
    pub limit: usize,
}

/// The page `req` asks for, with `here` the lowercase ids of the installed packages (listed only
/// when `show_installed`), and the packages on it.
pub fn page_for(list: &BrowseList, req: &Request, here: &HashSet<String>, show_installed: bool) -> (BrowsePage, Vec<Pkg>) {
    let query = req.query.as_deref().and_then(Query::new);
    let want = Want {
        category: req.category.as_deref().and_then(category_index),
        letter: req.letter.as_deref().and_then(|l| l.chars().next()).map(|c| c.to_ascii_uppercase()),
        query: query.as_ref(),
        sort: Sort::parse(req.sort.as_deref()),
    };
    let dates = dates();
    let sel = select(list, &want, req.offset, req.limit.clamp(1, 200), |i| show_installed || !here.contains(&i.keys.id), |i| dates.get(&i.keys.id).map(String::as_str));
    let pkgs: Vec<Pkg> = sel.items.iter().map(|i| i.pkg.clone()).collect();
    let entries = sel
        .items
        .iter()
        .map(|i| {
            let mut e = other::entry(&i.pkg, i.pkg.latest.clone(), Some(i.pkg.id.clone()), here.contains(&i.keys.id));
            apply(&mut e);
            e
        })
        .collect();
    let categories = CATEGORIES.iter().zip(sel.counts).map(|(name, count)| CategoryCount { name, count }).collect();
    (BrowsePage { total: sel.total, entries, packages: list.packages, categories, dated: sel.dated }, pkgs)
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
    /// Entries of this selection with a known release date: "Recently updated" is only as good as these
    pub dated: usize,
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
    /// Release date of the newest version ("2026-10-01"), when its manifest gives one
    #[serde(default)]
    pub date: Option<String>,
    /// The date above was looked for (details kept before dates were read lack it)
    #[serde(default)]
    pub dated: bool,
}

impl Detail {
    pub fn from_manifest(hash: &str, m: &winget::Manifest) -> Detail {
        Detail {
            hash: hash.to_string(),
            short: m.description.clone(),
            long: m.long_description.as_deref().map(|d| cut(d.trim(), 2000)).filter(|d| !d.is_empty() && Some(d.as_str()) != m.description.as_deref()),
            publisher: Some(m.publisher.clone()).filter(|p| !p.is_empty()),
            blocked: other::choose(m, &other::Want::fresh()).err(),
            date: day_of(m.release_date.as_deref()),
            dated: true,
        }
    }
}

/// "2026-10-01" of a manifest's ReleaseDate ("2026-10-01" or "2026-10-01T12:00:00Z"); None for anything else.
fn day_of(d: Option<&str>) -> Option<String> {
    let day = d?.trim().get(..10)?;
    let ok = day.bytes().enumerate().all(|(i, c)| if i == 4 || i == 7 { c == b'-' } else { c.is_ascii_digit() });
    (ok && !day.starts_with('0')).then(|| day.to_string())
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
            d.get(&k).is_none_or(|x| x.hash != p.hash || !x.dated) && !busy.contains(&k) && !failed.contains(&k)
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

    /// A to Z without a search, as Browse all listed before there were orders.
    fn page<'a>(l: &'a BrowseList, category: Option<usize>, letter: Option<char>, offset: usize, limit: usize, keep: impl Fn(&Item) -> bool) -> (usize, Vec<&'a Item>) {
        let s = select(l, &Want { category, letter, query: None, sort: Sort::NameAsc }, offset, limit, keep, |_| None);
        (s.total, s.items)
    }
    fn counts(l: &BrowseList, keep: impl Fn(&Item) -> bool) -> Vec<usize> {
        select(l, &Want { category: None, letter: None, query: None, sort: Sort::NameAsc }, 0, 1, keep, |_| None).counts
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

    fn keys(name: &str, id: &str, moniker: Option<&str>, t: &[&str]) -> Keys {
        Keys::new(name, id, moniker, &tags(t))
    }
    fn r(k: &Keys, q: &str) -> Option<u8> {
        rank(k, &Query::new(q).unwrap())
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_words_then_contains_then_publisher_and_tags() {
        let vlc = keys("VLC media player", "VideoLAN.VLC", Some("vlc"), &["video", "player"]);
        assert_eq!(r(&vlc, "VLC Media Player"), Some(0));
        assert_eq!(r(&vlc, "vlc"), Some(0), "moniker");
        assert_eq!(r(&vlc, "videolan.vlc"), Some(0), "package id");
        assert_eq!(r(&vlc, "vlc med"), Some(1));
        assert_eq!(r(&vlc, "media"), Some(2));
        assert_eq!(r(&vlc, "pla med"), Some(2), "every word starts a word, any order");
        assert_eq!(r(&vlc, "dia yer"), None, "word middles do not count");
        assert_eq!(r(&vlc, "edia"), Some(3), "inside the name");
        assert_eq!(r(&vlc, "videolan"), Some(3), "inside the id");
        assert_eq!(r(&vlc, "videolan video"), Some(4), "publisher and tag");
        assert_eq!(r(&vlc, "chrome"), None);
        let code = keys("Microsoft Visual Studio Code", "Microsoft.VisualStudioCode", Some("vscode"), &[]);
        assert_eq!(r(&code, "vscode"), Some(0));
        assert_eq!(r(&code, "visual studio code"), Some(2));
        assert_eq!(r(&code, "visualstudiocode"), Some(3), "spaces left out still meet the id");
        // "7zip" is 7-Zip: punctuation does not count.
        assert_eq!(r(&keys("7-Zip", "7zip.7zip", None, &[]), "7zip"), Some(0));
        assert_eq!(r(&keys("Notepad++", "Notepad++.Notepad++", Some("notepad++"), &[]), "notepad"), Some(0), "symbols do not count either");
        assert_eq!(r(&keys("Google Chrome", "Google.Chrome", None, &["browser"]), "chrome"), Some(2));
        assert_eq!(r(&keys("Google Chrome", "Google.Chrome", None, &["web-browser"]), "google browser"), Some(4), "tags split like names");
        assert!(Query::new(" -- ").is_none() && Query::new("").is_none());
    }

    fn item(id: &str, name: &str, moniker: Option<&str>, popular: u32, category: usize) -> Item {
        let pkg = Pkg { row: 0, id: id.into(), name: name.into(), moniker: moniker.map(String::from), latest: "1.0".into(), arp_min: None, arp_max: None, hash: String::new() };
        Item { keys: Keys::new(name, id, moniker, &[]), letter: letter_of(name), pkg, category, popular }
    }
    fn ranked_list() -> BrowseList {
        let mut items = vec![
            item("Google.Chrome", "Google Chrome", Some("chrome"), 0, 0),
            item("Google.Chrome.Dev", "Google Chrome Dev", None, NOT_POPULAR, 0),
            item("Google.ChromeRemoteDesktopHost", "Chrome Remote Desktop Host", None, NOT_POPULAR, 9),
            item("Acme.ChromeCleaner", "Chrome Cleaner", None, NOT_POPULAR, 6),
            item("Hibbiki.Chromium", "Chromium", None, 5, 0),
            item("Mozilla.Firefox", "Mozilla Firefox", Some("firefox"), 1, 0),
            item("Microsoft.VisualStudioCode", "Microsoft Visual Studio Code", Some("vscode"), 3, 4),
            item("Microsoft.VisualStudioCode.Insiders", "Microsoft Visual Studio Code Insiders", None, 4, 4),
            item("Acme.VisualStudioCodeThemes", "Visual Studio Code Themes", None, NOT_POPULAR, 4),
            item("Zed.Zed", "Zed", None, NOT_POPULAR, 4),
        ];
        items.sort_by_cached_key(|i| i.pkg.name.to_lowercase());
        BrowseList { items, packages: 10 }
    }
    fn ids_for(l: &BrowseList, q: Option<&str>, sort: Sort, date: impl Fn(&Item) -> Option<&'static str>) -> Vec<String> {
        let query = q.and_then(Query::new);
        select(l, &Want { category: None, letter: None, query: query.as_ref(), sort }, 0, 50, |_| true, date).items.iter().map(|i| i.pkg.id.clone()).collect()
    }

    #[test]
    fn search_results_put_well_known_apps_first_among_close_matches() {
        let l = ranked_list();
        let got = ids_for(&l, Some("chrome"), Sort::Popular, |_| None);
        // Google Chrome is an exact moniker; the starter list's Chromium does not match "chrome"
        // at all; name starts beat word starts.
        assert_eq!(got, ["Google.Chrome", "Acme.ChromeCleaner", "Google.ChromeRemoteDesktopHost", "Google.Chrome.Dev"]);
        let got = ids_for(&l, Some("visual studio code"), Sort::Popular, |_| None);
        // Starter list apps move up one step: ahead of a name that starts with the words.
        assert_eq!(got, ["Microsoft.VisualStudioCode", "Microsoft.VisualStudioCode.Insiders", "Acme.VisualStudioCodeThemes"]);
        assert_eq!(ids_for(&l, Some("vscode"), Sort::Popular, |_| None)[0], "Microsoft.VisualStudioCode");
        // Other orders sort the matches, not the whole list.
        assert_eq!(ids_for(&l, Some("chrome"), Sort::NameAsc, |_| None), ["Acme.ChromeCleaner", "Google.ChromeRemoteDesktopHost", "Google.Chrome", "Google.Chrome.Dev"]);
        assert_eq!(ids_for(&l, Some("nothing like it"), Sort::Popular, |_| None), Vec::<String>::new());
    }

    #[test]
    fn orders_without_a_search() {
        let l = ranked_list();
        let all = ids_for(&l, None, Sort::Popular, |_| None);
        assert_eq!(&all[..5], ["Google.Chrome", "Mozilla.Firefox", "Microsoft.VisualStudioCode", "Microsoft.VisualStudioCode.Insiders", "Hibbiki.Chromium"]);
        assert_eq!(&all[5..], ["Acme.ChromeCleaner", "Google.ChromeRemoteDesktopHost", "Google.Chrome.Dev", "Acme.VisualStudioCodeThemes", "Zed.Zed"], "the rest A to Z");
        let az = ids_for(&l, None, Sort::NameAsc, |_| None);
        let mut za = ids_for(&l, None, Sort::NameDesc, |_| None);
        za.reverse();
        assert_eq!(az, za);
        assert_eq!(az[0], "Acme.ChromeCleaner");
        // Newest first; undated after, A to Z.
        let date = |i: &Item| match i.pkg.id.as_str() {
            "Zed.Zed" => Some("2026-10-20"),
            "Mozilla.Firefox" => Some("2026-09-01"),
            "Google.Chrome" => Some("2026-10-01"),
            _ => None,
        };
        let up = ids_for(&l, None, Sort::Updated, date);
        assert_eq!(&up[..4], ["Zed.Zed", "Google.Chrome", "Mozilla.Firefox", "Acme.ChromeCleaner"]);
        let s = select(&l, &Want { category: None, letter: None, query: None, sort: Sort::Updated }, 0, 2, |_| true, date);
        assert_eq!((s.total, s.items.len(), s.dated), (10, 2, 3));
    }

    #[test]
    fn category_counts_follow_the_search() {
        let l = ranked_list();
        let q = Query::new("chrome").unwrap();
        let s = select(&l, &Want { category: Some(9), letter: None, query: Some(&q), sort: Sort::Popular }, 0, 50, |_| true, |_| None);
        assert_eq!(s.total, 1);
        assert_eq!((s.counts[0], s.counts[6], s.counts[9], s.counts[4]), (2, 1, 1, 0));
    }

    #[test]
    fn release_days() {
        assert_eq!(day_of(Some("2026-10-01")).as_deref(), Some("2026-10-01"));
        assert_eq!(day_of(Some(" 2026-10-01T08:00:00Z")).as_deref(), Some("2026-10-01"));
        assert_eq!(day_of(Some("10/01/2026")), None);
        assert_eq!(day_of(Some("2026-1-1")), None);
        assert_eq!(day_of(None), None);
    }

    #[test]
    fn details_kept_before_dates_are_read_again() {
        let p = Pkg { row: 0, id: "Old.Detail".into(), name: "Old".into(), moniker: None, latest: "1".into(), arp_min: None, arp_max: None, hash: "h".into() };
        let old: Detail = serde_json::from_str(r#"{"hash":"h","short":null,"long":null,"publisher":null,"blocked":null}"#).unwrap();
        assert!(!old.dated && old.date.is_none());
        DETAILS.lock().insert("old.detail".into(), old);
        assert_eq!(stale(std::slice::from_ref(&p)).len(), 1);
        DETAILS.lock().insert("old.detail".into(), Detail { hash: "h".into(), dated: true, ..Default::default() });
        assert!(stale(&[p]).is_empty());
        DETAILS.lock().remove("old.detail");
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
