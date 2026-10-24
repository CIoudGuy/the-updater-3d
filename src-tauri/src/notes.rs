//! Release notes read from the vendors' own pages (Blender and Adobe publish them as HTML).
//! Parsing is forgiving: when a page cannot be read the version list still works and links
//! to the full notes.

use crate::{http, version};
use std::collections::HashMap;
use std::time::Duration;

/// The whole published list; only a runaway page is cut.
const MAX_LINES: usize = 400;

#[derive(Default)]
pub struct Notes {
    pub date: Option<String>,
    pub lines: Vec<String>,
    /// The page of this version's own notes, when it has one (a forum post, a news item).
    pub url: Option<String>,
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
    let lines = without_boilerplate(lines);
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

/// Sections that list no changes: package and image lists, older versions, sign-offs.
const BOILERPLATE: [&str; 10] = ["packages updated", "new contributors", "packages updated in this release", "prior versions", "previous versions", "older versions", "docker images", "checksums", "sha256 checksums", "contributors"];

/// The lines without boilerplate sections (a heading from `BOILERPLATE` and everything under it).
fn without_boilerplate(lines: Vec<String>) -> Vec<String> {
    let level = |s: &str| s.chars().take_while(|c| *c == '#').count();
    let mut out = Vec::with_capacity(lines.len());
    let mut skip: Option<usize> = None;
    for l in lines {
        let lv = level(&l);
        let heading = lv > 0 && l.chars().nth(lv) == Some(' ');
        if heading {
            if skip.is_some_and(|s| lv > s) {
                continue;
            }
            let text = l[lv..].trim().trim_end_matches(':').to_ascii_lowercase();
            if BOILERPLATE.contains(&text.as_str()) {
                skip = Some(lv);
                continue;
            }
            skip = None;
        } else if skip.is_some() {
            continue;
        }
        out.push(l);
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
    let Some(html) = page(&base).await else { return Notes::default() };
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
    Notes { date, lines, url: None }
}

async fn page_owned(url: String) -> Option<String> {
    page(&url).await
}

pub(crate) fn strip_tags(s: &str) -> String {
    let re = regex::Regex::new(r"<[^>]+>").unwrap();
    let t = re.replace_all(s, "");
    // Named and numeric entities ("&#8216;Lattice Deform&#8217;"), then no long dashes.
    let t = unescape_html(&t.replace("&para;", "")).replace('\u{2014}', ", ").replace('\u{2013}', "-");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A line without the bare issue and commit references it ends with ("... name conflict #106584",
/// "... OpenPGL 0.5.0 85c7cc898d", "[bcfe4ce3b1600931fc854f70f5146a8bb2b36a4a]").
fn without_trailing_refs(s: &str) -> &str {
    let is_ref = |w: &str| {
        let w = w.trim_matches(|c| matches!(c, '(' | ')' | '[' | ']' | ','));
        let hex = (7..=40).contains(&w.len()) && w.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)) && w.chars().any(|c| c.is_ascii_digit()) && w.chars().any(|c| c.is_ascii_alphabetic());
        let issue = w.len() > 1 && w.starts_with('#') && w[1..].chars().all(|c| c.is_ascii_digit());
        hex || issue
    };
    let mut t = s.trim_end();
    while let Some((head, last)) = t.rsplit_once(char::is_whitespace) {
        if !is_ref(last) {
            break;
        }
        t = head.trim_end();
    }
    t
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
        let t = refs.replace(&t, "");
        let t = without_trailing_refs(&t).trim_end_matches('.').to_string();
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

/// A page, kept on disk for 12 hours (every version of an app asks for the same pages).
pub async fn page(url: &str) -> Option<String> {
    let f = tokio::time::timeout(Duration::from_secs(12), http::get_cached(url, &[], Duration::from_secs(12 * 3600))).await.ok()?.ok()?;
    Some(String::from_utf8_lossy(&f.body).into_owned())
}

/// Blender bug-fix lists for one series, keyed by full version ("4.5.14"), with the full release
/// notes for the first release (x.y.0) when `with_major` is set.
pub async fn blender(series: &str, with_major: bool) -> HashMap<String, Notes> {
    blender_with_source(series, with_major).await.0
}

/// Like `blender`, plus the address of the page the fix lists came from.
pub async fn blender_with_source(series: &str, with_major: bool) -> (HashMap<String, Notes>, Option<String>) {
    let (mut out, src) = blender_fixes(series).await;
    // The first release of the series has no fix list: its notes are the whole release notes.
    let first = format!("{series}.0");
    if with_major && !out.contains_key(&first) {
        let m = blender_major(series).await;
        if !m.lines.is_empty() {
            out.insert(first, m);
        }
    }
    (out, src)
}

/// The releases of a blender.org release or LTS page: "Blender 4.5.14 LTS<span>September 15,
/// 2026</span>" or "Blender 4.2.23 LTS &#8211; July 21, 2026" summaries, each followed by its
/// Download and Changelog lists. A release whose changelog is only a link (5.2.0) is left out.
pub fn blender_summaries(html: &str) -> HashMap<String, Notes> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static ENTRY: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<summary[^>]*>(.*?)</summary>(.*?)</details>").unwrap());
    static VER: Lazy<Regex> = Lazy::new(|| Regex::new(r"Blender\s+([0-9]+\.[0-9]+\.[0-9]+)").unwrap());
    static WHEN: Lazy<Regex> = Lazy::new(|| Regex::new(r"([A-Z][a-z]+ \d{1,2},? \d{4})").unwrap());
    let mut out = HashMap::new();
    for c in ENTRY.captures_iter(html) {
        let head = strip_tags(&c[1].replace("&#8211;", " "));
        let Some(v) = VER.captures(&head) else { continue };
        let body = &c[2];
        let body = body.find("Changelog").map(|i| &body[i..]).unwrap_or(body);
        let lines = items(body);
        if !lines.is_empty() {
            out.insert(v[1].to_string(), Notes { date: WHEN.captures(&head).and_then(|d| parse_date(&d[1])), lines, url: None });
        }
    }
    out
}

/// The releases of a docs "corrective releases" page: an h1 or h2 "Blender 3.5.1" per release.
pub fn blender_corrective(html: &str) -> HashMap<String, Notes> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static HEAD: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?s)<h[12][^>]*id="blender-\d+"[^>]*>\s*Blender\s+([0-9.]+)"#).unwrap());
    static RELEASED: Lazy<Regex> = Lazy::new(|| Regex::new(r"Released on ([A-Za-z]+ \d+(?:st|nd|rd|th)?,? \d{4})").unwrap());
    let starts: Vec<(usize, String)> = HEAD.captures_iter(html).map(|c| (c.get(0).unwrap().start(), c[1].trim_end_matches('.').to_string())).collect();
    let mut out = HashMap::new();
    for (k, (pos, ver)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map(|s| s.0).unwrap_or(html.len());
        let block = &html[*pos..end];
        let lines = items(block);
        if !lines.is_empty() {
            out.insert(ver.clone(), Notes { date: RELEASED.captures(block).and_then(|c| parse_date(&c[1])), lines, url: None });
        }
    }
    out
}

/// The fix lists from the first page that has them: blender.org/releases/<x-y>/ (4.5 and newer),
/// the LTS page, then the docs' corrective releases page.
async fn blender_fixes(series: &str) -> (HashMap<String, Notes>, Option<String>) {
    let dash = series.replace('.', "-");
    let mut pages = vec![];
    if !version::newer("4.5", series) {
        pages.push(format!("https://www.blender.org/releases/{dash}/"));
    }
    pages.push(format!("https://www.blender.org/download/lts/{dash}/"));
    for url in pages {
        if let Some(html) = page(&url).await {
            let found = blender_summaries(&html);
            if !found.is_empty() {
                return (found, Some(url));
            }
        }
    }
    let url = format!("https://developer.blender.org/docs/release_notes/{series}/corrective_releases/");
    if let Some(html) = page(&url).await {
        let found = blender_corrective(&html);
        if !found.is_empty() {
            return (found, Some(url));
        }
    }
    (HashMap::new(), None)
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

/// A page that many versions read at once (25 versions of one app are filled in together): one
/// fetch per address at a time, kept for ten minutes, so the others wait for it instead of
/// fetching it again. A failed fetch is kept for a minute.
type PageSlot = std::sync::Arc<tokio::sync::Mutex<Option<(std::time::Instant, Option<std::sync::Arc<String>>)>>>;
static PAGES: once_cell::sync::Lazy<parking_lot::Mutex<HashMap<String, PageSlot>>> = once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));

/// Forget fetched notes pages (whole web pages) so a closed app holds no more memory than it needs.
pub fn clear_page_cache() {
    PAGES.lock().clear();
}

/// Keep at most `cap` pages (as many as this PC is tuned for): pages past their time go first,
/// then the oldest. Pages being fetched or read stay.
fn prune_pages(pages: &mut HashMap<String, PageSlot>, cap: usize, now: std::time::Instant) {
    if pages.len() < cap {
        return;
    }
    let mut free: Vec<(String, Option<std::time::Instant>)> = pages
        .iter()
        .filter_map(|(k, s)| {
            let held = s.try_lock().ok()?;
            let at = held.as_ref().map(|(at, body)| (*at, body.is_some()));
            let stale = at.is_none_or(|(at, ok)| now.saturating_duration_since(at) >= Duration::from_secs(if ok { 600 } else { 60 }));
            Some((k.clone(), if stale { None } else { at.map(|(at, _)| at) }))
        })
        .collect();
    // Stale ones (None) first, then by age.
    free.sort_by_key(|(_, at)| *at);
    let extra = pages.len() + 1 - cap;
    for (i, (k, at)) in free.into_iter().enumerate() {
        if at.is_some() && i >= extra {
            break;
        }
        pages.remove(&k);
    }
}

