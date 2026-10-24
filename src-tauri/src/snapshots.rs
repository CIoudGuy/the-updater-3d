//! An app's settings, saved before it is updated or rolled back: its settings folders, files and
//! registry keys (the places `prefs` knows for it, and the settings files of a Documents folder
//! named after it, as ShareX keeps). A newer version can rewrite its settings in a form an older
//! one cannot read, so after a rollback Activity offers to put back the settings that version
//! had. Putting them back copies the saved files over the current ones and imports the saved
//! keys; nothing is deleted, and the current settings are saved first.

use crate::sources::{Installed, Step};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Kept per app (the newest)
const KEEP: usize = 3;
/// Kept at most this long
const MAX_AGE_DAYS: i64 = 90;
/// A file bigger than this is not settings (a cache, a database of the user's work)
const MAX_FILE: u64 = 25 * 1024 * 1024;
/// All saved files of one set together: an app whose settings are bigger gets no set (part of
/// them put back would mix two versions' settings)
const MAX_TOTAL: u64 = 100 * 1024 * 1024;
/// Folders inside settings folders that hold caches and logs, not settings
const SKIP_DIRS: &[&str] = &["cache", "caches", "cacheddata", "cachedextensionvsixs", "cachedprofilesdata", "code cache", "gpucache", "dxcache", "glcache", "media cache", "thumbnails", "shadercache", "dawncache", "dawngraphitecache", "dawnwebgpucache", "grshadercache", "crashpad", "crashes", "crash reports", "crashdumps", "logs", "log", "temp", "tmp", "service worker", "blob_storage", "screenshots", "backup", "backups"];
/// Files kept from a Documents folder: settings only, never the user's own work
const SETTINGS_EXT: &[&str] = &["json", "ini", "xml", "cfg", "conf", "config", "toml", "yaml", "yml", "db", "sqlite"];

/// One saved folder or file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Place {
    pub orig: PathBuf,
    /// Under the snapshot's folder
    pub stored: String,
    pub dir: bool,
    /// Only settings files (a Documents folder)
    #[serde(default)]
    pub only_settings: bool,
}

/// One saved registry key under HKEY_CURRENT_USER.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedKey {
    pub path: String,
    pub file: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Saved {
    pub id: String,
    pub app_id: String,
    pub app: String,
    /// The version that wrote these settings (the one installed when they were saved)
    pub version: String,
    /// What was installed next ("" when saved before putting other settings back)
    pub to_version: String,
    pub created: String,
    pub places: Vec<Place>,
    pub keys: Vec<SavedKey>,
    pub bytes: u64,
    #[serde(default)]
    pub restored: bool,
}

/// Where an app keeps its settings.
#[derive(Default, Clone, Debug, PartialEq)]
pub struct Locations {
    pub dirs: Vec<PathBuf>,
    pub files: Vec<PathBuf>,
    /// Folders of which only the settings files count
    pub settings_dirs: Vec<PathBuf>,
    /// Keys under HKEY_CURRENT_USER
    pub keys: Vec<String>,
}

impl Locations {
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.files.is_empty() && self.settings_dirs.is_empty() && self.keys.is_empty()
    }
}

pub fn root() -> PathBuf {
    crate::paths::data_dir().join("settings-saved")
}

/// The places `prefs` would clear for an app on "also delete settings" (its own folders, files
/// and keys; folders that need admin rights are left out), and the Documents folder named after
/// it (`documents`).
pub fn locations_from(steps: &[Step], documents: Option<&Path>, name: &str) -> Locations {
    let mut l = Locations::default();
    for s in steps {
        match s {
            Step::RemoveDir { path, elevated: false, .. } => l.dirs.push(path.clone()),
            Step::RemoveFile { path } => l.files.push(path.clone()),
            Step::RemoveUserRegKey { path, .. } => l.keys.push(path.clone()),
            _ => {}
        }
    }
    if let Some(docs) = documents {
        let d = docs.join(name.trim());
        if !name.trim().is_empty() && d.is_dir() && !l.dirs.contains(&d) {
            l.settings_dirs.push(d);
        }
    }
    l
}

/// Where an installed app keeps its settings, on this PC.
pub fn locations(inst: &Installed, others: &[Installed]) -> Locations {
    let steps = crate::prefs::steps(inst, others, &crate::prefs::Roots::system());
    let docs = std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents"));
    locations_from(&steps, docs.as_deref(), &inst.name)
}

fn skip_dir(name: &str) -> bool {
    SKIP_DIRS.iter().any(|s| s.eq_ignore_ascii_case(name.trim()))
}

