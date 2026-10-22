//! The app's brain: installed apps, latest versions, the job queue and the background checks.

use crate::download::{self, Control, Outcome, Progress, Throttle};
use crate::elevate::Elevator;
use crate::model::*;
use crate::sources::{self, adobe, blender, craft, other, tools, Installed, Latest, Plan, SourceData, Verify};
use crate::store::{self, Persisted};
use crate::{paths, procs, version, winutil};
use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{watch, Notify};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum JobRequest {
    Update { app_id: String, version: Option<String> },
    Install { id: String, version: Option<String> },
    /// `remove_prefs`: also delete the app's settings and add-ons ("Also delete settings and add-ons")
    Uninstall { app_id: String, remove_prefs: bool },
    Repair { app_id: String },
}

/// An Other app without an icon is searched for again after this many days.
const ICON_RETRY_DAYS: i64 = 7;

/// The icon of a package kept in the icon cache from an earlier run.
fn other_icon_on_disk(id: &str) -> Option<std::path::PathBuf> {
    let stem = crate::sources::icons::file_stem(id);
    ["png", "svg", "webp", "jpg", "gif", "ico"].iter().map(|e| paths::icons().join(format!("{stem}.{e}"))).find(|p| p.metadata().is_ok_and(|m| m.len() > 0))
}

struct JobRec {
    job: Job,
    req: JobRequest,
    ctl: watch::Sender<Control>,
    active: bool,
    /// The plan's folders and exe names that must be closed (set while the job waits for them),
    /// so "Close and continue" closes exactly those and not another copy of the app.
    block: (Vec<PathBuf>, Vec<String>),
    /// Queue order (installs that wait for their turn go in this order).
    seq: u64,
    phase: Phase,
    /// Restored after a restart: waits until the app knows what is installed again.
    held: bool,
    /// Where its downloads go, so a restart keeps their partial files.
    dests: Vec<PathBuf>,
    /// How to tell after a restart whether an install that was cut off finished.
    verify: Option<SavedVerify>,
}

/// Where a job is in its run. Only jobs that are downloading use a "downloads at once" slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum Phase {
    /// Not started.
    #[default]
    Queued,
    /// Planning and downloading.
    Download,
    /// Downloaded and checked; waiting for its turn or for the app to close. Nothing ran yet.
    Ready,
    /// Install steps are running.
    Running,
    /// Checking the result.
    Verifying,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum SavedVerify {
    Version { app_id: String, version: String },
    Removed { app_id: String },
}

/// An unfinished job as saved in the queue file.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
struct SavedJob {
    job: Job,
    req: JobRequest,
    #[serde(default)]
    phase: Phase,
    #[serde(default)]
    dests: Vec<PathBuf>,
    #[serde(default)]
    verify: Option<SavedVerify>,
}

/// What a restart does with a saved job.
#[derive(Debug, PartialEq, Eq)]
enum Restore {
    /// Queue it again in this state (Waiting or Paused). Its download continues where it was.
    Queue(JobState),
    /// It was installing when the app or PC stopped: look at the installed version first.
    Check,
}

fn restore_kind(state: JobState, phase: Phase) -> Restore {
    match phase {
        Phase::Running | Phase::Verifying => Restore::Check,
        _ if state == JobState::Paused => Restore::Queue(JobState::Paused),
        _ => Restore::Queue(JobState::Waiting),
    }
}

/// The installer of an interrupted job finished after all, by what the scan finds now.
fn finished_anyway(v: Option<&SavedVerify>, installed: &[Installed]) -> bool {
    match v {
        Some(SavedVerify::Version { app_id, version: want }) => installed.iter().any(|i| &i.id == app_id && !i.missing && !version::newer(want, &i.version)),
        Some(SavedVerify::Removed { app_id }) => !installed.iter().any(|i| &i.id == app_id),
        None => false,
    }
}

fn saved_verify(plan: &Plan) -> Option<SavedVerify> {
    if plan.kind == JobKind::Repair {
        // The version does not change: nothing tells whether it finished.
        return None;
    }
    match &plan.verify {
        Verify::Version { app_id, version } | Verify::Wait { app_id, version, .. } => Some(SavedVerify::Version { app_id: app_id.clone(), version: version.clone() }),
        Verify::Removed { app_id } => Some(SavedVerify::Removed { app_id: app_id.clone() }),
        Verify::None => None,
    }
}

/// Saved jobs: unfinished ones only.
fn queue_file() -> PathBuf {
    paths::data_dir().join("queue.json")
}

fn save_queue_to(file: &std::path::Path, jobs: &[SavedJob]) {
    let tmp = file.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec(jobs).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, file);
    }
}

fn load_queue_from(file: &std::path::Path) -> Vec<SavedJob> {
    std::fs::read(file).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// The jobs a free "downloads at once" slot goes to: waiting jobs in queue order, while fewer
/// than `limit` jobs are downloading. Jobs that finished downloading and wait to install do not
/// hold a slot. Each job is (running, phase, state, held).
fn pick_to_start(jobs: &[(bool, Phase, JobState, bool)], limit: usize) -> Vec<usize> {
    let downloading = jobs.iter().filter(|(active, phase, _, _)| *active && matches!(phase, Phase::Queued | Phase::Download)).count();
    let mut picked = Vec::new();
    for (k, (active, _, state, held)) in jobs.iter().enumerate() {
        if downloading + picked.len() >= limit {
            break;
        }
        if !active && !held && *state == JobState::Waiting {
            picked.push(k);
        }
    }
    picked
}

/// One installer at a time. The jobs waiting get their turn in queue order, not in the order
/// their downloads happened to finish.
struct TurnGate {
    st: Mutex<GateSt>,
    notify: Notify,
}

#[derive(Default)]
struct GateSt {
    /// Job id and app name of the job installing now.
    holder: Option<(String, String)>,
    /// Queue position and job id of the jobs waiting.
    waiting: Vec<(u64, String)>,
}

struct Turn<'a> {
    gate: &'a TurnGate,
}

impl Drop for Turn<'_> {
    fn drop(&mut self) {
        self.gate.st.lock().holder = None;
        self.gate.notify.notify_waiters();
    }
}

impl TurnGate {
    fn new() -> Self {
        TurnGate { st: Mutex::new(GateSt::default()), notify: Notify::new() }
    }
    /// The app installing now.
    fn holder(&self) -> Option<String> {
        self.st.lock().holder.as_ref().map(|h| h.1.clone())
    }
    /// Wait for this job's turn; None when the job is canceled meanwhile.
    async fn take(&self, id: &str, name: &str, seq: u64, mut ctl: watch::Receiver<Control>) -> Option<Turn<'_>> {
        self.st.lock().waiting.push((seq, id.to_string()));
        loop {
            let notified = self.notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut st = self.st.lock();
                let first = st.waiting.iter().min_by_key(|w| w.0).map(|w| w.1.clone());
                if st.holder.is_none() && first.as_deref() == Some(id) {
                    st.waiting.retain(|w| w.1 != id);
                    st.holder = Some((id.to_string(), name.to_string()));
                    return Some(Turn { gate: self });
                }
            }
            if *ctl.borrow_and_update() == Control::Cancel {
                self.st.lock().waiting.retain(|w| w.1 != id);
                self.notify.notify_waiters();
                return None;
            }
            tokio::select! {
                _ = &mut notified => {}
                r = ctl.changed() => {
                    if r.is_err() {
                        notified.await;
                    }
                }
            }
        }
    }
}

/// Locks by name (an app, a folder): two jobs that write the same place take turns.
#[derive(Default)]
struct KeyLocks {
    m: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl KeyLocks {
    /// Take every lock in `keys` (in sorted order, so two jobs never wait for each other).
    async fn take(&self, keys: &[String], mut ctl: watch::Receiver<Control>) -> Option<Vec<tokio::sync::OwnedMutexGuard<()>>> {
        let mut keys = keys.to_vec();
        keys.sort();
        keys.dedup();
        let mut held = Vec::new();
        for k in keys {
            let m = self.m.lock().entry(k).or_default().clone();
            let canceled = async {
                loop {
                    if *ctl.borrow_and_update() == Control::Cancel {
                        return;
                    }
                    if ctl.changed().await.is_err() {
                        std::future::pending::<()>().await;
                    }
                }
            };
            tokio::select! {
                g = m.lock_owned() => held.push(g),
                _ = canceled => return None,
            }
        }
        Some(held)
    }
}

/// What an install step does, for the job's detail line. `files` is the number of files in a
/// zip being unpacked.
fn step_detail(step: &sources::Step, files: Option<usize>) -> String {
    use sources::Step;
    match step {
        Step::Run { exe, args, label, .. } => {
            let file = exe.file_name().map(|f| f.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let low = label.to_ascii_lowercase();
            let uninstall = low.contains("uninstall") || low.contains("removing") || file.contains("uninst") || args.iter().any(|a| a.eq_ignore_ascii_case("/x"));
            if file.contains("remoteupdatemanager") {
                "Adobe Remote Update Manager is working".into()
            } else if file == "msiexec.exe" {
                if uninstall { "Windows Installer is removing it silently".into() } else { "Windows Installer is running silently".into() }
            } else if uninstall {
                "Running the uninstaller silently".into()
            } else {
                "Running the installer silently".into()
            }
        }
        Step::ExtractZip { .. } => match files {
            Some(n) => format!("Unpacking {} file{}", thousands(n as u64), if n == 1 { "" } else { "s" }),
            None => "Unpacking".into(),
        },
        Step::RemoveDir { path, .. } => format!("Deleting the folder {}", path.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default()),
        Step::RemoveIfOnlyUninstaller { .. } => "Removing the empty app folder".into(),
        Step::WriteFile { .. } => "Writing a settings file".into(),
        Step::Shortcut { lnk, .. } => {
            if lnk.to_string_lossy().to_ascii_lowercase().contains("start menu") {
                "Adding the Start menu shortcut".into()
            } else {
                "Adding a shortcut".into()
            }
        }
        Step::RemoveFile { .. } => "Removing a file".into(),
        Step::OpenUri { uri, .. } => format!("Handing over to {}", uri.split(':').next().unwrap_or("another app")),
        Step::RemoveUserRegKey { .. } => "Removing its registry entries".into(),
        Step::CheckSigner { signer, .. } => format!("Checking the signature by {signer}"),
    }
}

/// 1234567 -> "1,234,567"
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (k, c) in s.chars().enumerate() {
        if k > 0 && (s.len() - k) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Files (not folders) in a zip, read from its directory.
fn zip_files(p: &std::path::Path) -> Option<usize> {
    let f = std::fs::File::open(p).ok()?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f)).ok()?;
    let mut n = 0;
    for i in 0..z.len() {
        if z.by_index_raw(i).ok().is_some_and(|e| !e.is_dir()) {
            n += 1;
        }
    }
    Some(n)
}

/// The processes "Close and continue" closes: only the ones that block this job.
fn close_targets(list: &[procs::Proc], dirs: &[PathBuf], names: &[String]) -> Vec<u32> {
    procs::blocking(list, dirs, names).iter().map(|p| p.pid).collect()
}

#[derive(Default)]
struct St {
    installed: Vec<Installed>,
    latest: HashMap<String, Latest>,
    icons: HashMap<String, String>,
    failed: HashMap<String, JobError>,
    sizes: HashMap<String, u64>,
    /// "<app id>@<version>" -> download size of that update (0: the server did not say)
    dl_sizes: HashMap<String, u64>,
    /// Catalog id -> local icon URL (Adobe CDN and getartcraft.com icons, cached on disk)
    catalog_icons: HashMap<String, String>,
    /// Other apps ids ("other:<package>") whose icon is being searched for right now
    icon_busy: HashSet<String>,
    status: Option<Status>,
    /// Sources whose latest-version lookup failed on the last check.
    unknown_sources: HashSet<&'static str>,
}

pub struct Core {
    pub app: AppHandle,
    pub settings: RwLock<Settings>,
    st: Mutex<St>,
    pub adobe: RwLock<Option<Arc<adobe::Catalog>>>,
    pub craft: RwLock<Option<Arc<craft::Feed>>>,
    pub blender: RwLock<Option<Arc<blender::Feed>>>,
    pub tools: RwLock<Option<Arc<tools::Feed>>>,
    pub other: RwLock<Option<Arc<other::Feed>>>,
    jobs: Mutex<Vec<JobRec>>,
    pub elevator: Elevator,
    pub throttle: Arc<Throttle>,
    wake: Notify,
    install_gate: TurnGate,
    /// Jobs that write the same app or folder take turns, installers or not.
    write_locks: KeyLocks,
    /// A screen the tray asked the main window to show; read once by the window.
    pub pending_route: Mutex<Option<String>>,
    next_seq: std::sync::atomic::AtomicU64,
    /// What the saved queue file holds, to write it only when that changes.
    queue_saved: Mutex<String>,
    check_lock: tokio::sync::Mutex<()>,
    /// A check was asked for while one ran: run once more when it ends
    check_again: std::sync::atomic::AtomicBool,
    pub persisted: Mutex<Persisted>,
    jobs_dirty: std::sync::atomic::AtomicBool,
    /// "SAP@version" -> download size from the product manifest (the catalog often lacks it)
    adobe_sizes: Mutex<HashMap<String, u64>>,
    pub self_release: Mutex<Option<crate::selfupdate::Release>>,
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub fn fmt_bytes(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b} B")
    }
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} s")
    } else if s < 3600 {
        format!("{} min {} s", s / 60, s % 60)
    } else {
        format!("{} h {} min", s / 3600, (s % 3600) / 60)
    }
}

impl Core {
    pub fn new(app: AppHandle) -> Arc<Self> {
        let settings = store::load_settings();
        let throttle = Arc::new(Throttle::new());
        throttle.set(if settings.bandwidth_limit_enabled { settings.bandwidth_limit } else { 0 });
        let persisted = store::load_state();
        let core = Arc::new(Core {
            app,
            settings: RwLock::new(settings),
            st: Mutex::new(St::default()),
            adobe: RwLock::new(None),
            craft: RwLock::new(None),
            blender: RwLock::new(None),
            tools: RwLock::new(None),
            other: RwLock::new(None),
            jobs: Mutex::new(Vec::new()),
            elevator: Elevator::new(),
            throttle,
            wake: Notify::new(),
            install_gate: TurnGate::new(),
            write_locks: KeyLocks::default(),
            pending_route: Mutex::new(None),
            next_seq: std::sync::atomic::AtomicU64::new(0),
            queue_saved: Mutex::new(String::new()),
            check_lock: tokio::sync::Mutex::new(()),
            check_again: std::sync::atomic::AtomicBool::new(false),
            persisted: Mutex::new(persisted),
            jobs_dirty: std::sync::atomic::AtomicBool::new(false),
            adobe_sizes: Mutex::new(HashMap::new()),
            self_release: Mutex::new(None),
        });
        let last = core.persisted.lock().last_check.clone();
        core.st.lock().status = Some(Status {
            online: true,
            checking: false,
            last_check: last,
            cc_running: false,
            cc_installed: false,
            signed_in: false,
            disks: vec![],
            cache_size: 0,
            scan_step: Some("Reading installed apps".into()),
            first_scan: true,
            app_version: env!("CARGO_PKG_VERSION").into(),
            self_update: None,
        });
        core.restore_queue();
        core
    }

    // ------------------------------------------------------------ the saved queue

    /// Bring back the jobs that were not finished when the app or the PC stopped. They wait until
    /// the first scan (`release_restored`); their downloads continue where they were. Partial
    /// downloads no job needs any more are deleted.
    fn restore_queue(&self) {
        let saved = load_queue_from(&queue_file());
        let mut keep: HashSet<PathBuf> = HashSet::new();
        let mut jobs = self.jobs.lock();
        for s in saved {
            if s.job.state.is_finished() || jobs.iter().any(|j| j.job.app_id == s.job.app_id) {
                continue;
            }
            keep.extend(s.dests.iter().cloned());
            let mut job = s.job;
            let (state, phase) = match restore_kind(job.state, s.phase) {
                Restore::Queue(st) => (st, Phase::Queued),
                Restore::Check => (JobState::Installing, s.phase),
            };
            job.state = state;
            job.step = match state {
                JobState::Paused => "Paused".into(),
                JobState::Waiting => "Waiting".into(),
                _ => "Checking whether it finished".into(),
            };
            job.detail = match state {
                JobState::Installing => "The app stopped while this was installing".into(),
                _ if job.downloaded > 0 => "Continues where it stopped".into(),
                _ => String::new(),
            };
            job.speed = 0.0;
            job.eta = None;
            job.blocked_by.clear();
            job.cancelable = false;
            let (tx, _) = watch::channel(if state == JobState::Paused { Control::Pause } else { Control::Run });
            let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
            jobs.push(JobRec { job, req: s.req, ctl: tx, active: false, block: Default::default(), seq, phase, held: true, dests: s.dests, verify: s.verify });
        }
        let n = jobs.len();
        drop(jobs);
        if n > 0 {
            log::info!("restored {n} unfinished jobs");
        }
        let freed = download::sweep(&paths::downloads(), &keep);
        if freed > 0 {
            log::info!("deleted {} of unfinished downloads no job needs", fmt_bytes(freed));
        }
    }

    /// After the first scan: jobs cut off while installing are checked against what is installed
    /// now (never run again by themselves), and the others start.
    fn release_restored(self: &Arc<Self>) {
        let installed = self.st.lock().installed.clone();
        let mut failed: Vec<(String, String)> = Vec::new();
        let mut done: Vec<(String, String)> = Vec::new();
        {
            let mut jobs = self.jobs.lock();
            for j in jobs.iter_mut().filter(|j| j.held) {
                j.held = false;
                if !matches!(j.phase, Phase::Running | Phase::Verifying) {
                    continue;
                }
                j.phase = Phase::Queued;
                j.job.detail.clear();
                j.job.finished_at = Some(now());
                if finished_anyway(j.verify.as_ref(), &installed) {
                    j.job.state = JobState::Done;
                    j.job.progress = 1.0;
                    j.job.step = "Done".into();
                    done.push((j.job.id.clone(), with_version(&j.job.name, &j.job.to_version)));
                } else {
                    failed.push((j.job.id.clone(), j.job.name.clone()));
                }
            }
        }
        for (id, name) in failed {
            self.fail(
                &id,
                "Interrupted",
                "The PC or the app stopped while it was installing. It may be partly installed. Retry to install it again".into(),
                format!("{name} was installing when the app or the PC stopped. The installed version shows it did not finish, so it was not run again by itself."),
                None,
            );
        }
        for (_, name) in done {
            self.activity(ActivityKind::Install, format!("{name} finished installing before the restart"), None, None, None);
        }
        self.mark_jobs_dirty();
        self.emit_apps();
        self.wake.notify_one();
    }