async fn page_shared(url: &str) -> Option<std::sync::Arc<String>> {
    use std::time::Instant;
    let slot = {
        let mut pages = PAGES.lock();
        if !pages.contains_key(url) {
            prune_pages(&mut pages, crate::machine::tuned().note_pages, Instant::now());
        }
        pages.entry(url.to_string()).or_default().clone()
    };
    let mut held = slot.lock().await;
    if let Some((at, body)) = held.as_ref() {
        let keep = if body.is_some() { 600 } else { 60 };
        if at.elapsed() < Duration::from_secs(keep) {
            return body.clone();
        }
    }
    let body = page(url).await.map(std::sync::Arc::new);
    *held = Some((Instant::now(), body.clone()));
    body
}

/// Where `ver` stands in `text` as a whole version: "27.11" also written "v27.11", "16.0.0" also
/// written "16.0". Not inside a longer version ("27.11.1", "127.11") or a build ("27.0x41").
/// The byte range of the match.
fn version_at(text: &str, ver: &str) -> Option<(usize, usize)> {
    let mut forms = vec![ver.trim().to_string()];
    let mut short = forms[0].clone();
    while let Some(s) = short.strip_suffix(".0").filter(|s| s.contains('.')) {
        short = s.to_string();
        forms.push(short.clone());
    }
    for f in &forms {
        let mut from = 0;
        while let Some(i) = text[from..].find(f.as_str()) {
            let (s, e) = (from + i, from + i + f.len());
            from = e;
            let mut before = text[..s].chars().rev();
            let b = match before.next() {
                Some('v' | 'V') => before.next(),
                c => c,
            };
            let mut after = text[e..].chars();
            let a = after.next();
            let before_ok = b.is_none_or(|c| !c.is_alphanumeric() && c != '.');
            let after_ok = match a {
                None => true,
                Some('.') => after.next().is_none_or(|c| !c.is_ascii_digit()),
                Some(c) => !c.is_alphanumeric(),
            };
            if before_ok && after_ok {
                return Some((s, e));
            }
        }
    }
    None
}

/// One announcement in an Adobe community category.
#[derive(Clone, Debug, Default)]
pub struct Topic {
    pub title: String,
    pub published: Option<String>,
    pub url: Option<String>,
    pub content: String,
}

/// The topics of one page of an Adobe community category ("community.adobe.com/announcements-710")
/// and the category's number of pages. The page carries its list as JSON in the `data-props`
/// attribute of the CategoryTopicList module, with each post's full HTML in `content`.
pub fn community_page(html: &str) -> Option<(Vec<Topic>, u32)> {
    let at = html.find(r#"data-preact="destination/modules/Content/TopicList/CategoryTopicList""#)?;
    let tag_end = html[at..].find('>').map(|i| at + i)?;
    let tag = &html[at..tag_end];
    let props = tag.find(r#"data-props=""#).map(|i| &tag[i + 12..])?;
    let props = &props[..props.find('"')?];
    let v: serde_json::Value = serde_json::from_str(&unescape_html(props)).ok()?;
    let topics = v["topics"]
        .as_array()?
        .iter()
        .map(|t| Topic {
            title: t["title"].as_str().unwrap_or("").to_string(),
            published: t["publishedAt"].as_str().and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok()).map(|d| d.with_timezone(&chrono::Utc).format("%Y-%m-%dT%H:%M:%SZ").to_string()),
            url: t["topicUrl"]["destination"].as_str().map(String::from),
            content: t["content"].as_str().unwrap_or("").to_string(),
        })
        .collect();
    Some((topics, v["totalPageCount"].as_u64().unwrap_or(1) as u32))
}

/// HTML entities of an attribute value: named ones and numeric (&#34; &#x22;).
fn unescape_html(s: &str) -> String {
    use once_cell::sync::Lazy;
    static ENTITY: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"&(#[0-9]+|#[xX][0-9a-fA-F]+|[a-zA-Z]+);").unwrap());
    ENTITY
        .replace_all(s, |c: &regex::Captures| {
            let e = &c[1];
            let ch = if let Some(h) = e.strip_prefix("#x").or_else(|| e.strip_prefix("#X")) {
                u32::from_str_radix(h, 16).ok().and_then(char::from_u32)
            } else if let Some(d) = e.strip_prefix('#') {
                d.parse().ok().and_then(char::from_u32)
            } else {
                match e {
                    "quot" => Some('"'),
                    "amp" => Some('&'),
                    "lt" => Some('<'),
                    "gt" => Some('>'),
                    "apos" => Some('\''),
                    "nbsp" => Some('\u{a0}'),
                    "ndash" => Some('\u{2013}'),
                    "mdash" => Some('\u{2014}'),
                    "hellip" => Some('\u{2026}'),
                    "lsquo" => Some('\u{2018}'),
                    "rsquo" => Some('\u{2019}'),
                    "ldquo" => Some('\u{201c}'),
                    "rdquo" => Some('\u{201d}'),
                    _ => None,
                }
            };
            ch.map(String::from).unwrap_or_else(|| c[0].to_string())
        })
        .into_owned()
}

/// How well an announcement title names release `ver` of the app called `names`: None when it
/// does not name that exact version, or is about something else ("27.10 and above", a beta of a
/// release app, system requirements). Higher when the app name stands right before the version
/// ("Photoshop v27.6", "Adobe Premiere 26.5") and when the title starts with it.
fn community_score(title: &str, ver: &str, names: &[&str], beta: bool) -> Option<u8> {
    let low = title.to_lowercase();
    let (s, e) = version_at(&low, ver)?;
    let after = low[e..].trim_start();
    if ["and above", "and later", "or later", "and newer", "or newer", "+"].iter().any(|w| after.starts_with(w)) {
        return None;
    }
    if (!beta && low.contains("beta")) || ["sdk", "requirement", "no longer", "removed", "retire"].iter().any(|w| low.contains(w)) {
        return None;
    }
    let before = low[..s].trim_end().trim_end_matches(['v', '(']).trim_end().trim_end_matches("version").trim_end();
    let start = low.trim_start_matches(|c: char| !c.is_alphanumeric());
    let start = start.strip_prefix("adobe ").unwrap_or(start);
    let mut score = 1;
    if names.iter().any(|n| before.ends_with(&n.to_lowercase())) {
        score += 2;
    }
    if names.iter().any(|n| start.starts_with(&n.to_lowercase())) {
        score += 1;
    }
    (score >= 2).then_some(score)
}

/// The announcement of release `ver`: the best scoring title, the earliest post among equals
/// (the release post comes before the follow-up articles).
pub fn community_pick<'a>(topics: &'a [Topic], ver: &str, names: &[&str], beta: bool) -> Option<&'a Topic> {
    // Posts name releases by up to three parts; Creative Cloud's newest is a full build ("6.11.0.478.6").
    let ver = &version::trim_like(ver, "1.2.3");
    topics
        .iter()
        .filter_map(|t| community_score(&t.title, ver, names, beta).map(|s| (s, t)))
        .max_by(|(a, x), (b, y)| a.cmp(b).then_with(|| y.published.cmp(&x.published)))
        .map(|(_, t)| t)
}

/// An announcement's body as notes: its sections and items, without the closing "Download",
/// "Join the conversation" and feedback parts.
pub fn community_body(content: &str) -> Vec<String> {
    const END: &[&str] = &["join the conversation", "share your feedback", "get started", "how to update", "we want to hear", "tell us what you think"];
    const SKIP: &[&str] = &["download", "click here", "learn more", "to get ", "to update", "update to ", "if you found this", "for the full list", "for more details", "note: as always", "read more"];
    let mut out = Vec::new();
    for l in outline(content) {
        // Also past a leading emoji ("👉 Download the latest version").
        let low = l.trim_start_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
        if END.iter().any(|w| low.starts_with(w)) {
            break;
        }
        if !SKIP.iter().any(|w| low.starts_with(w)) {
            out.push(l);
        }
    }
    tidy_headings(out)
}

/// The announcement of `ver` in community category `id`: read page by page (at most three) until
/// it turns up. Pages are shared by all the versions of the app and kept for half an hour.
async fn community(id: u32, ver: &str) -> Option<Notes> {
    use once_cell::sync::Lazy;
    use std::sync::Arc;
    use std::time::Instant;
    const PAGES: u32 = 3;
    #[derive(Default)]
    struct Forum {
        topics: Vec<Topic>,
        read: u32,
        total: u32,
        at: Option<Instant>,
    }
    static FORUMS: Lazy<parking_lot::Mutex<HashMap<u32, Arc<tokio::sync::Mutex<Forum>>>>> = Lazy::new(|| parking_lot::Mutex::new(HashMap::new()));
    let (names, beta) = crate::sources::adobe::community_names(id)?;
    let slot = FORUMS.lock().entry(id).or_default().clone();
    let mut f = slot.lock().await;
    if f.at.is_none_or(|t| t.elapsed() > Duration::from_secs(1800)) {
        *f = Forum { at: Some(Instant::now()), total: 1, ..Default::default() };
    }
    loop {
        if let Some(t) = community_pick(&f.topics, ver, names, beta) {
            let lines = community_body(&t.content);
            return Some(Notes { date: t.published.clone(), lines, url: t.url.clone() }).filter(|n| !n.lines.is_empty());
        }
        if f.read >= f.total.min(PAGES) {
            return None;
        }
        // Plain category pages only: the site's robots rules exclude its search and sort links.
        let url = match f.read {
            0 => format!("https://community.adobe.com/announcements-{id}"),
            n => format!("https://community.adobe.com/announcements-{id}/index{}.html", n + 1),
        };
        f.read += 1;
        // Not kept on disk (about 2 MB a page): the topics stay in memory, the notes in core's cache.
        let Some((topics, total)) = fetch_page(&url).await.and_then(|h| community_page(&h)) else {
            // Tried again in a minute rather than in half an hour.
            f.at = Instant::now().checked_sub(Duration::from_secs(1740));
            return None;
        };
        f.total = total.max(1);
        f.topics.extend(topics);
    }
}

