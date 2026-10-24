//! Real icons for the Other apps catalog. The winget manifest names an icon for few apps, so the
//! rest come from the app's own web page: its touch icon, its icon links, its web app manifest,
//! the GitHub avatar of the project owner, and last the plain favicon. The web is read with our
//! own User-Agent and short limits; a site that refuses us is a miss.

use futures_util::StreamExt;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use regex::Regex;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Semaphore;
use url::Url;

/// Icons that are poor on the app's page (or missing there), by lowercase winget id.
const OVERRIDES: &[(&str, &str)] = &[
    ("Mojang.MinecraftLauncher", "https://www.minecraft.net/favicon.ico"),
    ("Autodesk.Fusion", "https://www.autodesk.com/favicon.ico"),
    ("Adobe.Acrobat.Reader.64-bit", "https://www.adobe.com/favicon.ico"),
    ("LyX.LyX", "https://www.lyx.org/favicon.ico"),
    // no IconUrl, and epicgames.com refuses us; the launcher's own site does not
    ("EpicGames.EpicGamesLauncher", "https://launcher.epicgames.com/apple-touch-icon.png"),
    // no IconUrl and no pages in the manifest, and webex.com refuses us: Webex's own GitHub account
    ("Cisco.Webex", "https://github.com/webex.png?size=128"),
    ("Cisco.CiscoWebexMeetings", "https://github.com/webex.png?size=128"),
];

/// Biggest page we read.
const MAX_PAGE: usize = 1024 * 1024;
/// Biggest picture we read; it is made small once, when it is saved.
const MAX_IMAGE: usize = 4 * 1024 * 1024;
/// One request from start to end. A site slower than this costs one card its icon for now, and it
/// is tried again later (see `retry_later`).
const TIMEOUT: Duration = Duration::from_secs(5);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Icon candidates tried per app after the page was read.
const MAX_TRIES: usize = 5;
/// Candidates fetched ahead while the better one before them is still loading.
const AHEAD: usize = 3;
/// Apps whose icon is searched for at the same time: as many as this PC is tuned for (16 on a
/// usual PC); each server still gets at most `PER_HOST`. No more than that: a search that started
/// early waits ahead of one that started later, so more of them would make the cards on screen
/// wait behind the ones around them.
pub fn parallel() -> usize {
    crate::machine::tuned().icon_parallel
}
/// Requests to one server at the same time (a browser opens 6), so a page of apps by one
/// publisher does not flood it.
const PER_HOST: usize = 6;
/// Edge of a saved icon in pixels: sharp at the biggest mark (80 px) on a 150% screen.
pub const EDGE: u32 = 128;
/// An app whose search failed for a passing reason (no answer in time, a busy server) is searched
/// again after this long, not remembered as having no icon.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// Our own client for icons: quick to give up on a server that does not answer.
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("TheUpdater3D/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(CONNECT_TIMEOUT)
        .gzip(true)
        .brotli(true)
        .pool_max_idle_per_host(PER_HOST)
        .build()
        .expect("http client")
});
static HOSTS: Lazy<Mutex<HashMap<String, Arc<Semaphore>>>> = Lazy::new(Default::default);
static RETRY: Lazy<Mutex<HashMap<String, Instant>>> = Lazy::new(Default::default);

/// Remember that the search for this app failed for a passing reason.
pub fn retry_later(id: &str) {
    RETRY.lock().insert(id.to_ascii_lowercase(), Instant::now() + RETRY_AFTER);
}

/// False while an app's last search failed for a passing reason only a moment ago.
pub fn may_search(id: &str) -> bool {
    let mut r = RETRY.lock();
    let k = id.to_ascii_lowercase();
    match r.get(&k) {
        Some(t) if *t > Instant::now() => false,
        Some(_) => {
            r.remove(&k);
            true
        }
        None => true,
    }
}

/// How soon an app's icon is wanted, most urgent first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    /// The app whose page the user opened.
    Pinned,
    /// The cards on screen.
    View,
    /// The rows kept just above and below the screen.
    Near,
    /// The starter list and installed apps, searched while nothing else waits.
    Rest,
}

/// Apps waiting for their icon search, the most urgent first. What the window shows is told with
/// `view`: the cards on screen go first, the rows around them next, and apps that were waiting
/// for a place that scrolled away are dropped (they are asked for again if the list comes back).
#[derive(Default)]
pub struct Queue {
    tiers: [VecDeque<String>; 4],
    /// Lowercase id -> the tier it waits in
    at: HashMap<String, Tier>,
}

impl Queue {
    fn lane(&mut self, t: Tier) -> &mut VecDeque<String> {
        &mut self.tiers[t as usize]
    }
    fn take_out(&mut self, key: &str) {
        if let Some(t) = self.at.remove(key) {
            self.lane(t).retain(|i| !i.eq_ignore_ascii_case(key));
        }
    }
    /// Put these ids in the front of a tier, in their order; ids waiting in a less urgent tier or
    /// further back in this one move up, ids in a more urgent tier stay where they are.
    pub fn front(&mut self, tier: Tier, ids: &[String]) {
        let mut seen = HashSet::new();
        let mut fresh: Vec<&String> = Vec::new();
        for id in ids {
            let k = id.to_ascii_lowercase();
            if self.at.get(&k).is_some_and(|t| *t < tier) || !seen.insert(k) {
                continue;
            }
            fresh.push(id);
        }
        for id in &fresh {
            self.take_out(&id.to_ascii_lowercase());
        }
        for id in fresh.into_iter().rev() {
            self.at.insert(id.to_ascii_lowercase(), tier);
            self.lane(tier).push_front(id.clone());
        }
    }
    /// Add these ids after everything waiting in the `Rest` tier; ids already waiting keep their place.
    pub fn back(&mut self, ids: &[String]) {
        for id in ids {
            let k = id.to_ascii_lowercase();
            if !self.at.contains_key(&k) {
                self.at.insert(k, Tier::Rest);
                self.lane(Tier::Rest).push_back(id.clone());
            }
        }
    }
    /// The window shows `visible` cards and keeps `near` ones around them: they wait in the
    /// `View` and `Near` tiers in that order, and what waited in those tiers for other places is
    /// dropped. Returns how many waiting apps were dropped.
    pub fn view(&mut self, visible: &[String], near: &[String]) -> usize {
        let keep: HashSet<String> = visible.iter().chain(near).map(|i| i.to_ascii_lowercase()).collect();
        let mut dropped = 0;
        for t in [Tier::View, Tier::Near] {
            for id in std::mem::take(self.lane(t)) {
                let k = id.to_ascii_lowercase();
                // the ones still wanted are put back below, in their new places
                dropped += !keep.contains(&k) as usize;
                self.at.remove(&k);
            }
        }
        self.front(Tier::Near, near);
        self.front(Tier::View, visible);
        dropped
    }
    pub fn pop(&mut self) -> Option<String> {
        for t in [Tier::Pinned, Tier::View, Tier::Near, Tier::Rest] {
            if let Some(id) = self.lane(t).pop_front() {
                self.at.remove(&id.to_ascii_lowercase());
                return Some(id);
            }
        }
        None
    }
    pub fn len(&self) -> usize {
        self.at.len()
    }
    pub fn is_empty(&self) -> bool {
        self.at.is_empty()
    }
    /// Apps waiting in a tier.
    pub fn waiting(&self, t: Tier) -> usize {
        self.tiers[t as usize].len()
    }
}

