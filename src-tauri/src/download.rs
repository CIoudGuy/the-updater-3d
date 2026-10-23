//! Downloads: resumable, with a shared bandwidth limit, pause and cancel. Big files from servers
//! that serve byte ranges come over several connections at once into one file, and every piece
//! continues where it stopped after a pause, a crash or a restart.

use crate::http::DL_CLIENT;
use crate::sources::Download;
use anyhow::{anyhow, bail, Result};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use md5::Md5;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::watch;

/// Shared bandwidth limit over every connection of every download. `limit` 0 means unlimited.
/// Each caller is given the moment its bytes fit under the limit (with a quarter second of
/// burst), so connections waiting at the same time share the limit instead of each waiting for
/// all of the others.
pub struct Throttle {
    limit: AtomicU64,
    /// When the bytes taken so far are paid off.
    next: Mutex<Instant>,
}

impl Throttle {
    pub fn new() -> Self {
        Throttle { limit: AtomicU64::new(0), next: Mutex::new(Instant::now()) }
    }
    pub fn set(&self, bytes_per_sec: u64) {
        self.limit.store(bytes_per_sec, Ordering::Relaxed);
    }
    fn limit(&self) -> u64 {
        self.limit.load(Ordering::Relaxed)
    }
    /// How long to wait before `n` more bytes may be used.
    fn take(&self, n: usize) -> Duration {
        let limit = self.limit.load(Ordering::Relaxed) as f64;
        if limit <= 0.0 {
            return Duration::ZERO;
        }
        let now = Instant::now();
        let mut next = self.next.lock();
        let floor = now.checked_sub(Duration::from_millis(250)).unwrap_or(now);
        let base = (*next).max(floor);
        *next = base + Duration::from_secs_f64(n as f64 / limit);
        next.saturating_duration_since(now)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Run,
    Pause,
    Cancel,
}

#[derive(Debug)]
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
    /// What the download does right now, for the job's detail line ("Downloading over 6
    /// connections from github.com", "Checking the SHA-256 checksum").
    pub detail: Mutex<Option<String>>,
    /// Connections open right now.
    pub conns: AtomicUsize,
}

impl Progress {
    pub fn new() -> Self {
        Progress { done: AtomicU64::new(0), note: Mutex::new(None), detail: Mutex::new(None), conns: AtomicUsize::new(0) }
    }
    fn set_detail(&self, s: impl Into<String>) {
        *self.detail.lock() = Some(s.into());
    }
}

/// When a download is split over several connections, and how far.
#[derive(Clone, Copy, Debug)]
pub struct Tuning {
    /// Files at least this big are fetched over several connections.
    pub multi_min: u64,
    pub start_conns: usize,
    pub max_conns: usize,
    /// The smallest piece another connection takes over from a running one.
    pub min_split: u64,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning { multi_min: 8 * 1024 * 1024, start_conns: 4, max_conns: 6, min_split: 2 * 1024 * 1024 }
    }
}

impl Tuning {
    /// The default, with as many connections as this PC is tuned for (fewer on a low end PC).
    pub fn for_this_pc() -> Self {
        let m = crate::machine::tuned();
        Tuning { start_conns: m.start_conns, max_conns: m.conns_per_file, ..Tuning::default() }
    }
}

/// Connections to one server, over all downloads at once.
const HOST_MAX: usize = 8;
/// Received data is written in blocks of this size.
const FLUSH_AT: usize = 256 * 1024;

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

fn with_suffix(dest: &Path, suffix: &str) -> PathBuf {
    let mut p = dest.as_os_str().to_owned();
    p.push(suffix);
    p.into()
}

/// The unfinished file.
pub fn part_path(dest: &Path) -> PathBuf {
    with_suffix(dest, ".part")
}

/// What is known about a download on disk: which file it is and how far each piece got.
pub fn sidecar(dest: &Path) -> PathBuf {
    with_suffix(dest, ".dl.json")
}

/// One byte range of the file: `pos` is how far it is written.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Seg {
    pub start: u64,
    pub end: u64,
    pub pos: u64,
    #[serde(skip)]
    pub id: u32,
    #[serde(skip)]
    pub busy: bool,
}