    /// Save the unfinished jobs when what is saved would change.
    fn save_queue(&self) {
        let list: Vec<SavedJob> = self
            .jobs
            .lock()
            .iter()
            .filter(|j| !j.job.state.is_finished())
            .map(|j| SavedJob { job: j.job.clone(), req: j.req.clone(), phase: j.phase, dests: j.dests.clone(), verify: j.verify.clone() })
            .collect();
        let sig: String = list.iter().map(|s| format!("{}:{:?}:{:?}:{}:{};", s.job.id, s.job.state, s.phase, s.dests.len(), s.job.to_version)).collect();
        let mut saved = self.queue_saved.lock();
        if *saved == sig {
            return;
        }
        save_queue_to(&queue_file(), &list);
        *saved = sig;
    }

    /// Saved at exit with the progress of each job.
    pub fn save_queue_now(&self) {
        self.queue_saved.lock().clear();
        self.save_queue();
    }

    // ------------------------------------------------------------ events

    fn emit<T: serde::Serialize + Clone>(&self, ev: &str, v: T) {
        let _ = self.app.emit(ev, v);
    }

    pub fn toast(&self, kind: &str, text: impl Into<String>, app_id: Option<String>) {
        self.emit("toast", Toast { kind: kind.into(), text: text.into(), app_id });
    }

    /// A Windows notification with a sound (Windows toasts are silent unless a sound is named).
    fn notify(&self, title: &str, body: &str) {
        self.notify_with(title, body, NOTIFY_SOUND);
    }

    fn notify_with(&self, title: &str, body: &str, sound: &str) {
        let _ = self.app.notification().builder().title(title).body(body).sound(sound).show();
    }

    fn activity(&self, kind: ActivityKind, title: impl Into<String>, meta: Option<String>, app_id: Option<String>, error: Option<JobError>) {
        let e = store::activity(kind, title, meta, app_id, error);
        self.emit("activity", e);
    }

    pub fn emit_apps(&self) {
        let apps = self.apps();
        let n = apps.iter().filter(|a| a.status == AppStatus::Update).count();
        self.emit("apps", apps);
        crate::tray::set_count(&self.app, n);
    }

    pub fn emit_status(&self) {
        let s = self.status();
        self.emit("status", s);
    }

    fn mark_jobs_dirty(&self) {
        self.jobs_dirty.store(true, Ordering::Relaxed);
    }

    /// Sends job progress at most 8 times a second.
    pub async fn jobs_ticker(self: Arc<Self>) {
        let mut t = tokio::time::interval(Duration::from_millis(125));
        loop {
            t.tick().await;
            if self.jobs_dirty.swap(false, Ordering::Relaxed) {
                let jobs = self.jobs();
                self.emit("jobs", jobs);
                // A crash or power cut must not lose the queue.
                self.save_queue();
            }
        }
    }

    // ------------------------------------------------------------ views

    pub fn status(&self) -> Status {
        self.st.lock().status.clone().unwrap()
    }

    fn set_status(&self, f: impl FnOnce(&mut Status)) {
        {
            let mut st = self.st.lock();
            if let Some(s) = st.status.as_mut() {
                f(s);
            }
        }
        self.emit_status();
    }

    pub fn jobs(&self) -> Vec<Job> {
        self.jobs.lock().iter().map(|j| j.job.clone()).collect()
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot { apps: self.apps(), jobs: self.jobs(), settings: self.settings.read().clone(), status: self.status() }
    }

    pub fn installed(&self, id: &str) -> Option<Installed> {
        self.st.lock().installed.iter().find(|i| i.id == id).cloned()
    }

    pub fn apps(&self) -> Vec<AppEntry> {
        let running = procs::list();
        let sizes = self.adobe_sizes.lock().clone();
        let st = self.st.lock();
        let jobs = self.jobs.lock();
        let mut out = Vec::new();
        for i in &st.installed {
            let job = jobs.iter().rev().find(|j| j.job.app_id == i.id && !j.job.state.is_finished()).map(|j| &j.job);
            let latest = st.latest.get(&i.id);
            let is_beta = i.channel.as_deref() == Some("beta");
            let has_update = !i.read_only
                && !i.missing
                && latest.is_some_and(|l| sources::update_offered(&l.version, &i.version, is_beta));
            let status = if i.missing {
                AppStatus::Missing
            } else if let Some(j) = job {
                match j.state {
                    JobState::Waiting => AppStatus::Queued,
                    JobState::Blocked => AppStatus::Blocked,
                    _ => AppStatus::Updating,
                }
            } else if st.failed.contains_key(&i.id) {
                AppStatus::Failed
            } else if has_update {
                AppStatus::Update
            } else if latest.is_none() && !i.read_only {
                AppStatus::Unknown
            } else {
                AppStatus::Current
            };
            let is_running = i.install_dir.as_ref().is_some_and(|d| !procs::blocking(&running, std::slice::from_ref(d), &[]).is_empty());
            let note = if i.missing {
                Some("Files are missing. Repair reinstalls it.".into())
            } else if i.read_only {
                i.tagline.clone()
            } else if has_update {
                latest.and_then(|l| l.note.clone())
            } else {
                None
            };
            let can_rollback = !i.read_only && !matches!(i.data, SourceData::Blender { kind: sources::BlenderKind::Beta, .. });
            out.push(AppEntry {
                id: i.id.clone(),
                source: i.source,
                name: i.name.clone(),
                code: i.code.clone(),
                fg: i.fg.clone(),
                bg: i.bg.clone(),
                icon_url: st.icons.get(&i.id).cloned(),
                version: i.version.clone(),
                latest: latest.map(|l| l.version.clone()),
                status,
                update_size: if has_update {
                    match &i.data {
                        SourceData::Adobe { sap, .. } => latest.and_then(|l| sizes.get(&format!("{sap}@{}", l.version)).copied().or(l.size)),
                        _ => latest.and_then(|l| l.size),
                    }
                } else {
                    None
                },
                size_on_disk: if i.missing { None } else { st.sizes.get(&i.id).copied().or(i.size_hint) },
                install_dir: i.install_dir.as_ref().map(|d| d.display().to_string()),
                updated_at: i.updated_at.clone(),
                tagline: i.tagline.clone(),
                group: i.source.group().into(),
                running: is_running,
                can_uninstall: !i.read_only && (i.uninstall.is_some() || matches!(i.data, SourceData::Blender { .. }) || matches!(&i.data, SourceData::Tools { key, .. } if key == "godot")),
                can_rollback,
                channel: i.channel.clone(),
                note,
                job_id: job.map(|j| j.id.clone()),
            });
        }
        out
    }

    // ------------------------------------------------------------ scanning and checking