fn is_settings_file(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| SETTINGS_EXT.iter().any(|s| s.eq_ignore_ascii_case(e)))
}

/// Copy a folder's files (caches and logs left out) into `dest`, within `budget` bytes (`over`
/// is set when a file did not fit).
fn copy_tree(from: &Path, dest: &Path, only_settings: bool, budget: &mut u64, over: &mut bool) -> Result<u64> {
    let mut n = 0;
    let Ok(rd) = std::fs::read_dir(from) else { return Ok(0) };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            if !skip_dir(&e.file_name().to_string_lossy()) {
                n += copy_tree(&p, &dest.join(e.file_name()), only_settings, budget, over)?;
            }
        } else if ft.is_file() && (!only_settings || is_settings_file(&p)) {
            let len = e.metadata().map(|m| m.len()).unwrap_or(0);
            if len > MAX_FILE {
                continue;
            }
            if len > *budget {
                *over = true;
                return Ok(n);
            }
            std::fs::create_dir_all(dest)?;
            // A file the app holds open (rare while it is closed) is skipped, not fatal.
            if std::fs::copy(&p, dest.join(e.file_name())).is_ok() {
                *budget -= len;
                n += len;
            }
        }
    }
    Ok(n)
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Save an app's settings at `l` in a new folder under `root`. None when there is nothing.
pub fn save_to(root: &Path, app_id: &str, app: &str, version: &str, to_version: &str, l: &Locations) -> Result<Option<Saved>> {
    if l.is_empty() {
        return Ok(None);
    }
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = root.join(&id);
    std::fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let mut budget = MAX_TOTAL;
    let mut over = false;
    let mut places = Vec::new();
    let mut bytes = 0;
    let all = l.dirs.iter().map(|d| (d, false)).chain(l.settings_dirs.iter().map(|d| (d, true)));
    for (k, (d, only)) in all.enumerate() {
        if !d.is_dir() {
            continue;
        }
        let stored = format!("files/{k}");
        let got = copy_tree(d, &dir.join(&stored), only, &mut budget, &mut over)?;
        if got > 0 {
            bytes += got;
            places.push(Place { orig: d.clone(), stored, dir: true, only_settings: only });
        }
    }
    for (k, f) in l.files.iter().enumerate() {
        let Ok(m) = std::fs::metadata(f) else { continue };
        if !m.is_file() || m.len() > MAX_FILE {
            continue;
        }
        if m.len() > budget {
            over = true;
            break;
        }
        let stored = format!("single/{k}");
        std::fs::create_dir_all(dir.join("single"))?;
        if std::fs::copy(f, dir.join(&stored)).is_ok() {
            budget -= m.len();
            bytes += m.len();
            places.push(Place { orig: f.clone(), stored, dir: false, only_settings: false });
        }
    }
    let mut keys = Vec::new();
    for (k, path) in l.keys.iter().enumerate() {
        if let Ok(text) = crate::uninstall::backup::export_key(crate::uninstall::Hive::Hkcu, path) {
            let file = format!("key{k}.reg");
            crate::uninstall::backup::write_reg(&dir.join(&file), &text)?;
            keys.push(SavedKey { path: path.clone(), file });
        }
    }
    if over {
        let _ = std::fs::remove_dir_all(&dir);
        return Err(anyhow!("{app}'s settings are bigger than {} MB: not saved", MAX_TOTAL / (1024 * 1024)));
    }
    if places.is_empty() && keys.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(None);
    }
    let s = Saved { id, app_id: app_id.into(), app: app.into(), version: version.into(), to_version: to_version.into(), created: now_iso(), places, keys, bytes, restored: false };
    write_manifest(root, &s)?;
    prune(root, app_id, chrono::Utc::now());
    Ok(Some(s))
}

/// Save an installed app's settings before it changes to `to_version`.
pub fn save_before(inst: &Installed, others: &[Installed], to_version: &str) -> Result<Option<Saved>> {
    save_to(&root(), &inst.id, &inst.name, &inst.version, to_version, &locations(inst, others))
}

pub fn write_manifest(root: &Path, s: &Saved) -> Result<()> {
    let text = serde_json::to_string_pretty(s)?;
    std::fs::write(root.join(&s.id).join("manifest.json"), text).context("write the saved settings' list")
}

/// A saved set by id (never outside `root`).
pub fn load(root: &Path, id: &str) -> Option<Saved> {
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    let text = std::fs::read_to_string(root.join(id).join("manifest.json")).ok()?;
    serde_json::from_str(&text).ok()
}