impl Seg {
    fn left(&self) -> u64 {
        self.end.saturating_sub(self.pos)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Meta {
    /// Which file this is (`file_key`): a partial file of another file is never continued.
    pub key: String,
    #[serde(default)]
    pub size: Option<u64>,
    /// The pieces of a download over several connections; empty for one stream appended to
    /// the part file (its length is how far it got).
    #[serde(default)]
    pub segs: Vec<Seg>,
    /// The download finished and passed its checks.
    #[serde(default)]
    pub complete: bool,
}

/// Identifies the file a download is for. A published checksum names the content itself;
/// without one the address and size do, so a new version at a new address never reuses an old file.
pub fn file_key(d: &Download) -> String {
    let nonempty = |s: &Option<String>| s.as_deref().map(str::trim).filter(|s| !s.is_empty()).map(str::to_ascii_lowercase);
    if let Some(h) = nonempty(&d.sha256) {
        return format!("sha256:{h}");
    }
    if let Some(h) = nonempty(&d.sha512) {
        return format!("sha512:{h}");
    }
    if let Some(h) = nonempty(&d.md5) {
        return format!("md5:{h}");
    }
    format!("url:{}|size:{}", d.url, d.size.map(|s| s.to_string()).unwrap_or_default())
}

fn has_checksum(d: &Download) -> bool {
    file_key(d).split(':').next() != Some("url")
}

pub fn load_meta(p: &Path) -> Option<Meta> {
    let mut m: Meta = serde_json::from_slice(&std::fs::read(p).ok()?).ok()?;
    // A damaged note (a piece ending before it starts) is ignored: the download starts over.
    if m.segs.iter().any(|s| s.start > s.end) {
        return None;
    }
    for (k, s) in m.segs.iter_mut().enumerate() {
        s.id = k as u32;
        s.busy = false;
        s.pos = s.pos.clamp(s.start, s.end);
    }
    Some(m)
}

/// Written to a temporary file and renamed, so a crash never leaves half a file.
fn save_meta(p: &Path, m: &Meta) {
    let tmp = with_suffix(p, ".tmp");
    if std::fs::write(&tmp, serde_json::to_vec(m).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

fn discard(dest: &Path) {
    let _ = std::fs::remove_file(part_path(dest));
    let _ = std::fs::remove_file(sidecar(dest));
}

/// Remove a finished or unfinished download and what is kept about it.
pub fn remove(dest: &Path) {
    let _ = std::fs::remove_file(dest);
    discard(dest);
}

/// Bytes written so far of an unfinished download of this file.
fn partial_bytes(d: &Download) -> u64 {
    match load_meta(&sidecar(&d.dest)) {
        Some(m) if m.key == file_key(d) && !m.complete && !m.segs.is_empty() => m.segs.iter().map(|s| s.pos - s.start).sum(),
        Some(m) if m.key != file_key(d) || m.complete => 0,
        _ => std::fs::metadata(part_path(&d.dest)).map(|m| m.len()).unwrap_or(0),
    }
}

/// Bytes already on disk for this download (finished or partial), used to resume progress.
pub fn existing_bytes(d: &Download) -> u64 {
    if let Ok(m) = std::fs::metadata(&d.dest) {
        if d.size.is_none_or(|s| s == m.len()) {
            return m.len();
        }
    }
    partial_bytes(d)
}

/// May a file downloaded earlier be used? It must pass its checks again, and a file without a
/// checksum must be the one this download names (same address and size).
fn reusable(d: &Download) -> bool {
    if !d.dest.exists() {
        return false;
    }
    if !has_checksum(d) && !load_meta(&sidecar(&d.dest)).is_some_and(|m| m.complete && m.key == file_key(d)) {
        return false;
    }
    verify(d).is_ok()
}

/// Delete unfinished downloads (and their notes) that no job needs any more: what a crash, a
/// failed job or a new version left in the downloads folder. `keep` are destinations of
/// downloads still to come.
pub fn sweep(root: &Path, keep: &std::collections::HashSet<PathBuf>) -> u64 {
    let mut freed = 0u64;
    let mut stack = vec![root.to_path_buf()];
    let keep: std::collections::HashSet<String> = keep.iter().map(|p| p.to_string_lossy().to_ascii_lowercase()).collect();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            let s = p.to_string_lossy().to_string();
            let low = s.to_ascii_lowercase();
            let base = [".part", ".dl.json.tmp", ".dl.json"].iter().find_map(|x| low.strip_suffix(x).map(|b| (b.to_string(), *x)));
            let Some((base, kind)) = base else { continue };
            if keep.contains(&base) {
                continue;
            }
            // The note of a finished download stays with its file (kept installers).
            if kind == ".dl.json" && Path::new(&s[..base.len()]).exists() && load_meta(&p).is_some_and(|m| m.complete) {
                continue;
            }
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            if std::fs::remove_file(&p).is_ok() {
                freed += len;
            }
        }
    }
    freed
}

/// Installers in the download cache that nothing needs are deleted after this long.
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(14 * 24 * 3600);
/// Above this the oldest installers go, until the rest fits.
pub const CACHE_CAP: u64 = 5 * 1024 * 1024 * 1024;

/// A file in the download cache, for `cache_victims`.
#[derive(Clone, Debug)]
pub struct CacheFile {
    pub path: PathBuf,
    pub size: u64,
    /// Since it was last written.
    pub age: Duration,
    /// A job that is not finished downloads to it or will run it: never deleted.
    pub in_use: bool,
    /// A finished installer kept for "Keep previous version" (roll back without downloading
    /// again): no age limit, and it goes last when the cache is too big.
    pub keep: bool,
    /// The update that used it is done (an installer of this app's own earlier update).
    pub done: bool,
}

/// What to delete from the download cache: installers whose update is done, then those older
/// than `max_age`, unless kept for rolling back, then the oldest (kept ones last) until the rest
/// fits in `cap`. Never a file a job still needs.
pub fn cache_victims(files: &[CacheFile], max_age: Duration, cap: u64) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut left: Vec<&CacheFile> = Vec::new();
    for f in files {
        if !f.in_use && !f.keep && (f.done || f.age > max_age) {
            out.push(f.path.clone());
        } else {
            left.push(f);
        }
    }
    let mut total: u64 = left.iter().map(|f| f.size).sum();
    let mut can_go: Vec<&CacheFile> = left.into_iter().filter(|f| !f.in_use).collect();
    can_go.sort_by(|a, b| a.keep.cmp(&b.keep).then(b.age.cmp(&a.age)));
    for f in can_go {
        if total <= cap {
            break;
        }
        total -= f.size;
        out.push(f.path.clone());
    }
    out
}

/// Apply `cache_victims` to the download cache in `root`. `in_use`: destinations of jobs that
/// are not finished. `keep_previous`: the "Keep previous version" setting. Partial downloads are
/// left to `sweep`. Returns the bytes freed.
pub fn trim_cache(root: &Path, in_use: &std::collections::HashSet<PathBuf>, keep_previous: bool) -> u64 {
    let low_path = |p: &Path| p.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    // A job's file, and what it unpacked next to it (Adobe's update tool).
    let busy: Vec<String> = in_use.iter().map(|p| low_path(p)).collect();
    let busy_dirs: Vec<String> = in_use.iter().filter_map(|p| p.parent()).filter(|d| *d != root).map(|d| format!("{}\\", low_path(d))).collect();
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    let now = std::time::SystemTime::now();
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            let low = p.to_string_lossy().to_ascii_lowercase();
            if [".part", ".dl.json", ".dl.json.tmp"].iter().any(|x| low.ends_with(x)) {
                continue;
            }
            let Ok(m) = e.metadata() else { continue };
            let age = m.modified().ok().and_then(|t| now.duration_since(t).ok()).unwrap_or_default();
            // The folder right under the cache: "self" holds this app's own updates, which are
            // done once this app runs again; installers of the other apps may be kept to roll back.
            let top = p.strip_prefix(root).ok().and_then(|r| r.components().next()).map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let finished = load_meta(&sidecar(&p)).is_some_and(|m| m.complete);
            files.push(CacheFile {
                in_use: busy.contains(&low_path(&p)) || busy_dirs.iter().any(|d| low_path(&p).starts_with(d.as_str())),
                keep: keep_previous && finished && ["other", "tools", "craft", "blender"].contains(&top.as_str()),
                done: top == "self" && age > Duration::from_secs(600),
                path: p,
                size: m.len(),
                age,
            });
        }
    }
    let mut freed = 0;
    for p in cache_victims(&files, CACHE_MAX_AGE, CACHE_CAP) {
        let len = files.iter().find(|f| f.path == p).map(|f| f.size).unwrap_or(0);
        remove(&p);
        if !p.exists() {
            freed += len;
            // Folders left empty go too (never the cache folder itself).
            let mut d = p.parent();
            while let Some(dir) = d.filter(|d| *d != root && d.starts_with(root)) {
                if std::fs::remove_dir(dir).is_err() {
                    break;
                }
                d = dir.parent();
            }
        }
    }
    freed
}