    /// Read what is installed (no network) and recompute statuses.
    pub async fn rescan(self: &Arc<Self>) {
        let s = self.settings.read().clone();
        let cat = self.adobe.read().clone();
        let of = self.other.read().clone();
        let installed = tauri::async_runtime::spawn_blocking(move || {
            let un = winutil::uninstall_entries();
            let mut v = Vec::new();
            if s.sources.adobe {
                v.extend(adobe::scan(cat.as_deref(), &un));
            }
            if s.sources.craft {
                v.extend(craft::scan(&un));
            }
            if s.sources.blender {
                v.extend(blender::scan(&un, &PathBuf::from(&s.blender_install_dir)));
            }
            if s.sources.tools {
                v.extend(tools::scan(&un, &PathBuf::from(&s.godot_install_dir)));
            }
            if s.sources.other {
                let found = other::scan(&un, &v, of.as_deref());
                v.extend(found);
            }
            // Leftover registrations whose files are gone are not installed apps (Adobe kept
            // listing a deleted Photoshop). They show up in Get apps instead.
            v.retain(|i| !i.missing);
            v
        })
        .await
        .unwrap_or_default();
        self.recompute_latest(installed);
        self.emit_apps();
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.refresh_icons_and_sizes().await });
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.fill_download_sizes().await });
    }

    /// Download sizes of Other apps updates (the catalog does not list them): asked from the
    /// download server once per version, so "Update all" shows the real total.
    async fn fill_download_sizes(self: Arc<Self>) {
        let Some(feed) = self.other.read().clone() else { return };
        let wanted: Vec<(String, String)> = {
            let st = self.st.lock();
            other::update_urls(&st.installed, &feed).into_iter().filter(|(key, _)| !st.dl_sizes.contains_key(key)).collect()
        };
        if wanted.is_empty() {
            return;
        }
        use futures_util::StreamExt;
        let got: Vec<(String, Option<u64>)> = futures_util::stream::iter(wanted)
            .map(|(key, url)| async move { (key, other::download_size(&url).await) })
            .buffer_unordered(8)
            .collect()
            .await;
        let installed = {
            let mut st = self.st.lock();
            for (key, size) in got {
                // 0 marks "the server did not say", so it is not asked again this run.
                st.dl_sizes.insert(key, size.unwrap_or(0));
            }
            st.installed.clone()
        };
        self.recompute_latest(installed);
        self.emit_apps();
    }

    fn recompute_latest(&self, installed: Vec<Installed>) {
        let cat = self.adobe.read().clone();
        let cf = self.craft.read().clone();
        let bf = self.blender.read().clone();
        let tf = self.tools.read().clone();
        let of = self.other.read().clone();
        let keep_patch = self.settings.read().blender_keep_patch_versions;
        let mut st = self.st.lock();
        let mut latest = HashMap::new();
        for i in &installed {
            let l = match i.source {
                Source::Adobe => cat.as_ref().and_then(|c| adobe::latest(i, &installed, c)),
                Source::Craft => cf.as_ref().and_then(|f| craft::latest(i, f)),
                Source::Blender => bf.as_ref().and_then(|f| blender::latest_kept(i, &installed, f, keep_patch)),
                Source::Tools => tf.as_ref().and_then(|f| tools::latest(i, &installed, f)),
                Source::Other => of.as_ref().and_then(|f| other::latest(i, f)),
            };
            if let Some(l) = l {
                latest.insert(i.id.clone(), with_download_size(l, &i.id, &st.dl_sizes));
            }
        }
        // Forget failures for apps that are now current or gone.
        let ids: HashSet<&str> = installed.iter().map(|i| i.id.as_str()).collect();
        st.failed.retain(|k, _| ids.contains(k.as_str()));
        st.latest = latest;
        st.installed = installed;
    }

    async fn refresh_icons_and_sizes(self: Arc<Self>) {
        let installed = self.st.lock().installed.clone();
        let mut changed = false;
        for i in &installed {
            if self.st.lock().icons.contains_key(&i.id) {
                continue;
            }
            let mut url = None;
            if let Some(remote) = &i.icon_remote {
                let name = format!("{}.png", sanitize(&format!("remote-{}", remote.rsplit('/').nth(2).unwrap_or(&i.id))));
                if let Some(p) = crate::http::cache_icon(remote, &name).await {
                    url = Some(paths::asset_url(&p));
                }
            }
            if url.is_none() {
                if let Some(exe) = i.exe.clone() {
                    let name = format!("{}.png", sanitize(&exe.to_string_lossy()));
                    let out = paths::icons().join(name);
                    let ok = out.exists() || {
                        let (e, o) = (exe.clone(), out.clone());
                        tauri::async_runtime::spawn_blocking(move || winutil::extract_icon_png(&e, &o)).await.unwrap_or(false)
                    };
                    if ok {
                        url = Some(paths::asset_url(&out));
                    }
                }
            }
            if let Some(u) = url {
                self.st.lock().icons.insert(i.id.clone(), u);
                changed = true;
            }
        }
        if changed {
            self.emit_apps();
        }
        // Sizes on disk: remembered per version, measured once (can take a moment for big apps).
        for i in &installed {
            let key = format!("{}@{}", i.id, i.version);
            if let Some(sz) = self.persisted.lock().sizes.get(&key).copied() {
                self.st.lock().sizes.insert(i.id.clone(), sz);
            }
        }
        self.emit_apps();
        let mut measured = false;
        for i in &installed {
            if self.st.lock().sizes.contains_key(&i.id) || i.missing {
                continue;
            }
            // Other apps report their size in the uninstall list; their folders can be huge
            // (a game launcher with its games), so they are not walked.
            if i.source == Source::Other && i.size_hint.is_some() {
                continue;
            }
            let Some(dir) = i.install_dir.clone() else { continue };
            let size = tauri::async_runtime::spawn_blocking(move || winutil::dir_size(&dir)).await.unwrap_or(0);
            if size > 0 {
                self.st.lock().sizes.insert(i.id.clone(), size);
                self.persisted.lock().sizes.insert(format!("{}@{}", i.id, i.version), size);
                measured = true;
            }
        }
        if measured {
            store::save_state(&self.persisted.lock());
            self.emit_apps();
        }
    }

    pub fn update_disk_status(&self) {
        let s = self.settings.read().clone();
        let mut roots: Vec<PathBuf> = vec![paths::downloads(), PathBuf::from(&s.adobe_install_dir), PathBuf::from(&s.blender_install_dir), PathBuf::from(&s.godot_install_dir), paths::program_files()];
        roots.dedup();
        let mut disks: Vec<DiskInfo> = Vec::new();
        for r in roots {
            let root = winutil::volume_root(&r);
            if disks.iter().any(|d| d.path == root) {
                continue;
            }
            if let Some((free, total)) = winutil::disk_space(&r) {
                disks.push(DiskInfo { path: root, free, total });
            }
        }
        let cache = winutil::dir_size(&paths::downloads());
        let procs = procs::list();
        let cc = adobe::cc_status(&procs);
        let mut st = self.st.lock();
        if let Some(status) = st.status.as_mut() {
            status.disks = disks;
            status.cache_size = cache;
            status.cc_installed = cc.installed;
            status.cc_running = cc.running;
            status.signed_in = cc.signed_in;
        }
    }

    /// Full check: refresh catalogs and feeds, rescan, notify about new updates. Asked for while
    /// a check runs (a source was just switched on), it runs again right after that one.
    pub async fn check_now(self: &Arc<Self>, manual: bool) -> Result<()> {
        coalesced(&self.check_lock, &self.check_again, || self.check_once(manual)).await.unwrap_or(Ok(()))
    }

    async fn check_once(self: &Arc<Self>, manual: bool) -> Result<()> {
        self.set_status(|s| {
            s.checking = true;
            if s.first_scan {
                s.scan_step = Some("Reading installed apps".into());
            }
        });
        // Show what is installed right away, versions follow.
        self.rescan().await;
        let online = crate::http::online().await;
        self.set_status(|s| {
            s.online = online;
            if s.first_scan {
                s.scan_step = Some("Checking for updates".into());
            }
        });
        let settings = self.settings.read().clone();
        let installed = self.st.lock().installed.clone();
        let mut unknown: HashSet<&'static str> = HashSet::new();

        let adobe_f = async {
            if !settings.sources.adobe {
                return None;
            }
            match adobe::fetch_catalog().await {
                Ok(c) => Some(Arc::new(c)),
                Err(e) => {
                    log::warn!("adobe catalog: {e:#}");
                    None
                }
            }
        };
        let craft_f = async {
            if !settings.sources.craft {
                return None;
            }
            match craft::fetch(settings.craft_prereleases).await {
                Ok(f) => Some(Arc::new(f)),
                Err(e) => {
                    log::warn!("craft feed: {e:#}");
                    None
                }
            }
        };
        let blender_f = async {
            if !settings.sources.blender {
                return None;
            }
            match blender::fetch(&installed, settings.blender_channel == "beta").await {
                Ok(f) => Some(Arc::new(f)),
                Err(e) => {
                    log::warn!("blender feed: {e:#}");
                    None
                }
            }
        };
        let tools_f = async {
            if !settings.sources.tools {
                return None;
            }
            match tools::fetch(&installed).await {
                Ok(f) => Some(Arc::new(f)),
                Err(e) => {
                    log::warn!("more apps feed: {e:#}");
                    None
                }
            }
        };
        let other_f = async {
            if !settings.sources.other {
                return None;
            }
            match other::fetch(&installed).await {
                Ok(f) => Some(Arc::new(f)),
                Err(e) => {
                    log::warn!("other apps: {e:#}");
                    None
                }
            }
        };
        let (a, c, b, t, o) = tokio::join!(adobe_f, craft_f, blender_f, tools_f, other_f);
        if a.is_some() {
            *self.adobe.write() = a;
        } else if settings.sources.adobe {
            unknown.insert("adobe");
        }
        if c.is_some() {
            *self.craft.write() = c;
        } else if settings.sources.craft {
            unknown.insert("craft");
        }
        if b.is_some() {
            *self.blender.write() = b;
        } else if settings.sources.blender {
            unknown.insert("blender");
        }
        if t.is_some() {
            *self.tools.write() = t;
        } else if settings.sources.tools {
            unknown.insert("tools");
        }
        if o.is_some() {
            *self.other.write() = o;
        } else if settings.sources.other {
            unknown.insert("other");
        }
        self.st.lock().unknown_sources = unknown.clone();
        self.rescan().await;
        {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.fill_adobe_sizes().await });
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.cache_catalog_icons().await });
            self.spawn_other_followups();
        }
        let me = self.clone();
        tauri::async_runtime::spawn_blocking(move || me.update_disk_status()).await.ok();

        let apps = self.apps();
        let updates: Vec<&AppEntry> = apps.iter().filter(|a| a.status == AppStatus::Update).collect();
        self.warm_versions(updates.iter().map(|a| a.id.clone()).collect());
        let n = updates.len();
        let checked = now();
        {
            let mut p = self.persisted.lock();
            p.last_check = Some(checked.clone());
            store::save_state(&p);
        }
        self.set_status(|s| {
            s.checking = false;
            s.last_check = Some(checked.clone());
            s.first_scan = false;
            s.scan_step = None;
        });
        let meta = if !unknown.is_empty() {
            let names: Vec<&str> = unknown.iter().map(|s| match *s { "adobe" => "Adobe", "craft" => "Craft apps", "tools" => "More apps", "other" => "Other apps", _ => "Blender" }).collect();
            Some(format!("Could not reach {}", names.join(", ")))
        } else {
            None
        };
        self.activity(
            ActivityKind::Check,
            match n {
                0 => "Checked for updates, everything is current".to_string(),
                1 => "Checked for updates, 1 found".to_string(),
                n => format!("Checked for updates, {n} found"),
            },
            meta,
            None,
            None,
        );
        if manual && !online {
            self.toast("warn", "You are offline. Showing what is installed.", None);
        }

        // Tell the user about updates they have not heard about yet.
        let fresh: Vec<&&AppEntry> = {
            let p = self.persisted.lock();
            updates.iter().filter(|a| p.notified.get(&a.id) != a.latest.as_ref()).collect()
        };
        if !fresh.is_empty() && settings.notify_updates && !manual {
            let fresh_names: Vec<String> = fresh.iter().map(|a| a.name.clone()).collect();
            let (title, body) = update_notice(updates.len(), &fresh_names);
            self.notify(&title, &body);
        }
        {
            let mut p = self.persisted.lock();
            for a in &updates {
                if let Some(l) = &a.latest {
                    p.notified.insert(a.id.clone(), l.clone());
                }
            }
            store::save_state(&p);
        }
        if !manual {
            self.auto_install(&apps);
        }
        if online && crate::selfupdate::enabled() {
            match crate::selfupdate::check().await {
                Ok(Some(r)) => {
                    let v = r.version.clone();
                    let first = self.self_release.lock().as_ref().map(|x| x.version.clone()) != Some(v.clone());
                    *self.self_release.lock() = Some(r);
                    self.set_status(|s| s.self_update = Some(v.clone()));
                    if first && settings.notify_updates {
                        self.notify(&format!("The Updater 3D {v} is ready"), "Open the app to update it.");
                    }
                }
                Ok(None) => {}
                Err(e) => log::warn!("self update check: {e:#}"),
            }
        }
        Ok(())
    }

    fn in_window(start: &str, end: &str) -> bool {
        let parse = |s: &str| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok();
        let (Some(a), Some(b)) = (parse(start), parse(end)) else { return true };
        let t = chrono::Local::now().time();
        if a <= b { t >= a && t < b } else { t >= a || t < b }
    }

    fn auto_install(self: &Arc<Self>, apps: &[AppEntry]) {
        let s = self.settings.read().clone();
        let mut queued = 0;
        for a in apps.iter().filter(|a| a.status == AppStatus::Update) {
            let mode = s.exceptions.get(&a.id).copied().unwrap_or(s.auto_mode);
            if mode != AutoMode::Install || !Self::in_window(&s.window_start, &s.window_end) || a.running {
                continue;
            }
            if self.enqueue(JobRequest::Update { app_id: a.id.clone(), version: None }).is_ok() {
                queued += 1;
            }
        }
        if queued > 0 {
            log::info!("auto-install queued {queued} updates");
        }
    }

    /// Background loop: periodic checks and the auto-install window.
    pub async fn scheduler(self: Arc<Self>) {
        self.preload().await;
        // The installed apps are known again: jobs from before a restart continue.
        self.release_restored();
        let _ = self.check_now(false).await;
        let mut tick = tokio::time::interval(Duration::from_secs(TICK_SECS));
        let mut n: u64 = 0;
        loop {
            tick.tick().await;
            n += 1;
            // Keep the admin helper while updates wait: a big download can take longer than its
            // idle time, and a new helper would need a new admin prompt in the middle of a batch.
            if n % (240 / TICK_SECS) == 0 && self.jobs.lock().iter().any(|j| !j.job.state.is_finished()) {
                self.elevator.keep_alive().await;
            }
            // "Running" flags and disk space: every minute, and every tick while Creative Cloud
            // is down so the warning goes away as soon as it starts (whatever the check interval).
            let cc_down = self.st.lock().status.as_ref().is_some_and(|s| s.cc_installed && !s.cc_running);
            if status_refresh_due(n, cc_down) {
                let me = self.clone();
                tauri::async_runtime::spawn_blocking(move || me.update_disk_status()).await.ok();
                self.emit_status();
            }
            if n % (60 / TICK_SECS) != 0 {
                continue;
            }
            let minute = n / (60 / TICK_SECS);
            // Pick up installs and updates made outside the app (no network needed).
            if minute % 5 == 0 && !self.jobs.lock().iter().any(|j| j.active) {
                self.rescan().await;
            }
            let hours = self.settings.read().check_interval;
            if hours == 0 {
                continue;
            }
            let due = self
                .persisted
                .lock()
                .last_check
                .as_deref()
                .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
                .is_none_or(|t| chrono::Utc::now().signed_duration_since(t).num_minutes() >= hours as i64 * 60);
            if due {
                let _ = self.check_now(false).await;
            }
        }
    }

    /// Icons for the Get apps list, downloaded once into the icon cache.
    async fn cache_catalog_icons(self: Arc<Self>) {
        let mut wanted: Vec<(String, String, String)> = Vec::new(); // id, url, file name
        if let Some(c) = self.adobe.read().clone() {
            let installed = self.st.lock().installed.clone();
            for e in adobe::catalog_entries(&c, &installed) {
                if let Some(u) = e.icon_url {
                    let sap = e.id.split(':').nth(1).unwrap_or("x").to_string();
                    wanted.push((e.id, u, format!("adobe-{sap}.png")));
                }
            }
        }
        for a in craft::APPS {
            let url = match a.key {
                "artcraft" | "artcraft-launcher" => "https://getartcraft.com/artcraft-icon.svg".to_string(),
                k => format!("https://getartcraft.com/images/apps/{k}/icon.webp"),
            };
            let ext = if url.ends_with(".svg") { "svg" } else { "webp" };
            wanted.push((format!("craft:{}", a.key), url, format!("craft-{}.{ext}", a.key)));
        }
        for a in tools::APPS {
            let ext = if a.icon.ends_with(".svg") { "svg" } else { "png" };
            wanted.push((format!("tools:{}", a.key), a.icon.to_string(), format!("tools-{}.{ext}", a.key)));
        }
        let mut have_now: Vec<(String, String)> = Vec::new();
        for (id, url, name) in wanted {
            if self.st.lock().catalog_icons.contains_key(&id) {
                continue;
            }
            // Short timeout: a blocked CDN must not hold up the others.
            let got = tokio::time::timeout(Duration::from_secs(8), crate::http::cache_icon(&url, &name)).await.ok().flatten();
            if let Some(p) = got {
                have_now.push((id, paths::asset_url(&p)));
            }
        }
        if !have_now.is_empty() {
            let mut st = self.st.lock();
            for (id, u) in have_now {
                st.catalog_icons.insert(id, u);
            }
            drop(st);
            self.emit_apps();
            self.emit("catalog_changed", ());
        }
    }

    /// Show the last known state right away from disk caches, before any network request.
    pub async fn preload(self: &Arc<Self>) {
        crate::http::PREFER_CACHE.store(true, Ordering::Relaxed);
        let settings = self.settings.read().clone();
        self.rescan().await;
        let installed = self.st.lock().installed.clone();
        let (a, c, b, t, o) = tokio::join!(
            async { if settings.sources.adobe { adobe::fetch_catalog().await.ok() } else { None } },
            async { if settings.sources.craft { craft::fetch(settings.craft_prereleases).await.ok() } else { None } },
            async { if settings.sources.blender { blender::fetch(&installed, settings.blender_channel == "beta").await.ok() } else { None } },
            async { if settings.sources.tools { tools::fetch(&installed).await.ok() } else { None } },
            async { if settings.sources.other { other::fetch(&installed).await.ok() } else { None } },
        );
        crate::http::PREFER_CACHE.store(false, Ordering::Relaxed);
        if let Some(a) = a {
            *self.adobe.write() = Some(Arc::new(a));
        }
        if let Some(c) = c {
            *self.craft.write() = Some(Arc::new(c));
        }
        if let Some(b) = b {
            *self.blender.write() = Some(Arc::new(b));
        }
        if let Some(t) = t {
            *self.tools.write() = Some(Arc::new(t));
        }
        if let Some(o) = o {
            *self.other.write() = Some(Arc::new(o));
        }
        self.rescan().await;
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.cache_catalog_icons().await });
        self.spawn_other_followups();
    }

    /// After the Other apps feed was read: look for the real icons, and finish reading the starter
    /// list when the first start could not read all of it in time.
    fn spawn_other_followups(self: &Arc<Self>) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            me.clone().cache_other_icons(None).await;
            me.finish_other().await;
        });
    }

    async fn finish_other(self: Arc<Self>) {
        for _ in 0..4 {
            if !self.other.read().as_ref().is_some_and(|f| f.unfinished) || !self.settings.read().sources.other {
                return;
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
            let installed = self.st.lock().installed.clone();
            if let Ok(f) = other::fetch(&installed).await {
                *self.other.write() = Some(Arc::new(f));
                self.emit("catalog_changed", ());
                self.clone().cache_other_icons(None).await;
            }
        }
    }

    /// Real icons for Other apps entries: the ones in the starter list and the installed ones the
    /// computer has no icon for (`wanted` None), or the given package ids (search results). Found
    /// icons are kept in the icon cache; apps without one are remembered for a week.
    async fn cache_other_icons(self: Arc<Self>, wanted: Option<Vec<String>>) {
        use futures_util::StreamExt;
        let feed = self.other.read().clone();
        let ids: Vec<String> = match wanted {
            Some(w) => w,
            None => match feed.as_deref() {
                Some(f) => f.popular.iter().map(|(p, _)| p.pkg.id.clone()).chain(f.pkgs.values().map(|p| p.pkg.id.clone())).collect(),
                None => return,
            },
        };
        let today = chrono::Local::now().date_naive();
        let misses = self.persisted.lock().icon_misses.clone();
        let recent_miss = |id: &str| misses.get(id).and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()).is_some_and(|d| (today - d).num_days() < ICON_RETRY_DAYS);
        let mut todo: Vec<String> = Vec::new();
        let mut from_disk = false;
        {
            let mut st = self.st.lock();
            for id in ids {
                let key = format!("other:{id}");
                if st.catalog_icons.contains_key(&key) || st.icons.contains_key(&key) || st.icon_busy.contains(&key) {
                    continue;
                }
                if let Some(p) = other_icon_on_disk(&id) {
                    st.catalog_icons.insert(key, paths::asset_url(&p));
                    from_disk = true;
                } else if !recent_miss(&id) {
                    st.icon_busy.insert(key);
                    todo.push(id);
                }
            }
        }
        if from_disk {
            self.emit_apps();
            self.emit("catalog_changed", ());
        }
        if todo.is_empty() {
            return;
        }
        let mut results = futures_util::stream::iter(todo.into_iter().map(|id| {
            let feed = feed.clone();
            async move {
                let got = tokio::time::timeout(Duration::from_secs(60), async {
                    let src = other::icon_source_of(feed.as_deref(), &id).await?;
                    let got = crate::sources::icons::find(&id, src.icon.as_deref(), &src.pages).await?;
                    let path = paths::icons().join(format!("{}.{}", crate::sources::icons::file_stem(&id), got.ext));
                    std::fs::write(&path, got.bytes).ok()?;
                    Some(path)
                })
                .await
                .ok()
                .flatten();
                (id, got)
            }
        }))
        .buffer_unordered(6);
        let (mut hits, mut unseen, mut missed): (usize, usize, Vec<String>) = (0, 0, vec![]);
        let mut last_emit = std::time::Instant::now();
        while let Some((id, got)) = results.next().await {
            let key = format!("other:{id}");
            let mut st = self.st.lock();
            st.icon_busy.remove(&key);
            match got {
                Some(p) => {
                    st.catalog_icons.insert(key, paths::asset_url(&p));
                    hits += 1;
                    unseen += 1;
                }
                None => missed.push(id),
            }
            drop(st);
            if unseen >= 12 || (unseen > 0 && last_emit.elapsed() > Duration::from_secs(3)) {
                self.emit_apps();
                self.emit("catalog_changed", ());
                unseen = 0;
                last_emit = std::time::Instant::now();
            }
        }
        if unseen > 0 {
            self.emit_apps();
            self.emit("catalog_changed", ());
        }
        // With no connection every search fails; that is no reason to skip these apps for a week.
        if !missed.is_empty() && (hits > 0 || crate::http::online().await) {
            let mut p = self.persisted.lock();
            for id in missed {
                p.icon_misses.insert(id, today.format("%Y-%m-%d").to_string());
            }
            store::save_state(&p);
        }
    }

    /// Exact download sizes for Adobe updates and the Get apps list, from product manifests.
    async fn fill_adobe_sizes(self: Arc<Self>) {
        let Some(cat) = self.adobe.read().clone() else { return };
        let lang = self.settings.read().language.clone();
        let mut wanted: Vec<adobe::Product> = Vec::new();
        {
            let st = self.st.lock();
            for i in &st.installed {
                if let (SourceData::Adobe { sap, platform, .. }, Some(l)) = (&i.data, st.latest.get(&i.id)) {
                    if let Some(p) = cat.find(sap, &l.version, platform) {
                        wanted.push(p.clone());
                    }
                }
            }
            for e in adobe::catalog_entries(&cat, &st.installed) {
                let sap = e.id.split(':').nth(1).unwrap_or("").to_string();
                if e.size.is_none() {
                    if let Some(p) = cat.find(&sap, &e.version, "win64") {
                        wanted.push(p.clone());
                    }
                }
            }
        }
        let mut changed = false;
        for p in wanted {
            let key = format!("{}@{}", p.sap, p.version);
            if self.adobe_sizes.lock().contains_key(&key) {
                continue;
            }
            if let Some(sz) = adobe::download_size(&p, &lang).await {
                self.adobe_sizes.lock().insert(key, sz);
                changed = true;
            }
        }
        if changed {
            self.emit_apps();
        }
    }

    // ------------------------------------------------------------ catalog and versions

    /// The Get apps list: what is not installed yet, or everything when the setting asks for it.
    /// `catalog()` stays complete for naming and labelling jobs.
    pub fn get_apps(&self) -> Vec<CatalogEntry> {
        let show_installed = self.settings.read().show_installed_in_get_apps;
        self.catalog().into_iter().filter(|e| show_installed || !e.installed).collect()
    }

    /// What "Also delete settings and add-ons" would remove for this app, with sizes. Reads the
    /// disk (folder sizes), so call it off the UI thread.
    pub fn prefs_preview(&self, app_id: &str) -> Result<Vec<crate::model::PrefsItem>> {
        let installed = self.st.lock().installed.clone();
        let inst = installed.iter().find(|i| i.id == app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
        let rest: Vec<sources::Installed> = installed.iter().filter(|i| i.id != inst.id).cloned().collect();
        Ok(crate::prefs::preview(&crate::prefs::steps(inst, &rest, &crate::prefs::Roots::system())))
    }

    /// Register (or remove) the scheduled task behind "Fewer admin prompts": one admin prompt.
    /// `automatic`: the app does it by itself (the setting is on but the task is missing); a "No"
    /// then switches the setting off with a note, so it is not asked again every start.
    pub fn set_up_silent_task(self: &Arc<Self>, enable: bool, automatic: bool) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let r: Result<()> = async {
                for step in crate::elevate::task_steps(enable)? {
                    let res = me.elevator.run(step, false).await?;
                    if !res.ok {
                        return Err(anyhow!("{} {}", res.message, res.output));
                    }
                }
                Ok(())
            }
            .await;
            // The prompt for the task is its own question, not a batch.
            me.elevator.reset_declined();
            match r {
                Ok(()) => me.toast("ok", if enable { "Updates now install without a prompt each time" } else { "Each install asks for admin rights again" }, None),
                Err(e) => {
                    let mut s = me.settings.write();
                    s.silent_elevation = !enable;
                    store::save_settings(&s);
                    let s2 = s.clone();
                    drop(s);
                    me.emit("settings", s2);
                    if automatic {
                        me.toast("info", "Each install asks for admin rights. Turn on Fewer admin prompts in Settings anytime.", None);
                    } else {
                        me.toast("err", format!("Could not change admin prompts: {e:#}"), None);
                    }
                }
            }
        });
    }

    /// When the window first opens: "Fewer admin prompts" is on but its task is not set up yet
    /// (a new install, or the task was removed): ask for the one admin prompt now, once per run.
    pub fn ensure_silent_task(self: &Arc<Self>) {
        static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        if !self.settings.read().silent_elevation || ASKED.swap(true, Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            // Also when the task starts another copy of the app (an older one cannot run the
            // steps this version sends): register it again for this exe.
            let current = tauri::async_runtime::spawn_blocking(crate::elevate::task_is_current).await.unwrap_or(true);
            if !current {
                me.set_up_silent_task(true, true);
            }
        });
    }

    /// One Get apps entry, installed or not (its page stays open after the install finishes).
    pub fn catalog_entry(&self, id: &str) -> Option<CatalogEntry> {
        self.catalog().into_iter().find(|e| e.id == id)
    }

    pub fn catalog(&self) -> Vec<CatalogEntry> {
        let st = self.st.lock();
        let s = self.settings.read().clone();
        let mut out = Vec::new();
        if s.sources.adobe {
            if let Some(c) = self.adobe.read().as_ref() {
                let sizes = self.adobe_sizes.lock();
                out.extend(adobe::catalog_entries(c, &st.installed).into_iter().map(|mut e| {
                    let sap = e.id.split(':').nth(1).unwrap_or("");
                    if let Some(sz) = sizes.get(&format!("{sap}@{}", e.version)) {
                        e.size = Some(*sz);
                    }
                    e
                }));
            }
        }
        if s.sources.craft {
            if let Some(f) = self.craft.read().as_ref() {
                out.extend(craft::catalog_entries(f, &st.installed));
            }
        }
        if s.sources.tools {
            if let Some(f) = self.tools.read().as_ref() {
                out.extend(tools::catalog_entries(f, &st.installed));
            }
        }
        if s.sources.other {
            if let Some(f) = self.other.read().as_ref() {
                out.extend(other::catalog_entries(f, &st.installed).into_iter().map(|mut e| {
                    e.icon_url = st.icons.get(&e.id).cloned();
                    e
                }));
            }
        }
        for e in &mut out {
            if let Some(u) = st.catalog_icons.get(&e.id) {
                e.icon_url = Some(u.clone());
            }
        }
        if s.sources.blender {
            if let Some(f) = self.blender.read().as_ref() {
                let mut v = blender::catalog_entries(f, &st.installed, s.blender_channel == "beta");
                // Reuse the real Blender icon from any installed copy.
                if let Some(icon) = st.installed.iter().filter(|i| i.source == Source::Blender).find_map(|i| st.icons.get(&i.id)) {
                    for e in &mut v {
                        e.icon_url = Some(icon.clone());
                    }
                }
                out.extend(v);
            }
        }
        out
    }

    /// Packages of the winget catalog for a search in Get apps (Other apps on only).
    pub async fn search_other(self: &Arc<Self>, q: String) -> Vec<CatalogEntry> {
        if !self.settings.read().sources.other {
            return vec![];
        }
        let installed = self.st.lock().installed.clone();
        let show_installed = self.settings.read().show_installed_in_get_apps;
        let found = tauri::async_runtime::spawn_blocking(move || other::search(&q, &installed, 40)).await.unwrap_or_default();
        // Like the rest of Get apps: installed apps only when the setting asks for them.
        let mut found: Vec<CatalogEntry> = found.into_iter().filter(|e| show_installed || !e.installed).collect();
        let missing: Vec<String> = {
            let st = self.st.lock();
            for e in &mut found {
                e.icon_url = st.catalog_icons.get(&e.id).or_else(|| st.icons.get(&e.id)).cloned();
            }
            found.iter().filter(|e| e.icon_url.is_none()).filter_map(|e| e.id.strip_prefix("other:").map(String::from)).collect()
        };
        if !missing.is_empty() {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.cache_other_icons(Some(missing)).await });
        }
        for e in &mut found {
            sources::browse::apply(e);
        }
        let ids: Vec<String> = found.iter().filter_map(|e| e.id.strip_prefix("other:").map(String::from)).collect();
        self.fetch_other_details(ids);
        found
    }

    /// One page of the whole winget catalog for Get apps, by category ("Browsers", ...) and first
    /// letter ("A" to "Z", "#"). Descriptions and icons the page lacks are looked for in the
    /// background; `other_details` and `catalog_changed` bring them.
    pub async fn browse_other(self: &Arc<Self>, category: Option<String>, letter: Option<String>, offset: usize, limit: usize) -> Result<sources::browse::BrowsePage> {
        use sources::browse;
        if !self.settings.read().sources.other {
            return Ok(browse::BrowsePage { total: 0, entries: vec![], packages: 0, categories: vec![] });
        }
        if !sources::winget::index_path().is_file() {
            sources::winget::refresh_index(Duration::from_secs(6 * 3600)).await?;
        }
        let installed = self.st.lock().installed.clone();
        let show_installed = self.settings.read().show_installed_in_get_apps;
        let (page, pkgs) = tauri::async_runtime::spawn_blocking(move || -> Result<(browse::BrowsePage, Vec<sources::winget::Pkg>)> {
            let list = browse::list()?;
            let here: HashSet<String> = installed.iter().filter(|i| i.source == Source::Other).map(|i| other::pkg_of(i).to_ascii_lowercase()).collect();
            let keep = |i: &browse::Item| show_installed || !here.contains(&i.pkg.id.to_ascii_lowercase());
            let cat = category.as_deref().and_then(browse::category_index);
            let letter = letter.and_then(|l| l.chars().next()).map(|c| c.to_ascii_uppercase());
            let (total, items) = browse::page(&list, cat, letter, offset, limit.clamp(1, 200), keep);
            let counts = browse::counts(&list, keep);
            let pkgs: Vec<sources::winget::Pkg> = items.iter().map(|i| i.pkg.clone()).collect();
            let entries = items
                .iter()
                .map(|i| {
                    let mut e = other::entry(&i.pkg, i.pkg.latest.clone(), Some(i.pkg.id.clone()), here.contains(&i.pkg.id.to_ascii_lowercase()));
                    browse::apply(&mut e);
                    e
                })
                .collect();
            let categories = browse::CATEGORIES.iter().zip(counts).map(|(name, count)| browse::CategoryCount { name, count }).collect();
            Ok((browse::BrowsePage { total, entries, packages: list.packages, categories }, pkgs))
        })
        .await
        .map_err(|e| anyhow!("{e}"))??;
        let mut page = page;
        let missing: Vec<String> = {
            let st = self.st.lock();
            for e in &mut page.entries {
                e.icon_url = st.catalog_icons.get(&e.id).or_else(|| st.icons.get(&e.id)).cloned();
            }
            page.entries.iter().filter(|e| e.icon_url.is_none()).filter_map(|e| e.id.strip_prefix("other:").map(String::from)).collect()
        };
        if !missing.is_empty() {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.cache_other_icons(Some(missing)).await });
        }
        let stale = browse::stale(&pkgs);
        if !stale.is_empty() {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { browse::fetch(stale, |p| me.emit("other_details", p)).await });
        }
        Ok(page)
    }

    /// A winget package's Get apps entry that is not in the starter list (a search result or a
    /// browsed one, opened on its own page).
    pub fn other_entry(self: &Arc<Self>, id: &str) -> Option<CatalogEntry> {
        let pkg_id = id.strip_prefix("other:")?;
        if !self.settings.read().sources.other {
            return None;
        }
        let pkg = sources::browse::pkgs_by_id(&[pkg_id.to_string()]).into_iter().next()?;
        let have = self.st.lock().installed.iter().any(|i| i.source == Source::Other && other::pkg_of(i).eq_ignore_ascii_case(pkg_id));
        let mut e = other::entry(&pkg, pkg.latest.clone(), Some(pkg.id.clone()), have);
        sources::browse::apply(&mut e);
        {
            let st = self.st.lock();
            e.icon_url = st.catalog_icons.get(&e.id).or_else(|| st.icons.get(&e.id)).cloned();
        }
        self.fetch_other_details(vec![pkg.id.clone()]);
        if e.icon_url.is_none() {
            let me = self.clone();
            let ids = vec![pkg.id.clone()];
            tauri::async_runtime::spawn(async move { me.cache_other_icons(Some(ids)).await });
        }
        Some(e)
    }

    /// Read the descriptions of these winget packages in the background, unless they are known.
    fn fetch_other_details(self: &Arc<Self>, ids: Vec<String>) {
        if ids.is_empty() {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let pkgs = tauri::async_runtime::spawn_blocking(move || sources::browse::stale(&sources::browse::pkgs_by_id(&ids))).await.unwrap_or_default();
            if !pkgs.is_empty() {
                sources::browse::fetch(pkgs, |p| me.emit("other_details", p)).await;
            }
        });
    }

    /// Get apps version list: the saved copy at once when there is one (refreshed in the
    /// background when it is a few minutes old), else fetched now and saved.
    pub async fn catalog_versions(self: &Arc<Self>, id: &str) -> Result<Vec<VersionInfo>> {
        let key = format!("get-{id}");
        if let Some((list, age)) = notes_cache::read(&key) {
            if age > notes_cache::FRESH {
                self.refresh_versions_later(key, id.to_string(), false);
            }
            return Ok(list);
        }
        let list = self.fetch_catalog_versions(id).await?;
        notes_cache::write(&key, &list);
        Ok(list)
    }

    /// Version history of an installed app, cached like `catalog_versions`.
    pub async fn versions(self: &Arc<Self>, app_id: &str) -> Result<Vec<VersionInfo>> {
        let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
        let key = format!("app-{app_id}");
        // A copy saved before an update marks the old version as installed: not usable.
        if let Some((list, age)) = notes_cache::read(&key).filter(|(l, _)| notes_cache::matches_installed(l, &inst.version)) {
            if age > notes_cache::FRESH {
                self.refresh_versions_later(key, app_id.to_string(), true);
            }
            return Ok(list);
        }
        let list = self.fetch_versions(app_id).await?;
        notes_cache::write(&key, &list);
        Ok(list)
    }

    /// Fetch a version list again in the background; tell the window when it changed.
    fn refresh_versions_later(self: &Arc<Self>, key: String, id: String, installed: bool) {
        if !notes_cache::start(&key) {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            let fresh = if installed { me.fetch_versions(&id).await } else { me.fetch_catalog_versions(&id).await };
            if let Ok(list) = fresh {
                let changed = notes_cache::read(&key).is_none_or(|(old, _)| old != list);
                notes_cache::write(&key, &list);
                if changed {
                    me.emit("versions-changed", serde_json::json!({ "id": id }));
                }
            }
            notes_cache::done(&key);
        });
    }

    /// After a check: load the version histories of apps that have an update, so they open at once.
    fn warm_versions(self: &Arc<Self>, app_ids: Vec<String>) {
        for id in app_ids {
            let key = format!("app-{id}");
            let stale = notes_cache::read(&key).is_none_or(|(_, age)| age > notes_cache::FRESH);
            if stale {
                self.refresh_versions_later(key, id, true);
            }
        }
    }

    async fn fetch_catalog_versions(&self, id: &str) -> Result<Vec<VersionInfo>> {
        let s = self.settings.read().clone();
        let mut parts = id.splitn(3, ':');
        match parts.next().unwrap_or("") {
            "adobe" => {
                let sap = parts.next().unwrap_or("");
                let cat = self.adobe.read().clone().ok_or_else(|| anyhow!("The Adobe catalog is not loaded yet"))?;
                Ok(with_adobe_notes(adobe::versions(sap, "win64", None, &cat)).await)
            }
            "craft" => craft::versions(parts.next().unwrap_or(""), None, s.craft_prereleases).await,
            "blender" => {
                let feed = self.blender.read().clone().ok_or_else(|| anyhow!("Blender releases are not loaded yet"))?;
                blender::catalog_versions(id, &feed).await
            }
            "tools" => {
                let feed = self.tools.read().clone().ok_or_else(|| anyhow!("The release lists are not loaded yet"))?;
                let installed = self.st.lock().installed.clone();
                tools::catalog_versions(parts.next().unwrap_or(""), &feed, &installed).await
            }
            "other" => {
                let feed = self.other.read().clone();
                let installed = self.st.lock().installed.clone();
                other::catalog_versions(id, feed.as_deref(), &installed).await
            }
            _ => Err(anyhow!("Unknown app {id}")),
        }
    }

    async fn fetch_versions(&self, app_id: &str) -> Result<Vec<VersionInfo>> {
        let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
        let s = self.settings.read().clone();
        match &inst.data {
            SourceData::Adobe { sap, platform, .. } => {
                let cat = self.adobe.read().clone().ok_or_else(|| anyhow!("The Adobe catalog is not loaded yet"))?;
                let list = adobe::versions(sap, platform, Some(&inst.version), &cat);
                Ok(with_adobe_notes(list).await)
            }
            SourceData::Craft { key, .. } => craft::versions(key, Some(&inst.version), s.craft_prereleases).await,
            SourceData::Blender { .. } => {
                let feed = self.blender.read().clone().ok_or_else(|| anyhow!("Blender releases are not loaded yet"))?;
                blender::versions(&inst, &feed).await
            }
            SourceData::Tools { .. } => {
                let feed = self.tools.read().clone().ok_or_else(|| anyhow!("The release lists are not loaded yet"))?;
                tools::versions(&inst, &feed).await
            }
            SourceData::Other { .. } => {
                let feed = self.other.read().clone().unwrap_or_default();
                other::versions(&inst, &feed).await
            }
        }
    }

    // ------------------------------------------------------------ jobs

    pub fn enqueue(self: &Arc<Self>, req: JobRequest) -> Result<String> {
        // A new batch (or a retry) may ask for admin rights again after an earlier "No".
        self.elevator.reset_declined();
        let (app_id, kind, to_version) = match &req {
            JobRequest::Update { app_id, version } => {
                let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
                if inst.read_only {
                    return Err(anyhow!("{} is updated by Steam", inst.name));
                }
                let latest = self.st.lock().latest.get(app_id).map(|l| l.version.clone());
                let target = version.clone().or(latest).unwrap_or_else(|| inst.version.clone());
                let kind = if version.is_some() && version::newer(&inst.version, &target) { JobKind::Rollback } else if version.is_some() && !version::newer(&target, &inst.version) && inst.channel.as_deref() != Some("beta") { JobKind::Install } else { JobKind::Update };
                (app_id.clone(), kind, target)
            }
            JobRequest::Install { id, version } => {
                let v = version.clone().or_else(|| self.catalog().into_iter().find(|c| &c.id == id).map(|c| c.version)).unwrap_or_default();
                (id.clone(), JobKind::Install, v)
            }
            JobRequest::Uninstall { app_id, .. } => {
                let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
                (app_id.clone(), JobKind::Uninstall, inst.version)
            }
            JobRequest::Repair { app_id } => {
                let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
                (app_id.clone(), JobKind::Repair, inst.version)
            }
        };
        // Gather everything before taking the jobs lock (lock order is st, then jobs).
        let catalog_entry = self.catalog().into_iter().find(|c| c.id == app_id);
        let (name, code, fg, bg, from) = match self.installed(&app_id) {
            Some(i) => (i.name, i.code, i.fg, i.bg, Some(i.version)),
            None => match &catalog_entry {
                Some(c) => (c.name.clone(), c.code.clone(), c.fg.clone(), c.bg.clone(), None),
                None => (app_id.clone(), "?".into(), "#EDEDED".into(), "#262626".into(), None),
            },
        };
        let icon = self.st.lock().icons.get(&app_id).cloned().or_else(|| catalog_entry.and_then(|c| c.icon_url));
        let mut jobs = self.jobs.lock();
        if let Some(j) = jobs.iter().find(|j| j.job.app_id == app_id && !j.job.state.is_finished()) {
            return Ok(j.job.id.clone());
        }
        let id = uuid::Uuid::new_v4().simple().to_string()[..12].to_string();
        let (tx, _) = watch::channel(Control::Run);
        jobs.push(JobRec {
            job: Job {
                id: id.clone(),
                app_id,
                name,
                code,
                fg,
                bg,
                icon_url: icon,
                kind,
                from_version: if kind == JobKind::Install { None } else { from },
                to_version,
                state: JobState::Waiting,
                downloaded: 0,
                total: 0,
                speed: 0.0,
                eta: None,
                progress: 0.0,
                step: "Waiting".into(),
                detail: String::new(),
                blocked_by: vec![],
                error: None,
                started_at: None,
                finished_at: None,
                cancelable: false,
            },
            req,
            ctl: tx,
            active: false,
            block: Default::default(),
            seq: self.next_seq.fetch_add(1, Ordering::Relaxed),
            phase: Phase::Queued,
            held: false,
            dests: vec![],
            verify: None,
        });
        drop(jobs);
        self.mark_jobs_dirty();
        self.emit_apps();
        self.wake.notify_one();
        Ok(id)
    }

    fn with_job<R>(&self, id: &str, f: impl FnOnce(&mut JobRec) -> R) -> Option<R> {
        let mut jobs = self.jobs.lock();
        let r = jobs.iter_mut().find(|j| j.job.id == id).map(f);
        drop(jobs);
        self.mark_jobs_dirty();
        r
    }

    pub fn pause(&self, id: &str) {
        self.with_job(id, |j| {
            if matches!(j.job.state, JobState::Downloading) {
                set_control(&j.ctl, Control::Pause);
            } else if j.job.state == JobState::Waiting {
                j.job.state = JobState::Paused;
                j.job.step = "Paused".into();
            }
        });
        self.emit_apps();
    }

    pub fn resume(&self, id: &str) {
        self.with_job(id, |j| {
            if j.job.state == JobState::Paused {
                set_control(&j.ctl, Control::Run);
                j.job.state = JobState::Waiting;
                j.job.step = "Waiting".into();
            }
        });
        self.wake.notify_one();
        self.emit_apps();
    }

    pub fn cancel(&self, id: &str) -> Result<()> {
        let r = self.with_job(id, |j| match j.job.state {
            JobState::Installing | JobState::Verifying if !j.job.cancelable => Err(anyhow!("{} is installing now and cannot be stopped safely", j.job.name)),
            JobState::Waiting | JobState::Paused if !j.active => {
                j.job.state = JobState::Canceled;
                j.job.step = "Canceled".into();
                j.job.finished_at = Some(now());
                Ok(())
            }
            _ => {
                set_control(&j.ctl, Control::Cancel);
                Ok(())
            }
        });
        self.emit_apps();
        r.unwrap_or(Ok(()))
    }

    pub fn retry(self: &Arc<Self>, id: &str) -> Result<()> {
        let req = self.with_job(id, |j| j.req.clone()).ok_or_else(|| anyhow!("That job is gone"))?;
        self.jobs.lock().retain(|j| j.job.id != id);
        self.st.lock().failed.remove(match &req {
            JobRequest::Update { app_id, .. } | JobRequest::Uninstall { app_id, .. } | JobRequest::Repair { app_id } => app_id,
            JobRequest::Install { id, .. } => id,
        });
        self.enqueue(req)?;
        Ok(())
    }

    pub fn reorder(&self, ids: &[String]) {
        let mut jobs = self.jobs.lock();
        let mut moved: Vec<JobRec> = Vec::new();
        for id in ids {
            if let Some(pos) = jobs.iter().position(|j| &j.job.id == id && j.job.state == JobState::Waiting && !j.active) {
                moved.push(jobs.remove(pos));
            }
        }
        // Waiting jobs go after the ones already running, in the new order.
        let insert_at = jobs.iter().rposition(|j| j.active || !matches!(j.job.state, JobState::Waiting | JobState::Paused)).map(|p| p + 1).unwrap_or(0);
        for (k, j) in moved.into_iter().enumerate() {
            let at = (insert_at + k).min(jobs.len());
            jobs.insert(at, j);
        }
        // Installs waiting for their turn follow the new order too.
        let mut seqs: Vec<u64> = jobs.iter().map(|j| j.seq).collect();
        seqs.sort_unstable();
        for (j, s) in jobs.iter_mut().zip(seqs) {
            j.seq = s;
        }
        drop(jobs);
        self.mark_jobs_dirty();
    }

    pub fn pause_all(&self) {
        let ids: Vec<String> = self.jobs.lock().iter().filter(|j| matches!(j.job.state, JobState::Downloading | JobState::Waiting)).map(|j| j.job.id.clone()).collect();
        for id in ids {
            self.pause(&id);
        }
    }

    pub fn resume_all(&self) {
        let ids: Vec<String> = self.jobs.lock().iter().filter(|j| j.job.state == JobState::Paused).map(|j| j.job.id.clone()).collect();
        for id in ids {
            self.resume(&id);
        }
    }

    pub fn clear_finished(&self) {
        self.jobs.lock().retain(|j| !j.job.state.is_finished());
        self.mark_jobs_dirty();
    }

    pub fn close_blockers(self: &Arc<Self>, id: &str) {
        let blockers = self.with_job(id, |j| j.job.blocked_by.clone()).unwrap_or_default();
        if blockers.is_empty() {
            return;
        }
        let me = self.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            let Some((dirs, names)) = me.with_job(&id, |j| j.block.clone()) else { return };
            if dirs.is_empty() && names.is_empty() {
                return;
            }
            let pids = close_targets(&procs::list_fresh(), &dirs, &names);
            procs::close(&pids, Duration::from_secs(12));
        });
    }

    /// Starts waiting jobs as slots free up.
    pub async fn queue_loop(self: Arc<Self>) {
        loop {
            let limit = self.settings.read().parallel_downloads.clamp(1, 4) as usize;
            let to_start: Vec<String> = {
                let mut jobs = self.jobs.lock();
                let view: Vec<(bool, Phase, JobState, bool)> = jobs.iter().map(|j| (j.active, j.phase, j.job.state, j.held)).collect();
                pick_to_start(&view, limit)
                    .into_iter()
                    .map(|k| {
                        let j = &mut jobs[k];
                        j.active = true;
                        j.phase = Phase::Download;
                        j.job.id.clone()
                    })
                    .collect()
            };
            for id in to_start {
                let me = self.clone();
                tauri::async_runtime::spawn(async move {
                    me.run_job(&id).await;
                    me.with_job(&id, |j| {
                        j.active = false;
                        j.phase = Phase::Queued;
                    });
                    me.wake.notify_one();
                });
            }
            let _ = tokio::time::timeout(Duration::from_secs(2), self.wake.notified()).await;
        }
    }

    fn set_job(&self, id: &str, f: impl FnOnce(&mut Job)) {
        self.with_job(id, |j| f(&mut j.job));
    }

    async fn make_plan(&self, req: &JobRequest) -> Result<Plan> {
        let s = self.settings.read().clone();
        let installed = self.st.lock().installed.clone();
        let update_like = match req {
            JobRequest::Update { app_id, version } => Some((app_id.clone(), version.clone(), false)),
            JobRequest::Repair { app_id } => Some((app_id.clone(), None, true)),
            _ => None,
        };
        match req {
            JobRequest::Update { .. } | JobRequest::Repair { .. } => {
                let (app_id, version, repair) = update_like.unwrap();
                let app_id = &app_id;
                let inst = installed.iter().find(|i| &i.id == app_id).cloned().ok_or_else(|| anyhow!("That app is no longer installed"))?;
                let latest = self.st.lock().latest.get(app_id).map(|l| l.version.clone());
                match &inst.data {
                    SourceData::Adobe { sap, platform, .. } => {
                        let cat = self.adobe.read().clone().ok_or_else(|| anyhow!("The Adobe catalog could not be loaded. Check your connection."))?;
                        let target = if repair { inst.version.clone() } else { version.clone().or(latest).unwrap_or_else(|| inst.version.clone()) };
                        let new_line = version::major(&target) != version::major(&inst.version);
                        let root = inst.install_dir.as_ref().and_then(|d| d.parent().map(|p| p.to_path_buf())).unwrap_or_else(|| PathBuf::from(&s.adobe_install_dir));
                        let kind = if repair { JobKind::Repair } else if version::newer(&inst.version, &target) { JobKind::Rollback } else { JobKind::Update };
                        adobe::plan_install(
                            adobe::InstallRequest {
                                sap,
                                version: &target,
                                platform,
                                kind,
                                app_id: app_id.clone(),
                                name: inst.name.clone(),
                                current: if new_line { None } else { Some(&inst) },
                                remove_after: if new_line && !s.keep_previous_versions && !repair { Some(&inst) } else { None },
                                lang: &s.language,
                                install_root: root,
                            },
                            &cat,
                        )
                        .await
                    }
                    SourceData::Craft { key, .. } => {
                        let feed = self.craft.read().clone();
                        let kind = if repair { JobKind::Repair } else if version.as_ref().is_some_and(|v| version::newer(&inst.version, v)) { JobKind::Rollback } else { JobKind::Update };
                        let v = if repair { Some(inst.version.clone()) } else { version.clone() };
                        craft::plan_install(key, v.as_deref(), kind, Some(&inst), feed.as_deref(), s.craft_prereleases).await
                    }
                    SourceData::Tools { .. } => {
                        let feed = self.tools.read().clone().ok_or_else(|| anyhow!("The release lists could not be loaded. Check your connection."))?;
                        let kind = if repair { JobKind::Repair } else if version.as_ref().is_some_and(|v| tools::cmp_ver(&inst.version, v).is_gt()) { JobKind::Rollback } else { JobKind::Update };
                        let target = if repair { Some(inst.version.clone()) } else { version.clone().or(latest) };
                        tools::plan_install(&inst.id, target.as_deref(), kind, Some(&inst), &installed, &feed, tools::Ctx { godot_root: std::path::Path::new(&s.godot_install_dir) }).await
                    }
                    SourceData::Other { .. } => {
                        let feed = self.other.read().clone();
                        let kind = if repair { JobKind::Repair } else if version.as_ref().is_some_and(|v| tools::cmp_ver(&inst.version, v).is_gt()) { JobKind::Rollback } else { JobKind::Update };
                        other::plan_install(&inst.id, if repair { None } else { version.as_deref() }, kind, Some(&inst), feed.as_deref()).await
                    }
                    SourceData::Blender { .. } => {
                        if repair {
                            return Err(anyhow!("Repair is not available for Blender. Install the same version again from its page."));
                        }
                        let feed = self.blender.read().clone().ok_or_else(|| anyhow!("Blender releases could not be loaded. Check your connection."))?;
                        let target = version.clone().or(latest);
                        let kind = if target.as_ref().is_some_and(|v| version::newer(&inst.version, v)) { JobKind::Rollback } else { JobKind::Update };
                        let target = if inst.channel.as_deref() == Some("beta") { None } else { target };
                        blender::plan_install(
                            &inst.id,
                            target.as_deref(),
                            kind,
                            Some(&inst),
                            &feed,
                            blender::Ctx { root: std::path::Path::new(&s.blender_install_dir), keep_patch_versions: s.blender_keep_patch_versions, keep_previous: s.keep_previous_versions },
                        )
                        .await
                    }
                }
            }
            JobRequest::Install { id, version } => {
                let mut parts = id.splitn(3, ':');
                let src = parts.next().unwrap_or("");
                let key = parts.next().unwrap_or("").to_string();
                match src {
                    "adobe" => {
                        let cat = self.adobe.read().clone().ok_or_else(|| anyhow!("The Adobe catalog could not be loaded"))?;
                        let p = match version {
                            Some(v) => cat.find(&key, v, "win64").cloned(),
                            None => cat.newest(&key, "win64", None).cloned(),
                        }
                        .ok_or_else(|| anyhow!("Adobe does not offer that version"))?;
                        let name = self.catalog().into_iter().find(|c| &c.id == id).map(|c| c.name).unwrap_or_else(|| p.name.clone());
                        adobe::plan_install(
                            adobe::InstallRequest {
                                sap: &key,
                                version: &p.version,
                                platform: &p.platform,
                                kind: JobKind::Install,
                                app_id: id.clone(),
                                name,
                                current: None,
                                remove_after: None,
                                lang: &s.language,
                                install_root: PathBuf::from(&s.adobe_install_dir),
                            },
                            &cat,
                        )
                        .await
                    }
                    "craft" => {
                        let feed = self.craft.read().clone();
                        craft::plan_install(&key, version.as_deref(), JobKind::Install, None, feed.as_deref(), s.craft_prereleases).await
                    }
                    "blender" => {
                        let feed = self.blender.read().clone().ok_or_else(|| anyhow!("Blender releases could not be loaded"))?;
                        blender::plan_install(
                            id,
                            version.as_deref(),
                            JobKind::Install,
                            None,
                            &feed,
                            blender::Ctx { root: std::path::Path::new(&s.blender_install_dir), keep_patch_versions: s.blender_keep_patch_versions, keep_previous: s.keep_previous_versions },
                        )
                        .await
                    }
                    "tools" => {
                        let feed = self.tools.read().clone().ok_or_else(|| anyhow!("The release lists could not be loaded"))?;
                        tools::plan_install(id, version.as_deref(), JobKind::Install, None, &installed, &feed, tools::Ctx { godot_root: std::path::Path::new(&s.godot_install_dir) }).await
                    }
                    "other" => {
                        let feed = self.other.read().clone();
                        other::plan_install(id, version.as_deref(), JobKind::Install, None, feed.as_deref()).await
                    }
                    _ => Err(anyhow!("Unknown app {id}")),
                }
            }
            JobRequest::Uninstall { app_id, remove_prefs } => {
                let inst = installed.iter().find(|i| &i.id == app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
                let mut plan = match inst.source {
                    Source::Adobe => adobe::plan_uninstall(inst),
                    Source::Craft => craft::plan_uninstall(inst),
                    Source::Blender => blender::plan_uninstall(inst),
                    Source::Tools => tools::plan_uninstall(inst),
                    Source::Other => other::plan_uninstall(inst),
                }?;
                // Whatever the vendor's uninstaller leaves in the app's own folder goes too. Other
                // apps: nothing is known about what they keep there, so their folder only goes when
                // nothing but the uninstaller is left (an NSIS uninstaller run in place can not
                // delete itself).
                let others: Vec<PathBuf> = installed.iter().filter(|i| i.id != inst.id).filter_map(|i| i.install_dir.clone()).collect();
                if let Some(dir) = inst.install_dir.as_deref().filter(|_| inst.source == Source::Other).and_then(|d| leftover_dir(d, &others)) {
                    plan.steps.push(sources::Step::RemoveIfOnlyUninstaller { elevated: under_program_files(&dir), path: dir, label: format!("Removing what is left of {}", inst.name) });
                }
                let removed_already = |d: &std::path::Path| plan.steps.iter().any(|s| matches!(s, sources::Step::RemoveDir { path, .. } if path.as_path() == d));
                if let Some(dir) = inst.install_dir.as_deref().filter(|_| inst.source != Source::Other).and_then(|d| leftover_dir(d, &others)).filter(|d| !removed_already(d)) {
                    plan.steps.push(sources::Step::RemoveDir { elevated: under_program_files(&dir), path: dir, label: format!("Removing what is left of {}", inst.name) });
                }
                if *remove_prefs {
                    let rest: Vec<sources::Installed> = installed.iter().filter(|i| i.id != inst.id).cloned().collect();
                    plan.steps.extend(crate::prefs::steps(inst, &rest, &crate::prefs::Roots::system()));
                }
                Ok(plan)
            }
        }
    }

    fn fail(&self, id: &str, code: &str, message: String, detail: String, log_path: Option<String>) {
        let err = JobError { code: code.into(), message, detail, log_path };
        let mut app_id = String::new();
        let mut title = String::new();
        self.set_job(id, |j| {
            j.state = JobState::Failed;
            j.step = err.message.clone();
            j.detail.clear();
            j.error = Some(err.clone());
            j.speed = 0.0;
            j.eta = None;
            j.finished_at = Some(now());
            app_id = j.app_id.clone();
            title = format!("{} {} failed", with_version(&j.name, &j.to_version), j.kind.verb().to_lowercase());
        });
        self.st.lock().failed.insert(app_id.clone(), err.clone());
        self.activity(ActivityKind::Failure, title.clone(), Some(err.code.clone()), Some(app_id.clone()), Some(err.clone()));
        if self.settings.read().notify_failed {
            self.notify_with(&title, &err.message, FAILED_SOUND);
        }
        self.emit_apps();
    }

    async fn run_job(self: &Arc<Self>, id: &str) {
        let Some(req) = self.with_job(id, |j| j.req.clone()) else { return };
        let started = Instant::now();
        self.set_job(id, |j| {
            j.state = JobState::Downloading;
            j.step = "Preparing".into();
            j.started_at.get_or_insert_with(now);
            j.error = None;
        });
        self.emit_apps();
        let plan = match self.make_plan(&req).await {
            Ok(p) => p,
            Err(e) => {
                self.fail(id, "Could not start", format!("{e:#}"), format!("{e:?}"), None);
                return;
            }
        };
        let dests: Vec<PathBuf> = plan.downloads.iter().chain(plan.fallback.iter().flat_map(|f| f.downloads.iter())).map(|d| d.dest.clone()).collect();
        let verify = saved_verify(&plan);
        self.with_job(id, |j| {
            j.dests = dests;
            j.verify = verify;
        });
        self.set_job(id, |j| {
            j.to_version = plan.to_version.clone();
            // Other apps pick a package version, which can read differently from the version in the
            // uninstall list; the plan knows whether it is an update or a roll back.
            if plan.app_id.starts_with("other:") && j.kind != JobKind::Install {
                j.kind = plan.kind;
            }
        });
        let mut result = self.execute(id, &plan).await;
        if let Err(e) = &result {
            // Other apps try again with admin rights only when Windows would not start the
            // installer without them (error 740), never after an installer that ran.
            let retry = !plan.app_id.starts_with("other:") || other::needs_elevation(&format!("{e:#}"));
            if let (Some(fb), true) = (&plan.fallback, retry) {
                log::warn!("{}: {e:#}; trying fallback", plan.name);
                self.set_job(id, |j| {
                    j.state = JobState::Downloading;
                    j.step = "Trying another way".into();
                    j.downloaded = 0;
                    j.total = 0;
                });
                let first = format!("{e:#}");
                result = self.execute(id, fb).await.map_err(|e2| anyhow!("{e2:#}\n\nFirst attempt: {first}"));
            }
        }
        match result {
            Ok(Outcome::Done) => {
                let total = self.with_job(id, |j| j.job.total).unwrap_or(0);
                let s = self.settings.read().clone();
                if !s.keep_previous_versions || plan.app_id.starts_with("adobe:") {
                    for c in &plan.cleanup {
                        if c.is_dir() {
                            let _ = std::fs::remove_dir_all(c);
                        } else {
                            // The installer and what is noted about its download.
                            download::remove(c);
                        }
                    }
                }
                let mut name = String::new();
                let mut kind = JobKind::Update;
                let mut from = None;
                let mut app_id = String::new();
                self.set_job(id, |j| {
                    j.state = JobState::Done;
                    j.progress = 1.0;
                    j.speed = 0.0;
                    j.eta = None;
                    j.step = "Done".into();
                    j.detail.clear();
                    j.blocked_by.clear();
                    j.finished_at = Some(now());
                    name = j.name.clone();
                    kind = j.kind;
                    from = j.from_version.clone();
                    app_id = j.app_id.clone();
                });
                self.st.lock().failed.remove(&app_id);
                self.st.lock().sizes.remove(&app_id);
                let to = plan.to_version.clone();
                let (akind, title) = match kind {
                    JobKind::Update => (ActivityKind::Update, match &from { Some(f) => format!("Updated {} to {to}", with_version(&name, f)), None => format!("Updated {}", with_version(&name, &to)) }),
                    JobKind::Install => (ActivityKind::Install, format!("Installed {}", with_version(&name, &to))),
                    JobKind::Uninstall => (ActivityKind::Uninstall, format!("Uninstalled {}", with_version(&name, &to))),
                    JobKind::Rollback => (ActivityKind::Rollback, match &from { Some(f) => format!("Rolled back {} to {to}", with_version(&name, f)), None => format!("Installed {}", with_version(&name, &to)) }),
                    JobKind::Repair => (ActivityKind::Install, format!("Repaired {}", with_version(&name, &to))),
                };
                let meta = if total > 0 { format!("{}, {}", fmt_bytes(total), fmt_dur(started.elapsed())) } else { fmt_dur(started.elapsed()) };
                self.activity(akind, title.clone(), Some(meta), Some(app_id.clone()), None);
                if s.notify_finished {
                    let body = match kind {
                        JobKind::Uninstall => "Removed from this PC.",
                        _ => "Ready to use.",
                    };
                    self.notify(&title, body);
                }
                self.toast("ok", title, Some(app_id));
                self.emit_apps();
                let me = self.clone();
                tauri::async_runtime::spawn_blocking(move || me.update_disk_status()).await.ok();
                self.emit_status();
            }
            Ok(Outcome::Paused) => {
                self.set_job(id, |j| {
                    j.state = JobState::Paused;
                    j.step = "Paused".into();
                    j.detail = if j.downloaded > 0 { format!("{} kept, continues where it stopped", fmt_bytes(j.downloaded)) } else { String::new() };
                    j.speed = 0.0;
                    j.eta = None;
                });
                self.emit_apps();
            }
            Ok(Outcome::Canceled) => {
                self.set_job(id, |j| {
                    j.state = JobState::Canceled;
                    j.step = "Canceled".into();
                    j.detail.clear();
                    j.speed = 0.0;
                    j.eta = None;
                    j.finished_at = Some(now());
                });
                self.emit_apps();
            }
            Err(e) => {
                let text = format!("{e:#}");
                let (code, message, detail, log) = explain(&text, &plan);
                self.fail(id, &code, message, detail, log);
            }
        }
    }

    /// Download, wait for the app to close, install, verify.
    async fn execute(self: &Arc<Self>, id: &str, plan: &Plan) -> Result<Outcome> {
        let ctl = self.with_job(id, |j| j.ctl.subscribe()).ok_or_else(|| anyhow!("job vanished"))?;
        let total: u64 = plan.downloads.iter().filter_map(|d| d.size).sum();
        self.with_job(id, |j| j.phase = Phase::Download);
        self.set_job(id, |j| {
            j.total = total;
            j.state = JobState::Downloading;
            j.step = if plan.downloads.is_empty() { "Preparing".into() } else { "Downloading".into() };
            j.detail = if plan.downloads.is_empty() { String::new() } else { format!("Connecting to {}", crate::http::host(&plan.downloads[0].url)) };
        });

        // Split the bar by expected time: a Blender MSI downloads in seconds but installs for
        // minutes, an Adobe app the other way round. Learned install times beat the defaults.
        // An uninstall's time follows how much there is to remove.
        let est_bytes = if plan.kind == JobKind::Uninstall { self.installed(&plan.app_id).and_then(|i| i.size_hint).unwrap_or(0) } else { total };
        let (time_key, default_secs) = install_estimate(plan, est_bytes);
        let inst_est = self.persisted.lock().install_times.get(&time_key).copied().unwrap_or(default_secs).max(5.0);
        let left_to_fetch = total.saturating_sub(plan.downloads.iter().map(download::existing_bytes).sum::<u64>());
        let dl_est = left_to_fetch as f64 / (15.0 * 1024.0 * 1024.0);
        let dl_share = if plan.downloads.is_empty() { 0.0 } else { (dl_est / (dl_est + inst_est)).clamp(0.1, 0.9) };

        // Room on disk: downloads plus about the same again while installing.
        let have_already: u64 = plan.downloads.iter().map(download::existing_bytes).sum();
        let need = total.saturating_sub(have_already);
        if need > 0 {
            if let Some((free, _)) = winutil::disk_space(&paths::downloads()) {
                if free < need + 512 * 1024 * 1024 {
                    return Err(anyhow!("LOWDISK|Not enough space on {}: needs {}, {} free", winutil::volume_root(&paths::downloads()), fmt_bytes(need), fmt_bytes(free)));
                }
            }
        }

        // ---- downloads
        let mut completed = 0u64;
        let mut speed = 0.0f64;
        for d in &plan.downloads {
            let progress = Arc::new(Progress::new());
            let me = self.clone();
            let p2 = progress.clone();
            let jid = id.to_string();
            let base = completed;
            let share = dl_share;
            let ticker = tauri::async_runtime::spawn(async move {
                let mut last = (Instant::now(), p2.done.load(Ordering::Relaxed));
                let mut ema = 0.0f64;
                loop {
                    tokio::time::sleep(Duration::from_millis(250)).await;
                    let d = p2.done.load(Ordering::Relaxed);
                    let dt = last.0.elapsed().as_secs_f64();
                    let inst = (d.saturating_sub(last.1)) as f64 / dt.max(0.001);
                    ema = if ema == 0.0 { inst } else { ema * 0.8 + inst * 0.2 };
                    last = (Instant::now(), d);
                    let note = p2.note.lock().clone();
                    let detail = p2.detail.lock().clone();
                    me.set_job(&jid, |j| {
                        j.step = note.unwrap_or_else(|| "Downloading".into());
                        if let Some(dt) = detail {
                            j.detail = dt;
                        }
                        j.downloaded = base + d;
                        j.speed = ema;
                        let remaining = j.total.saturating_sub(j.downloaded) as f64;
                        j.eta = if ema > 1.0 { Some(remaining / ema) } else { None };
                        j.progress = if j.total > 0 { share * (j.downloaded as f64 / j.total as f64).min(1.0) } else { 0.0 };
                    });
                }
            });
            let r = download::fetch(d, progress.clone(), self.throttle.clone(), ctl.clone()).await;
            ticker.abort();
            if let Some(dt) = progress.detail.lock().clone() {
                self.set_job(id, |j| j.detail = dt);
            }
            let got = progress.done.load(Ordering::Relaxed);
            match r? {
                Outcome::Done => completed += d.size.unwrap_or(got).max(got),
                other => return Ok(other),
            }
            speed = speed.max(0.0);
        }
        let _ = speed;
        // Downloaded: the slot goes to the next download now.
        self.with_job(id, |j| j.phase = Phase::Ready);
        self.wake.notify_one();
        let installer = plan.runs_installer();
        self.set_job(id, |j| {
            j.downloaded = j.total.max(completed);
            j.total = j.total.max(completed);
            j.speed = 0.0;
            j.eta = None;
            j.progress = dl_share;
            j.state = JobState::Installing;
            j.step = if installer { "Waiting for another install to finish".into() } else { "Getting ready to unpack".into() };
            j.detail.clear();
            j.cancelable = true;
        });

        // ---- one installer at a time, in queue order; nothing has run yet, so Cancel still
        // works while waiting. Plans that only unpack and copy files do not wait for installers.
        let mut turn: Option<Turn<'_>> = None;
        if installer {
            let seq = self.with_job(id, |j| j.seq).unwrap_or(0);
            let wait = self.install_gate.take(id, &plan.name, seq, ctl.clone());
            tokio::pin!(wait);
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            let got = loop {
                tokio::select! {
                    t = &mut wait => break t,
                    _ = tick.tick() => {
                        let line = self.install_gate.holder().map(|h| format!("{h} is installing now")).unwrap_or_default();
                        self.set_job(id, |j| j.detail = line);
                    }
                }
            };
            match got {
                Some(t) => turn = Some(t),
                None => return Ok(Outcome::Canceled),
            }
        }
        // Never two jobs on the same app or folder at once.
        let keys = plan.write_keys();
        let Some(_writing) = self.write_locks.take(&keys, ctl.clone()).await else {
            return Ok(Outcome::Canceled);
        };
        self.set_job(id, |j| {
            j.cancelable = false;
            j.detail.clear();
        });

        // ---- the app must be closed
        let block = (plan.block_dirs.clone(), plan.block_names.clone());
        self.with_job(id, |j| j.block = block.clone());
        loop {
            if *ctl.borrow() == Control::Cancel {
                return Ok(Outcome::Canceled);
            }
            // Listing processes is slow, blocking work: keep it off the async threads.
            let (dirs, bnames) = block.clone();
            let b = tauri::async_runtime::spawn_blocking(move || procs::blocking(&procs::list(), &dirs, &bnames)).await.unwrap_or_default();
            if b.is_empty() {
                break;
            }
            let mut names: Vec<String> = b.iter().map(|p| procs::friendly(&p.name)).collect();
            names.sort();
            names.dedup();
            let changed = self.with_job(id, |j| {
                let was = j.job.state != JobState::Blocked;
                j.job.state = JobState::Blocked;
                j.job.blocked_by = names.clone();
                j.job.step = format!("Waiting for {} to close", names.join(", "));
                j.job.detail = "Downloaded and checked. It installs once that is closed".into();
                was
            });
            if changed == Some(true) {
                let me = self.clone();
                let _ = tauri::async_runtime::spawn_blocking(move || me.emit_apps()).await;
            }
            tokio::time::sleep(Duration::from_millis(1200)).await;
        }
        self.set_job(id, |j| {
            j.state = JobState::Installing;
            j.blocked_by.clear();
            j.detail.clear();
        });
        self.with_job(id, |j| {
            j.block = Default::default();
            j.phase = Phase::Running;
        });
        // Saved before anything runs: after a crash from here on, the installer is not run
        // again by itself.
        self.save_queue();
        let me = self.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || me.emit_apps()).await;

        // ---- install steps
        let silent = self.settings.read().silent_elevation;
        let n = plan.steps.len();
        let install_t0 = Instant::now();
        // Each step owns a share of the bar by how long that kind of step takes, so the bar
        // follows the steps: it cannot run ahead of a step that has not started yet.
        let weights: Vec<f64> = plan.steps.iter().map(step_weight).collect();
        let weight_sum: f64 = weights.iter().sum::<f64>().max(f64::EPSILON);
        for (k, step) in plan.steps.iter().enumerate() {
            let label = if n > 1 { format!("{}, step {} of {}", step.label(), k + 1, n) } else { step.label().to_string() };
            let files = match step {
                sources::Step::ExtractZip { zip, .. } => {
                    let z = zip.clone();
                    tauri::async_runtime::spawn_blocking(move || zip_files(&z)).await.ok().flatten()
                }
                _ => None,
            };
            let detail = step_detail(step, files);
            self.set_job(id, |j| {
                j.step = label.clone();
                j.detail = detail;
            });
            let step_t0 = Instant::now();
            let step_est = (inst_est * weights[k] / weight_sum).max(2.0);
            let weights = weights.clone();
            // Installers report no progress: move the bar along the expected install time, and
            // past it keep creeping instead of stopping, so it never looks frozen.
            let rum = rum_watch(step, &plan.to_version, &self.adobe_sizes.lock());
            let ticker = {
                let me = self.clone();
                let jid = id.to_string();
                let label = label.clone();
                tauri::async_runtime::spawn(async move {
                    // Remote Update Manager downloads by itself; its folder shows how far it is.
                    let mut last = (Instant::now(), 0u64);
                    let mut ema = 0.0f64;
                    let mut dl_done_at: Option<Instant> = None;
                    loop {
                        let s = step_t0.elapsed().as_secs_f64();
                        if let Some((dir, prefix, expected)) = &rum {
                            let got = folder_bytes(dir, prefix);
                            let dt = last.0.elapsed().as_secs_f64().max(0.001);
                            let inst = got.saturating_sub(last.1) as f64 / dt;
                            ema = if ema == 0.0 { inst } else { ema * 0.8 + inst * 0.2 };
                            last = (Instant::now(), got);
                            let frac = if *expected > 0 { (got as f64 / *expected as f64).min(1.0) } else { 0.0 };
                            if frac < 0.995 && dl_done_at.is_none() {
                                me.set_job(&jid, |j| {
                                    j.step = if got == 0 { format!("{label}, starting") } else { "Downloading from Adobe".into() };
                                    j.downloaded = got;
                                    j.total = *expected;
                                    j.speed = ema;
                                    j.eta = if ema > 1.0 { Some(expected.saturating_sub(got) as f64 / ema) } else { None };
                                    j.progress = 0.75 * frac;
                                });
                            } else {
                                let t0 = *dl_done_at.get_or_insert_with(Instant::now);
                                let s2 = t0.elapsed().as_secs_f64();
                                me.set_job(&jid, |j| {
                                    j.step = format!("Installing, {}:{:02}", s2 as u64 / 60, s2 as u64 % 60);
                                    j.downloaded = *expected;
                                    j.speed = 0.0;
                                    j.eta = None;
                                    j.progress = 0.75 + 0.24 * install_curve(s2 / 180.0);
                                });
                            }
                        } else {
                            me.set_job(&jid, |j| {
                                j.step = format!("{label}, {}:{:02}", s as u64 / 60, s as u64 % 60);
                                j.progress = dl_share + (1.0 - dl_share) * 0.98 * steps_progress(&weights, k, s / step_est);
                            });
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                })
            };
            // Downloaded files this step runs are checked again by the helper right before it
            // runs them, so a file swapped after the download check is not run with admin rights.
            let checks = crate::elevate::file_checks(step, &plan.downloads);
            let res = self.elevator.run_checked(step.clone(), silent, checks).await;
            ticker.abort();
            let res = res?;
            if !res.ok {
                let log = match step {
                    sources::Step::Run { log, .. } => log.clone(),
                    _ => None,
                };
                return Err(anyhow!("{}", step_failure(&res, step.label(), log.as_deref())));
            }
        }

        if !plan.steps.is_empty() {
            let took = install_t0.elapsed().as_secs_f64();
            let mut p = self.persisted.lock();
            let e = p.install_times.entry(time_key.clone()).or_insert(took);
            *e = *e * 0.5 + took * 0.5;
            store::save_state(&p);
        }

        // ---- verify
        self.with_job(id, |j| j.phase = Phase::Verifying);
        self.set_job(id, |j| {
            j.state = JobState::Verifying;
            j.step = "Checking the result".into();
            j.detail = match &plan.verify {
                Verify::Removed { .. } => "Checking that it is gone".into(),
                Verify::Wait { .. } => "Creative Cloud is installing it".into(),
                Verify::None => String::new(),
                Verify::Version { version, .. } => format!("Checking that {version} is installed"),
            };
            j.progress = 0.98;
        });
        // Handed to another program: wait for it to finish, showing the wait.
        if let Verify::Wait { app_id, version: v, minutes } = &plan.verify {
            // Creative Cloud queues its own installs: let the next job hand off now, and let this
            // one be canceled (Creative Cloud may still finish what it started).
            drop(turn);
            self.set_job(id, |j| j.cancelable = true);
            let mut ctl = ctl.clone();
            let t0 = Instant::now();
            loop {
                self.rescan().await;
                let done = self.st.lock().installed.iter().any(|i| &i.id == app_id && !i.missing && !version::newer(v, &i.version));
                if done {
                    return Ok(Outcome::Done);
                }
                if *ctl.borrow() == Control::Cancel {
                    return Ok(Outcome::Canceled);
                }
                let s = t0.elapsed().as_secs();
                if s > minutes * 60 {
                    return Err(anyhow!("VERIFY|Creative Cloud did not finish installing {} {} within {} minutes", plan.name, v, minutes));
                }
                self.set_job(id, |j| {
                    j.state = JobState::Installing;
                    j.step = format!("Finishing in Creative Cloud, {}:{:02}", s / 60, s % 60);
                    j.progress = 0.05 + 0.94 * install_curve(s as f64 / 600.0);
                });
                // Wake at once on Cancel instead of after the next check.
                let _ = tokio::time::timeout(Duration::from_secs(5), ctl.changed()).await;
            }
        }
        // Installers sometimes finish registering a moment after they exit.
        for attempt in 0..6 {
            self.rescan().await;
            let ok = match &plan.verify {
                Verify::None => true,
                Verify::Version { app_id, version: v } => self
                    .st
                    .lock()
                    .installed
                    .iter()
                    .any(|i| &i.id == app_id && !i.missing && !version::newer(v, &i.version)),
                Verify::Removed { app_id } => !self.st.lock().installed.iter().any(|i| &i.id == app_id),
                Verify::Wait { .. } => true,
            };
            if ok {
                return Ok(Outcome::Done);
            }
            if attempt < 5 {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        match &plan.verify {
            Verify::Version { app_id, version: v } => {
                let now_v = self.installed(app_id).map(|i| i.version).unwrap_or_else(|| "not installed".into());
                Err(anyhow!("VERIFY|The installer finished, but {} reports {} instead of {}", plan.name, now_v, v))
            }
            Verify::Removed { .. } => Err(anyhow!("VERIFY|The uninstaller finished, but {} is still installed", plan.name)),
            Verify::None | Verify::Wait { .. } => unreachable!(),
        }
    }

    pub async fn apply_settings(self: &Arc<Self>, patch: serde_json::Value) -> Settings {
        let old = self.settings.read().clone();
        let new = store::merge(&old, patch);
        *self.settings.write() = new.clone();
        store::save_settings(&new);
        self.throttle.set(if new.bandwidth_limit_enabled { new.bandwidth_limit } else { 0 });
        if new.launch_at_startup != old.launch_at_startup {
            use tauri_plugin_autostart::ManagerExt;
            let m = self.app.autolaunch();
            let r = if new.launch_at_startup { m.enable() } else { m.disable() };
            if let Err(e) = r {
                self.toast("err", format!("Could not change startup: {e}"), None);
            }
        }
        if new.silent_elevation != old.silent_elevation {
            self.set_up_silent_task(new.silent_elevation, false);
        }
        let rescan = new.sources.adobe != old.sources.adobe
            || new.sources.craft != old.sources.craft
            || new.sources.blender != old.sources.blender
            || new.sources.tools != old.sources.tools
            || new.sources.other != old.sources.other
            || new.godot_install_dir != old.godot_install_dir
            || new.blender_channel != old.blender_channel
            || new.blender_keep_patch_versions != old.blender_keep_patch_versions
            || new.craft_prereleases != old.craft_prereleases
            || new.blender_install_dir != old.blender_install_dir
            || new.language != old.language;
        if rescan {
            let me = self.clone();
            tauri::async_runtime::spawn(async move {
                let _ = me.check_now(true).await;
            });
        }
        self.wake.notify_one();
        self.emit("settings", new.clone());
        new
    }
}

/// Fill in Adobe release notes for the newest versions (Adobe only publishes them as web pages).
async fn with_adobe_notes(mut list: Vec<VersionInfo>) -> Vec<VersionInfo> {
    // Pages are per minor version and cached, so every version costs only a few requests.
    let jobs = list.iter().take(25).map(|v| {
        let url = v.notes_url.clone();
        let ver = v.version.clone();
        async move {
            match url {
                Some(u) => crate::notes::adobe(&u, &ver).await,
                None => None,
            }
        }
    });
    let found = futures_util::future::join_all(jobs).await;
    for (v, n) in list.iter_mut().zip(found) {
        if let Some(n) = n {
            v.notes = n.lines;
            if v.date.is_none() {
                v.date = n.date;
            }
            // The version's own post, rather than the list of posts it was found in.
            if let Some(u) = n.url {
                v.notes_url = Some(u);
            }
        } else if version::parts(&v.version).get(2).copied().unwrap_or(0) > 0 {
            v.notes = vec!["Bug-fix release. Adobe lists no details for it.".into()];
        }
    }
    list
}

/// The app's install folder when it is safe to delete after its uninstaller ran: a folder of its
/// own, never a shared parent (Program Files, Adobe, Blender Foundation, Unity Hub, the user's
/// profile...) and never one that holds another installed app.
fn leftover_dir(dir: &std::path::Path, other_installs: &[PathBuf]) -> Option<PathBuf> {
    let norm = |p: &std::path::Path| p.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase();
    let me = norm(dir);
    let pf = paths::program_files();
    let pf86 = paths::program_files_x86();
    let shared = [
        pf.clone(),
        pf86.clone(),
        pf.join("Common Files"),
        pf86.join("Common Files"),
        pf.join("Adobe"),
        pf86.join("Adobe"),
        pf.join("Blender Foundation"),
        pf.join("Unity"),
        pf.join("Unity").join("Hub"),
        pf.join("Unity").join("Hub").join("Editor"),
        paths::local_app_data(),
        paths::local_app_data().join("Programs"),
        paths::app_data(),
        PathBuf::from(std::env::var("USERPROFILE").unwrap_or_default()),
        PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())),
    ];
    if dir.components().count() < 4 || shared.iter().any(|s| norm(s) == me) {
        return None;
    }
    if other_installs.iter().any(|o| {
        let o = norm(o);
        o == me || o.starts_with(&format!("{me}\\"))
    }) {
        return None;
    }
    Some(dir.to_path_buf())
}

fn under_program_files(p: &std::path::Path) -> bool {
    let s = p.to_string_lossy().to_ascii_lowercase();
    [paths::program_files(), paths::program_files_x86()].iter().any(|r| s.starts_with(&format!("{}\\", r.to_string_lossy().to_ascii_lowercase())))
}

/// Saved version histories and release notes (data folder `notes-cache`), so they open at once.
mod notes_cache {
    use crate::model::VersionInfo;
    use crate::{paths, version};
    use once_cell::sync::Lazy;
    use parking_lot::Mutex;
    use std::collections::HashSet;
    use std::time::Duration;

    /// Older than this, a saved list is shown and fetched again in the background.
    pub const FRESH: Duration = Duration::from_secs(5 * 60);

    static RUNNING: Lazy<Mutex<HashSet<String>>> = Lazy::new(|| Mutex::new(HashSet::new()));

    fn file(key: &str) -> std::path::PathBuf {
        let safe: String = key.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '.' { c } else { '_' }).collect();
        paths::data_dir().join("notes-cache").join(format!("{safe}.json"))
    }

    pub fn read(key: &str) -> Option<(Vec<VersionInfo>, Duration)> {
        let f = file(key);
        let age = f.metadata().ok()?.modified().ok()?.elapsed().unwrap_or_default();
        let list: Vec<VersionInfo> = serde_json::from_slice(&std::fs::read(&f).ok()?).ok()?;
        (!list.is_empty()).then_some((list, age))
    }

    pub fn write(key: &str, list: &[VersionInfo]) {
        if list.is_empty() {
            return;
        }
        let f = file(key);
        if let Some(d) = f.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&f, serde_json::to_vec(list).unwrap_or_default());
    }

    /// True when no refresh of `key` runs yet (and marks one as running).
    pub fn start(key: &str) -> bool {
        RUNNING.lock().insert(key.to_string())
    }

    pub fn done(key: &str) {
        RUNNING.lock().remove(key);
    }

    /// A saved list still fits the installed version: the one it marks installed is that version.
    pub fn matches_installed(list: &[VersionInfo], installed: &str) -> bool {
        let same = |v: &str| v == installed || (!v.contains(' ') && !installed.contains(' ') && version::cmp(v, installed).is_eq());
        match list.iter().find(|v| v.installed) {
            Some(v) => same(&v.version),
            None => !list.iter().any(|v| same(&v.version)),
        }
    }
}

