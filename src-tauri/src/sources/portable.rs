//! Portable copies of catalog apps: a program unpacked from a zip, which Windows' list of
//! installed programs does not have (Thorium's portable build). They are found through the
//! shortcuts that start them (Start menu, desktop, taskbar) and the web apps a portable browser
//! installed, and matched to the catalog by the program's own name and maker
//! (`other::portable_matches`). An update replaces the program in place from the app's portable
//! zip (`replace`): its program folder and launchers are replaced, the data next to them
//! (Thorium's USER_DATA) stays.

use super::ghapps::Asset;
use super::locate::{self, Shortcut};
use super::{Installed, Plan, SourceData, Step, Verify};
use crate::model::JobKind;
use crate::winutil::{self, UninstallEntry};
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};

/// The `hive` of an entry made for a portable copy; its `key` is the program file.
pub const HIVE: &str = "PORTABLE";

/// Names of the folder a portable app keeps its program in, with its data next to it
/// ("Thorium\BIN\thorium.exe" and "Thorium\USER_DATA").
const PROGRAM_DIRS: &[&str] = &["bin", "app", "program", "application"];

/// Where an update unpacks the new files and keeps the replaced ones until it is done, inside
/// the copy's own folder (same drive, so files move instead of being copied).
const STAGING: &str = ".the-updater-3d-new";
const BACKUP: &str = ".the-updater-3d-old";

/// Program files: an update replaces these where they are already there. Any other file that is
/// there already (a setting the user changed) stays, unless it is in a version's own folder.
const PROGRAM_EXT: &[&str] = &["exe", "dll", "sys", "pak", "node", "asar", "jar", "pyd", "mui", "manifest", "cat", "bat", "cmd", "com", "ocx", "winmd", "tlb", "drv", "cpl", "scr"];

/// The folder a portable copy lives in: the one holding its program folder ("Thorium" for
/// "Thorium\BIN\thorium.exe"), else the program's own folder.
pub fn root_of(exe: &Path) -> Option<PathBuf> {
    let dir = exe.parent()?;
    let name = dir.file_name()?.to_string_lossy().to_ascii_lowercase();
    if PROGRAM_DIRS.contains(&name.as_str()) {
        if let Some(up) = dir.parent().filter(|u| u.components().count() >= 3) {
            return Some(up.to_path_buf());
        }
    }
    Some(dir.to_path_buf())
}

/// Is this installed app a portable copy?
pub fn is_portable(i: &Installed) -> bool {
    matches!(&i.data, SourceData::Other { entry, .. } if entry.hive == HIVE)
}

/// The shortcut folders read for portable apps: the Start menu, the desktops and the taskbar.
pub fn shortcut_dirs() -> Vec<PathBuf> {
    let mut v = locate::start_menu_dirs();
    v.extend(desktops());
    if let Some(p) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(p).join("Microsoft\\Internet Explorer\\Quick Launch\\User Pinned"));
    }
    v
}

fn desktops() -> Vec<PathBuf> {
    use windows::Win32::UI::Shell::{FOLDERID_Desktop, FOLDERID_PublicDesktop, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    [FOLDERID_Desktop, FOLDERID_PublicDesktop]
        .iter()
        .filter_map(|id| unsafe {
            let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
            let s = p.to_string().ok();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0 as *const _));
            s.map(PathBuf::from)
        })
        .collect()
}

/// What a program file says about itself, read once per file version.
#[derive(Clone, Debug, Default)]
pub struct Program {
    pub exe: PathBuf,
    pub name: String,
    pub company: String,
    pub version: String,
}

/// Name, maker and version of a program file. The version is the file version, unless the
/// product version is a plain version number with more to it.
pub fn read_program(exe: &Path) -> Option<Program> {
    let t = winutil::version_text(exe)?;
    let name = if t.product_name.trim().is_empty() { t.description.clone() } else { t.product_name.clone() };
    let file = winutil::file_version(exe).filter(|v| v != "0.0.0.0");
    let product = Some(t.product_version.trim().to_string()).filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == '.') && v.contains('.'));
    let version = match (product, file) {
        (Some(p), Some(f)) => if p.len() > f.len() { p } else { f },
        (p, f) => p.or(f)?,
    };
    (!name.trim().is_empty()).then(|| Program { exe: exe.to_path_buf(), name: name.trim().to_string(), company: t.company.trim().to_string(), version })
}

