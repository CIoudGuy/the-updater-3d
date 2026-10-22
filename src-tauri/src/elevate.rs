//! Admin rights. Installers need them, the app itself does not. When a step needs admin rights
//! the app starts a copy of itself elevated ("the helper", one UAC prompt) and talks to it over
//! a private named pipe for the rest of the batch. With "Fewer admin prompts" on, the helper is
//! started through a scheduled task that runs with highest privileges, so there is no prompt.

use crate::sources::Step;
use crate::{paths, winutil};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeServer, ServerOptions};

pub const TASK_NAME: &str = "The Updater 3D Helper";
const IDLE_EXIT: Duration = Duration::from_secs(15 * 60);
/// What the app sends to keep the helper from leaving while updates wait in the queue.
const PING: &str = "ping";
/// How long the scheduled task gets to start the helper before the normal prompt is used.
/// How long the helper started by the task may take to connect. Right after sign-in the PC can
/// be slow; past this the normal prompt is used (or, with no window open, none).
const TASK_START: Duration = Duration::from_secs(60);

fn idle_exit() -> Duration {
    // Debug builds can test the idle exit without waiting 15 minutes.
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("UPDATER3D_TEST_IDLE_MS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_millis(ms);
    }
    IDLE_EXIT
}

/// What the app sends after the token to learn the helper's version; the helper answers
/// "version <x.y.z>". Helpers from before this existed do not answer.
const VERSION_ASK: &str = "version";
const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Every pipe the app creates starts with this; the helper refuses any other pipe.
pub const PIPE_PREFIX: &str = r"\\.\pipe\the-updater-3d-";
/// worker.json older than this is not used.
const WORKER_FILE_MAX_AGE: i64 = 60;

#[derive(Serialize, Deserialize, Debug)]
struct Request {
    id: u64,
    step: Step,
    /// Files the step runs, with the SHA-256 they had when the download was checked.
    #[serde(default)]
    checks: Vec<FileCheck>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct FileCheck {
    pub path: PathBuf,
    pub sha256: String,
}

/// The downloaded files (with a known SHA-256) an elevated Run step starts or opens: the exe
/// itself, or an argument such as the MSI after "/i".
pub fn file_checks(step: &Step, downloads: &[crate::sources::Download]) -> Vec<FileCheck> {
    let Step::Run { exe, args, elevated: true, .. } = step else { return vec![] };
    let same = |a: &str, b: &Path| a.trim_matches('"').eq_ignore_ascii_case(&b.to_string_lossy());
    downloads
        .iter()
        .filter_map(|d| {
            let sha = d.sha256.as_ref()?;
            (same(&exe.to_string_lossy(), &d.dest) || args.iter().any(|a| same(a, &d.dest))).then(|| FileCheck { path: d.dest.clone(), sha256: sha.clone() })
        })
        .collect()
}

/// A pipe name the helper may connect to.
pub fn pipe_name_ok(name: &str) -> bool {
    name.len() > PIPE_PREFIX.len() && name[..PIPE_PREFIX.len()].eq_ignore_ascii_case(PIPE_PREFIX) && name[PIPE_PREFIX.len()..].chars().all(|c| c.is_ascii_alphanumeric())
}

/// The exe a scheduled task starts, from its XML definition.
pub fn task_command(xml: &str) -> Option<PathBuf> {
    let start = xml.find("<Command>")? + "<Command>".len();
    let end = start + xml[start..].find("</Command>")?;
    let raw = xml[start..end].trim().trim_matches('"');
    let s = raw.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    (!s.is_empty()).then(|| PathBuf::from(s))
}

/// What is known about the "Fewer admin prompts" task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TaskState {
    /// It starts this exe.
    Current,
    /// It starts another exe (the app was moved or installed somewhere else).
    OtherExe,
    Missing,
    /// It could not be read; nothing is known.
    Unknown,
}

pub fn task_state() -> TaskState {
    let Ok(me) = std::env::current_exe() else { return TaskState::Unknown };
    match winutil::task_xml_checked(TASK_NAME) {
        Ok(None) => TaskState::Missing,
        Ok(Some(x)) => match task_command(&x) {
            Some(c) if winutil::same_path(&c, &me) => TaskState::Current,
            Some(_) => TaskState::OtherExe,
            None => TaskState::Unknown,
        },
        Err(_) => TaskState::Unknown,
    }
}

/// Should the app ask for the one admin prompt that sets up the task? Only with the setting on,
/// with the window open (never at sign-in or from the background), once per run, and only when
/// the task is known to be missing or to start another exe. An app update installs to the same
/// path, so its task stays current and nothing is asked again.
pub fn should_set_up_task(setting_on: bool, task: TaskState, window_open: bool, asked_this_run: bool) -> bool {
    setting_on && window_open && !asked_this_run && matches!(task, TaskState::Missing | TaskState::OtherExe)
}

/// The "Fewer admin prompts" task exists and starts this exe (not another, maybe older, copy).
pub fn task_is_current() -> bool {
    let Ok(me) = std::env::current_exe() else { return false };
    winutil::task_xml(TASK_NAME).and_then(|x| task_command(&x)).is_some_and(|c| winutil::same_path(&c, &me))
}