/// Runs `f` unless a run is already going. A request that arrives during a run is not dropped:
/// that run goes once more when it ends (with the settings as they are then), so switching a
/// source on during a check still shows its apps.
async fn coalesced<F, Fut, T>(lock: &tokio::sync::Mutex<()>, again: &std::sync::atomic::AtomicBool, f: F) -> Option<T>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    let Ok(_guard) = lock.try_lock() else {
        again.store(true, Ordering::SeqCst);
        return None;
    };
    loop {
        again.store(false, Ordering::SeqCst);
        let r = f().await;
        if !again.swap(false, Ordering::SeqCst) {
            return Some(r);
        }
    }
}

/// The scheduler wakes every few seconds; most ticks do nothing.
const TICK_SECS: u64 = 5;

/// Refresh the status (Creative Cloud running, disk space) on tick `n`: once a minute, and on
/// every tick while Creative Cloud is down.
fn status_refresh_due(n: u64, cc_down: bool) -> bool {
    cc_down || n % (60 / TICK_SECS) == 0
}

/// Tell a job to run, pause or cancel. `send_replace` stores the value even when no download is
/// listening right now: a paused job has ended its download, and a plain `send` would drop the
/// "run" from Resume, so the job read the old "pause" again and paused itself.
fn set_control(tx: &watch::Sender<Control>, c: Control) {
    tx.send_replace(c);
}