/// The program files the shortcuts start and the browsers the web apps were installed by, that
/// no entry of Windows' list and no app another source lists covers: possible portable copies,
/// one per folder. `read` reads a program file (`read_program`; tests give their own).
pub fn candidates(shortcuts: &[Shortcut], entries: &[UninstallEntry], handled: &[Installed], read: &dyn Fn(&Path) -> Option<Program>) -> Vec<UninstallEntry> {
    let mut programs: Vec<PathBuf> = shortcuts.iter().map(|s| s.target.clone()).collect();
    programs.extend(entries.iter().filter_map(|e| locate::web_app(e).map(|(browser, _)| browser)));
    // What Windows' list covers: folders, program files and names of its apps (not web apps,
    // whose program is the browser's).
    let listed: Vec<&UninstallEntry> = entries.iter().filter(|e| locate::web_app(e).is_none()).collect();
    let mut covered: Vec<PathBuf> = Vec::new();
    for e in &listed {
        covered.extend(locate::location_dir(e));
        covered.extend(locate::named_dir(e));
        covered.extend(locate::icon_exe(e).and_then(|x| x.parent().map(Path::to_path_buf)));
    }
    covered.extend(handled.iter().filter_map(|i| i.install_dir.clone().or_else(|| i.exe.as_ref().and_then(|x| x.parent().map(Path::to_path_buf)))));
    let names: Vec<String> = listed.iter().map(|e| super::winget::normalize_name(&e.display_name).name).filter(|n| !n.is_empty()).collect();
    let mut out: Vec<(PathBuf, Program)> = Vec::new();
    for exe in programs {
        if !locate::real_exe(&exe) || installers_own(&exe) || out.iter().any(|(_, p)| same_path(&p.exe, &exe)) {
            continue;
        }
        let Some(root) = root_of(&exe) else { continue };
        if covered.iter().any(|d| under(&exe, d) || under(d, &root)) || has_uninstaller(&exe, &root) {
            continue;
        }
        let Some(p) = read(&exe) else { continue };
        if locate::helper_exe(&exe, &p.name) || names.contains(&super::winget::normalize_name(&p.name).name) {
            continue;
        }
        // One program per copy: the one named like the app, then the biggest (a browser's own
        // program over the helper its web app shortcuts start).
        match out.iter().position(|(r, _)| same_path(r, &root)) {
            Some(k) => {
                if fit(&p) > fit(&out[k].1) {
                    out[k] = (root, p);
                }
            }
            None => out.push((root, p)),
        }
    }
    out.into_iter().map(|(root, p)| entry_for(&root, &p)).collect()
}

/// Program Files and ProgramData belong to installers: a program there without an entry of its
/// own is part of an installed app (a tool that came with it), not a portable copy.
fn installers_own(exe: &Path) -> bool {
    let mut roots = vec![crate::paths::program_files(), crate::paths::program_files_x86()];
    roots.extend(std::env::var_os("ProgramData").map(PathBuf::from));
    roots.iter().any(|r| under(exe, r))
}

fn fit(p: &Program) -> (u8, u64) {
    let stem = p.exe.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
    (locate::name_fit(&p.name, &stem), p.exe.metadata().map(|m| m.len()).unwrap_or(0))
}

/// The entry a portable copy stands for: its program's name, maker and version, its folder.
pub fn entry_for(root: &Path, p: &Program) -> UninstallEntry {
    UninstallEntry {
        key: p.exe.to_string_lossy().to_string(),
        hive: HIVE,
        display_name: p.name.clone(),
        display_version: p.version.clone(),
        publisher: p.company.clone(),
        install_location: root.to_string_lossy().to_string(),
        display_icon: p.exe.to_string_lossy().to_string(),
        written: p.exe.metadata().and_then(|m| m.modified()).ok(),
        ..Default::default()
    }
}

