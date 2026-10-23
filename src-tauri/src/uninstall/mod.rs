//! Uninstalling any program on the PC, not only the ones from a catalog, and cleaning up after it.
//!
//! - `programs`: every program in the uninstall list that no source lists ("Other programs").
//! - `switches`: the command that removes a program, silently when its installer type is known.
//! - `leftovers`: what is left after the uninstaller ran (folder, shortcuts, registry keys,
//!   services, tasks, startup entries, file types), shown first and deleted only when ticked.
//! - `safety`: places that are never deleted, whatever a scan finds.
//! - `backup`: registry keys and tasks are saved before they are deleted, to restore them.
//! - `run`: the scan, delete and restore commands of the window.

pub mod backup;
pub mod leftovers;
pub mod programs;
pub mod remove;
pub mod run;
pub mod safety;
pub mod switches;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[cfg(test)]
mod probe;
#[cfg(test)]
mod tests;

/// A registry hive. Paths are kept as Windows stores them (32-bit keys under "WOW6432Node"), and
/// opened in the 64-bit view, so no path is redirected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Hive {
    Hkcu,
    Hklm,
}

impl Hive {
    pub fn root(self) -> winreg::RegKey {
        use winreg::enums::*;
        winreg::RegKey::predef(match self {
            Hive::Hkcu => HKEY_CURRENT_USER,
            Hive::Hklm => HKEY_LOCAL_MACHINE,
        })
    }
    pub fn long(self) -> &'static str {
        match self {
            Hive::Hkcu => "HKEY_CURRENT_USER",
            Hive::Hklm => "HKEY_LOCAL_MACHINE",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Hive::Hkcu => "HKCU",
            Hive::Hklm => "HKLM",
        }
    }
    /// The hive of an uninstall entry ("HKLM64", "HKLM32", "HKCU").
    pub fn of_entry(hive: &str) -> Hive {
        if hive == "HKCU" {
            Hive::Hkcu
        } else {
            Hive::Hklm
        }
    }
}

/// Open a key to read (64-bit view, see `Hive`).
pub fn open(hive: Hive, path: &str) -> Option<winreg::RegKey> {
    use winreg::enums::*;
    hive.root().open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY).ok()
}

pub fn key_exists(hive: Hive, path: &str) -> bool {
    open(hive, path).is_some()
}

/// The uninstall entry's key as Windows stores it.
pub fn entry_key_path(hive: &str, key: &str) -> String {
    match hive {
        "HKLM32" => format!("SOFTWARE\\WOW6432Node\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{key}"),
        "HKCU" => format!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{key}"),
        _ => format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\{key}"),
    }
}

/// One thing to delete.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "k")]
pub enum Target {
    Dir { path: PathBuf },
    File { path: PathBuf },
    Key { hive: Hive, path: String },
    Value { hive: Hive, path: String, name: String },
    /// A Windows service, by its key name.
    Service { name: String },
    /// A scheduled task, by its full path ("\Vendor\Updater").
    Task { path: String },
}

impl Target {
    /// Needs admin rights: anything outside the user's own profile and registry hive.
    pub fn elevated(&self) -> bool {
        match self {
            Target::Dir { path } | Target::File { path } => !in_profile(path),
            Target::Key { hive, .. } | Target::Value { hive, .. } => *hive == Hive::Hklm,
            Target::Service { .. } | Target::Task { .. } => true,
        }
    }
    /// A stable id for the window's check boxes.
    pub fn id(&self) -> String {
        let s = match self {
            Target::Dir { path } => format!("dir:{}", path.display()),
            Target::File { path } => format!("file:{}", path.display()),
            Target::Key { hive, path } => format!("key:{}\\{path}", hive.short()),
            Target::Value { hive, path, name } => format!("value:{}\\{path}\\{name}", hive.short()),
            Target::Service { name } => format!("service:{name}"),
            Target::Task { path } => format!("task:{path}"),
        };
        s.to_ascii_lowercase()
    }
}

fn in_profile(p: &Path) -> bool {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    !home.is_empty() && under(p, Path::new(&home))
}

/// Lowercase path with backslashes and no trailing one.
pub fn norm(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

/// `p` is `dir` or inside it.
pub fn under(p: &Path, dir: &Path) -> bool {
    let (a, b) = (norm(p), norm(dir));
    !b.is_empty() && (a == b || a.starts_with(&format!("{b}\\")))
}

/// `p` is inside `dir` (not `dir` itself).
pub fn strictly_under(p: &Path, dir: &Path) -> bool {
    let (a, b) = (norm(p), norm(dir));
    !b.is_empty() && a.starts_with(&format!("{b}\\"))
}

/// The program a command line starts, with environment variables filled in ("%ProgramFiles%").
pub fn program_of(command: &str) -> Option<PathBuf> {
    let first = crate::winutil::split_program_line(command).into_iter().next()?;
    let p = expand_env(first.trim().trim_matches('"'));
    let p = p.strip_prefix("\\??\\").map(str::to_string).unwrap_or(p);
    let p = match p.to_ascii_lowercase().strip_prefix("\\systemroot\\") {
        Some(_) => format!("{}\\{}", std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()), &p[12..]),
        None => p,
    };
    let path = PathBuf::from(&p);
    // Only full paths ("C:\..."): a bare name is found on PATH, which tells nothing.
    (p.len() > 3 && p.as_bytes().get(1) == Some(&b':')).then_some(path)
}

/// "%ProgramFiles%\X" with the variables filled in; unknown ones stay as they are.
pub fn expand_env(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(a) = rest.find('%') {
        out.push_str(&rest[..a]);
        let after = &rest[a + 1..];
        match after.find('%') {
            Some(b) => {
                let name = &after[..b];
                match std::env::var(name) {
                    Ok(v) if !name.is_empty() => out.push_str(&v),
                    _ => {
                        out.push('%');
                        out.push_str(name);
                        out.push('%');
                    }
                }
                rest = &after[b + 1..];
            }
            None => {
                out.push_str(&rest[a..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}
