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
    let extra: String = headers.iter().map(|(k, v)| format!("{k}={v};")).collect();
    let k = key(url, &extra);
    let dir = crate::paths::http_cache();
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
