//! Release notes read from the vendors' own pages (Blender and Adobe publish them as HTML).
//! Parsing is forgiving: when a page cannot be read the version list still works and links
//! to the full notes.

use crate::{http, version};
use std::collections::HashMap;
use std::time::Duration;

/// The whole published list; only a runaway page is cut.
const MAX_LINES: usize = 400;

pub struct Notes {
    pub date: Option<String>,
    pub lines: Vec<String>,
}

fn strip_tags(s: &str) -> String {
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    let t = re.replace_all(s, "");
    let t = t
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&para;", "")
        .replace('\u{2014}', ", ")
        .replace('\u{2013}', "-");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// List items of one HTML fragment as clean lines, without trailing issue / commit references.
fn items(fragment: &str) -> Vec<String> {
    let li = regex::Regex::new(r"(?s)<li[^>]*>(.*?)</li>").unwrap();
    let refs = regex::Regex::new(r"\s*[\(\[]\s*(#\d+|[0-9a-f]{7,12})\s*[\)\]]\s*$").unwrap();
    let mut out = Vec::new();
    for c in li.captures_iter(fragment) {
        let mut t = strip_tags(&c[1]);
        // Nested lists repeat their children; skip very long merged items.
        if t.len() > 2000 {
            let cut = t.char_indices().nth(1997).map(|(i, _)| i).unwrap_or(t.len());
            t.truncate(cut);
            t.push_str("...");
        }
        let t = refs.replace(&t, "").trim().trim_end_matches('.').to_string();
        if !t.is_empty() && !out.contains(&t) {
            out.push(t);
        }
        if out.len() >= MAX_LINES {
            break;
        }
    }
    out
}

fn parse_date(s: &str) -> Option<String> {
    // "September 15, 2026" or "May 19th 2026"
    let clean = regex::Regex::new(r"(\d+)(st|nd|rd|th)").unwrap().replace_all(s.trim(), "$1").replace(',', "");
    for f in ["%B %d %Y", "%b %d %Y", "%d %B %Y"] {
        if let Ok(d) = chrono::NaiveDate::parse_from_str(&clean, f) {
            return Some(format!("{d}T00:00:00Z"));
        }
    }
    None
}

async fn page(url: &str) -> Option<String> {
    let f = tokio::time::timeout(Duration::from_secs(12), http::get_cached(url, &[], Duration::from_secs(12 * 3600))).await.ok()?.ok()?;
    Some(String::from_utf8_lossy(&f.body).into_owned())
}

/// Blender bug-fix lists for one series, keyed by full version ("4.5.14").
/// LTS series: blender.org/download/lts/<x-y>/. Others: the "corrective releases" docs page.
pub async fn blender(series: &str) -> HashMap<String, Notes> {
    let mut out = HashMap::new();
    let lts_url = format!("https://www.blender.org/download/lts/{}/", series.replace('.', "-"));
    if let Some(html) = page(&lts_url).await {
        // Two layouts: "Blender 4.5.14 LTS<span>September 15, 2026</span>" and
        // "Blender 4.2.23 LTS &#8211; July 21, 2026" followed by Download and Changelog lists.
        let re = regex::Regex::new(r"(?s)<summary[^>]*>(.*?)</summary>(.*?)</details>").unwrap();
        let ver = regex::Regex::new(r"Blender\s+([0-9]+\.[0-9]+\.[0-9]+)").unwrap();
        let when = regex::Regex::new(r"([A-Z][a-z]+ \d{1,2},? \d{4})").unwrap();
        for c in re.captures_iter(&html) {
            let head = strip_tags(&c[1].replace("&#8211;", " "));
            let Some(v) = ver.captures(&head) else { continue };
            let body = &c[2];
            let body = body.find("Changelog").map(|i| &body[i..]).unwrap_or(body);
            out.insert(v[1].to_string(), Notes { date: when.captures(&head).and_then(|d| parse_date(&d[1])), lines: items(body) });
        }
    }
    if out.is_empty() {
        let url = format!("https://developer.blender.org/docs/release_notes/{series}/corrective_releases/");
        if let Some(html) = page(&url).await {
            let h = regex::Regex::new(r#"(?s)<h2[^>]*id="blender-\d+"[^>]*>\s*Blender\s+([0-9.]+)"#).unwrap();
            let starts: Vec<(usize, String)> = h.captures_iter(&html).map(|c| (c.get(0).unwrap().start(), c[1].to_string())).collect();
            for (k, (pos, ver)) in starts.iter().enumerate() {
                let end = starts.get(k + 1).map(|s| s.0).unwrap_or(html.len());
                let block = &html[*pos..end];
                let date = regex::Regex::new(r"Released on ([A-Za-z]+ \d+(?:st|nd|rd|th)?,? \d{4})").unwrap().captures(block).and_then(|c| parse_date(&c[1]));
                out.insert(ver.clone(), Notes { date, lines: items(block) });
            }
        }
    }
    out
}

/// A page and the address it ended up at after redirects (Adobe's short links redirect to
/// helpx.adobe.com and then to experienceleague.adobe.com). Cached for the session.
async fn page_final(url: &str) -> Option<(String, String)> {
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    static CACHE: Lazy<Mutex<HashMap<String, (String, String)>>> = Lazy::new(|| Mutex::new(HashMap::new()));
    if let Some(hit) = CACHE.lock().get(url).cloned() {
        return Some(hit);
    }
    let resp = http::CLIENT.get(url).timeout(Duration::from_secs(15)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let final_url = resp.url().to_string();
    let body = resp.text().await.ok()?;
    CACHE.lock().insert(url.to_string(), (final_url.clone(), body.clone()));
    Some((final_url, body))
}

fn origin(url: &str) -> String {
    let rest = url.split("//").nth(1).unwrap_or("");
    format!("https://{}", rest.split('/').next().unwrap_or(""))
}

/// Headings of a page: (level, text, start, end).
fn headings(html: &str) -> Vec<(u8, String, usize, usize)> {
    let re = regex::Regex::new(r"(?s)<h([1-4])[^>]*>(.*?)</h[1-4]>").unwrap();
    re.captures_iter(html)
        .map(|c| {
            let m = c.get(0).unwrap();
            (c[1].parse().unwrap_or(4), strip_tags(&c[2]), m.start(), m.end())
        })
        .collect()
}

fn is_version(text: &str) -> bool {
    let t = text.trim();
    !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '.') && t.contains('.')
}


/// Notes for one version from an Adobe version page: the list under the heading for that exact
/// version ("12.2.0" in the Release Notes section); for a first release (x.y.0) without one, the
/// headlines of its new features.
fn adobe_from_version_page(html: &str, ver: &str) -> (Vec<String>, Option<String>) {
    let body = html.find("<main").map(|i| &html[i..]).unwrap_or(html);
    let heads = headings(body);
    let full = version::trim_like(&format!("{ver}.0.0"), "1.2.3");
    let want = |t: &str| {
        let t = t.trim();
        t == ver || version::cmp(t, &full).is_eq() && is_version(t)
    };
    if let Some(k) = heads.iter().position(|h| is_version(&h.1) && want(&h.1)) {
        let level = heads[k].0;
        let stop = heads[k + 1..]
            .iter()
            .find(|h| h.0 < level || (h.0 <= level && is_version(&h.1)) || h.1.eq_ignore_ascii_case("known issues"))
            .map(|h| h.2)
            .unwrap_or(body.len());
        let block = &body[heads[k].3..stop];
        let lines: Vec<String> = items(block);
        // "Release date: 2026/09/15"
        let date = regex::Regex::new(r"Release date:\s*(\d{4})/(\d{1,2})/(\d{1,2})")
            .unwrap()
            .captures(&strip_tags(block))
            .map(|c| format!("{}-{:0>2}-{:0>2}T00:00:00Z", &c[1], &c[2], &c[3]));
        if !lines.is_empty() {
            return (lines, date);
        }
    }
    // First release of a line: the feature headings before the release notes section.
    if version::parts(ver).get(2).copied().unwrap_or(0) == 0 {
        let notes_at = heads.iter().position(|h| h.0 == 2 && h.1.to_ascii_lowercase().contains("release notes")).unwrap_or(heads.len());
        let feats: Vec<String> = heads[..notes_at]
            .iter()
            .filter(|h| h.0 == 3 && !is_version(&h.1))
            .map(|h| h.1.clone())
            .take(MAX_LINES)
            .collect();
        if !feats.is_empty() {
            return (feats, None);
        }
    }
    (vec![], None)
}

/// Adobe notes for one version. The catalog's release notes link leads to a product page that
/// links one page per minor version ("version-12-2"); older products use one long page with a
/// heading per version.
pub async fn adobe(notes_url: &str, ver: &str) -> Option<Notes> {
    let (final_url, html) = page_final(notes_url).await?;
    let p = version::parts(ver);
    let minor = format!("{}-{}", p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0));
    // 1. The page for this minor version, linked from the product page or next to it.
    let link = regex::Regex::new(r##"href="([^"#?]*/version-(\d+)-(\d+)(?:\.html)?)""##).unwrap();
    let mut candidates: Vec<String> = link
        .captures_iter(&html)
        .filter(|c| format!("{}-{}", &c[2], &c[3]) == minor)
        .map(|c| if c[1].starts_with('/') { format!("{}{}", origin(&final_url), &c[1]) } else { c[1].to_string() })
        .collect();
    if let Some(i) = final_url.rfind('/') {
        candidates.push(format!("{}/version-{minor}", &final_url[..i]));
    }
    candidates.dedup();
    for url in candidates {
        if let Some((_, vp)) = page_final(&url).await {
            let (lines, date) = adobe_from_version_page(&vp, ver);
            if !lines.is_empty() {
                return Some(Notes { date, lines });
            }
        }
    }
    // 2. One long page: the heading that names this version, followed by its lists.
    let heads = headings(&html);
    let short = version::trim_like(ver, "1.2");
    for (k, h) in heads.iter().enumerate() {
        let words: Vec<&str> = h.1.split(|c: char| !(c.is_ascii_digit() || c == '.')).collect();
        if words.iter().any(|w| *w == ver || *w == short) {
            let stop = heads.get(k + 1).map(|n| n.2).unwrap_or(html.len());
            let lines: Vec<String> = items(&html[h.3..stop]);
            if !lines.is_empty() {
                return Some(Notes { date: None, lines });
            }
        }
    }
    None
}

/// The change list of a Krita release post: the list items under its "Changelog" heading.
/// Krita answers unknown posts with a normal page titled "404 Page not found", so that gives none.
/// Unity Hub's release notes page: every version with its date and full change list.
/// The page is a list of `<h2>3.22.3</h2>` headings, each followed by "October 13, 2026" and lists.
pub fn unity_hub(html: &str) -> Vec<(String, Option<String>, Vec<String>)> {
    static H2: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"<h2[^>]*>\s*(\d+\.\d+\.\d+)\s*</h2>").unwrap());
    static DATE: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"<p>\s*([A-Z][a-z]+ \d{1,2}, \d{4})\s*</p>").unwrap());
    let heads: Vec<(usize, usize, String)> = H2.captures_iter(html).map(|c| {
        let m = c.get(0).unwrap();
        (m.start(), m.end(), c[1].to_string())
    }).collect();
    let mut out: Vec<(String, Option<String>, Vec<String>)> = Vec::new();
    for (i, (_, end, ver)) in heads.iter().enumerate() {
        if out.iter().any(|(v, _, _)| v == ver) {
            continue;
        }
        let stop = heads.get(i + 1).map(|h| h.0).unwrap_or(html.len());
        // Section headings ("<li><strong>Projects</strong><ul>...") only hold a nested list: drop
        // the heading so each nested item is read on its own.
        static SECTION: once_cell::sync::Lazy<regex::Regex> = once_cell::sync::Lazy::new(|| regex::Regex::new(r"<li[^>]*>\s*<strong>[^<]*</strong>\s*<ul[^>]*>").unwrap());
        let block = SECTION.replace_all(&html[*end..stop], "<ul>");
        let date = DATE.captures(&block).and_then(|c| chrono::NaiveDate::parse_from_str(&c[1], "%B %d, %Y").ok()).map(|d| format!("{d}T00:00:00Z"));
        let lines: Vec<String> = items(&block);
        out.push((ver.clone(), date, lines));
    }
    out
}

