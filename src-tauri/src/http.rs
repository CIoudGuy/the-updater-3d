//! HTTP client plus a small on-disk cache that revalidates with ETag / Last-Modified,
//! so repeated checks are cheap (and GitHub 304 answers do not count against its rate limit).

use anyhow::{bail, Context, Result};
use once_cell::sync::Lazy;
use reqwest::header::{HeaderMap, HeaderValue, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Duration;

pub static CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("TheUpdater3D/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .gzip(true)
        .brotli(true)
        .build()
        .expect("http client")
});

/// For downloads: no automatic decompression, so byte ranges are ranges of the file itself, and
/// enough idle connections kept per server for downloads over several connections.
pub static DL_CLIENT: Lazy<reqwest::Client> = Lazy::new(|| {
    reqwest::Client::builder()
        .user_agent(concat!("TheUpdater3D/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(60))
        .gzip(false)
        .brotli(false)
        .pool_max_idle_per_host(8)
        .build()
        .expect("http client")
});

#[derive(Serialize, Deserialize, Default)]
struct Meta {
    url: String,
    etag: Option<String>,
    last_modified: Option<String>,
    fetched: i64,
}

fn key(url: &str, extra: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    h.update(extra.as_bytes());
    hex::encode(&h.finalize()[..12])
}

/// While set, cached answers are used no matter how old (fast startup before the real check).
pub static PREFER_CACHE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub struct Fetched {
    pub body: Vec<u8>,
    /// true when the server answered and the body is fresh (200 or 304)
    pub fresh: bool,
}

/// GET with revalidation. Falls back to the cached body when offline.
/// `max_age` skips the network entirely when the cached copy is younger.
pub async fn get_cached(url: &str, headers: &[(&str, &str)], max_age: Duration) -> Result<Fetched> {
    get_cached_in(&crate::paths::http_cache(), url, headers, max_age).await
}

/// `get_cached` with its cache in `dir`.
async fn get_cached_in(dir: &std::path::Path, url: &str, headers: &[(&str, &str)], max_age: Duration) -> Result<Fetched> {
    let extra: String = headers.iter().map(|(k, v)| format!("{k}={v};")).collect();
    let k = key(url, &extra);
    let body_path = dir.join(format!("{k}.body"));
    let meta_path = dir.join(format!("{k}.json"));
    let meta: Option<Meta> = std::fs::read(&meta_path).ok().and_then(|b| serde_json::from_slice(&b).ok());
    let now = chrono::Utc::now().timestamp();

    if PREFER_CACHE.load(std::sync::atomic::Ordering::Relaxed) && meta.is_some() {
        if let Ok(body) = std::fs::read(&body_path) {
            return Ok(Fetched { body, fresh: false });
        }
    }
    if let Some(m) = &meta {
        if (now - m.fetched) < max_age.as_secs() as i64 {
            if let Ok(body) = std::fs::read(&body_path) {
                return Ok(Fetched { body, fresh: true });
            }
        }
    }

    let mut req = CLIENT.get(url);
    let mut hm = HeaderMap::new();
    for (k, v) in headers {
        hm.insert(
            reqwest::header::HeaderName::from_bytes(k.as_bytes())?,
            HeaderValue::from_str(v)?,
        );
    }
    if body_path.exists() {
        if let Some(m) = &meta {
            if let Some(e) = &m.etag {
                hm.insert(IF_NONE_MATCH, HeaderValue::from_str(e)?);
            }
            if let Some(l) = &m.last_modified {
                hm.insert(IF_MODIFIED_SINCE, HeaderValue::from_str(l)?);
            }
        }
    }
    req = req.headers(hm).timeout(Duration::from_secs(90));

    match req.send().await {
        Ok(resp) => {
            let status = resp.status();
            if status == reqwest::StatusCode::NOT_MODIFIED {
                let body = std::fs::read(&body_path).context("cache body missing")?;
                let mut m = meta.unwrap_or_default();
                m.fetched = now;
                let _ = std::fs::write(&meta_path, serde_json::to_vec(&m)?);
                return Ok(Fetched { body, fresh: true });
            }
            if !status.is_success() {
                if let Ok(body) = std::fs::read(&body_path) {
                    log::warn!("{url}: HTTP {status}, using cached copy");
                    return Ok(Fetched { body, fresh: false });
                }
                bail!("HTTP {status} from {url}");
            }
            let etag = resp.headers().get(ETAG).and_then(|v| v.to_str().ok()).map(String::from);
            let lm = resp.headers().get(LAST_MODIFIED).and_then(|v| v.to_str().ok()).map(String::from);
            let body = resp.bytes().await?.to_vec();
            let _ = std::fs::write(&body_path, &body);
            let m = Meta { url: url.to_string(), etag, last_modified: lm, fetched: now };
            let _ = std::fs::write(&meta_path, serde_json::to_vec(&m)?);
            Ok(Fetched { body, fresh: true })
        }
        Err(e) => {
            if let Ok(body) = std::fs::read(&body_path) {
                log::warn!("{url}: {e}, using cached copy");
                return Ok(Fetched { body, fresh: false });
            }
            Err(e).with_context(|| format!("Could not reach {}", host(url)))
        }
    }
}

/// Parsed answers, kept by what was asked and the answer's checksum: an answer that did not
/// change (most of them: a 304, or a copy younger than its max age) is not parsed again. The
/// Adobe catalog takes 60 ms and 68 MB at its peak to parse; GitHub release lists tens of MB.
pub struct Parsed<T> {
    held: Lazy<parking_lot::Mutex<std::collections::HashMap<String, (Vec<u8>, T)>>>,
}

impl<T: Clone> Parsed<T> {
    pub const fn new() -> Self {
        Parsed { held: Lazy::new(Default::default) }
    }

    /// The answer `body` to `key` parsed, by `parse` unless it was parsed before. Failures are
    /// not kept.
    pub fn get_or(&self, key: &str, body: &[u8], parse: impl FnOnce(&[u8]) -> Result<T>) -> Result<T> {
        let sum = Sha256::digest(body).to_vec();
        if let Some((s, v)) = self.held.lock().get(key) {
            if *s == sum {
                return Ok(v.clone());
            }
        }
        let v = parse(body)?;
        self.held.lock().insert(key.to_string(), (sum, v.clone()));
        Ok(v)
    }
}

impl<T: Clone> Default for Parsed<T> {
    fn default() -> Self {
        Self::new()
    }
}

pub fn host(url: &str) -> &str {
    url.split("//").nth(1).and_then(|r| r.split('/').next()).unwrap_or(url)
}

/// Quick reachability probe used for the Offline state.
pub async fn online() -> bool {
    let probe = |url: &'static str| async move { CLIENT.head(url).timeout(Duration::from_secs(4)).send().await.is_ok() };
    let (a, b, c) = tokio::join!(probe("https://cdn-ffc.oobesaas.adobe.com/"), probe("https://api.github.com/"), probe("https://download.blender.org/"));
    a || b || c
}

/// Download a small icon to the icon cache once; returns the local path.
pub async fn cache_icon(url: &str, name: &str) -> Option<std::path::PathBuf> {
    let p = crate::paths::icons().join(name);
    if p.exists() && p.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Some(p);
    }
    let resp = CLIENT.get(url).timeout(Duration::from_secs(20)).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let bytes = resp.bytes().await.ok()?;
    std::fs::write(&p, &bytes).ok()?;
    Some(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A server with one file: its body and ETag can change; it answers 304 to a request that
    /// names the current ETag. Counts requests and the ones that came with an ETag.
    async fn server(state: Arc<parking_lot::Mutex<(String, String)>>, hits: Arc<AtomicUsize>, revalidated: Arc<AtomicUsize>) -> String {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let (state, hits, revalidated) = (state.clone(), hits.clone(), revalidated.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                    hits.fetch_add(1, Ordering::SeqCst);
                    let (body, etag) = state.lock().clone();
                    let sent = req.lines().find_map(|l| l.strip_prefix("if-none-match:")).map(|v| v.trim().to_string());
                    if sent.is_some() {
                        revalidated.fetch_add(1, Ordering::SeqCst);
                    }
                    let resp = if sent.as_deref() == Some(etag.to_ascii_lowercase().as_str()) {
                        let crlf = "\r\n";
                        format!("HTTP/1.1 304 Not Modified{crlf}ETag: {etag}{crlf}Content-Length: 0{crlf}Connection: close{crlf}{crlf}")
                    } else {
                        let crlf = "\r\n";
                        format!("HTTP/1.1 200 OK{crlf}ETag: {etag}{crlf}Content-Length: {}{crlf}Connection: close{crlf}{crlf}{body}", body.len())
                    };
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}/feed.json")
    }

    #[tokio::test]
    async fn feeds_are_asked_again_with_their_etag_and_unchanged_ones_are_not_sent_again() {
        let dir = std::env::temp_dir().join(format!("updater-http-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = Arc::new(parking_lot::Mutex::new(("one".to_string(), "\"v1\"".to_string())));
        let (hits, revalidated) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let url = server(state.clone(), hits.clone(), revalidated.clone()).await;
        let hour = Duration::from_secs(3600);

        let f = get_cached_in(&dir, &url, &[], hour).await.unwrap();
        assert_eq!((f.body.as_slice(), f.fresh, hits.load(Ordering::SeqCst)), (b"one".as_slice(), true, 1));
        // Younger than its max age: no request at all.
        let f = get_cached_in(&dir, &url, &[], hour).await.unwrap();
        assert_eq!((f.body.as_slice(), hits.load(Ordering::SeqCst)), (b"one".as_slice(), 1));
        // Older: asked again with the ETag, the server answers 304 and the kept body is used.
        let f = get_cached_in(&dir, &url, &[], Duration::ZERO).await.unwrap();
        assert_eq!((f.body.as_slice(), f.fresh, hits.load(Ordering::SeqCst), revalidated.load(Ordering::SeqCst)), (b"one".as_slice(), true, 2, 1));
        // Changed on the server: the new body.
        *state.lock() = ("two".into(), "\"v2\"".into());
        let f = get_cached_in(&dir, &url, &[], Duration::ZERO).await.unwrap();
        assert_eq!((f.body.as_slice(), f.fresh), (b"two".as_slice(), true));
        // Other request headers are another cache entry.
        let _ = get_cached_in(&dir, &url, &[("Accept", "x")], hour).await.unwrap();
        assert_eq!(hits.load(Ordering::SeqCst), 4);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unchanged_answer_is_not_parsed_again() {
        let p: Parsed<usize> = Parsed::new();
        let calls = AtomicUsize::new(0);
        let parse = |b: &[u8]| -> Result<usize> {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(b.len())
        };
        assert_eq!(p.get_or("a", b"abc", parse).unwrap(), 3);
        assert_eq!(p.get_or("a", b"abc", parse).unwrap(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // A changed answer, or another question with the same answer: parsed.
        assert_eq!(p.get_or("a", b"abcd", parse).unwrap(), 4);
        assert_eq!(p.get_or("b", b"abcd", parse).unwrap(), 4);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        // A failure is not kept: the next time parses again.
        assert!(p.get_or("c", b"x", |_| -> Result<usize> { bail!("bad") }).is_err());
        assert_eq!(p.get_or("c", b"x", parse).unwrap(), 1);
    }

    #[tokio::test]
    async fn an_unreachable_server_gives_the_kept_copy() {
        let dir = std::env::temp_dir().join(format!("updater-http-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let state = Arc::new(parking_lot::Mutex::new(("kept".to_string(), "\"k\"".to_string())));
        let url = server(state, Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0))).await;
        get_cached_in(&dir, &url, &[], Duration::ZERO).await.unwrap();
        // Same cache entry, but nothing listens there any more.
        let k = key(&url, "");
        let gone = "http://127.0.0.1:9/feed.json";
        for ext in ["body", "json"] {
            std::fs::copy(dir.join(format!("{k}.{ext}")), dir.join(format!("{}.{ext}", key(gone, "")))).unwrap();
        }
        let f = get_cached_in(&dir, gone, &[], Duration::ZERO).await.unwrap();
        assert_eq!((f.body.as_slice(), f.fresh), (b"kept".as_slice(), false));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