/// Every saved set, newest first.
pub fn list(root: &Path) -> Vec<Saved> {
    let mut v: Vec<Saved> = std::fs::read_dir(root).into_iter().flatten().flatten().filter_map(|e| load(root, &e.file_name().to_string_lossy())).collect();
    v.sort_by(|a, b| b.created.cmp(&a.created));
    v
}

/// The newest set of an app's settings that version `version` wrote, not yet put back.
pub fn for_version(root: &Path, app_id: &str, version: &str) -> Option<Saved> {
    list(root).into_iter().find(|s| s.app_id == app_id && !s.restored && !s.to_version.is_empty() && crate::version::cmp(&s.version, version).is_eq())
}

/// Keep the newest few sets of an app, none older than the limit.
pub fn prune(root: &Path, app_id: &str, now: chrono::DateTime<chrono::Utc>) {
    let mine: Vec<Saved> = list(root).into_iter().filter(|s| s.app_id == app_id).collect();
    for (k, s) in mine.iter().enumerate() {
        let old = chrono::DateTime::parse_from_rfc3339(&s.created).map(|t| (now - t.with_timezone(&chrono::Utc)).num_days() > MAX_AGE_DAYS).unwrap_or(false);
        if k >= KEEP || old {
            let _ = std::fs::remove_dir_all(root.join(&s.id));
        }
    }
}

/// The places and keys a set covers, to save the current settings there before putting it back.
pub fn locations_of(s: &Saved) -> Locations {
    let mut l = Locations::default();
    for p in &s.places {
        match (p.dir, p.only_settings) {
            (true, true) => l.settings_dirs.push(p.orig.clone()),
            (true, false) => l.dirs.push(p.orig.clone()),
            (false, _) => l.files.push(p.orig.clone()),
        }
    }
    l.keys = s.keys.iter().map(|k| k.path.clone()).collect();
    l
}

fn copy_back(from: &Path, to: &Path) -> Result<usize> {
    let mut n = 0;
    for e in std::fs::read_dir(from).with_context(|| format!("read {}", from.display()))?.flatten() {
        let p = e.path();
        let dest = to.join(e.file_name());
        if p.is_dir() {
            n += copy_back(&p, &dest)?;
        } else {
            std::fs::create_dir_all(to)?;
            std::fs::copy(&p, &dest).with_context(|| format!("put back {}", dest.display()))?;
            n += 1;
        }
    }
    Ok(n)
}

/// Put a saved set back: its files over the current ones (files added since stay), its keys
/// imported. Marked as put back. Returns how many files came back.
pub fn put_back(root: &Path, s: &Saved, import_key: impl Fn(&Path) -> Result<()>) -> Result<usize> {
    let dir = root.join(&s.id);
    let mut n = 0;
    for p in &s.places {
        let from = dir.join(&p.stored);
        if p.dir {
            n += copy_back(&from, &p.orig)?;
        } else {
            if let Some(parent) = p.orig.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::copy(&from, &p.orig).with_context(|| format!("put back {}", p.orig.display()))?;
            n += 1;
        }
    }
    for k in &s.keys {
        import_key(&dir.join(&k.file))?;
    }
    let mut done = s.clone();
    done.restored = true;
    write_manifest(root, &done)?;
    Ok(n)
}