/// An uninstaller next to the program: the app was installed after all (its entry is in a part
/// of the registry this app does not read, or gone).
fn has_uninstaller(exe: &Path, root: &Path) -> bool {
    let dirs: Vec<&Path> = [exe.parent(), Some(root)].into_iter().flatten().collect();
    dirs.iter().any(|d| {
        std::fs::read_dir(d).map(|rd| rd.flatten().any(|f| {
            let n = f.file_name().to_string_lossy().to_ascii_lowercase();
            n.ends_with(".exe") && (n.starts_with("unins") || n.starts_with("uninstall"))
        }))
        .unwrap_or(false)
    })
}

fn norm(p: &Path) -> String {
    format!("{}\\", p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase())
}

fn same_path(a: &Path, b: &Path) -> bool {
    norm(a) == norm(b)
}

/// Is `p` inside folder `dir` (or that folder)?
fn under(p: &Path, dir: &Path) -> bool {
    norm(p).starts_with(&norm(dir))
}

/// The words that can tell builds of one app apart ("AVX2" for Thorium's AVX2 build): the copy's
/// folders, and short marker files next to its program (Thorium writes its build to
/// `BIN\thor_ver`).
pub fn build_words(e: &UninstallEntry) -> Vec<String> {
    let mut text = e.display_name.clone();
    let program = PathBuf::from(e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"'));
    for p in [PathBuf::from(e.install_location.trim().trim_matches('"')), program.clone()] {
        for c in p.components().rev().skip(if p == program { 1 } else { 0 }).take(3) {
            text.push(' ');
            text.push_str(&c.as_os_str().to_string_lossy());
        }
    }
    if let Some(dir) = program.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for f in rd.flatten() {
                let small = f.metadata().is_ok_and(|m| m.is_file() && m.len() <= 64);
                let plain = Path::new(&f.file_name()).extension().is_none_or(|x| x.eq_ignore_ascii_case("txt"));
                if small && plain {
                    if let Ok(s) = std::fs::read_to_string(f.path()) {
                        text.push(' ');
                        text.push_str(&s);
                    }
                }
            }
        }
    }
    let mut words: Vec<String> = text.split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(|w| w.to_ascii_lowercase()).collect();
    words.sort();
    words.dedup();
    words
}

// ---------------------------------------------------------------- choosing the portable zip

/// Words in a file name that say it is for another system or kind of processor.
const ELSEWHERE: &[&str] = &["linux", "mac", "macos", "osx", "darwin", "android", "apk", "deb", "rpm", "appimage", "dmg", "arm32", "armhf", "aarch64"];

/// The portable zip among a release's files that goes with the catalog's installer
/// `installer_name` ("thorium_AVX2_mini_installer.exe" gives "Thorium_AVX2_156.0.8078.28.zip"):
/// a zip for Windows on this kind of processor that shares the longest start with the
/// installer's name. None when two fit equally well.
pub fn pick_zip<'a>(assets: &'a [Asset], installer_name: &str, arch: &str) -> Option<&'a Asset> {
    let inst = installer_name.to_ascii_lowercase();
    let words = |s: &str| -> Vec<String> { s.to_ascii_lowercase().split(|c: char| !c.is_ascii_alphanumeric()).filter(|w| !w.is_empty()).map(String::from).collect() };
    let inst_words = words(&inst);
    let other_cpu = |w: &str| match arch {
        "arm64" => matches!(w, "x64" | "x86" | "i386" | "win32" | "amd64"),
        "x86" => matches!(w, "x64" | "amd64" | "arm64"),
        _ => matches!(w, "arm64" | "arm" | "x86" | "i386" | "win32") && !inst_words.iter().any(|i| i == w),
    };
    let mut scored: Vec<(usize, &Asset)> = assets
        .iter()
        .filter(|a| a.name.to_ascii_lowercase().ends_with(".zip"))
        .filter(|a| {
            let w = words(&a.name);
            !w.iter().any(|x| ELSEWHERE.contains(&x.as_str()) || other_cpu(x) || x == "symbols" || x == "debug" || x == "pdb" || x == "source" || x == "src")
        })
        .map(|a| (inst.chars().zip(a.name.to_ascii_lowercase().chars()).take_while(|(x, y)| x == y).count(), a))
        .filter(|(n, _)| *n >= 4)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    match scored.as_slice() {
        [(a, best), rest @ ..] if rest.first().is_none_or(|(b, _)| b < a) => Some(*best),
        _ => None,
    }
}