pub fn krita_changelog(html: &str) -> Vec<String> {
    if html.contains("<title>404") {
        return vec![];
    }
    let heads = headings(html);
    let Some(k) = heads.iter().position(|h| h.1.trim().eq_ignore_ascii_case("changelog")) else { return vec![] };
    let level = heads[k].0;
    let stop = heads[k + 1..].iter().find(|h| h.0 <= level).map(|h| h.2).unwrap_or(html.len());
    items(&html[heads[k].3..stop])
}

/// Krita's post for a release. Posts are named with dots for recent releases and dashes for old
/// ones, under the year they came out. Returns the post address and its change list.
pub async fn krita(ver: &str, year: i32) -> Option<(String, Vec<String>)> {
    for y in [year, year - 1] {
        for name in [ver.to_string(), ver.replace('.', "-")] {
            let url = format!("https://krita.org/en/posts/{y}/krita-{name}-released/");
            if let Some(html) = page(&url).await {
                let lines = krita_changelog(&html);
                if !lines.is_empty() {
                    return Some((url, lines));
                }
            }
        }
    }
    None
}

// ---------------------------------------------------------------- any publisher's notes

/// Does `text` name version `ver`? "1.2" also counts for "1.2.0". Digits or dots right next to
/// it make it another version ("11.2.3", "1.2.30").
pub fn names_version(text: &str, ver: &str) -> bool {
    let base = ver.trim().trim_start_matches(['v', 'V']).to_string();
    let mut forms = vec![base.clone()];
    let mut short = base;
    while let Some(s) = short.strip_suffix(".0") {
        short = s.to_string();
        if short.contains('.') {
            forms.push(short.clone());
        }
    }
    let edge = |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_digit() || c == '.'));
    forms.iter().filter(|f| f.len() >= 2).any(|f| {
        let mut from = 0;
        while let Some(i) = text[from..].find(f.as_str()) {
            let (s, e) = (from + i, from + i + f.len());
            let before = text[..s].chars().last();
            let mut rest = text[e..].chars();
            let after = rest.next();
            // A full stop that ends a sentence ("Version 1.2.") still names 1.2.
            let after_ok = edge(after) || (after == Some('.') && rest.next().is_none_or(|c| !c.is_ascii_digit()));
            if edge(before) && after_ok {
                return true;
            }
            from = e;
        }
        false
    })
}

