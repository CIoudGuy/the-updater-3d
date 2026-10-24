//! Installing, updating and repairing a game without its launcher (Epic Games, GOG): the parts
//! every platform's engine shares. A sync brings a game folder to one build of the game: it
//! downloads what is missing or different, checks every file it writes against the build's own
//! hashes, and keeps the files it replaces in the game's undo folder until the next sync, so the
//! update can be put back. Nothing outside the game folder is written.
//!
//! A game the app installed has `.u3d\game.json` in its folder (`State`): which platform and
//! game it is and which build is installed. Games a launcher installed are left to the launcher.

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// What a sync does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Into an empty (or new) folder
    Install,
    /// From the installed build to a newer one: only what changed is downloaded
    Update,
    /// Check every file of the installed build and download the ones that are missing or wrong
    Repair,
}

/// How far a sync is, for the queue's progress line.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub done_bytes: u64,
    pub total_bytes: u64,
    pub files_done: usize,
    pub files_total: usize,
    /// "Downloading", "Checking files", "Writing files"
    pub step: String,
}

/// What a finished sync did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    /// The build now installed, as people know it
    pub version: String,
    pub bytes_downloaded: u64,
    pub files_written: usize,
    /// Files a repair found missing or wrong (and wrote again)
    pub files_repaired: usize,
    /// The undo folder of this sync, when it replaced or added files
    pub undo: Option<PathBuf>,
}

/// Which game is in a folder the app installed it into, and which build.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    /// "epic" or "gog"
    pub platform: String,
    /// The platform's id of the game (Epic's app name, GOG's product id)
    pub game: String,
    pub name: String,
    pub version: String,
    /// The platform's id of the installed build (Epic's build version, GOG's build id)
    pub build: String,
    /// The program that starts the game, relative to its folder
    #[serde(default)]
    pub launch: String,
    /// Arguments the platform names for starting it
    #[serde(default)]
    pub launch_args: Vec<String>,
}

/// The app's own folder inside a game folder.
pub const OWN_DIR: &str = ".u3d";
/// The files an update replaced or added, kept to put the update back.
pub const UNDO_DIR: &str = "undo";

pub fn state_file(dest: &Path) -> PathBuf {
    dest.join(OWN_DIR).join("game.json")
}

pub fn read_state(dest: &Path) -> Option<State> {
    serde_json::from_slice(&std::fs::read(state_file(dest)).ok()?).ok()
}

pub fn write_state(dest: &Path, s: &State) -> Result<()> {
    let f = state_file(dest);
    std::fs::create_dir_all(f.parent().unwrap_or(dest))?;
    let tmp = f.with_extension("json.part");
    std::fs::write(&tmp, serde_json::to_vec_pretty(s)?)?;
    std::fs::rename(&tmp, &f)?;
    Ok(())
}

/// A path from a build's file list, made safe to join to the game folder: relative, with no
/// "..", drive or root in it. None for anything else (a build never names such a path; a broken
/// or hostile one is refused).
pub fn safe_rel(rel: &str) -> Option<PathBuf> {
    let rel = rel.trim().replace('/', "\\");
    if rel.is_empty() || rel.contains(':') || rel.starts_with('\\') {
        return None;
    }
    let p = PathBuf::from(&rel);
    let ok = p.components().all(|c| matches!(c, Component::Normal(_)));
    let first_is_own = p.components().next().is_some_and(|c| c.as_os_str().eq_ignore_ascii_case(OWN_DIR));
    (ok && !first_is_own).then_some(p)
}

/// What an undo folder holds: the build it puts back, the files it restores and the files the
/// update added (deleted on undo).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UndoLog {
    pub from: Option<State>,
    pub replaced: Vec<String>,
    pub added: Vec<String>,
}

/// The undo folder of a game folder.
pub fn undo_dir(dest: &Path) -> PathBuf {
    dest.join(OWN_DIR).join(UNDO_DIR)
}

/// Keeps the files a sync replaces, to put them back. One per game folder: a new sync starts it
/// over (the update before is then final).
pub struct Undo {
    dest: PathBuf,
    log: UndoLog,
}

