//! The app's brain: installed apps, latest versions, the job queue and the background checks.

use crate::download::{self, Control, Outcome, Progress, Throttle};
use crate::elevate::Elevator;
use crate::model::*;
use crate::sources::{self, adobe, blender, board, chipset, craft, drivers, ghapps, other, tools, Installed, Latest, Plan, SourceData, Verify};
use crate::store::{self, Persisted};
use crate::{install_detail, paths, procs, version, winutil};
use anyhow::{anyhow, Result};
use parking_lot::{Mutex, RwLock};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use crate::notice::{self, Target};
use tauri::{AppHandle, Emitter};
use tokio::sync::{watch, Notify};

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum JobRequest {
    Update { app_id: String, version: Option<String> },
    Install { id: String, version: Option<String> },
    /// `remove_prefs`: also delete the app's settings and add-ons ("Also delete settings and add-ons")
    Uninstall { app_id: String, remove_prefs: bool },
    Repair { app_id: String },
    /// A driver update ("driver:nvidia:1022", "driver:dev:pci\\ven_8086..."). An Update of a
    /// "driver:" id becomes this one.
    Driver { id: String },
}

/// An Other app without an icon is searched for again after this many days.
const ICON_RETRY_DAYS: i64 = 7;

/// An installed app's icon: from its program file, else for an Other app the catalog's icon of
/// its package (cached for Get apps), so an app whose program is not found still shows its icon.
fn app_icon(i: &Installed, icons: &HashMap<String, String>, catalog: &HashMap<String, String>) -> Option<String> {
    icons.get(&i.id).or_else(|| (i.source == Source::Other).then(|| catalog.get(&format!("other:{}", other::pkg_of(i)))).flatten()).cloned()
}

/// The icon of a package kept in the icon cache from an earlier run.
fn other_icon_on_disk(id: &str) -> Option<std::path::PathBuf> {
    crate::sources::icons::on_disk(&crate::sources::icons::file_stem(id))
}