/// Release notes in plain text or markdown as short lines without markup: the bullet points, or
/// when there are none the lines of text. At most 12.
pub fn text_lines(t: &str) -> Vec<String> {
    lines_of(t, false)
}

fn lines_of(t: &str, bullets_only: bool) -> Vec<String> {
    let img = regex::Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap();
    let link = regex::Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap();
    let url = regex::Regex::new(r"\(?<?https?://[^\s)>]+>?\)?").unwrap();
    let numbered = regex::Regex::new(r"^\d{1,3}[.)]\s+(.*)$").unwrap();
    let handle = regex::Regex::new(r"\s+(by|thanks to)\s+@[\w-]+.*$").unwrap();
    // Markdown escapes ("async\_hooks") and the commit or pull request a line starts with
    // ("[b12bcc9ae1] - ", "80d23f64 ").
    let escaped = regex::Regex::new(r"\\([\[\]_*`#<>()])").unwrap();
    let commit = regex::Regex::new(r"^(\[[0-9a-f]{7,40}\]|[0-9a-f]{7,40}|#\d+)\s*(-\s*)?").unwrap();
    let clean = |s: &str| -> String {
        let s = escaped.replace_all(s, "$1");
        let s = img.replace_all(&s, "");
        let s = link.replace_all(&s, "$1");
        let s = url.replace_all(&s, "");
        let s = s.replace("**", "").replace("__", "").replace('`', "");
        let s = commit.replace(s.trim(), "");
        let s = handle.replace(&s, "");
        let s = strip_tags(&s);
        let s = s.trim().trim_start_matches(['-', '*', ':', ' ']).trim_end_matches(['.', ',', ';', ':', ' ']).to_string();
        s
    };
    let useful = |s: &String| {
        let low = s.to_lowercase();
        s.chars().filter(|c| c.is_alphanumeric()).count() >= 3 && !low.starts_with("full changelog") && !low.starts_with("what's changed") && !low.starts_with('@') && !low.starts_with("new contributors")
    };
    let bullet = |l: &str| -> Option<String> {
        for p in ["- ", "* ", "+ ", "\u{2022} ", "\u{b7} "] {
            if let Some(r) = l.strip_prefix(p) {
                return Some(r.to_string());
            }
        }
        numbered.captures(l).map(|c| c[1].to_string())
    };
    let lines: Vec<&str> = t.lines().map(str::trim).collect();
    let mut out: Vec<String> = Vec::new();
    for l in &lines {
        if let Some(b) = bullet(l) {
            let c = clean(&b);
            if useful(&c) && !out.contains(&c) {
                out.push(c);
            }
        }
        if out.len() >= MAX_LINES {
            return out;
        }
    }
    if !out.is_empty() || bullets_only {
        return out;
    }
    for l in &lines {
        if l.starts_with('#') || l.starts_with("===") || l.starts_with("---") || l.is_empty() {
            continue;
        }
        let c = clean(l);
        if useful(&c) && !out.contains(&c) {
            out.push(c);
        }
        if out.len() >= MAX_LINES {
            break;
        }
    }
    out
}

