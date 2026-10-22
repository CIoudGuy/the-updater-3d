//! Resumable downloads with a shared bandwidth limit, pause and cancel.

use crate::http::CLIENT;
use crate::sources::Download;
use anyhow::{bail, Result};
use futures_util::StreamExt;
use parking_lot::Mutex;
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Shared token bucket. `limit` 0 means unlimited.
pub struct Throttle {
    limit: AtomicU64,
    state: Mutex<(f64, Instant)>,
}

impl Throttle {
    pub fn new() -> Self {
        Throttle { limit: AtomicU64::new(0), state: Mutex::new((0.0, Instant::now())) }
    }
    pub fn set(&self, bytes_per_sec: u64) {
        self.limit.store(bytes_per_sec, Ordering::Relaxed);
    }
    /// How long to wait before `n` more bytes may be used.
    fn take(&self, n: usize) -> Duration {
        let limit = self.limit.load(Ordering::Relaxed) as f64;
        if limit <= 0.0 {
            return Duration::ZERO;
        }
        let mut st = self.state.lock();
        let now = Instant::now();
        let elapsed = now.duration_since(st.1).as_secs_f64();
        // Allow at most a quarter second of burst.
        st.0 = (st.0 + elapsed * limit).min(limit * 0.25);
        st.1 = now;
        st.0 -= n as f64;
        if st.0 >= 0.0 {
            Duration::ZERO
        } else {
            Duration::from_secs_f64(-st.0 / limit)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Run,
    Pause,
    Cancel,
}

pub enum Outcome {
    Done,
    Paused,
    Canceled,
}

/// Progress counter shared with the job runner.
pub struct Progress {
    pub done: AtomicU64,
    /// Shown as the job step while set ("Can't reach github.com, retrying").
    pub note: Mutex<Option<String>>,
}

impl Progress {
    pub fn new() -> Self {
        Progress { done: AtomicU64::new(0), note: Mutex::new(None) }
    }
}

/// A name people recognise for where a download comes from.
fn source_name(url: &str) -> String {
    let host = crate::http::host(url);
    if host == "github.com" || host.ends_with("githubusercontent.com") {
        "GitHub's download server".into()
    } else if host.ends_with("adobe.com") {
        "Adobe's download server".into()
    } else {
        host.to_string()
    }
}

fn part_path(d: &Download) -> std::path::PathBuf {
    let mut p = d.dest.clone().into_os_string();
    p.push(".part");
    p.into()
}

/// Bytes already on disk for this download (finished or partial), used to resume progress.
pub fn existing_bytes(d: &Download) -> u64 {
    if let Ok(m) = std::fs::metadata(&d.dest) {
        if d.size.is_none_or(|s| s == m.len()) {
            return m.len();
        }
    }
    std::fs::metadata(part_path(d)).map(|m| m.len()).unwrap_or(0)
}

fn sha256_file(p: &std::path::Path) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// Check a finished file. A cached file that fails is deleted so it downloads again.
pub fn verify(d: &Download) -> Result<()> {
    let m = std::fs::metadata(&d.dest)?;
    if let Some(s) = d.size {
        if m.len() != s {
            bail!("size {} instead of {}", m.len(), s);
        }
    }
    if let Some(want) = &d.sha256 {
        let got = sha256_file(&d.dest)?;
        if !got.eq_ignore_ascii_case(want) {
            bail!("Checksum mismatch: expected {want}, got {got}");
        }
    }
    Ok(())
}

pub async fn fetch(d: &Download, progress: Arc<Progress>, throttle: Arc<Throttle>, mut ctl: watch::Receiver<Control>) -> Result<Outcome> {
    if let Some(dir) = d.dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Already downloaded earlier and still valid.
    if d.dest.exists() {
        let ok = tokio::task::spawn_blocking({
            let d = d.clone();
            move || verify(&d).is_ok()
        })
        .await
        .unwrap_or(false);
        if ok {
            progress.done.fetch_add(std::fs::metadata(&d.dest).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
            return Ok(Outcome::Done);
        }
        let _ = std::fs::remove_file(&d.dest);
    }

    let part = part_path(d);
    let mut attempt = 0u32;
    loop {
        attempt += 1;
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        let mut req = CLIENT.get(&d.url);
        for (k, v) in &d.headers {
            req = req.header(k, v);
        }
        if have > 0 {
            req = req.header("Range", format!("bytes={have}-"));
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) if attempt < 5 => {
                log::warn!("download {}: {e}, retrying", d.url);
                *progress.note.lock() = Some(format!("Can't reach {}, retrying ({} of 4)", source_name(&d.url), attempt));
                tokio::time::sleep(Duration::from_secs(2 * attempt as u64)).await;
                continue;
            }
            Err(e) => {
                let name = source_name(e.url().map(|u| u.as_str()).unwrap_or(&d.url));
                let why = if e.is_timeout() || e.is_connect() { "the connection timed out or was blocked" } else { "the request failed" };
                bail!("Could not reach {name} ({why}). A VPN, firewall or antivirus may be blocking it. Try again later.

Details: {e:?}");
            }
        };
        let status = resp.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            // The part file is complete or bigger than the file: start over.
            let _ = std::fs::remove_file(&part);
            continue;
        }
        if !status.is_success() {
            if attempt < 3 && status.is_server_error() {
                tokio::time::sleep(Duration::from_secs(3)).await;
                continue;
            }
            bail!("HTTP {} from {}", status, crate::http::host(&d.url));
        }
        let resumed = status == reqwest::StatusCode::PARTIAL_CONTENT && have > 0;
        let mut file = std::fs::OpenOptions::new().create(true).append(resumed).write(true).truncate(!resumed).open(&part)?;
        if resumed {
            file.seek(std::io::SeekFrom::End(0))?;
            progress.done.fetch_add(have, Ordering::Relaxed);
        }
        let mut written_this_try = 0u64;
        let mut stream = resp.bytes_stream();
        let mut failed: Option<anyhow::Error> = None;
        loop {
            let ctl_now = *ctl.borrow();
            match ctl_now {
                Control::Pause => {
                    file.flush()?;
                    return Ok(Outcome::Paused);
                }
                Control::Cancel => {
                    drop(file);
                    let _ = std::fs::remove_file(&part);
                    return Ok(Outcome::Canceled);
                }
                Control::Run => {}
            }
            let next = tokio::select! {
                n = stream.next() => n,
                _ = ctl.changed() => continue,
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    failed = Some(anyhow::anyhow!("The download stalled"));
                    break;
                }
            };
            match next {
                None => break,
                Some(Ok(chunk)) => {
                    if written_this_try == 0 {
                        *progress.note.lock() = None;
                    }
                    file.write_all(&chunk)?;
                    written_this_try += chunk.len() as u64;
                    progress.done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    let wait = throttle.take(chunk.len());
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                }
                Some(Err(e)) => {
                    failed = Some(e.into());
                    break;
                }
            }
        }
        file.flush()?;
        drop(file);
        if let Some(e) = failed {
            // Give back the bytes we will count again after resuming.
            progress.done.fetch_sub(written_this_try + if resumed { have } else { 0 }, Ordering::Relaxed);
            if attempt < 6 {
                log::warn!("download {}: {e}, resuming", d.url);
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            return Err(e);
        }
        std::fs::rename(&part, &d.dest)?;
        let check = tokio::task::spawn_blocking({
            let d = d.clone();
            move || verify(&d)
        })
        .await?;
        if let Err(e) = check {
            let _ = std::fs::remove_file(&d.dest);
            bail!("{} failed its check ({e}). It was deleted, try again.", d.dest.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
        }
        return Ok(Outcome::Done);
    }
}