static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)<link\s[^>]*>").unwrap());
static ATTR: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?is)([a-zA-Z][a-zA-Z0-9:_-]*)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+))"#).unwrap());

/// An icon an HTML page names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub url: String,
    /// Edge length in pixels; 0 when the page does not say
    pub size: u32,
    pub vector: bool,
}

fn attrs(tag: &str) -> Vec<(String, String)> {
    ATTR.captures_iter(tag)
        .map(|c| {
            let v = c.get(2).or(c.get(3)).or(c.get(4)).map(|m| m.as_str()).unwrap_or("");
            (c[1].to_ascii_lowercase(), v.trim().to_string())
        })
        .collect()
}

/// Biggest edge of a `sizes` value such as "32x32 180x180" (0 for "any" or nothing).
fn parse_sizes(s: &str) -> u32 {
    let mut best = 0;
    for part in s.split_whitespace() {
        if let Some((w, h)) = part.to_ascii_lowercase().split_once('x').map(|(w, h)| (w.to_string(), h.to_string())) {
            if let (Ok(w), Ok(h)) = (w.parse::<u32>(), h.parse::<u32>()) {
                best = best.max(w.min(h));
            }
        }
    }
    best
}

fn is_vector(href: &str, mime: &str) -> bool {
    mime.contains("svg") || href.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase().ends_with(".svg")
}

fn join(base: &Url, href: &str) -> Option<String> {
    let href = href.trim();
    if href.is_empty() || href.starts_with("data:") || href.starts_with("javascript:") {
        return None;
    }
    let u = base.join(href).ok()?;
    matches!(u.scheme(), "http" | "https").then(|| u.to_string())
}

/// Higher is better: a sharp icon of at least 64 px first (a vector counts as big), the touch icon
/// when the size is unknown, small ones and `.ico` files last.
fn rank(f: &Found) -> (u8, u32) {
    let ico = f.url.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase().ends_with(".ico");
    let size = if f.vector { 512 } else { f.size };
    let tier = if ico {
        0
    } else if size >= 64 {
        3
    } else if size == 0 {
        2
    } else {
        1
    };
    // Among sharp ones prefer around 128 to 256 over huge ones, which scale down fine but cost more.
    (tier, if size >= 64 { 1024 - size.abs_diff(192).min(1000) } else { size })
}

/// The icons an HTML page names with `<link>`, best first. Relative addresses are resolved against
/// `page_url`. Also returns the address of its web app manifest when it has one.
pub fn icons_from_html(html: &str, page_url: &str) -> (Vec<Found>, Option<String>) {
    let Ok(base) = Url::parse(page_url) else { return (vec![], None) };
    let mut out: Vec<Found> = Vec::new();
    let mut manifest = None;
    for tag in LINK.find_iter(html) {
        let a = attrs(tag.as_str());
        let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("");
        let rel = get("rel").to_ascii_lowercase();
        let rels: Vec<&str> = rel.split_whitespace().collect();
        if rels.contains(&"manifest") {
            manifest = manifest.or_else(|| join(&base, get("href")));
            continue;
        }
        let touch = rels.iter().any(|r| r.starts_with("apple-touch-icon"));
        if !touch && !rels.contains(&"icon") {
            continue;
        }
        let Some(url) = join(&base, get("href")) else { continue };
        let size = parse_sizes(get("sizes"));
        let vector = is_vector(get("href"), get("type"));
        // A touch icon without a size is 180 px by convention.
        let size = if size == 0 && touch { 180 } else { size };
        if !out.iter().any(|f| f.url == url) {
            out.push(Found { url, size, vector });
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(rank(f)));
    (out, manifest)
}

/// Icons listed in a web app manifest (JSON), best first.
pub fn icons_from_web_manifest(json: &str, manifest_url: &str) -> Vec<Found> {
    let Ok(base) = Url::parse(manifest_url) else { return vec![] };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let mut out: Vec<Found> = Vec::new();
    for i in v["icons"].as_array().into_iter().flatten() {
        let Some(src) = i["src"].as_str() else { continue };
        let Some(url) = join(&base, src) else { continue };
        // Maskable-only icons are cropped to a shape, so they make poor logos.
        if i["purpose"].as_str().is_some_and(|p| p.split_whitespace().all(|w| w == "maskable")) {
            continue;
        }
        let size = parse_sizes(i["sizes"].as_str().unwrap_or(""));
        let vector = is_vector(src, i["type"].as_str().unwrap_or(""));
        if !out.iter().any(|f| f.url == url) {
            out.push(Found { url, size, vector });
        }
    }
    out.sort_by_key(|f| std::cmp::Reverse(rank(f)));
    out
}

/// The picture of the account behind a github.com address (an owner's page or one of its repos).
pub fn github_avatar(page_url: &str) -> Option<String> {
    let u = Url::parse(page_url).ok()?;
    let host = u.host_str()?.to_ascii_lowercase();
    if host != "github.com" && host != "www.github.com" {
        return None;
    }
    let owner = u.path_segments()?.next().filter(|o| !o.is_empty())?;
    // Site pages that are not an account
    if matches!(owner.to_ascii_lowercase().as_str(), "orgs" | "settings" | "marketplace" | "sponsors" | "topics" | "about" | "features" | "login" | "search") {
        return None;
    }
    if !owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return None;
    }
    Some(format!("https://github.com/{owner}.png?size=128"))
}