/// How long a step may run before it is given up, so a hung installer cannot hold the queue
/// forever. Installers get two hours; file steps one.
fn step_timeout(step: &Step) -> Duration {
    #[cfg(debug_assertions)]
    if let Some(ms) = std::env::var("UPDATER3D_TEST_STEP_TIMEOUT_MS").ok().and_then(|v| v.parse().ok()) {
        return Duration::from_millis(ms);
    }
    match step {
        Step::Run { .. } => Duration::from_secs(2 * 3600),
        _ => Duration::from_secs(3600),
    }
}

fn timed_out(step: &Step, limit: Duration) -> anyhow::Error {
    let mins = limit.as_secs() / 60;
    let span = if mins == 60 { "1 hour".to_string() } else if mins >= 120 && mins % 60 == 0 { format!("{} hours", mins / 60) } else if mins >= 1 { format!("{mins} minutes") } else { format!("{} seconds", limit.as_secs().max(1)) };
    anyhow!("{} did not finish within {span}, so it was given up. The installer may still be running or waiting for an answer in a hidden window. Restart the PC if the app does not work, then try again.", step.label())
}

/// Open the files to check so nobody can change, replace or rename them until the step is
/// done (only reading is shared), and compare their SHA-256 through those handles.
fn lock_and_check(checks: &[FileCheck]) -> Result<Vec<std::fs::File>> {
    use sha2::{Digest, Sha256};
    use std::os::windows::fs::OpenOptionsExt;
    const FILE_SHARE_READ: u32 = 1;
    let mut held = Vec::new();
    for c in checks {
        let mut f = std::fs::OpenOptions::new().read(true).share_mode(FILE_SHARE_READ).open(&c.path).with_context(|| format!("Could not open {} to check it", c.path.display()))?;
        let mut h = Sha256::new();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            h.update(&buf[..n]);
        }
        let got = hex::encode(h.finalize());
        if !got.eq_ignore_ascii_case(&c.sha256) {
            bail!("{} changed after it was downloaded and checked, so it was not run. Clear the download cache and try again.", c.path.display());
        }
        held.push(f);
    }
    Ok(held)
}

/// `execute`, after checking the step's files (they stay locked while it runs).
pub fn execute_checked(step: &Step, checks: &[FileCheck]) -> StepResult {
    execute_checked_with(step, checks, &|_| {})
}

/// `execute_checked`, telling `on_procs` the programs an installer started while it runs.
pub fn execute_checked_with(step: &Step, checks: &[FileCheck], on_procs: &(dyn Fn(Vec<String>) + Sync)) -> StepResult {
    match lock_and_check(checks) {
        Ok(held) => {
            let r = execute_with(step, on_procs);
            drop(held);
            r
        }
        Err(e) => StepResult { ok: false, code: None, message: format!("{e:#}"), output: String::new() },
    }
}