/// How long a step usually takes, relative to the others: running an installer or uninstaller is
/// the long part, unpacking is shorter, and removing leftovers or writing a file is quick.
fn step_weight(step: &sources::Step) -> f64 {
    match step {
        sources::Step::Run { .. } | sources::Step::OpenUri { .. } => 1.0,
        sources::Step::ExtractZip { .. } => 0.5,
        sources::Step::RemoveDir { .. } => 0.1,
        _ => 0.02,
    }
}

/// Share of the install phase done in step `k` after `x` = its elapsed / expected time: the
/// steps before it, plus its own share moving along `install_curve`, which never reaches the
/// end of the share. So step 1 of 5 cannot show the bar of a nearly finished install.
fn steps_progress(weights: &[f64], k: usize, x: f64) -> f64 {
    let total: f64 = weights.iter().sum::<f64>().max(f64::EPSILON);
    let before: f64 = weights.iter().take(k).sum();
    let own = weights.get(k).copied().unwrap_or(0.0);
    ((before + own * install_curve(x)) / total).min(1.0)
}

/// What kind of install this is (for learned timings) and a first guess of its seconds.
fn install_estimate(plan: &Plan, bytes: u64) -> (String, f64) {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    let runs_msi = plan.steps.iter().any(|s| matches!(s, sources::Step::Run { exe, .. } if exe.to_string_lossy().to_ascii_lowercase().contains("msiexec")));
    let runs_rum = plan.steps.iter().any(|s| matches!(s, sources::Step::Run { exe, .. } if exe.to_string_lossy().contains("RemoteUpdateManager")));
    let src = plan.app_id.split(':').next().unwrap_or("app");
    if plan.kind == JobKind::Uninstall {
        // Per app (removing Unity takes far longer than removing OBS), from its size on disk.
        let app: String = plan.app_id.split(':').take(2).collect::<Vec<_>>().join(":");
        return (format!("{app}:uninstall"), 20.0 + mb / 120.0);
    }
    if runs_rum {
        return ("adobe:rum".into(), 600.0);
    }
    match src {
        // Keyed per product: Photoshop and Bridge install at very different speeds.
        "adobe" => (format!("adobe:{}", plan.app_id.split(':').nth(1).unwrap_or("")), 30.0 + mb / 12.0),
        "blender" if runs_msi => ("blender:msi".into(), 150.0),
        "blender" => ("blender:zip".into(), 20.0 + mb / 30.0),
        "craft" => ("craft".into(), 25.0),
        "tools" => {
            let key = plan.app_id.split(':').nth(1).unwrap_or("");
            (format!("tools:{key}"), tools::install_seconds(key, mb))
        }
        // Learned per package: apps differ a lot.
        "other" => (plan.app_id.clone(), other::install_seconds(mb)),
        other => (other.to_string(), 30.0),
    }
}