/// What kind of image these bytes are (the file extension), or None when they are not one.
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("gif")
    } else if b.len() > 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some("webp")
    } else if b.starts_with(&[0, 0, 1, 0]) && b.len() > 22 {
        Some("ico")
    } else {
        // An SVG is XML text that has an <svg element early on; an HTML page does not.
        let head = String::from_utf8_lossy(&b[..b.len().min(2048)]).to_ascii_lowercase();
        let head = head.trim_start_matches('\u{feff}').trim_start();
        let xmlish = head.starts_with("<?xml") || head.starts_with("<svg") || head.starts_with("<!--") || head.starts_with("<!doctype svg");
        (xmlish && head.contains("<svg") && !head.contains("<html")).then_some("svg")
    }
}

/// A file name safe on Windows for a package id.
pub fn file_stem(id: &str) -> String {
    let s: String = id.to_ascii_lowercase().chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' }).collect();
    format!("other-{s}")
}

/// What one request came to.
enum Fetched {
    Body(Vec<u8>),
    /// The server answered and has nothing for us: not found, too big.
    Gone,
    /// The server refuses us (401, 403, 451): the site is skipped, and it would refuse again.
    Refused,
    /// No answer in time, no connection, or a busy server (429, 5xx): worth trying again later.
    Failed,
}

fn gate(url: &str) -> Option<Arc<Semaphore>> {
    let host = Url::parse(url).ok()?.host_str()?.to_ascii_lowercase();
    Some(HOSTS.lock().entry(host).or_insert_with(|| Arc::new(Semaphore::new(PER_HOST))).clone())
}

/// What a status that is not a success means for us.
fn failure(s: reqwest::StatusCode) -> Fetched {
    match s.as_u16() {
        401 | 403 | 451 => Fetched::Refused,
        408 | 425 | 429 => Fetched::Failed,
        _ if s.is_server_error() => Fetched::Failed,
        _ => Fetched::Gone,
    }
}

/// A fetched body of at most `max` bytes, with at most `PER_HOST` requests to one server at once.
async fn get(url: &str, max: usize) -> Fetched {
    let Some(g) = gate(url) else { return Fetched::Gone };
    let Ok(_permit) = g.acquire_owned().await else { return Fetched::Failed };
    let mut resp = match CLIENT.get(url).timeout(TIMEOUT).send().await {
        Ok(r) => r,
        Err(_) => return Fetched::Failed,
    };
    let status = resp.status();
    if !status.is_success() {
        return failure(status);
    }
    if resp.content_length().is_some_and(|n| n as usize > max) {
        return Fetched::Gone;
    }
    let mut body = Vec::new();
    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > max {
                    return Fetched::Gone;
                }
            }
            Ok(None) => return Fetched::Body(body),
            Err(_) => return Fetched::Failed,
        }
    }
}

/// A found icon: the picture, its file type and where it came from.
#[derive(Clone)]
pub struct Got {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub via: &'static str,
    /// Mostly dark on a clear background (a black logo): shown on a light plate in the dark theme.
    pub dark: bool,
}

/// No icon was found. `passing`: some step failed for a reason that may be gone soon (no answer in
/// time, a busy server), so the app is searched again later instead of being remembered as having
/// no icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Miss {
    pub passing: bool,
}

/// Pictures and pages already asked for in this run. Apps of one publisher name the same pages
/// and the same icon, so the first search reads them and the others, even the ones started at the
/// same moment, take its answer instead of asking the server again.
type Shared<T> = futures_util::future::Shared<futures_util::future::BoxFuture<'static, T>>;

/// The work of one memo entry runs as its own task, so a search that gives up (it took too long,
/// or its card scrolled away) never leaves half of it holding a place on a server; the others
/// waiting for the same answer still get it.
fn shared<T: Clone + Send + 'static>(work: impl std::future::Future<Output = T> + Send + 'static, lost: T) -> Shared<T> {
    let task = tokio::spawn(work);
    futures_util::FutureExt::shared(futures_util::FutureExt::boxed(async move { task.await.unwrap_or(lost) }))
}

/// What a picture came to: the answer, or a failure that may be gone soon (not remembered).
#[derive(Clone)]
enum Pic {
    Got(Arc<Got>),
    Gone,
    Passing,
}

/// Biggest picture kept for the other searches; bigger ones are read again when asked for.
const KEEP_PIC: usize = 256 * 1024;
/// Entries kept in each memo before it starts over.
const KEEP_ENTRIES: usize = 512;

static PICS: Lazy<Mutex<HashMap<String, Shared<Pic>>>> = Lazy::new(Default::default);

async fn fetch_pic(url: String, via: &'static str) -> Pic {
    match get(&url, MAX_IMAGE).await {
        Fetched::Body(bytes) => match sniff(&bytes) {
            Some(ext) => Pic::Got(Arc::new(Got { bytes, ext, via, dark: false })),
            None => Pic::Gone,
        },
        Fetched::Failed => Pic::Passing,
        Fetched::Gone | Fetched::Refused => Pic::Gone,
    }
}

/// Ok with the picture, or Err(true) when it failed for a passing reason.
async fn image(url: &str, via: &'static str) -> Result<Got, bool> {
    let fut = {
        let mut m = PICS.lock();
        if m.len() >= KEEP_ENTRIES {
            m.clear();
        }
        m.entry(url.to_string()).or_insert_with(|| shared(fetch_pic(url.to_string(), via), Pic::Passing)).clone()
    };
    match fut.await {
        Pic::Got(g) => {
            if g.bytes.len() > KEEP_PIC {
                PICS.lock().remove(url);
            }
            Ok((*g).clone())
        }
        Pic::Gone => Err(false),
        Pic::Passing => {
            PICS.lock().remove(url);
            Err(true)
        }
    }
}

