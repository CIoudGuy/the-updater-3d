//! Pictures in release notes. Only the ones GitHub itself serves (screenshots attached to a
//! release or kept in a project) are shown: the app reads them, checks that they are pictures, and
//! keeps them with the icons, which is all the window may load. No other site is ever asked.

use crate::paths;
use once_cell::sync::Lazy;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use std::time::Duration;

/// Biggest picture read.
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// Pictures kept; the oldest go first.
const KEEP: usize = 300;

fn host_of(u: &url::Url) -> String {
    u.host_str().unwrap_or("").to_ascii_lowercase()
}

/// Is this where GitHub keeps a project's pictures (or where it sends a request for one)?
fn github_host(u: &url::Url) -> bool {
    let h = host_of(u);
    u.scheme() == "https"
        && (h == "github.com"
            || h.ends_with(".githubusercontent.com")
            // GitHub's own bucket for attached files ("github-production-user-asset-6210df").
            || (h.starts_with("github-production-user-asset-") && h.ends_with(".s3.amazonaws.com")))
}

/// A picture address in release notes that is shown: a file attached on GitHub
/// ("github.com/user-attachments/assets/<id>", "github.com/<owner>/<repo>/assets/<id>") or one
/// GitHub serves from a project or an older attachment.
pub fn allowed(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url.trim()) else { return false };
    if u.scheme() != "https" || !u.username().is_empty() || u.port().is_some() {
        return false;
    }
    let h = host_of(&u);
    let path = u.path();
    match h.as_str() {
        "github.com" => path.starts_with("/user-attachments/assets/") || path.split('/').nth(3) == Some("assets"),
        "user-images.githubusercontent.com" | "raw.githubusercontent.com" | "private-user-images.githubusercontent.com" => true,
        _ => false,
    }
}

/// Where a picture is kept: named by its address, with its file type.
fn stem(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.trim().as_bytes());
    format!("note-{}", hex::encode(&h.finalize()[..10]))
}

fn dir() -> PathBuf {
    let d = paths::icons().join("notes");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn kept(stem: &str) -> Option<PathBuf> {
    ["png", "jpg", "gif", "webp"].iter().map(|e| dir().join(format!("{stem}.{e}"))).find(|p| p.is_file())
}

/// Only redirects to GitHub's own hosts are followed.
static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("TheUpdater3D/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(45))
        .redirect(reqwest::redirect::Policy::custom(|a| if a.previous().len() < 5 && github_host(a.url()) { a.follow() } else { a.stop() }))
        .build()
        .expect("http client")
});

static GATE: Lazy<tokio::sync::Semaphore> = Lazy::new(|| tokio::sync::Semaphore::new(3));

/// The kind of picture these bytes are: the ones a release notes page shows (no SVG).
fn picture_kind(b: &[u8]) -> Option<&'static str> {
    match crate::sources::icons::sniff(b)? {
        k @ ("png" | "jpg" | "gif" | "webp") => Some(k),
        _ => None,
    }
}

async fn read(url: &str) -> Option<(Vec<u8>, &'static str)> {
    let _permit = GATE.acquire().await.ok()?;
    let mut resp = CLIENT.get(url).send().await.ok()?;
    if !resp.status().is_success() || !github_host(resp.url()) || resp.content_length().is_some_and(|n| n as usize > MAX_BYTES) {
        return None;
    }
    let mut body = Vec::new();
    while let Some(chunk) = resp.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_BYTES {
            return None;
        }
    }
    let kind = picture_kind(&body)?;
    Some((body, kind))
}

/// Drop the oldest pictures beyond `KEEP`.
fn prune() {
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if files.len() <= KEEP {
        return;
    }
    files.sort();
    for (_, p) in files.iter().take(files.len() - KEEP) {
        let _ = std::fs::remove_file(p);
    }
}

/// The picture at `url` on this PC, read now when it is not kept yet. None when it is not a
/// GitHub picture, can not be read, or is not a picture.
pub async fn get(url: &str) -> Option<PathBuf> {
    if !allowed(url) {
        return None;
    }
    let stem = stem(url);
    if let Some(p) = kept(&stem) {
        return Some(p);
    }
    let (bytes, kind) = read(url.trim()).await?;
    let path = dir().join(format!("{stem}.{kind}"));
    let tmp = path.with_extension("part");
    std::fs::write(&tmp, &bytes).ok()?;
    std::fs::rename(&tmp, &path).ok()?;
    prune();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_pictures_github_serves_are_shown() {
        for ok in [
            "https://github.com/user-attachments/assets/984de630-77a6-4a41-bd4b-87e915a6ec0c",
            "https://github.com/flameshot-org/flameshot/assets/46930769/0b1c2d3e",
            "https://user-images.githubusercontent.com/1/2.png",
            "https://raw.githubusercontent.com/o/r/main/shot.png",
        ] {
            assert!(allowed(ok), "{ok}");
        }
        for no in [
            "https://i.imgur.com/abc.png",
            "https://img.shields.io/badge/x-y-green",
            "http://github.com/user-attachments/assets/1",
            "https://github.com/o/r/blob/main/a.png",
            "https://github.com.evil.example/user-attachments/assets/1",
            "https://user@github.com/user-attachments/assets/1",
            "https://github.com:8443/user-attachments/assets/1",
            "not a url",
        ] {
            assert!(!allowed(no), "{no}");
        }
    }

    #[test]
    fn redirects_stay_on_githubs_hosts() {
        let u = |s: &str| url::Url::parse(s).unwrap();
        assert!(github_host(&u("https://github-production-user-asset-6210df.s3.amazonaws.com/1/2.png")));
        assert!(github_host(&u("https://private-user-images.githubusercontent.com/1/2.png?jwt=x")));
        assert!(!github_host(&u("https://other-bucket.s3.amazonaws.com/a.png")));
        assert!(!github_host(&u("https://tracker.example/pixel.gif")));
        assert!(!github_host(&u("http://github.com/a")));
    }

    #[test]
    fn only_real_pictures_are_kept() {
        assert_eq!(picture_kind(b"\x89PNG\r\n\x1a\n...."), Some("png"));
        assert_eq!(picture_kind(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("jpg"));
        assert_eq!(picture_kind(b"<?xml version=\"1.0\"?><svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"), None);
        assert_eq!(picture_kind(b"<!doctype html><html></html>"), None);
        // The same address is the same file.
        assert_eq!(stem("https://github.com/user-attachments/assets/1"), stem(" https://github.com/user-attachments/assets/1 "));
        assert_ne!(stem("https://github.com/user-attachments/assets/1"), stem("https://github.com/user-attachments/assets/2"));
    }

    /// Run with `cargo test noteimg -- --ignored`: reads a real attached screenshot.
    #[tokio::test]
    #[ignore]
    async fn live_attached_screenshot() {
        let p = get("https://github.com/user-attachments/assets/984de630-77a6-4a41-bd4b-87e915a6ec0c").await.expect("a picture");
        eprintln!("{}", p.display());
        assert!(p.extension().is_some_and(|e| e == "png"));
        let _ = std::fs::remove_file(p);
    }
}
