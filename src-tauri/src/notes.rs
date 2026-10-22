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

// Change lists are plain lines. A line starting with "# ", "## " or "### " is a section heading
// (biggest first); the window shows those as headings and every other line as a bullet.

/// A page fragment in reading order: its h2/h3/h4 headings as "## " / "### " lines and every
/// list item and paragraph as a line, with commit and issue references removed.
pub fn outline(fragment: &str) -> Vec<String> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static TOKEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<(h[2-4]|li|p)\b[^>]*>(.*?)</(?:h[2-4]|li|p)>").unwrap());
    static REFS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*[\(\[]\s*(?:(?:#\d+|[0-9a-f]{7,12})\s*,?\s*)+[\)\]]").unwrap());
    let mut out: Vec<String> = Vec::new();
    for c in TOKEN.captures_iter(fragment) {
        let text = strip_tags(&c[2]);
        let text = REFS.replace_all(&text, "").trim().trim_end_matches('.').trim().to_string();
        if text.is_empty() {
            continue;
        }
        let line = match &c[1] {
            "h2" => format!("## {text}"),
            "h3" | "h4" => format!("### {text}"),
            // Image captions ("Before", "After") and other scraps are not notes.
            _ if text.split_whitespace().count() < 3 => continue,
            _ => text,
        };
        if out.last() != Some(&line) {
            out.push(line);
        }
        if out.len() >= MAX_LINES {
            break;
        }
    }
    tidy_headings(out)
}

/// Drops headings with nothing under them (a heading followed by a heading of the same or a
/// bigger level, or by the end of the list), so empty sections do not show.
pub fn tidy_headings(lines: Vec<String>) -> Vec<String> {
    let level = |s: &str| s.chars().take_while(|c| *c == '#').count();
    let mut keep = vec![true; lines.len()];
    // From the end, so a section whose subsections were all dropped is dropped too.
    let mut next_kept: Option<usize> = None;
    for i in (0..lines.len()).rev() {
        let l = &lines[i];
        if level(l) > 0 && l.chars().nth(level(l)) == Some(' ') {
            let has_body = next_kept.is_some_and(|n| level(&lines[n]) == 0 || level(&lines[n]) > level(l));
            if !has_body {
                keep[i] = false;
                continue;
            }
        }
        next_kept = Some(i);
    }
    let mut out: Vec<String> = lines.into_iter().zip(keep).filter(|(_, k)| *k).map(|(l, _)| l).collect();
    let is_heading = |s: &str| level(s) > 0 && s.chars().nth(level(s)) == Some(' ');
    // One heading on top of everything ("What's Changed") says nothing.
    if out.iter().filter(|l| is_heading(l)).count() == 1 && out.first().is_some_and(|l| is_heading(l)) {
        out.remove(0);
    }
    // Top sections show as "## " whatever level the page used ("#### Improvements"); "# " stays
    // for lists that group whole topics (Blender's release notes).
    if let Some(min) = out.iter().filter(|l| is_heading(l)).map(|l| level(l)).min().filter(|m| *m > 2) {
        for l in out.iter_mut().filter(|l| is_heading(l)) {
            let lv = level(l) - (min - 2);
            *l = format!("{} {}", "#".repeat(lv.min(3)), l[level(l)..].trim());
        }
    }
    out
}

/// The full notes of a new Blender series (5.2.0): the release notes page's own lists
/// (compatibility and the like) and every topic page it links to, as "# Topic" sections.
pub async fn blender_major(series: &str) -> Notes {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static ARTICLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<article[^>]*>(.*?)</article>").unwrap());
    static TOPIC: Lazy<Regex> = Lazy::new(|| Regex::new(r#"<a[^>]*href="([a-z_\-]+/)"[^>]*>([^<]{2,80})</a>"#).unwrap());
    static RELEASED: Lazy<Regex> = Lazy::new(|| Regex::new(r"released on ([A-Z][a-z]+ \d{1,2}(?:st|nd|rd|th)?,? \d{4})").unwrap());
    let base = format!("https://developer.blender.org/docs/release_notes/{series}/");
    let Some(html) = page(&base).await else { return Notes { date: None, lines: vec![] } };
    let article = ARTICLE.captures(&html).map(|c| c[1].to_string()).unwrap_or_default();
    let date = RELEASED.captures(&strip_tags(&article)).and_then(|c| parse_date(&c[1]));
    let topics: Vec<(String, String)> = TOPIC
        .captures_iter(&article)
        .map(|c| (c[1].to_string(), strip_tags(&c[2])))
        .filter(|(href, _)| href != "bugfixes/" && href != "corrective_releases/")
        .fold(Vec::new(), |mut acc, t| {
            if !acc.iter().any(|(h, _): &(String, String)| *h == t.0) {
                acc.push(t);
            }
            acc
        });
    let link_texts: Vec<String> = TOPIC.captures_iter(&article).map(|c| strip_tags(&c[2])).collect();
    let mut lines: Vec<String> = Vec::new();
    // The page's own sections (Compatibility...): not its intro ("was released on...", a link to
    // blender.org) nor its lists of topic links.
    let mut in_section = false;
    for l in outline(&article) {
        if l.starts_with("## ") {
            in_section = !l.starts_with("## Bugfixes");
            if in_section {
                lines.push(l.replacen("## ", "# ", 1));
            }
            continue;
        }
        if in_section && !link_texts.iter().any(|t| *t == l) {
            lines.push(l);
        }
    }
    let pages = futures_util::future::join_all(topics.iter().map(|(href, _)| page_owned(format!("{base}{href}")))).await;
    for ((_, name), html) in topics.iter().zip(pages) {
        let Some(html) = html else { continue };
        let body = ARTICLE.captures(&html).map(|c| c[1].to_string()).unwrap_or_default();
        let items = outline(&body);
        if items.is_empty() {
            continue;
        }
        lines.push(format!("# {name}"));
        // A topic's single top heading repeats the topic name ("Modeling" under "Modeling & UV").
        let one_top = items.iter().filter(|l| l.starts_with("## ")).count() == 1 && items.first().is_some_and(|l| l.starts_with("## "));
        lines.extend(items.into_iter().filter(|l| !(one_top && l.starts_with("## ") && name.starts_with(l.trim_start_matches("## ")))));
    }
    lines.truncate(MAX_LINES * 3);
    Notes { date, lines }
}

async fn page_owned(url: String) -> Option<String> {
    page(&url).await
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
pub async fn blender(series: &str, with_major: bool) -> HashMap<String, Notes> {
    let mut out = blender_fixes(series).await;
    // The first release of the series has no fix list: its notes are the whole release notes.
    let first = format!("{series}.0");
    if with_major && !out.contains_key(&first) {
        let m = blender_major(series).await;
        if !m.lines.is_empty() {
            out.insert(first, m);
        }
    }
    out
}

async fn blender_fixes(series: &str) -> HashMap<String, Notes> {
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
    fn release_pages_keep_their_sections() {
        // Shape of a Blender topic page.
        let html = r#"<h1>Blender 5.2 LTS: Modeling &amp; UV&para;</h1><h2>Modeling&para;</h2><h3>General&para;</h3><ul><li>"Circle" from the Loop Tools add-on is now built in as "To Circle".
(<a href="x">d2fabc952a</a>)</li></ul><h3>3D Text&para;</h3><ul><li><p>Improved support for combining characters (<a>43e52c3892</a>, <a>73fa859c1f</a>).</p></li></ul><p>Before</p><h3>Empty&para;</h3><h2>UV&para;</h2><ul><li>Add Select by Winding operator (<a>1b97e5afe6</a>).</li></ul>"#;
        assert_eq!(
            outline(html),
            vec![
                "## Modeling",
                "### General",
                "\"Circle\" from the Loop Tools add-on is now built in as \"To Circle\"",
                "### 3D Text",
                "Improved support for combining characters",
                "## UV",
                "Add Select by Winding operator",
            ]
        );
        // Sections left empty go, including a parent whose children all went.
        let t = tidy_headings(vec!["# A".into(), "## B".into(), "# C".into(), "x".into(), "# D".into(), "y".into()]);
        assert_eq!(t, vec!["# C", "x", "# D", "y"]);
        // A lone heading on top of everything goes; deep headings are lifted to "## ".
        assert_eq!(tidy_headings(vec!["## What's Changed".into(), "x".into()]), vec!["x"]);
        assert_eq!(tidy_headings(vec!["#### Fixes".into(), "x".into(), "#### Improvements".into(), "y".into()]), vec!["## Fixes", "x", "## Improvements", "y"]);
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
    fn parses_dates() {
        assert_eq!(parse_date("September 15, 2026").as_deref(), Some("2026-09-15T00:00:00Z"));
        assert_eq!(parse_date("May 19th 2026").as_deref(), Some("2026-05-19T00:00:00Z"));
    }
}
