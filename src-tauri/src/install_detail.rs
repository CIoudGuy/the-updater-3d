//! Live detail for a running install step: what Windows Installer and Inno Setup write to their
//! logs, which helper program the installer runs, and how much it wrote. Everything here only
//! reports what is seen; nothing is guessed.

use parking_lot::Mutex;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// The detail line stays one short line.
pub const MAX_LEN: usize = 70;

// ---------------------------------------------------------------- log files

/// Reads the lines added to a log file since the last read. Windows Installer writes ANSI or
/// UTF-16 logs, Inno Setup UTF-8 or ANSI; the encoding is read from the start of the file.
pub struct LogTail {
    path: PathBuf,
    /// A log last written before this is left from an earlier run and not read.
    since: SystemTime,
    pos: u64,
    utf16: Option<bool>,
    carry: Vec<u8>,
}

impl LogTail {
    pub fn new(path: PathBuf, since: SystemTime) -> Self {
        LogTail { path, since, pos: 0, utf16: None, carry: vec![] }
    }

    /// New complete lines, and whether the file started over (the parser should start over too).
    pub fn read_lines(&mut self) -> (bool, Vec<String>) {
        let Ok(mut f) = std::fs::File::open(&self.path) else { return (false, vec![]) };
        let Ok(meta) = f.metadata() else { return (false, vec![]) };
        let len = meta.len();
        let mut reset = false;
        if len < self.pos {
            // Written anew (a retry overwrites the log).
            *self = LogTail::new(self.path.clone(), self.since);
            reset = true;
        }
        if self.pos == 0 && meta.modified().is_ok_and(|m| m + Duration::from_secs(2) < self.since) {
            return (reset, vec![]);
        }
        if len == self.pos || f.seek(SeekFrom::Start(self.pos)).is_err() {
            return (reset, vec![]);
        }
        // At most 8 MB a read; the rest comes with the next one.
        let mut buf = Vec::new();
        let n = f.take(8 << 20).read_to_end(&mut buf).unwrap_or(0);
        self.pos += n as u64;
        (reset, self.push(&buf))
    }

    /// Split bytes into complete lines, keeping a partial last line for later.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<String> {
        self.carry.extend_from_slice(bytes);
        if self.utf16.is_none() {
            if self.carry.len() < 4 && !self.carry.contains(&b'\n') {
                return vec![];
            }
            let bom16 = self.carry.starts_with(&[0xFF, 0xFE]);
            self.utf16 = Some(bom16 || (self.carry.len() >= 4 && self.carry[1] == 0 && self.carry[3] == 0));
            if bom16 {
                self.carry.drain(..2);
            } else if self.carry.starts_with(&[0xEF, 0xBB, 0xBF]) {
                self.carry.drain(..3);
            }
        }
        let mut out = vec![];
        if self.utf16 == Some(true) {
            let even = self.carry.len() & !1;
            let units: Vec<u16> = self.carry[..even].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let last_nl = units.iter().rposition(|u| *u == 0x0A);
            let Some(end) = last_nl else { return out };
            for l in units[..end].split(|u| *u == 0x0A) {
                out.push(String::from_utf16_lossy(l).trim_end_matches('\r').to_string());
            }
            self.carry.drain(..(end + 1) * 2);
        } else {
            let Some(end) = self.carry.iter().rposition(|b| *b == b'\n') else { return out };
            for l in self.carry[..end].split(|b| *b == b'\n') {
                out.push(String::from_utf8_lossy(l).trim_end_matches('\r').to_string());
            }
            self.carry.drain(..=end);
        }
        out
    }
}

// ---------------------------------------------------------------- Windows Installer logs