// ---------------------------------------------------------------- replacing a copy in place

/// What `replace` did, to undo it when a later part fails.
enum Done {
    Added(PathBuf),
    Replaced { at: PathBuf, old: PathBuf },
    Moved { from: PathBuf, to: PathBuf },
}

/// A folder named like a version ("156.0.8078.28"): the program files of one version.
fn version_named(name: &str) -> bool {
    let parts: Vec<&str> = name.split('.').collect();
    parts.len() >= 2 && parts.iter().all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

/// Does the update replace this file, when there is one already? Program files and anything in
/// a version's own folder: yes. Other files (settings) stay as they are.
fn replaces(rel: &Path) -> bool {
    let ext = rel.extension().map(|x| x.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    PROGRAM_EXT.contains(&ext.as_str()) || rel.parent().is_some_and(|p| p.components().any(|c| version_named(&c.as_os_str().to_string_lossy())))
}

/// The file names in the zip (its single top folder left off), with `/` as `\`.
fn zip_files(zip: &Path) -> Result<(Option<PathBuf>, Vec<PathBuf>)> {
    let f = std::fs::File::open(zip).with_context(|| format!("open {}", zip.display()))?;
    let mut ar = zip::ZipArchive::new(std::io::BufReader::new(f))?;
    let mut all = Vec::new();
    for i in 0..ar.len() {
        let e = ar.by_index(i)?;
        if e.is_dir() {
            continue;
        }
        if let Some(p) = e.enclosed_name() {
            all.push(p.to_path_buf());
        }
    }
    let top = all.first().and_then(|p| p.components().next()).map(|c| PathBuf::from(c.as_os_str())).filter(|t| all.len() > 1 && all.iter().all(|p| p.starts_with(t) && p != t));
    let files = all.into_iter().map(|p| top.as_ref().and_then(|t| p.strip_prefix(t).ok().map(Path::to_path_buf)).unwrap_or(p)).collect();
    Ok((top, files))
}

/// The program's place in the zip: the path the catalog names (`nested`), else the one file of
/// the program's name whose folders end like the copy's ("BIN\thorium.exe").
fn program_in(files: &[PathBuf], exe: &Path, nested: Option<&str>) -> Result<PathBuf> {
    let name = exe.file_name().ok_or_else(|| anyhow!("no program file"))?.to_string_lossy().to_ascii_lowercase();
    if let Some(n) = nested.map(|n| n.replace('/', "\\")).filter(|n| !n.is_empty()) {
        let want = n.trim_start_matches(".\\").to_ascii_lowercase();
        if let Some(f) = files.iter().find(|f| f.to_string_lossy().to_ascii_lowercase() == want || f.to_string_lossy().to_ascii_lowercase().ends_with(&format!("\\{want}"))) {
            return Ok(f.clone());
        }
    }
    let here: Vec<String> = exe.components().rev().map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase()).collect();
    let fits: Vec<&PathBuf> = files
        .iter()
        .filter(|f| f.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_lowercase() == name))
        .filter(|f| f.components().rev().map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase()).zip(here.iter()).all(|(a, b)| a == *b))
        .collect();
    match fits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("its portable download has no {} where the copy here has it", exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
        _ => bail!("its portable download has more than one {}", exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()),
    }
}