/// Steam news (BBCode) as markdown-like text for `text_lines`: list items as "- " lines, headings
/// as "## " lines, every other tag dropped.
fn bbcode_text(s: &str) -> String {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static HEAD: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)\[h[1-3]\](.*?)\[/h[1-3]\]").unwrap());
    static IMG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)\[(img|previewyoutube|video)[^\]]*\].*?\[/(img|previewyoutube|video)\]").unwrap());
    static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[/?[a-zA-Z0-9]+(=[^\]]*)?\]").unwrap());
    let t = HEAD.replace_all(s, "\n## $1\n");
    let t = IMG.replace_all(&t, "");
    let t = t.replace("[*]", "\n- ").replace("[/*]", "\n").replace("[/p]", "\n").replace("[br]", "\n");
    let t = t.replace("[list]", "\n").replace("[/list]", "\n").replace("[olist]", "\n").replace("[/olist]", "\n");
    TAG.replace_all(&t, "").into_owned()
}

/// The Steam news item for release `ver` (a title such as "Modeler 1.22.7" or "1.18 Release"),
/// the longest when several name it.
pub fn steam_news(json: &str, ver: &str) -> Option<Notes> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let item = v["appnews"]["newsitems"]
        .as_array()?
        .iter()
        .filter(|n| version_at(n["title"].as_str().unwrap_or(""), ver).is_some())
        .max_by_key(|n| n["contents"].as_str().map_or(0, str::len))?;
    let lines = text_lines(&bbcode_text(item["contents"].as_str().unwrap_or("")));
    if lines.is_empty() {
        return None;
    }
    let date = item["date"].as_i64().and_then(|d| chrono::DateTime::from_timestamp(d, 0)).map(|d| d.format("%Y-%m-%dT%H:%M:%SZ").to_string());
    // Not the item's own address: Steam gives a CDN redirect; the store's news page stays the link.
    Some(Notes { date, lines, url: None })
}