/// Remote Update Manager step: its download folder, the folder prefix for this product and the
/// expected download size, so the job can show real progress while it downloads.
fn rum_watch(step: &sources::Step, to_version: &str, sizes: &HashMap<String, u64>) -> Option<(PathBuf, String, u64)> {
    let sources::Step::Run { exe, args, .. } = step else { return None };
    if !exe.to_string_lossy().contains("RemoteUpdateManager") {
        return None;
    }
    let sap = args.iter().find_map(|a| a.strip_prefix("--productVersions="))?.split('#').next()?.to_string();
    let expected = sizes.get(&format!("{sap}@{to_version}")).copied().unwrap_or(0);
    Some((paths::local_app_data().join("Adobe").join("RemoteUpdateManager_Downloads"), format!("{sap}_"), expected))
}

/// Bytes in the sub-folders of `dir` whose names start with `prefix`.
fn folder_bytes(dir: &std::path::Path, prefix: &str) -> u64 {
    let Ok(rd) = std::fs::read_dir(dir) else { return 0 };
    rd.flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .map(|e| winutil::dir_size(&e.path()))
        .sum()
}

/// Share of the install phase to show after `x` = elapsed / expected: steady up to 90% at the
/// expected time, then slower and slower toward 100% without ever stopping.
fn install_curve(x: f64) -> f64 {
    let x = x.max(0.0);
    if x <= 1.0 {
        0.9 * x
    } else {
        0.9 + 0.1 * (1.0 - (-(x - 1.0)).exp())
    }
}