/// The part of a markdown changelog under the heading for version `ver`.
pub fn md_section(md: &str, ver: &str) -> Option<String> {
    let lines: Vec<&str> = md.lines().collect();
    let level = |l: &str| {
        let t = l.trim_start();
        let n = t.chars().take_while(|c| *c == '#').count();
        (n > 0 && n <= 6 && t[n..].starts_with(' ')).then_some(n)
    };
    let start = lines.iter().position(|l| level(l).is_some() && names_version(l, ver))?;
    let lv = level(lines[start]).unwrap_or(1);
    let end = lines[start + 1..].iter().position(|l| level(l).is_some_and(|n| n <= lv)).map(|i| start + 1 + i).unwrap_or(lines.len());
    let body = lines[start + 1..end].join("\n");
    (!body.trim().is_empty()).then_some(body)
}

/// The lines after a short line that names version `ver` ("6.7.3 (2026-05-26)", "Version 6.7.3"),
/// up to the next short line that names another version. For changelogs without headings.
pub fn text_section(text: &str, ver: &str) -> Option<String> {
    let other = regex::Regex::new(r"(^|[^\d.])v?\d+\.\d+([.\-]\d+)*").unwrap();
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    let is_head = |l: &str| !l.is_empty() && l.chars().count() <= 90 && !l.starts_with(['-', '*', '+', '\u{2022}']);
    let start = lines.iter().position(|l| is_head(l) && names_version(l, ver))?;
    let end = lines[start + 1..].iter().position(|l| is_head(l) && other.is_match(l) && !names_version(l, ver)).map(|i| start + 1 + i).unwrap_or(lines.len());
    let body = lines[start + 1..end].join("\n");
    (!body.trim().is_empty()).then_some(body)
}

