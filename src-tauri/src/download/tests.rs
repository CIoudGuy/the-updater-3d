use super::*;
use std::sync::atomic::AtomicBool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn dl(dest: std::path::PathBuf, size: Option<u64>, md5: Option<String>) -> Download {
    Download { url: String::new(), dest, size, sha256: None, md5, signer: None, sha512: None, headers: vec![] }
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join("u3d-dl-tests").join(format!("{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn md5_and_size_are_checked() {
    let dir = scratch("md5");
    let f = dir.join("a.bin");
    std::fs::write(&f, b"hello world").unwrap();
    // md5("hello world") = 5eb63bbbe01eeed093cb22bb8f5acdc3
    let good = "5eb63bbbe01eeed093cb22bb8f5acdc3".to_string();
    assert!(verify(&dl(f.clone(), Some(11), Some(good.clone()))).is_ok());
    assert!(verify(&dl(f.clone(), Some(11), Some(good.to_ascii_uppercase()))).is_ok());
    let e = verify(&dl(f.clone(), Some(11), Some("00".repeat(16)))).unwrap_err().to_string();
    assert!(e.contains("Checksum mismatch"), "{e}");
    assert!(verify(&dl(f.clone(), Some(12), Some(good))).unwrap_err().to_string().contains("size"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unity_integrity_value_becomes_hex() {
    // The value from Unity's release API for 6000.3.27f1.
    assert_eq!(md5_hex_from_integrity("md5-e5QStPCXCBTa8atkaZRINg==").as_deref(), Some("7b9412b4f0970814daf1ab6469944836"));
    assert_eq!(md5_hex_from_integrity("sha256-abc"), None);
    assert_eq!(md5_hex_from_integrity("md5-AAAA"), None);
}

#[test]
fn file_hashes_are_lowercase_hex_of_known_values() {
    let dir = std::env::temp_dir().join(format!("u3d-hash-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let f = dir.join("abc.bin");
    std::fs::write(&f, b"abc").unwrap();
    assert_eq!(hash_file(&f, Sha256::new()).unwrap(), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    assert_eq!(hash_file(&f, Md5::new()).unwrap(), "900150983cd24fb0d6963f7d28e17f72");
    assert_eq!(
        hash_file(&f, sha2::Sha512::new()).unwrap(),
        "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn content_ranges_are_read() {
    assert_eq!(parse_content_range("bytes 0-99/1000"), Some((0, 99, Some(1000))));
    assert_eq!(parse_content_range("bytes 500-999/*"), Some((500, 999, None)));
    assert_eq!(parse_content_range(" bytes  7-8 / 9 "), Some((7, 8, Some(9))));
    assert_eq!(parse_content_range("items 0-1/2"), None);
    assert_eq!(parse_content_range("bytes x-1/2"), None);
}

fn seg(id: u32, start: u64, end: u64, pos: u64, busy: bool) -> Seg {
    Seg { start, end, pos, id, busy }
}

#[test]
fn free_connections_take_half_of_the_biggest_piece() {
    // One connection fetches the whole 100 MB file.
    let mb = 1024 * 1024;
    let mut segs = vec![seg(0, 0, 100 * mb, 0, true)];
    let a = claim(&mut segs, 2 * mb).unwrap();
    let b = claim(&mut segs, 2 * mb).unwrap();
    let c = claim(&mut segs, 2 * mb).unwrap();
    assert_eq!(segs.len(), 4);
    assert_eq!([a, b, c], [1, 2, 3]);
    // Four equal quarters, in file order, covering it all without gaps.
    let ranges: Vec<(u64, u64)> = segs.iter().map(|s| (s.start, s.end)).collect();
    assert_eq!(ranges, vec![(0, 25 * mb), (25 * mb, 50 * mb), (50 * mb, 75 * mb), (75 * mb, 100 * mb)]);
    assert!(segs.iter().all(|s| s.busy));
    // A piece that is half done is split in what is left of it.
    let mut segs = vec![seg(0, 0, 10 * mb, 6 * mb, true)];
    claim(&mut segs, mb).unwrap();
    assert_eq!((segs[0].end, segs[1].start, segs[1].pos, segs[1].end), (8 * mb, 8 * mb, 8 * mb, 10 * mb));
    // Too little left to share: no new piece.
    let mut segs = vec![seg(0, 0, 10 * mb, 7 * mb, true)];
    assert!(!has_work(&segs, 2 * mb));
    assert_eq!(claim(&mut segs, 2 * mb), None);
    assert_eq!(segs.len(), 1);
}

#[test]
fn free_pieces_from_before_are_taken_first() {
    // A restart: pieces from the last run, none of them busy.
    let mut segs = vec![seg(0, 0, 50, 50, false), seg(1, 50, 100, 70, false), seg(2, 100, 150, 100, false)];
    assert!(has_work(&segs, 10));
    assert_eq!(claim(&mut segs, 10), Some(1));
    assert_eq!(claim(&mut segs, 10), Some(2));
    // Both pieces left are busy and 30 and 50 long: only the 50 one can be split in two of 25.
    assert_eq!(claim(&mut segs, 10), Some(3));
    assert_eq!((segs[2].start, segs[2].end, segs[3].start, segs[3].end), (100, 125, 125, 150));
}

#[test]
fn files_are_known_by_checksum_or_by_address_and_size() {
    let mut a = dl("x".into(), Some(10), None);
    a.url = "https://example.com/v1/setup.exe".into();
    let mut b = a.clone();
    b.url = "https://example.com/v2/setup.exe".into();
    assert_ne!(file_key(&a), file_key(&b), "a new address is another file");
    a.sha256 = Some("AB".repeat(32));
    b.sha256 = Some("ab".repeat(32));
    assert_eq!(file_key(&a), file_key(&b), "the same checksum is the same file, wherever it comes from");
    let mut c = dl("x".into(), Some(11), None);
    c.url = a.url.clone();
    a.sha256 = None;
    assert_ne!(file_key(&a), file_key(&c), "another size is another file");
}

#[test]
fn the_check_says_what_it_checks() {
    let mut d = dl("x".into(), None, None);
    assert_eq!(check_text(&d), "Checking the file size");
    d.sha256 = Some("a".into());
    assert_eq!(check_text(&d), "Checking the SHA-256 checksum");
    d.sha256 = None;
    d.signer = Some("Epic Games Inc.".into());
    assert_eq!(check_text(&d), "Checking the signature by Epic Games Inc.");
}

#[test]
fn leftovers_of_old_downloads_are_swept() {
    let dir = scratch("sweep");
    let sub = dir.join("tools");
    std::fs::create_dir_all(&sub).unwrap();
    let keep = sub.join("keep.exe");
    let gone = sub.join("gone.exe");
    let done = dir.join("done.msi");
    for p in [part_path(&keep), sidecar(&keep), part_path(&gone), sidecar(&gone), with_suffix(&sidecar(&gone), ".tmp")] {
        std::fs::write(&p, b"x").unwrap();
    }
    std::fs::write(&done, b"installer").unwrap();
    save_meta(&sidecar(&done), &Meta { key: "k".into(), size: Some(9), segs: vec![], complete: true });
    std::fs::write(sub.join("other.zip"), b"zip").unwrap();
    let keep_set: std::collections::HashSet<PathBuf> = [keep.clone()].into_iter().collect();
    sweep(&dir, &keep_set);
    assert!(part_path(&keep).exists() && sidecar(&keep).exists(), "a job still needs these");
    assert!(!part_path(&gone).exists() && !sidecar(&gone).exists() && !with_suffix(&sidecar(&gone), ".tmp").exists());
    assert!(done.exists() && sidecar(&done).exists(), "a finished file keeps its note");
    assert!(sub.join("other.zip").exists(), "finished files are not touched");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn bytes_on_disk_come_from_the_pieces() {
    let dir = scratch("existing");
    let mut d = dl(dir.join("f.bin"), Some(1000), None);
    d.url = "http://h/f.bin".into();
    // A preallocated part file is as long as the whole file; only what the pieces hold counts.
    std::fs::write(part_path(&d.dest), vec![0u8; 1000]).unwrap();
    save_meta(&sidecar(&d.dest), &Meta { key: file_key(&d), size: Some(1000), segs: vec![seg(0, 0, 500, 120, false), seg(1, 500, 1000, 530, false)], complete: false });
    assert_eq!(existing_bytes(&d), 150);
    // Pieces of another file count nothing.
    d.url = "http://h/v2/f.bin".into();
    assert_eq!(existing_bytes(&d), 0);
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------- a local test server

#[derive(Default)]
struct Cfg {
    /// Answer Range requests with 206 (else always 200 with the whole file).
    ranges: bool,
    /// Bytes per second per connection (0: as fast as it goes).
    rate: u64,
    /// The first this many connections close after `drop_after` bytes.
    drops: AtomicU32,
    drop_after: u64,
    /// Answer 503 while more than this many connections are open (0: never).
    busy_over: usize,
    /// "/file" sends to "/signed/<n>", which expires after `expire_after` requests.
    signed: bool,
    expire_after: u32,
}

#[derive(Default)]
struct Stats {
    requests: AtomicU32,
    ranged: AtomicU32,
    open: AtomicUsize,
    max_open: AtomicUsize,
    body_bytes: AtomicU64,
    busy: AtomicU32,
    expired: AtomicU32,
    redirects: AtomicU32,
    generation: AtomicU32,
    /// Requests per signed address
    hits: Mutex<HashMap<u32, u32>>,
    stop: AtomicBool,
}

async fn serve(body: Vec<u8>, cfg: Cfg) -> (u16, Arc<Stats>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = Arc::new(body);
    let cfg = Arc::new(cfg);
    let stats = Arc::new(Stats::default());
    let st = stats.clone();
    tokio::spawn(async move {
        loop {
            let Ok((s, _)) = listener.accept().await else { return };
            if st.stop.load(Ordering::Relaxed) {
                return;
            }
            let (body, cfg, st) = (body.clone(), cfg.clone(), st.clone());
            tokio::spawn(async move {
                let n = st.open.fetch_add(1, Ordering::SeqCst) + 1;
                st.max_open.fetch_max(n, Ordering::SeqCst);
                let _ = answer(s, &body, &cfg, &st, n).await;
                st.open.fetch_sub(1, Ordering::SeqCst);
            });
        }
    });
    (port, stats)
}

async fn answer(mut s: tokio::net::TcpStream, body: &[u8], cfg: &Cfg, st: &Stats, open_now: usize) -> std::io::Result<()> {
    let mut req = Vec::new();
    let mut b = [0u8; 2048];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = s.read(&mut b).await?;
        if n == 0 {
            return Ok(());
        }
        req.extend_from_slice(&b[..n]);
    }
    let text = String::from_utf8_lossy(&req).to_string();
    let path = text.split_whitespace().nth(1).unwrap_or("/").to_string();
    st.requests.fetch_add(1, Ordering::SeqCst);
    let range = text.lines().find_map(|l| l.strip_prefix("Range: ").or_else(|| l.strip_prefix("range: "))).map(|v| v.trim().to_string());
    if cfg.signed {
        if path == "/file" {
            st.redirects.fetch_add(1, Ordering::SeqCst);
            let g = st.generation.fetch_add(1, Ordering::SeqCst) + 1;
            s.write_all(format!("HTTP/1.1 302 Found\r\nLocation: /signed/{g}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await?;
            return Ok(());
        }
        let g: u32 = path.trim_start_matches("/signed/").parse().unwrap_or(0);
        let hits = {
            let mut m = st.hits.lock();
            let h = m.entry(g).or_default();
            *h += 1;
            *h
        };
        if hits > cfg.expire_after {
            st.expired.fetch_add(1, Ordering::SeqCst);
            s.write_all(b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
            return Ok(());
        }
    }
    if cfg.busy_over > 0 && open_now > cfg.busy_over && range.is_some() {
        st.busy.fetch_add(1, Ordering::SeqCst);
        s.write_all(b"HTTP/1.1 503 Service Unavailable\r\nRetry-After: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(());
    }
    let total = body.len() as u64;
    let (a, z, head) = match range.as_deref().and_then(|r| r.strip_prefix("bytes=")).filter(|_| cfg.ranges) {
        Some(r) => {
            st.ranged.fetch_add(1, Ordering::SeqCst);
            let (a, z) = r.split_once('-').unwrap();
            let a: u64 = a.parse().unwrap();
            let z: u64 = if z.is_empty() { total - 1 } else { z.parse::<u64>().unwrap().min(total - 1) };
            if a >= total {
                s.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await?;
                return Ok(());
            }
            (a, z, format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {a}-{z}/{total}\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n", z - a + 1))
        }
        None => (0, total - 1, format!("HTTP/1.1 200 OK\r\nContent-Length: {total}\r\nConnection: close\r\n\r\n")),
    };
    s.write_all(head.as_bytes()).await?;
    let drop_at = if cfg.drops.load(Ordering::SeqCst) > 0 {
        cfg.drops.fetch_sub(1, Ordering::SeqCst);
        Some(cfg.drop_after)
    } else {
        None
    };
    let mut at = a;
    let mut sent = 0u64;
    let chunk = 16 * 1024u64;
    let t0 = Instant::now();
    while at <= z {
        let n = chunk.min(z + 1 - at);
        if drop_at.is_some_and(|d| sent + n > d) {
            return Ok(()); // the connection drops
        }
        s.write_all(&body[at as usize..(at + n) as usize]).await?;
        st.body_bytes.fetch_add(n, Ordering::SeqCst);
        at += n;
        sent += n;
        if cfg.rate > 0 {
            let due = Duration::from_secs_f64(sent as f64 / cfg.rate as f64);
            if let Some(w) = due.checked_sub(t0.elapsed()) {
                tokio::time::sleep(w).await;
            }
        }
    }
    s.flush().await?;
    Ok(())
}

fn data(n: usize) -> Vec<u8> {
    // Not repeating, so a piece written at the wrong place shows.
    let mut v = Vec::with_capacity(n);
    let mut x: u32 = 0x1234_5678;
    for _ in 0..n {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        v.push((x >> 11) as u8);
    }
    v
}

fn sha(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

const KB: u64 = 1024;
const MB: u64 = 1024 * 1024;

fn small_tuning() -> Tuning {
    Tuning { multi_min: 512 * KB, start_conns: 4, max_conns: 6, min_split: 128 * KB }
}

fn job(port: u16, path: &str, dir: &Path, body: &[u8], checksum: bool) -> Download {
    Download {
        url: format!("http://127.0.0.1:{port}{path}"),
        dest: dir.join("file.bin"),
        size: Some(body.len() as u64),
        sha256: checksum.then(|| sha(body)),
        md5: None,
        signer: None,
        sha512: None,
        headers: vec![],
    }
}

async fn run(d: &Download, t: Tuning) -> (Result<Outcome>, Arc<Progress>) {
    let (_tx, rx) = watch::channel(Control::Run);
    let p = Arc::new(Progress::new());
    let r = fetch_tuned(d, p.clone(), Arc::new(Throttle::new()), rx, t).await;
    (r, p)
}

#[tokio::test]
async fn big_files_come_over_several_connections_and_are_whole() {
    let body = data(4 * MB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: true, rate: 2 * MB, ..Default::default() }).await;
    let dir = scratch("multi");
    let d = job(port, "/f.bin", &dir, &body, true);
    let t0 = Instant::now();
    let (r, p) = run(&d, small_tuning()).await;
    let secs = t0.elapsed().as_secs_f64();
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    assert_eq!(p.done.load(Ordering::Relaxed), body.len() as u64);
    let most = st.max_open.load(Ordering::SeqCst);
    println!("4 MB at 2 MB/s per connection: {secs:.2} s, up to {most} connections, {} requests", st.requests.load(Ordering::SeqCst));
    assert!(most >= 4, "only {most} connections");
    // One connection needs 2 s; four or more need well under half of that each.
    assert!(secs < 1.4, "{secs:.2} s");
    // Little is fetched twice.
    assert!(st.body_bytes.load(Ordering::SeqCst) < body.len() as u64 + 1024 * 1024);
    assert!(load_meta(&sidecar(&d.dest)).is_some_and(|m| m.complete), "the finished file is noted");
    assert!(!part_path(&d.dest).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_server_without_ranges_gets_one_stream() {
    let body = data(2 * MB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: false, ..Default::default() }).await;
    let dir = scratch("noranges");
    let d = job(port, "/f.bin", &dir, &body, true);
    let (r, _) = run(&d, small_tuning()).await;
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    assert_eq!(st.requests.load(Ordering::SeqCst), 1, "the first answer is the download");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn small_files_and_unknown_sizes_still_work() {
    let body = data(100 * KB as usize);
    let (port, _) = serve(body.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = scratch("small");
    let mut d = job(port, "/f.bin", &dir, &body, true);
    let (r, p) = run(&d, small_tuning()).await;
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    assert_eq!(p.conns.load(Ordering::Relaxed), 0);
    // Size unknown up front: the range answer tells it (here too small to split).
    std::fs::remove_file(&d.dest).unwrap();
    d.size = None;
    let (r, _) = run(&d, small_tuning()).await;
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn pause_keeps_each_piece_and_resume_continues_them() {
    let body = data(3 * MB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: true, rate: 1 * MB, ..Default::default() }).await;
    let dir = scratch("pause");
    let d = job(port, "/f.bin", &dir, &body, true);
    let (tx, rx) = watch::channel(Control::Run);
    let p = Arc::new(Progress::new());
    let h = tokio::spawn({
        let (d, p) = (d.clone(), p.clone());
        async move { fetch_tuned(&d, p, Arc::new(Throttle::new()), rx, small_tuning()).await }
    });
    tokio::time::sleep(Duration::from_millis(700)).await;
    tx.send_replace(Control::Pause);
    let out = tokio::time::timeout(Duration::from_secs(5), h).await.expect("pause is quick").unwrap().unwrap();
    assert!(matches!(out, Outcome::Paused));
    let m = load_meta(&sidecar(&d.dest)).expect("the pieces are noted");
    assert!(m.segs.len() >= 4, "{:?}", m.segs);
    let kept = existing_bytes(&d);
    assert!(kept > 300 * KB && kept < body.len() as u64, "{kept}");
    assert!(m.segs.iter().filter(|s| s.pos > s.start).count() >= 3, "several pieces got somewhere: {:?}", m.segs);
    let served_before = st.body_bytes.load(Ordering::SeqCst);

    // "The app restarts": a new download of the same file continues the pieces.
    let (r, p2) = run(&d, small_tuning()).await;
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    assert_eq!(p2.done.load(Ordering::Relaxed), body.len() as u64, "kept bytes count once");
    let served_after = st.body_bytes.load(Ordering::SeqCst) - served_before;
    assert!(served_after <= body.len() as u64 - kept + 256 * KB, "fetched {served_after} again, {kept} were kept");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dropped_connections_continue_their_piece() {
    let body = data(3 * MB as usize);
    let cfg = Cfg { ranges: true, drops: AtomicU32::new(3), drop_after: 200 * KB, ..Default::default() };
    let (port, st) = serve(body.clone(), cfg).await;
    let dir = scratch("drops");
    let d = job(port, "/f.bin", &dir, &body, true);
    let (r, _) = tokio::time::timeout(Duration::from_secs(20), run(&d, small_tuning())).await.unwrap();
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    assert!(st.requests.load(Ordering::SeqCst) >= 5);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_cancel_deletes_the_pieces() {
    let body = data(2 * MB as usize);
    let (port, _) = serve(body.clone(), Cfg { ranges: true, rate: MB, ..Default::default() }).await;
    let dir = scratch("cancel");
    let d = job(port, "/f.bin", &dir, &body, true);
    let (tx, rx) = watch::channel(Control::Run);
    let h = tokio::spawn({
        let d = d.clone();
        async move { fetch_tuned(&d, Arc::new(Progress::new()), Arc::new(Throttle::new()), rx, small_tuning()).await }
    });
    tokio::time::sleep(Duration::from_millis(400)).await;
    tx.send_replace(Control::Cancel);
    let out = tokio::time::timeout(Duration::from_secs(5), h).await.unwrap().unwrap().unwrap();
    assert!(matches!(out, Outcome::Canceled));
    assert!(!part_path(&d.dest).exists() && !sidecar(&d.dest).exists() && !d.dest.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_expired_signed_address_is_asked_for_again() {
    let body = data(3 * MB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: true, signed: true, expire_after: 2, rate: 3 * MB, ..Default::default() }).await;
    let dir = scratch("signed");
    let d = job(port, "/file", &dir, &body, true);
    let (r, _) = tokio::time::timeout(Duration::from_secs(20), run(&d, small_tuning())).await.unwrap();
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    println!("redirects {}, expired answers {}", st.redirects.load(Ordering::SeqCst), st.expired.load(Ordering::SeqCst));
    assert!(st.expired.load(Ordering::SeqCst) >= 1 && st.redirects.load(Ordering::SeqCst) >= 2);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_busy_server_gets_fewer_connections() {
    let body = data(3 * MB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: true, busy_over: 2, rate: 2 * MB, ..Default::default() }).await;
    let dir = scratch("busy");
    let d = job(port, "/f.bin", &dir, &body, true);
    let (r, _) = tokio::time::timeout(Duration::from_secs(30), run(&d, small_tuning())).await.unwrap();
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    let busy = st.busy.load(Ordering::SeqCst);
    println!("busy answers: {busy}");
    // It stops opening more connections soon instead of retrying them over and over.
    assert!(busy >= 1 && busy <= 6, "{busy}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_bandwidth_limit_counts_all_connections() {
    let body = data(3 * MB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = scratch("limit");
    let d = job(port, "/f.bin", &dir, &body, true);
    let th = Arc::new(Throttle::new());
    th.set(2 * MB);
    let (_tx, rx) = watch::channel(Control::Run);
    let t0 = Instant::now();
    let r = fetch_tuned(&d, Arc::new(Progress::new()), th, rx, small_tuning()).await;
    let secs = t0.elapsed().as_secs_f64();
    assert!(matches!(r.unwrap(), Outcome::Done));
    println!("3 MB with a 2 MB/s limit: {secs:.2} s, up to {} connections", st.max_open.load(Ordering::SeqCst));
    assert!(secs > 1.1, "{secs:.2} s is faster than the limit");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_finished_download_is_checked_and_reused_after_a_restart() {
    let body = data(1 * MB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = scratch("reuse");
    for checksum in [true, false] {
        let d = job(port, "/f.bin", &dir, &body, checksum);
        remove(&d.dest);
        let (r, _) = run(&d, small_tuning()).await;
        assert!(matches!(r.unwrap(), Outcome::Done));
        let before = st.requests.load(Ordering::SeqCst);
        let (r, p) = run(&d, small_tuning()).await;
        assert!(matches!(r.unwrap(), Outcome::Done));
        assert_eq!(st.requests.load(Ordering::SeqCst), before, "nothing downloaded again (checksum {checksum})");
        assert_eq!(p.done.load(Ordering::Relaxed), body.len() as u64);
        // A damaged file is fetched again.
        if checksum {
            std::fs::write(&d.dest, vec![0u8; body.len()]).unwrap();
            let (r, _) = run(&d, small_tuning()).await;
            assert!(matches!(r.unwrap(), Outcome::Done));
            assert_eq!(std::fs::read(&d.dest).unwrap(), body);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// A new version at a new address but the same file name: the old file (no checksum to tell
/// them apart) must not be installed instead.
#[tokio::test]
async fn an_old_file_of_the_same_name_is_not_reused() {
    let new_body = data(300 * KB as usize);
    let (port, st) = serve(new_body.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = scratch("stale");
    let mut d = job(port, "/v2/setup.exe", &dir, &new_body, false);
    d.size = None;
    std::fs::write(&d.dest, b"the old version").unwrap();
    let (r, _) = run(&d, small_tuning()).await;
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(st.requests.load(Ordering::SeqCst), 1, "the new version was downloaded");
    assert_eq!(std::fs::read(&d.dest).unwrap(), new_body);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A partial file left by an earlier version of the download is not continued with this one.
#[tokio::test]
async fn a_partial_file_of_another_version_is_not_continued() {
    let body = data(200 * KB as usize);
    let (port, _) = serve(body.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = scratch("stalepart");
    let d = job(port, "/v2/setup.exe", &dir, &body, true);
    std::fs::write(part_path(&d.dest), vec![9u8; 50_000]).unwrap();
    let mut old = d.clone();
    old.url = format!("http://127.0.0.1:{port}/v1/setup.exe");
    old.sha256 = Some("11".repeat(32));
    save_meta(&sidecar(&d.dest), &Meta { key: file_key(&old), size: None, segs: vec![], complete: false });
    let (r, _) = run(&d, small_tuning()).await;
    assert!(matches!(r.unwrap(), Outcome::Done), "the stale part is dropped, not continued");
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A part file from an older version of the app (no note) still continues as one stream.
#[tokio::test]
async fn a_part_file_without_a_note_continues() {
    let body = data(300 * KB as usize);
    let (port, st) = serve(body.clone(), Cfg { ranges: true, ..Default::default() }).await;
    let dir = scratch("oldpart");
    let d = job(port, "/f.bin", &dir, &body, true);
    std::fs::write(part_path(&d.dest), &body[..100_000]).unwrap();
    let (r, _) = run(&d, small_tuning()).await;
    assert!(matches!(r.unwrap(), Outcome::Done));
    assert_eq!(std::fs::read(&d.dest).unwrap(), body);
    assert_eq!(st.body_bytes.load(Ordering::SeqCst), body.len() as u64 - 100_000);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A damaged note must not crash the app (a piece ending before it starts made `clamp` panic at
/// start-up): it is ignored and the download starts over.
#[test]
fn a_damaged_download_note_is_ignored() {
    let d = scratch("badnote");
    let note = d.join("x.exe.dl.json");
    std::fs::write(&note, r#"{"key":"sha256:ab","size":100,"segs":[{"start":50,"end":10,"pos":20}]}"#).unwrap();
    assert!(load_meta(&note).is_none());
    std::fs::write(&note, r#"{"key":"sha256:ab","size":100,"segs":[{"start":0,"end":50,"pos":80}]}"#).unwrap();
    assert_eq!(load_meta(&note).unwrap().segs[0].pos, 50);
    let _ = std::fs::remove_dir_all(&d);
}