/// "Blender 2.83" + "2.83.20" -> "Blender 2.83.20" (no repeated version); otherwise "name version".
fn with_version(name: &str, ver: &str) -> String {
    if let Some((head, last)) = name.rsplit_once(' ') {
        if last.chars().next().is_some_and(|c| c.is_ascii_digit()) && ver.starts_with(last) {
            return format!("{head} {ver}");
        }
    }
    if name.ends_with(ver) {
        return name.to_string();
    }
    format!("{name} {ver}")
}

fn sanitize(s: &str) -> String {
    let mut h = sha2::Sha256::new();
    use sha2::Digest;
    h.update(s.as_bytes());
    hex::encode(&h.finalize()[..10])
}

/// A failed install step, carried in the job's error text after "STEP|".
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct StepFailure {
    code: String,
    label: String,
    message: String,
    output: String,
    log: Option<String>,
}

fn step_failure(res: &crate::elevate::StepResult, label: &str, log: Option<&std::path::Path>) -> String {
    let f = StepFailure {
        code: res.code.map(|c| c.to_string()).unwrap_or_default(),
        label: label.to_string(),
        message: res.message.clone(),
        output: res.output.clone(),
        log: log.map(|l| l.display().to_string()),
    };
    format!("STEP|{}", serde_json::to_string(&f).unwrap_or_default())
}

/// Turn an internal error string into the UI's code / message / detail.
fn explain(text: &str, plan: &Plan) -> (String, String, String, Option<String>) {
    if let Some(rest) = text.strip_prefix("LOWDISK|") {
        return ("Low disk space".into(), rest.to_string(), rest.to_string(), None);
    }
    if let Some(rest) = text.strip_prefix("VERIFY|") {
        let msg = rest.lines().next().unwrap_or(rest).to_string();
        return ("Not updated".into(), msg, rest.to_string(), None);
    }
    if let Some(rest) = text.strip_prefix("STEP|") {
        // One JSON object (installer output can hold any character), maybe followed by more
        // text such as "First attempt: ..." from a fallback.
        let mut it = serde_json::Deserializer::from_str(rest).into_iter::<StepFailure>();
        let f = it.next().and_then(|r| r.ok()).unwrap_or_default();
        let more = rest[it.byte_offset()..].trim();
        let code = f.code.as_str();
        let label = f.label.as_str();
        let message = f.message.as_str();
        let output = if more.is_empty() { f.output.clone() } else { format!("{}\n\n{more}", f.output) };
        let output = output.as_str();
        let log = f.log.clone().filter(|s| !s.trim().is_empty());
        let friendly = match code {
            "1618" => "Another installation is already running. Try again when it is done.".to_string(),
            "1603" => "Windows Installer hit a fatal error. The log has the details.".to_string(),
            "1638" => "Another version of this app is already installed.".to_string(),
            "1602" | "1223" => "The installer was canceled.".to_string(),
            "1612" | "1620" => "The installer package could not be opened. Clear the download cache and try again.".to_string(),
            "183" => "Files are in use. Close the app and try again.".to_string(),
            "" => message.to_string(),
            c => format!("{label} stopped with exit code {c}"),
        };
        let code_s = if code.is_empty() { "Install failed".to_string() } else { format!("Error {code}") };
        let mut detail = format!("{label}\n{friendly}");
        if !output.trim().is_empty() {
            detail += &format!("\n\n{}", output.trim());
        }
        if let Some(l) = &log {
            detail += &format!("\nLog: {l}");
        }
        detail += &format!("\nNothing else was changed. {} is still as it was.", plan.name);
        return (code_s, friendly, detail, log);
    }
    let first = text.lines().next().unwrap_or(text).to_string();
    let code = if text.contains("Checksum") { "Checksum mismatch" } else if text.contains("declined") { "Not allowed" } else if text.contains("HTTP") || text.contains("reach") || text.contains("download") { "Download failed" } else { "Failed" };
    (code.into(), first, text.to_string(), None)
}

/// Windows notification sounds: the usual one, and a chime that stands out for failures.
const NOTIFY_SOUND: &str = "Default";
const FAILED_SOUND: &str = "Reminder";

/// The "updates ready" notification: the title counts every update waiting, the text names the
/// ones the user has not heard about yet.
fn update_notice(total: usize, fresh: &[String]) -> (String, String) {
    let title = format!("{total} update{} ready", if total == 1 { "" } else { "s" });
    let names: Vec<&str> = fresh.iter().take(3).map(String::as_str).collect();
    let list = if fresh.len() > 3 { format!("{} and {} more", names.join(", "), fresh.len() - 3) } else { names.join(", ") };
    let body = if fresh.len() < total { format!("New: {list}") } else { list };
    (title, body)
}

/// An update's size from the download server when its source did not give one.
fn with_download_size(mut l: Latest, app_id: &str, dl_sizes: &HashMap<String, u64>) -> Latest {
    if l.size.is_none() {
        l.size = dl_sizes.get(&format!("{app_id}@{}", l.version)).copied().filter(|n| *n > 0);
    }
    l
}

#[cfg(test)]
mod tests {
    use super::{update_notice, with_download_size};
    use super::{folder_bytes, install_curve, leftover_dir, rum_watch, set_control, status_refresh_due, under_program_files, with_version, TICK_SECS};
    use crate::download::Control;

    /// Sync commands run on the main thread, where there is no Tokio runtime: a raw Tokio spawn
    /// there aborts the app at start (1.1.2 crashed so). Tauri's spawn works from any thread.
    #[test]
    fn background_work_is_started_on_tauris_runtime() {
        let raw = [concat!("tokio::", "spawn("), concat!("tokio::task::", "spawn_blocking(")];
        for (name, src) in [("core.rs", include_str!("core.rs")), ("commands.rs", include_str!("commands.rs"))] {
            for r in raw {
                assert!(!src.contains(r), "{name} uses {r}");
            }
        }
    }

    /// Windows plays no sound for a toast without a sound name, and only knows a few names.
    #[test]
    fn every_notification_plays_a_known_windows_sound() {
        let known = ["Default", "IM", "Mail", "Reminder", "SMS"];
        assert!(known.contains(&super::NOTIFY_SOUND) && known.contains(&super::FAILED_SOUND));
        for (name, src) in [("core.rs", include_str!("core.rs")), ("commands.rs", include_str!("commands.rs"))] {
            for (i, line) in src.lines().enumerate() {
                if line.contains(concat!(".builder()", ".title(")) {
                    assert!(line.contains(".sound("), "{name}:{} sends a silent notification", i + 1);
                }
            }
        }
    }

    #[test]
    fn the_update_notice_counts_every_waiting_update() {
        let fresh: Vec<String> = ["Blender 5.2", "Blender 4.5", "MEGAsync"].iter().map(|s| s.to_string()).collect();
        let (title, body) = update_notice(38, &fresh);
        assert_eq!(title, "38 updates ready");
        assert_eq!(body, "New: Blender 5.2, Blender 4.5, MEGAsync");
        let (title, body) = update_notice(3, &fresh);
        assert_eq!(title, "3 updates ready");
        assert_eq!(body, "Blender 5.2, Blender 4.5, MEGAsync");
        assert_eq!(update_notice(1, &fresh[..1]).0, "1 update ready");
    }

    #[test]
    fn an_update_without_a_size_gets_the_size_from_the_server() {
        let mut cache = std::collections::HashMap::new();
        cache.insert("other:7zip.7zip@26.01".to_string(), 1_600_000u64);
        cache.insert("other:Gone@2".to_string(), 0u64);
        let l = |v: &str, size| crate::sources::Latest { version: v.into(), size, note: None };
        assert_eq!(with_download_size(l("26.01", None), "other:7zip.7zip", &cache).size, Some(1_600_000));
        // A size the source gave wins, an unknown size stays unknown.
        assert_eq!(with_download_size(l("26.01", Some(5)), "other:7zip.7zip", &cache).size, Some(5));
        assert_eq!(with_download_size(l("2", None), "other:Gone", &cache).size, None);
    }

    #[tokio::test]
    async fn cancel_works_while_waiting_for_the_install_slot() {
        let gate = super::TurnGate::new();
        let (_t0, r0) = tokio::sync::watch::channel(Control::Run);
        let busy = gate.take("a", "Docker Desktop", 0, r0).await.unwrap(); // another job is installing
        assert_eq!(gate.holder().as_deref(), Some("Docker Desktop"));
        let (tx, rx) = tokio::sync::watch::channel(Control::Run);
        let wait = gate.take("b", "B", 1, rx);
        tokio::pin!(wait);
        // Still waiting while the other install runs.
        assert!(tokio::time::timeout(std::time::Duration::from_millis(50), &mut wait).await.is_err());
        set_control(&tx, Control::Cancel);
        let got = tokio::time::timeout(std::time::Duration::from_millis(500), &mut wait).await.expect("Cancel must end the wait");
        assert!(got.is_none());
        drop(busy);
        // With nothing else installing, the slot is taken right away.
        let (_tx2, rx2) = tokio::sync::watch::channel(Control::Run);
        assert!(gate.take("c", "C", 2, rx2).await.is_some());
    }

