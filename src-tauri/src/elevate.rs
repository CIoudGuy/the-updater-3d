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

#[derive(Serialize, Deserialize, Debug)]
struct Request {
    id: u64,
    step: Step,
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

/// Run one step in this process (the helper, or the app when it already runs as admin).
pub fn execute(step: &Step) -> StepResult {
    let r: Result<(Option<u32>, String)> = (|| match step {
        Step::Run { exe, args, cwd, ok_codes, .. } => {
            let mut c = winutil::cmd(exe);
            winutil::add_args(&mut c, args);
            if let Some(d) = cwd {
                c.current_dir(d);
            }
            let out = c.output().with_context(|| format!("Could not start {}", exe.display()))?;
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
    let (pipe, token) = if args.iter().any(|a| a == "--worker-task") {
        let Ok(b) = std::fs::read(paths::worker_file()) else { return 2 };
        let Ok(w) = serde_json::from_slice::<WorkerFile>(&b) else { return 2 };
        if (chrono::Utc::now().timestamp() - w.created).abs() > 120 {
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
        let (r, mut w) = tokio::io::split(client);
        let mut r = BufReader::new(r);
        if w.write_all(format!("{token}\n").as_bytes()).await.is_err() {
            return 5;
        }
        loop {
            let mut line = String::new();
            let read = tokio::time::timeout(IDLE_EXIT, r.read_line(&mut line)).await;
            match read {
                Ok(Ok(0)) | Err(_) | Ok(Err(_)) => return 0,
                Ok(Ok(_)) => {}
            }
            let Ok(req) = serde_json::from_str::<Request>(&line) else { continue };
            let result = tokio::task::spawn_blocking(move || execute(&req.step)).await.unwrap_or(StepResult {
                ok: false,
                code: None,
                message: "The admin helper crashed".into(),
                output: String::new(),
            });
            let reply = serde_json::json!({ "id": req.id, "result": result });
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

pub struct Elevator {
    conn: tokio::sync::Mutex<Option<Conn>>,
}

impl Elevator {
    pub fn new() -> Self {
        Elevator { conn: tokio::sync::Mutex::new(None) }
    }

    /// Run a step with admin rights when it needs them.
    pub async fn run(&self, step: Step, silent: bool) -> Result<StepResult> {
        if !step.elevated() || winutil::is_elevated() {
            return Ok(tokio::task::spawn_blocking(move || execute(&step)).await?);
        }
        let mut guard = self.conn.lock().await;
        for attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(connect(silent).await?);
            }
            let c = guard.as_mut().unwrap();
            c.next_id += 1;
            let id = c.next_id;
            let line = serde_json::to_string(&Request { id, step: step.clone() })? + "\n";
            let sent = c.writer.write_all(line.as_bytes()).await;
            if sent.is_ok() {
                let mut reply = String::new();
                // Installers can take a long time; no timeout here, the helper always answers or dies.
                if let Ok(n) = c.reader.read_line(&mut reply).await {
                    if n > 0 {
                        let v: serde_json::Value = serde_json::from_str(&reply)?;
                        return Ok(serde_json::from_value(v["result"].clone())?);
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

async fn connect(silent: bool) -> Result<Conn> {
    let pipe = format!(r"\\.\pipe\the-updater-3d-{}", uuid::Uuid::new_v4().simple());
    let token = uuid::Uuid::new_v4().simple().to_string();
    let server = ServerOptions::new().first_pipe_instance(true).max_instances(1).create(&pipe)?;
    let exe = std::env::current_exe()?;

    let mut started = false;
    if silent && task_exists() {
        let wf = WorkerFile { pipe: pipe.clone(), token: token.clone(), created: chrono::Utc::now().timestamp() };
        std::fs::write(paths::worker_file(), serde_json::to_vec(&wf)?)?;
        started = tokio::task::spawn_blocking(|| winutil::run_task(TASK_NAME)).await.unwrap_or(false);
    }
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
            Err(e) if e.to_string() == "declined" => bail!("Admin rights were declined, so nothing was installed."),
            Err(e) => return Err(e),
        }
    }
    // Wait for the helper (the user may take a while on the UAC prompt).
    tokio::time::timeout(Duration::from_secs(180), server.connect())
        .await
        .map_err(|_| anyhow!("The admin helper did not start"))??;
    let _ = std::fs::remove_file(paths::worker_file());
    let (r, w) = tokio::io::split(server);
    let mut reader = BufReader::new(r);
    let mut first = String::new();
    tokio::time::timeout(Duration::from_secs(20), reader.read_line(&mut first)).await.map_err(|_| anyhow!("The admin helper did not answer"))??;
    if first.trim() != token {
        bail!("The admin helper could not be verified");
    }
    Ok(Conn { reader, writer: w, next_id: 0 })
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