/// The helper's answer to one line from the app (None: nothing to answer).
fn answer(line: &str) -> Option<(Option<Request>, String)> {
    let t = line.trim();
    if t.is_empty() || t == PING {
        return None;
    }
    if t == VERSION_ASK {
        return Some((None, format!("{VERSION_ASK} {VERSION}")));
    }
    match serde_json::from_str::<Request>(t) {
        Ok(req) => Some((Some(req), String::new())),
        // A step this copy does not know (the app is newer): say so instead of leaving the app
        // waiting for an answer that never comes.
        Err(e) => {
            let id = serde_json::from_str::<serde_json::Value>(t).ok().and_then(|v| v["id"].as_u64())?;
            let result = StepResult { ok: false, code: None, message: format!("The admin helper ({VERSION}) could not read this step: {e}"), output: String::new() };
            Some((None, serde_json::json!({ "id": id, "result": result }).to_string()))
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct StepResult {
    pub ok: bool,
    pub code: Option<u32>,
    pub message: String,
    pub output: String,
}

#[derive(Serialize, Deserialize)]
struct WorkerFile {
    pipe: String,
    token: String,
    created: i64,
}

// ---------------------------------------------------------------- executing steps

fn tail(s: &[u8], max: usize) -> String {
    let t = String::from_utf8_lossy(s);
    let t = t.trim();
    if t.len() > max {
        let mut start = t.len() - max;
        while !t.is_char_boundary(start) {
            start += 1;
        }
        format!("...{}", &t[start..])
    } else {
        t.to_string()
    }
}

fn extract_zip(zip: &Path, dest: &Path, strip_root: bool) -> Result<()> {
    let f = std::fs::File::open(zip).with_context(|| format!("open {}", zip.display()))?;
    let mut ar = zip::ZipArchive::new(std::io::BufReader::new(f))?;
    // Strip the single top folder ("blender-5.2.2-windows-x64/") when every entry has it.
    let mut root: Option<PathBuf> = None;
    if strip_root {
        for i in 0..ar.len() {
            let e = ar.by_index(i)?;
            let Some(p) = e.enclosed_name() else { continue };
            let first = p.components().next().map(|c| PathBuf::from(c.as_os_str()));
            match (&root, first) {
                (None, Some(f)) => root = Some(f),
                (Some(r), Some(f)) if *r == f => {}
                _ => {
                    root = None;
                    break;
                }
            }
        }
    }
    std::fs::create_dir_all(dest)?;
    let mut buf = vec![0u8; 1 << 20];
    for i in 0..ar.len() {
        let mut e = ar.by_index(i)?;
        let Some(rel) = e.enclosed_name() else { continue };
        let rel = match &root {
            Some(r) => match rel.strip_prefix(r) {
                Ok(x) => x.to_path_buf(),
                Err(_) => continue,
            },
            None => rel.to_path_buf(),
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = dest.join(&rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut w = std::io::BufWriter::new(std::fs::File::create(&out).with_context(|| format!("write {}", out.display()))?);
        loop {
            let n = e.read(&mut buf)?;
            if n == 0 {
                break;
            }
            w.write_all(&buf[..n])?;
        }
        w.flush()?;
    }
    Ok(())
}

fn remove_dir(p: &Path) -> Result<()> {
    if !p.exists() {
        return Ok(());
    }
    let mut last = None;
    for _ in 0..5 {
        match std::fs::remove_dir_all(p) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(600));
            }
        }
    }
    Err(anyhow!("Could not remove {}: {}", p.display(), last.map(|e| e.to_string()).unwrap_or_default()))
}

/// Folders the helper processes of an installer run from: temp folders (a bootstrapper's
/// unpacked setup, a self-elevated copy, an uninstaller copy), the download cache and the
/// installer cache of bundles. The app an installer starts when it is done runs from its install
/// folder, so it is never waited for.
fn installer_roots() -> Vec<PathBuf> {
    let mut v = vec![std::env::temp_dir(), paths::downloads()];
    if let Some(w) = std::env::var_os("SystemRoot").map(PathBuf::from) {
        v.push(w.join("Temp"));
        v.push(w.join("SystemTemp"));
    }
    if let Some(l) = std::env::var_os("LOCALAPPDATA").map(PathBuf::from) {
        v.push(l.join("Temp"));
    }
    if let Some(p) = std::env::var_os("ProgramData").map(PathBuf::from) {
        v.push(p.join("Package Cache"));
    }
    v
}

/// Is a process the installer started still part of the install? Windows Installer, another
/// copy of the installer, or a program in one of `roots`.
pub fn installer_child(image: &Path, installer: &Path, roots: &[PathBuf]) -> bool {
    let name = image.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if name == "msiexec.exe" || installer.file_name().is_some_and(|i| i.to_string_lossy().eq_ignore_ascii_case(&name)) {
        return true;
    }
    let img = image.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    roots.iter().any(|r| {
        let r = r.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
        let r = r.trim_end_matches('\\');
        !r.is_empty() && img.starts_with(&format!("{r}\\"))
    })
}

/// Run an installer and wait for it and for the helper processes it started (see
/// `installer_child`), within the step's time limit. Installers that hand the work to a child
/// and exit first would otherwise be checked before they are done. While it runs, `on_procs`
/// gets the exe names of the other programs in the job whenever they change.
fn run_tracked(step: &Step, exe: &Path, mut c: std::process::Command, on_procs: &(dyn Fn(Vec<String>) + Sync)) -> Result<std::process::Output> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;
    const CREATE_SUSPENDED: u32 = 0x0000_0004;
    let t0 = std::time::Instant::now();
    let job = winutil::InstallerJob::new();
    let suspended = job.is_some();
    if suspended {
        // Suspended until it is in the job, so nothing it starts escapes the job.
        c.creation_flags(winutil::CREATE_NO_WINDOW | CREATE_SUSPENDED);
    }
    c.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = c.spawn().with_context(|| format!("Could not start {}", exe.display()))?;
    let job = job.filter(|j| j.add(&child));
    if suspended && !winutil::resume_process(child.id()) {
        let _ = child.kill();
        bail!("Could not start {}", exe.display());
    }
    let root = child.id();
    let me = std::process::id();
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        // Which programs the installer runs, for the job's detail line.
        if let Some(job) = job.as_ref() {
            let done = &done;
            s.spawn(move || {
                let mut last: Vec<String> = vec![];
                while !done.load(std::sync::atomic::Ordering::Relaxed) {
                    let names: Vec<String> = job
                        .pids()
                        .into_iter()
                        .filter(|p| *p != me && *p != root)
                        .filter_map(winutil::process_image)
                        .filter_map(|i| i.file_name().map(|n| n.to_string_lossy().to_string()))
                        .collect();
                    if names != last {
                        on_procs(names.clone());
                        last = names;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            });
        }
        let r = (|| {
            let out = child.wait_with_output().with_context(|| format!("Could not run {}", exe.display()))?;
            if let Some(job) = job.as_ref() {
                let roots = installer_roots();
                let limit = step_timeout(step);
                let limit = limit.saturating_sub(limit / 20);
                while t0.elapsed() < limit {
                    let busy = job.pids().into_iter().filter(|p| *p != me).filter_map(winutil::process_image).any(|img| installer_child(&img, exe, &roots));
                    if !busy {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
            Ok(out)
        })();
        done.store(true, std::sync::atomic::Ordering::Relaxed);
        r
    })
}

/// Run one step in this process (the helper, or the app when it already runs as admin).
pub fn execute(step: &Step) -> StepResult {
    execute_with(step, &|_| {})
}

/// `execute`, telling `on_procs` the programs an installer started while it runs.
pub fn execute_with(step: &Step, on_procs: &(dyn Fn(Vec<String>) + Sync)) -> StepResult {
    let r: Result<(Option<u32>, String)> = (|| match step {
        Step::Run { exe, args, cwd, ok_codes, .. } => {
            let mut c = winutil::cmd(exe);
            winutil::add_args(&mut c, args);
            if let Some(d) = cwd {
                c.current_dir(d);
            }
            let out = run_tracked(step, exe, c, on_procs)?;
            let code = out.status.code().map(|c| c as u32).unwrap_or(1);
            let mut text = tail(&out.stdout, 3000);
            let err = tail(&out.stderr, 3000);
            if !err.is_empty() {
                text = format!("{text}\n{err}").trim().to_string();
            }
            if ok_codes.contains(&code) {
                Ok((Some(code), text))
            } else {
                Err(anyhow!("{}|{}", code, text))
            }
        }
        Step::ExtractZip { zip, dest, strip_root, .. } => extract_zip(zip, dest, *strip_root).map(|_| (None, String::new())),
        Step::CheckSigner { path, signer, .. } => crate::sources::check_signer(path, signer, winutil::authenticode_signer(path)).map(|_| (None, String::new())),
        Step::RemoveDir { path, .. } => remove_dir(path).map(|_| (None, String::new())),
        Step::WriteFile { path, contents, .. } => {
            if let Some(p) = path.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::write(path, contents)?;
            Ok((None, String::new()))
        }
        Step::RemoveFile { path } => {
            let _ = std::fs::remove_file(path);
            Ok((None, String::new()))
        }
        Step::Shortcut { lnk, target, .. } => {
            if let Some(p) = lnk.parent() {
                std::fs::create_dir_all(p)?;
            }
            // Written directly (no PowerShell): antivirus tools stop hidden script hosts.
            let mut link = mslnk::ShellLink::new(target).map_err(|e| anyhow!("Could not create the shortcut: {e}"))?;
            link.set_working_dir(target.parent().map(|p| p.to_string_lossy().to_string()));
            link.create_lnk(lnk).map_err(|e| anyhow!("Could not create the shortcut: {e}"))?;
            Ok((None, String::new()))
        }
        Step::OpenUri { uri, .. } => winutil::shell_open(uri).map(|_| (None, String::new())),
        Step::RemoveIfOnlyUninstaller { path, .. } => {
            if path.is_dir() && winutil::only_uninstaller_left(path) {
                remove_dir(path)?;
            }
            Ok((None, String::new()))
        }
        Step::RemoveUserRegKey { path, .. } => {
            match winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER).delete_subkey_all(path) {
                Ok(()) => Ok((None, String::new())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((None, String::new())),
                Err(e) => Err(anyhow!("Could not delete HKCU\\{path}: {e}")),
            }
        }
    })();
    match r {
        Ok((code, output)) => StepResult { ok: true, code, message: String::new(), output },
        Err(e) => {
            let s = e.to_string();
            // Run failures carry "code|output"
            if let Some((code, out)) = s.split_once('|') {
                if let Ok(c) = code.parse::<u32>() {
                    return StepResult { ok: false, code: Some(c), message: format!("Exit code {c}"), output: out.to_string() };
                }
            }
            StepResult { ok: false, code: None, message: s, output: String::new() }
        }
    }
}

// ---------------------------------------------------------------- the helper process

/// Entry point when started as `--worker <pipe> <token>` or `--worker-task`.
pub fn worker_main(args: &[String]) -> i32 {
    let from_task = args.iter().any(|a| a == "--worker-task");
    let (pipe, token) = if from_task {
        // Started by the scheduled task, which runs with admin rights without a prompt: worker.json
        // sits in a folder any program of this user can write, so trust it only when it is fresh,
        // names one of this app's pipes, and the pipe is served by this same exe (checked below).
        let file = paths::worker_file();
        let Ok(b) = std::fs::read(&file) else { return 2 };
        let _ = std::fs::remove_file(&file);
        let Ok(w) = serde_json::from_slice::<WorkerFile>(&b) else { return 2 };
        let age = chrono::Utc::now().timestamp() - w.created;
        if !(-5..=WORKER_FILE_MAX_AGE).contains(&age) {
            return 3;
        }
        (w.pipe, w.token)
    } else {
        let i = args.iter().position(|a| a == "--worker").unwrap_or(0);
        match (args.get(i + 1), args.get(i + 2)) {
            (Some(p), Some(t)) => (p.clone(), t.clone()),
            _ => return 2,
        }
    };
    if !pipe_name_ok(&pipe) {
        return 6;
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
    rt.block_on(async move {
        let mut client = None;
        for _ in 0..50 {
            match ClientOptions::new().open(&pipe) {
                Ok(c) => {
                    client = Some(c);
                    break;
                }
                Err(_) => tokio::time::sleep(Duration::from_millis(100)).await,
            }
        }
        let Some(client) = client else { return 4 };
        if from_task {
            use std::os::windows::io::AsRawHandle;
            let server_exe = winutil::pipe_server_pid(client.as_raw_handle() as isize).and_then(winutil::process_image);
            let me = std::env::current_exe().ok();
            if !matches!((server_exe, me), (Some(s), Some(m)) if winutil::same_path(&s, &m)) {
                return 7;
            }
        }
        let (r, mut w) = tokio::io::split(client);
        let mut r = BufReader::new(r);
        if w.write_all(format!("{token}\n").as_bytes()).await.is_err() {
            return 5;
        }
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(idle_exit(), r.read_line(&mut line)).await;
            match read {
                Ok(Ok(0)) | Err(_) | Ok(Err(_)) => return 0,
                Ok(Ok(_)) => {}
            }
            // "ping": the app is still busy with a batch, stay.
            let Some((req, reply)) = answer(&line) else { continue };
            let Some(req) = req else {
                if w.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
                    return 0;
                }
                continue;
            };
            let id = req.id;
            // The programs the installer runs go to the app as they change, before the result.
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<String>>();
            let mut run = tokio::task::spawn_blocking(move || {
                execute_checked_with(&req.step, &req.checks, &|names| {
                    let _ = tx.send(names);
                })
            });
            let result = loop {
                tokio::select! {
                    r = &mut run => break r,
                    Some(names) = rx.recv() => {
                        let line = serde_json::json!({ "id": id, "procs": names });
                        if w.write_all(format!("{line}\n").as_bytes()).await.is_err() {
                            return 0;
                        }
                    }
                }
            };
            let result = result.unwrap_or(StepResult {
                ok: false,
                code: None,
                message: "The admin helper crashed".into(),
                output: String::new(),
            });
            let reply = serde_json::json!({ "id": id, "result": result });
            if w.write_all(format!("{reply}\n").as_bytes()).await.is_err() {
                return 0;
            }
        }
    })
}

// ---------------------------------------------------------------- the app side

struct Conn {
    reader: BufReader<tokio::io::ReadHalf<NamedPipeServer>>,
    writer: tokio::io::WriteHalf<NamedPipeServer>,
    next_id: u64,
}

/// The exe names of the programs a running installer started (see `Elevator::run_watched`).
pub type Procs = std::sync::Arc<parking_lot::Mutex<Vec<String>>>;

pub struct Elevator {
    conn: tokio::sync::Mutex<Option<Conn>>,
    /// Helpers started so far (tests check that one helper serves a whole batch)
    connects: std::sync::atomic::AtomicU32,
    /// Times a helper was asked for (each one can be an admin prompt)
    prompts: std::sync::atomic::AtomicU32,
    /// The admin prompt was declined during this batch: the rest of the batch does not ask again
    declined: std::sync::atomic::AtomicBool,
    /// Debug builds: act as if the prompt was declined, to test that without a real prompt
    decline_for_test: bool,
}

const DECLINED: &str = "Admin rights were declined, so nothing was installed.";
/// No window is open, so no admin prompt is shown (at sign-in nobody may be there to answer it).
const NO_PROMPT: &str = "Admin rights are needed. Open the app to allow them.";

/// Admin rights could not be had for a step: the prompt was declined, could not be shown or was
/// not answered, or it was not shown because no window is open. Nothing ran, so the job can
/// wait for the user to allow admin rights instead of failing.
#[derive(Debug)]
pub struct AdminUnavailable(pub String);

impl std::fmt::Display for AdminUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AdminUnavailable {}

/// Did a step fail only because admin rights could not be had (see `AdminUnavailable`)?
pub fn admin_unavailable(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.is::<AdminUnavailable>())
}

impl Elevator {
    pub fn new() -> Self {
        Elevator { conn: tokio::sync::Mutex::new(None), connects: Default::default(), prompts: Default::default(), declined: Default::default(), decline_for_test: false }
    }

    #[cfg(debug_assertions)]
    pub fn declining_for_test() -> Self {
        Elevator { decline_for_test: true, ..Self::new() }
    }

    pub fn connects(&self) -> u32 {
        self.connects.load(std::sync::atomic::Ordering::SeqCst)
    }

    pub fn prompts(&self) -> u32 {
        self.prompts.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A new batch (or a retry) may ask for admin rights again.
    pub fn reset_declined(&self) {
        self.declined.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    /// Keep the helper from leaving while updates still wait (a long download can take longer
    /// than its idle time). Does nothing while a step runs or when there is no helper.
    pub async fn keep_alive(&self) {
        let Ok(mut guard) = self.conn.try_lock() else { return };
        if let Some(c) = guard.as_mut() {
            if c.writer.write_all(format!("{PING}\n").as_bytes()).await.is_err() {
                *guard = None;
            }
        }
    }

    /// Run a step with admin rights when it needs them.
    pub async fn run(&self, step: Step, silent: bool) -> Result<StepResult> {
        self.run_checked(step, silent, vec![]).await
    }

    /// `run`, with files the step runs checked against their SHA-256 right before it runs.
    pub async fn run_checked(&self, step: Step, silent: bool, checks: Vec<FileCheck>) -> Result<StepResult> {
        self.run_with(step, silent, checks, true).await
    }

    /// `run_checked`; `may_prompt` false: never show an admin prompt (only the "Fewer admin
    /// prompts" task may start the helper), for work done with no window open.
    pub async fn run_with(&self, step: Step, silent: bool, checks: Vec<FileCheck>, may_prompt: bool) -> Result<StepResult> {
        self.run_watched(step, silent, checks, may_prompt, None).await
    }

    /// `run_with`; `procs` gets the exe names of the programs an installer started while it
    /// runs (from the helper too), for the job's detail line.
    pub async fn run_watched(&self, step: Step, silent: bool, checks: Vec<FileCheck>, may_prompt: bool, procs: Option<Procs>) -> Result<StepResult> {
        let limit = step_timeout(&step);
        if !step.elevated() || winutil::is_elevated() {
            let s2 = step.clone();
            let p2 = procs.clone();
            let run = move || {
                execute_checked_with(&s2, &checks, &|names| {
                    if let Some(p) = &p2 {
                        *p.lock() = names;
                    }
                })
            };
            return match tokio::time::timeout(limit, tokio::task::spawn_blocking(run)).await {
                Ok(r) => Ok(r?),
                Err(_) => Err(timed_out(&step, limit)),
            };
        }
        let mut guard = self.conn.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                // Declined once in this batch: the other updates do not open a prompt each.
                if self.declined.load(std::sync::atomic::Ordering::SeqCst) {
                    return Err(AdminUnavailable(DECLINED.into()).into());
                }
                self.prompts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                match connect(silent, self.decline_for_test, may_prompt).await {
                    Ok(c) => {
                        self.connects.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        *guard = Some(c);
                    }
                    Err(e) => {
                        let text = format!("{e:#}");
                        // Declined, failed or not answered: the rest of the batch does not ask
                        // again until the user allows admin rights.
                        if text != NO_PROMPT {
                            self.declined.store(true, std::sync::atomic::Ordering::SeqCst);
                        }
                        return Err(AdminUnavailable(text).into());
                    }
                }
            }
            let c = guard.as_mut().unwrap();
            c.next_id += 1;
            let id = c.next_id;
            let line = serde_json::to_string(&Request { id, step: step.clone(), checks: checks.clone() })? + "\n";
            let sent = c.writer.write_all(line.as_bytes()).await;
            if sent.is_ok() {
                // Installers can take a long time, but not forever: past the limit the step fails
                // and this helper (still busy with it) is let go.
                let deadline = tokio::time::Instant::now() + limit;
                loop {
                    let mut reply = String::new();
                    match tokio::time::timeout_at(deadline, c.reader.read_line(&mut reply)).await {
                        Err(_) => {
                            *guard = None;
                            return Err(timed_out(&step, limit));
                        }
                        Ok(Ok(n)) if n > 0 => {
                            let v: serde_json::Value = serde_json::from_str(&reply)?;
                            // The programs the installer runs, sent while it runs.
                            if let Some(names) = v.get("procs").and_then(|p| p.as_array()) {
                                if let Some(p) = &procs {
                                    *p.lock() = names.iter().filter_map(|n| n.as_str().map(str::to_string)).collect();
                                }
                                continue;
                            }
                            return Ok(serde_json::from_value(v["result"].clone())?);
                        }
                        Ok(_) => break,
                    }
                }
            }
            // The helper went away (idle timeout); start a new one once.
            *guard = None;
            if attempt == 1 {
                bail!("Lost the connection to the admin helper");
            }
        }
        unreachable!()
    }

    pub async fn disconnect(&self) {
        *self.conn.lock().await = None;
    }
}

fn new_pipe() -> Result<(String, String, NamedPipeServer)> {
    let pipe = format!("{PIPE_PREFIX}{}", uuid::Uuid::new_v4().simple());
    let token = uuid::Uuid::new_v4().simple().to_string();
    let server = ServerOptions::new().first_pipe_instance(true).max_instances(1).create(&pipe)?;
    Ok((pipe, token, server))
}

/// Start the helper through the "Fewer admin prompts" task. None when the task cannot be used
/// (missing, set up for another copy of the app, or its helper is another version); the
/// normal prompt is used then instead of failing the update.
async fn connect_via_task() -> Option<Conn> {
    if !tokio::task::spawn_blocking(task_is_current).await.unwrap_or(false) {
        return None;
    }
    let (pipe, token, server) = new_pipe().ok()?;
    let wf = WorkerFile { pipe, token: token.clone(), created: chrono::Utc::now().timestamp() };
    std::fs::write(paths::worker_file(), serde_json::to_vec(&wf).ok()?).ok()?;
    let mut connected = false;
    if tokio::task::spawn_blocking(|| winutil::run_task(TASK_NAME)).await.unwrap_or(false) {
        connected = matches!(tokio::time::timeout(TASK_START, server.connect()).await, Ok(Ok(())));
    }
    let _ = std::fs::remove_file(paths::worker_file());
    if !connected {
        return None;
    }
    match handshake(server, &token).await {
        Ok(c) => Some(c),
        Err(e) => {
            log::warn!("admin helper from the task not used: {e:#}");
            None
        }
    }
}

/// Check the helper's token and version.
async fn handshake(server: NamedPipeServer, token: &str) -> Result<Conn> {
    let (r, mut w) = tokio::io::split(server);
    let mut reader = BufReader::new(r);
    let mut first = String::new();
    tokio::time::timeout(Duration::from_secs(20), reader.read_line(&mut first)).await.map_err(|_| anyhow!("The admin helper did not answer"))??;
    if first.trim() != token {
        bail!("The admin helper could not be verified");
    }
    // A helper from another version may not know the steps this one sends.
    w.write_all(format!("{VERSION_ASK}\n").as_bytes()).await?;
    let mut ver = String::new();
    let got = tokio::time::timeout(Duration::from_secs(5), reader.read_line(&mut ver)).await;
    let want = format!("{VERSION_ASK} {VERSION}");
    if !matches!(got, Ok(Ok(n)) if n > 0) || ver.trim() != want {
        bail!("The admin helper is another version of the app ({})", ver.trim());
    }
    Ok(Conn { reader, writer: w, next_id: 0 })
}

async fn connect(silent: bool, decline_for_test: bool, may_prompt: bool) -> Result<Conn> {
    if silent {
        if let Some(c) = connect_via_task().await {
            return Ok(c);
        }
    }
    if !may_prompt {
        bail!("{NO_PROMPT}");
    }
    if decline_for_test {
        bail!("{DECLINED}");
    }
    let (pipe, token, server) = new_pipe()?;
    let exe = std::env::current_exe()?;
    #[allow(unused_mut)]
    let mut started = false;
    // Debug builds can test the helper protocol without a UAC prompt.
    #[cfg(debug_assertions)]
    if !started && std::env::var_os("UPDATER3D_TEST_NO_UAC").is_some() {
        std::process::Command::new(std::env::var_os("UPDATER3D_TEST_EXE").map(PathBuf::from).unwrap_or(exe.clone())).args(["--worker", &pipe, &token]).spawn()?;
        started = true;
    }
    if !started {
        let params = format!("--worker {} {}", winutil::quote_arg(&pipe), token);
        let exe2 = exe.clone();
        let res = tokio::task::spawn_blocking(move || winutil::shell_execute_elevated(&exe2, &params, false)).await?;
        match res {
            Ok(h) => {
                // We do not need the process handle, the pipe tells us when it is gone.
                std::thread::spawn(move || {
                    winutil::wait_process(h);
                });
            }
            Err(e) if e.to_string() == "declined" => bail!("{DECLINED}"),
            Err(e) => return Err(e),
        }
    }
    // Wait for the helper (the user may take a while on the UAC prompt).
    tokio::time::timeout(Duration::from_secs(180), server.connect())
        .await
        .map_err(|_| anyhow!("The admin helper did not start"))??;
    handshake(server, &token).await
}

pub fn task_exists() -> bool {
    winutil::task_exists(TASK_NAME)
}

/// Steps that register (or remove) the scheduled task used for prompt-free installs.
pub fn task_steps(enable: bool) -> Result<Vec<Step>> {
    if !enable {
        return Ok(vec![Step::Run {
            exe: PathBuf::from("schtasks.exe"),
            args: vec!["/Delete".into(), "/TN".into(), TASK_NAME.into(), "/F".into()],
            cwd: None,
            ok_codes: vec![0, 1],
            elevated: true,
            label: "Turning off prompt-free installs".into(),
            log: None,
        }]);
    }
    let exe = std::env::current_exe()?;
    let sid = winutil::current_user_sid().ok_or_else(|| anyhow!("Could not read your Windows account"))?;
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Lets The Updater 3D install updates without a UAC prompt each time.</Description>
    <SecurityDescriptor>D:(A;;FA;;;BA)(A;;FA;;;SY)(A;;FRFX;;;{sid})</SecurityDescriptor>
  </RegistrationInfo>
  <Principals>
    <Principal id="Author">
      <UserId>{sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>HighestAvailable</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>Parallel</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>true</Hidden>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{exe}</Command>
      <Arguments>--worker-task</Arguments>
    </Exec>
  </Actions>
</Task>"#,
        sid = sid,
        exe = exe.display().to_string().replace('&', "&amp;").replace('<', "&lt;")
    );
    let file = paths::data_dir().join("helper-task.xml");
    // schtasks wants UTF-16 for XML with that declaration.
    let mut bytes: Vec<u8> = vec![0xFF, 0xFE];
    for u in xml.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(&file, bytes)?;
    Ok(vec![Step::Run {
        exe: PathBuf::from("schtasks.exe"),
        args: vec!["/Create".into(), "/TN".into(), TASK_NAME.into(), "/XML".into(), file.to_string_lossy().into(), "/F".into()],
        cwd: None,
        ok_codes: vec![0],
        elevated: true,
        label: "Turning on prompt-free installs".into(),
        log: None,
    }])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn helper_only_uses_this_apps_pipes() {
        assert!(pipe_name_ok(&format!("{PIPE_PREFIX}0123abcd")));
        assert!(pipe_name_ok(r"\\.\pipe\the-updater-3d-0123abcd"));
        assert!(!pipe_name_ok(r"\\.\pipe\something-else"));
        assert!(!pipe_name_ok(r"\\.\pipe\the-updater-3d-"));
        assert!(!pipe_name_ok(r"\\server\pipe\the-updater-3d-abc"));
        assert!(!pipe_name_ok(r"\\.\pipe\the-updater-3d-a\..\other"));
    }

    #[test]
    fn task_exe_is_read_from_its_xml() {
        let xml = r#"<Task><Actions Context="Author"><Exec><Command>C:\Apps\A &amp; B\the-updater-3d.exe</Command><Arguments>--worker-task</Arguments></Exec></Actions></Task>"#;
        assert_eq!(task_command(xml), Some(PathBuf::from(r"C:\Apps\A & B\the-updater-3d.exe")));
        assert_eq!(task_command("<Task/>"), None);
    }

    #[test]
    fn unknown_step_gets_a_failed_answer_with_its_id() {
        let (req, reply) = answer(r#"{"id":42,"step":{"t":"SomethingNew","x":1}}"#).expect("an answer");
        assert!(req.is_none());
        let v: serde_json::Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(v["id"], 42);
        assert_eq!(v["result"]["ok"], false);
        assert!(answer("ping\n").is_none());
        assert_eq!(answer("version\n").unwrap().1, format!("version {VERSION}"));
    }

    #[test]
    fn elevated_runs_of_downloads_are_checked_again() {
        let dl = |dest: &str, sha: Option<&str>| crate::sources::Download { url: String::new(), dest: PathBuf::from(dest), size: None, sha256: sha.map(String::from), md5: None, signer: None, sha512: None, headers: vec![] };
        let downloads = vec![dl(r"C:\d\setup.exe", Some("aa")), dl(r"C:\d\app.msi", Some("bb")), dl(r"C:\d\nosum.exe", None)];
        let run = |exe: &str, args: &[&str], elevated: bool| Step::Run { exe: exe.into(), args: args.iter().map(|s| s.to_string()).collect(), cwd: None, ok_codes: vec![0], elevated, label: "x".into(), log: None };
        assert_eq!(file_checks(&run(r"C:\d\setup.exe", &["/S"], true), &downloads), vec![FileCheck { path: r"C:\d\setup.exe".into(), sha256: "aa".into() }]);
        assert_eq!(file_checks(&run("msiexec.exe", &["/i", r"C:\d\app.msi", "/qn"], true), &downloads), vec![FileCheck { path: r"C:\d\app.msi".into(), sha256: "bb".into() }]);
        assert!(file_checks(&run(r"C:\d\nosum.exe", &[], true), &downloads).is_empty());
        assert!(file_checks(&run(r"C:\d\setup.exe", &[], false), &downloads).is_empty());
    }

    #[test]
    fn a_swapped_file_is_not_run_and_a_checked_one_is_locked() {
        let dir = std::env::temp_dir().join(format!("u3d-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("setup.bin");
        std::fs::write(&f, b"hello world").unwrap();
        // sha256("hello world")
        let good = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
        let step = Step::WriteFile { path: dir.join("ran.txt"), contents: "1".into(), elevated: false };
        let bad = execute_checked(&step, &[FileCheck { path: f.clone(), sha256: "00".repeat(32) }]);
        assert!(!bad.ok && bad.message.contains("changed after it was downloaded"), "{}", bad.message);
        assert!(!dir.join("ran.txt").exists(), "the step must not run");
        let held = lock_and_check(&[FileCheck { path: f.clone(), sha256: good.into() }]).unwrap();
        assert!(std::fs::write(&f, b"swapped").is_err(), "nobody can change it while it runs");
        assert!(std::fs::rename(&f, dir.join("moved.bin")).is_err());
        assert!(std::fs::read(&f).is_ok(), "the installer can still read it");
        drop(held);
        assert!(execute_checked(&step, &[FileCheck { path: f.clone(), sha256: good.into() }]).ok);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod prompt_tests {
    use super::{installer_child, should_set_up_task, TaskState};
    use std::path::{Path, PathBuf};

    #[test]
    fn the_task_prompt_is_asked_only_with_the_window_open_and_only_when_needed() {
        use TaskState::*;
        // Set up once when it is missing or starts another exe, with the window open.
        assert!(should_set_up_task(true, Missing, true, false));
        assert!(should_set_up_task(true, OtherExe, true, false));
        // Never at sign-in or from the background.
        assert!(!should_set_up_task(true, Missing, false, false));
        assert!(!should_set_up_task(true, OtherExe, false, false));
        // Not again in the same run, not with the setting off.
        assert!(!should_set_up_task(true, Missing, true, true));
        assert!(!should_set_up_task(false, Missing, true, false));
        // An app update keeps the exe path, so its task stays current.
        assert!(!should_set_up_task(true, Current, true, false));
        // A task that could not be read (Task Scheduler slow right after sign-in) is not
        // registered again with a prompt.
        assert!(!should_set_up_task(true, Unknown, true, false));
    }

    #[test]
    fn only_the_installers_own_helpers_are_waited_for() {
        let roots = vec![PathBuf::from(r"C:\Users\A\AppData\Local\Temp"), PathBuf::from(r"C:\Users\A\AppData\Local\app.theupdater3d\data\downloads")];
        let setup = Path::new(r"C:\Users\A\AppData\Local\app.theupdater3d\data\downloads\other\Mega\MEGAsyncSetup64.exe");
        assert!(installer_child(Path::new(r"C:\Users\A\AppData\Local\Temp\nsu1.tmp\Au_.exe"), setup, &roots));
        assert!(installer_child(Path::new(r"C:\Windows\System32\msiexec.exe"), setup, &roots));
        assert!(installer_child(Path::new(r"C:\Somewhere\MEGAsyncSetup64.exe"), setup, &roots));
        assert!(installer_child(Path::new(r"c:\users\a\appdata\local\temp\{GUID}\.cr\python-3.14.7-amd64.exe"), setup, &roots));
        // The app the installer starts when it is done keeps running and is not waited for.
        assert!(!installer_child(Path::new(r"C:\Users\A\AppData\Local\MEGAsync\MEGAsync.exe"), setup, &roots));
        assert!(!installer_child(Path::new(r"C:\Windows\System32\cmd.exe"), setup, &roots));
        assert!(!installer_child(Path::new(r"C:\Users\A\AppData\Local\Temperature\x.exe"), setup, &roots));
    }
}