/// Update the portable copy whose program is `exe` from the zip of a newer (or older) version:
/// the zip is unpacked next to the copy, then its files take the place of the old ones (program
/// files and the version's own folder; files that are new are added; files the user may have
/// changed stay), and the program folders of the old version go. Anything that fails puts the
/// copy back as it was. Returns what was done.
pub fn replace(zip: &Path, exe: &Path, nested: Option<&str>, unpack: &dyn Fn(&Path, &Path, bool) -> Result<()>) -> Result<String> {
    // A portable app that is a single program file: the new file takes its place.
    if zip.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe")) {
        return replace_file(zip, exe);
    }
    let (top, files) = zip_files(zip)?;
    let rel = program_in(&files, exe, nested)?;
    // The copy's folder: the program file with the zip's path to it taken off.
    let mut root = exe.to_path_buf();
    for _ in rel.components() {
        root = root.parent().ok_or_else(|| anyhow!("{} is laid out differently from its portable download", exe.display()))?.to_path_buf();
    }
    if !same_path(&root.join(&rel), exe) || root.components().count() < 3 {
        bail!("{} is laid out differently from its portable download", exe.display());
    }
    let staging = root.join(STAGING);
    let backup = root.join(BACKUP);
    for d in [&staging, &backup] {
        if d.exists() {
            std::fs::remove_dir_all(d).with_context(|| format!("remove {}", d.display()))?;
        }
    }
    unpack(zip, &staging, top.is_some())?;
    if !staging.join(&rel).is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        bail!("the portable download did not unpack its program");
    }
    let mut done: Vec<Done> = Vec::new();
    let r = (|| -> Result<(usize, usize, Vec<String>)> {
        let (mut added, mut replaced) = (0usize, 0usize);
        move_in(&staging, &root, &backup, Path::new(""), &mut done, &mut added, &mut replaced)?;
        // The old version's program folders ("BIN\155.0.7999.12") the new one does not have.
        let mut gone = Vec::new();
        let prog_dir = rel.parent().map(|p| root.join(p)).unwrap_or_else(|| root.clone());
        let new_prog = rel.parent().map(|p| staging.join(p)).unwrap_or_else(|| staging.clone());
        if let Ok(rd) = std::fs::read_dir(&prog_dir) {
            for f in rd.flatten() {
                let n = f.file_name().to_string_lossy().to_string();
                if f.path().is_dir() && version_named(&n) && !new_prog.join(&n).exists() && !done.iter().any(|d| matches!(d, Done::Added(p) if same_path(p, &f.path()))) {
                    let to = backup.join(f.path().strip_prefix(&root).unwrap_or(Path::new(&n)));
                    if let Some(p) = to.parent() {
                        std::fs::create_dir_all(p)?;
                    }
                    std::fs::rename(f.path(), &to).with_context(|| format!("move {}", f.path().display()))?;
                    done.push(Done::Moved { from: f.path(), to });
                    gone.push(n);
                }
            }
        }
        Ok((added, replaced, gone))
    })();
    match r {
        Ok((added, replaced, gone)) => {
            let _ = std::fs::remove_dir_all(&staging);
            let _ = std::fs::remove_dir_all(&backup);
            let mut s = format!("Replaced {replaced} files, added {added}");
            if !gone.is_empty() {
                s.push_str(&format!(", removed the old version {}", gone.join(", ")));
            }
            Ok(s)
        }
        Err(e) => {
            undo(done);
            let _ = std::fs::remove_dir_all(&staging);
            let _ = std::fs::remove_dir_all(&backup);
            Err(e.context("the copy was left as it was"))
        }
    }
}

