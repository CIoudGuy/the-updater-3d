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

/// Adobe notes for one version from the product's release notes page. Adobe uses two layouts:
/// one page per version (linked from the main page), or one long page with a heading per version.
pub async fn adobe(notes_url: &str, ver: &str) -> Option<Notes> {
    let html = page(notes_url).await?;
    let short = version::trim_like(ver, "1.2");
    let dashed = short.replace('.', "-");
    // 1. A link to a page for this version ("version-12-2.html").
    let link = regex::Regex::new(r#"href="([^"]*version-([0-9-]+)\.html)""#).unwrap();
    for c in link.captures_iter(&html) {
        if c[2] == dashed || c[2] == ver.replace('.', "-") {
            let mut url = c[1].to_string();
            if url.starts_with('/') {
                url = format!("https://{}{}", http::host(notes_url), url);
            }
            if let Some(vp) = page(&url).await {
                let body = vp.find("<main").map(|i| &vp[i..]).unwrap_or(&vp);
                let lines = items(body);
                if !lines.is_empty() {
                    return Some(Notes { date: None, lines });
                }
            }
        }
    }
    // 2. A heading that names this version, followed by its lists.
    let heading = regex::Regex::new(r"(?s)<h([1-4])[^>]*>(.*?)</h[1-4]>").unwrap();
    let heads: Vec<(usize, usize, String)> = heading.captures_iter(&html).map(|c| (c.get(0).unwrap().start(), c.get(0).unwrap().end(), strip_tags(&c[2]))).collect();
    for (k, (_, end, text)) in heads.iter().enumerate() {
        let words: Vec<&str> = text.split(|c: char| !(c.is_ascii_digit() || c == '.')).collect();
        if words.iter().any(|w| *w == ver || *w == short) {
            let stop = heads.get(k + 1).map(|h| h.0).unwrap_or(html.len());
            let lines = items(&html[*end..stop]);
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
    fn parses_dates() {
        assert_eq!(parse_date("September 15, 2026").as_deref(), Some("2026-09-15T00:00:00Z"));
        assert_eq!(parse_date("May 19th 2026").as_deref(), Some("2026-05-19T00:00:00Z"));
    }
}