/// A folder as compared with another: lowercase, backslashes, no slash at the end.
fn norm_dir(d: &std::path::Path) -> String {
    d.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

/// The Updater 3D's own entry in the apps list: its update is the one its own feed offers
/// (installed by "Update and restart", never by a job), and its page is the app's own page.
fn mark_self(a: &mut AppEntry, ready: Option<&str>) {
    a.is_self = true;
    a.pre_channel = None;
    a.group = Source::Other.group().into();
    a.tagline = Some("Keeps your apps and drivers up to date".into());
    a.note = None;
    a.can_rollback = false;
    if a.status == AppStatus::Missing {
        return;
    }
    match ready.filter(|v| version::newer(v, &a.version)) {
        Some(v) => {
            a.status = AppStatus::Update;
            a.latest = Some(v.to_string());
            // Not a job: Update all, the count and auto-install leave it out.
            a.manual_url = Some("theupdater3d://about".into());
        }
        None => {
            if a.status != AppStatus::Updating && a.status != AppStatus::Queued {
                a.status = AppStatus::Current;
            }
            a.latest = Some(a.version.clone());
        }
    }
}

/// What the game sign-ins of 1.2.2 kept on this PC: the encrypted sign-ins, the game libraries,
/// the Steam window's cookies and the game icons. The games feature is gone, so they go too.
fn forget_game_data(data: &std::path::Path, icons: &std::path::Path) {
    let _ = std::fs::remove_dir_all(data.join("accounts"));
    if let Ok(rd) = std::fs::read_dir(icons) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_ascii_lowercase();
            if n.starts_with("owned-") || n.starts_with("game-") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// The version an installed Other app is offered: its newest GitHub pre-release (`pre`, given
/// only when the app follows them) when that is newer than the copy and the catalog's version,
/// else the catalog's.
fn other_offer(stable: Option<Latest>, pre: Option<&Latest>, installed: &str) -> Option<Latest> {
    match pre {
        Some(p) if other::is_newer(&p.version, installed) && stable.as_ref().is_none_or(|s| other::is_newer(&p.version, &s.version)) => Some(p.clone()),
        _ => stable,
    }
}

/// The apps the catalog sources look at: Other programs are what no source matched, so they
/// must not count as handled by one.
fn catalog_apps(v: &[Installed]) -> Vec<Installed> {
    v.iter().filter(|i| i.source != Source::Programs).cloned().collect()
}

fn icon_is_small(p: &std::path::Path) -> bool {
    crate::sources::icons::is_small(p)
}

/// Rules a remembered icon miss was made under. Misses saved by earlier versions, which also
/// remembered searches that only timed out, are searched again once.
const ICON_MISS_RULES: u32 = 2;

/// The day of a remembered miss ("2026-10-22/2"), None for one made under older rules.
fn icon_miss_day(v: &str) -> Option<chrono::NaiveDate> {
    let (d, rules) = v.split_once('/')?;
    if rules.parse::<u32>().ok()? < ICON_MISS_RULES {
        return None;
    }
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()
}

/// A found Other apps icon, for the window.
#[derive(Clone, Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IconFound {
    id: String,
    /// None: the search is over and found no icon (the card shows its painted mark)
    icon_url: Option<String>,
}

/// Results of the icon search not handed on yet.
#[derive(Default)]
struct IconOut {
    found: Vec<IconFound>,
    missed: Vec<String>,
}

impl IconOut {
    fn is_empty(&self) -> bool {
        self.found.is_empty() && self.missed.is_empty()
    }
}

/// What is known of some packages' icons.
#[derive(Default)]
struct IconsNow {
    /// Kept in memory already: the window may not have them.
    known: Vec<IconFound>,
    /// Small files from an earlier run, read just now.
    disk: Vec<IconFound>,
    /// Files of an earlier version at full size: made small before they are shown.
    convert: Vec<String>,
    /// No icon yet and worth searching for.
    search: Vec<String>,
    /// Searched not long ago and found none.
    none: Vec<String>,
}

static ICON_QUEUE: once_cell::sync::Lazy<Mutex<crate::sources::icons::Queue>> = once_cell::sync::Lazy::new(Default::default);
static ICON_BUSY: once_cell::sync::Lazy<Mutex<HashSet<String>>> = once_cell::sync::Lazy::new(Default::default);
static ICON_OUT: once_cell::sync::Lazy<Mutex<IconOut>> = once_cell::sync::Lazy::new(Default::default);
static ICON_CONVERTING: once_cell::sync::Lazy<Mutex<HashSet<String>>> = once_cell::sync::Lazy::new(Default::default);
static ICON_WORKERS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static ICON_FLUSHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// How often found icons are sent to the window while the search runs.
const ICON_FLUSH_MS: u64 = 80;

/// Makers whose icon was asked for in this run (a maker is read once, not once per driver).
static MAKERS_ASKED: once_cell::sync::Lazy<Mutex<HashSet<&'static str>>> = once_cell::sync::Lazy::new(Default::default);

/// An icon saved by an earlier version at full size, made small and saved again (the old file is
/// removed). None when there is no such file. A picture Windows cannot read is kept as it is.
async fn convert_old_icon(id: &str) -> Option<PathBuf> {
    use crate::sources::icons;
    let old = other_icon_on_disk(id).filter(|p| !icon_is_small(p))?;
    let key = id.to_string();
    tauri::async_runtime::spawn_blocking(move || -> Option<PathBuf> {
        let bytes = std::fs::read(&old).ok()?;
        let ext = icons::sniff(&bytes)?;
        let got = icons::normalize(icons::Got { bytes, ext, via: "cache", dark: false });
        if got.ext != "png" {
            return Some(old);
        }
        let p = paths::icons().join(icons::saved_name(&key, &got));
        std::fs::write(&p, &got.bytes).ok()?;
        let _ = std::fs::remove_file(&old);
        icons::wrote();
        Some(p)
    })
    .await
    .ok()
    .flatten()
}

/// An app's icon as a small file in the icon cache: an icon saved at full size by an earlier
/// version made small, or one searched for on the web.
async fn find_other_icon(feed: Option<Arc<other::Feed>>, id: String) -> std::result::Result<PathBuf, crate::sources::icons::Miss> {
    use crate::sources::icons::{self, Miss};
    if let Some(p) = convert_old_icon(&id).await {
        return Ok(p);
    }
    let src = other::icon_source_of(feed.as_deref(), &id).await.ok_or(Miss { passing: true })?;
    let got = icons::find(&id, src.icon.as_deref(), &src.pages).await?;
    tauri::async_runtime::spawn_blocking(move || {
        let got = icons::normalize(got);
        let p = paths::icons().join(icons::saved_name(&id, &got));
        std::fs::write(&p, &got.bytes).ok()?;
        icons::wrote();
        Some(p)
    })
    .await
    .ok()
    .flatten()
    .ok_or(Miss { passing: true })
}

struct JobRec {
    job: Job,
    req: JobRequest,
    ctl: watch::Sender<Control>,
    active: bool,
    /// The plan's folders and exe names that must be closed (set while the job waits for them),
    /// so "Close and continue" closes exactly those and not another copy of the app.
    /// What must close before the install: programs in these folders, with these names, or
    /// that have these files loaded.
    block: (Vec<PathBuf>, Vec<String>, Vec<PathBuf>),
    /// Queue order (installs that wait for their turn go in this order).
    seq: u64,
    phase: Phase,
    /// Restored after a restart: waits until the app knows what is installed again.
    held: bool,
    /// Where its downloads go, so a restart keeps their partial files.
    dests: Vec<PathBuf>,
    /// How to tell after a restart whether an install that was cut off finished.
    verify: Option<SavedVerify>,
    /// Its installer said a restart finishes the install.
    restart: bool,
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
        // Drivers are not in the installed list: a cut-off driver install is checked again.
        Verify::None | Verify::Driver { .. } => None,
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

/// Write the queue file when it would change. `saved` is what the last write held; it starts
/// empty, before anything was written. The signature is never empty (it starts with the job
/// count), so the first save always writes, even an empty queue: jobs restored at start that all
/// finish (or are canceled) must leave the file, or they come back at every restart.
fn write_queue_if_changed(file: &std::path::Path, saved: &mut String, list: &[SavedJob]) {
    let sig: String = std::iter::once(format!("{}#", list.len())).chain(list.iter().map(|s| format!("{}:{:?}:{:?}:{}:{};", s.job.id, s.job.state, s.phase, s.dests.len(), s.job.to_version))).collect();
    if *saved == sig {
        return;
    }
    save_queue_to(file, list);
    *saved = sig;
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
    /// The queue was reordered: the jobs waiting take their turns in the new order.
    fn reorder(&self, seqs: &HashMap<String, u64>) {
        let mut st = self.st.lock();
        for w in st.waiting.iter_mut() {
            if let Some(s) = seqs.get(&w.1) {
                w.0 = *s;
            }
        }
        drop(st);
        self.notify.notify_waiters();
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

/// A new job's place in the install line. An uninstall is asked for now: it takes the next turn,
/// before updates and installs that only wait for theirs (after the one installing now, and
/// after uninstalls asked for earlier). Everything else goes to the end.
fn turn_seq(kind: JobKind, open: impl Iterator<Item = (JobKind, u64)>, next: impl FnOnce() -> u64) -> u64 {
    if kind != JobKind::Uninstall {
        return next();
    }
    let open: Vec<(JobKind, u64)> = open.collect();
    let first_other = open.iter().filter(|(k, _)| *k != JobKind::Uninstall).map(|(_, s)| *s).min();
    let last_uninstall = open.iter().filter(|(k, _)| *k == JobKind::Uninstall).map(|(_, s)| *s).max();
    match (last_uninstall, first_other) {
        // Right after the uninstalls asked for earlier (they are ahead of the updates already).
        (Some(u), _) => u + 1,
        // Well ahead of the waiting updates, with room for more uninstalls after it.
        (None, Some(o)) => o.saturating_sub(1 << 20),
        (None, None) => next(),
    }
}

/// Put the items `ids` names that pass `movable` in the order of `ids`, in the places those
/// items have now. Every other item stays where it is.
fn reorder_in_place<T>(items: &mut [T], ids: &[String], id_of: impl Fn(&T) -> &String, movable: impl Fn(&T) -> bool) {
    let mut seen = HashSet::new();
    let order: Vec<&String> = ids.iter().filter(|id| seen.insert(*id) && items.iter().any(|j| id_of(j) == *id && movable(j))).collect();
    let slots: Vec<usize> = items.iter().enumerate().filter(|(_, j)| movable(j) && order.contains(&id_of(j))).map(|(i, _)| i).collect();
    for (k, id) in order.iter().enumerate() {
        if let (Some(p), Some(&to)) = (items.iter().position(|j| id_of(j) == *id), slots.get(k)) {
            items.swap(p, to);
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
/// Where an app's own install times are kept: installs and uninstalls take different times.
fn runs_key(plan: &Plan) -> String {
    format!("{}:{}", plan.app_id, if plan.kind == JobKind::Uninstall { "uninstall" } else { "install" })
}

/// Does a Run step remove an app (an uninstaller, `msiexec /x`)?
fn run_removes(step: &sources::Step) -> bool {
    let sources::Step::Run { exe, args, label, .. } = step else { return false };
    let file = exe.file_name().map(|f| f.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let low = label.to_ascii_lowercase();
    low.contains("uninstall") || low.contains("removing") || file.contains("uninst") || args.iter().any(|a| a.eq_ignore_ascii_case("/x"))
}

/// What to watch while a step runs, for its live detail line: the installer's log, the programs
/// it starts, and the folder it changes. `before` is the copy as it was.
fn watch_spec(step: &sources::Step, base: String, files: Option<usize>, before: Option<&sources::Installed>, hint: Option<String>, procs: &crate::elevate::Procs) -> install_detail::WatchSpec {
    use install_detail::{DirWatch, LogKind};
    use sources::Step;
    let app_dir = before.and_then(|b| b.install_dir.clone()).filter(|d| d.is_dir() && sources::other::own_dir(d));
    let (log, prog, dir) = match step {
        Step::Run { exe, args, log, .. } => {
            let name = exe.file_name().map(|f| f.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let log = log.as_ref().and_then(|l| {
                if name == "msiexec.exe" {
                    Some((l.clone(), LogKind::Msi))
                } else if args.contains(&format!("/LOG={}", l.display())) {
                    Some((l.clone(), LogKind::Inno))
                } else {
                    None
                }
            });
            let dir = app_dir.map(|d| if run_removes(step) { DirWatch::Remove { dir: d } } else { DirWatch::Copy { dir: d, expected: before.and_then(|b| b.size_hint).filter(|s| *s > 0) } });
            (log, Some((procs.clone(), name)), dir)
        }
        Step::ExtractZip { dest, .. } => (None, None, Some(DirWatch::Unpack { dir: dest.clone(), files })),
        Step::RemoveDir { path, .. } => (None, None, Some(DirWatch::Remove { dir: path.clone() })),
        _ => (None, None, None),
    };
    install_detail::WatchSpec { base, log, procs: prog, dir, hint, since: std::time::SystemTime::now() }
}

fn step_detail(step: &sources::Step, files: Option<usize>) -> String {
    use sources::Step;
    match step {
        Step::Run { exe, .. } => {
            let file = exe.file_name().map(|f| f.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
            let uninstall = run_removes(step);
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
        Step::AddPackage { update: true, .. } => "Windows is updating the app package".into(),
        Step::AddPackage { .. } => "Windows is installing the app package".into(),
        Step::RemovePackage { .. } => "Windows is removing the app package".into(),
        Step::Leftover { label, .. } => label.clone(),
        Step::ReplacePortable { .. } => "Replacing its program files, keeping its data".into(),
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
/// The running programs an install waits for: `procs::blocking`, and the ones that have one of
/// `files` loaded (Restart Manager, only asked when there are files).
fn blockers(list: &[procs::Proc], dirs: &[PathBuf], names: &[String], files: &[PathBuf]) -> Vec<procs::Proc> {
    let mut b = procs::blocking(list, dirs, names);
    if !files.is_empty() {
        let using = procs::using_files(files);
        let more: Vec<procs::Proc> = list.iter().filter(|p| using.contains(&p.pid) && !b.iter().any(|x| x.pid == p.pid)).cloned().collect();
        b.extend(more);
    }
    b
}

/// What the last driver check found (also saved, see `drivers::Saved`).
#[derive(Default)]
pub struct DriversState {
    found: Vec<drivers::Found>,
    checking: bool,
    last_check: Option<String>,
    /// Windows Update's last answer: shown again while it is asked anew (it is the slow one)
    wu: Option<drivers::Feed>,
}

impl From<drivers::Saved> for DriversState {
    fn from(s: drivers::Saved) -> Self {
        DriversState { found: s.found, checking: false, last_check: s.last_check, wu: s.wu }
    }
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
    /// Ids (installed apps, "other:<package>" of the catalog) whose icon search is over and found
    /// none: their cards show the painted mark instead of waiting for an icon.
    icon_none: HashSet<String>,
    /// Maker key -> local icon URL of the maker of drivers (see `sources::makers`)
    maker_icons: HashMap<String, String>,
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
    /// Releases of the GitHub projects that Other programs name (see `ghapps`)
    pub ghapps: RwLock<Option<Arc<ghapps::Feed>>>,
    /// The newest GitHub pre-release of each installed Other app the user set to follow them
    /// (app id -> version)
    /// (and whether the copy here is one of them, so it follows them until the user chooses)
    other_pre: RwLock<HashMap<String, (Latest, bool)>>,
    jobs: Mutex<Vec<JobRec>>,
    pub elevator: Elevator,
    /// Bumped by "Allow admin rights": the jobs waiting for admin rights try again.
    admin_gen: watch::Sender<u64>,
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
    /// What the last driver check found: listed with the apps (group "Drivers").
    pub drivers: Mutex<DriversState>,
    jobs_dirty: std::sync::atomic::AtomicBool,
    /// Wakes the job progress sender, which sleeps while nothing changes.
    jobs_wake: Notify,
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
        paths::set_downloads(Some(&settings.download_dir));
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
            ghapps: RwLock::new(None),
            other_pre: RwLock::new(HashMap::new()),
            jobs: Mutex::new(Vec::new()),
            elevator: Elevator::new(),
            admin_gen: watch::channel(0).0,
            throttle,
            wake: Notify::new(),
            install_gate: TurnGate::new(),
            write_locks: KeyLocks::default(),
            pending_route: Mutex::new(None),
            // Room below for jobs that go first (uninstalls, see `turn_seq`).
            next_seq: std::sync::atomic::AtomicU64::new(1 << 32),
            queue_saved: Mutex::new(String::new()),
            check_lock: tokio::sync::Mutex::new(()),
            check_again: std::sync::atomic::AtomicBool::new(false),
            persisted: Mutex::new(persisted),
            // Shown at once, and a driver job queued before a restart still knows what to install.
            drivers: Mutex::new(drivers::load(&drivers::saved_file()).into()),
            jobs_dirty: std::sync::atomic::AtomicBool::new(false),
            jobs_wake: Notify::new(),
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
            machine: crate::machine::this().line(),
            machine_tier: crate::machine::this().tier,
            restart_needed: false,
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
            job.ready = false;
            // Asked again when its turn comes; the old "needs admin rights" no longer holds.
            job.needs_admin = false;
            let (tx, _) = watch::channel(if state == JobState::Paused { Control::Pause } else { Control::Run });
            let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
            jobs.push(JobRec { job, req: s.req, ctl: tx, active: false, block: Default::default(), seq, phase, held: true, dests: s.dests, verify: s.verify, restart: false });
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
        let freed = download::trim_cache(&paths::downloads(), &keep, self.settings.read().keep_previous_versions);
        if freed > 0 {
            log::info!("deleted {} of old installers from the download cache", fmt_bytes(freed));
        }
    }

    /// Keep the download cache in bounds (see `download::cache_victims`) after a job finished.
    fn trim_download_cache(&self) {
        let busy: HashSet<PathBuf> = self.jobs.lock().iter().filter(|j| !j.job.state.is_finished()).flat_map(|j| j.dests.iter().cloned()).collect();
        let freed = download::trim_cache(&paths::downloads(), &busy, self.settings.read().keep_previous_versions);
        if freed > 0 {
            log::info!("deleted {} of old installers from the download cache", fmt_bytes(freed));
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
        write_queue_if_changed(&queue_file(), &mut self.queue_saved.lock(), &list);
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

    /// A Windows notification with a sound. A click on it opens the main window on `target`,
    /// also when the app runs only in the tray.
    fn notify(&self, title: &str, body: &str, target: Target) {
        self.notify_with(title, body, NOTIFY_SOUND, target);
    }

    fn notify_with(&self, title: &str, body: &str, sound: &str, target: Target) {
        notice::show(&self.app, title, body, sound, &target);
    }

    /// The window was closed and the app keeps running in the background. Clicking the notice
    /// opens the window again.
    pub fn tell_still_running(&self, busy: bool) {
        let (title, body) = still_running_notice(busy);
        self.notify(title, body, Target::Home);
    }

    fn activity(&self, kind: ActivityKind, title: impl Into<String>, meta: Option<String>, app_id: Option<String>, error: Option<JobError>) {
        let e = store::activity(kind, title, meta, app_id, error);
        self.emit("activity", e);
    }

    pub fn emit_apps(&self) {
        let apps = self.apps();
        let n = apps.iter().filter(|a| a.status == AppStatus::Update && a.manual_url.is_none()).count();
        self.emit("apps", apps);
        crate::tray::set_count(&self.app, n);
    }

    pub fn emit_status(&self) {
        let s = self.status();
        self.emit("status", s);
    }

    fn mark_jobs_dirty(&self) {
        self.jobs_dirty.store(true, Ordering::Relaxed);
        self.jobs_wake.notify_one();
    }

    /// Sends job progress at most 8 times a second, and sleeps (no timer) while nothing changes.
    pub async fn jobs_ticker(self: Arc<Self>) {
        let mut t = tokio::time::interval(Duration::from_millis(125));
        t.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            if !self.jobs_dirty.load(Ordering::Relaxed) {
                self.jobs_wake.notified().await;
            }
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

    pub fn installed_all(&self) -> Vec<Installed> {
        self.st.lock().installed.clone()
    }

    /// An Activity entry, with the backup "Restore" puts back.
    pub fn log_activity(&self, kind: ActivityKind, title: String, meta: Option<String>, restore_id: Option<String>) {
        let mut e = store::new_activity(kind, title, meta, None, None);
        e.restore_id = restore_id;
        store::append_activity(&e);
        self.emit("activity", e);
    }

    pub fn apps(&self) -> Vec<AppEntry> {
        let running = procs::list();
        let sizes = self.adobe_sizes.lock().clone();
        let recorded = self.persisted.lock().updated_at.clone();
        let driver_list: Vec<drivers::Found> = if self.settings.read().sources.drivers { self.drivers.lock().found.clone() } else { vec![] };
        let ignored = self.settings.read().ignored.clone();
        let pre = self.settings.read().pre_releases.clone();
        let craft_pre = self.settings.read().craft_prereleases;
        let gh = self.ghapps.read().clone();
        let cf = self.craft.read().clone();
        let tf = self.tools.read().clone();
        let opre = self.other_pre.read().clone();
        let me = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| norm_dir(d)));
        let self_ready = self.self_release.lock().as_ref().map(|r| r.version.clone());
        let st = self.st.lock();
        let jobs = self.jobs.lock();
        let mut out = Vec::new();
        for i in &st.installed {
            // An Other program whose GitHub releases were read: updated like Other apps.
            let gh_feed = gh.as_deref().filter(|f| i.source == Source::Programs && f.get(&i.id).is_some());
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
            } else if latest.is_none() && !i.read_only && i.source != Source::Programs {
                AppStatus::Unknown
            } else {
                AppStatus::Current
            };
            let is_running = i.install_dir.as_ref().is_some_and(|d| !procs::blocking(&running, std::slice::from_ref(d), &[]).is_empty());
            let note = if i.missing && i.source == Source::Programs {
                Some("Its uninstaller is missing. Remove from list cleans up what is left.".into())
            } else if i.missing {
                Some("Files are missing. Repair reinstalls it.".into())
            } else if i.read_only {
                i.tagline.clone()
            } else if has_update {
                latest.and_then(|l| l.note.clone())
            } else {
                None
            };
            let can_rollback = !i.read_only && (i.source != Source::Programs || gh_feed.is_some_and(|f| ghapps::can_roll_back(i, f))) && !matches!(i.data, SourceData::Blender { kind: sources::BlenderKind::Beta, .. });
            let icon = app_icon(i, &st.icons, &st.catalog_icons);
            out.push(AppEntry {
                id: i.id.clone(),
                source: i.source,
                name: i.name.clone(),
                code: i.code.clone(),
                fg: i.fg.clone(),
                bg: i.bg.clone(),
                icon_url: icon.clone(),
                icon_miss: icon.is_none() && st.icon_none.contains(&i.id) && (i.source != Source::Other || st.icon_none.contains(&format!("other:{}", other::pkg_of(i)))),
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
                updated_at: newest_update(recorded.get(&i.id).map(String::as_str), i.updated_at.as_deref()),
                // A GitHub program: its project's own description.
                tagline: gh_feed.and_then(|f| f.about_of(&i.id)).map(String::from).or_else(|| i.tagline.clone()),
                group: if gh_feed.is_some() { Source::Other.group().into() } else { i.source.group().into() },
                running: is_running,
                can_uninstall: !i.read_only && (i.uninstall.is_some() || i.source == Source::Programs || other::package_family(i).is_some() || sources::portable::is_portable(i) || matches!(&i.data, SourceData::Other { entry, .. } if !entry.uninstall_string.trim().is_empty() || !entry.quiet_uninstall_string.trim().is_empty()) || matches!(i.data, SourceData::Blender { .. }) || matches!(&i.data, SourceData::Tools { key, .. } if key == "godot")),
                can_rollback,
                read_only: i.read_only,
                // Only for copies the Store updates: a sideloaded app package is updated here.
                store_url: i.store.as_ref().filter(|_| i.read_only).map(|s| s.page_url()),
                channel: i.channel.clone(),
                manual_url: gh_feed.filter(|_| has_update).and_then(|f| ghapps::manual_page(i, f, pre.get(&i.id).copied())),
                github: gh_feed.and_then(|f| f.get(&i.id)).map(|(t, _)| t.repo.clone()),
                is_self: false,
                studio: None,
                pre_channel: {
                    let choice = pre.get(&i.id).copied();
                    match i.source {
                        Source::Craft => cf.as_deref().and_then(|f| craft::follows_pre(i, f, choice, craft_pre)),
                        Source::Tools => tf.as_deref().and_then(|f| tools::follows_pre(i, &st.installed, f, choice)),
                        Source::Programs => gh.as_deref().and_then(|f| ghapps::follows_pre(i, f, choice)),
                        // Its GitHub pre-releases show in its version list, which offers the
                        // choice; a copy that is one follows them until the user chooses.
                        Source::Other => choice.or_else(|| opre.get(&i.id).filter(|(_, copy)| *copy).map(|_| true)),
                        _ => None,
                    }
                },
                note,
                job_id: job.map(|j| j.id.clone()),
            });
            // This app's own entry: listed like the others, updated its own way.
            if i.source == Source::Programs && me.is_some() && i.install_dir.as_deref().map(norm_dir) == me {
                if let Some(a) = out.last_mut() {
                    mark_self(a, self_ready.as_deref());
                }
            }
        }
        for f in &driver_list {
            let e = &f.entry;
            let job = jobs.iter().rev().find(|j| j.job.app_id == e.id && !j.job.state.is_finished()).map(|j| &j.job);
            let status = match job.map(|j| j.state) {
                Some(JobState::Waiting) => AppStatus::Queued,
                Some(JobState::Blocked) => AppStatus::Blocked,
                Some(_) => AppStatus::Updating,
                None if st.failed.contains_key(&e.id) => AppStatus::Failed,
                None => e.status,
            };
            let mut d = driver_app(e, status, f.how.is_some(), recorded.get(&e.id).cloned(), job.map(|j| j.id.clone()));
            d.icon_url = sources::makers::site_of(&e.maker).and_then(|m| st.maker_icons.get(m.key)).cloned();
            out.push(d);
        }
        skip_ignored(&mut out, &ignored);
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
            // Every other program in Windows' list, to uninstall it here (a broken one, whose
            // uninstaller is gone, is listed to remove it from the list).
            let programs = crate::uninstall::programs::scan(&un, &v);
            v.extend(programs);
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

    /// Look for driver updates: every source at the same time, and the list shown as soon as the
    /// makers answered (Windows Update, the slow one, follows). Runs with each update check.
    pub async fn check_drivers(self: &Arc<Self>) {
        if !self.settings.read().sources.drivers {
            return;
        }
        {
            let mut d = self.drivers.lock();
            if d.checking {
                return;
            }
            d.checking = true;
        }
        let t0 = Instant::now();
        let wu_task = tauri::async_runtime::spawn(drivers::fetch());
        let (present, board, chipset_have) = tauri::async_runtime::spawn_blocking(|| (drivers::present(), board::read(), chipset::installed())).await.unwrap_or_default();
        let win11 = drivers::windows_11();
        let gpus: Vec<(String, String)> = present.iter().filter(|p| sources::nvidia::is_gpu(p)).map(|p| (p.matching_id.clone(), p.desc.clone())).collect();
        let nvidia_f = async {
            if gpus.is_empty() {
                return Ok(vec![]);
            }
            let products = sources::nvidia::products().await?;
            // Each card at the same time.
            let asks = gpus.iter().map(|(matching, desc)| {
                let pfid = sources::nvidia::product_id(desc, &products).unwrap_or_default();
                async move {
                    let (rel, beta, studio) = if pfid.is_empty() {
                        (None, None, None)
                    } else {
                        let (rel, beta, studio) = tokio::join!(sources::nvidia::latest(&pfid), sources::nvidia::latest_beta(&pfid), sources::nvidia::latest_studio(&pfid));
                        (rel.map_err(|e| log::warn!("nvidia lookup {desc}: {e:#}")).ok(), beta, studio)
                    };
                    ((matching.clone(), pfid.clone(), rel), beta.map(|b| (pfid.clone(), b)), studio.map(|s| (pfid, s)))
                }
            });
            anyhow::Ok(futures_util::future::join_all(asks).await)
        };
        // Adrenalin for AMD cards, AMD Software: PRO Edition (from the card's own page) for each Radeon PRO card.
        let has_amd = present.iter().any(|p| sources::amd::is_gpu(p) && !sources::amd::is_pro(&p.desc));
        let amd_f = async { if has_amd { sources::amd::latest(win11).await.map(Some) } else { Ok(None) } };
        let pro_cards: Vec<(String, String)> = present.iter().filter(|p| sources::amd::is_gpu(p) && sources::amd::is_pro(&p.desc)).map(|p| (p.matching_id.clone(), p.desc.clone())).collect();
        let pro_f = futures_util::future::join_all(pro_cards.iter().map(|(matching, desc)| async move {
            let rel = sources::amd::latest_pro(desc).await.map_err(|e| log::warn!("amd pro lookup {desc}: {e:#}")).ok().flatten();
            (matching.clone(), rel)
        }));
        // AMD's RAID and NPU drivers, for a PC that has a RAID controller or an NPU.
        let raid_have = chipset::raid_installed(&present);
        let npu_f = async { if present.iter().any(sources::npu::is_npu) { sources::npu::latest().await.map(Some) } else { Ok(None) } };
        // Intel's list covers Intel graphics, Wi-Fi, Bluetooth and network adapters (and Killer).
        let has_intel = present.iter().any(|p| {
            let id = p.matching_id.to_ascii_lowercase();
            id.contains("ven_8086") || id.contains("vid_8087")
        });
        let intel_f = async { if has_intel { sources::intel::fetch().await.map(Some) } else { Ok(None) } };
        let product = board.as_ref().map(|b| b.product.clone()).unwrap_or_default();
        let chipset_f = async { if chipset_have.is_some() { chipset::latest(&product).await.map(Some) } else { Ok(None) } };
        let raid_f = async { if raid_have.is_some() { chipset::latest_raid(&product).await } else { Ok(None) } };
        // The BIOS and the drivers the board's or PC's maker lists, at the same time.
        let bios_board = board.clone();
        let bios_f = async {
            match bios_board {
                Some(b) => {
                    let (bios, list) = tokio::join!(board::check(b.clone()), board::drivers(&b));
                    anyhow::Ok(Some((bios, list)))
                }
                None => Ok(None),
            }
        };
        let (nv, amd, intel, chip, bios, amd_pro, raid, npu) = tokio::join!(nvidia_f, amd_f, intel_f, chipset_f, bios_f, pro_f, raid_f, npu_f);
        let warn = |what: &str, e: anyhow::Error| log::warn!("{what} drivers: {e:#}");
        let nv = nv.unwrap_or_else(|e| {
            warn("nvidia", e);
            vec![]
        });
        let (nv, nv_beta, nv_studio): (Vec<drivers::NvidiaAnswer>, Vec<_>, Vec<_>) = nv.into_iter().fold((vec![], vec![], vec![]), |(mut a, mut b, mut c), (x, y, z)| {
            a.push(x);
            b.extend(y);
            c.extend(z);
            (a, b, c)
        });
        let amd = amd.unwrap_or_else(|e| {
            warn("amd", e);
            None
        });
        let intel = intel.unwrap_or_else(|e| {
            warn("intel", e);
            None
        });
        let chip = chip.unwrap_or_else(|e| {
            warn("amd chipset", e);
            None
        });
        let raid = raid.unwrap_or_else(|e| {
            warn("amd raid", e);
            None
        });
        let npu = npu.unwrap_or_else(|e| {
            warn("amd npu", e);
            None
        });
        let (bios, maker_list) = match bios.unwrap_or(None) {
            Some((b, l)) => (Some(b), l),
            None => (None, vec![]),
        };
        let maker_name = bios.as_ref().and_then(|b| b.maker).map(|m| m.name()).unwrap_or_default();
        let pre = self.settings.read().pre_releases.clone();
        let studio = self.settings.read().driver_studio.clone();
        let inputs = |wu: Option<&drivers::Feed>| {
            drivers::collect(&drivers::Inputs {
                wu,
                present: &present,
                nvidia: &nv,
                amd: amd.as_ref(),
                intel: intel.as_deref(),
                chipset: chipset_have.as_deref().map(|h| (h, chip.as_ref())),
                raid: raid_have.as_deref().zip(raid.as_ref()),
                npu: npu.as_ref(),
                amd_pro: &amd_pro,
                bios: bios.as_ref(),
                maker: &maker_list,
                maker_name,
                windows11: win11,
                pre: Some(&pre),
                nvidia_beta: &nv_beta,
                studio: Some(&studio),
                nvidia_studio: &nv_studio,
            })
        };
        // The makers answered: show that now, with Windows Update's last answer.
        let before = self.drivers.lock().wu.clone();
        let first = inputs(before.as_ref());
        log::info!("timing: drivers from the makers in {} ms", t0.elapsed().as_millis());
        self.drivers.lock().found = first;
        self.emit_apps();
        self.spawn_maker_icons();
        let wu = match wu_task.await {
            Ok(Ok(f)) => Some(f),
            Ok(Err(e)) => {
                warn("windows update", e);
                None
            }
            Err(e) => {
                warn("windows update", anyhow!("{e}"));
                None
            }
        };
        let found = inputs(wu.as_ref());
        log::info!("timing: driver check found {} drivers in {} ms", found.len(), t0.elapsed().as_millis());
        let saved = {
            let mut d = self.drivers.lock();
            d.found = found;
            d.checking = false;
            d.last_check = Some(now());
            if wu.is_some() {
                d.wu = wu;
            }
            drivers::Saved { found: d.found.clone(), last_check: d.last_check.clone(), wu: d.wu.clone() }
        };
        let _ = tauri::async_runtime::spawn_blocking(move || drivers::save(&drivers::saved_file(), &saved)).await;
        self.emit_apps();
        self.spawn_maker_icons();
    }

    /// The versions a driver's page shows: the installed one and the newest, with what is new.
    fn driver_versions(&self, id: &str) -> Vec<VersionInfo> {
        let d = self.drivers.lock();
        let Some(f) = d.found.iter().find(|f| f.entry.id == id) else { return vec![] };
        let e = &f.entry;
        let newest = e.latest.clone().filter(|l| l != &e.version);
        let mut out = Vec::new();
        if let Some(l) = newest {
            out.push(VersionInfo { version: l, date: e.date.clone(), notes: e.notes.clone(), notes_url: e.notes_url.clone(), size: e.size, installed: false, latest: true, prerelease: false });
        }
        let current = e.latest.as_deref() == Some(e.version.as_str());
        out.push(VersionInfo {
            version: e.version.clone(),
            date: if current { e.date.clone() } else { None },
            notes: if current { e.notes.clone() } else { vec![] },
            notes_url: if current { e.notes_url.clone() } else { None },
            size: None,
            installed: true,
            latest: current,
            prerelease: false,
        });
        out
    }

    /// The maker's page for a driver that is updated from there (a BIOS), or the maker's own
    /// updater for a card whose maker was not reached.
    pub fn driver_page(&self, id: &str) -> Option<String> {
        // An Other program's newest release that only downloads from its GitHub page.
        if id.starts_with("prog:") {
            let inst = self.installed(id)?;
            let feed = self.ghapps.read().clone()?;
            let pre = self.settings.read().pre_releases.get(id).copied();
            return ghapps::manual_page(&inst, &feed, pre).filter(|u| drivers::host_is(u, &["github.com"]));
        }
        self.drivers.lock().found.iter().find(|f| f.entry.id == id).and_then(|f| drivers::safe_link(f.entry.vendor_url.clone()))
    }

    /// A driver job finished: show the new version until the next check, and ask for a restart
    /// when the driver needs one to finish.
    fn driver_installed(&self, id: &str, restart: bool) {
        let saved = {
            let mut d = self.drivers.lock();
            if let Some(f) = d.found.iter_mut().find(|f| f.entry.id == id) {
                if let Some(l) = f.entry.latest.clone() {
                    f.entry.version = l;
                }
                f.entry.status = AppStatus::Current;
                f.entry.size = None;
                f.entry.note = Some(if restart { "Installed. Restart your PC to finish." } else { "Installed." }.into());
                f.how = None;
            }
            drivers::Saved { found: d.found.clone(), last_check: d.last_check.clone(), wu: d.wu.clone() }
        };
        drivers::save(&drivers::saved_file(), &saved);
        if restart {
            let first = !self.status().restart_needed;
            self.set_status(|s| s.restart_needed = true);
            if first {
                self.notify("Restart to finish installing drivers", "Open The Updater 3D to restart now, or restart later yourself.", Target::Home);
            }
        }
        self.emit_apps();
    }

    /// Restart the PC in a minute (Windows shows its own notice; `shutdown /a` stops it).
    pub fn restart_pc(&self) -> Result<()> {
        let mut c = winutil::cmd("shutdown.exe");
        c.args(["/r", "/t", "60", "/c", "The Updater 3D: restarting to finish installing drivers. Save your work."]);
        let out = c.output()?;
        if !out.status.success() {
            return Err(anyhow!("Windows did not start the restart (exit code {})", out.status.code().unwrap_or(-1)));
        }
        Ok(())
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
            .buffer_unordered(crate::machine::tuned().size_parallel)
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
        let gf = self.ghapps.read().clone();
        let keep_patch = self.settings.read().blender_keep_patch_versions;
        let pre = self.settings.read().pre_releases.clone();
        let opre = self.other_pre.read().clone();
        let mut st = self.st.lock();
        let mut latest = HashMap::new();
        for i in &installed {
            let l = match i.source {
                Source::Adobe => cat.as_ref().and_then(|c| adobe::latest(i, &installed, c)),
                Source::Craft => cf.as_ref().and_then(|f| craft::latest_with(i, f, pre.get(&i.id).copied())),
                Source::Blender => bf.as_ref().and_then(|f| blender::latest_kept(i, &installed, f, keep_patch)),
                Source::Tools => tf.as_ref().and_then(|f| tools::latest_with(i, &installed, f, pre.get(&i.id).copied())),
                // An app that follows pre-releases is offered its newest GitHub pre-release when
                // that is newer than the catalog's version and the copy here.
                Source::Other => other_offer(of.as_ref().and_then(|f| other::latest(i, f)), opre.get(&i.id).filter(|(_, copy)| pre.get(&i.id).copied().unwrap_or(*copy)).map(|(l, _)| l), &i.version),
                // Listed to uninstall them; no catalog knows their versions, their GitHub project may.
                Source::Programs => gf.as_ref().and_then(|f| ghapps::latest(i, f, pre.get(&i.id).copied())),
                // Not in the installed list: the driver check has their versions.
                Source::Drivers => None,
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

    /// Recompute statuses with the feeds known so far and show them (during a check, as each
    /// source answers). The Other apps feed also changes what is matched, so it waits for the
    /// full rescan at the end.
    fn show_versions_now(&self) {
        let installed = self.st.lock().installed.clone();
        self.recompute_latest(installed);
        self.emit_apps();
    }

    /// After an Other app was installed or updated: make sure the feed has its package (one
    /// installed from a search result is not in it until the next refresh), so the app shows
    /// its latest version at once instead of "Latest version unknown".
    async fn add_to_other_feed(self: Arc<Self>, app_id: &str) {
        let rest = app_id.strip_prefix("other:").unwrap_or(app_id);
        let pkg = rest.strip_suffix(":user").unwrap_or(rest).to_string();
        let Some(feed) = self.other.read().clone() else { return };
        if feed.get(&pkg).is_none() {
            let Some(pf) = other::read_package(&pkg).await else { return };
            *self.other.write() = Some(Arc::new(feed.with_package(pf)));
        }
        let installed = self.st.lock().installed.clone();
        self.recompute_latest(installed);
        self.emit_apps();
    }

    async fn refresh_icons_and_sizes(self: Arc<Self>) {
        let installed = self.st.lock().installed.clone();
        let mut changed = false;
        // apps whose own files gave no icon: looked up on the web afterwards
        let mut later: Vec<Installed> = Vec::new();
        for i in &installed {
            if self.st.lock().icons.contains_key(&i.id) {
                continue;
            }
            let mut url = None;
            // An app package's logo, a file in its folder (the webview may not read that folder).
            if let Some(local) = i.icon_remote.as_deref().filter(|r| !r.starts_with("http")) {
                let src = PathBuf::from(local);
                let out = paths::icons().join(format!("{}.png", sanitize(&format!("package-{}", i.id))));
                let ok = out.is_file() || std::fs::copy(&src, &out).is_ok();
                if ok {
                    url = Some(paths::asset_url(&out));
                }
            } else if let Some(remote) = &i.icon_remote {
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
            if url.is_none() {
                if let SourceData::Other { entry, .. } = &i.data {
                    url = sources::iconfetch::program_here((**entry).clone()).await.map(|(p, _)| paths::asset_url(&p));
                }
            }
            match url {
                Some(u) => {
                    let mut st = self.st.lock();
                    st.icon_none.remove(&i.id);
                    st.icons.insert(i.id.clone(), u);
                    changed = true;
                }
                None => later.push(i.clone()),
            }
        }
        if changed {
            self.emit_apps();
        }
        // What the PC has nothing for: Steam's store, the publisher's site. A few at a time; each
        // answer is kept (or remembered as none), so a site or the store is asked once.
        if !later.is_empty() {
            let gate = Arc::new(tokio::sync::Semaphore::new(4));
            let mut set = tokio::task::JoinSet::new();
            for i in later {
                let (me, gate) = (self.clone(), gate.clone());
                set.spawn(async move {
                    let _turn = gate.acquire().await;
                    let found = match &i.data {
                        SourceData::Other { entry, .. } => me.program_icon_web(&i.id, (**entry).clone()).await,
                        _ => None,
                    };
                    (i.id, found)
                });
            }
            while let Some(r) = set.join_next().await {
                let Ok((id, found)) = r else { continue };
                let mut st = self.st.lock();
                match found {
                    Some(p) => {
                        st.icon_none.remove(&id);
                        st.icons.insert(id, paths::asset_url(&p));
                    }
                    // the search is over: the card shows its painted mark, not a shimmer
                    None => {
                        st.icon_none.insert(id);
                    }
                }
            }
            self.emit_apps();
        }
        // Sizes on disk: remembered per version, measured once (can take a moment for big apps).
        let mut forgot = false;
        for i in &installed {
            let remembered = remembered_size(&self.persisted.lock().sizes, i);
            match remembered {
                Some(sz) => {
                    self.st.lock().sizes.insert(i.id.clone(), sz);
                }
                // Measured from a folder the app no longer has: not its size.
                None if i.install_dir.is_none() => {
                    forgot |= self.persisted.lock().sizes.remove(&format!("{}@{}", i.id, i.version)).is_some();
                    self.st.lock().sizes.remove(&i.id);
                }
                None => {}
            }
        }
        if forgot {
            store::save_state(&self.persisted.lock());
        }
        self.emit_apps();
        let mut measured = false;
        for i in &installed {
            if self.st.lock().sizes.contains_key(&i.id) || i.missing {
                continue;
            }
            // Other apps report their size in the uninstall list; their folders can be huge
            // (a launcher with all it downloaded), so they are not walked.
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
        if !s.install_root.trim().is_empty() {
            roots.push(PathBuf::from(s.install_root.trim()));
        }
        // Every fixed drive, for the free space Settings and Get apps show.
        roots.extend(winutil::fixed_drives());
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
        let r = coalesced(&self.check_lock, &self.check_again, || self.check_once(manual)).await.unwrap_or(Ok(()));
        // A check in the background leaves pages and lists behind that nothing shows.
        if tauri::Manager::get_webview_window(&self.app, "main").is_none() {
            self.trim_memory();
        }
        r
    }

    /// With no window open and nothing in the queue, drop what only the window uses (fetched notes
    /// pages, change lists, the whole-catalog list) and give the memory back to Windows. The
    /// checker keeps its feeds, so the next check needs no more network than before.
    pub fn trim_memory(&self) {
        if self.jobs.lock().iter().any(|j| !j.job.state.is_finished()) {
            return;
        }
        crate::notes::clear_page_cache();
        other::clear_notes_cache();
        sources::browse::drop_list();
        tauri::async_runtime::spawn_blocking(crate::winutil::give_back_memory);
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
        // Drivers are checked next to the apps: the apps never wait for Windows Update.
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.check_drivers().await });
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
                Ok(c) => {
                    let c = Arc::new(c);
                    *self.adobe.write() = Some(c.clone());
                    self.show_versions_now();
                    Some(c)
                }
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
                Ok(f) => {
                    let f = Arc::new(f);
                    *self.craft.write() = Some(f.clone());
                    self.show_versions_now();
                    Some(f)
                }
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
                Ok(f) => {
                    let f = Arc::new(f);
                    *self.blender.write() = Some(f.clone());
                    self.show_versions_now();
                    Some(f)
                }
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
                Ok(f) => {
                    let f = Arc::new(f);
                    *self.tools.write() = Some(f.clone());
                    self.show_versions_now();
                    Some(f)
                }
                Err(e) => {
                    log::warn!("more apps feed: {e:#}");
                    None
                }
            }
        };
        let prev_other = self.other.read().clone();
        let other_f = async {
            if !settings.sources.other {
                return None;
            }
            match other::fetch(&catalog_apps(&installed), prev_other.as_deref()).await {
                Ok(f) => Some(Arc::new(f)),
                Err(e) => {
                    log::warn!("other apps: {e:#}");
                    None
                }
            }
        };
        // Other programs that name their GitHub project (the list from the scan above).
        let gh_f = async {
            if !settings.sources.other {
                return None;
            }
            Some(Arc::new(ghapps::fetch(&installed).await))
        };
        let (a, c, b, t, o, g) = tokio::join!(adobe_f, craft_f, blender_f, tools_f, other_f, gh_f);
        *self.ghapps.write() = g;
        if a.is_some() {
            *self.adobe.write() = a;
        } else if settings.sources.adobe {
            unknown.insert("adobe");
        } else {
            // Adobe is off: no sizes or icons are fetched from it for an old catalog.
            *self.adobe.write() = None;
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
        self.refresh_other_pre().await;
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
        // This app's own update is told about by its own check (see `check_self`).
        let updates: Vec<&AppEntry> = apps.iter().filter(|a| a.status == AppStatus::Update && !a.is_self).collect();
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
        let told = told_updates(&updates, &settings);
        let fresh: Vec<&&AppEntry> = {
            let p = self.persisted.lock();
            told.iter().filter(|a| p.notified.get(&a.id) != a.latest.as_ref()).collect()
        };
        if !fresh.is_empty() && settings.notify_updates && !manual {
            let fresh_names: Vec<String> = fresh.iter().map(|a| a.name.clone()).collect();
            let (title, body) = update_notice(told.len(), &fresh_names);
            let ids: Vec<&str> = told.iter().map(|a| a.id.as_str()).collect();
            self.notify(&title, &body, notice::updates_target(&ids));
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
            self.auto_install(Some(&apps));
        }
        if online {
            if let Err(e) = self.check_self(settings.notify_updates).await {
                log::warn!("self update check: {e:#}");
            }
        }
        Ok(())
    }

    /// Is there a newer version of this app? Only this app: Settings > About asks this alone.
    /// Returns the newer version, if there is one.
    pub async fn check_self(&self, notify: bool) -> Result<Option<String>> {
        if !crate::selfupdate::enabled() {
            return Ok(None);
        }
        match crate::selfupdate::check().await? {
            Some(r) => {
                let v = r.version.clone();
                let first = self.self_release.lock().as_ref().map(|x| x.version.clone()) != Some(v.clone());
                self.offer_self_update(r);
                if first && notify {
                    self.notify(&format!("The Updater 3D {v} is ready"), "Open the app to update it.", Target::About);
                }
                Ok(Some(v))
            }
            // Nothing newer any more: an offer found before is not kept.
            None => {
                self.forget_self_update();
                Ok(None)
            }
        }
    }

    /// A newer version of this app to offer.
    pub fn offer_self_update(&self, r: crate::selfupdate::Release) {
        let v = r.version.clone();
        *self.self_release.lock() = Some(r);
        self.set_status(|s| s.self_update = Some(v));
    }

    /// No newer version of this app to offer.
    pub fn forget_self_update(&self) {
        if self.self_release.lock().take().is_some() {
            self.set_status(|s| s.self_update = None);
        }
    }

    /// Queue the updates set to install automatically, when it is time for them (see
    /// `auto_install_due`). `apps`: the list a check just made; None on the scheduler's tick.
    fn auto_install(self: &Arc<Self>, apps: Option<&[AppEntry]>) {
        let s = self.settings.read().clone();
        if s.auto_mode != AutoMode::Install && !s.exceptions.values().any(|m| *m == AutoMode::Install) {
            return;
        }
        let last = self.persisted.lock().last_auto_install_day.clone();
        let Some(day) = auto_install_due(chrono::Local::now().naive_local(), &s.window_start, &s.window_end, last.as_deref(), apps.is_some()) else { return };
        {
            let mut p = self.persisted.lock();
            p.last_auto_install_day = Some(day.to_string());
            store::save_state(&p);
        }
        let listed;
        let apps = match apps {
            Some(a) => a,
            None => {
                listed = self.apps();
                &listed
            }
        };
        let mut queued = 0;
        // Drivers ask for admin rights and some restart the screen; a game or a release that
        // only downloads from its page is updated there.
        for a in apps.iter().filter(|a| a.status == AppStatus::Update && a.source != Source::Drivers && a.manual_url.is_none()) {
            if auto_mode_of(&s, &a.id) != AutoMode::Install || a.running {
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
        if self.persisted.lock().last_check.is_some() {
            self.set_status(|s| {
                s.first_scan = false;
                s.scan_step = None;
            });
        }
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
            let cc_down = self.settings.read().sources.adobe && self.st.lock().status.as_ref().is_some_and(|s| s.cc_installed && !s.cc_running);
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
            if minute % 5 == 0 && !self.jobs.lock().iter().any(rescan_waits_for) {
                self.rescan().await;
            }
            let hours = self.settings.read().check_interval;
            if hours != 0 {
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
            // Updates a check found before the install window opened are installed in it.
            self.auto_install(None);
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
        drop(tauri::async_runtime::spawn_blocking(|| forget_game_data(&paths::data_dir(), &paths::icons())));
        let settings = self.settings.read().clone();
        self.rescan().await;
        let installed = self.st.lock().installed.clone();
        let (a, c, b, t, o) = tokio::join!(
            async { if settings.sources.adobe { adobe::fetch_catalog().await.ok() } else { None } },
            async { if settings.sources.craft { craft::fetch(settings.craft_prereleases).await.ok() } else { None } },
            async { if settings.sources.blender { blender::fetch(&installed, settings.blender_channel == "beta").await.ok() } else { None } },
            async { if settings.sources.tools { tools::fetch(&installed).await.ok() } else { None } },
            async { if settings.sources.other { other::fetch(&catalog_apps(&installed), None).await.ok() } else { None } },
        );
        let g = if settings.sources.other { Some(Arc::new(ghapps::fetch(&installed).await)) } else { None };
        crate::http::PREFER_CACHE.store(false, Ordering::Relaxed);
        *self.ghapps.write() = g;
        if settings.sources.other && crate::machine::tuned().prebuild_browse && self.window_open() {
            // The whole catalog list for Other apps, built now so the first page in Get apps is
            // quick. Not while only the tray runs (it would only be dropped again), nor on a low
            // end PC, where Get apps builds it when it opens.
            tauri::async_runtime::spawn_blocking(|| {
                let _ = sources::browse::list();
            });
        }
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
        self.refresh_other_pre().await;
        self.rescan().await;
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.cache_catalog_icons().await });
        self.spawn_other_followups();
        self.spawn_maker_icons();
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
            let prev = self.other.read().clone();
            if let Ok(f) = other::fetch(&catalog_apps(&installed), prev.as_deref()).await {
                *self.other.write() = Some(Arc::new(f));
                self.emit("catalog_changed", ());
                self.clone().cache_other_icons(None).await;
            }
        }
    }

    /// Sort package ids by what is known of their icons: in memory, a small file saved by an
    /// earlier run (read here, off the lock), a file of an earlier version to make small first,
    /// none found a short while ago, or not looked for yet.
    async fn sort_icons(self: &Arc<Self>, ids: Vec<String>) -> IconsNow {
        let mut now = IconsNow::default();
        let mut rest: Vec<String> = Vec::new();
        {
            let st = self.st.lock();
            for id in ids {
                let key = format!("other:{id}");
                match st.catalog_icons.get(&key).or_else(|| st.icons.get(&key)) {
                    Some(u) => now.known.push(IconFound { id: key, icon_url: Some(u.clone()) }),
                    None => rest.push(id),
                }
            }
        }
        if rest.is_empty() {
            return now;
        }
        let probed = tauri::async_runtime::spawn_blocking(move || rest.into_iter().map(|id| (other_icon_on_disk(&id), id)).collect::<Vec<_>>()).await.unwrap_or_default();
        let today = chrono::Local::now().date_naive();
        let recent_miss: HashSet<String> = {
            let p = self.persisted.lock();
            probed
                .iter()
                .filter(|(file, id)| file.is_none() && p.icon_misses.get(id).and_then(|d| icon_miss_day(d)).is_some_and(|d| (today - d).num_days() < ICON_RETRY_DAYS))
                .map(|(_, id)| id.clone())
                .collect()
        };
        let mut st = self.st.lock();
        for (file, id) in probed {
            let key = format!("other:{id}");
            match file {
                Some(p) if icon_is_small(&p) => {
                    let url = paths::asset_url(&p);
                    st.catalog_icons.insert(key.clone(), url.clone());
                    now.disk.push(IconFound { id: key, icon_url: Some(url) });
                }
                // shown once it is small, so the window never gets the address of a file that is replaced
                Some(_) => now.convert.push(id),
                None if recent_miss.contains(&id) => {
                    st.icon_none.insert(key);
                    now.none.push(id);
                }
                None if sources::icons::may_search(&id) => now.search.push(id),
                // searched a moment ago without an answer: its tile shows until the search is tried again
                None => {
                    st.icon_none.insert(key);
                    now.none.push(id);
                }
            }
        }
        now
    }

    /// Make icons saved by an earlier version at full size small, a few at a time and apart from
    /// the web search; each one reaches the window when it is done.
    fn convert_icons(self: &Arc<Self>, ids: Vec<String>) {
        let ids: Vec<String> = {
            let mut busy = ICON_CONVERTING.lock();
            ids.into_iter().filter(|i| busy.insert(i.to_ascii_lowercase())).collect()
        };
        if ids.is_empty() {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            futures_util::StreamExt::for_each_concurrent(futures_util::stream::iter(ids), 4, |id| {
                let me = me.clone();
                async move {
                    let made = convert_old_icon(&id).await;
                    ICON_CONVERTING.lock().remove(&id.to_ascii_lowercase());
                    if let Some(p) = made {
                        let (key, url) = (format!("other:{id}"), paths::asset_url(&p));
                        me.st.lock().catalog_icons.insert(key.clone(), url.clone());
                        ICON_OUT.lock().found.push(IconFound { id: key, icon_url: Some(url) });
                        me.start_icon_flusher();
                    }
                }
            })
            .await;
        });
    }

    /// The window shows these Other apps cards and keeps those around them: the icons of the cards
    /// on screen are searched for first, then the ones around them, and searches that waited for
    /// places that scrolled away are dropped. Returns the icons known right now (and the apps
    /// that were searched for without success), so they show at once.
    pub async fn icons_in_view(self: &Arc<Self>, visible: Vec<String>, near: Vec<String>) -> Vec<IconFound> {
        let pkgs = |v: Vec<String>| -> Vec<String> { v.into_iter().take(240).filter_map(|i| i.strip_prefix("other:").map(String::from)).collect() };
        let (visible, near) = (pkgs(visible), pkgs(near));
        let all: Vec<String> = visible.iter().chain(&near).cloned().collect();
        let now = self.sort_icons(all).await;
        let searching: HashSet<String> = now.search.iter().map(|i| i.to_ascii_lowercase()).collect();
        let of = |v: &[String]| -> Vec<String> { v.iter().filter(|i| searching.contains(&i.to_ascii_lowercase())).cloned().collect() };
        ICON_QUEUE.lock().view(&of(&visible), &of(&near));
        self.convert_icons(now.convert);
        self.start_icon_workers();
        let mut out = now.known;
        out.extend(now.disk);
        out.extend(now.none.into_iter().map(|id| IconFound { id: format!("other:{id}"), icon_url: None }));
        out
    }

    /// Real icons for Other apps entries: the ones in the starter list and the installed ones the
    /// computer has no icon for (`wanted` None, searched after everything else), or the given
    /// package ids (the page of one app: searched first). Found icons are made small and kept in
    /// the icon cache; apps that have none are remembered for a week, apps whose search failed for
    /// a passing reason are searched again a few minutes later. Found icons reach the window in
    /// `other_icons` batches. The cards of the Get apps list are searched for as the window says
    /// what is on screen (`icons_in_view`).
    async fn cache_other_icons(self: Arc<Self>, wanted: Option<Vec<String>>) {
        let front = wanted.is_some();
        let ids: Vec<String> = match wanted {
            Some(w) => w,
            None => match self.other.read().as_deref() {
                Some(f) => f.popular.iter().map(|(p, _)| p.pkg.id.clone()).chain(f.pkgs.values().map(|p| p.pkg.id.clone())).collect(),
                None => return,
            },
        };
        let now = self.sort_icons(ids).await;
        if !now.disk.is_empty() {
            ICON_OUT.lock().found.extend(now.disk);
        }
        self.convert_icons(now.convert);
        {
            let mut q = ICON_QUEUE.lock();
            if front {
                q.front(sources::icons::Tier::Pinned, &now.search);
            } else {
                q.back(&now.search);
            }
        }
        self.start_icon_workers();
    }

    /// Start the workers that take the next app from the queue until it is empty, and the sender
    /// of what they find.
    fn start_icon_workers(self: &Arc<Self>) {
        self.clone().start_icon_flusher();
        let want = ICON_QUEUE.lock().len().min(sources::icons::parallel());
        loop {
            let running = ICON_WORKERS.load(Ordering::SeqCst);
            if running >= want {
                break;
            }
            if ICON_WORKERS.compare_exchange(running, running + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
                let me = self.clone();
                tauri::async_runtime::spawn(async move { me.icon_worker().await });
            }
        }
    }

    async fn icon_worker(self: Arc<Self>) {
        loop {
            loop {
                let next = ICON_QUEUE.lock().pop();
                let Some(id) = next else { break };
                let key = format!("other:{id}");
                // found by another search while this one waited
                if self.st.lock().catalog_icons.contains_key(&key) {
                    continue;
                }
                if !ICON_BUSY.lock().insert(key.clone()) {
                    continue;
                }
                let feed = self.other.read().clone();
                let r = tokio::time::timeout(Duration::from_secs(20), find_other_icon(feed, id.clone())).await.unwrap_or(Err(sources::icons::Miss { passing: true }));
                ICON_BUSY.lock().remove(&key);
                match r {
                    Ok(p) => {
                        let url = paths::asset_url(&p);
                        let old = {
                            let mut st = self.st.lock();
                            st.icon_none.remove(&key);
                            st.catalog_icons.insert(key.clone(), url.clone())
                        };
                        if old.as_deref() != Some(url.as_str()) {
                            ICON_OUT.lock().found.push(IconFound { id: key, icon_url: Some(url) });
                        }
                    }
                    // no answer in time: the card shows its tile now, the search is tried again later
                    Err(m) if m.passing => {
                        sources::icons::retry_later(&id);
                        self.st.lock().icon_none.insert(key.clone());
                        ICON_OUT.lock().found.push(IconFound { id: key, icon_url: None });
                    }
                    Err(_) => {
                        self.st.lock().icon_none.insert(key.clone());
                        let mut o = ICON_OUT.lock();
                        o.found.push(IconFound { id: key, icon_url: None });
                        o.missed.push(id);
                    }
                }
            }
            ICON_WORKERS.fetch_sub(1, Ordering::SeqCst);
            // more apps were queued while this worker was leaving: take a place again if one is free
            let running = ICON_WORKERS.load(Ordering::SeqCst);
            if ICON_QUEUE.lock().is_empty() || running >= sources::icons::parallel() || ICON_WORKERS.compare_exchange(running, running + 1, Ordering::SeqCst, Ordering::SeqCst).is_err() {
                break;
            }
        }
    }

    /// Sends found icons to the window every few tens of milliseconds while the search runs, so a
    /// screenful of icons arrives in a few small updates instead of one per icon, and saves the
    /// apps that have none.
    fn start_icon_flusher(self: Arc<Self>) {
        if ICON_FLUSHING.swap(true, Ordering::SeqCst) {
            return;
        }
        tauri::async_runtime::spawn(async move {
            let mut apps_at = Instant::now();
            let mut apps_dirty = false;
            loop {
                tokio::time::sleep(Duration::from_millis(ICON_FLUSH_MS)).await;
                let (found, missed) = {
                    let mut o = ICON_OUT.lock();
                    (std::mem::take(&mut o.found), std::mem::take(&mut o.missed))
                };
                let idle = ICON_WORKERS.load(Ordering::SeqCst) == 0 && ICON_QUEUE.lock().is_empty() && ICON_CONVERTING.lock().is_empty();
                if !found.is_empty() {
                    // installed apps show only the icons that were found
                    apps_dirty |= found.iter().any(|f| f.icon_url.is_some());
                    self.emit("other_icons", found);
                }
                if !missed.is_empty() {
                    let day = format!("{}/{ICON_MISS_RULES}", chrono::Local::now().date_naive().format("%Y-%m-%d"));
                    let mut p = self.persisted.lock();
                    for id in missed {
                        p.icon_misses.insert(id, day.clone());
                    }
                    store::save_state(&p);
                }
                // installed apps show these icons too; their list is heavier, so it goes less often
                if apps_dirty && (idle || apps_at.elapsed() > Duration::from_secs(3)) {
                    self.emit_apps();
                    apps_dirty = false;
                    apps_at = Instant::now();
                }
                if idle && ICON_OUT.lock().is_empty() {
                    ICON_FLUSHING.store(false, Ordering::SeqCst);
                    // a search queued between the check and here still gets its flusher
                    let more = !ICON_OUT.lock().is_empty() || !ICON_QUEUE.lock().is_empty() || ICON_WORKERS.load(Ordering::SeqCst) > 0 || !ICON_CONVERTING.lock().is_empty();
                    if more && !ICON_FLUSHING.swap(true, Ordering::SeqCst) {
                        continue;
                    }
                    break;
                }
            }
        });
    }

    /// The icons of the makers of the listed drivers, one for each maker (not one per driver):
    /// from the icon cache, else read once from the maker's home page, in the background. A
    /// maker whose site gave none is asked again after a week.
    pub fn spawn_maker_icons(self: &Arc<Self>) {
        if !self.settings.read().sources.drivers {
            return;
        }
        let makers: Vec<String> = self.drivers.lock().found.iter().map(|f| f.entry.maker.clone()).collect();
        let sites: Vec<&'static sources::makers::Site> = {
            let st = self.st.lock();
            let mut asked = MAKERS_ASKED.lock();
            let mut v = Vec::new();
            for m in makers {
                if let Some(site) = sources::makers::site_of(&m) {
                    if !st.maker_icons.contains_key(site.key) && asked.insert(site.key) {
                        v.push(site);
                    }
                }
            }
            v
        };
        if sites.is_empty() {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move { me.cache_maker_icons(sites).await });
    }

    async fn cache_maker_icons(self: Arc<Self>, sites: Vec<&'static sources::makers::Site>) {
        let today = chrono::Local::now().date_naive();
        let recent: HashSet<&'static str> = {
            let p = self.persisted.lock();
            sites.iter().filter(|s| p.icon_misses.get(&format!("maker:{}", s.key)).and_then(|d| icon_miss_day(d)).is_some_and(|d| (today - d).num_days() < ICON_RETRY_DAYS)).map(|s| s.key).collect()
        };
        // a few makers at a time; every maker has its own server
        let gate = Arc::new(tokio::sync::Semaphore::new(4));
        let mut set = tokio::task::JoinSet::new();
        for site in sites {
            let (skip, gate) = (recent.contains(site.key), gate.clone());
            set.spawn(async move {
                let _turn = gate.acquire().await;
                (site.key, sources::iconfetch::maker(site, skip).await)
            });
        }
        let mut got: Vec<(&'static str, Option<std::result::Result<PathBuf, ()>>)> = Vec::new();
        while let Some(r) = set.join_next().await {
            got.extend(r.ok());
        }
        let mut changed = false;
        let mut missed = Vec::new();
        {
            let mut st = self.st.lock();
            for (key, r) in &got {
                if let Some(Ok(p)) = r {
                    st.maker_icons.insert(key.to_string(), paths::asset_url(p));
                    changed = true;
                }
            }
        }
        for (key, r) in &got {
            if let Some(Err(())) = r {
                missed.push(format!("maker:{key}"));
            }
        }
        if !missed.is_empty() {
            let day = format!("{}/{ICON_MISS_RULES}", today.format("%Y-%m-%d"));
            let mut p = self.persisted.lock();
            for id in missed {
                p.icon_misses.insert(id, day.clone());
            }
            store::save_state(&p);
        }
        if changed {
            self.emit_apps();
        }
    }

    /// The icon of a program or game from the web, when its own files gave none (see
    /// `iconfetch::program_web`). A search that found nothing is remembered for a week.
    async fn program_icon_web(self: &Arc<Self>, id: &str, e: crate::winutil::UninstallEntry) -> Option<PathBuf> {
        let today = chrono::Local::now().date_naive();
        let recent = |key: &str| self.persisted.lock().icon_misses.get(key).and_then(|d| icon_miss_day(d)).is_some_and(|d| (today - d).num_days() < ICON_RETRY_DAYS);
        let miss = |key: String| {
            let mut p = self.persisted.lock();
            p.icon_misses.insert(key, format!("{}/{ICON_MISS_RULES}", today.format("%Y-%m-%d")));
            store::save_state(&p);
        };
        let (path, from) = sources::iconfetch::program_web(id, e, &sources::iconfetch::Memory { recent: &recent, miss: &miss }).await?;
        log::debug!("icon of {id}: {from}");
        Some(path)
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

    /// The Get apps list, installed apps included: the page leaves them out unless the setting
    /// asks for them or the user searches (a search for an app you have must find it).
    pub fn get_apps(&self) -> Vec<CatalogEntry> {
        self.catalog()
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
    /// Never at sign-in (no window), and not when the task could not be read.
    pub fn ensure_silent_task(self: &Arc<Self>) {
        static ASKED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let on = self.settings.read().silent_elevation;
        let open = self.window_open();
        if !on || !open || ASKED.load(Ordering::SeqCst) {
            return;
        }
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            // Also when the task starts another copy of the app (an older one cannot run the
            // steps this version sends): register it again for this exe.
            let task = tauri::async_runtime::spawn_blocking(crate::elevate::task_state).await.unwrap_or(crate::elevate::TaskState::Unknown);
            if crate::elevate::should_set_up_task(on, task, open, ASKED.load(Ordering::SeqCst)) && !ASKED.swap(true, Ordering::SeqCst) {
                me.set_up_silent_task(true, true);
            }
        });
    }

    /// Is the main window open? It exists only while it is shown (closing it closes it for
    /// real); started at sign-in the app has none, and nobody may be there to answer a prompt.
    pub fn window_open(&self) -> bool {
        use tauri::Manager;
        self.app.get_webview_window("main").is_some()
    }

    /// "Allow admin rights": ask for the admin prompt once more and continue every job that
    /// waits for it.
    pub fn allow_admin(&self) {
        self.elevator.reset_declined();
        self.admin_gen.send_modify(|g| *g += 1);
    }

    /// The newest GitHub pre-release of each installed Other app that follows them: the user
    /// chose them, or the copy here is one.
    async fn refresh_other_pre(&self) {
        let choices = self.settings.read().pre_releases.clone();
        let feed = self.other.read().clone();
        let found = match feed {
            Some(f) => {
                let follow: Vec<Installed> = self.st.lock().installed.iter().filter(|i| i.source == Source::Other && other::may_follow_pre(i, &f, choices.get(&i.id).copied())).cloned().collect();
                if follow.is_empty() {
                    HashMap::new()
                } else {
                    other::newest_pre(&follow, &f).await
                }
            }
            None => HashMap::new(),
        };
        *self.other_pre.write() = found;
    }

    /// Does an installed Other app follow its GitHub pre-releases? The user's choice, else
    /// whether the copy here is one.
    fn other_follows(&self, id: &str) -> bool {
        let choice = self.settings.read().pre_releases.get(id).copied();
        choice.unwrap_or_else(|| self.other_pre.read().get(id).is_some_and(|(_, copy)| *copy))
    }

    /// The user changed which apps follow pre-releases: their offered versions and version lists
    /// follow at once.
    fn pre_releases_changed(self: &Arc<Self>, changed: Vec<String>) {
        let me = self.clone();
        tauri::async_runtime::spawn(async move {
            for id in &changed {
                notes_cache::forget(&format!("app-{id}"));
            }
            me.refresh_other_pre().await;
            let installed = me.st.lock().installed.clone();
            me.recompute_latest(installed);
            me.emit_apps();
            for id in &changed {
                me.emit("versions-changed", serde_json::json!({ "id": id }));
            }
            // A driver's beta choice: the driver check picks its driver again.
            if changed.iter().any(|id| id.starts_with("driver:")) {
                me.check_drivers().await;
            }
        });
    }

    /// Put back an app's saved settings (Activity's Restore): the app must be closed; its current
    /// settings are saved first, so this can be undone the same way.
    pub async fn restore_settings(self: &Arc<Self>, id: &str) -> Result<()> {
        let root = crate::snapshots::root();
        let s = crate::snapshots::load(&root, id).filter(|s| !s.restored).ok_or_else(|| anyhow!("Those saved settings are gone"))?;
        let running = procs::running_as(&procs::list_fresh(), &s.app);
        if !running.is_empty() {
            return Err(anyhow!("Close {} first, then use Restore", s.app));
        }
        let now_ver = self.installed(&s.app_id).map(|i| i.version).unwrap_or_default();
        let (r, s2) = (root.clone(), s.clone());
        let (current, n) = tauri::async_runtime::spawn_blocking(move || -> Result<(Option<crate::snapshots::Saved>, usize)> {
            let current = crate::snapshots::save_to(&r, &s2.app_id, &s2.app, &now_ver, "", &crate::snapshots::locations_of(&s2))?;
            let n = crate::snapshots::put_back(&r, &s2, crate::snapshots::import_reg)?;
            Ok((current, n))
        })
        .await
        .map_err(|e| anyhow!("{e}"))??;
        let undo = current.map(|c| format!("settings:{}", c.id));
        self.log_activity(ActivityKind::Rollback, format!("Put back the settings of {} {}", s.app, s.version), Some(format!("{n} files{}. The settings it had before are saved.", if s.keys.is_empty() { String::new() } else { " and its registry keys".into() })), undo);
        Ok(())
    }

    /// The other builds of an Other app or catalog package (its beta, preview or nightly
    /// packages), for its page.
    pub async fn other_builds(&self, id: &str) -> Vec<OtherBuild> {
        let pkg = match self.installed(id) {
            Some(i) if i.source == Source::Other => other::pkg_of(&i).to_string(),
            Some(_) => return vec![],
            None => match id.strip_prefix("other:") {
                Some(p) => p.to_string(),
                None => return vec![],
            },
        };
        let path = sources::winget::index_path();
        tauri::async_runtime::spawn_blocking(move || {
            let Ok(idx) = sources::winget::Index::open(&path) else { return vec![] };
            other::other_builds(&idx, &pkg).into_iter().map(|(id, name)| OtherBuild { id: format!("other:{id}"), name }).collect()
        })
        .await
        .unwrap_or_default()
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
            e.icon_miss = e.icon_url.is_none() && st.icon_none.contains(&e.id);
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

    /// One page of the whole winget catalog for Get apps: a search (or none), a category
    /// ("Browsers", ...), a first letter ("A" to "Z", "#") and an order ("popular", "az", "za",
    /// "updated"). Descriptions and icons the page lacks are looked for in the background;
    /// `other_details` and `catalog_changed` bring them.
    pub async fn browse_other(self: &Arc<Self>, req: sources::browse::Request) -> Result<sources::browse::BrowsePage> {
        use sources::browse;
        if !self.settings.read().sources.other {
            return Ok(browse::BrowsePage { total: 0, entries: vec![], packages: 0, categories: vec![], dated: 0 });
        }
        if !sources::winget::index_path().is_file() {
            sources::winget::refresh_index(Duration::from_secs(6 * 3600)).await?;
        }
        let installed = self.st.lock().installed.clone();
        // A search finds installed apps too.
        let show_installed = self.settings.read().show_installed_in_get_apps || req.query.as_deref().is_some_and(|q| !q.trim().is_empty());
        let (page, pkgs) = tauri::async_runtime::spawn_blocking(move || -> Result<(browse::BrowsePage, Vec<sources::winget::Pkg>)> {
            let list = browse::list()?;
            let here: HashSet<String> = installed.iter().filter(|i| i.source == Source::Other).flat_map(|i| other::same_app_ids(other::pkg_of(i))).collect();
            Ok(browse::page_for(&list, &req, &here, show_installed))
        })
        .await
        .map_err(|e| anyhow!("{e}"))??;
        let mut page = page;
        // What is known of the icons goes with the page, so they show at once. The ones still to
        // search for are asked for when their cards are on screen (`icons_in_view`).
        let ids: Vec<String> = page.entries.iter().filter_map(|e| e.id.strip_prefix("other:").map(String::from)).collect();
        let now = self.sort_icons(ids).await;
        let by: HashMap<String, Option<String>> = now.known.into_iter().chain(now.disk).map(|f| (f.id, f.icon_url)).chain(now.none.into_iter().map(|id| (format!("other:{id}"), None))).collect();
        for e in &mut page.entries {
            if let Some(u) = by.get(&e.id) {
                e.icon_url = u.clone();
                e.icon_miss = u.is_none();
            }
        }
        self.convert_icons(now.convert);
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
        let have = self.st.lock().installed.iter().any(|i| i.source == Source::Other && other::same_app_ids(other::pkg_of(i)).contains(&pkg_id.to_ascii_lowercase()));
        let mut e = other::entry(&pkg, pkg.latest.clone(), Some(pkg.id.clone()), have);
        sources::browse::apply(&mut e);
        {
            let st = self.st.lock();
            e.icon_url = st.catalog_icons.get(&e.id).or_else(|| st.icons.get(&e.id)).cloned();
            e.icon_miss = e.icon_url.is_none() && st.icon_none.contains(&e.id);
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
        if app_id.starts_with("driver:") {
            return Ok(self.driver_versions(app_id));
        }
        let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
        if inst.source == Source::Programs {
            // No catalog knows its versions: its GitHub releases, when it names its project.
            let pre = self.settings.read().pre_releases.get(app_id).copied();
            let feed = self.ghapps.read().clone();
            return Ok(match feed {
                Some(f) => ghapps::versions_read(&inst, &f, pre).await,
                None => vec![],
            });
        }
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
            SourceData::Craft { key, .. } => craft::versions(key, Some(&inst.version), s.pre_releases.get(&inst.id).copied().unwrap_or(s.craft_prereleases)).await,
            SourceData::Blender { .. } => {
                let feed = self.blender.read().clone().ok_or_else(|| anyhow!("Blender releases are not loaded yet"))?;
                blender::versions(&inst, &feed).await
            }
            SourceData::Tools { .. } => {
                let feed = self.tools.read().clone().ok_or_else(|| anyhow!("The release lists are not loaded yet"))?;
                let all = self.installed_all();
                tools::versions_with(&inst, &all, &feed, s.pre_releases.get(&inst.id).copied()).await
            }
            SourceData::Other { .. } => {
                let feed = self.other.read().clone().unwrap_or_default();
                other::versions(&inst, &feed, self.other_follows(&inst.id)).await
            }
        }
    }

    // ------------------------------------------------------------ jobs

    pub fn enqueue(self: &Arc<Self>, req: JobRequest) -> Result<String> {
        // A new batch (or a retry) may ask for admin rights again after an earlier "No".
        self.elevator.reset_declined();
        let (app_id, kind, to_version) = match &req {
            JobRequest::Update { app_id, version } if app_id.starts_with("driver:") => {
                if version.is_some() {
                    return Err(anyhow!("Drivers install only their newest version"));
                }
                return self.enqueue(JobRequest::Driver { id: app_id.clone() });
            }
            JobRequest::Update { app_id, version } => {
                let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
                if inst.read_only {
                    return Err(read_only_error(&inst));
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
                if inst.read_only {
                    return Err(read_only_error(&inst));
                }
                // Where it was, for the leftover review once it is gone.
                crate::uninstall::run::remember(&inst);
                (app_id.clone(), JobKind::Uninstall, inst.version)
            }
            JobRequest::Repair { app_id } => {
                let inst = self.installed(app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
                if inst.read_only {
                    return Err(read_only_error(&inst));
                }
                (app_id.clone(), JobKind::Repair, inst.version)
            }
            JobRequest::Driver { id } => {
                let d = self.drivers.lock();
                let f = d.found.iter().find(|f| &f.entry.id == id).ok_or_else(|| anyhow!("That driver is no longer listed. Check again."))?;
                if f.how.is_none() {
                    if f.entry.status == AppStatus::Update && f.entry.vendor_url.is_some() {
                        return Err(anyhow!("{} is updated from {}'s page, not here", f.entry.name, f.entry.maker));
                    }
                    return Err(anyhow!("{} has no update to install", f.entry.name));
                }
                (id.clone(), JobKind::Update, f.entry.latest.clone().unwrap_or_default())
            }
        };
        // Gather everything before taking the jobs lock (lock order is st, then jobs).
        let catalog_entry = self.catalog().into_iter().find(|c| c.id == app_id);
        let driver = self.drivers.lock().found.iter().find(|f| f.entry.id == app_id).map(|f| f.entry.clone());
        let (name, code, fg, bg, from) = match self.installed(&app_id) {
            Some(i) => (i.name, i.code, i.fg, i.bg, Some(i.version)),
            None if driver.is_some() => {
                let e = driver.unwrap();
                let (fg, bg) = driver_colors(&e.maker);
                (e.name, driver_code(&e.maker).into(), fg.into(), bg.into(), Some(e.version))
            }
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
        let seq = turn_seq(kind, jobs.iter().filter(|j| !j.job.state.is_finished()).map(|j| (j.job.kind, j.seq)), || self.next_seq.fetch_add(1, Ordering::Relaxed));
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
                ready: false,
                needs_admin: false,
            },
            req,
            ctl: tx,
            active: false,
            block: Default::default(),
            seq,
            phase: Phase::Queued,
            held: false,
            dests: vec![],
            verify: None,
            restart: false,
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
            JobRequest::Install { id, .. } | JobRequest::Driver { id } => id,
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
        // Downloaded jobs waiting for their install turn swap places among themselves (they
        // keep the places they have, so nothing jumps ahead of a job that is running).
        reorder_in_place(&mut jobs[..], ids, |j| &j.job.id, |j| j.job.ready);
        // Installs waiting for their turn follow the new order too.
        let mut seqs: Vec<u64> = jobs.iter().map(|j| j.seq).collect();
        seqs.sort_unstable();
        for (j, s) in jobs.iter_mut().zip(seqs) {
            j.seq = s;
        }
        let order: HashMap<String, u64> = jobs.iter().map(|j| (j.job.id.clone(), j.seq)).collect();
        drop(jobs);
        self.install_gate.reorder(&order);
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
        let waiting_for = self.with_job(id, |j| j.job.blocked_by.clone()).unwrap_or_default();
        if waiting_for.is_empty() {
            return;
        }
        let me = self.clone();
        let id = id.to_string();
        tauri::async_runtime::spawn(async move {
            let Some((d, n, f)) = me.with_job(&id, |j| j.block.clone()) else { return };
            if d.is_empty() && n.is_empty() && f.is_empty() {
                return;
            }
            let left = tauri::async_runtime::spawn_blocking(move || {
                let pids: Vec<u32> = blockers(&procs::list_fresh(), &d, &n, &f).iter().map(|p| p.pid).collect();
                let mut left = procs::close(&pids, Duration::from_secs(12));
                // A background helper its parent starts again right away (a browser restarts
                // its native messaging host): end the new ones too, for a few seconds.
                let until = Instant::now() + Duration::from_secs(8);
                while left.is_empty() && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(500));
                    let again: Vec<u32> = blockers(&procs::list_fresh(), &d, &n, &f).iter().map(|p| p.pid).filter(|p| !winutil::has_windows(*p)).collect();
                    if !again.is_empty() {
                        left = procs::close(&again, Duration::from_secs(2));
                    }
                }
                left
            })
            .await
            .unwrap_or_default();
            if left.is_empty() {
                return;
            }
            // Still running: it runs as admin, so only the admin helper can end it.
            let mut args = vec!["/F".to_string()];
            for p in &left {
                args.extend(["/PID".to_string(), p.to_string()]);
            }
            let step = sources::Step::Run { exe: PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("taskkill.exe"), args, cwd: None, ok_codes: vec![0, 128], elevated: true, label: "Closing apps".into(), log: None };
            let silent = me.settings.read().silent_elevation;
            let ended = matches!(me.elevator.run(step, silent).await, Ok(r) if r.ok);
            if !ended {
                let names = me.with_job(&id, |j| j.job.blocked_by.join(", ")).unwrap_or_default();
                me.set_job(&id, |j| j.detail = format!("Could not close {names}. Close it yourself and the update continues"));
            }
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
                        // The newest version this app is offered (its own pre-release choice included).
                        let v = if repair { Some(inst.version.clone()) } else { version.clone().or(latest) };
                        craft::plan_install(key, v.as_deref(), kind, Some(&inst), feed.as_deref(), s.craft_prereleases).await
                    }
                    SourceData::Tools { .. } => {
                        let feed = self.tools.read().clone().ok_or_else(|| anyhow!("The release lists could not be loaded. Check your connection."))?;
                        let kind = if repair { JobKind::Repair } else if version.as_ref().is_some_and(|v| tools::cmp_ver(&inst.version, v).is_gt()) { JobKind::Rollback } else { JobKind::Update };
                        let target = if repair { Some(inst.version.clone()) } else { version.clone().or(latest) };
                        tools::plan_install(&inst.id, target.as_deref(), kind, Some(&inst), &installed, &feed, tools::Ctx { godot_root: std::path::Path::new(&s.godot_install_dir) }).await
                    }
                    SourceData::Other { .. } if inst.source == Source::Programs => {
                        if repair {
                            return Err(anyhow!("Repair is not available for {}. Install the same version again from its page.", inst.name));
                        }
                        let feed = self.ghapps.read().clone();
                        let kind = if version.as_ref().is_some_and(|v| tools::cmp_ver(&inst.version, v).is_gt()) { JobKind::Rollback } else { JobKind::Update };
                        // The newest version this program is offered (its own pre-release choice included).
                        ghapps::plan_install(&inst, version.clone().or(latest).as_deref(), kind, feed.as_deref())
                    }
                    SourceData::Other { .. } => {
                        let feed = self.other.read().clone();
                        let kind = if repair { JobKind::Repair } else if version.as_ref().is_some_and(|v| tools::cmp_ver(&inst.version, v).is_gt()) { JobKind::Rollback } else { JobKind::Update };
                        // The offered version is a GitHub pre-release the catalog does not have: named.
                        let pre_target = self.other_pre.read().get(&inst.id).filter(|(p, _)| latest.as_deref() == Some(p.version.as_str())).map(|(p, _)| p.version.clone());
                        let target = if repair { None } else { version.clone().or(pre_target) };
                        other::plan_install(&inst.id, target.as_deref(), kind, Some(&inst), feed.as_deref()).await
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
                        // A new app goes into the chosen install folder when its installer can be told.
                        let root = Some(s.install_root.trim()).filter(|r| !r.is_empty()).map(PathBuf::from);
                        other::plan_install_in(id, version.as_deref(), feed.as_deref(), root.as_deref()).await
                    }
                    _ => Err(anyhow!("Unknown app {id}")),
                }
            }
            JobRequest::Driver { id } => {
                let f = self.drivers.lock().found.iter().find(|f| &f.entry.id == id).cloned();
                drivers::plan(&f.ok_or_else(|| anyhow!("That driver is no longer listed. Check for updates again."))?)
            }
            JobRequest::Uninstall { app_id, remove_prefs } => {
                let inst = installed.iter().find(|i| &i.id == app_id).ok_or_else(|| anyhow!("That app is no longer installed"))?;
                let mut plan = match inst.source {
                    Source::Adobe => adobe::plan_uninstall(inst),
                    Source::Craft => craft::plan_uninstall(inst),
                    Source::Blender => blender::plan_uninstall(inst),
                    Source::Tools => tools::plan_uninstall(inst),
                    Source::Other => other::plan_uninstall(inst),
                    Source::Programs => crate::uninstall::run::plan(inst),
                    Source::Drivers => Err(anyhow!("Drivers are not uninstalled here")),
                }?;
                // Whatever the vendor's uninstaller leaves in the app's own folder goes too. Other
                // apps: nothing is known about what they keep there, so their folder only goes when
                // nothing but the uninstaller is left (an NSIS uninstaller run in place can not
                // delete itself). The same for Other programs; the leftover review after the
                // uninstall offers the rest.
                let by_uninstaller = matches!(inst.source, Source::Other | Source::Programs);
                let others: Vec<PathBuf> = installed.iter().filter(|i| i.id != inst.id).filter_map(|i| i.install_dir.clone()).collect();
                if let Some(dir) = inst.install_dir.as_deref().filter(|_| by_uninstaller).and_then(|d| leftover_dir(d, &others)) {
                    plan.steps.push(sources::Step::RemoveIfOnlyUninstaller { elevated: under_program_files(&dir), path: dir, label: format!("Removing what is left of {}", inst.name) });
                }
                let removed_already = |d: &std::path::Path| plan.steps.iter().any(|s| matches!(s, sources::Step::RemoveDir { path, .. } if path.as_path() == d));
                if let Some(dir) = inst.install_dir.as_deref().filter(|_| !by_uninstaller).and_then(|d| leftover_dir(d, &others)).filter(|d| !removed_already(d)) {
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
        let mut kind = JobKind::Update;
        self.set_job(id, |j| {
            kind = j.kind;
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
            self.notify_with(&title, &err.message, FAILED_SOUND, notice::failed_target(kind, &app_id));
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
        let mut ran: &Plan = &plan;
        if let Err(e) = &result {
            // Other apps try again with admin rights only when Windows would not start the
            // installer without them (error 740), never after an installer that ran.
            let retry = !crate::elevate::admin_unavailable(e) && (!plan.app_id.starts_with("other:") || other::needs_elevation(&format!("{e:#}")));
            if let (Some(fb), true) = (&plan.fallback, retry) {
                log::warn!("{}: {e:#}; trying fallback", plan.name);
                self.set_job(id, |j| {
                    j.state = JobState::Downloading;
                    j.step = "Trying another way".into();
                    j.downloaded = 0;
                    j.total = 0;
                });
                let first = format!("{e:#}");
                ran = &**fb;
                result = self.execute(id, fb).await.map_err(|e2| if crate::elevate::admin_unavailable(&e2) { e2 } else { anyhow!("{e2:#}\n\nFirst attempt: {first}") });
            }
        }
        // Admin rights could not be had (declined, the prompt failed or was not answered, or no
        // window is open to show it): nothing ran. The job waits in the queue until the user
        // allows admin rights, and the jobs that need none go on meanwhile.
        while matches!(&result, Err(e) if crate::elevate::admin_unavailable(e)) {
            let Some(mut ctl) = self.with_job(id, |j| j.ctl.subscribe()) else { break };
            let mut gen = self.admin_gen.subscribe();
            gen.borrow_and_update();
            let first = !self.jobs.lock().iter().any(|j| j.job.needs_admin && !j.job.state.is_finished());
            self.with_job(id, |j| j.phase = Phase::Ready);
            self.set_job(id, |j| {
                j.state = JobState::Blocked;
                j.needs_admin = true;
                j.cancelable = true;
                j.ready = false;
                j.step = "Needs admin rights".into();
                j.detail = "Nothing was installed yet. Allow admin rights to continue".into();
                j.speed = 0.0;
                j.eta = None;
            });
            // Nothing ran: after a restart it is queued again, not shown as cut off.
            self.save_queue();
            self.wake.notify_one();
            self.emit_apps();
            if first {
                if self.window_open() {
                    self.toast("info", "Some updates need admin rights. Allow them in the Queue to continue.", None);
                } else {
                    self.notify("Updates need admin rights", "Open The Updater 3D and allow admin rights to install them.", Target::Queue);
                }
            }
            let canceled = tokio::select! {
                _ = gen.changed() => false,
                _ = until_canceled(&mut ctl) => true,
            };
            if canceled {
                result = Ok(Outcome::Canceled);
                break;
            }
            self.with_job(id, |j| j.phase = Phase::Download);
            self.set_job(id, |j| {
                j.needs_admin = false;
                j.cancelable = false;
                j.state = JobState::Downloading;
                j.step = "Preparing".into();
                j.detail.clear();
            });
            self.emit_apps();
            result = self.execute(id, ran).await;
        }
        self.set_job(id, |j| j.needs_admin = false);
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
                self.trim_download_cache();
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
                if kind != JobKind::Uninstall {
                    let mut p = self.persisted.lock();
                    p.updated_at.insert(app_id.clone(), now());
                    store::save_state(&p);
                    drop(p);
                    if app_id.starts_with("driver:") {
                        let said = self.with_job(id, |j| j.restart).unwrap_or(false);
                        // Two registry reads: quick enough to do right here.
                        let restart = said || drivers::restart_pending();
                        self.driver_installed(&app_id, restart);
                    }
                    if app_id.starts_with("other:") {
                        let me = self.clone();
                        let id = app_id.clone();
                        tauri::async_runtime::spawn(async move { me.add_to_other_feed(&id).await });
                    }
                }
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
                    self.notify(&title, body, notice::finished_target(kind, &app_id));
                }
                self.toast("ok", title, Some(app_id.clone()));
                // Back on an older version (rolled back, or installed again after an uninstall): the
                // settings it had before the update can be put back.
                if matches!(kind, JobKind::Rollback | JobKind::Install) {
                    let (aid, ver) = (app_id.clone(), to.clone());
                    let saved = tauri::async_runtime::spawn_blocking(move || crate::snapshots::for_version(&crate::snapshots::root(), &aid, &ver)).await.ok().flatten();
                    if let Some(s) = saved {
                        let day = s.created.get(..10).unwrap_or(&s.created).to_string();
                        self.log_activity(ActivityKind::Rollback, format!("{} {}: its settings from before the update can be put back", name, s.version), Some(format!("Saved {day}, before {} {}. Restore puts them back; the current ones are saved first.", name, s.to_version)), Some(format!("settings:{}", s.id)));
                        self.toast("info", format!("{name}'s settings from {} are saved. Activity can put them back.", s.version), Some(app_id.clone()));
                    }
                }
                // Another copy of the app stays (ShareX from Steam next to the installed one):
                // some uninstallers remove the shortcuts and menu entries both copies use.
                if kind == JobKind::Uninstall {
                    let other = self.st.lock().installed.iter().find(|i| i.id != app_id && i.name.trim().eq_ignore_ascii_case(name.trim())).map(|i| i.id.clone());
                    if let Some(other) = other {
                        self.toast("info", format!("Another {name} is still installed. If its shortcuts or right-click menu are gone, use Repair on its page."), Some(other));
                    }
                }
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
    /// Wait until none of `block`'s programs run (the ones in these folders, with these names, or
    /// holding these files): the job shows Blocked with their names, and "Close and continue"
    /// closes them. false: the job was canceled.
    async fn wait_closed(self: &Arc<Self>, id: &str, ctl: &watch::Receiver<Control>, block: (Vec<PathBuf>, Vec<String>, Vec<PathBuf>)) -> bool {
        self.with_job(id, |j| j.block = block.clone());
        loop {
            if *ctl.borrow() == Control::Cancel {
                return false;
            }
            // Listing processes is slow, blocking work: keep it off the async threads.
            let (dirs, bnames, files) = block.clone();
            let b = tauri::async_runtime::spawn_blocking(move || blockers(&procs::list(), &dirs, &bnames, &files)).await.unwrap_or_default();
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
                if !j.job.detail.starts_with("Could not close") {
                    j.job.detail = "Downloaded and checked. It installs once that is closed".into();
                }
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
        self.with_job(id, |j| j.block = Default::default());
        true
    }

    async fn execute(self: &Arc<Self>, id: &str, plan: &Plan) -> Result<Outcome> {
        let ctl = self.with_job(id, |j| j.ctl.subscribe()).ok_or_else(|| anyhow!("job vanished"))?;
        // The copy as it was, to tell what the install changed.
        let before = self.installed(&plan.app_id);
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
            j.ready = installer;
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
            j.ready = false;
            j.detail.clear();
        });

        // ---- the app must be closed
        let block = (plan.block_dirs.clone(), plan.block_names.clone(), plan.block_files());
        if !self.wait_closed(id, &ctl, block).await {
            return Ok(Outcome::Canceled);
        }
        self.with_job(id, |j| j.phase = Phase::Running);
        // Saved before anything runs: after a crash from here on, the installer is not run
        // again by itself.
        self.save_queue();
        // The app's settings before it changes version, closed now: a rollback can put back
        // the ones the older version wrote (see `snapshots`).
        if let (JobKind::Update | JobKind::Rollback, Some(b)) = (plan.kind, before.clone()) {
            let others: Vec<Installed> = self.installed_all().into_iter().filter(|i| i.id != b.id).collect();
            let to = plan.to_version.clone();
            match tauri::async_runtime::spawn_blocking(move || crate::snapshots::save_before(&b, &others, &to)).await {
                Ok(Err(e)) => log::warn!("saving the settings of {}: {e:#}", plan.name),
                Err(e) => log::warn!("saving the settings of {}: {e}", plan.name),
                _ => {}
            }
        }
        let me = self.clone();
        let _ = tauri::async_runtime::spawn_blocking(move || me.emit_apps()).await;
        // The program's own files before it is updated, to tell afterwards that they are still
        // there (see `integrity`).
        let programs_before = match (plan.kind, before.as_ref().and_then(|b| b.install_dir.clone())) {
            (JobKind::Update | JobKind::Repair | JobKind::Rollback, Some(dir)) if other::own_dir(&dir) => tauri::async_runtime::spawn_blocking(move || crate::integrity::program_names(&dir)).await.ok(),
            _ => None,
        };

        // ---- install steps
        let silent = self.settings.read().silent_elevation;
        let n = plan.steps.len();
        let install_t0 = Instant::now();
        // Uninstall entries written from here on were written by this install (a little slack
        // for the registry clock).
        let since = std::time::SystemTime::now() - Duration::from_secs(2);
        // A program already ran: from here on the job can not simply start over.
        let mut ran_program = false;
        // Each step owns a share of the bar by how long that kind of step takes, so the bar
        // follows the steps: it cannot run ahead of a step that has not started yet.
        let weights: Vec<f64> = plan.steps.iter().map(step_weight).collect();
        let weight_sum: f64 = weights.iter().sum::<f64>().max(f64::EPSILON);
        // How long this app's own installs took before, when there were any.
        let runs_key = runs_key(plan);
        let hint = self.persisted.lock().install_runs.get(&runs_key).and_then(|r| install_detail::usual_time(r.secs, r.runs));
        for (k, step) in plan.steps.iter().enumerate() {
            let label = if n > 1 { format!("{}, step {} of {}", step.label(), k + 1, n) } else { step.label().to_string() };
            let files = match step {
                sources::Step::ExtractZip { zip, .. } => {
                    let z = zip.clone();
                    tauri::async_runtime::spawn_blocking(move || zip_files(&z)).await.ok().flatten()
                }
                _ => None,
            };
            let waiting = step.label().starts_with(sources::WAITING_FOR_YOU);
            let detail = if waiting { "Its window may be behind this one. The update continues once you finish it".to_string() } else { step_detail(step, files) };
            self.set_job(id, |j| {
                j.step = label.clone();
                j.detail = detail.clone();
            });
            if waiting {
                self.notify("An installer is waiting for you", step.label().trim_start_matches(sources::WAITING_FOR_YOU).trim_start_matches(':').trim(), Target::Queue);
            }
            let step_t0 = Instant::now();
            let step_est = (inst_est * weights[k] / weight_sum).max(2.0);
            let weights = weights.clone();
            // Installers report no progress: move the bar along the expected install time, and
            // past it keep creeping instead of stopping, so it never looks frozen.
            let rum = rum_watch(step, &plan.to_version, &self.adobe_sizes.lock());
            // What the step does right now, from its log, the programs it runs and its folder,
            // watched off the async threads.
            let procs: crate::elevate::Procs = Default::default();
            let live: Arc<Mutex<install_detail::Live>> = Default::default();
            let stop_watch = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let _stop_watch = install_detail::StopOnDrop(stop_watch.clone());
            if rum.is_none() {
                let spec = watch_spec(step, detail.clone(), files, before.as_ref(), hint.clone(), &procs);
                let (stop, out) = (stop_watch.clone(), live.clone());
                drop(tauri::async_runtime::spawn_blocking(move || install_detail::watch(spec, stop, out)));
            }
            let ticker = {
                let me = self.clone();
                let jid = id.to_string();
                let label = label.clone();
                let live = live.clone();
                tauri::async_runtime::spawn(async move {
                    // Remote Update Manager downloads by itself; its folder shows how far it is.
                    let mut last = (Instant::now(), 0u64);
                    let mut shown = String::new();
                    let mut floor = 0.0f64;
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
                            let now = live.lock().clone();
                            // Real progress (files copied or unpacked) moves the bar on when it is
                            // ahead of the expected time.
                            let x = (s / step_est).max(now.fraction.unwrap_or(0.0));
                            floor = floor.max(dl_share + (1.0 - dl_share) * 0.98 * steps_progress(&weights, k, x));
                            me.set_job(&jid, |j| {
                                j.step = format!("{label}, {}:{:02}", s as u64 / 60, s as u64 % 60);
                                j.progress = floor;
                                if !now.text.is_empty() && now.text != shown {
                                    j.detail = now.text.clone();
                                }
                            });
                            shown = now.text;
                        }
                        tokio::time::sleep(Duration::from_millis(500)).await;
                    }
                })
            };
            // Downloaded files this step runs are checked again by the helper right before it
            // runs them, so a file swapped after the download check is not run with admin rights.
            let checks = crate::elevate::file_checks(step, &plan.downloads);
            let mut res = self.elevator.run_watched(step.clone(), silent, checks.clone(), self.window_open(), Some(procs.clone())).await;
            ticker.abort();
            stop_watch.store(true, Ordering::Relaxed);
            // Windows Installer runs one install at a time (1618): wait for the other one to
            // finish and run the step again.
            let busy_t0 = Instant::now();
            let mut tries = 0;
            while let Some(wait) = res.as_ref().ok().and_then(|r| busy_retry_delay(r, tries, busy_t0.elapsed())) {
                tries += 1;
                self.set_job(id, |j| {
                    j.step = "Waiting for another Windows install to finish".into();
                    j.detail = "Windows installs one app at a time. This tries again by itself".into();
                    j.cancelable = true;
                });
                let mut c = ctl.clone();
                if sleep_or_cancel(&mut c, wait).await {
                    return Ok(Outcome::Canceled);
                }
                self.set_job(id, |j| {
                    j.step = label.clone();
                    j.detail.clear();
                    j.cancelable = false;
                });
                res = self.elevator.run_with(step.clone(), silent, checks.clone(), self.window_open()).await;
            }
            // An Inno Setup installer that found the app running (a copy from elsewhere: Steam's,
            // a portable one) stops before it changes anything: wait for that app to close, as
            // before an install, and run the installer again.
            let mut running_tries = 0;
            while let Some(app) = res.as_ref().ok().filter(|r| !r.ok).and_then(|_| step_log(step)).and_then(|l| log_tail(&l)).and_then(|t| inno_found_running(&t)) {
                let names = {
                    let app = app.clone();
                    tauri::async_runtime::spawn_blocking(move || procs::running_as(&procs::list_fresh(), &app)).await.unwrap_or_default()
                };
                if names.is_empty() || running_tries >= 3 {
                    let log = step_log(step).map(|l| l.display().to_string()).unwrap_or_default();
                    return Err(anyhow!("RUNNING|{app}|{log}"));
                }
                running_tries += 1;
                if !self.wait_closed(id, &ctl, (vec![], names, vec![])).await {
                    return Ok(Outcome::Canceled);
                }
                self.set_job(id, |j| j.step = label.clone());
                res = self.elevator.run_with(step.clone(), silent, checks.clone(), self.window_open()).await;
            }
            let res = match res {
                // A program already ran: the job can not wait and start over, so it fails.
                Err(e) if ran_program && crate::elevate::admin_unavailable(&e) => return Err(anyhow!("{e:#}")),
                r => r?,
            };
            if matches!(step, sources::Step::Run { .. }) {
                ran_program = true;
                if res.ok && res.code.is_some_and(|c| sources::restart_code(&plan.app_id, c)) {
                    self.with_job(id, |j| j.restart = true);
                }
            }
            if !res.ok {
                return Err(anyhow!("{}", step_failure(&res, step.label(), step_log(step).as_deref())));
            }
        }

        if !plan.steps.is_empty() {
            let took = install_t0.elapsed().as_secs_f64();
            let mut p = self.persisted.lock();
            let e = p.install_times.entry(time_key.clone()).or_insert(took);
            *e = *e * 0.5 + took * 0.5;
            p.install_runs.entry(runs_key.clone()).or_default().add(took);
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
                Verify::Version { version, .. } | Verify::Driver { version, .. } => format!("Checking that {version} is installed"),
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
        // Installers sometimes finish registering a while after they exit (a process they
        // started writes the uninstall entry): keep checking, longest while the app is not in
        // the list at all.
        // An uninstall handed to a launcher waits for the user to confirm it there.
        let launcher = crate::uninstall::run::handed_to(plan);
        let mut ctl = ctl.clone();
        let t0 = Instant::now();
        loop {
            self.rescan().await;
            let state = self.check_result(plan, before.as_ref(), since).await;
            if state == Checked::Done {
                if let Some(gone) = self.programs_gone(plan, before.as_ref(), programs_before.as_ref()).await {
                    return Err(anyhow!("VERIFY|The installer finished, but most of {}'s programs are missing ({}). Restart the PC, then use Retry to install it again.", plan.name, gone.into_iter().take(4).collect::<Vec<_>>().join(", ")));
                }
                return Ok(Outcome::Done);
            }
            let waited = t0.elapsed();
            if !keep_checking(&state, waited, launcher.is_some()) {
                if let Some(by) = &launcher {
                    return Err(anyhow!("VERIFY|{} is still installed: {by} did not uninstall it. Confirm the uninstall in {by}, then use Retry.", plan.name));
                }
                return Err(match (&plan.verify, state) {
                    (Verify::Version { version: v, .. }, Checked::Shows { version: now_v, .. }) => anyhow!("VERIFY|The installer finished, but {} reports {} instead of {}", plan.name, now_v, v),
                    (Verify::Version { version: v, .. }, _) => anyhow!("VERIFY|The installer finished, but {} reports not installed instead of {}", plan.name, v),
                    (Verify::Driver { version: v, .. }, Checked::Shows { version: now_v, .. }) => anyhow!("VERIFY|The installer finished, but Windows still has {} {} instead of {}", plan.name, now_v, v),
                    _ => anyhow!("VERIFY|The uninstaller finished, but {} is still installed", plan.name),
                });
            }
            if let Some(by) = &launcher {
                self.set_job(id, |j| j.step = format!("Confirm the uninstall in {by}"));
                if *ctl.borrow() == Control::Cancel {
                    return Ok(Outcome::Canceled);
                }
                let _ = tokio::time::timeout(Duration::from_secs(5), ctl.changed()).await;
                continue;
            }
            if waited >= Duration::from_secs(10) {
                self.set_job(id, |j| j.step = "Waiting for the installer to finish registering".into());
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }

    /// The programs an update took away from the app's folder, when it took most of them: the
    /// installer finished and Windows lists the new version, but the app is not whole.
    async fn programs_gone(self: &Arc<Self>, plan: &Plan, before: Option<&Installed>, programs_before: Option<&std::collections::BTreeSet<String>>) -> Option<Vec<String>> {
        let was = programs_before?.clone();
        let dir = self.installed(&plan.app_id).and_then(|i| i.install_dir).or_else(|| before.and_then(|b| b.install_dir.clone()))?;
        let now = tauri::async_runtime::spawn_blocking(move || crate::integrity::program_names(&dir)).await.ok()?;
        crate::integrity::mostly_gone(&was, &now)
    }

    /// What the uninstall list shows after the steps, against what the plan expects.
    async fn check_result(self: &Arc<Self>, plan: &Plan, before: Option<&Installed>, since: std::time::SystemTime) -> Checked {
        match &plan.verify {
            Verify::None | Verify::Wait { .. } => Checked::Done,
            Verify::Driver { app_id, version: v } => {
                let id = app_id.clone();
                match tauri::async_runtime::spawn_blocking(move || drivers::installed_version(&id)).await.ok().flatten() {
                    Some(now) if version::newer(v, &now) => Checked::Shows { unchanged: before.is_some_and(|b| b.version == now), version: now },
                    _ => Checked::Done,
                }
            }
            Verify::Removed { app_id } => {
                if self.st.lock().installed.iter().any(|i| &i.id == app_id) {
                    Checked::Shows { version: String::new(), unchanged: true }
                } else {
                    Checked::Done
                }
            }
            Verify::Version { app_id, version: v } => {
                let found = self.installed(app_id).filter(|i| !i.missing);
                if found.as_ref().is_some_and(|i| !version::newer(v, &i.version)) {
                    return Checked::Done;
                }
                let unchanged = |now: &str| before.is_some_and(|b| b.version == now);
                // A portable copy: its program file tells its version.
                if let Some(exe) = before.filter(|b| sources::portable::is_portable(b)).and_then(|b| b.exe.clone()) {
                    return match tauri::async_runtime::spawn_blocking(move || sources::portable::version_now(&exe)).await.ok().flatten() {
                        Some(now) if !version::newer(v, &now) && !version::newer(&now, v) => Checked::Done,
                        Some(now) => Checked::Shows { unchanged: unchanged(&now), version: now },
                        None => Checked::Missing,
                    };
                }
                let Some(rest) = app_id.strip_prefix("other:") else {
                    return match found {
                        Some(i) => Checked::Shows { unchanged: unchanged(&i.version), version: i.version },
                        None => Checked::Missing,
                    };
                };
                // Other apps: the catalog's versions can read differently from the uninstall
                // list, and an installer can move the app to another scope.
                let pkg = rest.strip_suffix(":user").unwrap_or(rest).to_string();
                let feed = self.other.read().clone();
                let x = other::Expect {
                    app_id: app_id.clone(),
                    pkg: pkg.clone(),
                    pkg_version: plan.to_version.clone(),
                    expected: v.clone(),
                    range: feed.as_deref().and_then(|f| f.get(&pkg)).and_then(|pf| other::arp_range(pf, &plan.to_version)),
                    learned: other::learned_shows(&pkg, &plan.to_version),
                    declared_scope: other::declared_scope(feed.as_deref(), &pkg, &plan.to_version, before),
                    before: before.map(|b| b.version.clone()),
                };
                let handled: Vec<Installed> = self.st.lock().installed.iter().filter(|i| !matches!(i.source, Source::Other | Source::Programs)).cloned().collect();
                let p2 = pkg.clone();
                let copies = tauri::async_runtime::spawn_blocking(move || other::package_copies(&p2, &winutil::uninstall_entries(), &handled, since)).await.unwrap_or_default();
                match other::check_install(&x, &copies) {
                    c @ other::Check::Done { .. } => {
                        other::learn_from(&x, &c);
                        if c != (other::Check::Done { shows: None, scope: None }) {
                            // List the app again with what was learned (its scope, its version).
                            self.rescan().await;
                        }
                        Checked::Done
                    }
                    other::Check::Missing => Checked::Missing,
                    other::Check::Shows(now) => Checked::Shows { unchanged: unchanged(&now), version: now },
                }
            }
        }
    }

    pub async fn apply_settings(self: &Arc<Self>, patch: serde_json::Value) -> Settings {
        let old = self.settings.read().clone();
        let new = store::merge(&old, patch);
        *self.settings.write() = new.clone();
        store::save_settings(&new);
        self.throttle.set(if new.bandwidth_limit_enabled { new.bandwidth_limit } else { 0 });
        if new.download_dir != old.download_dir || new.install_root != old.install_root {
            paths::set_downloads(Some(&new.download_dir));
            let me = self.clone();
            tauri::async_runtime::spawn_blocking(move || me.update_disk_status());
        }
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
        if new.pre_releases != old.pre_releases {
            let changed: Vec<String> = new.pre_releases.keys().chain(old.pre_releases.keys()).filter(|k| new.pre_releases.get(*k) != old.pre_releases.get(*k)).cloned().collect();
            self.pre_releases_changed(changed);
        }
        // The user chose Game Ready or Studio for a card: the driver check picks its driver again.
        if new.driver_studio != old.driver_studio {
            let me = self.clone();
            tauri::async_runtime::spawn(async move { me.check_drivers().await });
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

    /// Drop a saved list (what it marks as the newest changed).
    pub fn forget(key: &str) {
        let _ = std::fs::remove_file(file(key));
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
/// What a check after the install steps found.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Checked {
    Done,
    /// The app is not in the uninstall list.
    Missing,
    /// It shows another version (`unchanged`: the one from before the install), or for an
    /// uninstall, it is still there.
    Shows { version: String, unchanged: bool },
}

/// Check again after `waited`? Installers that hand the work to another process can exit before
/// that one writes the uninstall entry, and remove the old entry first: a missing app gets three
/// minutes, an entry the install has not changed yet one, anything else ten seconds.
fn keep_checking(state: &Checked, waited: Duration, handed_off: bool) -> bool {
    let limit = match state {
        Checked::Done => return false,
        // A launcher asked to uninstall: the user confirms there, then it removes the app.
        _ if handed_off => 15 * 60,
        Checked::Missing => 180,
        Checked::Shows { unchanged: true, .. } => 60,
        Checked::Shows { .. } => 10,
    };
    waited < Duration::from_secs(limit)
}

/// Windows Installer runs one install at a time and answers 1618 while another one runs (often
/// the end of the previous installer, or Windows Update). How long to wait before running the
/// step again: 15 s, then doubling up to 2 minutes, for about 15 minutes in all.
fn busy_retry_delay(res: &crate::elevate::StepResult, tries: u32, waited: Duration) -> Option<Duration> {
    if res.ok || res.code != Some(1618) || waited >= Duration::from_secs(15 * 60) {
        return None;
    }
    Some(Duration::from_secs((15u64 << tries.min(3)).min(120)))
}

/// Sleep for `d`; true, and sooner, when the job is canceled.
async fn sleep_or_cancel(ctl: &mut watch::Receiver<Control>, d: Duration) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(d) => *ctl.borrow() == Control::Cancel,
        _ = until_canceled(ctl) => true,
    }
}

/// Wait until the job is canceled (forever when it is not).
async fn until_canceled(ctl: &mut watch::Receiver<Control>) {
    loop {
        if *ctl.borrow_and_update() == Control::Cancel {
            return;
        }
        if ctl.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

fn set_control(tx: &watch::Sender<Control>, c: Control) {
    tx.send_replace(c);
}

/// How long a step usually takes, relative to the others: running an installer or uninstaller is
/// the long part, unpacking is shorter, and removing leftovers or writing a file is quick.
fn step_weight(step: &sources::Step) -> f64 {
    match step {
        sources::Step::Run { .. } | sources::Step::OpenUri { .. } | sources::Step::AddPackage { .. } | sources::Step::RemovePackage { .. } => 1.0,
        sources::Step::ExtractZip { .. } | sources::Step::ReplacePortable { .. } => 0.5,
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

/// The app an Inno Setup installer found running, from its log. Its "close all instances of it"
/// box answers Cancel when the install is silent, and the installer stops before it changed
/// anything ("Setup has detected that ShareX is currently running"). Also an uninstaller's.
pub fn inno_found_running(log: &str) -> Option<String> {
    let at = log.find("Defaulting to Cancel for suppressed message box")?;
    let rest = &log[at..];
    let start = rest.find(" has detected that ")? + " has detected that ".len();
    let end = start + rest[start..].find(" is currently running")?;
    let name = rest[start..end].trim();
    (!name.is_empty() && name.len() <= 80 && !name.contains('\n')).then(|| name.to_string())
}

/// The end of an installer's log (the reason it stopped is at the end).
fn log_tail(p: &std::path::Path) -> Option<String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(p).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(SeekFrom::Start(len.saturating_sub(16 * 1024))).ok()?;
    let mut b = Vec::new();
    f.read_to_end(&mut b).ok()?;
    Some(String::from_utf8_lossy(&b).into_owned())
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

/// The log a step's installer writes, when it writes one.
fn step_log(step: &sources::Step) -> Option<PathBuf> {
    match step {
        sources::Step::Run { log, .. } => log.clone(),
        _ => None,
    }
}

/// Turn an internal error string into the UI's code / message / detail.
fn explain(text: &str, plan: &Plan) -> (String, String, String, Option<String>) {
    if let Some(rest) = text.strip_prefix("RUNNING|") {
        let (app, log) = rest.split_once('|').unwrap_or((rest, ""));
        let msg = format!("{app} is running, and its installer will not go on while it is. Close {app} (also a copy from Steam or a portable one), then use Retry.");
        let log = Some(log.trim().to_string()).filter(|l| !l.is_empty());
        let mut detail = msg.clone();
        if let Some(l) = &log {
            detail += &format!("\nLog: {l}");
        }
        detail += &format!("\nNothing else was changed. {} is still as it was.", plan.name);
        return ("Close it first".into(), msg, detail, log);
    }
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
            // Windows Installer will not put an older version over a newer one (Tailscale back from
            // its unstable track): the way back is a new install of the older version.
            "1638" if plan.kind == JobKind::Rollback => format!("{}'s installer does not put an older version over a newer one. Uninstall {}, then install {} from Get apps: its settings from before are saved, and Activity can put them back.", plan.name, plan.name, plan.to_version),
            "1638" => "Another version of this app is already installed.".to_string(),
            "1602" | "1223" => "The installer was canceled.".to_string(),
            "1612" | "1620" => "The installer package could not be opened. Clear the download cache and try again.".to_string(),
            "183" => "Files are in use. Close the app and try again.".to_string(),
            "5" if plan.app_id == "tools:obs" => "OBS Studio is running. Close it and try again.".to_string(),
            "6" if plan.app_id == "tools:obs" => "Another app is using OBS Studio's files (a game OBS captured, or an app using the OBS Virtual Camera). Close it and try again.".to_string(),
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

/// Does this job keep the 5-minute rescan from running? Only work in progress does; a job that
/// only waits for admin rights can wait for hours and must not freeze the app list.
fn rescan_waits_for(j: &JobRec) -> bool {
    j.active && !j.job.needs_admin
}

/// Windows notification sounds: the usual one, and a chime that stands out for failures.
const NOTIFY_SOUND: &str = "Default";
const FAILED_SOUND: &str = "Reminder";

/// The notification shown when the window closes and the app keeps running ("Keep checking after
/// you close the window" is on, or jobs are still in the queue).
fn still_running_notice(busy: bool) -> (&'static str, &'static str) {
    let body = if busy {
        "It is finishing the queue. Click here to open it, or quit from its icon next to the clock."
    } else {
        "It keeps checking for updates. Click here to open it, or quit from its icon next to the clock."
    };
    ("The Updater 3D is still running", body)
}

/// What happens to an app's updates: its own rule, or the one for every app.
fn auto_mode_of(s: &Settings, id: &str) -> AutoMode {
    s.exceptions.get(id).copied().unwrap_or(s.auto_mode)
}

/// The updates the "updates ready" notice tells about: not those whose automatic updates are
/// Off (for every app, or by the app's own rule).
fn told_updates<'a>(updates: &[&'a AppEntry], s: &Settings) -> Vec<&'a AppEntry> {
    updates.iter().copied().filter(|a| auto_mode_of(s, &a.id) != AutoMode::Off).collect()
}

/// The day the install window open at `now` opened on, or None while it is closed. A window
/// past midnight (22:00 to 02:00) at 01:00 opened the day before. Times that do not read leave
/// it always open.
fn window_day(now: chrono::NaiveDateTime, start: &str, end: &str) -> Option<chrono::NaiveDate> {
    let parse = |s: &str| chrono::NaiveTime::parse_from_str(s, "%H:%M").ok();
    let (Some(a), Some(b)) = (parse(start), parse(end)) else { return Some(now.date()) };
    let t = now.time();
    if a <= b {
        (t >= a && t < b).then_some(now.date())
    } else if t >= a {
        Some(now.date())
    } else {
        (t < b).then(|| now.date() - chrono::Days::new(1))
    }
}

/// Is it time for automatic installs, and in which day's window? After a background check:
/// whenever the window is open (the check may have found more). On the scheduler's tick: once
/// per window, so what a check made outside the window found (checks every 12 hours or once a
/// day may never land in it) is installed in it. `last_day`: the window they last ran in.
fn auto_install_due(now: chrono::NaiveDateTime, start: &str, end: &str, last_day: Option<&str>, after_check: bool) -> Option<chrono::NaiveDate> {
    let day = window_day(now, start, end)?;
    (after_check || last_day != Some(day.to_string().as_str())).then_some(day)
}

/// The "updates ready" notification: the title counts every update waiting, the text names the
/// ones the user has not heard about yet.
fn update_notice(total: usize, fresh: &[String]) -> (String, String) {
    let title = format!("{total} update{} ready", if total == 1 { "" } else { "s" });
    let names: Vec<&str> = fresh.iter().take(3).map(String::as_str).collect();
    let list = if fresh.len() > 3 { format!("{} and {} more", names.join(", "), fresh.len() - 3) } else { names.join(", ") };
    let body = if fresh.len() < total { format!("New: {list}") } else { list };
    (title, body)
}

/// When an app was last updated: the exact time this app recorded, or the source's date when it
/// is later (updated outside the app). A source date with no time (only the day) never beats a
/// recorded time on the same day.
fn newest_update(recorded: Option<&str>, source: Option<&str>) -> Option<String> {
    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    match (recorded, source) {
        (Some(r), Some(s)) => match (parse(r), parse(s)) {
            (Some(rt), Some(st)) => {
                let day_only = s.ends_with("T00:00:00Z");
                let later = if day_only { st.date_naive() > rt.date_naive() } else { st > rt };
                Some(if later { s } else { r }.to_string())
            }
            (Some(_), None) => Some(r.to_string()),
            _ => Some(s.to_string()),
        },
        (r, s) => r.or(s).map(String::from),
    }
}

/// An update's size from the download server when its source did not give one.
/// Why a copy that Steam or the Microsoft Store manages is not changed here.
/// Updates the user turned off (Settings > Apps, or "Don't update" on the app) are not shown
/// as updates: not listed, counted, notified or installed.
fn skip_ignored(apps: &mut [AppEntry], ignored: &std::collections::BTreeSet<String>) {
    for a in apps.iter_mut().filter(|a| a.status == AppStatus::Update && ignored.contains(&a.id)) {
        a.status = AppStatus::Current;
        a.update_size = None;
        a.note = Some(format!("Updates are turned off for {}", a.name));
    }
}

/// A driver as the apps list shows it. `installable`: its update installs here.
fn driver_app(e: &DriverEntry, status: AppStatus, installable: bool, updated_at: Option<String>, job_id: Option<String>) -> AppEntry {
    let (fg, bg) = driver_colors(&e.maker);
    let manual = status == AppStatus::Update && !installable;
    AppEntry {
        id: e.id.clone(),
        source: Source::Drivers,
        name: e.name.clone(),
        code: driver_code(&e.maker).into(),
        fg: fg.into(),
        bg: bg.into(),
        icon_url: None,
        icon_miss: false,
        version: e.version.clone(),
        latest: e.latest.clone(),
        status,
        update_size: if status == AppStatus::Update { e.size } else { None },
        size_on_disk: None,
        install_dir: None,
        updated_at,
        tagline: Some(if e.kind == "BIOS" || e.kind == "Chipset" { format!("{} from {}", e.kind, e.maker) } else { format!("{} driver from {}", e.kind, e.maker) }),
        group: Source::Drivers.group().into(),
        running: false,
        can_uninstall: false,
        can_rollback: false,
        read_only: false,
        store_url: None,
        manual_url: if manual || !installable { drivers::safe_link(e.vendor_url.clone()) } else { None },
        github: None,
        is_self: false,
        pre_channel: e.pre_channel,
        studio: e.studio,
        channel: None,
        note: e.note.clone(),
        job_id,
    }
}

/// The two letters a driver shows when it has no icon: its maker's.
fn driver_code(maker: &str) -> &'static str {
    match maker.to_ascii_lowercase().as_str() {
        m if m.starts_with("nvidia") => "NV",
        m if m.starts_with("amd") || m.starts_with("advanced micro") => "AMD",
        m if m.starts_with("intel") => "In",
        m if m.starts_with("realtek") => "Rt",
        m if m.starts_with("gigabyte") => "GB",
        m if m.starts_with("mediatek") => "MT",
        m if m.starts_with("qualcomm") => "Qc",
        _ => "Dr",
    }
}

/// Job colors for a driver: the maker's.
fn driver_colors(maker: &str) -> (&'static str, &'static str) {
    match maker.to_ascii_lowercase().as_str() {
        m if m.starts_with("nvidia") => ("#FFFFFF", "#4E7A00"),
        m if m.starts_with("amd") || m.starts_with("advanced micro") => ("#FFFFFF", "#9E1B1B"),
        m if m.starts_with("intel") => ("#FFFFFF", "#0B5CAD"),
        _ => ("#EDEDED", "#3A4A5C"),
    }
}

fn read_only_error(inst: &Installed) -> anyhow::Error {
    let by = if inst.store.is_some() { "the Microsoft Store" } else { "Steam" };
    anyhow!("{} is updated and removed by {by}", inst.name)
}

/// The size measured earlier for an app at its version, while it still has the folder it was
/// measured from (a browser's web app was once given its browser's folder: that size is not its own).
fn remembered_size(sizes: &std::collections::BTreeMap<String, u64>, i: &Installed) -> Option<u64> {
    i.install_dir.as_ref()?;
    sizes.get(&format!("{}@{}", i.id, i.version)).copied()
}

fn with_download_size(mut l: Latest, app_id: &str, dl_sizes: &HashMap<String, u64>) -> Latest {
    if l.size.is_none() {
        l.size = dl_sizes.get(&format!("{app_id}@{}", l.version)).copied().filter(|n| *n > 0);
    }
    l
}

#[cfg(test)]
mod tests {
    use super::{update_notice, with_download_size};

    #[test]
    fn a_size_measured_from_a_folder_the_app_no_longer_has_is_not_shown() {
        // "Paper Planner", a web app Thorium installed: 781 MB was measured from
        // Thorium's folder before web apps had no folder of their own.
        let e = crate::winutil::UninstallEntry { key: "3c9e1f0a7b2d4e6f8a1c3e5b7d9f0a2c".into(), hive: "HKCU", display_name: "Paper Planner".into(), display_version: "1.0".into(), publisher: "Thorium".into(), uninstall_string: r#""C:\Browsers\Thorium\BIN\thorium.exe" --uninstall-app-id=kmpdfnlgcoapkiibcehndbgjmnihblme"#.into(), ..Default::default() };
        let web = crate::uninstall::programs::make(&e, None, None, &|_| true, None);
        let sizes: std::collections::BTreeMap<String, u64> = [(format!("{}@1.0", web.id), 781 * 1024 * 1024)].into();
        assert_eq!(super::remembered_size(&sizes, &web), None);
        // An app that still has its folder keeps the size measured from it.
        let app = crate::sources::Installed { install_dir: Some(std::path::PathBuf::from(r"C:\Apps\Zapster")), ..web.clone() };
        assert_eq!(super::remembered_size(&sizes, &app), Some(781 * 1024 * 1024));
    }

    #[test]
    fn an_other_app_that_follows_pre_releases_is_offered_the_newest_one() {
        use crate::sources::Latest;
        let at = |v: &str| Some(Latest { version: v.into(), size: None, note: None });
        let offer = |stable: Option<Latest>, pre: Option<&str>, here: &str| super::other_offer(stable, pre.map(|v| Latest { version: v.into(), size: Some(139_000_000), note: None }).as_ref(), here).map(|l| l.version);
        // ShareX 21.0.0 here, the catalog at 21.0.0, 22.0.0 a pre-release on GitHub.
        assert_eq!(offer(at("21.0.0"), Some("22.0.0"), "21.0.0").as_deref(), Some("22.0.0"));
        // Not following them: the catalog's.
        assert_eq!(offer(at("21.0.0"), None, "21.0.0").as_deref(), Some("21.0.0"));
        // The pre-release installed: nothing newer.
        assert_eq!(offer(at("21.0.0"), Some("22.0.0"), "22.0.0").as_deref(), Some("21.0.0"));
        // A final release newer than the pre-release wins.
        assert_eq!(offer(at("22.1.0"), Some("22.0.0"), "21.0.0").as_deref(), Some("22.1.0"));
        // A final 22.0.0 after its own beta is newer than it.
        assert_eq!(offer(at("22.0.0"), Some("22.0.0-beta.2"), "22.0.0-beta.2").as_deref(), Some("22.0.0"));
        // The pre-release keeps its download size.
        let sized = super::other_offer(at("21.0.0"), Some(&Latest { version: "22.0.0".into(), size: Some(131_695_352), note: None }), "21.0.0");
        assert_eq!(sized.and_then(|l| l.size), Some(131_695_352));
    }

    #[test]
    fn an_installer_that_found_the_app_running_names_it() {
        // ShareX 21.0.0's Inno Setup log on this PC, with ShareX running from Steam's folder.
        let log = r"2026-10-24 16:18:22.012   Created protected temporary directory: C:\Users\x\AppData\Local\Temp\is-9ERD256333.tmp
2026-10-24 16:18:22.042   Defaulting to Cancel for suppressed message box (OK/Cancel):
                          Setup has detected that ShareX is currently running.
                          
                          Please close all instances of it now, then click OK to continue, or Cancel to exit.
2026-10-24 16:18:22.042   Got EAbort exception.
2026-10-24 16:18:22.042   Deinitializing Setup.
";
        assert_eq!(super::inno_found_running(log).as_deref(), Some("ShareX"));
        let un = "Defaulting to Cancel for suppressed message box (OK/Cancel):
   Uninstall has detected that Notepad++ is currently running.
";
        assert_eq!(super::inno_found_running(un).as_deref(), Some("Notepad++"));
        // The same words without the silent box, or another box: not this case.
        assert_eq!(super::inno_found_running("Setup has detected that ShareX is currently running."), None);
        assert_eq!(super::inno_found_running("Defaulting to Cancel for suppressed message box (OK/Cancel):
  This program requires Windows 11.
"), None);
    }

    #[test]
    fn an_app_that_keeps_running_fails_with_what_to_do() {
        use crate::model::JobKind;
        use crate::sources::{Plan, Verify};
        let plan = Plan { app_id: "other:ShareX.ShareX".into(), kind: JobKind::Install, name: "ShareX".into(), to_version: "21.0.0".into(), downloads: vec![], steps: vec![], block_dirs: vec![], block_names: vec![], verify: Verify::None, fallback: None, cleanup: vec![] };
        let (code, msg, detail, log) = super::explain(r"RUNNING|ShareX|C:\logs\other-ShareX.ShareX-21.0.0.log", &plan);
        assert_eq!(code, "Close it first");
        assert!(msg.starts_with("ShareX is running") && msg.contains("Close ShareX") && msg.contains("Retry"), "{msg}");
        assert!(!msg.contains("exit code"));
        assert_eq!(log.as_deref(), Some(r"C:\logs\other-ShareX.ShareX-21.0.0.log"));
        assert!(detail.contains("Nothing else was changed"));
    }

    #[test]
    fn the_game_sign_ins_and_icons_left_on_this_pc_go() {
        let root = std::env::temp_dir().join(format!("u3d-forget-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (data, icons) = (root.join("data"), root.join("icons"));
        std::fs::create_dir_all(data.join("accounts").join("web").join("steam")).unwrap();
        std::fs::write(data.join("accounts").join("epic.bin"), b"sealed").unwrap();
        std::fs::write(data.join("settings.json"), b"{}").unwrap();
        std::fs::create_dir_all(&icons).unwrap();
        for f in ["owned-1a2b.png", "game-steam-440.jpg", "obs.png"] {
            std::fs::write(icons.join(f), b"x").unwrap();
        }
        super::forget_game_data(&data, &icons);
        assert!(!data.join("accounts").exists());
        assert!(data.join("settings.json").is_file());
        let left: Vec<String> = std::fs::read_dir(&icons).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(left, vec!["obs.png".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
    }
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

    #[test]
    fn the_apps_own_entry_shows_its_own_update_and_is_never_a_job() {
        use crate::model::{AppStatus, DriverEntry, Source};
        let d = DriverEntry { id: "prog:hkcu:the updater 3d".into(), name: "The Updater 3D".into(), kind: String::new(), maker: String::new(), source: String::new(), version: "1.2.0".into(), latest: None, status: AppStatus::Current, size: None, date: None, notes_url: None, vendor_url: None, note: Some("Mega Limited".into()), notes: vec![], pre_channel: None, studio: None };
        let mut a = super::driver_app(&d, AppStatus::Current, false, None, None);
        a.source = Source::Programs;
        a.manual_url = None;
        super::mark_self(&mut a, Some("1.2.1"));
        assert!(a.is_self);
        assert_eq!((a.status, a.latest.as_deref()), (AppStatus::Update, Some("1.2.1")));
        // Update all, the update count and auto-install skip what has a manual page.
        assert!(a.manual_url.is_some());
        assert_eq!(a.group, "Other apps");
        assert_eq!(a.note, None);
        // The version it has, or an older one in the feed: current.
        let mut b = super::driver_app(&d, AppStatus::Current, false, None, None);
        b.manual_url = None;
        super::mark_self(&mut b, Some("1.2.0"));
        assert_eq!((b.status, b.manual_url.as_deref()), (AppStatus::Current, None));
        let mut c = super::driver_app(&d, AppStatus::Current, false, None, None);
        c.manual_url = None;
        super::mark_self(&mut c, None);
        assert_eq!(c.status, AppStatus::Current);
    }

    #[test]
    fn closing_the_window_says_the_app_still_runs_and_how_to_quit() {
        for busy in [false, true] {
            let (title, body) = super::still_running_notice(busy);
            assert!(title.contains("still running"));
            assert!(body.contains("quit from its icon"));
            assert!(!body.contains('\u{2014}') && !body.contains('\u{2013}'));
        }
        assert!(super::still_running_notice(true).1.contains("queue"));
        // The window's close handler sends it whenever the app stays open.
        let lib = include_str!("lib.rs");
        let close = lib.find("WindowEvent::CloseRequested").unwrap();
        let keep = lib[close..].find("close_to_tray || busy").unwrap() + close;
        let exit = lib[keep..].find("app.exit(0)").unwrap() + keep;
        assert!(lib[keep..exit].contains("tell_still_running(busy)"));
    }

    /// Windows plays no sound for a toast without a sound name, and only knows a few names.
    #[test]
    fn every_notification_plays_a_known_windows_sound() {
        let known = ["Default", "IM", "Mail", "Reminder", "SMS"];
        assert!(known.contains(&super::NOTIFY_SOUND) && known.contains(&super::FAILED_SOUND));
        for (name, src) in [("core.rs", include_str!("core.rs")), ("commands.rs", include_str!("commands.rs")), ("notice.rs", include_str!("notice.rs"))] {
            for (i, line) in src.lines().enumerate() {
                if line.contains(concat!(".builder()", ".title(")) {
                    assert!(line.contains(".sound("), "{name}:{} sends a silent notification", i + 1);
                }
            }
        }
    }

    #[test]
    fn recently_updated_uses_the_exact_time_the_app_recorded() {
        use super::newest_update;
        // Windows only knows the day; the app knows the minute: the minute wins on that day.
        assert_eq!(newest_update(Some("2026-10-22T12:10:00+00:00"), Some("2026-10-22T00:00:00Z")).as_deref(), Some("2026-10-22T12:10:00+00:00"));
        // Updated outside the app later on: the newer source date wins.
        assert_eq!(newest_update(Some("2026-10-20T12:10:00+00:00"), Some("2026-10-22T00:00:00Z")).as_deref(), Some("2026-10-22T00:00:00Z"));
        assert_eq!(newest_update(Some("2026-10-22T09:00:00+00:00"), Some("2026-10-22T11:30:00+00:00")).as_deref(), Some("2026-10-22T11:30:00+00:00"));
        assert_eq!(newest_update(None, Some("2026-10-22T00:00:00Z")).as_deref(), Some("2026-10-22T00:00:00Z"));
        assert_eq!(newest_update(Some("2026-10-22T09:00:00+00:00"), None).as_deref(), Some("2026-10-22T09:00:00+00:00"));
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
    fn automatic_updates_off_tells_nothing_about_that_apps_updates() {
        use crate::model::{AppStatus, AutoMode, DriverEntry, Settings};
        let app = |id: &str| {
            let d = DriverEntry { id: id.into(), name: id.into(), kind: String::new(), maker: String::new(), source: String::new(), version: "1.0".into(), latest: Some("2.0".into()), status: AppStatus::Update, size: None, date: None, notes_url: None, vendor_url: None, note: None, notes: vec![], pre_channel: None };
            super::driver_app(&d, AppStatus::Update, true, None, None)
        };
        let (krita, obs) = (app("tools:krita"), app("tools:obs"));
        let updates = vec![&krita, &obs];
        let ids = |s: &Settings| super::told_updates(&updates, s).iter().map(|a| a.id.clone()).collect::<Vec<_>>();
        let mut s = Settings { auto_mode: AutoMode::Off, ..Settings::default() };
        assert!(ids(&s).is_empty());
        // An app's own rule wins over the one for every app, both ways.
        s.exceptions.insert("tools:obs".into(), AutoMode::Notify);
        assert_eq!(ids(&s), ["tools:obs"]);
        s.auto_mode = AutoMode::Install;
        s.exceptions.insert("tools:obs".into(), AutoMode::Off);
        assert_eq!(ids(&s), ["tools:krita"]);
        s.exceptions.clear();
        s.auto_mode = AutoMode::Notify;
        assert_eq!(ids(&s), ["tools:krita", "tools:obs"]);
    }

    #[test]
    fn automatic_installs_run_in_the_window_even_when_no_check_lands_in_it() {
        use super::auto_install_due;
        let at = |s: &str| chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M").unwrap();
        let day = |s: &str| Some(chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap());
        // Checks once a day at 15:00: the minute tick at 03:00 installs what that check found,
        // once per window.
        assert_eq!(auto_install_due(at("2026-10-26 03:00"), "02:00", "06:00", None, false), day("2026-10-26"));
        assert_eq!(auto_install_due(at("2026-10-26 03:00"), "02:00", "06:00", Some("2026-10-25"), false), day("2026-10-26"));
        assert_eq!(auto_install_due(at("2026-10-26 03:01"), "02:00", "06:00", Some("2026-10-26"), false), None);
        // A check inside the window installs what it found, whenever it runs.
        assert_eq!(auto_install_due(at("2026-10-26 04:00"), "02:00", "06:00", Some("2026-10-26"), true), day("2026-10-26"));
        // Never outside the window.
        assert_eq!(auto_install_due(at("2026-10-26 15:00"), "02:00", "06:00", None, false), None);
        assert_eq!(auto_install_due(at("2026-10-26 15:00"), "02:00", "06:00", None, true), None);
        assert_eq!(auto_install_due(at("2026-10-26 06:00"), "02:00", "06:00", None, false), None);
        // A window past midnight is one window: it does not run again after midnight.
        assert_eq!(auto_install_due(at("2026-10-26 23:00"), "22:00", "02:00", None, false), day("2026-10-26"));
        assert_eq!(auto_install_due(at("2026-10-27 01:00"), "22:00", "02:00", Some("2026-10-26"), false), None);
        assert_eq!(auto_install_due(at("2026-10-27 01:00"), "22:00", "02:00", Some("2026-10-25"), false), day("2026-10-26"));
        assert_eq!(auto_install_due(at("2026-10-27 12:00"), "22:00", "02:00", None, false), None);
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

    #[tokio::test]
    async fn downloaded_jobs_install_in_the_order_they_were_moved_to() {
        use std::sync::Arc;
        let gate = Arc::new(super::TurnGate::new());
        let (_t0, r0) = tokio::sync::watch::channel(Control::Run);
        let busy = gate.take("a", "A", 0, r0).await.unwrap();
        let order = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let mut hs = Vec::new();
        for (id, seq) in [("first", 1u64), ("second", 2)] {
            let (g, o) = (gate.clone(), order.clone());
            hs.push(tauri::async_runtime::spawn(async move {
                let (_tx, rx) = tokio::sync::watch::channel(Control::Run);
                let t = g.take(id, id, seq, rx).await.unwrap();
                o.lock().push(id);
                tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                drop(t);
            }));
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        // Both are downloaded and waiting; the user drags "second" above "first".
        gate.reorder(&[("second".to_string(), 1u64), ("first".to_string(), 2)].into_iter().collect());
        drop(busy);
        for h in hs {
            h.await.unwrap();
        }
        assert_eq!(*order.lock(), vec!["second", "first"]);
    }

    #[test]
    fn an_uninstall_takes_the_next_install_turn() {
        use crate::model::JobKind;
        use super::turn_seq;
        // Seen: Cline's uninstall waited behind the AMD driver and the chipset update.
        let base = 1u64 << 32;
        let open = vec![(JobKind::Update, base), (JobKind::Update, base + 1), (JobKind::Update, base + 2)];
        let u1 = turn_seq(JobKind::Uninstall, open.clone().into_iter(), || base + 3);
        assert!(u1 < base, "before every waiting update");
        // A second uninstall goes after the first, still before the updates.
        let mut with = open.clone();
        with.push((JobKind::Uninstall, u1));
        let u2 = turn_seq(JobKind::Uninstall, with.into_iter(), || base + 4);
        assert!(u1 < u2 && u2 < base, "{u1} {u2}");
        // Updates still go to the end; an uninstall with nothing else waiting too.
        assert_eq!(turn_seq(JobKind::Update, open.into_iter(), || base + 9), base + 9);
        assert_eq!(turn_seq(JobKind::Uninstall, std::iter::empty(), || base + 9), base + 9);
    }

    #[test]
    fn reordering_ready_jobs_keeps_everything_else_in_place() {
        // (id, ready): one installing, three ready, one downloading.
        let mut v: Vec<(String, bool)> = [("inst", false), ("r1", true), ("dl", false), ("r2", true), ("r3", true)].iter().map(|(a, b)| (a.to_string(), *b)).collect();
        let ids: Vec<String> = ["r3", "r1", "r2", "r3", "dl", "gone"].iter().map(|s| s.to_string()).collect();
        super::reorder_in_place(&mut v, &ids, |j| &j.0, |j| j.1);
        let got: Vec<&str> = v.iter().map(|j| j.0.as_str()).collect();
        assert_eq!(got, vec!["inst", "r3", "dl", "r1", "r2"]);
        // Only some of them named: those swap among their own places.
        let ids: Vec<String> = ["r2", "r3"].iter().map(|s| s.to_string()).collect();
        super::reorder_in_place(&mut v, &ids, |j| &j.0, |j| j.1);
        let got: Vec<&str> = v.iter().map(|j| j.0.as_str()).collect();
        assert_eq!(got, vec!["inst", "r2", "dl", "r1", "r3"]);
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
    use super::{blockers, explain, skip_ignored, step_failure};

    #[test]
    fn an_app_set_to_dont_update_shows_no_update() {
        let d = crate::model::DriverEntry { id: "other:7zip.7zip".into(), name: "7-Zip".into(), kind: "x".into(), maker: "x".into(), source: "x".into(), version: "24.0".into(), latest: Some("25.0".into()), status: crate::model::AppStatus::Update, size: Some(1), date: None, notes_url: None, vendor_url: None, note: None, notes: vec![], pre_channel: None, studio: None };
        let mut apps = vec![super::driver_app(&d, crate::model::AppStatus::Update, true, None, None), super::driver_app(&crate::model::DriverEntry { id: "other:vlc".into(), ..d.clone() }, crate::model::AppStatus::Update, true, None, None)];
        skip_ignored(&mut apps, &["other:7zip.7zip".to_string()].into_iter().collect());
        assert_eq!((apps[0].status, apps[0].update_size, apps[0].latest.as_deref()), (crate::model::AppStatus::Current, None, Some("25.0")));
        assert!(apps[0].note.as_deref().unwrap().contains("turned off"));
        assert_eq!(apps[1].status, crate::model::AppStatus::Update, "other apps are left alone");
    }
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
        assert_eq!(blockers(&list, &dirs, &[], &[]).iter().map(|p| p.pid).collect::<Vec<_>>(), vec![100]);
    }

    #[test]
    fn obs_exit_code_6_says_another_app_uses_its_files() {
        let plan = Plan { app_id: "tools:obs".into(), kind: JobKind::Update, name: "OBS Studio".into(), to_version: "33.0.0".into(), downloads: vec![], steps: vec![], block_dirs: vec![], block_names: vec![], verify: Verify::None, fallback: None, cleanup: vec![] };
        let res = StepResult { ok: false, code: Some(6), message: "Exit code 6".into(), output: String::new() };
        let (code, msg, _, _) = explain(&step_failure(&res, "Installing OBS Studio 33.0.0", None), &plan);
        assert_eq!(code, "Error 6");
        assert!(msg.contains("using OBS Studio's files"), "{msg}");
        // Exit code 6 means nothing special for other apps.
        let other = Plan { app_id: "tools:krita".into(), ..plan };
        assert!(explain(&step_failure(&res, "Installing Krita", None), &other).1.contains("exit code 6"));
    }

    #[test]
    fn a_rollback_refused_by_windows_installer_says_how_to_go_back() {
        let plan = Plan { app_id: "other:Tailscale.Tailscale".into(), kind: JobKind::Rollback, name: "Tailscale".into(), to_version: "1.104.2".into(), downloads: vec![], steps: vec![], block_dirs: vec![], block_names: vec![], verify: Verify::None, fallback: None, cleanup: vec![] };
        let res = StepResult { ok: false, code: Some(1638), message: "Exit code 1638".into(), output: String::new() };
        let (_, msg, _, _) = explain(&step_failure(&res, "Installing Tailscale", None), &plan);
        assert!(msg.contains("Uninstall Tailscale, then install 1.104.2 from Get apps"), "{msg}");
        // Not a rollback: as before.
        let update = Plan { kind: JobKind::Update, ..plan };
        assert_eq!(explain(&step_failure(&res, "Installing Tailscale", None), &update).1, "Another version of this app is already installed.");
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
            ready: false,
            needs_admin: false,
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
            store: None,
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
    fn restored_jobs_that_all_finish_leave_the_queue_file() {
        // Seen in the real app: two jobs cut off while installing were restored, one ended Done
        // and one Failed, and queue.json kept both, so every restart brought them back.
        let dir = std::env::temp_dir().join(format!("u3d-queue-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("queue.json");
        let cut_off = SavedJob { job: job("a", JobState::Installing), req: JobRequest::Install { id: "tools:godot".into(), version: None }, phase: Phase::Running, dests: vec![], verify: None };
        save_queue_to(&file, &[cut_off]);
        assert_eq!(load_queue_from(&file).len(), 1);
        // What a fresh Core holds before its first save.
        let mut saved = String::new();
        write_queue_if_changed(&file, &mut saved, &[]);
        assert!(load_queue_from(&file).is_empty(), "the finished restored job is still in the queue file");
        // Nothing changed since: no second write.
        std::fs::remove_file(&file).unwrap();
        write_queue_if_changed(&file, &mut saved, &[]);
        assert!(!file.exists());
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
    fn each_step_watches_its_log_programs_and_folder() {
        use crate::install_detail::{DirWatch, LogKind};
        use crate::sources::Step;
        let dir = std::env::temp_dir().join(format!("u3d-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut before = installed("other:Example.Zipper", "26.04");
        before.install_dir = Some(dir.clone());
        before.size_hint = Some(1_400_000_000);
        let procs: crate::elevate::Procs = Default::default();
        let log = std::path::PathBuf::from(r"C:\logs\other-zipper-26.05.log");
        let run = |exe: &str, label: &str, args: Vec<String>, log: Option<std::path::PathBuf>| Step::Run { exe: exe.into(), args, cwd: None, ok_codes: vec![0], elevated: false, label: label.into(), log };
        let msi = run("msiexec.exe", "Installing Zipper", vec!["/i".into(), "z.msi".into(), "/l*".into(), log.display().to_string()], Some(log.clone()));
        let w = watch_spec(&msi, "x".into(), None, Some(&before), None, &procs);
        assert_eq!(w.log, Some((log.clone(), LogKind::Msi)));
        assert_eq!(w.procs.as_ref().map(|p| p.1.as_str()), Some("msiexec.exe"));
        assert!(matches!(&w.dir, Some(DirWatch::Copy { dir: d, expected: Some(1_400_000_000) }) if d == &dir));
        let inno = run(r"C:\dl\zipper-setup.exe", "Installing Zipper", vec!["/VERYSILENT".into(), format!("/LOG={}", log.display())], Some(log.clone()));
        let w = watch_spec(&inno, "x".into(), None, None, Some("usually takes about 3 min".into()), &procs);
        assert_eq!(w.log, Some((log.clone(), LogKind::Inno)));
        assert!(w.dir.is_none(), "a fresh install has no folder to watch yet");
        assert_eq!(w.hint.as_deref(), Some("usually takes about 3 min"));
        let un = run(r"C:\Program Files\Zipper\unins000.exe", "Uninstalling Zipper", vec!["/VERYSILENT".into()], None);
        let w = watch_spec(&un, "x".into(), None, Some(&before), None, &procs);
        assert!(w.log.is_none());
        assert!(matches!(&w.dir, Some(DirWatch::Remove { dir: d }) if d == &dir));
        let unzip = Step::ExtractZip { zip: "a.zip".into(), dest: "D:/B".into(), strip_root: true, elevated: false, label: String::new() };
        let w = watch_spec(&unzip, "x".into(), Some(12), None, None, &procs);
        assert!(matches!(&w.dir, Some(DirWatch::Unpack { files: Some(12), .. })) && w.procs.is_none());
        // A shared root such as Program Files itself is never walked.
        before.install_dir = Some(crate::paths::program_files());
        assert!(watch_spec(&msi, "x".into(), None, Some(&before), None, &procs).dir.is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn past_runs_are_kept_per_app() {
        let mut r = crate::store::InstallRuns::default();
        r.add(100.0);
        assert_eq!(r, crate::store::InstallRuns { runs: 1, secs: 100.0 });
        r.add(200.0);
        assert_eq!(r.runs, 2);
        assert!((r.secs - 140.0).abs() < 1e-9);
        // State saved before this existed still loads.
        let p: Persisted = serde_json::from_str(r#"{"last_check":null,"notified":{},"last_auto_install_day":null}"#).unwrap();
        assert!(p.install_runs.is_empty());
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

#[cfg(test)]
mod install_check_tests {
    use super::{busy_retry_delay, keep_checking, Checked};
    use crate::elevate::StepResult;
    use std::time::Duration;

    // MEGAsync 6.7.0.3: its installer left the work to a process that wrote the uninstall entry
    // later; the check gave up after about ten seconds.
    #[test]
    fn a_missing_app_is_checked_for_three_minutes_after_its_installer_exits() {
        let s = Duration::from_secs;
        assert!(keep_checking(&Checked::Missing, s(60), false));
        assert!(keep_checking(&Checked::Missing, s(170), false));
        assert!(!keep_checking(&Checked::Missing, s(181), false));
        let unchanged = Checked::Shows { version: "1.0".into(), unchanged: true };
        assert!(keep_checking(&unchanged, s(30), false));
        assert!(!keep_checking(&unchanged, s(61), false));
        let other = Checked::Shows { version: "1.5".into(), unchanged: false };
        assert!(keep_checking(&other, s(5), false));
        assert!(!keep_checking(&other, s(11), false));
        assert!(!keep_checking(&Checked::Done, s(0), false));
    }

    // ShareX from Steam: Steam asks the user to confirm its uninstall; the check gave up after
    // a minute, before the user had answered.
    #[test]
    fn an_uninstall_handed_to_a_launcher_waits_for_the_user_to_confirm_it() {
        let s = Duration::from_secs;
        let unchanged = Checked::Shows { version: String::new(), unchanged: true };
        assert!(keep_checking(&unchanged, s(5 * 60), true));
        assert!(!keep_checking(&unchanged, s(15 * 60 + 1), true));
        assert!(!keep_checking(&Checked::Done, s(0), true));
    }

    // Python 3.14.7: "Another installation is already running" (1618) failed the update.
    #[test]
    fn a_busy_windows_installer_is_waited_for_instead_of_failing() {
        let r = |ok: bool, code: Option<u32>| StepResult { ok, code, message: String::new(), output: String::new() };
        let s = Duration::from_secs;
        let busy = r(false, Some(1618));
        assert_eq!(busy_retry_delay(&busy, 0, s(0)), Some(s(15)));
        assert_eq!(busy_retry_delay(&busy, 1, s(15)), Some(s(30)));
        assert_eq!(busy_retry_delay(&busy, 3, s(200)), Some(s(120)));
        assert_eq!(busy_retry_delay(&busy, 9, s(800)), Some(s(120)));
        assert_eq!(busy_retry_delay(&busy, 10, s(15 * 60)), None, "gives up after 15 minutes");
        assert_eq!(busy_retry_delay(&r(false, Some(1603)), 0, s(0)), None);
        assert_eq!(busy_retry_delay(&r(true, Some(0)), 0, s(0)), None);
    }
}

#[cfg(test)]
mod app_icon_tests {
    use super::*;

    fn other_app(id: &str, pkg: &str) -> Installed {
        Installed {
            id: id.into(),
            source: Source::Other,
            name: "Media Player".into(),
            code: "Mp".into(),
            fg: String::new(),
            bg: String::new(),
            version: "1.0".into(),
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
            store: None,
            data: SourceData::Other { pkg: pkg.into(), how: other::How::Name, entry: Box::default(), upgrade_code: None },
        }
    }

    /// An Other app whose program file is not found shows its package's catalog icon, not its letters.
    #[test]
    fn an_installed_app_without_a_program_icon_shows_the_catalog_icon() {
        let catalog = HashMap::from([("other:Example.MediaPlayer".to_string(), "asset://catalog.png".to_string())]);
        let mut icons = HashMap::new();
        assert_eq!(app_icon(&other_app("other:Example.MediaPlayer", "Example.MediaPlayer"), &icons, &catalog).as_deref(), Some("asset://catalog.png"));
        assert_eq!(app_icon(&other_app("other:Example.MediaPlayer:user", "Example.MediaPlayer"), &icons, &catalog).as_deref(), Some("asset://catalog.png"));
        // The program's own icon comes first.
        icons.insert("other:Example.MediaPlayer".to_string(), "asset://exe.png".to_string());
        assert_eq!(app_icon(&other_app("other:Example.MediaPlayer", "Example.MediaPlayer"), &icons, &catalog).as_deref(), Some("asset://exe.png"));
    }
}

#[cfg(test)]
mod icon_miss_tests {
    use super::*;

    #[test]
    fn misses_saved_under_the_old_rules_are_searched_again() {
        // the old rules also remembered searches that only timed out
        assert_eq!(icon_miss_day("2026-10-22"), None);
        assert_eq!(icon_miss_day("2026-10-22/1"), None);
        assert_eq!(icon_miss_day("2026-10-22/2"), chrono::NaiveDate::from_ymd_opt(2026, 10, 22));
        assert_eq!(icon_miss_day("junk/2"), None);
    }

    #[test]
    fn small_icons_are_told_from_full_size_ones_of_earlier_versions() {
        assert!(icon_is_small(std::path::Path::new("C:/x/other-a.b.icon.png")));
        assert!(icon_is_small(std::path::Path::new("C:/x/other-a.b.dark.png")));
        assert!(icon_is_small(std::path::Path::new("C:/x/other-a.b.svg")));
        assert!(!icon_is_small(std::path::Path::new("C:/x/other-a.b.png")));
        assert!(!icon_is_small(std::path::Path::new("C:/x/other-a.b.ico")));
    }
}