enum MsiAction {
    /// Plain text for the user.
    Text(&'static str),
    /// Writes into the install script only: nothing changes on the PC until the script runs.
    Scripted(&'static str),
    Copy,
    /// A standard action with nothing worth showing: the line keeps what it said.
    Quiet,
}

/// Windows Installer's own actions. Any other name is one of the package's custom actions.
fn msi_action(name: &str) -> Option<MsiAction> {
    use MsiAction::*;
    Some(match name {
        "CostInitialize" | "FileCost" | "CostFinalize" | "IsolateComponents" | "ResolveSource" => Text("Checking disk space"),
        "AppSearch" | "LaunchConditions" | "FindRelatedProducts" | "CCPSearch" | "RMCCPSearch" | "ValidateProductID" | "MigrateFeatureStates" => Text("Checking this PC"),
        "InstallValidate" => Text("Checking for files in use"),
        "InstallInitialize" => Text("Starting the install"),
        "RemoveExistingProducts" => Text("Removing the old version"),
        "InstallFinalize" | "InstallExecute" | "InstallExecuteAgain" => Text("Applying the changes"),
        "InstallFiles" | "PatchFiles" | "DuplicateFiles" | "MoveFiles" => Copy,
        "RemoveFiles" | "RemoveFolders" | "RemoveDuplicateFiles" | "RemoveShortcuts" => Scripted("Removing files"),
        "CreateFolders" => Scripted("Creating folders"),
        "CreateShortcuts" => Scripted("Creating shortcuts"),
        "WriteRegistryValues" | "RemoveRegistryValues" | "WriteIniValues" | "RemoveIniValues" | "WriteEnvironmentStrings" | "RemoveEnvironmentStrings" | "RegisterClassInfo"
        | "RegisterProgIdInfo" | "RegisterExtensionInfo" | "RegisterMIMEInfo" | "RegisterTypeLibraries" | "RegisterFonts" | "SelfRegModules" | "UnregisterClassInfo"
        | "UnregisterProgIdInfo" | "UnregisterExtensionInfo" | "UnregisterMIMEInfo" | "UnregisterTypeLibraries" | "UnregisterFonts" | "SelfUnregModules"
        | "AllocateRegistrySpace" | "RegisterComPlus" | "UnregisterComPlus" | "InstallODBC" | "RemoveODBC" | "SetODBCFolders" | "BindImage" => Scripted("Writing settings"),
        "InstallServices" | "StartServices" | "MsiConfigureServices" | "DeleteServices" => Scripted("Setting up services"),
        "StopServices" => Scripted("Stopping services"),
        "RegisterProduct" | "PublishProduct" | "PublishFeatures" | "PublishComponents" | "RegisterUser" | "MsiPublishAssemblies" | "UnpublishFeatures"
        | "UnpublishComponents" | "MsiUnpublishAssemblies" => Scripted("Registering"),
        "ProcessComponents" => Scripted("Updating components"),
        "INSTALL" | "ExecuteAction" | "ScheduleReboot" | "ForceReboot" | "InstallAdminPackage" | "InstallSFPCatalogFile" | "MsiProcessDrivers" | "SetProgress"
        | "MsiUnpublishAssemblies2" | "InstallUISequence" | "InstallExecuteSequence" | "ADMIN" | "ADVERTISE" => Quiet,
        _ => return None,
    })
}

/// What a Windows Installer log (`/l*` or `/l*v`) says the install is doing.
#[derive(Default, Debug)]
pub struct MsiLog {
    action: Option<String>,
    rollback: bool,
    /// The install script runs (InstallFinalize): file and registry actions now change the PC.
    executing: bool,
    /// Inside RemoveExistingProducts: the old version's own uninstall writes into this log.
    removing_old: bool,
    /// Files the verbose log says are to be installed, counted before the script runs.
    planned: usize,
    total: Option<usize>,
    /// Files copied: "Executing op: FileCopy" (verbose) and "InstallFiles: File:" lines. A log
    /// has one or both for each file, so the larger count is the number of files.
    ops: usize,
    data: usize,
    /// The last text shown, kept for actions that have nothing to show.
    last: Option<String>,
}

impl MsiLog {
    fn start(&mut self, name: &str) {
        let name = name.trim().trim_end_matches('.').trim();
        if name.is_empty() {
            return;
        }
        if name == "Rollback" {
            self.rollback = true;
        }
        if self.removing_old {
            return;
        }
        if name == "RemoveExistingProducts" {
            self.removing_old = true;
        }
        if matches!(name, "InstallFinalize" | "InstallExecute" | "InstallExecuteAgain") {
            self.begin_script();
        }
        self.action = Some(name.to_string());
        if let Some(t) = self.text_now() {
            self.last = Some(t);
        }
    }

    fn begin_script(&mut self) {
        if !self.executing && !self.removing_old {
            self.executing = true;
            if self.total.is_none() && self.planned > 0 {
                self.total = Some(self.planned);
            }
        }
    }

    fn ended(&mut self, name: &str) {
        let name = name.trim();
        if name == "RemoveExistingProducts" {
            self.removing_old = false;
        } else if !self.removing_old && matches!(name, "InstallFinalize" | "InstallExecute" | "InstallExecuteAgain") {
            // The script is done: the line keeps the last thing it did.
            self.executing = false;
            self.action = None;
        }
    }

    pub fn feed(&mut self, line: &str) {
        let l = line.trim();
        if let Some(rest) = l.strip_prefix("Action ended ") {
            // "Action ended 14:01:22: InstallFinalize. Return value 1."
            if let Some((_, r)) = rest.split_once(": ") {
                self.ended(r.split('.').next().unwrap_or(""));
            }
            return;
        }
        if let Some(rest) = l.strip_prefix("Action start ") {
            // "Action start 14:01:01: InstallFiles."
            if let Some((_, r)) = rest.split_once(": ") {
                self.start(r);
            }
            return;
        }
        if let Some(rest) = l.strip_prefix("Action ").filter(|r| r.starts_with(|c: char| c.is_ascii_digit())) {
            // "Action 14:01:04: InstallFiles. Copying new files"
            if let Some((_, r)) = rest.split_once(": ") {
                if r.contains("Rolling back action") {
                    self.rollback = true;
                }
                self.start(r.split(". ").next().unwrap_or(r));
            }
            return;
        }
        if l.contains("Rolling back action") {
            self.rollback = true;
        }
        if let Some(i) = l.find("Doing action: ") {
            self.start(&l[i + "Doing action: ".len()..]);
            return;
        }
        if l.contains("Running Script: ") {
            self.begin_script();
            return;
        }
        if let Some(i) = l.find("Executing op: ") {
            self.begin_script();
            let op = &l[i + "Executing op: ".len()..];
            if let Some(a) = op.strip_prefix("ActionStart(Name=") {
                let end = a.find([',', ')']).unwrap_or(a.len());
                self.start(&a[..end]);
            } else if op.starts_with("FileCopy(") && !self.removing_old {
                self.ops += 1;
            }
            return;
        }
        if l.starts_with("InstallFiles: File: ") && !self.removing_old {
            self.data += 1;
            return;
        }
        if !self.executing && !self.removing_old && l.contains("]: File: ") && l.contains("To be installed") {
            self.planned += 1;
        }
    }

    fn copied(&self) -> usize {
        self.ops.max(self.data)
    }

    fn text_now(&self) -> Option<String> {
        if self.rollback {
            return Some("Undoing changes".into());
        }
        if self.removing_old {
            return Some("Removing the old version".into());
        }
        let name = self.action.as_deref()?;
        Some(match msi_action(name) {
            None => "Running the installer's own steps".into(),
            Some(MsiAction::Quiet) => return None,
            Some(MsiAction::Text(t)) => t.into(),
            Some(MsiAction::Scripted(t)) => if self.executing { t.into() } else { "Preparing the changes".into() },
            Some(MsiAction::Copy) => {
                let n = self.copied();
                if !self.executing && n == 0 {
                    "Preparing the changes".into()
                } else if n == 0 {
                    "Copying files".into()
                } else {
                    match self.total {
                        Some(m) if n <= m => format!("Copying files ({} of {})", thousands(n as u64), thousands(m as u64)),
                        _ => format!("Copying files ({})", thousands(n as u64)),
                    }
                }
            }
        })
    }

    /// The line for the job, or None before the log says anything useful.
    pub fn text(&self) -> Option<String> {
        self.text_now().or_else(|| self.last.clone())
    }

    /// Share of the files copied, when the log said how many there are and copying runs.
    pub fn fraction(&self) -> Option<f64> {
        let m = self.total? as f64;
        let n = self.copied() as f64;
        (self.executing && !self.rollback && n > 0.0 && n <= m).then(|| n / m)
    }
}

// ---------------------------------------------------------------- Inno Setup logs

/// What an Inno Setup log (`/LOG=`) says the install is doing.
#[derive(Default, Debug)]
pub struct InnoLog {
    in_file_entry: bool,
    dest: usize,
    extracted: usize,
    current: Option<String>,
    phase: Option<&'static str>,
    rollback: bool,
}

impl InnoLog {
    pub fn feed(&mut self, line: &str) {
        // "2026-10-21 14:01:03.601   Dest filename: C:\Program Files\Foo\foo.exe"
        let msg = inno_message(line);
        if msg.starts_with("-- ") && msg.ends_with(" --") {
            self.in_file_entry = msg == "-- File entry --";
            self.phase = match msg {
                "-- File entry --" => Some("files"),
                "-- Icon entry --" => Some("Creating shortcuts"),
                "-- Run entry --" => Some("Running the installer's own steps"),
                _ => self.phase,
            };
        } else if let Some(f) = msg.strip_prefix("Dest filename: ") {
            if self.in_file_entry {
                self.dest += 1;
                self.current = Some(f.trim().to_string());
            }
        } else if let Some(f) = msg.strip_prefix("Extracting file: ") {
            self.extracted += 1;
            self.current = Some(f.trim().to_string());
            self.phase = Some("files");
        } else if msg.starts_with("Starting the installation process") {
            self.phase = Some("Starting the install");
        } else if msg.starts_with("Rolling back changes") {
            self.rollback = true;
        } else if msg.starts_with("Installation process succeeded") {
            self.phase = Some("Finishing");
        }
    }

    pub fn text(&self) -> Option<String> {
        if self.rollback {
            return Some("Undoing changes".into());
        }
        match self.phase? {
            "files" => {
                let n = self.dest.max(self.extracted);
                let head = format!("Copying files ({})", thousands(n as u64));
                Some(match self.current.as_deref().map(file_part) {
                    Some(f) if !f.is_empty() => format!("{head}: {}", shorten(f, MAX_LEN.saturating_sub(head.len() + 2).min(36))),
                    _ => head,
                })
            }
            t => Some(t.to_string()),
        }
    }
}

/// The message of an Inno Setup log line, without its time stamp.
fn inno_message(line: &str) -> &str {
    let l = line.trim();
    // "YYYY-MM-DD HH:MM:SS.mmm   message"
    let b = l.as_bytes();
    if b.len() > 24 && b[4] == b'-' && b[7] == b'-' && b[10] == b' ' && b[13] == b':' && l.is_char_boundary(19) {
        if let Some(i) = l[19..].find(' ') {
            return l[19 + i..].trim();
        }
    }
    l
}

fn file_part(p: &str) -> &str {
    p.rsplit(['\\', '/']).next().unwrap_or(p)
}

// ---------------------------------------------------------------- processes

/// What a program the installer started does, by its exe name (lowercase). The rank says how
/// much it tells: 0 and 1 name real work, 3 is Windows Installer, 4 any other program.
fn program(name: &str) -> (u8, String) {
    let n = name.to_ascii_lowercase();
    if n.starts_with("vc_redist") || n.starts_with("vcredist") {
        (0, "Installing the Visual C++ runtime".into())
    } else if n.starts_with("dotnet-") || n.starts_with("windowsdesktop-runtime") || n.starts_with("aspnetcore-runtime") {
        (0, "Installing the .NET runtime".into())
    } else if n.starts_with("ndp") || n.starts_with("dotnetfx") {
        (0, "Installing the .NET Framework".into())
    } else if n == "dxsetup.exe" || n.starts_with("dxwebsetup") {
        (0, "Installing DirectX".into())
    } else if n == "ngen.exe" || n == "mscorsvw.exe" || n == "ngentask.exe" {
        (1, "Optimizing .NET code".into())
    } else if n == "msiexec.exe" {
        (3, "Waiting for Windows Installer".into())
    } else {
        (4, format!("Running {}", shorten(name, 40)))
    }
}

/// The program that matters among the ones the installer started (`names`, the installer itself
/// left out): a known one first, else the newest. The flag says it names real work, which beats
/// what the log says.
pub fn program_text(names: &[String], installer: &str) -> Option<(String, bool)> {
    let me = installer.to_ascii_lowercase();
    let mut best: Option<(u8, String)> = None;
    for raw in names {
        let n = raw.to_ascii_lowercase();
        if n.is_empty() || n == me || n == "conhost.exe" || n.ends_with(".tmp") {
            continue;
        }
        let p = program(raw);
        // Same rank: the later one (the job lists processes oldest first).
        if best.as_ref().is_none_or(|b| p.0 <= b.0) {
            best = Some(p);
        }
    }
    best.map(|(rank, t)| (t, rank <= 1))
}

// ---------------------------------------------------------------- folders

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DirScan {
    pub files: u64,
    pub bytes: u64,
    /// Files created or written since the step started, and their bytes.
    pub new_files: u64,
    pub new_bytes: u64,
}

/// Walk one folder, at most `cap` entries and about 1.5 seconds. None when it is not there or
/// is bigger than that (the caller stops watching it).
pub fn scan_dir(dir: &Path, since: SystemTime, cap: usize) -> Option<DirScan> {
    if !dir.is_dir() {
        return None;
    }
    let t0 = Instant::now();
    let mut s = DirScan::default();
    let mut seen = 0usize;
    let mut stack = vec![dir.to_path_buf()];
    let fresh = |m: &std::fs::Metadata| m.modified().is_ok_and(|t| t >= since) || m.created().is_ok_and(|t| t >= since);
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > cap || (seen % 512 == 0 && t0.elapsed() > Duration::from_millis(1500)) {
                return None;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(m) = e.metadata() {
                s.files += 1;
                s.bytes += m.len();
                if fresh(&m) {
                    s.new_files += 1;
                    s.new_bytes += m.len();
                }
            }
        }
    }
    Some(s)
}

// ---------------------------------------------------------------- text

pub fn thousands(n: u64) -> String {
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

/// Keep a name under `max` characters, cutting the middle ("Qt6WebEn...Core.dll").
pub fn shorten(s: &str, max: usize) -> String {
    let n = s.chars().count();
    if n <= max || max < 8 {
        return s.to_string();
    }
    let keep = max - 3;
    let tail = keep / 2;
    let head = keep - tail;
    let a: String = s.chars().take(head).collect();
    let b: String = s.chars().skip(n - tail).collect();
    format!("{a}...{b}")
}

/// Parts joined with ", ", leaving out the later ones that would make the line too long.
pub fn join_fit(parts: &[String], max: usize) -> String {
    let mut out = String::new();
    for p in parts.iter().filter(|p| !p.is_empty()) {
        if out.is_empty() {
            out = shorten(p, max);
        } else if out.chars().count() + 2 + p.chars().count() <= max {
            out = format!("{out}, {p}");
        }
    }
    out
}

/// How long this app's installs took before ("usually takes about 3 min").
pub fn usual_time(secs: f64, runs: u32) -> Option<String> {
    if runs == 0 || !secs.is_finite() || secs <= 0.0 {
        return None;
    }
    let min = if secs < 60.0 { 0 } else { ((secs / 60.0).round() as u64).max(1) };
    Some(match (runs, min) {
        (1, 0) => "under a minute last time".into(),
        (1, m) => format!("about {m} min last time"),
        (_, 0) => "usually under a minute".into(),
        (_, m) => format!("usually takes about {m} min"),
    })
}

// ---------------------------------------------------------------- the watcher

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LogKind {
    Msi,
    Inno,
    /// The progress file of a Windows update install (see `winupdate::ProgressLog`)
    WindowsUpdate,
}

/// The folder a step changes.
#[derive(Clone, Debug)]
pub enum DirWatch {
    /// An installer writes into the app's folder; `expected` is its size before.
    Copy { dir: PathBuf, expected: Option<u64> },
    /// An uninstaller empties it.
    Remove { dir: PathBuf },
    /// A zip is unpacked into it.
    Unpack { dir: PathBuf, files: Option<usize> },
}

pub struct WatchSpec {
    /// What the step does when nothing more is known.
    pub base: String,
    pub log: Option<(PathBuf, LogKind)>,
    /// Exe names of the programs the installer started, filled while it runs.
    pub procs: Option<(Arc<Mutex<Vec<String>>>, String)>,
    pub dir: Option<DirWatch>,
    /// "usually takes about 3 min"
    pub hint: Option<String>,
    pub since: SystemTime,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub text: String,
    /// Real share of the step done, when it is known (MSI file counts, unpacked files).
    pub fraction: Option<f64>,
}

enum Parser {
    Msi(MsiLog),
    Inno(InnoLog),
    WindowsUpdate(crate::sources::winupdate::ProgressLog),
}

impl Parser {
    fn new(k: LogKind) -> Self {
        match k {
            LogKind::Msi => Parser::Msi(MsiLog::default()),
            LogKind::Inno => Parser::Inno(InnoLog::default()),
            LogKind::WindowsUpdate => Parser::WindowsUpdate(Default::default()),
        }
    }
    fn feed(&mut self, l: &str) {
        match self {
            Parser::Msi(p) => p.feed(l),
            Parser::Inno(p) => p.feed(l),
            Parser::WindowsUpdate(p) => p.feed(l),
        }
    }
    fn text(&self) -> Option<String> {
        match self {
            Parser::Msi(p) => p.text(),
            Parser::Inno(p) => p.text(),
            Parser::WindowsUpdate(p) => p.text(),
        }
    }
    fn fraction(&self) -> Option<f64> {
        match self {
            Parser::Msi(p) => p.fraction(),
            Parser::Inno(_) => None,
            Parser::WindowsUpdate(p) => p.fraction(),
        }
    }
}

/// The line for what was seen: a program doing named work, else the log, else another program,
/// else the step's own text; then the folder and the usual time when they fit.
pub fn compose(base: &str, log: Option<String>, prog: Option<(String, bool)>, dir: Option<String>, hint: Option<&str>) -> String {
    let mut dir = dir;
    let main = match (prog, log) {
        (Some((p, true)), _) => p,
        (_, Some(l)) => l,
        (Some((p, false)), None) => p,
        // Only the folder says more than "Running the installer silently": it leads.
        (None, None) => match dir.take() {
            Some(d) if d.starts_with("copied ") => format!("C{}", &d[1..]),
            d => {
                dir = d;
                base.to_string()
            }
        },
    };
    let mut parts = vec![main];
    parts.extend(dir);
    parts.extend(hint.map(str::to_string));
    join_fit(&parts, MAX_LEN)
}

/// Stops a `watch` when the step is over, also when the job is dropped halfway.
pub struct StopOnDrop(pub Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Watch a step until `stop`, writing the line to `out`. Blocking: run it off the async threads.
/// Logs are read 4 times a second, folders every 3 seconds.
pub fn watch(spec: WatchSpec, stop: Arc<AtomicBool>, out: Arc<Mutex<Live>>) {
    let mut tail = spec.log.as_ref().map(|(p, _)| LogTail::new(p.clone(), spec.since));
    let mut parser = spec.log.as_ref().map(|(_, k)| Parser::new(*k));
    let mut dir = spec.dir.clone();
    let mut baseline: Option<u64> = None;
    let mut dir_text: Option<String> = None;
    let mut dir_frac: Option<f64> = None;
    let mut last_scan: Option<Instant> = None;
    let mut removing = false;
    while !stop.load(Ordering::Relaxed) {
        if let (Some(t), Some(p)) = (tail.as_mut(), parser.as_mut()) {
            let (reset, lines) = t.read_lines();
            if reset {
                *p = Parser::new(spec.log.as_ref().map(|l| l.1).unwrap_or(LogKind::Msi));
            }
            for l in &lines {
                p.feed(l);
            }
        }
        if dir.is_some() && last_scan.is_none_or(|t| t.elapsed() >= Duration::from_secs(3)) {
            last_scan = Some(Instant::now());
            let w = dir.clone().unwrap();
            let path = match &w {
                DirWatch::Copy { dir, .. } | DirWatch::Remove { dir } | DirWatch::Unpack { dir, .. } => dir.clone(),
            };
            match scan_dir(&path, spec.since, 50_000) {
                None if path.exists() => dir = None,
                None => {
                    if matches!(w, DirWatch::Remove { .. }) && baseline.is_some() {
                        removing = true;
                        dir_text = None;
                    }
                }
                Some(s) => match &w {
                    DirWatch::Copy { expected, .. } => {
                        dir_text = (s.new_bytes >= 1_000_000).then(|| match expected {
                            Some(e) if *e > s.new_bytes => format!("copied {} of about {}", fmt_bytes(s.new_bytes), fmt_bytes(*e)),
                            _ => format!("copied {}", fmt_bytes(s.new_bytes)),
                        });
                    }
                    DirWatch::Remove { .. } => {
                        let b0 = *baseline.get_or_insert(s.bytes);
                        if s.bytes + 1_000_000 < b0 {
                            removing = true;
                            dir_text = Some(format!("{} left", fmt_bytes(s.bytes)));
                        }
                    }
                    DirWatch::Unpack { files, .. } => {
                        let n = s.new_files;
                        dir_text = None;
                        if n > 0 {
                            dir_frac = files.filter(|f| *f > 0 && n <= *f as u64).map(|f| n as f64 / f as f64);
                            let t = match files {
                                Some(f) if n <= *f as u64 => format!("Unpacking {} of {} files", thousands(n), thousands(*f as u64)),
                                _ => format!("Unpacking {} files", thousands(n)),
                            };
                            dir_text = Some(t);
                        }
                    }
                },
            }
        }
        let prog = spec.procs.as_ref().and_then(|(names, me)| program_text(&names.lock(), me));
        let log_text = parser.as_ref().and_then(|p| p.text());
        let unpack = matches!(spec.dir, Some(DirWatch::Unpack { .. }));
        let text = if unpack {
            compose(dir_text.as_deref().unwrap_or(&spec.base), None, None, None, spec.hint.as_deref())
        } else {
            let base = if removing { "Removing files" } else { spec.base.as_str() };
            compose(base, log_text, prog, dir_text.clone(), spec.hint.as_deref())
        };
        let fraction = parser.as_ref().and_then(|p| p.fraction()).or(dir_frac);
        {
            let mut o = out.lock();
            o.text = text;
            o.fraction = fraction;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

#[cfg(test)]
#[path = "install_detail_tests.rs"]
mod tests;