/// A picture at an address, read as the icon search reads every picture: our own agent, one
/// request, the server's turn kept. Err(true): it failed for a passing reason.
pub async fn picture(url: &str) -> Result<Got, bool> {
    image(url, "site").await
}

/// A small text (a JSON answer) at an address; None when the server has none or does not answer.
pub async fn text(url: &str) -> Option<String> {
    match get(url, MAX_PAGE).await {
        Fetched::Body(b) => Some(String::from_utf8_lossy(&b).into_owned()),
        _ => None,
    }
}

/// What reading one page came to.
#[derive(Clone)]
enum Links {
    /// The server refuses us.
    Refused,
    /// The icons the page names (and its web app manifest), best first; none when it names none.
    Read(Arc<Vec<String>>),
    /// Some step failed for a passing reason; what was read until then.
    Passing(Arc<Vec<String>>),
}

static PAGES: Lazy<Mutex<HashMap<String, Shared<Links>>>> = Lazy::new(Default::default);
/// Servers that refused us in this run (401, 403, 451): nothing more is asked of them.
static REFUSED: Lazy<Mutex<HashSet<String>>> = Lazy::new(Default::default);

async fn read_page(page: String) -> Links {
    let mut tries: Vec<String> = Vec::new();
    match get(&page, MAX_PAGE).await {
        Fetched::Body(body) => {
            let html = String::from_utf8_lossy(&body).into_owned();
            let (found, web_manifest) = icons_from_html(&html, &page);
            tries.extend(found.into_iter().map(|f| f.url));
            if let Some(m) = web_manifest {
                match get(&m, MAX_PAGE).await {
                    Fetched::Body(b2) => tries.extend(icons_from_web_manifest(&String::from_utf8_lossy(&b2), &m).into_iter().map(|f| f.url)),
                    Fetched::Failed => return Links::Passing(Arc::new(tries)),
                    _ => {}
                }
            }
            Links::Read(Arc::new(tries))
        }
        Fetched::Refused => Links::Refused,
        Fetched::Failed => Links::Passing(Arc::new(tries)),
        Fetched::Gone => Links::Read(Arc::new(tries)),
    }
}

async fn links_of(page: &str) -> Links {
    let fut = {
        let mut m = PAGES.lock();
        if m.len() >= KEEP_ENTRIES * 8 {
            m.clear();
        }
        m.entry(page.to_string()).or_insert_with(|| shared(read_page(page.to_string()), Links::Passing(Arc::new(Vec::new())))).clone()
    };
    let out = fut.await;
    if matches!(out, Links::Passing(_)) {
        PAGES.lock().remove(page);
    }
    out
}

/// Find an icon for a package. `manifest_icon` is the manifest's own `IconUrl`; `pages` are the
/// package and publisher pages from the manifest. The cheap, reliable sources go first (the
/// manifest's icon, a known address, the GitHub avatar), and the search stops at the first good one.
pub async fn find(id: &str, manifest_icon: Option<&str>, pages: &[String]) -> Result<Got, Miss> {
    let mut passing = false;
    if let Some(u) = manifest_icon {
        match image(u, "manifest").await {
            Ok(r) => return Ok(r),
            Err(p) => passing |= p,
        }
    }
    if let Some((_, u)) = OVERRIDES.iter().find(|(k, _)| k.eq_ignore_ascii_case(id)) {
        match image(u, "override").await {
            Ok(r) => return Ok(r),
            Err(p) => passing |= p,
        }
    }
    let mut seen_hosts: Vec<String> = Vec::new();
    for page in pages {
        if let Some(u) = github_avatar(page) {
            match image(&u, "github").await {
                Ok(r) => return Ok(r),
                Err(p) => passing |= p,
            }
            continue;
        }
        let Ok(url) = Url::parse(page) else { continue };
        let Some(host) = url.host_str().map(|h| h.to_ascii_lowercase()) else { continue };
        if seen_hosts.contains(&host) || !matches!(url.scheme(), "http" | "https") || REFUSED.lock().contains(&host) {
            continue;
        }
        seen_hosts.push(host.clone());
        let mut tries: Vec<String> = Vec::new();
        match links_of(page).await {
            Links::Read(t) => tries.extend(t.iter().cloned()),
            Links::Passing(t) => {
                passing = true;
                tries.extend(t.iter().cloned());
            }
            Links::Refused => {
                REFUSED.lock().insert(host);
                continue;
            }
        }
        let root = url.join("/").ok().map(|u| u.to_string()).unwrap_or_default();
        tries.push(format!("{root}apple-touch-icon.png"));
        tries.push(format!("{root}favicon.ico"));
        let mut done: Vec<String> = Vec::new();
        for t in tries {
            if !done.contains(&t) && done.len() < MAX_TRIES {
                done.push(t);
            }
        }
        // The best candidate wins; the next ones load meanwhile, so a miss costs no extra wait.
        let mut got = futures_util::stream::iter(done.into_iter().map(|t| async move {
            let via = if t.ends_with("favicon.ico") { "favicon" } else { "page" };
            image(&t, via).await
        }))
        .buffered(AHEAD);
        while let Some(r) = got.next().await {
            match r {
                Ok(r) => return Ok(r),
                Err(p) => passing |= p,
            }
        }
    }
    Err(Miss { passing })
}

/// Is a picture mostly dark where it is not clear (a black logo on a clear background)? `rgba` is
/// straight RGBA. A dark tile with a light sign on it is not: the sign reads on its own.
pub fn mostly_dark(rgba: &[u8]) -> bool {
    let (mut seen, mut light, mut lum) = (0u64, 0u64, 0f64);
    for p in rgba.chunks_exact(4) {
        if p[3] < 128 {
            continue;
        }
        let l = (0.2126 * p[0] as f64 + 0.7152 * p[1] as f64 + 0.0722 * p[2] as f64) / 255.0;
        seen += 1;
        lum += l;
        if l > 0.45 {
            light += 1;
        }
    }
    // a mostly clear picture with a few dark pixels is just as hard to see
    seen > 0 && lum / (seen as f64) < 0.22 && (light as f64) < (seen as f64) * 0.06
}

