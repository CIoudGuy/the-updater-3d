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
/// Apps whose icon is searched for at the same time.
pub const PARALLEL: usize = 16;
/// Requests to one server at the same time, so a page of apps by one publisher does not flood it.
const PER_HOST: usize = 4;
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

/// Apps waiting for their icon search, the ones on screen first. A page that is shown puts its
/// apps in front of everything asked for earlier (the list scrolled past is still searched, after
/// it); the starter list in the background goes to the back.
#[derive(Default)]
pub struct Queue {
    order: VecDeque<String>,
    queued: HashSet<String>,
}

impl Queue {
    /// Put these ids first, in their order; ids already waiting move up.
    pub fn front(&mut self, ids: &[String]) {
        let keys: HashSet<String> = ids.iter().map(|i| i.to_ascii_lowercase()).collect();
        self.order.retain(|i| !keys.contains(&i.to_ascii_lowercase()));
        let mut seen = HashSet::new();
        for id in ids.iter().rev() {
            if seen.insert(id.to_ascii_lowercase()) {
                self.order.push_front(id.clone());
            }
        }
        self.queued.extend(keys);
    }
    /// Add these ids after everything waiting; ids already waiting keep their place.
    pub fn back(&mut self, ids: &[String]) {
        for id in ids {
            if self.queued.insert(id.to_ascii_lowercase()) {
                self.order.push_back(id.clone());
            }
        }
    }
    pub fn pop(&mut self) -> Option<String> {
        let id = self.order.pop_front()?;
        self.queued.remove(&id.to_ascii_lowercase());
        Some(id)
    }
    pub fn len(&self) -> usize {
        self.order.len()
    }
    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
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

/// Ok with the picture, or Err(true) when it failed for a passing reason.
async fn image(url: &str, via: &'static str) -> Result<Got, bool> {
    match get(url, MAX_IMAGE).await {
        Fetched::Body(bytes) => match sniff(&bytes) {
            Some(ext) => Ok(Got { bytes, ext, via, dark: false }),
            None => Err(false),
        },
        Fetched::Failed => Err(true),
        Fetched::Gone | Fetched::Refused => Err(false),
    }
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
        if seen_hosts.contains(&host) || !matches!(url.scheme(), "http" | "https") {
            continue;
        }
        seen_hosts.push(host);
        let mut tries: Vec<String> = Vec::new();
        match get(page, MAX_PAGE).await {
            Fetched::Body(body) => {
                let html = String::from_utf8_lossy(&body).into_owned();
                let (found, web_manifest) = icons_from_html(&html, page);
                tries.extend(found.into_iter().map(|f| f.url));
                if let Some(m) = web_manifest {
                    match get(&m, MAX_PAGE).await {
                        Fetched::Body(b2) => tries.extend(icons_from_web_manifest(&String::from_utf8_lossy(&b2), &m).into_iter().map(|f| f.url)),
                        Fetched::Failed => passing = true,
                        _ => {}
                    }
                }
            }
            Fetched::Refused => continue,
            Fetched::Failed => passing = true,
            Fetched::Gone => {}
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
    let stem = file_stem(id);
    match (got.ext, got.dark) {
        ("png", true) => format!("{stem}.dark.png"),
        ("png", false) => format!("{stem}.icon.png"),
        (ext, _) => format!("{stem}.{ext}"),
    }
}

/// File types an icon of an earlier version was saved with, before icons were made small.
pub const OLD_EXTS: &[&str] = &["png", "webp", "jpg", "gif", "ico"];

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

    #[test]
    fn the_page_on_screen_is_searched_first_and_the_rest_after_it() {
        let mut q = Queue::default();
        q.back(&ids(&["s1", "s2", "s3"]));
        q.front(&ids(&["p1", "s2"]));
        q.front(&ids(&["v1", "v2", "p1"]));
        q.back(&ids(&["s1", "s4"]));
        let mut out = vec![];
        while let Some(i) = q.pop() {
            out.push(i);
        }
        assert_eq!(out, ids(&["v1", "v2", "p1", "s2", "s1", "s3", "s4"]));
        assert!(q.is_empty());
        q.back(&ids(&["s1"]));
        assert_eq!(q.len(), 1);
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