/// The section for `ver` of a Substance 3D "All changes" page in markdown: "### **6.0.4**" (or
/// "### 5.1.0 ILE FLOTTANTE"), a release date, then "**Added:**" / "**Fixed:**" blocks of
/// "\[Area] text" lines or ASCII table "List" cells, as "## Added" sections and lines.
pub fn substance_changes(md: &str, ver: &str) -> Option<Notes> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static RELEASED: Lazy<Regex> = Lazy::new(|| Regex::new(r"Released:?\s*([A-Z][a-z]+ \d{1,2}(?:st|nd|rd|th)?,? \d{4})").unwrap());
    static YMD: Lazy<Regex> = Lazy::new(|| Regex::new(r"Release date:\W*(\d{4})/(\d{1,2})/(\d{1,2})").unwrap());
    static BOLD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\*\*([^*]{2,60}?):?\*\*:?$").unwrap());
    static ESCAPE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\([\[\]_*`#<>()+\-.!|~])").unwrap());
    static IMG: Lazy<Regex> = Lazy::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
    let lines: Vec<&str> = md.lines().collect();
    let heading_ver = |l: &str| -> Option<String> {
        let t = l.strip_prefix("### ")?.replace("**", "");
        t.split_whitespace().next().filter(|w| is_version(w)).map(String::from)
    };
    let start = lines.iter().position(|l| heading_ver(l).is_some_and(|v| version::cmp(&v, ver).is_eq()))?;
    let end = lines[start + 1..].iter().position(|l| l.starts_with('#')).map(|i| start + 1 + i).unwrap_or(lines.len());
    let mut date = None;
    let mut out: Vec<String> = Vec::new();
    // An item of a table cell can wrap onto the next row.
    let mut in_item = false;
    for raw in &lines[start + 1..end] {
        let l = raw.trim();
        let table = l.starts_with('|');
        let cell = l.trim_matches('|').trim().trim_end_matches('\\').trim();
        if cell.is_empty() || l.starts_with("+-") || l.starts_with("+=") || l == "---" || cell == "List" {
            in_item = false;
            continue;
        }
        if date.is_none() {
            if let Some(c) = RELEASED.captures(cell) {
                date = parse_date(&c[1]);
                continue;
            }
            if let Some(c) = YMD.captures(cell) {
                date = Some(format!("{}-{:0>2}-{:0>2}T00:00:00Z", &c[1], &c[2], &c[3]));
                continue;
            }
        }
        if cell.starts_with("Summary:") {
            continue;
        }
        if let Some(c) = BOLD.captures(cell) {
            out.push(format!("## {}", c[1].trim()));
            in_item = false;
            continue;
        }
        let text = IMG.replace_all(cell, "");
        let text = LINK.replace_all(&text, "$1");
        let text = ESCAPE.replace_all(&text, "$1").replace("**", "");
        if let Some(item) = text.strip_prefix("- ").or_else(|| text.strip_prefix("* ")) {
            out.push(item.trim().to_string());
            in_item = table;
        } else if table && in_item {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(text.trim());
            }
        } else {
            out.push(text.trim().to_string());
            in_item = false;
        }
    }
    let out: Vec<String> = out.into_iter().map(|l| strip_tags(&l).trim_end_matches('.').trim().to_string()).filter(|l| !l.is_empty()).take(MAX_LINES).collect();
    let out = tidy_headings(out);
    (!out.is_empty()).then_some(Notes { date, lines: out, url: None })
}

/// Adobe notes for one version, by the kind of release notes link the app has: an Adobe community
/// announcements category, Steam news, a Substance 3D "All changes" page, or a product page.
pub async fn adobe(notes_url: &str, ver: &str) -> Option<Notes> {
    if let Some(id) = notes_url.strip_prefix("https://community.adobe.com/announcements-").and_then(|r| r.trim_end_matches('/').parse::<u32>().ok()) {
        return community(id, ver).await;
    }
    if let Some(app) = notes_url.strip_prefix("https://store.steampowered.com/news/app/").and_then(|r| r.trim_end_matches('/').parse::<u32>().ok()) {
        let api = format!("https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={app}&count=20&maxlength=0&format=json");
        return steam_news(&page_shared(&api).await?, ver);
    }
    if notes_url.starts_with("https://experienceleague.adobe.com/") && notes_url.trim_end_matches('/').ends_with("/all-changes") {
        let md = page_shared(&format!("{}.md", notes_url.trim_end_matches('/'))).await;
        if let Some(n) = md.and_then(|md| substance_changes(&md, ver)) {
            return Some(n);
        }
        return match crate::sources::adobe::community_fallback(notes_url) {
            Some(id) => community(id, ver).await,
            None => None,
        };
    }
    adobe_product_page(notes_url, ver).await
}

/// Notes from an Adobe product page: it links one page per minor version ("version-12-2"); older
/// products use one long page with a heading per version.
async fn adobe_product_page(notes_url: &str, ver: &str) -> Option<Notes> {
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
                return Some(Notes { date, lines, url: None });
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
                return Some(Notes { date: None, lines, url: None });
            }
        }
    }
    None
}

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

/// The change list of a Krita release post: the list items under its "Changelog", "Changes
/// since ..." or "Bug fixes" heading; in posts without one, the list items between the title and
/// the first "Download" heading. Krita answers unknown posts with a normal page titled "404 Page
/// not found", so that gives none.
pub fn krita_changelog(html: &str) -> Vec<String> {
    if html.contains("<title>404") {
        return vec![];
    }
    let heads = headings(html);
    let named = |t: &str| {
        let t = t.trim().to_lowercase();
        ["changelog", "changes since", "bug fixes", "bugfixes"].iter().any(|w| t.starts_with(w))
    };
    if let Some(k) = heads.iter().position(|h| h.0 >= 2 && named(&h.1)) {
        let level = heads[k].0;
        let stop = heads[k + 1..].iter().find(|h| h.0 <= level).map(|h| h.2).unwrap_or(html.len());
        let lines = items(&html[heads[k].3..stop]);
        if !lines.is_empty() {
            return lines;
        }
    }
    let Some(h1) = heads.iter().find(|h| h.0 == 1) else { return vec![] };
    let Some(download) = heads.iter().find(|h| h.2 > h1.2 && h.0 == 2 && h.1.to_lowercase().contains("download")) else { return vec![] };
    items(&html[h1.3..download.2])
}

/// The post that announces Krita `ver`: 6.0.N comes out with 5.3.N and is announced in its post
/// ("Download 6.0.4" under "Krita 5.3.4 Released!").
fn krita_post_version(ver: &str) -> String {
    match ver.strip_prefix("6.0.") {
        Some(rest) => format!("5.3.{rest}"),
        None => ver.to_string(),
    }
}

/// Addresses in a sitemap.
fn sitemap_locs(xml: &str) -> Vec<&str> {
    use once_cell::sync::Lazy;
    static LOC: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"<loc>\s*([^<\s]+)\s*</loc>").unwrap());
    LOC.captures_iter(xml).map(|c| c.get(1).unwrap().as_str()).collect()
}

/// Names a Krita page may use for `ver`: "5.2.9", "5-2-9", and for x.y.0 also "5.2" and "5-2".
fn krita_names(ver: &str) -> Vec<String> {
    let mut forms = vec![ver.to_string()];
    let mut short = ver.to_string();
    while let Some(s) = short.strip_suffix(".0").filter(|s| s.contains('.')) {
        short = s.to_string();
        forms.push(short.clone());
    }
    forms.iter().flat_map(|f| [f.clone(), f.replace('.', "-")]).collect()
}

/// The release post of Krita `ver` in krita.org's sitemap ("/posts/2026/krita-5.3.4-released/",
/// "/posts/2025/krita-5-2-9-released/").
pub fn krita_post_url(sitemap: &str, ver: &str) -> Option<String> {
    let locs = sitemap_locs(sitemap);
    krita_names(ver).iter().find_map(|n| {
        let tail = format!("/krita-{n}-released/");
        locs.iter().find(|l| l.contains("/posts/") && l.ends_with(&tail)).map(|l| l.to_string())
    })
}

/// The release notes page of Krita series x.y in the sitemap ("/release-notes/krita-5-3-release-notes/").
pub fn krita_notes_url(sitemap: &str, ver: &str) -> Option<String> {
    let p = version::parts(ver);
    let (x, y) = (p.first().copied().unwrap_or(0), p.get(1).copied().unwrap_or(0));
    let locs = sitemap_locs(sitemap);
    [format!("/krita-{x}-{y}-release-notes/"), format!("/krita-{x}-{y}-0-release-notes/")]
        .iter()
        .find_map(|tail| locs.iter().find(|l| l.ends_with(tail.as_str())).map(|l| l.to_string()))
}

/// A Krita release notes page as notes: its sections and paragraphs, up to the contributors.
pub fn krita_release_notes(html: &str) -> Vec<String> {
    if html.contains("<title>404") {
        return vec![];
    }
    let from = html.find("<h1").unwrap_or(0);
    let to = html[from..].find("<footer").map(|i| from + i).unwrap_or(html.len());
    let mut out = Vec::new();
    for l in outline(&html[from..to]) {
        let low = l.to_lowercase();
        if l.starts_with('#') && (low.trim_start_matches(['#', ' ']).starts_with("contributors") || low.contains("like what we are doing")) {
            break;
        }
        // The fallback text of an embedded video.
        if !low.contains("your browser does not seem to support") {
            out.push(l);
        }
    }
    tidy_headings(out)
}

/// Krita's notes for a release: the change list of its post, found through krita.org's sitemap
/// (or, without it, under the names posts usually have in the release year). A first release
/// (x.y.0) whose post has no list gets the series' release notes page. Returns the page address
/// and the lines.
pub async fn krita(ver: &str, year: i32) -> Option<(String, Vec<String>)> {
    let post_ver = krita_post_version(ver);
    let sitemap = page_shared("https://krita.org/en/sitemap.xml").await;
    let posts: Vec<String> = match &sitemap {
        Some(s) => krita_post_url(s, &post_ver).into_iter().collect(),
        None => [year, year - 1].iter().flat_map(|y| [post_ver.clone(), post_ver.replace('.', "-")].map(|n| format!("https://krita.org/en/posts/{y}/krita-{n}-released/"))).collect(),
    };
    for url in posts {
        if let Some(html) = page(&url).await {
            let lines = krita_changelog(&html);
            if !lines.is_empty() {
                return Some((url, lines));
            }
        }
    }
    let p = version::parts(&post_ver);
    if p.len() <= 3 && p.get(2).copied().unwrap_or(0) == 0 {
        let url = krita_notes_url(sitemap.as_deref()?, &post_ver)?;
        let lines = krita_release_notes(&page(&url).await?);
        if !lines.is_empty() {
            return Some((url, lines));
        }
    }
    None
}

// ---------------------------------------------------------------- any publisher's notes

/// Does `text` name version `ver`? "1.2" also counts for "1.2.0". Digits or dots right next to
/// it make it another version ("11.2.3", "1.2.30").
/// `page_notes` for a page at `url`: when the address itself names the version
/// ("/changelogs/25.03/", "gimp-3.2.html" for 3.2.6, "RN-...-8-09-28-1124.html") the whole page
/// is about it, so its main content is the change list even when no heading repeats the version.
pub fn page_notes_at(url: &str, body: &str, ver: &str) -> Vec<String> {
    let lines = page_notes(body, ver);
    if !lines.is_empty() || !url_names_version(url, ver) {
        return lines;
    }
    // A page for a whole series ("Changelog-7.2") that names other releases of it but not this
    // one has no notes for this one yet.
    if series_page_without(body, ver) {
        return vec![];
    }
    let mut html = body.to_string();
    for tag in ["script", "style", "nav", "svg", "noscript", "title", "header", "footer", "aside"] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}[\s>].*?</{tag}>")).unwrap();
        html = re.replace_all(&html, "").into_owned();
    }
    let main = ["<main", "<article"].iter().find_map(|t| html.find(t)).map(|i| &html[i..]).unwrap_or(&html);
    outline(main)
}

/// Does `body` name other releases of `ver`'s series ("7.2.20" for 7.2.22) but not `ver`?
fn series_page_without(body: &str, ver: &str) -> bool {
    let parts: Vec<&str> = ver.trim().trim_start_matches(['v', 'V']).split('.').collect();
    if parts.len() < 3 {
        return false;
    }
    let series = format!("{}.{}.", parts[0], parts[1]);
    let re = regex::Regex::new(&format!(r"(^|[^0-9.]){}\d+", regex::escape(&series))).unwrap();
    re.is_match(body) && !names_version(body, ver)
}

/// Does the path of `url` name `ver`, or its first two parts for a bug-fix version ("3.2" of
/// "3.2.6"), written with dots, dashes or underscores?
pub fn url_names_version(url: &str, ver: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else { return false };
    let path = u.path().to_ascii_lowercase();
    let ver = ver.trim().trim_start_matches(['v', 'V']).to_ascii_lowercase();
    let parts: Vec<&str> = ver.split('.').collect();
    let mut forms = vec![ver.clone()];
    if parts.len() >= 3 {
        forms.push(parts[..2].join("."));
    }
    forms.iter().filter(|f| f.contains('.') && f.len() >= 3).any(|f| {
        [f.clone(), f.replace('.', "-"), f.replace('.', "_")].iter().any(|g| {
            let mut from = 0;
            while let Some(i) = path[from..].find(g.as_str()) {
                let (s, e) = (from + i, from + i + g.len());
                let before = path[..s].chars().last();
                let mut rest = path[e..].chars();
                let after = rest.next();
                let edge_before = before.is_none_or(|c| !c.is_ascii_alphanumeric() && c != '.');
                let edge_after = after.is_none_or(|c| !c.is_ascii_digit() && c != '.' && c != '-' && c != '_') || (matches!(after, Some('.' | '-' | '_')) && rest.next().is_none_or(|c| !c.is_ascii_digit()));
                if edge_before && edge_after {
                    return true;
                }
                from = e;
            }
            false
        })
    })
}

pub fn names_version(text: &str, ver: &str) -> bool {
    let base = ver.trim().trim_start_matches(['v', 'V']).to_string();
    let mut forms = vec![base.clone()];
    // A packaged build of a release: "21.0.12.101" and "6.0.6-1" are 21.0.12 and 6.0.6 on the
    // publisher's page.
    if let Some((rel, n)) = base.rsplit_once('-') {
        if !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()) && rel.contains('.') {
            forms.push(rel.to_string());
        }
    }
    let parts: Vec<&str> = base.split('.').collect();
    if parts.len() == 4 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())) {
        forms.push(parts[..3].join("."));
    }
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

/// Release notes in markdown or plain text (a GitHub release, a winget manifest's notes), in
/// full: headings as "## " / "### " lines, every other line (bullets and text) as an item, with
/// markup, links, images and commit or issue references removed. Boilerplate sections
/// (contributors, downloads, checksums) and code blocks and tables are left out.
pub fn text_lines(t: &str) -> Vec<String> {
    lines_of(t, false)
}

/// The changelog page a release text only points to, as ShareX's does ("**Changelog:**
/// https://getsharex.com/changelog#v22.0.0" and a table of checksums): a text whose one line is
/// a label like "Changelog" or "See the release notes" and that links one page. A comparison of
/// two tags ("Full Changelog: .../compare/v1...v2") and download links are no changelog.
pub fn linked_changelog(body: &str) -> Option<String> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static URL: Lazy<Regex> = Lazy::new(|| Regex::new(r#"https?://[^\s<>()\[\]"'`|*]+"#).unwrap());
    let label = match text_lines(body).as_slice() {
        [] => true,
        [one] => is_notes_label(one),
        _ => false,
    };
    if !label {
        return None;
    }
    let mut urls: Vec<&str> = URL.find_iter(body).map(|m| m.as_str().trim_end_matches(['.', ',', ';', ':'])).filter(|u| !u.contains("/compare/") && !u.contains("/releases/download/")).collect();
    urls.sort_unstable();
    urls.dedup();
    match urls.as_slice() {
        [one] => Some(one.to_string()),
        _ => None,
    }
}

/// A line that only names the change list ("Changelog", "See the release notes", "What's new").
fn is_notes_label(line: &str) -> bool {
    let l = line.trim().trim_end_matches([':', '.']).trim().to_lowercase().replace('\u{2019}', "'");
    let l = ["see the ", "see ", "read the ", "full ", "the "].iter().fold(l, |l, p| l.strip_prefix(p).map(str::to_string).unwrap_or(l));
    matches!(l.as_str(), "changelog" | "change log" | "changes" | "release notes" | "notes" | "what's new" | "whats new")
}

/// Pictures kept in one version's change list.
const MAX_PICTURES: usize = 6;

/// A picture in a change list: a line of its own, "![](<address>)" (see `noteimg`).
pub fn picture_line(url: &str) -> String {
    format!("![]({url})")
}