/// Move what is in `from` (the unpacked zip) into `to` (the copy), folder `rel` of both.
fn move_in(staging: &Path, root: &Path, backup: &Path, rel: &Path, done: &mut Vec<Done>, added: &mut usize, replaced: &mut usize) -> Result<()> {
    let rd = std::fs::read_dir(staging.join(rel)).with_context(|| format!("read {}", staging.join(rel).display()))?;
    for f in rd.flatten() {
        let r = rel.join(f.file_name());
        let src = staging.join(&r);
        let dst = root.join(&r);
        if src.is_dir() {
            if !dst.exists() {
                // A folder the copy does not have: moved in whole.
                std::fs::rename(&src, &dst).with_context(|| format!("move in {}", dst.display()))?;
                done.push(Done::Added(dst));
                *added += 1;
            } else if dst.is_dir() {
                move_in(staging, root, backup, &r, done, added, replaced)?;
            }
            continue;
        }
        if !dst.exists() {
            std::fs::rename(&src, &dst).with_context(|| format!("move in {}", dst.display()))?;
            done.push(Done::Added(dst));
            *added += 1;
        } else if dst.is_file() && replaces(&r) {
            let old = backup.join(&r);
            if let Some(p) = old.parent() {
                std::fs::create_dir_all(p)?;
            }
            std::fs::rename(&dst, &old).with_context(|| format!("move aside {}", dst.display()))?;
            done.push(Done::Replaced { at: dst.clone(), old });
            std::fs::rename(&src, &dst).with_context(|| format!("move in {}", dst.display()))?;
            *replaced += 1;
        }
    }
    Ok(())
}

/// `replace` for an app that is one program file.
fn replace_file(new: &Path, exe: &Path) -> Result<String> {
    let dir = exe.parent().ok_or_else(|| anyhow!("no folder"))?;
    let old = dir.join(format!("{}{BACKUP}", exe.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()));
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old).with_context(|| format!("move aside {}", exe.display()))?;
    if let Err(e) = std::fs::copy(new, exe) {
        let _ = std::fs::remove_file(exe);
        let _ = std::fs::rename(&old, exe);
        return Err(anyhow!("copy {}: {e}", exe.display()).context("the copy was left as it was"));
    }
    let _ = std::fs::remove_file(&old);
    Ok("Replaced the program file".into())
}

fn undo(done: Vec<Done>) {
    for d in done.into_iter().rev() {
        match d {
            Done::Added(p) => {
                let _ = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
            }
            Done::Replaced { at, old } => {
                let _ = std::fs::remove_file(&at);
                let _ = std::fs::rename(&old, &at);
            }
            Done::Moved { from, to } => {
                let _ = std::fs::rename(&to, &from);
            }
        }
    }
}

/// Uninstalling a portable copy: its program folder (the program file, when it sits in the copy's
/// own folder) and its Start menu and desktop shortcuts go. What is left next to it (its data,
/// its launchers) is then offered as leftovers, kept in a backup when deleted.
pub fn plan_uninstall(inst: &Installed) -> Result<Plan> {
    let exe = inst.exe.clone().ok_or_else(|| anyhow!("{} names no program file", inst.name))?;
    let dir = exe.parent().map(Path::to_path_buf).ok_or_else(|| anyhow!("{} has no folder", inst.name))?;
    let root = inst.install_dir.clone().unwrap_or_else(|| dir.clone());
    let label = format!("Uninstalling {}", inst.name);
    let elevated = crate::uninstall::Target::Dir { path: dir.clone() }.elevated();
    let mut steps = vec![if same_path(&dir, &root) { Step::RemoveFile { path: exe.clone() } } else { Step::RemoveDir { path: dir.clone(), elevated, label: label.clone() } }];
    for s in locate::shortcuts_in(&locate::start_menu_dirs().into_iter().chain(desktops()).collect::<Vec<_>>()) {
        if same_path(&s.target, &exe) {
            let target = crate::uninstall::Target::File { path: s.lnk };
            steps.push(Step::Leftover { elevated: target.elevated(), target, label: format!("Removing the shortcut {}", s.name) });
        }
    }
    Ok(Plan {
        app_id: inst.id.clone(),
        kind: JobKind::Uninstall,
        name: inst.name.clone(),
        to_version: inst.version.clone(),
        downloads: vec![],
        steps,
        block_dirs: vec![dir],
        block_names: vec![],
        verify: Verify::Removed { app_id: inst.id.clone() },
        fallback: None,
        cleanup: vec![],
    })
}

/// The program file's version now (after an update), as `read_program` gives it.
pub fn version_now(exe: &Path) -> Option<String> {
    read_program(exe).map(|p| p.version)
}

#[cfg(test)]
mod tests;