/// HTML as text lines: one per block, list items as "- " bullets.
fn html_lines(html: &str) -> String {
    let li = regex::Regex::new(r"(?i)<li[^>]*>").unwrap();
    let block = regex::Regex::new(r"(?i)</?(p|div|br|h[1-6]|tr|dt|dd|ul|ol|summary|details|section|article|pre)[^>]*>").unwrap();
    let t = li.replace_all(html, "\n- ");
    let t = block.replace_all(&t, "\n");
    t.lines().map(strip_tags).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// Text paragraphs of an HTML fragment, first sentence of each.
fn paragraphs(html: &str) -> Vec<String> {
    let p = regex::Regex::new(r"(?s)<p[^>]*>(.*?)</p>").unwrap();
    p.captures_iter(html).map(|c| strip_tags(&c[1])).filter(|t| t.len() > 20).collect()
}

/// The change list for version `ver` on a publisher's release notes page: the list under the
/// heading that names the version, or the page's own list when the whole page is about that
/// version. A markdown or text file gives the section under its heading for the version.
pub fn page_notes(body: &str, ver: &str) -> Vec<String> {
    let head = &body[..body.len().min(2000)].to_ascii_lowercase();
    if !(head.trim_start().starts_with('<') || head.contains("<html") || head.contains("<body")) {
        if let Some(s) = md_section(body, ver) {
            return text_lines(&s);
        }
        return text_section(body, ver).map(|s| lines_of(&s, true)).unwrap_or_default();
    }
    let mut html = body.to_string();
    // Not <header>: release pages put each version's heading in one.
    for tag in ["script", "style", "nav", "svg", "noscript"] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}[\s>].*?</{tag}>")).unwrap();
        html = re.replace_all(&html, "").into_owned();
    }
    let heads = headings(&html);
    if let Some(k) = heads.iter().position(|h| names_version(&h.1, ver)) {
        let level = heads[k].0;
        let stop = heads[k + 1..].iter().find(|h| h.0 <= level).map(|h| h.2).unwrap_or(html.len());
        let block = &html[heads[k].3..stop];
        let lines = items(block);
        return if lines.is_empty() { paragraphs(block) } else { lines };
    }
    // A short paragraph that names the version, then its list.
    if let Some(s) = text_section(&html_lines(&html), ver) {
        let lines = lines_of(&s, true);
        if !lines.is_empty() {
            return lines;
        }
    }
    let title = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap().captures(&html).map(|c| strip_tags(&c[1])).unwrap_or_default();
    let h1 = heads.iter().find(|h| h.0 == 1).map(|h| h.1.clone()).unwrap_or_default();
    if names_version(&title, ver) || names_version(&h1, ver) {
        let main = ["<main", "<article"].iter().find_map(|t| html.find(t)).map(|i| &html[i..]).unwrap_or(&html);
        let lines = items(main);
        return if lines.is_empty() { paragraphs(main) } else { lines };
    }
    vec![]
}