/// `reg import` of a saved key (HKEY_CURRENT_USER: no admin rights).
pub fn import_reg(file: &Path) -> Result<()> {
    let reg = PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())).join("System32").join("reg.exe");
    let mut c = std::process::Command::new(reg);
    c.arg("import").arg(file);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let out = c.output().context("run reg import")?;
    if out.status.success() {
        Ok(())
    } else {
        Err(anyhow!("reg import {}: {}", file.display(), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join("updater3d-tests").join(format!("snap-{name}"));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn write(p: &Path, text: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn settings_saved_before_a_beta_come_back_after_the_rollback() {
        let t = tmp("roundtrip");
        let (root, appdata, docs) = (t.join("saved"), t.join("AppData\\Roaming\\Zapster"), t.join("Documents"));
        // Zapster 21.0.0's settings: a settings folder with a cache, and a Documents folder
        // with its settings and the user's screenshots (as ShareX keeps them).
        write(&appdata.join("settings.json"), "{\"theme\":\"dark\",\"v\":21}");
        write(&appdata.join("Cache\\big.bin"), "cache");
        write(&docs.join("Zapster\\ApplicationConfig.json"), "{\"hotkey\":\"PrintScreen\",\"v\":21}");
        write(&docs.join("Zapster\\Screenshots\\shot.png"), "png");
        write(&docs.join("Zapster\\notes.txt"), "the user's own file");
        let steps = vec![Step::RemoveDir { path: appdata.clone(), elevated: false, label: String::new() }];
        let l = locations_from(&steps, Some(&docs), "Zapster");
        assert_eq!(l.settings_dirs, vec![docs.join("Zapster")]);
        let s = save_to(&root, "other:Zap.Zapster", "Zapster", "21.0.0", "22.0.0", &l).unwrap().unwrap();
        // Caches, screenshots and the user's own files are not settings.
        let stored: Vec<PathBuf> = walk(&root.join(&s.id));
        assert!(stored.iter().any(|p| p.ends_with("settings.json")) && stored.iter().any(|p| p.ends_with("ApplicationConfig.json")), "{stored:?}");
        assert!(!stored.iter().any(|p| p.ends_with("big.bin") || p.ends_with("shot.png") || p.ends_with("notes.txt")), "{stored:?}");

        // The beta rewrites them; the user takes a new screenshot.
        write(&appdata.join("settings.json"), "{\"look\":{\"theme\":\"dark\"},\"v\":22}");
        write(&docs.join("Zapster\\ApplicationConfig.json"), "{\"hotkeys\":[\"PrintScreen\"],\"v\":22}");
        write(&docs.join("Zapster\\Screenshots\\new.png"), "png");

        // After the rollback to 21.0.0 that set is offered; not for another version or app.
        let found = for_version(&root, "other:Zap.Zapster", "21.0.0").unwrap();
        assert_eq!(found.id, s.id);
        assert!(for_version(&root, "other:Zap.Zapster", "20.0.0").is_none() && for_version(&root, "other:Other", "21.0.0").is_none());
        // The current settings are saved first, then the old ones come back; nothing is deleted.
        let current = save_to(&root, &found.app_id, &found.app, "22.0.0", "", &locations_of(&found)).unwrap().unwrap();
        let n = put_back(&root, &found, |_| Ok(())).unwrap();
        assert_eq!(n, 2);
        assert!(std::fs::read_to_string(appdata.join("settings.json")).unwrap().contains("\"v\":21"));
        assert!(std::fs::read_to_string(docs.join("Zapster\\ApplicationConfig.json")).unwrap().contains("\"v\":21"));
        assert!(docs.join("Zapster\\Screenshots\\new.png").exists() && docs.join("Zapster\\notes.txt").exists());
        // Put back once; the beta's settings can be put back in turn.
        assert!(for_version(&root, "other:Zap.Zapster", "21.0.0").is_none());
        assert!(load(&root, &current.id).is_some_and(|c| c.version == "22.0.0" && !c.restored));
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn only_the_newest_sets_of_an_app_are_kept_and_ids_stay_inside() {
        let t = tmp("prune");
        let root = t.join("saved");
        let dir = t.join("cfg");
        write(&dir.join("a.ini"), "x=1");
        let l = Locations { dirs: vec![dir], ..Default::default() };
        let mut ids = Vec::new();
        for k in 0..6 {
            let s = save_to(&root, "tools:obs", "OBS Studio", &format!("32.{k}.0"), "33.0.0", &l).unwrap().unwrap();
            // Distinct times: the list is ordered by when each set was saved.
            let mut s2 = s.clone();
            s2.created = format!("2026-10-{:02}T10:00:00Z", 10 + k);
            write_manifest(&root, &s2).unwrap();
            ids.push(s.id);
        }
        prune(&root, "tools:obs", chrono::DateTime::parse_from_rfc3339("2026-10-24T10:00:00Z").unwrap().with_timezone(&chrono::Utc));
        let left: Vec<String> = list(&root).into_iter().map(|s| s.version).collect();
        assert_eq!(left, vec!["32.5.0", "32.4.0", "32.3.0"]);
        // Older than the limit: gone too.
        prune(&root, "tools:obs", chrono::DateTime::parse_from_rfc3339("2027-02-01T10:00:00Z").unwrap().with_timezone(&chrono::Utc));
        assert!(list(&root).is_empty());
        assert!(load(&root, "..\\x").is_none() && load(&root, "").is_none());
        // Nothing to save: no set.
        assert!(save_to(&root, "a", "A", "1", "2", &Locations::default()).unwrap().is_none());
        let _ = std::fs::remove_dir_all(&t);
    }

    fn walk(d: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
        out
    }
}