fn lines_of(t: &str, bullets_only: bool) -> Vec<String> {
    use once_cell::sync::Lazy;
    use regex::Regex;
    static IMG: Lazy<Regex> = Lazy::new(|| Regex::new(r"!\[[^\]]*\]\([^)]*\)").unwrap());
    // The address of a markdown picture and of an HTML one (GitHub writes attached screenshots so).
    static IMG_SRC: Lazy<Regex> = Lazy::new(|| Regex::new(r#"!\[[^\]]*\]\(\s*<?(https://[^\s)>]+)>?(?:\s+"[^"]*")?\s*\)"#).unwrap());
    static IMG_TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)<img\b[^>]*?\bsrc\s*=\s*["'](https://[^"']+)["']"#).unwrap());
    static LINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[([^\]]+)\]\([^)]*\)").unwrap());
    static URL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\(?<?https?://[^\s)>]+>?\)?").unwrap());
    static NUMBERED: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\d{1,3}[.)]\s+(.*)$").unwrap());
    static HANDLE: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s+(by|thanks to)\s+@[\w-]+.*$").unwrap());
    // Markdown escapes ("async\_hooks"), the commit a line starts with ("[b12bcc9ae1] - ",
    // "80d23f64 ") and issue or commit references in brackets ("(#123)").
    static ESCAPED: Lazy<Regex> = Lazy::new(|| Regex::new(r"\\([\[\]_*`#<>()])").unwrap());
    static COMMIT: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\[[0-9a-f]{7,40}\]|[0-9a-f]{7,40}|#\d+)\s*(-\s*)?").unwrap());
    static REFS: Lazy<Regex> = Lazy::new(|| Regex::new(r"\s*[\(\[]\s*(?:(?:#\d+|[0-9a-f]{7,12})\s*,?\s*)+[\)\]]").unwrap());
    // GitHub emoji codes (":rocket:").
    static EMOJI: Lazy<Regex> = Lazy::new(|| Regex::new(r":[a-z][a-z0-9_+\-]{1,30}:").unwrap());
    // A line that is all bold ("**Bug fixes:**") titles what follows.
    static BOLD: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(?:\*\*|__)([^*_]{2,80}?)(?:\*\*|__):?$").unwrap());
    let clean = |s: &str| -> String {
        let s = ESCAPED.replace_all(s, "$1");
        let s = EMOJI.replace_all(&s, "");
        let s = IMG.replace_all(&s, "");
        let s = LINK.replace_all(&s, "$1");
        let s = URL.replace_all(&s, "");
        let s = s.replace("**", "").replace("__", "").replace('`', "");
        let s = COMMIT.replace(s.trim(), "");
        let s = HANDLE.replace(&s, "");
        let s = REFS.replace_all(&s, "");
        let s = strip_tags(&s);
        s.trim().trim_start_matches(['-', '*', ':', ' ']).trim_end_matches(['.', ',', ';', ':', ' ']).to_string()
    };
    let useful = |s: &String| {
        let low = s.to_lowercase();
        s.chars().filter(|c| c.is_alphanumeric()).count() >= 3 && !low.starts_with("full changelog") && !low.starts_with('@') && !low.starts_with("new contributors")
    };
    let bullet = |l: &str| -> Option<String> {
        for p in ["- ", "* ", "+ ", "\u{2022} ", "\u{b7} "] {
            if let Some(r) = l.strip_prefix(p) {
                return Some(r.to_string());
            }
        }
        NUMBERED.captures(l).map(|c| c[1].to_string())
    };
    let mut out: Vec<String> = Vec::new();
    let mut pictures = 0;
    // Inside a boilerplate section or a code block: skipped.
    let (mut skipping, mut fence) = (false, false);
    // Sentences some publishers run together with no line break ("the application.Note: It
    // is", "Downloads:- The installer") become separate lines.
    static RUN_ON: Lazy<Regex> = Lazy::new(|| Regex::new(r"([a-z]{2,})\.([A-Z][a-z])").unwrap());
    static COLON_DASH: Lazy<Regex> = Lazy::new(|| Regex::new(r":-\s+").unwrap());
    let t = RUN_ON.replace_all(t, "$1.
$2");
    let t = COLON_DASH.replace_all(&t, "
");
    for raw in t.lines() {
        let l = raw.trim();
        if l.starts_with("```") || l.starts_with("~~~") {
            fence = !fence;
            continue;
        }
        if fence || l.is_empty() || l.starts_with('|') || l.starts_with("<!--") || l.starts_with("===") || l.starts_with("---") || l.starts_with("***") {
            continue;
        }
        let hashes = l.chars().take_while(|c| *c == '#').count();
        let heading = if hashes > 0 && l.chars().nth(hashes) == Some(' ') {
            Some(EMOJI.replace_all(&ESCAPED.replace_all(l, "$1"), "").into_owned())
        } else {
            BOLD.captures(l).map(|c| format!("### {}", &c[1]))
        };
        if let Some(h) = heading {
            match crate::sources::craft::markdown_heading(&h) {
                Some(h) => {
                    skipping = false;
                    out.push(strip_tags(&h));
                }
                None => skipping = true,
            }
            continue;
        }
        if skipping {
            continue;
        }
        let item = match bullet(l) {
            Some(b) => b,
            None if bullets_only => continue,
            None => l.to_string(),
        };
        let c = clean(&item);
        if useful(&c) && out.last() != Some(&c) {
            out.push(c);
        }
        // Screenshots GitHub serves follow the text they are in, each on a line of its own.
        for cap in IMG_SRC.captures_iter(l).chain(IMG_TAG.captures_iter(l)) {
            let line = picture_line(&cap[1]);
            if pictures < MAX_PICTURES && crate::noteimg::allowed(&cap[1]) && !out.contains(&line) {
                out.push(line);
                pictures += 1;
            }
        }
        if out.len() >= MAX_LINES {
            break;
        }
    }
    tidy_headings(out)
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

/// HTML as markdown-like text: h2 to h4 as "## " / "### " lines, list items as "- " bullets,
/// one line per other block.
fn html_lines(html: &str) -> String {
    let head = regex::Regex::new(r"(?is)<h([2-4])[^>]*>(.*?)</h[2-4]>").unwrap();
    let li = regex::Regex::new(r"(?i)<li[^>]*>").unwrap();
    let block = regex::Regex::new(r"(?i)</?(p|div|br|h[1-6]|tr|dt|dd|ul|ol|summary|details|section|article|pre)[^>]*>").unwrap();
    let t = head.replace_all(html, |c: &regex::Captures| format!("\n{} {}\n", if &c[1] == "2" { "##" } else { "###" }, strip_tags(&c[2])));
    let t = li.replace_all(&t, "\n- ");
    let t = block.replace_all(&t, "\n");
    t.lines().map(strip_tags).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

/// The change list for version `ver` on a publisher's release notes page, in full with its
/// sections: what follows the heading or the short paragraph that names the version, or the
/// page's own content when the whole page is about that version. A markdown or text file gives
/// the section under its heading for the version.
pub fn page_notes(body: &str, ver: &str) -> Vec<String> {
    // The first 2000 bytes, cut where a character starts (a page with "é" or "™" there crashed).
    let mut cut = body.len().min(2000);
    while !body.is_char_boundary(cut) {
        cut -= 1;
    }
    let head = &body[..cut].to_ascii_lowercase();
    if !(head.trim_start().starts_with('<') || head.contains("<html") || head.contains("<body")) {
        if let Some(s) = md_section(body, ver) {
            return text_lines(&s);
        }
        return text_section(body, ver).map(|s| lines_of(&s, true)).unwrap_or_default();
    }
    let mut html = body.to_string();
    let title = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title>").unwrap().captures(&html).map(|c| strip_tags(&c[1])).unwrap_or_default();
    // Not <header>: release pages put each version's heading in one.
    for tag in ["script", "style", "nav", "svg", "noscript", "title"] {
        let re = regex::Regex::new(&format!(r"(?is)<{tag}[\s>].*?</{tag}>")).unwrap();
        html = re.replace_all(&html, "").into_owned();
    }
    let heads = headings(&html);
    if let Some(k) = heads.iter().position(|h| names_version(&h.1, ver)) {
        let level = heads[k].0;
        let stop = heads[k + 1..].iter().find(|h| h.0 <= level).map(|h| h.2).unwrap_or(html.len());
        return outline(&html[heads[k].3..stop]);
    }
    // A short paragraph that names the version, then its list.
    if let Some(s) = text_section(&html_lines(&html), ver) {
        let lines = lines_of(&s, true);
        if !lines.is_empty() {
            return lines;
        }
    }
    let h1 = heads.iter().find(|h| h.0 == 1).map(|h| h.1.clone()).unwrap_or_default();
    if names_version(&title, ver) || names_version(&h1, ver) {
        let main = ["<main", "<article"].iter().find_map(|t| html.find(t)).map(|i| &html[i..]).unwrap_or(&html);
        return outline(main);
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
    #[test]
    fn a_release_text_that_only_links_its_changelog_names_that_page() {
        // ShareX 22.0.0's GitHub release text.
        let sharex = "**Changelog:** https://getsharex.com/changelog#v22.0.0\r\n\r\n| File | SHA256 |\r\n| --- | --- |\r\n| ShareX-22.0.0-portable-x64.zip | `2D46ED557A052030D26297635739C7EF470D873B9C97E46DC1EB4D19E96F2FC5` |\r\n| ShareX-22.0.0-setup-x64.exe | `AB8339422721F7123AB031C6845323BA7A8F2E1AB25DB849D76032AF6539EFA4` |";
        assert_eq!(super::linked_changelog(sharex).as_deref(), Some("https://getsharex.com/changelog#v22.0.0"));
        assert_eq!(super::linked_changelog("See the [release notes](https://example.com/news/2-0).").as_deref(), Some("https://example.com/news/2-0"));
        assert_eq!(super::linked_changelog("https://example.com/changelog").as_deref(), Some("https://example.com/changelog"));
        // A change list of its own, a comparison of tags, and two pages to choose from are not.
        assert_eq!(super::linked_changelog("- Fixed a crash\n- Faster start\n\nChangelog: https://example.com/changelog"), None);
        assert_eq!(super::linked_changelog("**Full Changelog**: https://github.com/a/b/compare/v1.0...v1.1"), None);
        assert_eq!(super::linked_changelog("Changelog: https://example.com/a or https://example.com/b"), None);
        assert_eq!(super::linked_changelog("This release fixes a crash: https://example.com/issue/1"), None);
    }

    #[test]
    fn a_changelog_page_gives_the_section_of_its_version() {
        // The shape of ShareX's changelog page: one section per version, newest first.
        let page = r#"<h2 id="v22.0.0"><a href="https://github.com/ShareX/ShareX/releases/tag/v22.0.0">ShareX 22.0.0</a> - 2026-10-20</h2>
<p>The whole interface is new.</p>
<ul>
  <li>Screen recording changes:
    <ul><li>Added native screen recording.</li><li>Added system audio recording.</li></ul>
  </li>
  <li>Added a recorder bar.</li>
</ul>
<h2 id="v21.0.0"><a href="https://github.com/ShareX/ShareX/releases/tag/v21.0.0">ShareX 21.0.0</a> - 2026-07-03</h2>
<ul><li>Added "Background remover" tool.</li></ul>"#;
        let lines = super::page_notes_at("https://getsharex.com/changelog#v22.0.0", page, "22.0.0");
        assert!(lines.iter().any(|l| l.contains("native screen recording")), "{lines:?}");
        assert!(lines.iter().any(|l| l.contains("recorder bar")), "{lines:?}");
        assert!(!lines.iter().any(|l| l.contains("Background remover")), "{lines:?}");
    }

    #[test]
    fn screenshots_github_serves_stay_in_the_change_list() {
        let body = "## New\r\n- **Pin windows.** Keep a window on top.\r\n<img width=\"1200\" height=\"700\" alt=\"pin\" src=\"https://github.com/user-attachments/assets/984de630-77a6-4a41-bd4b-87e915a6ec0c\" />\r\n- Dark theme ![shot](https://user-images.githubusercontent.com/1/dark.png)\r\n![badge](https://img.shields.io/badge/build-passing-green) ![ad](https://i.imgur.com/x.png)\r\n";
        let lines = super::text_lines(body);
        assert_eq!(
            lines,
            [
                "Pin windows. Keep a window on top",
                "![](https://github.com/user-attachments/assets/984de630-77a6-4a41-bd4b-87e915a6ec0c)",
                "Dark theme",
                "![](https://user-images.githubusercontent.com/1/dark.png)",
            ]
        );
        // At most a few pictures per version, each once.
        let many: String = (0..10).map(|i| format!("![a](https://github.com/user-attachments/assets/{i})\n![a](https://github.com/user-attachments/assets/{i})\n")).collect();
        let lines = super::text_lines(&format!("- A change\n{many}"));
        assert_eq!(lines.iter().filter(|l| l.starts_with("![](")).count(), 6);
    }

    #[test]
    fn kept_notes_pages_stay_under_the_cap_old_ones_first() {
        use std::sync::Arc;
        use std::time::{Duration, Instant};
        let now = Instant::now() + Duration::from_secs(3600);
        let slot = |age: u64, ok: bool| -> PageSlot { Arc::new(tokio::sync::Mutex::new(Some((now - Duration::from_secs(age), ok.then(|| Arc::new(String::from("page"))))))) };
        let mut pages: HashMap<String, PageSlot> = HashMap::new();
        pages.insert("fresh".into(), slot(10, true));
        pages.insert("older".into(), slot(300, true));
        pages.insert("stale".into(), slot(700, true));
        pages.insert("failed".into(), slot(90, false));
        // Under the cap: nothing goes.
        prune_pages(&mut pages, 10, now);
        assert_eq!(pages.len(), 4);
        // At the cap: what is past its time goes, then the oldest, leaving room for one more.
        prune_pages(&mut pages, 2, now);
        let mut left: Vec<&String> = pages.keys().collect();
        left.sort();
        assert_eq!(left, ["fresh"]);
        // A page being fetched stays.
        let busy = slot(5000, true);
        let _held = busy.try_lock().unwrap();
        pages.insert("busy".into(), busy.clone());
        prune_pages(&mut pages, 1, now);
        assert!(pages.contains_key("busy") && !pages.contains_key("fresh"));
    }

    use super::*;

    /// Jellyfin Media Player 1.11.0's notes run their sentences together in the catalog.
    #[test]
    fn run_together_sentences_become_separate_lines() {
        let t = "This release switches to the external web client instead of when I get a chance to release the application.Note: It is normal to have to log in again after installing this update.About Windows Downloads:- The desktop installer requires administrator rights";
        let l = text_lines(t);
        assert_eq!(l, vec![
            "This release switches to the external web client instead of when I get a chance to release the application",
            "Note: It is normal to have to log in again after installing this update",
            "About Windows Downloads",
            "The desktop installer requires administrator rights",
        ]);
        // Names with dots stay whole.
        assert_eq!(text_lines("Updated Node.js and e.g. ASP.NET support"), vec!["Updated Node.js and e.g. ASP.NET support"]);
    }

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
    fn finds_a_version_in_text() {
        assert!(names_version("Version 1.2.3 (2026-10-01)", "1.2.3"));
        assert!(names_version("## [v1.2.3] - today", "1.2.3"));
        assert!(names_version("Released 1.2.", "1.2.0"));
        assert!(!names_version("Version 11.2.3", "1.2.3"));
        assert!(!names_version("Version 1.2.30", "1.2.3"));
        assert!(!names_version("1.2.3.4", "1.2.3"));
    }

    #[test]
    fn markdown_and_plain_notes_come_in_full_with_their_sections() {
        let md = "## What's Changed\n### :rocket: New\n* Fixed the [export](https://x.y/z) crash by @someone in https://github.com/o/r/pull/1\n* **Faster** start up. It now opens in half the time, even with many files.\n\n**Bug fixes:**\n* \\[b12bcc9ae1] - async\\_hooks: add a scope\n* 80d23f64 (patch: Restore the entitlements)\n* Fixed saving (#1234)\n```\ncode that is not a note\n```\n| table | row |\n## New Contributors\n* @someone made their first contribution\n\n**Full Changelog**: https://github.com/o/r/compare/a...b";
        assert_eq!(
            text_lines(md),
            vec![
                "## What's Changed",
                "### New",
                "Fixed the export crash",
                "Faster start up. It now opens in half the time, even with many files",
                "### Bug fixes",
                "async_hooks: add a scope",
                "(patch: Restore the entitlements)",
                "Fixed saving",
            ]
        );
        let plain = "This release fixes a crash when saving.\nIt also updates translations.";
        assert_eq!(text_lines(plain), vec!["This release fixes a crash when saving", "It also updates translations"]);
        assert_eq!(text_lines("1. First fix\n2) Second fix"), vec!["First fix", "Second fix"]);
        assert!(text_lines("\n\n").is_empty());
        // No cap: a long list stays whole.
        let long: String = (0..60).map(|i| format!("- Fixed problem number {i}\n")).collect();
        assert_eq!(text_lines(&long).len(), 60);
    }

    #[test]
    fn a_packaged_build_is_found_under_its_release() {
        // Seen with OpenJDK, TreeSize and BleachBit: the catalog's version has a build part.
        assert!(names_version("Release notes for 21.0.12 (October 2026)", "21.0.12.101"));
        assert!(names_version("BleachBit 6.0.6 was released", "6.0.6-1"));
        assert!(!names_version("Release notes for 21.0.13", "21.0.12.101"));
        assert!(!names_version("Version 2.1", "2.1.0.4-beta"), "only a number after the dash");
    }

    #[test]
    fn package_lists_and_older_versions_are_not_changes() {
        let lines: Vec<String> = ["## Security fixes", "Fixed CVE-2026-1234 in the runtime", "## Packages updated in this release", "Microsoft.NETCore.App 8.0.32", "Microsoft.AspNetCore.App 8.0.32", "## Known issues", "A known issue that is still open", "## Prior Versions", "### 4.6.8", "Older fix"].iter().map(|s| s.to_string()).collect();
        let out = tidy_headings(lines);
        assert_eq!(out, vec!["## Security fixes", "Fixed CVE-2026-1234 in the runtime", "## Known issues", "A known issue that is still open"]);
    }

    #[test]
    fn a_series_page_without_this_release_gives_nothing() {
        // Seen with VirtualBox: "Changelog-7.2" lists 7.2.20 and older, not yet 7.2.22.
        let page = "<html><body><main><h3>VirtualBox 7.2.20 (released 2026-09-01)</h3><ul><li>Fixed a crash when resizing a window</li></ul></main></body></html>";
        assert!(page_notes_at("https://www.virtualbox.org/wiki/Changelog-7.2", page, "7.2.22").is_empty());
        let one = "<html><body><main><h2>New in this release</h2><ul><li>Faster startup on large projects here</li></ul></main></body></html>";
        assert!(!page_notes_at("https://www.gimp.org/release-notes/gimp-3.2.html", one, "3.2.6").is_empty());
    }

    #[test]
    fn a_page_whose_address_names_the_version_is_read_whole() {
        assert!(url_names_version("https://www.codeblocks.org/changelogs/25.03/", "25.03"));
        assert!(url_names_version("https://www.gimp.org/release-notes/gimp-3.2.html", "3.2.6"));
        assert!(url_names_version("https://www.amd.com/en/resources/release-notes/RN-RYZEN-CHIPSET-8-09-28-1124.html", "8.09.28.1124"));
        assert!(!url_names_version("https://www.gimp.org/release-notes/gimp-3.20.html", "3.2.6"));
        assert!(!url_names_version("https://example.org/news/", "3.2.6"));
        assert!(!url_names_version("https://example.org/v13.2.6/", "3.2.6"), "13.2.6 is not 3.2.6");
        // Seen with Code::Blocks: the page for 25.03 never repeats "25.03" in a heading.
        let page = "<html><head><title>Changelog</title></head><body><nav><ul><li>Home page link item here</li></ul></nav><main><h2>Editor</h2><ul><li>Fixed a crash when closing many tabs</li></ul></main></body></html>";
        assert!(page_notes(page, "25.03").is_empty());
        let whole = page_notes_at("https://www.codeblocks.org/changelogs/25.03/", page, "25.03");
        assert!(whole.iter().any(|l| l == "Fixed a crash when closing many tabs") && !whole.iter().any(|l| l.contains("Home page")), "{whole:?}");
        assert!(page_notes_at("https://www.codeblocks.org/news/", page, "25.03").is_empty());
    }

    #[test]
    fn a_page_with_a_long_character_at_its_start_is_read() {
        // Seen on a live catalog page: a two-byte character right where the start is cut.
        let page = format!("{}é
## 2.0
- Fixed a crash on start
", "a".repeat(1999));
        assert_eq!(page_notes(&page, "2.0"), vec!["Fixed a crash on start".to_string()]);
    }

    #[test]
    fn notes_for_one_version_from_a_page() {
        let html = r#"<html><head><title>Changelog</title></head><body><nav><ul><li>Home page link here</li></ul></nav>
<h2>Version 2.1.0</h2><h3>New</h3><ul><li>New export dialog for images</li></ul><h3>Fixed</h3><ul><li>Fixed a crash when saving. More words here.</li></ul>
<h2>Version 2.0.9</h2><ul><li>An older fix for loading</li></ul></body></html>"#;
        assert_eq!(page_notes(html, "2.1.0"), vec!["## New", "New export dialog for images", "## Fixed", "Fixed a crash when saving. More words here"]);
        assert_eq!(page_notes(html, "2.0.9"), vec!["An older fix for loading"]);
        assert!(page_notes(html, "3.0").is_empty(), "a page without the version gives nothing");
        let release = r#"<html><title>App 5.4 released</title><main><p>Intro text that is long enough to count.</p><ul><li>Better colors in dark mode</li></ul></main></html>"#;
        assert_eq!(page_notes(release, "5.4"), vec!["Intro text that is long enough to count", "Better colors in dark mode"]);
        let inno = r#"<html><body><details open=""><summary>Tool 6.7</summary><p><a name="6.7.3"></a><span class="ver">6.7.3 </span><span class="date">(2026-05-26)</span></p><p><b>Using it at work?</b> Buy a license.</p><ul><li>Improved the startup</li><li>Fix: the example script</li></ul><p><a name="6.7.2"></a><span class="ver">6.7.2 </span></p><ul><li>Older</li></ul></details></body></html>"#;
        assert_eq!(page_notes(inno, "6.7.3"), vec!["Improved the startup", "Fix: the example script"]);
        assert_eq!(page_notes(inno, "6.7.2"), vec!["Older"]);
        let changelog = "# Changelog\n\n## 1.4.0\n### Added\n- Added dark mode\n### Fixed\n- Fixed login\n\n## 1.3.2\n- Old fix\n";
        assert_eq!(page_notes(changelog, "1.4.0"), vec!["## Added", "Added dark mode", "## Fixed", "Fixed login"]);
        assert_eq!(page_notes(changelog, "1.3.2"), vec!["Old fix"]);
        assert!(page_notes(changelog, "1.5").is_empty());
    }

    #[test]
    fn parses_dates() {
        assert_eq!(parse_date("September 15, 2026").as_deref(), Some("2026-09-15T00:00:00Z"));
        assert_eq!(parse_date("May 19th 2026").as_deref(), Some("2026-05-19T00:00:00Z"));
    }

    /// An Adobe community category page: the topic list is JSON in an HTML-escaped attribute.
    fn community_html(topics: serde_json::Value, pages: u32) -> String {
        let props = serde_json::json!({ "topics": topics, "totalPageCount": pages, "nextPageUrl": null }).to_string();
        let esc = props.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;");
        format!(r#"<html><body><div class="widget-placeholder"><div data-preact="destination/modules/Content/TopicList/CategoryTopicList" class="" data-props="{esc}"></div></div></body></html>"#)
    }

    #[test]
    fn adobe_community_announcements_give_the_post_of_each_release() {
        let body = r#"<p>The September 2026 release of Photoshop 27.11 introduces Enhance Edge&nbsp;and more.</p><p><a href="x">Click here for more details</a>.</p><h3><span>Bug fixes &#8211; Thanks to you!</span></h3><ul><li><a href="y">Canvas Size dialog did not retain the previously used units.</a></li><li>Edit &gt; Stroke used the previously selected color.</li></ul><section class="callout"><p>👉 Download the latest version of Photoshop here.<br />To get Photoshop 27.11, open the Creative Cloud desktop app.</p></section><h3>Join the conversation</h3><p>Have you tried Enhance Edge in your workflow? Tell us.</p>"#;
        let topic = |title: &str, at: &str, content: &str| serde_json::json!({ "title": title, "publishedAt": at, "contentType": "productUpdate", "topicUrl": { "destination": format!("https://community.adobe.com/announcements-710/{}", title.len()) }, "content": content });
        let html = community_html(
            serde_json::json!([
                topic("Edit your photos with a text prompt with Prompt to Edit in Photoshop 27.10 and above", "2026-09-17T20:09:27+02:00", "<p>Not the release post at all</p>"),
                topic("Photoshop 27.11: Enhance Edge, GPT Image 2, and more!", "2026-10-05T14:43:44+00:00", body),
                topic("Meet Photoshop's AI Assisted Editor (Beta) : Edit Images in Photoshop 27.10", "2026-09-03T19:51:46+02:00", "<p>A beta feature post, not the release</p>"),
                topic("See your designs from a whole new angle with Rotate Objects in Photoshop 27.6", "2026-05-12T14:41:24+02:00", "<p>A follow up article about one feature</p>"),
                topic("Photoshop v27.6 Is Live! Rotate in 3D, Clean Up Faster", "2026-04-28T15:12:36+02:00", "<p>Rotate objects in 3D and clean up faster</p>"),
                topic("Alert: If you updated to Sonoma 14.2 please install the update", "2023-12-04T10:00:00+00:00", "<p>Not about this app's version</p>"),
            ]),
            2,
        );
        let (topics, pages) = community_page(&html).expect("topic list");
        assert_eq!((topics.len(), pages), (6, 2));
        assert_eq!(topics[1].published.as_deref(), Some("2026-10-05T14:43:44Z"));
        let names = ["Photoshop"];
        let pick = |v: &str| community_pick(&topics, v, &names, false).map(|t| t.title.clone());
        assert_eq!(pick("27.11").as_deref(), Some("Photoshop 27.11: Enhance Edge, GPT Image 2, and more!"));
        // The release post wins over a later article about one of its features.
        assert_eq!(pick("27.6").as_deref(), Some("Photoshop v27.6 Is Live! Rotate in 3D, Clean Up Faster"));
        // "27.10 and above" and a beta post are not the 27.10 release; no exact post, no notes.
        assert_eq!(pick("27.10"), None);
        assert_eq!(pick("27.1"), None);
        assert_eq!(pick("14.2"), None);
        assert_eq!(
            community_body(&topics[1].content),
            vec![
                "The September 2026 release of Photoshop 27.11 introduces Enhance Edge and more",
                "## Bug fixes - Thanks to you!",
                "Canvas Size dialog did not retain the previously used units",
                "Edit > Stroke used the previously selected color",
            ]
        );
        // Betas: posts of beta builds, matched by the exact build.
        let beta = vec![Topic { title: "Beta Build 30.8.28 Release Notes".into(), ..Default::default() }, Topic { title: "AE Beta 27.0x41 - Audio De-esser".into(), ..Default::default() }];
        assert!(community_pick(&beta, "30.8.28", &["Beta Build"], true).is_some());
        assert!(community_pick(&beta, "27.0", &["AE Beta"], true).is_none());
        let cc = vec![Topic { title: "Creative Cloud Desktop v6.11 has been released!".into(), ..Default::default() }];
        assert!(community_pick(&cc, "6.11.0.478.6", &["Creative Cloud Desktop"], false).is_some());
        assert!(community_page("<html>no list</html>").is_none());
    }

    #[test]
    fn whole_versions_in_titles() {
        assert_eq!(version_at("Adobe Bridge 16.0 is now available", "16.0.0"), Some((13, 17)));
        assert!(version_at("Photoshop v27.6 Is Live", "27.6").is_some());
        assert!(version_at("Photoshop 27.6.1", "27.6").is_none());
        assert!(version_at("Photoshop 127.6", "27.6").is_none());
        assert!(version_at("AE Beta 27.0x41", "27.0").is_none());
        assert!(version_at("Released 1.22.", "1.22.0").is_some());
    }

    #[test]
    fn steam_news_items_by_version() {
        let json = serde_json::json!({ "appnews": { "newsitems": [
            { "title": "Security updates", "date": 1791326577, "contents": "We released a security patch for Modeler." },
            { "title": "Modeler 1.21.0", "date": 1741711548, "contents": "Short note." },
            { "title": "Modeler 1.21.0 Released", "date": 1741711916, "url": "https://store.steampowered.com/news/app/1745780/view/1", "contents": "[p]New in this release:[/p][list][*]Primitives can be [b]beveled[/b][/*][*]Faster [url=https://x.y]export[/url][/*][/list][img]{STEAM_CLAN_IMAGE}/x.png[/img]" },
            { "title": "1.18 Release", "date": 1736360134, "contents": "Bug fixes and a new brush." }
        ] } })
        .to_string();
        let n = steam_news(&json, "1.21.0").expect("1.21.0");
        assert_eq!(n.lines, vec!["New in this release", "Primitives can be beveled", "Faster export"]);
        assert_eq!(n.date.as_deref(), Some("2025-03-11T16:51:56Z"));
        assert_eq!(steam_news(&json, "1.18.0").unwrap().lines, vec!["Bug fixes and a new brush"]);
        assert!(steam_news(&json, "1.22.8").is_none());
    }

    #[test]
    fn substance_all_changes_markdown() {
        let sampler = "## Version 6\n\n### **6.0.4**\n\n*(Released: September 24th, 2026)*\n\n**Changed**\\\n\\[Engine] Update Substance Engine to 9.6.1\\\n\n**Fixed**\\\n\\[Layers] Crash when adding image to emboss mask\\\n\\[Security] General fixes\n\n### **6.0.2**\n\n*(Released: June 25th, 2026)*\n\n**Added:**\n\n+------------------------------+\n| List                         |\n+------------------------------+\n| - \\[Assets] Check sbsar      |\n|   version and warn users     |\n| - \\[Captis] Add back option \\+ more |\n+------------------------------+\n\n## Version 5\n\n### **5.1.0 \u{ce}LE FLOTTANTE**\n\n**Fixed:**\n\n\\[UI] A fix\n\n### **4.2 DORAYAKI**\n\n**Fixed:**\n\n- See [the docs](/en/docs/x) for the new filter\n";
        let n = substance_changes(sampler, "6.0.4").expect("6.0.4");
        assert_eq!(n.lines, vec!["## Changed", "[Engine] Update Substance Engine to 9.6.1", "## Fixed", "[Layers] Crash when adding image to emboss mask", "[Security] General fixes"]);
        assert_eq!(n.date.as_deref(), Some("2026-09-24T00:00:00Z"));
        // Table cells, an item wrapped onto the next row, escapes.
        assert_eq!(substance_changes(sampler, "6.0.2").unwrap().lines, vec!["[Assets] Check sbsar version and warn users", "[Captis] Add back option + more"]);
        // A release name after the version, and "4.2" for 4.2.0.
        assert_eq!(substance_changes(sampler, "5.1.0").unwrap().lines, vec!["[UI] A fix"]);
        assert_eq!(substance_changes(sampler, "4.2.0").unwrap().lines, vec!["See the docs for the new filter"]);
        assert!(substance_changes(sampler, "6.0.3").is_none());
        // Painter's shape: a release date line and a summary.
        let painter = "### 12.1.5\n\nRelease date: **2026/09/15**\n\nSummary: **Minor release**\n\n**Fixed:**\n\n+----+\n| List |\n+----+\n| - Export image from shelf to a network does not work anymore |\n+----+\n\n---\n\n### 12.1.4\n";
        let n = substance_changes(painter, "12.1.5").unwrap();
        assert_eq!(n.lines, vec!["Export image from shelf to a network does not work anymore"]);
        assert_eq!(n.date.as_deref(), Some("2026-09-15T00:00:00Z"));
    }

    #[test]
    fn blender_release_pages() {
        // blender.org/releases/5-2/: 5.2.0 only links to the release notes, so it is left for them.
        let releases = r#"<details><summary class="d-flex">Blender 5.2.2 LTS<span class="fw-normal">September 15, 2026</span></summary><div class="box changelog"><h4>Changelog</h4><ul><li>Fix: VSE: Cannot prefetch strips <a href="x">#156001</a></li></ul></div></details>
<details><summary class="d-flex">Blender 5.2.0 LTS<span class="fw-normal">July 14, 2026</span></summary><div class="box changelog"><h4>Changelog</h4><p><a href="https://developer.blender.org/docs/release_notes/5.2/">Release Notes</a></p></div></details>"#;
        let m = blender_summaries(releases);
        assert_eq!(m.len(), 1);
        assert_eq!(m["5.2.2"].lines, vec!["Fix: VSE: Cannot prefetch strips"]);
        assert_eq!(m["5.2.2"].date.as_deref(), Some("2026-09-15T00:00:00Z"));
        // The 3.3 LTS page: a summary over several lines, Download before Changelog.
        let lts = "<details><summary>\n   Blender 3.3.21 LTS <span class=\"x\"> &#8211; July 16, 2024</span>\n</summary><h3>Download</h3><ul><li><a>Windows &#8211; Installer</a></li></ul><h3>Changelog</h3><ul><li>Assert when undoing Make Single User</li></ul></details>";
        assert_eq!(blender_summaries(lts)["3.3.21"].lines, vec!["Assert when undoing Make Single User"]);
        assert_eq!(blender_summaries(lts)["3.3.21"].date.as_deref(), Some("2024-07-16T00:00:00Z"));
        // Corrective releases pages with h1 (3.0 to 3.5) or h2 headings.
        let corrective = r##"<h1 id="blender-351">Blender 3.5.1<a class="headerlink" href="#blender-351">&para;</a></h1><p>Released on April 25th 2023, Blender 3.5.1 features the following bug fixes:</p><ul><li>Active/default UV map legacy conversion with name conflict   <a href="x">#106584</a></li><li>Add support for OpenPGL 0.5.0   <a href="y">85c7cc898d</a></li></ul>
<h2 id="blender-350">Blender 3.5.0</h2><ul><li>&#8216;Lattice Deform Selected&#8217; shows a warning</li></ul>"##;
        let c = blender_corrective(corrective);
        assert_eq!(c["3.5.1"].lines, vec!["Active/default UV map legacy conversion with name conflict", "Add support for OpenPGL 0.5.0"]);
        assert_eq!(c["3.5.1"].date.as_deref(), Some("2023-04-25T00:00:00Z"));
        assert_eq!(c["3.5.0"].lines, vec!["\u{2018}Lattice Deform Selected\u{2019} shows a warning"]);
    }

    #[test]
    fn list_items_lose_bare_refs_and_entities() {
        assert_eq!(without_trailing_refs("Crash on exit #119907"), "Crash on exit");
        assert_eq!(without_trailing_refs("Release: Update license.md. [bcfe4ce3b1600931fc854f70f5146a8bb2b36a4a]"), "Release: Update license.md.");
        assert_eq!(without_trailing_refs("Update bakers to version 3.23.0"), "Update bakers to version 3.23.0");
        assert_eq!(without_trailing_refs("Use the 2024 defaults"), "Use the 2024 defaults");
        assert_eq!(items("<li>&#8216;Lattice&#8217; &#8211; fixed &amp; faster &hellip;</li>"), vec!["\u{2018}Lattice\u{2019} - fixed & faster \u{2026}"]);
    }

    #[test]
    fn krita_posts_in_their_shapes() {
        // "Bug Fixes" or "Changes since" headings, h2 or h3.
        let bug_fixes = "<html><title>Krita 5.2.11 Released! | Krita</title><h1>Krita 5.2.11 Released!</h1><p>Intro</p><h3 id=bug-fixes>Bug Fixes</h3><ul><li>Fix an issue updating the canvas</li></ul><h2>Download</h2><ul><li>Not a note</li></ul></html>";
        assert_eq!(krita_changelog(bug_fixes), vec!["Fix an issue updating the canvas"]);
        let since = "<html><h1>Krita 5.2.3 Released!</h1><h2>Changes since 5.2.3 beta:</h2><ul><li>Various fixes to tool shortcuts</li></ul><h2>Download</h2></html>";
        assert_eq!(krita_changelog(since), vec!["Various fixes to tool shortcuts"]);
        let colon = "<html><h1>Krita 5.2.14 bugfix release!</h1><h3>New Color Picker Preview</h3><p>x</p><h3 id=changelog>Changelog:</h3><ul><li>Fix brush preview outlines</li></ul><h2>Download</h2></html>";
        assert_eq!(krita_changelog(colon), vec!["Fix brush preview outlines"]);
        // No such heading: the items between the title and the first Download heading.
        let plain = "<html><nav><ul><li>Menu entry</li></ul></nav><h1 style=margin-bottom:.5rem>Krita 5.2.9 Released!</h1><p>Today we're releasing Krita 5.2.9!</p><li>Add shortcuts to bezier curve</li><li>Fix Photobash Crash</li><h2>Download</h2><h3>Windows</h3><ul><li>64 bits installer</li></ul><footer><ul><li>Footer</li></ul></footer></html>";
        assert_eq!(krita_changelog(plain), vec!["Add shortcuts to bezier curve", "Fix Photobash Crash"]);
        // A post of prose only has no list.
        assert!(krita_changelog("<html><h1>Krita 5.2.16 bugfix release!</h1><p>Only text.</p><h2>Download</h2><ul><li>64 bits installer</li></ul></html>").is_empty());
        // The sitemap names posts with dots or dashes; 6.0.N comes with 5.3.N.
        let sitemap = "<urlset><url><loc>https://krita.org/en/posts/2026/krita-5.3.4-released/</loc></url><url><loc>https://krita.org/en/posts/2025/krita-5-2-9-released/</loc></url><url><loc>https://krita.org/en/posts/2023/krita-5-2-released/</loc></url><url><loc>https://krita.org/en/posts/2023/first-beta-for-krita-5-2-0-released/</loc></url><url><loc>https://krita.org/en/release-notes/krita-5-3-release-notes/</loc></url></urlset>";
        assert_eq!(krita_post_url(sitemap, &krita_post_version("6.0.4")).as_deref(), Some("https://krita.org/en/posts/2026/krita-5.3.4-released/"));
        assert_eq!(krita_post_version("6.0.4.1"), "5.3.4.1");
        assert_eq!(krita_post_url(sitemap, "5.2.9").as_deref(), Some("https://krita.org/en/posts/2025/krita-5-2-9-released/"));
        assert_eq!(krita_post_url(sitemap, "5.2.0").as_deref(), Some("https://krita.org/en/posts/2023/krita-5-2-released/"));
        assert_eq!(krita_post_url(sitemap, "5.3.4.1"), None);
        assert_eq!(krita_notes_url(sitemap, "5.3.0").as_deref(), Some("https://krita.org/en/release-notes/krita-5-3-release-notes/"));
        let notes = "<html><nav><ul><li>Menu entry here</li></ul></nav><h1>Krita 5.3 and 6.0 Release Notes</h1><p>Several years in the making, Krita 5.3 is a big release.</p><p>There should have been a video here but your browser does not seem to support it.</p><h2>Text Tool</h2><p>Text can now flow into shapes.</p><h2>Contributors</h2><p>Many people helped with this.</p><footer><p>Footer text goes here</p></footer></html>";
        assert_eq!(krita_release_notes(notes), vec!["Several years in the making, Krita 5.3 is a big release", "## Text Tool", "Text can now flow into shapes"]);
        assert!(krita_release_notes("<html><title>404 Page not found | Krita</title><h1>x</h1><p>Some text that is long</p></html>").is_empty());
    }
}
