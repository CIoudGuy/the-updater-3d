//! Real icons for the Other apps catalog. The winget manifest names an icon for few apps, so the
//! rest come from the app's own web page: its touch icon, its icon links, its web app manifest,
//! the GitHub avatar of the project owner, and last the plain favicon. The web is read with our
//! own User-Agent and short limits; a site that refuses us is a miss.

use crate::http::CLIENT;
use once_cell::sync::Lazy;
use regex::Regex;
use std::time::Duration;
use url::Url;

/// Icons that are poor on the app's page (or missing there), by lowercase winget id.
const OVERRIDES: &[(&str, &str)] = &[
    ("Mojang.MinecraftLauncher", "https://www.minecraft.net/favicon.ico"),
    ("Autodesk.Fusion", "https://www.autodesk.com/favicon.ico"),
    ("Adobe.Acrobat.Reader.64-bit", "https://www.adobe.com/favicon.ico"),
    ("LyX.LyX", "https://www.lyx.org/favicon.ico"),
];

/// Biggest image or page we read.
const MAX_BYTES: usize = 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(8);
/// Icon candidates tried per app after the page was read.
const MAX_TRIES: usize = 5;

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

/// A fetched body, capped, with the status.
async fn get(url: &str) -> Option<(reqwest::StatusCode, Vec<u8>)> {
    let mut resp = CLIENT.get(url).timeout(TIMEOUT).send().await.ok()?;
    let status = resp.status();
    if !status.is_success() {
        return Some((status, vec![]));
    }
    if resp.content_length().is_some_and(|n| n as usize > MAX_BYTES) {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            return None;
        }
    }
    Some((status, body))
}

/// A found icon: the picture, its file type and where it came from.
pub struct Got {
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    pub via: &'static str,
}

async fn image(url: &str, via: &'static str) -> Option<Got> {
    let (_, bytes) = get(url).await?;
    let ext = sniff(&bytes)?;
    Some(Got { bytes, ext, via })
}

fn blocked(s: reqwest::StatusCode) -> bool {
    matches!(s.as_u16(), 401 | 403 | 429 | 451 | 503)
}

/// Find an icon for a package. `manifest_icon` is the manifest's own `IconUrl`; `pages` are the
/// package and publisher pages from the manifest.
pub async fn find(id: &str, manifest_icon: Option<&str>, pages: &[String]) -> Option<Got> {
    if let Some(u) = manifest_icon {
        if let Some(r) = image(u, "manifest").await {
            return Some(r);
        }
    }
    if let Some((_, u)) = OVERRIDES.iter().find(|(k, _)| k.eq_ignore_ascii_case(id)) {
        if let Some(r) = image(u, "override").await {
            return Some(r);
        }
    }
    let mut seen_hosts: Vec<String> = Vec::new();
    for page in pages {
        if let Some(u) = github_avatar(page) {
            if let Some(r) = image(&u, "github").await {
                return Some(r);
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
        let mut refused = false;
        match get(page).await {
            Some((st, body)) if st.is_success() => {
                let html = String::from_utf8_lossy(&body).into_owned();
                let (found, web_manifest) = icons_from_html(&html, page);
                tries.extend(found.into_iter().map(|f| f.url));
                if let Some(m) = web_manifest {
                    if let Some((st2, b2)) = get(&m).await {
                        if st2.is_success() {
                            tries.extend(icons_from_web_manifest(&String::from_utf8_lossy(&b2), &m).into_iter().map(|f| f.url));
                        }
                    }
                }
            }
            Some((st, _)) => refused = blocked(st),
            None => {}
        }
        if refused {
            continue;
        }
        let root = url.join("/").ok().map(|u| u.to_string()).unwrap_or_default();
        tries.push(format!("{root}apple-touch-icon.png"));
        tries.push(format!("{root}favicon.ico"));
        let mut done: Vec<String> = Vec::new();
        for t in tries {
            if done.contains(&t) {
                continue;
            }
            done.push(t.clone());
            if done.len() > MAX_TRIES {
                break;
            }
            if let Some(r) = image(&t, if t.ends_with("favicon.ico") { "favicon" } else { "page" }).await {
                return Some(r);
            }
        }
    }
    None
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
}
