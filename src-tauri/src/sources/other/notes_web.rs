//! Release notes the catalog does not name, found from what it does name: the release page of
//! the forge the download comes from (GitHub, GitLab, Codeberg), and the changelog page the
//! publisher's own site links to. Used for every app, after the catalog's own notes.

use crate::notes;
use once_cell::sync::Lazy;
use regex::Regex;

/// The release page of a download from a forge release: GitHub and Codeberg
/// ".../releases/download/<tag>/<file>", GitLab ".../-/releases/<tag>/downloads/<file>".
pub fn release_page_of(download: &str) -> Option<String> {
    for host in ["https://github.com/", "https://codeberg.org/"] {
        if let Some(rest) = download.strip_prefix(host) {
            let parts: Vec<&str> = rest.splitn(6, '/').collect();
            if let [o, r, "releases", "download", tag, _file] = parts.as_slice() {
                if !o.is_empty() && !r.is_empty() && !tag.is_empty() {
                    return Some(format!("{host}{o}/{r}/releases/tag/{tag}"));
                }
            }
        }
    }
    let rest = download.strip_prefix("https://gitlab.com/")?;
    let (project, after) = rest.split_once("/-/releases/")?;
    let tag = after.split('/').next().filter(|t| !t.is_empty())?;
    (!project.is_empty()).then(|| format!("https://gitlab.com/{project}/-/releases/{tag}"))
}

/// The notes of a GitLab release page, from GitLab's API (the page itself is built by script).
pub async fn gitlab_release_notes(page: &str) -> Vec<String> {
    let Some(rest) = page.strip_prefix("https://gitlab.com/") else { return vec![] };
    let Some((project, tag)) = rest.split_once("/-/releases/") else { return vec![] };
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let api = format!("https://gitlab.com/api/v4/projects/{}/releases/{}", enc(project), enc(tag.trim_end_matches('/')));
    let Some(body) = notes::fetch_page(&api).await else { return vec![] };
    let v: serde_json::Value = serde_json::from_str(&body).unwrap_or_default();
    v["description"].as_str().map(notes::text_lines).unwrap_or_default()
}

/// Links on a publisher's page that lead to its release notes, best first: "Changelog",
/// "Release notes", "What's new", "Version history" (in the link text or address). Only links
/// on the same site, at most `max`.
pub fn changelog_links(page_url: &str, html: &str, max: usize) -> Vec<String> {
    static A: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?is)<a\b[^>]*?href\s*=\s*["']([^"'#][^"']*)["'][^>]*>(.*?)</a>"#).unwrap());
    static STRONG: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)change[\s_-]*log|release[\s_-]*notes|what'?s[\s_-]*new|whatsnew|version[\s_-]*history|revision[\s_-]*history|news[\s_-]*and[\s_-]*changes").unwrap());
    let Ok(base) = url::Url::parse(page_url) else { return vec![] };
    let site = |u: &url::Url| u.host_str().map(|h| h.trim_start_matches("www.").to_ascii_lowercase());
    let Some(home) = site(&base) else { return vec![] };
    let mut out: Vec<String> = Vec::new();
    for c in A.captures_iter(html) {
        let (href, text) = (c[1].trim(), notes::strip_tags(&c[2]));
        if !(STRONG.is_match(&text) || STRONG.is_match(href)) || text.len() > 80 {
            continue;
        }
        let Ok(u) = base.join(href) else { continue };
        let Some(h) = site(&u) else { continue };
        let same = h == home || h.ends_with(&format!(".{home}")) || home.ends_with(&format!(".{h}"));
        if u.scheme() != "https" || !same {
            continue;
        }
        let s = u.as_str().split('#').next().unwrap_or("").to_string();
        if s != page_url && !out.contains(&s) {
            out.push(s);
        }
        if out.len() >= max {
            break;
        }
    }
    out
}

/// The change list for version `ver` from the publisher's site: its homepage's link to a
/// changelog, release notes or what's new page, read for that version. The page and the list.
pub async fn publisher_notes(home: &str, ver: &str) -> Option<(String, Vec<String>)> {
    let html = notes::page(home).await?;
    // The page the catalog names may be the changelog itself ("/changelog", "/release-notes").
    let path = url::Url::parse(home).map(|u| u.path().to_ascii_lowercase()).unwrap_or_default();
    if ["change", "release", "whatsnew", "whats-new", "history"].iter().any(|w| path.contains(w)) {
        let own = notes::page_notes(&html, ver);
        if !own.is_empty() {
            return Some((home.to_string(), own));
        }
    }
    for link in changelog_links(home, &html, 2) {
        if let Some(body) = notes::page(&link).await {
            let lines = crate::render::page_notes(&link, &body, ver).await;
            if !lines.is_empty() {
                return Some((link, lines));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forge_download_leads_to_its_release_page() {
        assert_eq!(release_page_of("https://github.com/o/app/releases/download/v1.2.3/app-1.2.3-setup.exe").as_deref(), Some("https://github.com/o/app/releases/tag/v1.2.3"));
        assert_eq!(release_page_of("https://codeberg.org/o/app/releases/download/1.2/app.msi").as_deref(), Some("https://codeberg.org/o/app/releases/tag/1.2"));
        assert_eq!(release_page_of("https://gitlab.com/group/sub/app/-/releases/v2.0/downloads/app.exe").as_deref(), Some("https://gitlab.com/group/sub/app/-/releases/v2.0"));
        assert_eq!(release_page_of("https://example.com/releases/download/v1/app.exe"), None);
        assert_eq!(release_page_of("https://github.com/o/app/archive/refs/tags/v1.zip"), None);
    }

    #[test]
    fn the_publishers_changelog_link_is_found_on_its_own_site() {
        let html = r#"<nav><a href="/download">Download</a><a href="https://twitter.com/app">News</a>
            <a href="/docs/changelog">Changelog</a><a href="https://evil.example/release-notes">Release notes</a>
            <a class="x" href="https://blog.app.example/whats-new/">What's new in App</a><a href="/about">About</a></nav>"#;
        assert_eq!(changelog_links("https://app.example/", html, 5), vec!["https://app.example/docs/changelog".to_string(), "https://blog.app.example/whats-new/".to_string()]);
        assert_eq!(changelog_links("https://app.example/", html, 1).len(), 1);
        assert!(changelog_links("not a url", html, 5).is_empty());
        // Plain http links are not followed.
        assert!(changelog_links("https://app.example/", r#"<a href="http://app.example/changelog">Changelog</a>"#, 5).is_empty());
    }
}