impl Undo {
    /// Starts the undo folder of a sync from `from` (the build installed now, if any).
    pub fn begin(dest: &Path, from: Option<State>) -> Result<Undo> {
        let d = undo_dir(dest);
        if d.exists() {
            std::fs::remove_dir_all(&d).with_context(|| format!("Could not clear {}", d.display()))?;
        }
        std::fs::create_dir_all(&d)?;
        let u = Undo { dest: dest.to_path_buf(), log: UndoLog { from, ..Default::default() } };
        u.save()?;
        Ok(u)
    }

    fn save(&self) -> Result<()> {
        std::fs::write(undo_dir(&self.dest).join("undo.json"), serde_json::to_vec_pretty(&self.log)?)?;
        Ok(())
    }

    /// Before `rel` is written: an existing file is moved into the undo folder, a new one noted
    /// as added. Call once per file, before writing it.
    pub fn keep(&mut self, rel: &Path) -> Result<()> {
        let key = rel.to_string_lossy().replace('/', "\\");
        if self.log.replaced.contains(&key) || self.log.added.contains(&key) {
            return Ok(());
        }
        let src = self.dest.join(rel);
        if src.is_file() {
            let to = undo_dir(&self.dest).join("files").join(rel);
            std::fs::create_dir_all(to.parent().unwrap_or(&self.dest))?;
            // A move on the same drive: instant, and no copy of a big file.
            std::fs::rename(&src, &to).or_else(|_| std::fs::copy(&src, &to).map(|_| ()).and_then(|_| std::fs::remove_file(&src)))?;
            self.log.replaced.push(key);
        } else {
            self.log.added.push(key);
        }
        self.save()
    }

    /// A file the new build no longer has: moved into the undo folder too.
    pub fn remove(&mut self, rel: &Path) -> Result<()> {
        if self.dest.join(rel).is_file() {
            self.keep(rel)?;
            // keep() noted it as replaced and moved it away; nothing is written in its place.
        }
        Ok(())
    }

    /// The undo folder when the sync changed anything, else None (and the folder is gone).
    pub fn finish(self) -> Result<Option<PathBuf>> {
        let d = undo_dir(&self.dest);
        if self.log.replaced.is_empty() && self.log.added.is_empty() {
            let _ = std::fs::remove_dir_all(&d);
            return Ok(None);
        }
        self.save()?;
        Ok(Some(d))
    }
}

/// Can the last update of this game folder be put back?
pub fn can_undo(dest: &Path) -> Option<UndoLog> {
    let log: UndoLog = serde_json::from_slice(&std::fs::read(undo_dir(dest).join("undo.json")).ok()?).ok()?;
    (!log.replaced.is_empty() || !log.added.is_empty()).then_some(log)
}

/// Puts the last update back: the files it added are deleted, the ones it replaced restored,
/// and the folder's state is the build before. The undo folder is gone afterwards.
pub fn undo(dest: &Path) -> Result<Option<State>> {
    let log = can_undo(dest).ok_or_else(|| anyhow!("There is no update to put back"))?;
    let kept = undo_dir(dest).join("files");
    for rel in &log.added {
        let Some(p) = safe_rel(rel) else { continue };
        let f = dest.join(&p);
        if f.is_file() {
            std::fs::remove_file(&f).with_context(|| format!("Could not remove {}", f.display()))?;
        }
    }
    for rel in &log.replaced {
        let Some(p) = safe_rel(rel) else { continue };
        let from = kept.join(&p);
        let to = dest.join(&p);
        if !from.is_file() {
            bail!("The kept copy of {} is missing", p.display());
        }
        std::fs::create_dir_all(to.parent().unwrap_or(dest))?;
        if to.exists() {
            std::fs::remove_file(&to)?;
        }
        std::fs::rename(&from, &to).or_else(|_| std::fs::copy(&from, &to).map(|_| ()))?;
    }
    if let Some(s) = &log.from {
        write_state(dest, s)?;
    }
    let _ = std::fs::remove_dir_all(undo_dir(dest));
    Ok(log.from)
}