/// A release notes page of any publisher (not cached: the notes read from it are).
pub async fn fetch_page(url: &str) -> Option<String> {
    let resp = http::CLIENT.get(url).timeout(Duration::from_secs(12)).send().await.ok()?;
    if !resp.status().is_success() || resp.content_length().is_some_and(|n| n > 4_000_000) {
        return None;
    }
    let b = resp.bytes().await.ok()?;
    Some(String::from_utf8_lossy(&b[..b.len().min(4_000_000)]).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn krita_post_changelog() {
        let post = r#"<html><title>Krita 5.3.4 Released! | Krita</title><ul><li>Download</li></ul><h1>Krita 5.3.4 Released!</h1><p>text</p>
<h2>Changelog</h2><ul><li>Fix layer thumbnails scaling when using display scaling</li><li>Fix openening large exr files (<a href="x">Bug 525361</a>)</li></ul>
<h2>Download 5.3.4</h2><ul><li>Not a note</li></ul></html>"#;
        assert_eq!(krita_changelog(post), vec!["Fix layer thumbnails scaling when using display scaling", "Fix openening large exr files (Bug 525361)"]);
        assert!(krita_changelog("<html><title>404 Page not found | Krita</title><h2>Changelog</h2><ul><li>x</li></ul>").is_empty());
        assert!(krita_changelog("<html><h1>No list here</h1></html>").is_empty());
    }

    #[test]
    fn reads_list_items_without_refs() {
        let html = r#"<ul> <li>Crash in BLI_mmap.cc on WIN32. (<a href="x">956fd000a2</a>)</li> <li>GLTF export ignores light color. (<a href="y">#156753</a>)</li></ul>"#;
        assert_eq!(items(html), vec!["Crash in BLI_mmap.cc on WIN32", "GLTF export ignores light color"]);
    }

    #[test]
    fn adobe_version_page() {
        let html = r#"<main><h1>Version 12.2</h1><h2>Major features</h2><h3>Improved Polygon Fill</h3><p>x</p>
<h3>New cage painting in Baking mode</h3><h2>Release Notes</h2><h3>12.2.0</h3><ul><li>Fixed a crash when baking. More words here.</li><li>Faster export</li></ul>
<h3>12.2.1</h3><ul><li>Fixed the viewport</li></ul><h3>Known issues</h3><ul><li>Not a note</li></ul></main>"#;
        // Whole items, as the page lists them.
        assert_eq!(adobe_from_version_page(html, "12.2.0").0, vec!["Fixed a crash when baking. More words here", "Faster export"]);
        assert_eq!(adobe_from_version_page(html, "12.2.1").0, vec!["Fixed the viewport"]);
        let dated = r#"<main><h2>Release Notes</h2><h3>12.1.5</h3><p>Release date: 2026/09/15</p><ul><li>Fixed a crash</li></ul></main>"#;
        assert_eq!(adobe_from_version_page(dated, "12.1.5").1.as_deref(), Some("2026-09-15T00:00:00Z"));
        // A first release without its own list falls back to the feature headlines.
        let no_list = r#"<main><h1>Version 12.2</h1><h2>Major features</h2><h3>Improved Polygon Fill</h3><h3>New cage painting</h3><h2>Release Notes</h2></main>"#;
        assert_eq!(adobe_from_version_page(no_list, "12.2.0").0, vec!["Improved Polygon Fill", "New cage painting"]);
    }

    #[test]
    fn finds_a_version_in_text() {
        assert!(names_version("Version 1.2.3 (2026-10-01)", "1.2.3"));
        assert!(names_version("## [v1.2.3] - today", "1.2.3"));
        assert!(names_version("Released 1.2.", "1.2.0"));
        assert!(!names_version("Version 11.2.3", "1.2.3"));
        assert!(!names_version("Version 1.2.30", "1.2.3"));
        assert!(!names_version("1.2.3.4", "1.2.3"));
    }

    #[test]
    fn plain_and_markdown_notes_become_short_lines() {
        let md = "## What's Changed\n* Fixed the [export](https://x.y/z) crash by @someone in https://github.com/o/r/pull/1\n* **Faster** start up.\n* \\[b12bcc9ae1] - async\\_hooks: add a scope\n* 80d23f64 (patch: Restore the entitlements)\n\n**Full Changelog**: https://github.com/o/r/compare/a...b";
        assert_eq!(text_lines(md), vec!["Fixed the export crash", "Faster start up", "async_hooks: add a scope", "(patch: Restore the entitlements)"]);
        let plain = "This release fixes a crash when saving.\nIt also updates translations.";
        assert_eq!(text_lines(plain), vec!["This release fixes a crash when saving", "It also updates translations"]);
        assert_eq!(text_lines("1. First fix\n2) Second fix"), vec!["First fix", "Second fix"]);
        assert!(text_lines("\n\n").is_empty());
    }

    #[test]
    fn notes_for_one_version_from_a_page() {
        let html = r#"<html><head><title>Changelog</title></head><body><nav><ul><li>Home</li></ul></nav>
<h2>Version 2.1.0</h2><ul><li>New export dialog</li><li>Fixed a crash when saving. More words here.</li></ul>
<h2>Version 2.0.9</h2><ul><li>Older fix</li></ul></body></html>"#;
        assert_eq!(page_notes(html, "2.1.0"), vec!["New export dialog", "Fixed a crash when saving. More words here"]);
        assert_eq!(page_notes(html, "2.0.9"), vec!["Older fix"]);
        assert!(page_notes(html, "3.0").is_empty(), "a page without the version gives nothing");
        let release = r#"<html><title>App 5.4 released</title><main><p>Intro text that is long enough to count.</p><ul><li>Better colors</li></ul></main></html>"#;
        assert_eq!(page_notes(release, "5.4"), vec!["Better colors"]);
        let inno = r#"<html><body><details open=""><summary>Tool 6.7</summary><p><a name="6.7.3"></a><span class="ver">6.7.3 </span><span class="date">(2026-05-26)</span></p><p><b>Using it at work?</b> Buy a license.</p><ul><li>Improved the startup</li><li>Fix: the example script</li></ul><p><a name="6.7.2"></a><span class="ver">6.7.2 </span></p><ul><li>Older</li></ul></details></body></html>"#;
        assert_eq!(page_notes(inno, "6.7.3"), vec!["Improved the startup", "Fix: the example script"]);
        assert_eq!(page_notes(inno, "6.7.2"), vec!["Older"]);
        let changelog = "# Changelog\n\n## 1.4.0\n- Added dark mode\n- Fixed login\n\n## 1.3.2\n- Old fix\n";
        assert_eq!(page_notes(changelog, "1.4.0"), vec!["Added dark mode", "Fixed login"]);
        assert_eq!(page_notes(changelog, "1.3.2"), vec!["Old fix"]);
        assert!(page_notes(changelog, "1.5").is_empty());
    }

    #[test]
    fn parses_dates() {
        assert_eq!(parse_date("September 15, 2026").as_deref(), Some("2026-09-15T00:00:00Z"));
        assert_eq!(parse_date("May 19th 2026").as_deref(), Some("2026-05-19T00:00:00Z"));
    }
}