/// The picture as a PNG of at most `EDGE` pixels, read once so the webview decodes a small file.
/// A vector stays as it is; a picture Windows cannot read stays as it is too.
pub fn normalize(got: Got) -> Got {
    if got.ext == "svg" {
        return got;
    }
    match decode_scaled(&got.bytes, EDGE) {
        Some((w, h, rgba)) => match encode_png(w, h, &rgba) {
            Some(bytes) => Got { bytes, ext: "png", via: got.via, dark: mostly_dark(&rgba) },
            None => got,
        },
        None => got,
    }
}

/// A tall or wide picture cut to its middle square and made small, as an icon (the store
/// pictures of a game). A picture Windows cannot read stays as it is.
pub fn normalize_square(got: Got) -> Got {
    if let Some((w, h, rgba)) = decode_scaled(&got.bytes, 4096) {
        let side = w.min(h);
        let (x0, y0) = ((w - side) / 2, (h - side) / 2);
        let mut square: Vec<u8> = Vec::with_capacity((side * side * 4) as usize);
        for y in 0..side {
            let from = (((y0 + y) * w + x0) * 4) as usize;
            square.extend_from_slice(&rgba[from..from + (side * 4) as usize]);
        }
        if let Some(png) = encode_png(side, side, &square) {
            return normalize(Got { bytes: png, ext: "png", via: got.via, dark: false });
        }
    }
    normalize(got)
}

fn encode_png(w: u32, h: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::High);
        let mut wr = enc.write_header().ok()?;
        wr.write_image_data(rgba).ok()?;
    }
    Some(out)
}

/// Decode any picture Windows reads (PNG, JPEG, GIF, ICO, BMP, WebP) into straight RGBA, made
/// smaller to fit `edge` (never larger). For an .ico the biggest image in it is used.
#[cfg(windows)]
pub fn decode_scaled(bytes: &[u8], edge: u32) -> Option<(u32, u32, Vec<u8>)> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let r = (|| -> windows::core::Result<(u32, u32, Vec<u8>)> {
            let f: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
            let stream = f.CreateStream()?;
            stream.InitializeFromMemory(bytes)?;
            let dec = f.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand)?;
            let (mut best, mut best_px) = (None, 0u64);
            for i in 0..dec.GetFrameCount()?.min(16) {
                let fr = dec.GetFrame(i)?;
                let (mut w, mut h) = (0u32, 0u32);
                fr.GetSize(&mut w, &mut h)?;
                if (w as u64) * (h as u64) > best_px {
                    best_px = (w as u64) * (h as u64);
                    best = Some((fr, w, h));
                }
            }
            let (fr, w, h) = best.ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_FAIL))?;
            let scale = (edge as f64 / w.max(h) as f64).min(1.0);
            let (nw, nh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
            let src: IWICBitmapSource = if (nw, nh) == (w, h) {
                fr.cast()?
            } else {
                let s = f.CreateBitmapScaler()?;
                s.Initialize(&fr, nw, nh, WICBitmapInterpolationModeHighQualityCubic)?;
                s.cast()?
            };
            let conv = f.CreateFormatConverter()?;
            conv.Initialize(&src, &GUID_WICPixelFormat32bppRGBA, WICBitmapDitherTypeNone, None::<&IWICPalette>, 0.0, WICBitmapPaletteTypeCustom)?;
            let mut buf = vec![0u8; (nw * nh * 4) as usize];
            conv.CopyPixels(std::ptr::null(), nw * 4, &mut buf)?;
            Ok((nw, nh, buf))
        })()
        // an error can hold a COM object: it is dropped here, before COM is shut down for this thread
        .ok();
        if init.is_ok() {
            CoUninitialize();
        }
        r
    }
}

#[cfg(not(windows))]
pub fn decode_scaled(_bytes: &[u8], _edge: u32) -> Option<(u32, u32, Vec<u8>)> {
    None
}

/// The file an icon is saved as: normalized pictures are `<stem>.icon.png`, or `<stem>.dark.png`
/// when they need the light plate; a vector keeps its own type.
pub fn saved_name(id: &str, got: &Got) -> String {
    saved_name_for(&file_stem(id), got)
}

/// `saved_name` for a file name stem that is not a package's (a maker's, a Steam app's).
pub fn saved_name_for(stem: &str, got: &Got) -> String {
    match (got.ext, got.dark) {
        ("png", true) => format!("{stem}.dark.png"),
        ("png", false) => format!("{stem}.icon.png"),
        (ext, _) => format!("{stem}.{ext}"),
    }
}

/// File types an icon of an earlier version was saved with, before icons were made small.
pub const OLD_EXTS: &[&str] = &["png", "webp", "jpg", "gif", "ico"];

/// The files in the icon cache (lower case name and size), read at most every few seconds and
/// again after an icon is written or removed here: a page of 60 apps asks for 60 icons at once,
/// and asking Windows about each of the several names an icon can have takes longer than reading
/// the whole folder once.
struct Listing {
    dir: std::path::PathBuf,
    at: Instant,
    files: Arc<HashMap<String, u64>>,
}

static LISTING: Lazy<Mutex<Option<Listing>>> = Lazy::new(Default::default);
const LISTING_FRESH: Duration = Duration::from_secs(4);

/// An icon file was written or removed just now: the next look at the folder reads it again.
pub fn wrote() {
    *LISTING.lock() = None;
}

fn listing(dir: &std::path::Path) -> Arc<HashMap<String, u64>> {
    if let Some(l) = LISTING.lock().as_ref().filter(|l| l.dir == dir && l.at.elapsed() < LISTING_FRESH) {
        return l.files.clone();
    }
    let files: HashMap<String, u64> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.file_name().to_string_lossy().to_ascii_lowercase(), e.metadata().ok()?.len())))
        .collect();
    let files = Arc::new(files);
    *LISTING.lock() = Some(Listing { dir: dir.to_path_buf(), at: Instant::now(), files: files.clone() });
    files
}

/// An icon kept in the icon cache from an earlier run under this file name stem: a small one
/// first, else one saved at full size by an earlier version.
pub fn on_disk(stem: &str) -> Option<std::path::PathBuf> {
    // the folder's own path is asked for (and made, if need be) only when the listing is read again
    let kept = LISTING.lock().as_ref().filter(|l| l.at.elapsed() < LISTING_FRESH).map(|l| (l.dir.clone(), l.files.clone()));
    let (dir, files) = kept.unwrap_or_else(|| {
        let dir = crate::paths::icons();
        let files = listing(&dir);
        (dir, files)
    });
    pick(&dir, &files, stem)
}

