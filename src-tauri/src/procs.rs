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
    pub parent: Option<u32>,
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
            parent: p.parent().map(|x| x.as_u32()),
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

/// Programs that have any of `files` open or loaded (Windows' Restart Manager: what installers
/// ask before they replace files). Files that are not there are skipped.
pub fn using_files(files: &[PathBuf]) -> Vec<u32> {
    use windows::core::{HSTRING, PCWSTR, PWSTR};
    use windows::Win32::System::RestartManager::*;
    let files: Vec<HSTRING> = files.iter().filter(|f| f.exists()).map(|f| HSTRING::from(f.as_os_str())).collect();
    if files.is_empty() {
        return vec![];
    }
    let me = std::process::id();
    unsafe {
        let mut session = 0u32;
        let mut key = [0u16; (CCH_RM_SESSION_KEY + 1) as usize];
        if RmStartSession(&mut session, None, PWSTR(key.as_mut_ptr())).is_err() {
            return vec![];
        }
        let names: Vec<PCWSTR> = files.iter().map(|f| PCWSTR(f.as_ptr())).collect();
        let mut out = vec![];
        if RmRegisterResources(session, Some(&names), None, None).is_ok() {
            let mut need = 0u32;
            let mut have = 0u32;
            let mut reasons = 0u32;
            let _ = RmGetList(session, &mut need, &mut have, None, &mut reasons);
            if need > 0 {
                let mut list = vec![RM_PROCESS_INFO::default(); need as usize + 4];
                have = list.len() as u32;
                if RmGetList(session, &mut need, &mut have, Some(list.as_mut_ptr()), &mut reasons).is_ok() {
                    out = list[..have as usize].iter().map(|p| p.Process.dwProcessId).filter(|&p| p != 0 && p != me).collect();
                }
            }
        }
        let _ = RmEndSession(session);
        out.sort_unstable();
        out.dedup();
        out
    }
}