    #[tokio::test]
    async fn installs_take_their_turn_in_queue_order() {
        use std::sync::Arc;
        let gate = Arc::new(super::TurnGate::new());
        let (_t0, r0) = tokio::sync::watch::channel(Control::Run);
        let busy = gate.take("a", "A", 0, r0).await.unwrap();
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
        // The 5th job finished downloading first, then the 2nd.
        let mut hs = Vec::new();
        for (id, seq) in [("fifth", 5u64), ("second", 2)] {
            let (g, o) = (gate.clone(), order.clone());
            hs.push(tauri::async_runtime::spawn(async move {
                let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
                let t = g.take(id, id, seq, rx).await.unwrap();
                o.lock().push(id);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                drop(t);
            }));
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        drop(busy);
        for h in hs {
            h.await.unwrap();
        }
        assert_eq!(*order.lock(), vec!["second", "fifth"]);
    }

    #[test]
    fn jobs_waiting_to_install_do_not_hold_a_download_slot() {
        use super::{pick_to_start, Phase};
        use crate::model::JobState;
        // 3 downloads at once. Two jobs finished downloading and wait to install, one downloads,
        // 23 wait: two more downloads start (it started none and showed 0 MB/s before).
        let mut jobs = vec![(true, Phase::Ready, JobState::Installing, false), (true, Phase::Ready, JobState::Installing, false), (true, Phase::Download, JobState::Downloading, false)];
        jobs.extend(std::iter::repeat_n((false, Phase::Queued, JobState::Waiting, false), 23));
        assert_eq!(pick_to_start(&jobs, 3), vec![3, 4]);
        // Running installers do not count either; paused and restored (held) jobs are skipped.
        let jobs = vec![(true, Phase::Running, JobState::Installing, false), (false, Phase::Queued, JobState::Paused, false), (false, Phase::Queued, JobState::Waiting, true), (false, Phase::Queued, JobState::Waiting, false)];
        assert_eq!(pick_to_start(&jobs, 1), vec![3]);
        // Three downloading: nothing more starts.
        let jobs = vec![(true, Phase::Download, JobState::Downloading, false); 3].into_iter().chain([(false, Phase::Queued, JobState::Waiting, false)]).collect::<Vec<_>>();
        assert!(pick_to_start(&jobs, 3).is_empty());
    }

    #[test]
    fn creative_cloud_status_refreshes_within_seconds_while_it_is_down() {
        // Creative Cloud was started a moment ago: the next tick (5 s) must look again.
        assert!(status_refresh_due(1, true));
        assert!(TICK_SECS <= 10);
        // Otherwise once a minute is enough.
        assert!(!status_refresh_due(1, false));
        assert!(status_refresh_due(60 / TICK_SECS, false));
    }

    #[test]
    fn uninstall_removes_only_the_apps_own_leftover_folder() {
        use std::path::PathBuf;
        let pf = crate::paths::program_files();
        let obs = pf.join("obs-studio");
        let others = vec![pf.join("Blender Foundation").join("Blender 4.5"), pf.join("Adobe").join("Adobe Photoshop 2026")];
        assert_eq!(leftover_dir(&obs, &others), Some(obs.clone()));
        assert!(under_program_files(&obs));
        assert_eq!(leftover_dir(&pf.join("Blender Foundation").join("Blender 5.2"), &others).is_some(), true);
        // Shared parents are never removed.
        for shared in [pf.clone(), pf.join("Adobe"), pf.join("Blender Foundation"), pf.join("Unity").join("Hub").join("Editor"), crate::paths::local_app_data().join("Programs")] {
            assert_eq!(leftover_dir(&shared, &[]), None, "{}", shared.display());
        }
        // Nor a folder that holds another installed app.
        assert_eq!(leftover_dir(&pf.join("Adobe").join("Adobe Photoshop 2026"), &others), None);
        assert_eq!(leftover_dir(&pf.join("Blender Foundation"), &[]), None);
        let parent_of_other = pf.join("Suite");
        assert_eq!(leftover_dir(&parent_of_other, &[parent_of_other.join("App")]), None);
        assert_eq!(leftover_dir(&PathBuf::from("C:\\"), &[]), None);
    }

    #[tokio::test]
    async fn a_check_asked_for_during_a_check_runs_after_it() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
        use std::sync::Arc;
        let lock = Arc::new(tokio::sync::Mutex::new(()));
        let again = Arc::new(AtomicBool::new(false));
        let runs = Arc::new(AtomicU32::new(0));
        let gate = Arc::new(tokio::sync::Notify::new());
        // A check is running (waiting on `gate`)...
        let first = {
            let (lock, again, runs, gate) = (lock.clone(), again.clone(), runs.clone(), gate.clone());
            tauri::async_runtime::spawn(async move {
                super::coalesced(&lock, &again, || {
                    let (runs, gate) = (runs.clone(), gate.clone());
                    async move {
                        if runs.fetch_add(1, Ordering::SeqCst) == 0 {
                            gate.notified().await;
                        }
                    }
                })
                .await
            })
        };
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        // ...when Other apps is switched on: that request returns at once...
        assert!(super::coalesced(&lock, &again, || async {}).await.is_none());
        gate.notify_one();
        first.await.unwrap();
        // ...and the running check goes once more, so the new source is checked.
        assert_eq!(runs.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn install_bar_follows_the_steps() {
        use super::steps_progress;
        // Unity's uninstall: its uninstaller, then the leftover folders and files (5 steps).
        let w = [1.0, 0.1, 0.1, 0.02, 0.02];
        let step2_starts = 1.0 / w.iter().sum::<f64>();
        // The uninstaller runs far past its estimate: still step 1, so the bar stays short of
        // where step 2 begins (it showed 98% here before).
        let p = steps_progress(&w, 0, 20.0);
        assert!(p < step2_starts && p > 0.7, "{p}");
        // It keeps moving forward through the later steps and ends at the end.
        assert!(steps_progress(&w, 1, 0.0) >= p);
        assert!(steps_progress(&w, 2, 0.5) > steps_progress(&w, 1, 0.5));
        assert!(steps_progress(&w, 4, 1e9) > 0.999);
        // A plan with one step behaves as before.
        assert!((steps_progress(&[1.0], 0, 1.0) - 0.9).abs() < 1e-9);
    }

    #[test]
    fn saved_version_lists_are_dropped_after_an_update() {
        use crate::model::VersionInfo;
        let v = |ver: &str, installed: bool| VersionInfo { version: ver.into(), date: None, notes: vec![], notes_url: None, size: None, installed, latest: false, prerelease: false };
        let saved = vec![v("4.5.14", false), v("4.5.5", true)];
        assert!(super::notes_cache::matches_installed(&saved, "4.5.5"));
        // Updated to 4.5.14 since: the saved list still marks 4.5.5.
        assert!(!super::notes_cache::matches_installed(&saved, "4.5.14"));
        // Lists that mark nothing (Get apps) fit unless they contain the installed version.
        assert!(super::notes_cache::matches_installed(&[v("5.2.2", false)], "4.5.5"));
        // Blender pre-release builds differ only by their hash.
        let b = vec![v("5.4.0 Alpha fbe82a4", true)];
        assert!(!super::notes_cache::matches_installed(&b, "5.4.0 Alpha 1234567"));
    }

    #[test]
    fn resume_after_pause_is_not_lost() {
        // Pause while the download listens, the download ends, then Resume with nobody listening.
        let (tx, rx) = tokio::sync::watch::channel(Control::Run);
        set_control(&tx, Control::Pause);
        assert_eq!(*rx.borrow(), Control::Pause);
        drop(rx);
        set_control(&tx, Control::Run);
        // The restarted download must see "run", not the old "pause".
        assert_eq!(*tx.subscribe().borrow(), Control::Run);
    }

    #[test]
    fn remote_update_manager_progress_comes_from_its_folder() {
        let step = crate::sources::Step::Run {
            exe: "C:/x/RemoteUpdateManager.exe".into(),
            args: vec!["--action=install".into(), "--productVersions=SBSTP".into()],
            cwd: None,
            ok_codes: vec![0],
            elevated: true,
            label: String::new(),
            log: None,
        };
        let mut sizes = std::collections::HashMap::new();
        sizes.insert("SBSTP@12.2.0".to_string(), 2_500_000u64);
        let (dir, prefix, expected) = rum_watch(&step, "12.2.0", &sizes).unwrap();
        assert!(dir.ends_with("RemoteUpdateManager_Downloads"));
        assert_eq!((prefix.as_str(), expected), ("SBSTP_", 2_500_000));
        // Only folders of this product count.
        let root = std::env::temp_dir().join(format!("u3d-rum-{}", std::process::id()));
        std::fs::create_dir_all(root.join("SBSTP_12.2.0.5909_win64")).unwrap();
        std::fs::create_dir_all(root.join("PHSP_27.11_win64")).unwrap();
        std::fs::write(root.join("SBSTP_12.2.0.5909_win64").join("a.zip"), vec![0u8; 1000]).unwrap();
        std::fs::write(root.join("PHSP_27.11_win64").join("b.zip"), vec![0u8; 500]).unwrap();
        assert_eq!(folder_bytes(&root, "SBSTP_"), 1000);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn names_do_not_repeat_the_version() {
        assert_eq!(with_version("Blender 2.83", "2.83.20"), "Blender 2.83.20");
        assert_eq!(with_version("Blender 4.5", "4.5.5"), "Blender 4.5.5");
        assert_eq!(with_version("Photoshop 2026", "27.11"), "Photoshop 2026 27.11");
        assert_eq!(with_version("PhotoCraft", "0.12.0"), "PhotoCraft 0.12.0");
        assert_eq!(with_version("Blender 5.2.3", "5.2.3"), "Blender 5.2.3");
    }

    #[test]
    fn install_bar_always_moves_and_never_finishes() {
        assert_eq!(install_curve(0.0), 0.0);
        assert!((install_curve(0.5) - 0.45).abs() < 1e-9);
        assert!((install_curve(1.0) - 0.9).abs() < 1e-9);
        // Past the estimate it keeps creeping: every extra second still moves the bar.
        let mut last = install_curve(1.0);
        for k in 1..50 {
            let v = install_curve(1.0 + k as f64 * 0.1);
            assert!(v > last && v < 1.0, "at {k}: {v}");
            last = v;
        }
    }
}

#[cfg(test)]
mod job_fix_tests {
    use super::{close_targets, explain, step_failure};
    use crate::elevate::StepResult;
    use crate::model::JobKind;
    use crate::procs::Proc;
    use crate::sources::{Plan, Verify};
    use std::path::PathBuf;

    fn proc(pid: u32, exe: &str) -> Proc {
        Proc { pid, name: "blender.exe".into(), exe: Some(PathBuf::from(exe)), cpu: 0.0, memory: 0 }
    }

    #[test]
    fn close_and_continue_leaves_the_other_copy_open() {
        // Blender 4.2 and 5.0 both open; only 4.2 is being updated.
        let list = vec![proc(100, r"C:\Program Files\Blender Foundation\Blender 4.2\blender.exe"), proc(200, r"C:\Program Files\Blender Foundation\Blender 5.0\blender.exe")];
        let dirs = vec![PathBuf::from(r"C:\Program Files\Blender Foundation\Blender 4.2")];
        assert_eq!(close_targets(&list, &dirs, &[]), vec![100]);
    }

    #[test]
    fn step_error_output_with_a_bar_keeps_the_log_path() {
        let plan = Plan { app_id: "x".into(), kind: JobKind::Update, name: "Zipper".into(), to_version: "1".into(), downloads: vec![], steps: vec![], block_dirs: vec![], block_names: vec![], verify: Verify::None, fallback: None, cleanup: vec![] };
        let res = StepResult { ok: false, code: Some(1603), message: "Exit code 1603".into(), output: "a|b".into() };
        let text = step_failure(&res, "Installing Zipper", Some(std::path::Path::new(r"C:\logs\zipper.log")));
        let (code, _msg, detail, log) = explain(&text, &plan);
        assert_eq!(code, "Error 1603");
        assert_eq!(log.as_deref(), Some(r"C:\logs\zipper.log"));
        assert!(detail.contains("a|b"), "{detail}");
        // Text added after the step error (a fallback's first attempt) is kept in the details.
        let (_, _, detail, log) = explain(&format!("{text}\n\nFirst attempt: boom"), &plan);
        assert_eq!(log.as_deref(), Some(r"C:\logs\zipper.log"));
        assert!(detail.contains("First attempt: boom"), "{detail}");
    }
}

#[cfg(test)]
mod queue_tests {
    use super::*;

    fn job(id: &str, state: JobState) -> Job {
        Job {
            id: id.into(),
            app_id: format!("other:{id}"),
            name: id.into(),
            code: "X".into(),
            fg: "#fff".into(),
            bg: "#000".into(),
            icon_url: None,
            kind: JobKind::Update,
            from_version: Some("1.0".into()),
            to_version: "2.0".into(),
            state,
            downloaded: 5_000_000,
            total: 9_000_000,
            speed: 1.0,
            eta: Some(3.0),
            progress: 0.4,
            step: "Downloading".into(),
            detail: String::new(),
            blocked_by: vec![],
            error: None,
            started_at: None,
            finished_at: None,
            cancelable: false,
        }
    }

    fn installed(id: &str, version: &str) -> Installed {
        Installed {
            id: id.into(),
            source: Source::Other,
            name: id.into(),
            code: String::new(),
            fg: String::new(),
            bg: String::new(),
            version: version.into(),
            install_dir: None,
            exe: None,
            uninstall: None,
            updated_at: None,
            size_hint: None,
            tagline: None,
            channel: None,
            missing: false,
            read_only: false,
            icon_remote: None,
            data: SourceData::Tools { key: String::new(), series: String::new() },
        }
    }

    #[test]
    fn the_queue_file_keeps_unfinished_jobs() {
        let dir = std::env::temp_dir().join(format!("u3d-queue-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("queue.json");
        let jobs = vec![
            SavedJob { job: job("a", JobState::Downloading), req: JobRequest::Update { app_id: "other:a".into(), version: None }, phase: Phase::Download, dests: vec!["C:/dl/a.exe".into()], verify: None },
            SavedJob {
                job: job("b", JobState::Installing),
                req: JobRequest::Uninstall { app_id: "other:b".into(), remove_prefs: true },
                phase: Phase::Running,
                dests: vec![],
                verify: Some(SavedVerify::Removed { app_id: "other:b".into() }),
            },
        ];
        save_queue_to(&file, &jobs);
        let back = load_queue_from(&file);
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].job.id, "a");
        assert_eq!(back[0].dests, vec![PathBuf::from("C:/dl/a.exe")]);
        assert!(matches!(&back[1].req, JobRequest::Uninstall { remove_prefs: true, .. }));
        assert_eq!(back[1].phase, Phase::Running);
        assert_eq!(back[1].verify, Some(SavedVerify::Removed { app_id: "other:b".into() }));
        assert!(!file.with_extension("json.tmp").exists());
        // A damaged or missing file is an empty queue, not a crash.
        std::fs::write(&file, b"[{").unwrap();
        assert!(load_queue_from(&file).is_empty());
        assert!(load_queue_from(&dir.join("none.json")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restart_continues_downloads_and_never_reruns_a_cut_off_installer() {
        // Waiting, downloading, downloaded and waiting for a turn or for the app to close:
        // queued again (the files on disk are reused or continued).
        assert_eq!(restore_kind(JobState::Waiting, Phase::Queued), Restore::Queue(JobState::Waiting));
        assert_eq!(restore_kind(JobState::Downloading, Phase::Download), Restore::Queue(JobState::Waiting));
        assert_eq!(restore_kind(JobState::Installing, Phase::Ready), Restore::Queue(JobState::Waiting));
        assert_eq!(restore_kind(JobState::Blocked, Phase::Ready), Restore::Queue(JobState::Waiting));
        assert_eq!(restore_kind(JobState::Paused, Phase::Download), Restore::Queue(JobState::Paused));
        // The installer was running, or its result was being checked: look first.
        assert_eq!(restore_kind(JobState::Installing, Phase::Running), Restore::Check);
        assert_eq!(restore_kind(JobState::Verifying, Phase::Verifying), Restore::Check);
    }

    #[test]
    fn a_cut_off_install_counts_as_done_only_when_the_new_version_is_there() {
        let want = SavedVerify::Version { app_id: "other:a".into(), version: "2.0".into() };
        assert!(finished_anyway(Some(&want), &[installed("other:a", "2.0")]));
        assert!(finished_anyway(Some(&want), &[installed("other:a", "2.1")]));
        assert!(!finished_anyway(Some(&want), &[installed("other:a", "1.0")]));
        assert!(!finished_anyway(Some(&want), &[]));
        let gone = SavedVerify::Removed { app_id: "other:a".into() };
        assert!(finished_anyway(Some(&gone), &[installed("other:b", "1")]));
        assert!(!finished_anyway(Some(&gone), &[installed("other:a", "1")]));
        assert!(!finished_anyway(None, &[installed("other:a", "2.0")]), "nothing to tell by: shown as interrupted");
        // A repair keeps the version, so it can not tell.
        let plan = Plan { app_id: "other:a".into(), kind: JobKind::Repair, name: "A".into(), to_version: "2.0".into(), downloads: vec![], steps: vec![], block_dirs: vec![], block_names: vec![], verify: Verify::Version { app_id: "other:a".into(), version: "2.0".into() }, fallback: None, cleanup: vec![] };
        assert_eq!(saved_verify(&plan), None);
    }

    #[test]
    fn each_step_says_what_it_does() {
        use crate::sources::Step;
        let run = |exe: &str, label: &str, args: Vec<String>| Step::Run { exe: exe.into(), args, cwd: None, ok_codes: vec![0], elevated: false, label: label.into(), log: None };
        assert_eq!(step_detail(&run("C:/dl/Docker Desktop Installer.exe", "Installing Docker Desktop", vec![]), None), "Running the installer silently");
        assert_eq!(step_detail(&run("C:/Windows/System32/msiexec.exe", "Installing", vec!["/i".into()]), None), "Windows Installer is running silently");
        assert_eq!(step_detail(&run("C:/Windows/System32/msiexec.exe", "Uninstalling X", vec!["/x".into()]), None), "Windows Installer is removing it silently");
        assert_eq!(step_detail(&run("C:/Program Files/X/unins000.exe", "Removing the old X", vec![]), None), "Running the uninstaller silently");
        let unzip = Step::ExtractZip { zip: "a.zip".into(), dest: "D:/B".into(), strip_root: true, elevated: false, label: String::new() };
        assert_eq!(step_detail(&unzip, Some(1234)), "Unpacking 1,234 files");
        assert_eq!(step_detail(&unzip, Some(1)), "Unpacking 1 file");
        assert_eq!(step_detail(&Step::RemoveDir { path: "D:/Blender/5.1".into(), elevated: false, label: String::new() }, None), "Deleting the folder 5.1");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(thousands(999), "999");
    }

    #[test]
    fn a_zip_counts_its_files() {
        let dir = std::env::temp_dir().join(format!("u3d-zipcount-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&p).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            w.add_directory("top/", o).unwrap();
            for k in 0..3 {
                w.start_file(format!("top/{k}.txt"), o).unwrap();
                std::io::Write::write_all(&mut w, b"x").unwrap();
            }
            w.finish().unwrap();
        }
        assert_eq!(zip_files(&p), Some(3));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