#[cfg(test)]
fn on_disk_in(dir: &std::path::Path, stem: &str) -> Option<std::path::PathBuf> {
    pick(dir, &listing(dir), stem)
}

fn pick(dir: &std::path::Path, files: &HashMap<String, u64>, stem: &str) -> Option<std::path::PathBuf> {
    ["icon.png", "dark.png", "svg"].iter().chain(OLD_EXTS).map(|e| format!("{stem}.{e}").to_ascii_lowercase()).find(|n| files.get(n).is_some_and(|len| *len > 0)).map(|n| dir.join(n))
}

#[cfg(test)]
mod listing_tests {
    use super::*;

    #[test]
    fn icons_are_found_among_the_files_of_the_folder_and_a_change_is_seen_after_wrote() {
        let d = std::env::temp_dir().join(format!("tu3d-icons-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        wrote();
        assert_eq!(on_disk_in(&d, "other-a.b"), None);
        std::fs::write(d.join("other-a.b.png"), b"old").unwrap();
        wrote();
        // an icon of an earlier version at full size is found
        assert_eq!(on_disk_in(&d, "other-a.b"), Some(d.join("other-a.b.png")));
        // a small one comes before it, whatever the case of its name
        std::fs::write(d.join("Other-A.B.icon.png"), b"new").unwrap();
        wrote();
        assert_eq!(on_disk_in(&d, "other-a.b"), Some(d.join("other-a.b.icon.png")));
        // an empty file is no icon
        std::fs::write(d.join("other-e.mpty.icon.png"), b"").unwrap();
        wrote();
        assert_eq!(on_disk_in(&d, "other-e.mpty"), None);
        // the folder is read once for many questions, not again for each
        std::fs::write(d.join("other-late.icon.png"), b"x").unwrap();
        assert_eq!(on_disk_in(&d, "other-late"), None);
        wrote();
        assert_eq!(on_disk_in(&d, "other-late"), Some(d.join("other-late.icon.png")));
        // removed icons are gone for the next look
        std::fs::remove_file(d.join("other-a.b.png")).unwrap();
        std::fs::remove_file(d.join("Other-A.B.icon.png")).unwrap();
        wrote();
        assert_eq!(on_disk_in(&d, "other-a.b"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// Made small already (or a vector, which needs no making small).
pub fn is_small(p: &std::path::Path) -> bool {
    let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    n.ends_with(".icon.png") || n.ends_with(".dark.png") || n.ends_with(".svg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_biggest_sharp_touch_icon_and_resolves_relative_addresses() {
        let html = r#"<html><head>
            <link rel="shortcut icon" href="/favicon.ico">
            <link rel="icon" type="image/png" sizes="16x16" href="img/16.png">
            <link rel="icon" type="image/png" sizes="32x32" href="img/32.png">
            <link rel="apple-touch-icon" sizes="180x180" href="../apple.png">
            <link rel="icon" sizes="192x192" href='https://cdn.example.org/192.png'>
            <link rel="stylesheet" href="/a.css">
            <link rel="manifest" href="/site.webmanifest">
        </head></html>"#;
        let (found, manifest) = icons_from_html(html, "https://example.com/en/app/index.html");
        let urls: Vec<&str> = found.iter().map(|f| f.url.as_str()).collect();
        assert_eq!(urls[0], "https://cdn.example.org/192.png");
        assert_eq!(urls[1], "https://example.com/en/apple.png");
        // small ones after the sharp ones, the .ico file last
        assert_eq!(urls[2], "https://example.com/en/app/img/32.png");
        assert_eq!(*urls.last().unwrap(), "https://example.com/favicon.ico");
        assert_eq!(manifest.as_deref(), Some("https://example.com/site.webmanifest"));
    }

    #[test]
    fn a_vector_icon_beats_small_pngs_and_a_touch_icon_without_size_counts_as_big() {
        let html = r#"<link rel="icon" href="/logo.svg" type="image/svg+xml"><link rel="icon" sizes="48x48" href="/48.png">"#;
        let (found, _) = icons_from_html(html, "https://example.com/");
        assert_eq!(found[0].url, "https://example.com/logo.svg");
        assert!(found[0].vector);
        let (found, _) = icons_from_html(r#"<link rel="icon" sizes="32x32" href="/32.png"><link rel="apple-touch-icon" href="/t.png">"#, "https://example.com/");
        assert_eq!(found[0].url, "https://example.com/t.png");
        assert_eq!(found[0].size, 180);
    }

    #[test]
    fn reads_attributes_in_any_order_and_quote_style() {
        let html = "<LINK HREF=/x.png SIZES=\"96x96\" REL=icon>";
        let (found, _) = icons_from_html(html, "https://example.com/p/");
        assert_eq!(found, vec![Found { url: "https://example.com/x.png".into(), size: 96, vector: false }]);
        assert!(icons_from_html(r#"<link rel="icon" href="data:image/png;base64,AAAA">"#, "https://example.com/").0.is_empty());
    }

    #[test]
    fn web_app_manifests_skip_maskable_only_icons() {
        let json = r#"{"icons":[{"src":"/m.png","sizes":"512x512","purpose":"maskable"},{"src":"i/192.png","sizes":"192x192"},{"src":"i/512.png","sizes":"512x512","purpose":"any maskable"}]}"#;
        let f = icons_from_web_manifest(json, "https://example.com/app/site.webmanifest");
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].url, "https://example.com/app/i/192.png");
    }

    #[test]
    fn github_pages_use_the_owner_avatar() {
        assert_eq!(github_avatar("https://github.com/notepad-plus-plus/notepad-plus-plus").as_deref(), Some("https://github.com/notepad-plus-plus.png?size=128"));
        assert_eq!(github_avatar("https://github.com/Owner").as_deref(), Some("https://github.com/Owner.png?size=128"));
        assert_eq!(github_avatar("https://example.com/foo/bar"), None);
        assert_eq!(github_avatar("https://github.com/"), None);
        assert_eq!(github_avatar("https://github.com/orgs/x/people"), None);
        assert_eq!(github_avatar("https://notgithub.com/a/b"), None);
    }

    #[test]
    fn tells_images_from_pages() {
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n0000"), Some("png"));
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0, 0]), Some("jpg"));
        assert_eq!(sniff(b"RIFF1234WEBPVP8 "), Some("webp"));
        let mut ico = vec![0, 0, 1, 0, 1, 0];
        ico.resize(40, 0);
        assert_eq!(sniff(&ico), Some("ico"));
        assert_eq!(sniff(b"<?xml version=\"1.0\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"), Some("svg"));
        assert_eq!(sniff(b"  <svg viewBox=\"0 0 1 1\"/>"), Some("svg"));
        assert_eq!(sniff(b"<!DOCTYPE html><html><body><svg></svg></body></html>"), None);
        assert_eq!(sniff(b"Not found"), None);
        assert_eq!(sniff(b""), None);
    }

    #[test]
    fn file_names_are_safe() {
        assert_eq!(file_stem("Notepad++.Notepad++"), "other-notepad__.notepad__");
        assert_eq!(file_stem("7zip.7zip"), "other-7zip.7zip");
    }

    #[test]
    fn apps_whose_sites_refuse_us_have_a_known_icon() {
        for id in ["EpicGames.EpicGamesLauncher", "Cisco.Webex", "cisco.ciscowebexmeetings"] {
            assert!(OVERRIDES.iter().any(|(k, _)| k.eq_ignore_ascii_case(id)), "{id}");
        }
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn drain(q: &mut Queue) -> Vec<String> {
        let mut out = vec![];
        while let Some(i) = q.pop() {
            out.push(i);
        }
        out
    }

    #[test]
    fn the_cards_on_screen_go_first_then_the_rows_around_them_then_the_background() {
        let mut q = Queue::default();
        q.back(&ids(&["s1", "s2", "s3"]));
        q.view(&ids(&["v1", "v2"]), &ids(&["n1", "v2", "n2"]));
        q.back(&ids(&["s1", "s4"]));
        assert_eq!(q.waiting(Tier::View), 2);
        assert_eq!(q.waiting(Tier::Near), 2);
        assert_eq!(drain(&mut q), ids(&["v1", "v2", "n1", "n2", "s1", "s2", "s3", "s4"]));
        assert!(q.is_empty());
        q.back(&ids(&["s1"]));
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn searches_for_places_that_scrolled_away_are_dropped() {
        let mut q = Queue::default();
        q.back(&ids(&["s1"]));
        assert_eq!(q.view(&ids(&["a", "b"]), &ids(&["c", "d"])), 0);
        // the list was flung on: a, c and d are no longer wanted, b is only around the screen now
        assert_eq!(q.view(&ids(&["e", "f"]), &ids(&["b"])), 3);
        assert_eq!(drain(&mut q), ids(&["e", "f", "b", "s1"]));
        // an empty view drops everything it had queued
        q.view(&ids(&["a"]), &ids(&["b"]));
        assert_eq!(q.view(&[], &[]), 2);
        assert!(q.is_empty());
    }

    #[test]
    fn a_card_that_comes_into_view_moves_up_from_the_background() {
        let mut q = Queue::default();
        q.back(&ids(&["s1", "S2", "s3"]));
        q.view(&ids(&["s2"]), &[]);
        assert_eq!(drain(&mut q), ids(&["s2", "s1", "s3"]));
    }

    #[test]
    fn the_page_of_one_app_is_searched_before_the_cards_and_stays_when_the_view_changes() {
        let mut q = Queue::default();
        q.view(&ids(&["v1"]), &[]);
        q.front(Tier::Pinned, &ids(&["p1", "v1"]));
        q.view(&ids(&["v2", "p1"]), &[]);
        assert_eq!(drain(&mut q), ids(&["p1", "v1", "v2"]));
    }

    #[test]
    fn ids_that_differ_in_case_are_one_app() {
        let mut q = Queue::default();
        q.view(&ids(&["Azul.Zulu", "azul.zulu"]), &ids(&["AZUL.ZULU"]));
        assert_eq!(q.len(), 1);
        assert_eq!(drain(&mut q), ids(&["Azul.Zulu"]));
    }

    #[test]
    fn statuses_tell_passing_failures_from_lasting_ones() {
        use reqwest::StatusCode as S;
        assert!(matches!(failure(S::NOT_FOUND), Fetched::Gone));
        assert!(matches!(failure(S::GONE), Fetched::Gone));
        assert!(matches!(failure(S::FORBIDDEN), Fetched::Refused));
        assert!(matches!(failure(S::TOO_MANY_REQUESTS), Fetched::Failed));
        assert!(matches!(failure(S::SERVICE_UNAVAILABLE), Fetched::Failed));
        assert!(matches!(failure(S::GATEWAY_TIMEOUT), Fetched::Failed));
    }

    #[tokio::test]
    async fn a_server_that_does_not_answer_is_a_passing_miss_and_quickly_given_up() {
        // a port nothing listens on: the connection is refused at once
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        drop(l);
        let t = Instant::now();
        let r = find("Nobody.Here", Some(&format!("http://127.0.0.1:{port}/icon.png")), &[]).await;
        assert_eq!(r.err(), Some(Miss { passing: true }));
        assert!(t.elapsed() < TIMEOUT + Duration::from_secs(1));
        // nothing to try at all is a lasting miss
        assert_eq!(find("Nobody.Here", None, &[]).await.err(), Some(Miss { passing: false }));
    }

    #[test]
    fn passing_failures_wait_a_while_then_are_searched_again() {
        assert!(may_search("Test.RetryLater"));
        retry_later("Test.RetryLater");
        assert!(!may_search("test.retrylater"));
        RETRY.lock().insert("test.retrylater".into(), Instant::now() - Duration::from_secs(1));
        assert!(may_search("Test.RetryLater"));
    }

    fn png_of(w: u32, h: u32, px: [u8; 4]) -> Vec<u8> {
        let rgba: Vec<u8> = (0..w * h).flat_map(|_| px).collect();
        encode_png(w, h, &rgba).unwrap()
    }

    /// A server with a few files; answers 404 to the rest and notes every path asked for.
    async fn serve(bind: &str, files: Vec<(&'static str, u16, &'static str, Vec<u8>)>) -> (String, Arc<Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind(format!("{bind}:0")).await.unwrap();
        let base = format!("http://{}", l.local_addr().unwrap());
        let log: Arc<Mutex<Vec<String>>> = Arc::default();
        let seen = log.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = l.accept().await {
                let (files, seen) = (files.clone(), seen.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    seen.lock().push(path.clone());
                    let (code, kind, body) = files.iter().find(|f| f.0 == path).map(|f| (f.1, f.2, f.3.clone())).unwrap_or((404, "text/plain", b"no".to_vec()));
                    let head = format!("HTTP/1.1 {code} X\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                    let _ = sock.write_all(head.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (base, log)
    }

    fn hits(log: &Arc<Mutex<Vec<String>>>, path: &str) -> usize {
        log.lock().iter().filter(|p| *p == path).count()
    }

    #[tokio::test]
    async fn apps_of_one_publisher_read_the_same_page_and_icon_once() {
        let html = b"<html><head><link rel=\"icon\" sizes=\"128x128\" href=\"/logo.png\"></head></html>".to_vec();
        let (base, log) = serve("127.0.0.1", vec![("/downloads", 200, "text/html", html), ("/logo.png", 200, "image/png", png_of(64, 64, [200, 30, 30, 255]))]).await;
        let pages = vec![format!("{base}/downloads")];
        // the whole run of a publisher's apps starts at the same moment
        let runs = futures_util::future::join_all((0..12).map(|i| {
            let pages = pages.clone();
            async move { find(&format!("Pub.App{i}"), None, &pages).await }
        }))
        .await;
        assert!(runs.iter().all(|r| r.as_ref().is_ok_and(|g| g.ext == "png")));
        assert_eq!(hits(&log, "/downloads"), 1);
        assert_eq!(hits(&log, "/logo.png"), 1);
        // and an app that comes later takes the answer too
        assert!(find("Pub.Later", None, &pages).await.is_ok());
        assert_eq!(hits(&log, "/downloads"), 1);
        assert_eq!(hits(&log, "/logo.png"), 1);
    }

    #[tokio::test]
    async fn a_server_that_refuses_us_is_asked_once() {
        let (base, log) = serve("127.0.0.2", vec![("/a", 403, "text/html", b"no".to_vec()), ("/b", 403, "text/html", b"no".to_vec())]).await;
        let first = find("Pub.One", None, &[format!("{base}/a")]).await;
        assert_eq!(first.err(), Some(Miss { passing: false }));
        // another app of the same publisher does not knock again, not even for its favicon
        let second = find("Pub.Two", None, &[format!("{base}/b")]).await;
        assert_eq!(second.err(), Some(Miss { passing: false }));
        assert_eq!(log.lock().clone(), vec!["/a".to_string()]);
    }

    #[tokio::test]
    async fn a_page_that_failed_for_a_passing_reason_is_read_again_by_the_next_search() {
        // nothing listens here: the page cannot be read, which is a passing failure
        let l = std::net::TcpListener::bind("127.0.0.3:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        let page = format!("http://{addr}/p");
        assert_eq!(find("Pub.One", None, &[page.clone()]).await.err(), Some(Miss { passing: true }));
        assert!(PAGES.lock().get(&page).is_none(), "a failure for a passing reason is not kept");
    }

    #[test]
    fn dark_logos_on_clear_backgrounds_are_told_apart() {
        let px = |c: [u8; 4], n: usize| -> Vec<u8> { (0..n).flat_map(|_| c).collect() };
        // a black logo with clear parts (Spotify's)
        let mut a = px([0, 0, 0, 255], 600);
        a.extend(px([0, 0, 0, 0], 400));
        assert!(mostly_dark(&a));
        // a dark tile with a white sign on it reads fine
        let mut b = px([20, 20, 30, 255], 800);
        b.extend(px([255, 255, 255, 255], 200));
        assert!(!mostly_dark(&b));
        // a colored logo
        assert!(!mostly_dark(&px([30, 215, 96, 255], 100)));
        // nothing visible at all
        assert!(!mostly_dark(&px([0, 0, 0, 0], 100)));
    }

    #[cfg(windows)]
    #[test]
    fn a_tall_picture_becomes_its_middle_square() {
        // 60 x 180: red on top, green in the middle, blue at the bottom
        let mut rgba = Vec::new();
        for y in 0..180u32 {
            let px = if y < 60 { [255, 0, 0, 255] } else if y < 120 { [0, 255, 0, 255] } else { [0, 0, 255, 255] };
            rgba.extend((0..60).flat_map(|_| px));
        }
        let got = normalize_square(Got { bytes: encode_png(60, 180, &rgba).unwrap(), ext: "png", via: "site", dark: false });
        assert_eq!(got.ext, "png");
        let (w, h, out) = decode_scaled(&got.bytes, 4096).unwrap();
        assert_eq!((w, h), (60, 60));
        assert!(out.chunks(4).all(|p| p[0] < 10 && p[1] > 245 && p[2] < 10), "only the middle third is left");
    }

    #[cfg(windows)]
    #[test]
    fn big_pictures_are_made_small_once_and_small_ones_kept_as_they_are() {
        let got = normalize(Got { bytes: png_of(512, 256, [0, 0, 0, 255]), ext: "png", via: "page", dark: false });
        assert_eq!(got.ext, "png");
        assert!(got.dark);
        let d = png::Decoder::new(std::io::Cursor::new(&got.bytes));
        let info = d.read_info().unwrap();
        assert_eq!((info.info().width, info.info().height), (EDGE, EDGE / 2));
        let (w, h, _) = decode_scaled(&png_of(48, 48, [200, 60, 60, 255]), EDGE).unwrap();
        assert_eq!((w, h), (48, 48));
        // a vector and bytes Windows cannot read are kept
        let svg = normalize(Got { bytes: b"<svg/>".to_vec(), ext: "svg", via: "page", dark: false });
        assert_eq!(svg.ext, "svg");
        let junk = normalize(Got { bytes: vec![0, 0, 1, 0, 9, 9, 9], ext: "ico", via: "page", dark: false });
        assert_eq!(junk.ext, "ico");
        assert_eq!(saved_name("A.B", &got), "other-a.b.dark.png");
    }
}