/// The running programs (exe names, lowercase) that are the app `app` names, wherever they run
/// from: "ShareX" is ShareX.exe in Steam's folder as much as in Program Files. A program whose
/// name is the app's, or starts with it ("Discord" and DiscordPTB.exe); never this app itself.
pub fn running_as(procs: &[Proc], app: &str) -> Vec<String> {
    let plain = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let want = plain(app);
    if want.is_empty() {
        return Vec::new();
    }
    let me = std::process::id();
    let mut out: Vec<String> = procs
        .iter()
        .filter(|p| p.pid != me)
        .filter(|p| {
            let stem = plain(p.name.trim_end_matches(".exe").trim_end_matches(".EXE"));
            stem == want || (want.len() >= 4 && stem.starts_with(&want))
        })
        .map(|p| p.name.to_ascii_lowercase())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// The process to close for `pid`: the first one of its program. A browser's camera helper leads
/// to the browser, which would only start the helper again if the helper alone were ended.
pub fn owner(procs: &[Proc], pid: u32) -> u32 {
    let mut cur = pid;
    for _ in 0..16 {
        let Some(p) = procs.iter().find(|x| x.pid == cur) else { break };
        let Some(up) = p.parent.and_then(|u| procs.iter().find(|x| x.pid == u)) else { break };
        if !up.name.eq_ignore_ascii_case(&p.name) {
            break;
        }
        cur = up.pid;
    }
    cur
}

/// A program that keeps `file` open until it is killed, as a game keeps OBS's capture hook loaded.
/// ping has next to nothing to load: PowerShell took over 15 s to start on a busy GitHub runner.
#[cfg(test)]
pub(crate) fn holder_of(file: &Path) -> std::process::Child {
    let f = std::fs::File::open(file).unwrap();
    crate::winutil::cmd("ping.exe").args(["-n", "60", "127.0.0.1"]).stdin(f).stdout(std::process::Stdio::null()).spawn().unwrap()
}

/// Friendly name for a blocking process: "Photoshop" from "Photoshop.exe".
pub fn friendly(name: &str) -> String {
    name.trim_end_matches(".exe").trim_end_matches(".EXE").to_string()
}

/// Ask the processes to close (like clicking X), then force them after `grace`. A process with
/// no window of its own (a background helper such as a browser's native messaging host) has
/// nothing to click, so it is ended at once. Returns the ones still running (one that runs as
/// admin cannot be ended from here).
pub fn close(pids: &[u32], grace: std::time::Duration) -> Vec<u32> {
    let (windowed, windowless): (Vec<u32>, Vec<u32>) = pids.iter().partition(|p| crate::winutil::has_windows(**p));
    for pid in &windowed {
        crate::winutil::close_process_windows(*pid);
    }
    for pid in &windowless {
        crate::winutil::kill_process(*pid);
    }
    let deadline = std::time::Instant::now() + grace;
    let mut forced = false;
    loop {
        let alive: Vec<u32> = list_fresh().iter().map(|p| p.pid).filter(|p| pids.contains(p)).collect();
        if alive.is_empty() {
            return alive;
        }
        if forced {
            return alive;
        }
        if std::time::Instant::now() > deadline || alive.iter().all(|p| windowless.contains(p)) {
            for pid in &alive {
                crate::winutil::kill_process(*pid);
            }
            forced = true;
            // Ending a process takes a moment.
            std::thread::sleep(std::time::Duration::from_millis(600));
            continue;
        }
        std::thread::sleep(std::time::Duration::from_millis(400));
    }
}

#[cfg(test)]
mod live {
    /// The programs on this PC that hold OBS Studio's files: `cargo test --lib live_obs_files -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_obs_files() {
        let files = crate::sources::tools::obs_in_use_files(&std::path::PathBuf::from(r"C:\Program Files\obs-studio"));
        let pids = super::using_files(&files);
        let list = super::list_fresh();
        for p in &pids {
            println!("{p} {}", list.iter().find(|x| x.pid == *p).map(|x| x.name.as_str()).unwrap_or("?"));
        }
        println!("{} of {} files exist", files.iter().filter(|f| f.exists()).count(), files.len());
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_app_is_found_running_by_its_name_wherever_it_runs() {
        let p = |pid: u32, name: &str, exe: &str| super::Proc { pid, name: name.into(), exe: Some(exe.into()), parent: None, cpu: 0.0, memory: 0 };
        let list = vec![
            p(10, "ShareX.exe", r"C:\Program Files (x86)\Steam\steamapps\common\ShareX\ShareX.exe"),
            p(11, "sharex.exe", r"D:\Portable\ShareX\sharex.exe"),
            p(12, "DiscordPTB.exe", r"C:\Users\x\AppData\Local\DiscordPTB\DiscordPTB.exe"),
            p(13, "explorer.exe", r"C:\Windows\explorer.exe"),
            p(14, "ShareXHelper.exe", r"C:\x\ShareXHelper.exe"),
        ];
        assert_eq!(super::running_as(&list, "ShareX"), vec!["sharex.exe".to_string(), "sharexhelper.exe".to_string()]);
        assert_eq!(super::running_as(&list, "Discord"), vec!["discordptb.exe".to_string()]);
        // A short name only matches the whole name ("Go" is not GOG Galaxy).
        assert!(super::running_as(&[p(20, "GoogleDrive.exe", r"C:\g.exe")], "Go").is_empty());
        assert!(super::running_as(&list, "OBS Studio").is_empty());
        assert!(super::running_as(&list, "  ").is_empty());
    }

    #[test]
    fn a_helper_leads_to_the_program_that_would_start_it_again() {
        let p = |pid: u32, name: &str, parent: Option<u32>| super::Proc { pid, name: name.into(), exe: None, parent, cpu: 0.0, memory: 0 };
        let list = vec![p(1, "explorer.exe", None), p(10, "Browser.exe", Some(1)), p(11, "browser.exe", Some(10)), p(12, "browser.exe", Some(11)), p(20, "wenativehost.exe", Some(10)), p(30, "obs64.exe", Some(99))];
        // The camera helper, however deep: the browser itself, not what started the browser.
        assert_eq!(super::owner(&list, 12), 10);
        assert_eq!(super::owner(&list, 10), 10);
        // Another program started by it, a parent that is gone, a process that is gone.
        assert_eq!(super::owner(&list, 20), 20);
        assert_eq!(super::owner(&list, 30), 30);
        assert_eq!(super::owner(&list, 500), 500);
    }

    #[test]
    fn a_program_holding_a_file_open_is_found() {
        let dir = std::env::temp_dir().join(format!("u3d-rm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("graphics-hook64.dll");
        std::fs::write(&f, b"x").unwrap();
        assert!(super::using_files(&[f.clone()]).is_empty());
        let mut child = super::holder_of(&f);
        let t = std::time::Instant::now();
        let mut found = vec![];
        while found.is_empty() && t.elapsed() < std::time::Duration::from_secs(15) {
            std::thread::sleep(std::time::Duration::from_millis(300));
            found = super::using_files(&[f.clone()]);
        }
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(found, vec![child.id()]);
        assert!(super::using_files(&[dir.join("missing.dll")]).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_helper_with_no_window_is_closed_at_once() {
        // Like a browser's native messaging host (wenativehost): nothing to click X on.
        let mut child = crate::winutil::cmd("ping.exe").args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
        let pid = child.id();
        let t = std::time::Instant::now();
        let left = super::close(&[pid], std::time::Duration::from_secs(12));
        assert!(left.is_empty(), "still running");
        assert!(t.elapsed() < std::time::Duration::from_secs(4), "waited {:?} for a window that is not there", t.elapsed());
        assert!(child.try_wait().unwrap().is_some());
    }
}