fn hash_file<H: Digest>(p: &std::path::Path, mut h: H) -> Result<String> {
    let mut f = std::fs::File::open(p)?;
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

/// Unity publishes "md5-<base64>" as a file's integrity value; this is its lowercase hex form.
pub fn md5_hex_from_integrity(integrity: &str) -> Option<String> {
    use base64::Engine;
    let b64 = integrity.strip_prefix("md5-")?;
    let raw = base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?;
    (raw.len() == 16).then(|| hex::encode(raw))
}

/// What the check of a finished download looks at, for the job's detail line.
pub fn check_text(d: &Download) -> String {
    let sum = if d.sha256.as_deref().is_some_and(|s| !s.is_empty()) {
        Some("SHA-256")
    } else if d.sha512.as_deref().is_some_and(|s| !s.is_empty()) {
        Some("SHA-512")
    } else if d.md5.as_deref().is_some_and(|s| !s.is_empty()) {
        Some("MD5")
    } else {
        None
    };
    match (sum, d.signer.as_deref().filter(|s| !s.is_empty())) {
        (Some(s), Some(by)) => format!("Checking the {s} checksum and the signature by {by}"),
        (Some(s), None) => format!("Checking the {s} checksum"),
        (None, Some(by)) => format!("Checking the signature by {by}"),
        (None, None) => "Checking the file size".into(),
    }
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
        let got = hash_file(&d.dest, Sha256::new())?;
        if !got.eq_ignore_ascii_case(want) {
            bail!("Checksum mismatch: expected {want}, got {got}");
        }
    }
    if let Some(want) = &d.md5 {
        let got = hash_file(&d.dest, Md5::new())?;
        if !got.eq_ignore_ascii_case(want) {
            bail!("Checksum mismatch: expected MD5 {want}, got {got}");
        }
    }
    if let Some(want) = &d.sha512 {
        let got = hash_file(&d.dest, sha2::Sha512::new())?;
        if !got.eq_ignore_ascii_case(want) {
            bail!("Checksum mismatch: expected SHA-512 {want}, got {got}");
        }
    }
    if let Some(want) = &d.signer {
        match crate::winutil::authenticode_signer(&d.dest) {
            Some(got) if &got == want => {}
            Some(got) => bail!("Signature check failed: signed by {got}, not {want}"),
            None => bail!("Signature check failed: not validly signed by {want}"),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- byte ranges

/// "bytes 0-99/1000" -> (0, 99, Some(1000)); "bytes 0-99/*" -> (0, 99, None).
pub fn parse_content_range(v: &str) -> Option<(u64, u64, Option<u64>)> {
    let rest = v.trim().strip_prefix("bytes")?.trim();
    let (range, total) = rest.split_once('/')?;
    let (a, b) = range.split_once('-')?;
    let total = if total.trim() == "*" { None } else { Some(total.trim().parse().ok()?) };
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?, total))
}

fn content_range(r: &reqwest::Response) -> Option<(u64, u64, Option<u64>)> {
    r.headers().get(reqwest::header::CONTENT_RANGE).and_then(|v| v.to_str().ok()).and_then(parse_content_range)
}

/// A piece for a connection that is free: a piece nobody fetches first, else the second half of
/// the piece with the most left (when both halves are at least `min_split`). Marks it busy.
pub fn claim(segs: &mut Vec<Seg>, min_split: u64) -> Option<u32> {
    if let Some(s) = segs.iter_mut().filter(|s| !s.busy && s.left() > 0).min_by_key(|s| s.start) {
        s.busy = true;
        return Some(s.id);
    }
    let (k, s) = segs.iter().enumerate().filter(|(_, s)| s.busy && s.left() >= 2 * min_split.max(1)).max_by_key(|(_, s)| s.left())?;
    let mid = s.pos + s.left() / 2;
    let id = segs.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    let new = Seg { start: mid, end: s.end, pos: mid, id, busy: true };
    segs[k].end = mid;
    segs.insert(k + 1, new);
    Some(id)
}

/// Is there anything a new connection could take over?
pub fn has_work(segs: &[Seg], min_split: u64) -> bool {
    segs.iter().any(|s| (!s.busy && s.left() > 0) || (s.busy && s.left() >= 2 * min_split.max(1)))
}

fn left_total(segs: &[Seg]) -> u64 {
    segs.iter().map(Seg::left).sum()
}

/// Connections per server, over every download running at once.
static HOSTS: Lazy<Mutex<HashMap<String, usize>>> = Lazy::new(|| Mutex::new(HashMap::new()));

struct HostSlot(String);

impl HostSlot {
    /// The first connection of a download always gets a slot.
    fn take(host: &str) -> HostSlot {
        *HOSTS.lock().entry(host.to_string()).or_default() += 1;
        HostSlot(host.to_string())
    }
    fn try_take(host: &str) -> Option<HostSlot> {
        let mut m = HOSTS.lock();
        let n = m.entry(host.to_string()).or_default();
        if *n >= HOST_MAX {
            return None;
        }
        *n += 1;
        Some(HostSlot(host.to_string()))
    }
}

impl Drop for HostSlot {
    fn drop(&mut self) {
        if let Some(n) = HOSTS.lock().get_mut(&self.0) {
            *n = n.saturating_sub(1);
        }
    }
}

#[cfg(windows)]
fn make_sparse(f: &std::fs::File) {
    // A file written at offsets far into it would otherwise be filled with zeros up to there
    // first, which for a big file costs as much as writing it.
    use std::os::windows::io::AsRawHandle;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::IO::DeviceIoControl;
    const FSCTL_SET_SPARSE: u32 = 0x0009_00C4;
    let mut ret = 0u32;
    unsafe {
        let _ = DeviceIoControl(HANDLE(f.as_raw_handle() as *mut _), FSCTL_SET_SPARSE, None, 0, None, 0, Some(&mut ret), None);
    }
}

#[cfg(not(windows))]
fn make_sparse(_f: &std::fs::File) {}

fn write_at(f: &std::fs::File, mut buf: &[u8], mut off: u64) -> std::io::Result<()> {
    #[cfg(windows)]
    use std::os::windows::fs::FileExt;
    #[cfg(unix)]
    use std::os::unix::fs::FileExt;
    while !buf.is_empty() {
        #[cfg(windows)]
        let n = f.seek_write(buf, off)?;
        #[cfg(unix)]
        let n = f.write_at(buf, off)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        off += n as u64;
    }
    Ok(())
}

/// Resolves once the control value changes (never, when its sender is gone).
async fn changed(ctl: &mut watch::Receiver<Control>) {
    if ctl.changed().await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// Sleep, ending early on Pause or Cancel (returns false then).
async fn nap(ctl: &mut watch::Receiver<Control>, d: Duration) -> bool {
    let end = Instant::now() + d;
    loop {
        if *ctl.borrow() != Control::Run {
            return false;
        }
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        tokio::select! {
            _ = changed(ctl) => {}
            _ = tokio::time::sleep(left) => return true,
        }
    }
}

fn retry_after(r: &reqwest::Response) -> Option<Duration> {
    r.headers().get(reqwest::header::RETRY_AFTER)?.to_str().ok()?.trim().parse::<u64>().ok().map(|s| Duration::from_secs(s.min(60)))
}

fn with_headers(mut req: reqwest::RequestBuilder, d: &Download) -> reqwest::RequestBuilder {
    for (k, v) in &d.headers {
        req = req.header(k, v);
    }
    req
}

// ---------------------------------------------------------------- the download

pub async fn fetch(d: &Download, progress: Arc<Progress>, throttle: Arc<Throttle>, ctl: watch::Receiver<Control>) -> Result<Outcome> {
    fetch_tuned(d, progress, throttle, ctl, Tuning::for_this_pc()).await
}

pub async fn fetch_tuned(d: &Download, progress: Arc<Progress>, throttle: Arc<Throttle>, mut ctl: watch::Receiver<Control>, t: Tuning) -> Result<Outcome> {
    if let Some(dir) = d.dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // Already downloaded earlier and still valid.
    if d.dest.exists() {
        progress.set_detail(format!("Already downloaded. {}", check_text(d)));
        let ok = tokio::task::spawn_blocking({
            let d = d.clone();
            move || reusable(&d)
        })
        .await
        .unwrap_or(false);
        if ok {
            progress.done.store(std::fs::metadata(&d.dest).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
            return Ok(Outcome::Done);
        }
        remove(&d.dest);
    }
    // A partial file is only continued when it is of this very file. One without a note is from
    // an older version of the app: it continues as one stream, the check at the end decides.
    let key = file_key(d);
    if load_meta(&sidecar(&d.dest)).is_some_and(|m| m.key != key || m.complete) {
        discard(&d.dest);
    }
    let mut fresh_left = true;
    loop {
        let resumed_from = partial_bytes(d);
        let o = transfer(d, &key, &progress, &throttle, &mut ctl, t).await?;
        if !matches!(o, Outcome::Done) {
            *progress.note.lock() = None;
            return Ok(o);
        }
        *progress.note.lock() = None;
        progress.set_detail(check_text(d));
        std::fs::rename(part_path(&d.dest), &d.dest)?;
        let check = tokio::task::spawn_blocking({
            let d = d.clone();
            move || verify(&d)
        })
        .await?;
        match check {
            Ok(()) => {
                let size = std::fs::metadata(&d.dest).map(|m| m.len()).ok();
                save_meta(&sidecar(&d.dest), &Meta { key: key.clone(), size, segs: vec![], complete: true });
                return Ok(Outcome::Done);
            }
            Err(e) => {
                remove(&d.dest);
                // What was kept from before (a crash can leave a damaged piece) is the likely
                // cause: download it once more from the start.
                if resumed_from > 0 && fresh_left {
                    log::warn!("download {}: {e:#} after resuming, downloading again from the start", d.url);
                    fresh_left = false;
                    progress.done.store(0, Ordering::Relaxed);
                    continue;
                }
                bail!("{} failed its check ({e}). It was deleted, try again.", d.dest.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
            }
        }
    }
}

enum MultiEnd {
    Done,
    Paused,
    Canceled,
    /// The server sent another file than before (size changed): start over.
    Restart,
    /// The server does not serve ranges after all: start over as one stream.
    NoRanges,
}

/// Fetch into the part file: continues an earlier download of the same file, over several
/// connections when the server allows it. Done means the part file is complete.
async fn transfer(d: &Download, key: &str, progress: &Progress, throttle: &Throttle, ctl: &mut watch::Receiver<Control>, t: Tuning) -> Result<Outcome> {
    let part = part_path(&d.dest);
    let side = sidecar(&d.dest);
    let mut send_fails = 0u32;
    let mut server_fails = 0u32;
    // Since when the network has been failing; downloads wait it out (sleep, Wi-Fi switch).
    let mut offline_since: Option<Instant> = None;
    let mut backoff = Duration::from_secs(2);
    let mut no_multi = t.max_conns <= 1;
    loop {
        if let Some(o) = stop_now(ctl, &d.dest) {
            return Ok(o);
        }
        // The pieces of an earlier download over several connections.
        if let Some(m) = load_meta(&side).filter(|m| m.key == key && !m.complete && !m.segs.is_empty()) {
            if no_multi {
                discard(&d.dest);
                continue;
            }
            match multi(d, m, None, progress, throttle, ctl, t).await? {
                MultiEnd::Done => return Ok(Outcome::Done),
                MultiEnd::Paused => return Ok(Outcome::Paused),
                MultiEnd::Canceled => return Ok(Outcome::Canceled),
                MultiEnd::Restart => discard(&d.dest),
                MultiEnd::NoRanges => {
                    discard(&d.dest);
                    no_multi = true;
                }
            }
            continue;
        }
        let have = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
        // Ask for a range when resuming, or to learn whether the server serves ranges.
        let ask_range = have > 0 || (!no_multi && d.size.is_none_or(|s| s >= t.multi_min));
        let mut req = with_headers(DL_CLIENT.get(&d.url), d);
        if ask_range {
            req = req.header("Range", format!("bytes={have}-"));
        }
        if have > 0 {
            progress.set_detail(format!("Resuming from {}", crate::core::fmt_bytes(have)));
        } else {
            progress.set_detail(format!("Connecting to {}", crate::http::host(&d.url)));
        }
        let resp = match req.send().await {
            Ok(r) => r,
            Err(e) if is_network_error(&e) && net_wait_left(&mut offline_since) => {
                log::warn!("download {}: {e}, waiting for the network", d.url);
                *progress.note.lock() = Some(format!("Waiting for the network, can't reach {}", source_name(&d.url)));
                if let Some(o) = net_pause(ctl, &d.dest, &mut backoff).await {
                    return Ok(o);
                }
                continue;
            }
            Err(e) if !is_network_error(&e) && send_fails < 4 => {
                send_fails += 1;
                log::warn!("download {}: {e}, retrying", d.url);
                *progress.note.lock() = Some(format!("Can't reach {}, retrying ({} of 4)", source_name(&d.url), send_fails));
                nap(ctl, Duration::from_secs(2 * send_fails as u64)).await;
                continue;
            }
            Err(e) => {
                let name = source_name(e.url().map(|u| u.as_str()).unwrap_or(&d.url));
                let why = if e.is_timeout() || e.is_connect() { "the connection timed out or was blocked" } else { "the request failed" };
                bail!("Could not reach {name} ({why}). A VPN, firewall or antivirus may be blocking it. Try again later.

Details: {e:?}");
            }
        };
        if let Some(o) = stop_now(ctl, &d.dest) {
            return Ok(o);
        }
        let status = resp.status();
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            // The part file is complete or bigger than the file: start over.
            discard(&d.dest);
            continue;
        }
        if (status == reqwest::StatusCode::TOO_MANY_REQUESTS || status == reqwest::StatusCode::SERVICE_UNAVAILABLE) && server_fails < 4 {
            server_fails += 1;
            *progress.note.lock() = Some(format!("{} is busy, trying again", source_name(&d.url)));
            let wait = retry_after(&resp).unwrap_or(Duration::from_secs(3 * server_fails as u64));
            drop(resp);
            nap(ctl, wait).await;
            continue;
        }
        if !status.is_success() {
            if server_fails < 2 && status.is_server_error() {
                server_fails += 1;
                nap(ctl, Duration::from_secs(3)).await;
                continue;
            }
            bail!("HTTP {} from {}", status, crate::http::host(&d.url));
        }
        let partial = status == reqwest::StatusCode::PARTIAL_CONTENT;
        let cr = if partial { content_range(&resp) } else { None };
        if partial && cr.is_none_or(|(a, _, _)| a != have) {
            // A range other than the one asked for: start over without ranges.
            discard(&d.dest);
            no_multi = true;
            continue;
        }
        let total = match cr {
            Some((_, _, total)) => total,
            None => resp.content_length(),
        };
        if let (true, Some(total)) = (partial && !no_multi, total) {
            if total >= t.multi_min {
                let m = Meta { key: key.to_string(), size: Some(total), segs: vec![Seg { start: 0, end: total, pos: have, id: 0, busy: true }], complete: false };
                save_meta(&side, &m);
                match multi(d, m, Some(resp), progress, throttle, ctl, t).await? {
                    MultiEnd::Done => return Ok(Outcome::Done),
                    MultiEnd::Paused => return Ok(Outcome::Paused),
                    MultiEnd::Canceled => return Ok(Outcome::Canceled),
                    MultiEnd::Restart => discard(&d.dest),
                    MultiEnd::NoRanges => {
                        discard(&d.dest);
                        no_multi = true;
                    }
                }
                continue;
            }
        }
        // ---- one stream, appended to the part file
        let resumed = partial && have > 0;
        save_meta(&side, &Meta { key: key.to_string(), size: total, segs: vec![], complete: false });
        let mut file = std::fs::OpenOptions::new().create(true).append(resumed).write(true).truncate(!resumed).open(&part)?;
        let start = if resumed { have } else { 0 };
        progress.done.store(start, Ordering::Relaxed);
        progress.conns.store(1, Ordering::Relaxed);
        progress.set_detail(if resumed { format!("Resuming from {}", crate::core::fmt_bytes(have)) } else { format!("Downloading from {}", crate::http::host(resp.url().as_str())) });
        let resumed_at = Instant::now();
        let mut written_this_try = 0u64;
        let mut stream = resp.bytes_stream();
        let mut failed: Option<anyhow::Error> = None;
        loop {
            let ctl_now = *ctl.borrow();
            match ctl_now {
                Control::Pause => {
                    file.flush()?;
                    progress.conns.store(0, Ordering::Relaxed);
                    return Ok(Outcome::Paused);
                }
                Control::Cancel => {
                    drop(file);
                    discard(&d.dest);
                    progress.conns.store(0, Ordering::Relaxed);
                    return Ok(Outcome::Canceled);
                }
                Control::Run => {}
            }
            let next = tokio::select! {
                n = stream.next() => n,
                _ = changed(ctl) => continue,
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    failed = Some(anyhow!("The download stalled"));
                    break;
                }
            };
            match next {
                None => break,
                Some(Ok(chunk)) => {
                    if written_this_try == 0 {
                        *progress.note.lock() = None;
                        offline_since = None;
                        backoff = Duration::from_secs(2);
                    }
                    file.write_all(&chunk)?;
                    written_this_try += chunk.len() as u64;
                    progress.done.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                    if resumed && resumed_at.elapsed() > Duration::from_secs(4) {
                        progress.set_detail(format!("Downloading from {}", crate::http::host(&d.url)));
                    }
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
        progress.conns.store(0, Ordering::Relaxed);
        if let Some(e) = failed {
            // What is on disk is what the next try continues from.
            progress.done.store(std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
            // The connection dropped mid-download: wait for the network, then resume.
            if net_wait_left(&mut offline_since) {
                log::warn!("download {}: {e}, resuming", d.url);
                *progress.note.lock() = Some("Waiting for the network".into());
                if let Some(o) = net_pause(ctl, &d.dest, &mut backoff).await {
                    return Ok(o);
                }
                continue;
            }
            return Err(e);
        }
        return Ok(Outcome::Done);
    }
}

/// Pause or Cancel asked for: end now (Cancel deletes the partial file).
fn stop_now(ctl: &watch::Receiver<Control>, dest: &Path) -> Option<Outcome> {
    match *ctl.borrow() {
        Control::Run => None,
        Control::Pause => Some(Outcome::Paused),
        Control::Cancel => {
            discard(dest);
            Some(Outcome::Canceled)
        }
    }
}

/// State shared by the connections of one download (they all run in the same task).
struct Shared<'a> {
    d: &'a Download,
    file: std::fs::File,
    segs: Mutex<Vec<Seg>>,
    /// Where the address led (GitHub sends to a signed address that expires).
    url: Mutex<String>,
    size: u64,
    progress: &'a Progress,
    throttle: &'a Throttle,
    t: Tuning,
    last_bytes: Mutex<Instant>,
    /// Connections waiting for the network right now.
    waiting_net: AtomicUsize,
    /// Connections the server takes now (lowered when it answers 429 or 503).
    cap: AtomicUsize,
    /// Expired signed addresses looked up again.
    resolves: AtomicU32,
}

enum WEnd {
    NoWork,
    Stopped,
    /// The server asked for fewer connections.
    Backoff,
    Restart,
    NoRanges,
    Fatal(anyhow::Error),
}

impl Shared<'_> {
    fn release(&self, id: u32) {
        if let Some(s) = self.segs.lock().iter_mut().find(|s| s.id == id) {
            s.busy = false;
        }
    }
    fn range_of(&self, id: u32) -> Option<(u64, u64)> {
        self.segs.lock().iter().find(|s| s.id == id).map(|s| (s.pos, s.end))
    }
    /// Write the buffered bytes of piece `id`; true when the piece is complete. Bytes past its
    /// end (another connection took that part over) are dropped.
    fn flush(&self, id: u32, buf: &mut Vec<u8>) -> std::io::Result<bool> {
        let mut segs = self.segs.lock();
        let Some(s) = segs.iter_mut().find(|s| s.id == id) else {
            buf.clear();
            return Ok(true);
        };
        let n = (s.left() as usize).min(buf.len());
        if n > 0 {
            write_at(&self.file, &buf[..n], s.pos)?;
            s.pos += n as u64;
        }
        if buf.len() > n {
            self.progress.done.fetch_sub((buf.len() - n) as u64, Ordering::Relaxed);
        }
        buf.clear();
        let done = s.left() == 0;
        if done {
            s.busy = false;
        }
        Ok(done)
    }
    fn meta(&self, key: &str) -> Meta {
        Meta { key: key.to_string(), size: Some(self.size), segs: self.segs.lock().clone(), complete: false }
    }
}

/// One connection: fetches pieces until none is left.
async fn worker(sh: &Shared<'_>, mut ctl: watch::Receiver<Control>, mut first: Option<(u32, reqwest::Response)>, _slot: HostSlot) -> WEnd {
    let mut backoff = Duration::from_secs(2);
    let mut server_fails = 0u32;
    let mut current: Option<u32> = None;
    loop {
        if *ctl.borrow() != Control::Run {
            if let Some(id) = current {
                sh.release(id);
            }
            return WEnd::Stopped;
        }
        let id = match (current, &first) {
            (Some(id), _) => id,
            (None, Some((id, _))) => *id,
            (None, None) => match claim(&mut sh.segs.lock(), sh.t.min_split) {
                Some(id) => id,
                None => return WEnd::NoWork,
            },
        };
        current = Some(id);
        let Some((pos, end)) = sh.range_of(id) else { return WEnd::NoWork };
        if pos >= end {
            sh.release(id);
            current = None;
            continue;
        }
        let used_url = sh.url.lock().clone();
        let sent = match first.take() {
            Some((_, r)) => Ok(r),
            None => with_headers(DL_CLIENT.get(&used_url), sh.d).header("Range", format!("bytes={pos}-{}", end - 1)).send().await,
        };
        let resp = match sent {
            Ok(r) => r,
            Err(e) => {
                sh.release(id);
                current = None;
                if !is_network_error(&e) && server_fails >= 3 {
                    return WEnd::Fatal(e.into());
                }
                server_fails += !is_network_error(&e) as u32;
                if sh.last_bytes.lock().elapsed() > NET_WAIT {
                    return WEnd::Fatal(anyhow!("Could not reach {}: {e}", source_name(&sh.d.url)));
                }
                sh.waiting_net.fetch_add(1, Ordering::Relaxed);
                let go_on = nap(&mut ctl, backoff).await;
                sh.waiting_net.fetch_sub(1, Ordering::Relaxed);
                backoff = (backoff * 2).min(Duration::from_secs(30));
                if !go_on {
                    return WEnd::Stopped;
                }
                continue;
            }
        };
        let status = resp.status();
        if status == reqwest::StatusCode::OK {
            // The server ignores ranges (on this address at least).
            sh.release(id);
            return WEnd::NoRanges;
        }
        if matches!(status.as_u16(), 401 | 403 | 410) && used_url != sh.d.url && sh.resolves.fetch_add(1, Ordering::Relaxed) < 20 {
            // The signed address expired: ask the original address again.
            *sh.url.lock() = sh.d.url.clone();
            continue;
        }
        if status == reqwest::StatusCode::TOO_MANY_REQUESTS || status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            sh.release(id);
            current = None;
            let live = sh.progress.conns.load(Ordering::Relaxed).max(1);
            sh.cap.store((live - 1).max(1), Ordering::Relaxed);
            let wait = retry_after(&resp).unwrap_or(backoff);
            drop(resp);
            server_fails += 1;
            if server_fails > 6 {
                return WEnd::Fatal(anyhow!("HTTP {status} from {}", crate::http::host(&sh.d.url)));
            }
            if live > 1 {
                return WEnd::Backoff;
            }
            if !nap(&mut ctl, wait).await {
                return WEnd::Stopped;
            }
            backoff = (backoff * 2).min(Duration::from_secs(30));
            continue;
        }
        if status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            sh.release(id);
            return WEnd::Restart;
        }
        if status.is_server_error() && server_fails < 3 {
            server_fails += 1;
            sh.release(id);
            current = None;
            drop(resp);
            if !nap(&mut ctl, Duration::from_secs(3)).await {
                return WEnd::Stopped;
            }
            continue;
        }
        if status != reqwest::StatusCode::PARTIAL_CONTENT {
            sh.release(id);
            return WEnd::Fatal(anyhow!("HTTP {status} from {}", crate::http::host(&sh.d.url)));
        }
        match content_range(&resp) {
            Some((a, _, total)) if a == pos && total.is_none_or(|t| t == sh.size) => {}
            Some((_, _, Some(t))) if t != sh.size => {
                sh.release(id);
                return WEnd::Restart;
            }
            _ => {
                sh.release(id);
                return WEnd::NoRanges;
            }
        }
        if used_url == sh.d.url && resp.url().as_str() != used_url {
            *sh.url.lock() = resp.url().to_string();
        }
        // ---- stream this piece
        let mut buf: Vec<u8> = Vec::with_capacity(FLUSH_AT + 64 * 1024);
        let mut stream = resp.bytes_stream();
        let mut err: Option<anyhow::Error> = None;
        let mut piece_done = false;
        loop {
            if *ctl.borrow() != Control::Run {
                if let Err(e) = sh.flush(id, &mut buf) {
                    return WEnd::Fatal(e.into());
                }
                sh.release(id);
                return WEnd::Stopped;
            }
            let next = tokio::select! {
                n = stream.next() => n,
                _ = changed(&mut ctl) => continue,
                _ = tokio::time::sleep(Duration::from_secs(60)) => {
                    err = Some(anyhow!("The download stalled"));
                    break;
                }
            };
            match next {
                Some(Ok(chunk)) => {
                    let n = chunk.len();
                    buf.extend_from_slice(&chunk);
                    sh.progress.done.fetch_add(n as u64, Ordering::Relaxed);
                    *sh.last_bytes.lock() = Instant::now();
                    backoff = Duration::from_secs(2);
                    server_fails = 0;
                    let left = sh.range_of(id).map(|(p, e)| e.saturating_sub(p)).unwrap_or(0);
                    let full = buf.len() >= FLUSH_AT || buf.len() as u64 >= left;
                    if full {
                        match sh.flush(id, &mut buf) {
                            Ok(done) => piece_done = done,
                            Err(e) => return WEnd::Fatal(e.into()),
                        }
                    }
                    // Every byte received counts against the limit, the last ones of a piece too.
                    let wait = sh.throttle.take(n);
                    if !wait.is_zero() {
                        tokio::time::sleep(wait).await;
                    }
                    if piece_done {
                        break;
                    }
                }
                Some(Err(e)) => {
                    err = Some(e.into());
                    break;
                }
                None => break,
            }
        }
        match sh.flush(id, &mut buf) {
            Ok(true) => piece_done = true,
            Ok(false) => {}
            Err(e) => return WEnd::Fatal(e.into()),
        }
        drop(stream);
        current = None;
        if piece_done {
            continue;
        }
        // The connection ended early or failed: the piece is free again; try again after a pause.
        sh.release(id);
        if let Some(e) = &err {
            log::warn!("download {}: {e}, resuming the piece", sh.d.url);
        }
        if sh.last_bytes.lock().elapsed() > NET_WAIT {
            return WEnd::Fatal(err.unwrap_or_else(|| anyhow!("The connection keeps closing")));
        }
        sh.waiting_net.fetch_add(1, Ordering::Relaxed);
        let go_on = nap(&mut ctl, Duration::from_millis(500)).await;
        sh.waiting_net.fetch_sub(1, Ordering::Relaxed);
        if !go_on {
            return WEnd::Stopped;
        }
    }
}

/// Fetch the pieces in `m` over several connections into the part file.
#[allow(clippy::too_many_arguments)]
async fn multi(d: &Download, m: Meta, first: Option<reqwest::Response>, progress: &Progress, throttle: &Throttle, ctl: &mut watch::Receiver<Control>, t: Tuning) -> Result<MultiEnd> {
    let size = m.size.unwrap_or(0);
    let part = part_path(&d.dest);
    let side = sidecar(&d.dest);
    let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&part)?;
    if file.metadata()?.len() != size {
        make_sparse(&file);
        file.set_len(size)?;
    }
    let resumed: u64 = m.segs.iter().map(|s| s.pos - s.start).sum();
    progress.done.store(resumed, Ordering::Relaxed);
    let mut segs = m.segs.clone();
    let first = match first {
        Some(r) => {
            // The first answer belongs to the piece that starts where the file stops.
            let id = segs.iter().find(|s| s.left() > 0).map(|s| s.id).unwrap_or(0);
            for s in segs.iter_mut() {
                s.busy = s.id == id;
            }
            Some((id, r))
        }
        None => None,
    };
    let host = crate::http::host(&d.url).to_string();
    let url = first.as_ref().map(|(_, r)| r.url().to_string()).unwrap_or_else(|| d.url.clone());
    let sh = Shared {
        d,
        file,
        segs: Mutex::new(segs),
        url: Mutex::new(url),
        size,
        progress,
        throttle,
        t,
        last_bytes: Mutex::new(Instant::now()),
        waiting_net: AtomicUsize::new(0),
        cap: AtomicUsize::new(t.max_conns),
        resolves: AtomicU32::new(0),
    };
    let key = m.key.clone();
    let mut ws = FuturesUnordered::new();
    if let Some(f) = first {
        ws.push(worker(&sh, ctl.clone(), Some(f), HostSlot::take(&host)));
    }
    let mut target = t.start_conns.clamp(1, t.max_conns.max(1));
    let t0 = Instant::now();
    let mut tick = tokio::time::interval(Duration::from_millis(500));
    let mut saved_at = Instant::now();
    // Adding connections while each one adds speed.
    let mut window = (Instant::now(), progress.done.load(Ordering::Relaxed));
    let mut speed_before: Option<f64> = None;
    let mut grow_done = false;
    let mut end: Option<WEnd> = None;
    loop {
        if *ctl.borrow() == Control::Run && end.is_none() {
            let cap = sh.cap.load(Ordering::Relaxed).max(1);
            while ws.len() < target.min(cap) && has_work(&sh.segs.lock(), t.min_split) {
                let slot = if ws.is_empty() { Some(HostSlot::take(&host)) } else { HostSlot::try_take(&host) };
                let Some(slot) = slot else { break };
                ws.push(worker(&sh, ctl.clone(), None, slot));
            }
        }
        progress.conns.store(ws.len(), Ordering::Relaxed);
        if ws.is_empty() {
            break;
        }
        tokio::select! {
            Some(w) = ws.next() => match w {
                WEnd::NoWork | WEnd::Stopped => {}
                WEnd::Backoff => {
                    target = sh.cap.load(Ordering::Relaxed).max(1);
                    grow_done = true;
                }
                other => {
                    end = Some(other);
                    break;
                }
            },
            _ = tick.tick() => {
                let n = ws.len();
                let waiting = sh.waiting_net.load(Ordering::Relaxed);
                *progress.note.lock() = (waiting > 0 && waiting >= n).then(|| format!("Waiting for the network, can't reach {}", source_name(&d.url)));
                if resumed > 0 && t0.elapsed() < Duration::from_secs(4) {
                    progress.set_detail(format!("Resuming from {}", crate::core::fmt_bytes(resumed)));
                } else if n > 1 {
                    progress.set_detail(format!("Downloading over {n} connections from {host}"));
                } else {
                    progress.set_detail(format!("Downloading from {host}"));
                }
                if saved_at.elapsed() >= Duration::from_secs(2) {
                    // Pieces are written before they are counted, and on disk before they are noted.
                    let m = sh.meta(&key);
                    let _ = sh.file.sync_data();
                    save_meta(&side, &m);
                    saved_at = Instant::now();
                }
                let el = window.0.elapsed().as_secs_f64();
                if el >= 3.0 {
                    let now = progress.done.load(Ordering::Relaxed);
                    let speed = now.saturating_sub(window.1) as f64 / el;
                    window = (Instant::now(), now);
                    let limit = throttle.limit() as f64;
                    let limited = limit > 0.0 && speed >= 0.9 * limit;
                    if !grow_done && !limited && n >= target && target < t.max_conns.min(sh.cap.load(Ordering::Relaxed)) {
                        match speed_before {
                            Some(prev) if speed < prev * 1.1 => grow_done = true,
                            _ => {
                                speed_before = Some(speed);
                                target += 1;
                            }
                        }
                    }
                }
            }
        }
    }
    drop(ws);
    progress.conns.store(0, Ordering::Relaxed);
    // Keep what was written, whatever happens next.
    let _ = sh.file.sync_data();
    let meta = sh.meta(&key);
    save_meta(&side, &meta);
    let complete = left_total(&meta.segs) == 0;
    match end {
        Some(WEnd::Fatal(e)) => return Err(e),
        Some(WEnd::Restart) => return Ok(MultiEnd::Restart),
        Some(WEnd::NoRanges) => return Ok(MultiEnd::NoRanges),
        _ => {}
    }
    match *ctl.borrow() {
        Control::Cancel => {
            discard(&d.dest);
            return Ok(MultiEnd::Canceled);
        }
        Control::Pause if !complete => return Ok(MultiEnd::Paused),
        _ => {}
    }
    if complete {
        Ok(MultiEnd::Done)
    } else {
        Err(anyhow!("The download stopped before it was complete"))
    }
}

/// How long a download waits for the network to come back before it fails.
const NET_WAIT: Duration = Duration::from_secs(10 * 60);

/// The request could not get through (no network, DNS, refused, reset, timed out).
fn is_network_error(e: &reqwest::Error) -> bool {
    e.is_connect() || e.is_timeout() || e.is_request()
}

/// Starts the wait on the first failure; false once the network has been gone too long.
fn net_wait_left(since: &mut Option<Instant>) -> bool {
    since.get_or_insert_with(Instant::now).elapsed() < NET_WAIT
}

/// Sleep before the next try (growing up to 30 s). Pause and Cancel still work meanwhile and
/// end the download at once (a paused download keeps its partial file).
async fn net_pause(ctl: &mut watch::Receiver<Control>, dest: &Path, backoff: &mut Duration) -> Option<Outcome> {
    let wait = *backoff;
    *backoff = (*backoff * 2).min(Duration::from_secs(30));
    nap(ctl, wait).await;
    stop_now(ctl, dest)
}

#[cfg(test)]
mod tests;