/// Stop when the job is canceled.
pub fn check_cancel(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("CANCELED|Canceled");
    }
    Ok(())
}

/// Write `bytes` to `rel` in the game folder: into a part file first, renamed when whole, so a
/// cut-off sync never leaves a half file under the real name.
pub fn write_file(dest: &Path, rel: &Path, bytes: &[u8]) -> Result<()> {
    let to = dest.join(rel);
    std::fs::create_dir_all(to.parent().unwrap_or(dest))?;
    let part = to.with_extension(format!("{}u3dpart", to.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()));
    std::fs::write(&part, bytes)?;
    if to.exists() {
        std::fs::remove_file(&to)?;
    }
    std::fs::rename(&part, &to)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("u3d-sync-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_builds_paths_stay_inside_the_game_folder() {
        assert_eq!(safe_rel("FortniteGame/Binaries/Win64/Game.exe"), Some(PathBuf::from(r"FortniteGame\Binaries\Win64\Game.exe")));
        for bad in ["../evil.exe", r"..\evil.exe", "C:/Windows/evil.dll", r"\Windows\evil.dll", "a/../../b", "", ".u3d/game.json", r".U3D\undo\undo.json"] {
            assert_eq!(safe_rel(bad), None, "{bad}");
        }
    }

    #[test]
    fn an_update_can_be_put_back_file_for_file() {
        let d = tmp("undo");
        std::fs::write(d.join("game.exe"), b"old exe").unwrap();
        std::fs::create_dir_all(d.join("data")).unwrap();
        std::fs::write(d.join("data").join("old.pak"), b"old pak").unwrap();
        let before = State { platform: "epic".into(), game: "Sugar".into(), name: "Game".into(), version: "1.0".into(), build: "b1".into(), launch: "game.exe".into(), launch_args: vec![] };
        write_state(&d, &before).unwrap();
        // The update: game.exe replaced, new.pak added, old.pak dropped.
        let mut u = Undo::begin(&d, read_state(&d)).unwrap();
        u.keep(Path::new("game.exe")).unwrap();
        write_file(&d, Path::new("game.exe"), b"new exe").unwrap();
        u.keep(Path::new(r"data\new.pak")).unwrap();
        write_file(&d, Path::new(r"data\new.pak"), b"new pak").unwrap();
        u.remove(Path::new(r"data\old.pak")).unwrap();
        assert!(u.finish().unwrap().is_some());
        write_state(&d, &State { version: "2.0".into(), build: "b2".into(), ..before.clone() }).unwrap();
        assert_eq!(std::fs::read(d.join("game.exe")).unwrap(), b"new exe");
        assert!(!d.join("data").join("old.pak").exists());
        assert!(can_undo(&d).is_some());
        // Undo: back to 1.0 exactly.
        let back = undo(&d).unwrap();
        assert_eq!(back.map(|s| s.version).as_deref(), Some("1.0"));
        assert_eq!(std::fs::read(d.join("game.exe")).unwrap(), b"old exe");
        assert_eq!(std::fs::read(d.join("data").join("old.pak")).unwrap(), b"old pak");
        assert!(!d.join("data").join("new.pak").exists());
        assert_eq!(read_state(&d).map(|s| s.version).as_deref(), Some("1.0"));
        assert!(can_undo(&d).is_none(), "an undo is done once");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_sync_that_changed_nothing_keeps_no_undo_folder() {
        let d = tmp("nothing");
        let u = Undo::begin(&d, None).unwrap();
        assert_eq!(u.finish().unwrap(), None);
        assert!(!undo_dir(&d).exists());
        assert!(undo(&d).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_is_written_whole_or_not_at_all() {
        let d = tmp("write");
        write_file(&d, Path::new(r"a\b.bin"), b"first").unwrap();
        write_file(&d, Path::new(r"a\b.bin"), b"second").unwrap();
        assert_eq!(std::fs::read(d.join("a").join("b.bin")).unwrap(), b"second");
        let leftovers: Vec<_> = std::fs::read_dir(d.join("a")).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains("u3dpart")).collect();
        assert!(leftovers.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
