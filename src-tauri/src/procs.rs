//! Running processes (for "app is open" checks and the Services screen).

use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

#[derive(Clone, Debug)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub exe: Option<PathBuf>,
    /// percent of the whole machine
    pub cpu: f32,
    pub memory: u64,
}

static SYS: Lazy<Mutex<System>> = Lazy::new(|| Mutex::new(System::new()));
static CACHE: Lazy<Mutex<Option<(std::time::Instant, Vec<Proc>)>>> = Lazy::new(|| Mutex::new(None));

/// Running processes, refreshed at most every 1.5 s (listing them is the slow part of a redraw).
pub fn list() -> Vec<Proc> {
    if let Some((t, v)) = CACHE.lock().as_ref() {
        if t.elapsed() < std::time::Duration::from_millis(1500) {
            return v.clone();
        }
    }
    let v = list_fresh();
    *CACHE.lock() = Some((std::time::Instant::now(), v.clone()));
    v
}

pub fn list_fresh() -> Vec<Proc> {
    let mut sys = SYS.lock();
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing().with_cpu().with_memory().with_exe(UpdateKind::OnlyIfNotSet),
    );
    let cores = sys.cpus().len().max(1) as f32;
    let cores = if cores <= 1.0 { std::thread::available_parallelism().map(|n| n.get() as f32).unwrap_or(1.0) } else { cores };
    sys.processes()
        .values()
        .map(|p| Proc {
            pid: p.pid().as_u32(),
            name: p.name().to_string_lossy().to_string(),
            exe: p.exe().map(Path::to_path_buf),
            cpu: p.cpu_usage() / cores,
            memory: p.memory(),
        })
        .collect()
}

fn under(exe: &Path, dir: &Path) -> bool {
    let e = exe.to_string_lossy().to_ascii_lowercase();
    let d = dir.to_string_lossy().to_ascii_lowercase();
    let d = d.trim_end_matches('\\');
    !d.is_empty() && e.starts_with(&format!("{d}\\"))
}

/// Processes running from inside any of `dirs`, or named like one of `names` (lowercase exe names).
pub fn blocking(procs: &[Proc], dirs: &[PathBuf], names: &[String]) -> Vec<Proc> {
    let me = std::process::id();
    procs
        .iter()
        .filter(|p| p.pid != me)
        .filter(|p| {
            p.exe.as_ref().is_some_and(|e| dirs.iter().any(|d| under(e, d))) || names.iter().any(|n| p.name.eq_ignore_ascii_case(n))
        })
        .cloned()
        .collect()
}

/// Friendly name for a blocking process: "Photoshop" from "Photoshop.exe".
pub fn friendly(name: &str) -> String {
    name.trim_end_matches(".exe").trim_end_matches(".EXE").to_string()
}

/// Ask the processes to close (like clicking X), then force them after `grace`.
pub fn close(pids: &[u32], grace: std::time::Duration) {
    for pid in pids {
        crate::winutil::close_process_windows(*pid);
    }
    let deadline = std::time::Instant::now() + grace;
    loop {
        let alive: Vec<u32> = list_fresh().iter().map(|p| p.pid).filter(|p| pids.contains(p)).collect();
        if alive.is_empty() {
            return;
        }
        if std::time::Instant::now() > deadline {
            for pid in alive {
                crate::winutil::kill_process(pid);
            }
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}
