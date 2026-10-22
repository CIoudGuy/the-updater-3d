//! The downloader against real servers: one connection against several, the files checked
//! against their published SHA-256. Not run by default (about 900 MB of downloads):
//! cargo test --release --test live_downloads -- --ignored --nocapture --test-threads 1

use sha2::{Digest, Sha256};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use updater_lib::download::{self, Control, Outcome, Progress, Throttle, Tuning};
use updater_lib::sources::Download;

fn dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join("updater3d-live-dl").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sha_of(p: &std::path::Path) -> String {
    let mut f = std::fs::File::open(p).unwrap();
    let mut h = Sha256::new();
    std::io::copy(&mut f, &mut h).unwrap();
    hex::encode(h.finalize())
}

fn single() -> Tuning {
    Tuning { max_conns: 1, ..Tuning::default() }
}

/// Download `d`; returns MB/s, the most connections seen and the outcome.
async fn timed(d: &Download, t: Tuning, stop_after: Option<Duration>) -> (f64, usize, Outcome, u64) {
    let (tx, rx) = tokio::sync::watch::channel(Control::Run);
    let p = Arc::new(Progress::new());
    let t0 = Instant::now();
    let watch = {
        let p = p.clone();
        tokio::spawn(async move {
            let mut most = 0;
            loop {
                tokio::time::sleep(Duration::from_millis(250)).await;
                most = most.max(p.conns.load(Ordering::Relaxed));
                if let Some(s) = stop_after {
                    if t0.elapsed() >= s {
                        tx.send_replace(Control::Pause);
                    }
                }
                // The download returned (it held the other reference).
                if Arc::strong_count(&p) == 1 {
                    return most;
                }
            }
        })
    };
    let out = download::fetch_tuned(d, p.clone(), Arc::new(Throttle::new()), rx, t).await.expect("download");
    let secs = t0.elapsed().as_secs_f64();
    let got = p.done.load(Ordering::Relaxed);
    drop(p);
    let most = watch.await.unwrap();
    (got as f64 / secs / 1e6, most, out, got)
}

async fn compare(name: &str, url: &str, size: Option<u64>, sha: &str, full_single: bool) {
    let d0 = dir(name);
    let multi = Download { url: url.into(), dest: d0.join("multi.bin"), size, sha256: Some(sha.into()), md5: None, signer: None, sha512: None, headers: vec![] };
    // Several connections, with a pause in the middle and a resume ("the app restarts").
    let (_, _, out, kept) = timed(&multi, Tuning::default(), Some(Duration::from_secs(4))).await;
    assert!(matches!(out, Outcome::Paused), "{name}: {out:?}");
    println!("{name}: paused after 4 s with {:.1} MB kept", kept as f64 / 1e6);
    let (mbps_multi, conns, out, _) = timed(&multi, Tuning::default(), None).await;
    assert!(matches!(out, Outcome::Done), "{name}: {out:?}");
    assert_eq!(sha_of(&multi.dest), sha, "{name}: multi-connection file differs from the published hash");
    download::remove(&multi.dest);

    let one = Download { dest: d0.join("single.bin"), ..multi.clone() };
    if full_single {
        let (mbps_single, _, out, _) = timed(&one, single(), None).await;
        assert!(matches!(out, Outcome::Done));
        assert_eq!(sha_of(&one.dest), sha, "{name}: single-stream file differs");
        println!("{name}: one connection {mbps_single:.1} MB/s, several {mbps_multi:.1} MB/s (up to {conns} connections), both match the published SHA-256");
    } else {
        // A big file: one connection for 20 s is enough to know its speed.
        let (mbps_single, _, out, _) = timed(&one, single(), Some(Duration::from_secs(20))).await;
        assert!(matches!(out, Outcome::Paused));
        println!("{name}: one connection {mbps_single:.1} MB/s (20 s sample), several {mbps_multi:.1} MB/s (up to {conns} connections), the file matches the published SHA-256");
    }
    download::remove(&one.dest);
    let _ = std::fs::remove_dir_all(&d0);
}

#[tokio::test]
#[ignore]
async fn github_release_asset() {
    let url = "https://github.com/balena-io/etcher/releases/download/v2.1.8/balenaEtcher-2.1.8.Setup.exe";
    // From the GitHub release API ("digest" of the asset).
    compare("github", url, Some(200108184), "6b812ccb4264a5f955d75260f02a081f027a2d66901984c57e96c20a5d37589d", true).await;
}

#[tokio::test]
#[ignore]
async fn blender_mirror() {
    let list = reqwest::get("https://download.blender.org/release/Blender4.5/blender-4.5.9.sha256").await.unwrap().text().await.unwrap();
    let sha = list.lines().find(|l| l.ends_with("blender-4.5.9-windows-x64.zip")).and_then(|l| l.split_whitespace().next()).expect("published hash").to_string();
    compare("blender", "https://download.blender.org/release/Blender4.5/blender-4.5.9-windows-x64.zip", None, &sha, false).await;
}

#[tokio::test]
#[ignore]
async fn winget_installer() {
    use updater_lib::sources::winget;
    let path = winget::refresh_index(Duration::from_secs(24 * 3600)).await.expect("winget index");
    let idx = winget::Index::open(&path).unwrap();
    let pkg = idx.by_id("VideoLAN.VLC").expect("VLC in the catalog");
    let versions = winget::versions(&pkg).await.unwrap();
    let m = winget::manifest(versions.first().unwrap()).await.unwrap();
    let i = m.installers.iter().find(|i| i.arch == "x64" && i.url.ends_with(".exe")).or_else(|| m.installers.first()).expect("an installer");
    println!("winget: {} {} {}", m.id, m.version, i.url);
    compare("winget", &i.url, None, &i.sha256.to_ascii_lowercase(), true).await;
}
