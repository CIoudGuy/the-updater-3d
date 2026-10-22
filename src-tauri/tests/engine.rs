//! Engine tests: downloads (resume, pause, limit, checksum), zip unpacking and the
//! admin helper protocol. Network tests are ignored by default:
//! cargo test --test engine -- --include-ignored --nocapture

use std::sync::atomic::AtomicU64;
use std::sync::Arc;
use std::time::{Duration, Instant};
use updater_lib::download::{self, Control, Outcome, Progress, Throttle};
use updater_lib::elevate::{execute, Elevator};
use updater_lib::sources::{Download, Step};

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join("updater3d-tests").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

// GridCraft 0.10.0 MSI: 14 MB, SHA-256 published in the ArtCraft feed.
const URL: &str = "https://github.com/storytold/gridcraft/releases/download/v0.10.0/gridcraft-0.10.0-windows-x64.msi";
const SIZE: u64 = 14209024;

async fn sha_from_feed() -> String {
    let v: serde_json::Value = reqwest::get("https://api.storyteller.ai/v1/craft_apps/release_info").await.unwrap().json().await.unwrap();
    let assets = v["release_info"]["apps"]["gridcraft"]["latest_release"]["assets"].as_array().unwrap().clone();
    let a = assets.iter().find(|a| a["name"] == "gridcraft-0.10.0-windows-x64.msi").expect("asset in feed");
    a["sha256"].as_str().unwrap().to_string()
}

#[tokio::test]
#[ignore]
async fn download_pauses_resumes_limits_and_verifies() {
    let dir = tmp("dl");
    let d = Download { url: URL.into(), dest: dir.join("g.msi"), size: Some(SIZE), sha256: Some(sha_from_feed().await), headers: vec![] };
    let throttle = Arc::new(Throttle::new());
    throttle.set(3 * 1024 * 1024);
    let (tx, rx) = tokio::sync::watch::channel(Control::Run);
    let p = Arc::new(Progress::new());
    let t0 = Instant::now();
    let h = tokio::spawn({
        let (d, p, th) = (d.clone(), p.clone(), throttle.clone());
        async move { download::fetch(&d, p, th, rx).await }
    });
    tokio::time::sleep(Duration::from_millis(1500)).await;
    tx.send(Control::Pause).unwrap();
    let out = h.await.unwrap().unwrap();
    assert!(matches!(out, Outcome::Paused), "should pause");
    let got = p.done.load(std::sync::atomic::Ordering::Relaxed);
    let rate = got as f64 / t0.elapsed().as_secs_f64();
    println!("paused at {got} bytes, {:.1} MB/s with a 3 MB/s limit", rate / 1048576.0);
    assert!(got > 0 && got < SIZE);
    assert!(rate < 4.5 * 1048576.0, "limit not respected");
    let part = download::existing_bytes(&d);
    assert!(part > 0, "partial file kept");

    // Resume without a limit; must finish and pass the checksum.
    throttle.set(0);
    let (_tx2, rx2) = tokio::sync::watch::channel(Control::Run);
    let p2 = Arc::new(Progress::new());
    let out = download::fetch(&d, p2.clone(), throttle, rx2).await.unwrap();
    assert!(matches!(out, Outcome::Done));
    assert_eq!(std::fs::metadata(&d.dest).unwrap().len(), SIZE);
    assert_eq!(p2.done.load(std::sync::atomic::Ordering::Relaxed), SIZE, "progress counts resumed bytes once");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
#[ignore]
async fn bad_checksum_is_rejected_and_deleted() {
    let dir = tmp("badsum");
    let d = Download { url: URL.into(), dest: dir.join("g.msi"), size: Some(SIZE), sha256: Some("00".repeat(32)), headers: vec![] };
    let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
    let r = download::fetch(&d, Arc::new(Progress::new()), Arc::new(Throttle::new()), rx).await;
    assert!(r.is_err());
    assert!(!d.dest.exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn zip_unpacks_and_strips_the_top_folder() {
    let dir = tmp("zip");
    let zpath = dir.join("t.zip");
    {
        let f = std::fs::File::create(&zpath).unwrap();
        let mut w = zip::ZipWriter::new(f);
        let o = zip::write::SimpleFileOptions::default();
        w.add_directory("blender-9.9.9-windows-x64/", o).unwrap();
        w.start_file("blender-9.9.9-windows-x64/blender.exe", o).unwrap();
        std::io::Write::write_all(&mut w, b"exe").unwrap();
        w.start_file("blender-9.9.9-windows-x64/9.9/scripts/a.py", o).unwrap();
        std::io::Write::write_all(&mut w, b"print(1)").unwrap();
        w.finish().unwrap();
    }
    let dest = dir.join("Blender 9.9.9");
    let r = execute(&Step::ExtractZip { zip: zpath, dest: dest.clone(), strip_root: true, elevated: false, label: "x".into() });
    assert!(r.ok, "{}", r.message);
    assert_eq!(std::fs::read(dest.join("blender.exe")).unwrap(), b"exe");
    assert!(dest.join("9.9").join("scripts").join("a.py").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn run_step_reports_exit_codes() {
    let ok = execute(&Step::Run { exe: "cmd.exe".into(), args: vec!["/c".into(), "exit 3010".into()], cwd: None, ok_codes: vec![0, 3010], elevated: false, label: "x".into(), log: None });
    assert!(ok.ok);
    let bad = execute(&Step::Run { exe: "cmd.exe".into(), args: vec!["/c".into(), "echo nope & exit 1603".into()], cwd: None, ok_codes: vec![0], elevated: false, label: "x".into(), log: None });
    assert!(!bad.ok);
    assert_eq!(bad.code, Some(1603));
    assert!(bad.output.contains("nope"));
}

/// Talks to a real helper process over the named pipe (started without UAC in debug builds).
#[tokio::test]
async fn helper_runs_steps_over_the_pipe() {
    let exe = env!("CARGO_BIN_EXE_the-updater-3d");
    std::env::set_var("UPDATER3D_TEST_NO_UAC", "1");
    std::env::set_var("UPDATER3D_TEST_EXE", exe);
    let dir = tmp("helper");
    let el = Elevator::new();
    let f = dir.join("hello.txt");
    let r = el.run(Step::WriteFile { path: f.clone(), contents: "hi".into(), elevated: true }, false).await.unwrap();
    assert!(r.ok, "{}", r.message);
    assert_eq!(std::fs::read_to_string(&f).unwrap(), "hi");
    let r = el.run(Step::Run { exe: "cmd.exe".into(), args: vec!["/c".into(), "exit 7".into()], cwd: None, ok_codes: vec![0], elevated: true, label: "x".into(), log: None }, false).await.unwrap();
    assert!(!r.ok);
    assert_eq!(r.code, Some(7));
    // Same helper keeps serving.
    let r = el.run(Step::RemoveDir { path: dir.clone(), elevated: true, label: "x".into() }, false).await.unwrap();
    assert!(r.ok);
    assert!(!dir.exists());
    el.disconnect().await;
}
