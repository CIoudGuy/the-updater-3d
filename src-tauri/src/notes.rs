//! Release notes read from the vendors' own pages (Blender and Adobe publish them as HTML).
//! Parsing is forgiving: when a page cannot be read the version list still works and links
//! to the full notes.

use crate::{http, version};
use std::collections::HashMap;
use std::time::Duration;

const MAX_LINES: usize = 12;

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
        if t.len() > 300 {
            t.truncate(297);
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

/// Shorten a long list item to its first sentence so the version list stays readable.
fn first_sentence(s: &str) -> String {
    let cut = s.char_indices().find(|(i, c)| *c == '.' && *i > 15 && s[i + 1..].starts_with(' ')).map(|(i, _)| i);
    let mut t = match cut {
        Some(i) => s[..i].to_string(),
        None => s.to_string(),
    };
    if t.chars().count() > 160 {
        t = t.chars().take(157).collect::<String>() + "...";
    }
    t
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
        let lines: Vec<String> = items(block).into_iter().map(|l| first_sentence(&l)).collect();
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
            let lines: Vec<String> = items(&html[h.3..stop]).into_iter().map(|l| first_sentence(&l)).collect();
            if !lines.is_empty() {
                return Some(Notes { date: None, lines });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(adobe_from_version_page(html, "12.2.0").0, vec!["Fixed a crash when baking", "Faster export"]);
        assert_eq!(adobe_from_version_page(html, "12.2.1").0, vec!["Fixed the viewport"]);
        let dated = r#"<main><h2>Release Notes</h2><h3>12.1.5</h3><p>Release date: 2026/09/15</p><ul><li>Fixed a crash</li></ul></main>"#;
        assert_eq!(adobe_from_version_page(dated, "12.1.5").1.as_deref(), Some("2026-09-15T00:00:00Z"));
        // A first release without its own list falls back to the feature headlines.
        let no_list = r#"<main><h1>Version 12.2</h1><h2>Major features</h2><h3>Improved Polygon Fill</h3><h3>New cage painting</h3><h2>Release Notes</h2></main>"#;
        assert_eq!(adobe_from_version_page(no_list, "12.2.0").0, vec!["Improved Polygon Fill", "New cage painting"]);
    }

    #[test]
    fn parses_dates() {
        assert_eq!(parse_date("September 15, 2026").as_deref(), Some("2026-09-15T00:00:00Z"));
        assert_eq!(parse_date("May 19th 2026").as_deref(), Some("2026-05-19T00:00:00Z"));
    }
}
