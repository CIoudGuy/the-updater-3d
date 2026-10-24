//! After an update: are the program's own files still there? An installer can finish, and the
//! version in Windows' list can be the new one, while most of the program is gone. Git's
//! installer removes the old version before it copies the new one; when that went wrong, only a
//! few of its files were left and the update still looked fine.

use std::collections::BTreeSet;
use std::path::Path;

/// Most entries read from one folder.
const MAX_ENTRIES: usize = 4000;

/// The names of the programs in a folder and in its folders one level down ("git-bash.exe",
/// "bin\bash.exe" gives "bash.exe"), lowercase.
pub fn program_names(dir: &Path) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut seen = 0usize;
    let take = |p: &Path, out: &mut BTreeSet<String>| {
        if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
            if let Some(n) = p.file_name() {
                out.insert(n.to_string_lossy().to_ascii_lowercase());
            }
        }
    };
    let Ok(rd) = std::fs::read_dir(dir) else { return out };
    let mut subdirs = Vec::new();
    for e in rd.flatten().take(MAX_ENTRIES) {
        seen += 1;
        let p = e.path();
        match e.file_type() {
            Ok(t) if t.is_dir() => subdirs.push(p),
            Ok(t) if t.is_file() => take(&p, &mut out),
            _ => {}
        }
    }
    for d in subdirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            seen += 1;
            if seen > MAX_ENTRIES {
                return out;
            }
            if e.file_type().is_ok_and(|t| t.is_file()) {
                take(&e.path(), &mut out);
            }
        }
    }
    out
}

/// The programs an update took away, when it took most of them (a few renamed or dropped
/// programs are normal between versions). None when the program looks whole.
pub fn mostly_gone(before: &BTreeSet<String>, after: &BTreeSet<String>) -> Option<Vec<String>> {
    if before.len() < 3 {
        return None;
    }
    let gone: Vec<String> = before.difference(after).cloned().collect();
    (gone.len() * 2 > before.len()).then_some(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn an_update_that_left_most_of_the_program_out_is_caught() {
        // Git after the failed update: only git-lfs.exe was left in cmd, bin was gone.
        let before = set(&["git-bash.exe", "git-cmd.exe", "unins000.exe", "bash.exe", "git.exe", "sh.exe", "git-gui.exe", "gitk.exe", "git-lfs.exe"]);
        let after = set(&["git-bash.exe", "git-cmd.exe", "unins000.exe", "git-lfs.exe"]);
        assert_eq!(mostly_gone(&before, &after), Some(vec!["bash.exe".into(), "git-gui.exe".into(), "git.exe".into(), "gitk.exe".into(), "sh.exe".into()]));
    }

    #[test]
    fn a_normal_update_is_not_flagged() {
        let before = set(&["app.exe", "helper.exe", "updater.exe", "crash-handler.exe"]);
        // One program dropped, one new: normal.
        assert_eq!(mostly_gone(&before, &set(&["app.exe", "helper.exe", "updater.exe", "reporter.exe"])), None);
        // A small app has too few programs to judge.
        assert_eq!(mostly_gone(&set(&["app.exe", "unins000.exe"]), &set(&[])), None);
        // Squirrel apps move into a new version folder; their names stay.
        assert_eq!(mostly_gone(&set(&["update.exe", "app.exe", "squirrel.exe"]), &set(&["update.exe", "app.exe", "squirrel.exe"])), None);
    }

    #[test]
    fn programs_are_read_from_the_folder_and_one_level_down() {
        let dir = std::env::temp_dir().join(format!("u3d-integrity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bin").join("deep")).unwrap();
        for f in ["git-bash.exe", "LICENSE.txt", "bin\\bash.exe", "bin\\Git.EXE", "bin\\deep\\hidden.exe"] {
            std::fs::write(dir.join(f), b"x").unwrap();
        }
        assert_eq!(program_names(&dir), set(&["bash.exe", "git-bash.exe", "git.exe"]));
        assert!(program_names(&dir.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
