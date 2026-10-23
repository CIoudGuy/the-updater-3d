//! Deleting one leftover (in the admin helper when it needs admin rights). The safety checks run
//! again here, right before the delete, whatever the scan decided.

use super::safety::{key_refused, value_refused, Guard};
use super::{strictly_under, under, Hive, Target};
use anyhow::{anyhow, bail, Result};
use std::path::Path;

fn system32(exe: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join(exe)
}

fn run(exe: &str, args: &[&str]) -> Result<u32> {
    let out = crate::winutil::cmd(system32(exe)).args(args).output()?;
    Ok(out.status.code().map(|c| c as u32).unwrap_or(1))
}

/// A settings file `prefs` found (not a shortcut): only inside the user's AppData.
fn settings_file_ok(p: &Path, g: &Guard) -> bool {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    !home.is_empty() && strictly_under(p, &Path::new(&home).join("AppData")) && !g.trees.iter().any(|t| under(p, t))
}

/// Remove an empty Start menu folder a removed shortcut was in (never a Start menu root).
fn tidy_start_menu(file: &Path) {
    let Some(parent) = file.parent() else { return };
    let empty = std::fs::read_dir(parent).map(|mut r| r.next().is_none()).unwrap_or(false);
    let in_menu = crate::sources::locate::start_menu_dirs().iter().any(|m| strictly_under(parent, m) && !super::safety::same_dir(parent, &m.join("Startup")));
    if empty && in_menu {
        let _ = std::fs::remove_dir(parent);
    }
}

/// Remove a vendor key left empty by the removal of its last app key ("Software\Vendor").
fn tidy_key(hive: Hive, path: &str) {
    let Some((parent, _)) = path.rsplit_once('\\') else { return };
    if key_refused(hive, parent).is_some() || parent.matches('\\').count() < 1 {
        return;
    }
    let Some(k) = super::open(hive, parent) else { return };
    let empty = k.enum_keys().next().is_none() && k.enum_values().next().is_none();
    if empty {
        let _ = hive.root().delete_subkey_with_flags(parent, winreg::enums::KEY_WOW64_64KEY);
    }
}

fn valid_name(s: &str) -> bool {
    !s.is_empty() && !s.contains(['"', '\r', '\n', '\0'])
}

/// Delete one leftover. Something already gone counts as done.
pub fn apply(t: &Target) -> Result<()> {
    match t {
        Target::Dir { path } => {
            if let Some(why) = Guard::system(vec![]).dir_refused(path) {
                bail!("{} was kept: {why}", path.display());
            }
            if !path.exists() {
                return Ok(());
            }
            let mut last = None;
            for _ in 0..4 {
                match std::fs::remove_dir_all(path) {
                    Ok(()) => return Ok(()),
                    Err(e) => last = Some(e),
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
            Err(anyhow!("Could not delete {}: {}", path.display(), last.map(|e| e.to_string()).unwrap_or_default()))
        }
        Target::File { path } => {
            let g = Guard::system(vec![]);
            if g.shortcut_refused(path).is_some() && !settings_file_ok(path, &g) {
                bail!("{} was kept: not a shortcut or settings file", path.display());
            }
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(anyhow!("Could not delete {}: {e}", path.display())),
            }
            tidy_start_menu(path);
            Ok(())
        }
        Target::Key { hive, path } => {
            if let Some(why) = key_refused(*hive, path) {
                bail!("{}\\{path} was kept: {why}", hive.short());
            }
            match hive.root().open_subkey_with_flags(path, winreg::enums::KEY_ALL_ACCESS | winreg::enums::KEY_WOW64_64KEY) {
                Ok(k) => {
                    k.delete_subkey_all("").or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })?;
                    drop(k);
                    match hive.root().delete_subkey_with_flags(path, winreg::enums::KEY_WOW64_64KEY) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(anyhow!("Could not delete {}\\{path}: {e}", hive.short())),
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(anyhow!("Could not open {}\\{path}: {e}", hive.short())),
            }
            tidy_key(*hive, path);
            Ok(())
        }
        Target::Value { hive, path, name } => {
            if let Some(why) = value_refused(*hive, path) {
                bail!("{}\\{path} was kept: {why}", hive.short());
            }
            let k = match hive.root().open_subkey_with_flags(path, winreg::enums::KEY_SET_VALUE | winreg::enums::KEY_WOW64_64KEY) {
                Ok(k) => k,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                Err(e) => return Err(anyhow!("Could not open {}\\{path}: {e}", hive.short())),
            };
            match k.delete_value(name) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(anyhow!("Could not delete {name}: {e}")),
            }
        }
        Target::Service { name } => {
            if !valid_name(name) || name.contains('\\') {
                bail!("Not a service name: {name}");
            }
            let _ = run("sc.exe", &["stop", name]);
            match run("sc.exe", &["delete", name])? {
                // 1060: no such service; 1072: already marked for deletion.
                0 | 1060 | 1072 => Ok(()),
                c => Err(anyhow!("Could not remove the service {name} (sc.exe exit code {c})")),
            }
        }
        Target::Task { path } => {
            if !valid_name(path) || !path.starts_with('\\') || path.trim_matches('\\').is_empty() || path.to_ascii_lowercase().starts_with("\\microsoft\\") {
                bail!("{path} was kept: not a task of an app");
            }
            match run("schtasks.exe", &["/delete", "/tn", path, "/f"])? {
                0 => Ok(()),
                c => Err(anyhow!("Could not remove the task {path} (schtasks exit code {c})")),
            }
        }
    }
}
