//! The command that removes a program. Silent only with a switch known to be safe for its
//! installer type; any other uninstaller runs as the vendor made it, with its own window, and is
//! waited for (with the helper processes it starts).

use crate::winutil::UninstallEntry;
use std::path::Path;

/// The installer type that wrote an uninstall entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Msi,
    Nsis,
    Inno,
    /// A WiX Burn bundle (its uninstaller is the cached setup, "/uninstall").
    Burn,
    /// Squirrel (Electron apps): "Update.exe --uninstall".
    Squirrel,
    Unknown,
}

/// How the program is removed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub args: Vec<String>,
    /// No questions: the entry's own quiet command or a known silent switch.
    pub silent: bool,
    pub kind: Kind,
}

fn is_guid(s: &str) -> bool {
    let t = s.trim();
    t.len() == 38 && t.starts_with('{') && t.ends_with('}')
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// The installer type, from the entry and the first bytes of its uninstaller (`head`).
pub fn detect(e: &UninstallEntry, head: &[u8]) -> Kind {
    let u = e.uninstall_string.to_ascii_lowercase();
    if (e.windows_installer || u.contains("msiexec")) && is_guid(&e.key) {
        return Kind::Msi;
    }
    if e.key.to_ascii_lowercase().ends_with("_is1") {
        return Kind::Inno;
    }
    if e.bundle || (u.contains("\\package cache\\") && u.contains("/uninstall")) {
        return Kind::Burn;
    }
    let prog = crate::winutil::split_program_line(&e.uninstall_string);
    let file = prog.first().and_then(|p| Path::new(p).file_name().map(|f| f.to_string_lossy().to_ascii_lowercase())).unwrap_or_default();
    if file == "update.exe" && prog.iter().any(|a| a.eq_ignore_ascii_case("--uninstall")) {
        return Kind::Squirrel;
    }
    if contains(head, b"Nullsoft") || contains(head, b"NullsoftInst") {
        return Kind::Nsis;
    }
    if contains(head, b"Inno Setup") {
        return Kind::Inno;
    }
    Kind::Unknown
}

/// The first bytes of a file (what `detect` reads), or nothing.
pub fn head_of(p: &Path) -> Vec<u8> {
    use std::io::Read;
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(p) {
        let _ = f.take(8 << 20).read_to_end(&mut buf);
    }
    buf
}

fn has(v: &[String], s: &str) -> bool {
    v.iter().any(|a| a.eq_ignore_ascii_case(s))
}

/// The command that removes the program, None when the entry names none.
pub fn choose(e: &UninstallEntry, kind: Kind) -> Option<Choice> {
    let kind_str = match kind {
        Kind::Nsis => Some("nsis"),
        Kind::Inno => Some("inno"),
        _ => None,
    };
    // The entry's quiet command, Windows Installer's, Inno's and NSIS's (shared with Other apps).
    if let Some(args) = crate::sources::other::uninstall_cmd(e, kind_str) {
        return Some(Choice { args, silent: true, kind });
    }
    let mut v = crate::winutil::split_program_line(e.uninstall_string.trim());
    if v.is_empty() {
        return None;
    }
    match kind {
        Kind::Burn => {
            for s in ["/quiet", "/norestart"] {
                if !has(&v, s) {
                    v.push(s.into());
                }
            }
            Some(Choice { args: v, silent: true, kind })
        }
        Kind::Squirrel => {
            if !has(&v, "-s") && !has(&v, "--silent") {
                v.push("-s".into());
            }
            Some(Choice { args: v, silent: true, kind })
        }
        // Run as the vendor made it.
        _ => Some(Choice { args: v, silent: false, kind }),
    }
}

/// The program the entry's uninstaller runs (a full path), for checking it is still there.
pub fn uninstaller_file(e: &UninstallEntry) -> Option<std::path::PathBuf> {
    let cmd = if e.uninstall_string.trim().is_empty() { &e.quiet_uninstall_string } else { &e.uninstall_string };
    super::program_of(cmd)
}

/// Windows Installer's packed form of "{GUID}" (the reverse of `winutil::unpack_guid`).
pub fn pack_guid(g: &str) -> Option<String> {
    let h: String = g.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if h.len() != 32 || !is_guid(g) {
        return None;
    }
    let c: Vec<char> = h.chars().collect();
    let rev = |a: usize, b: usize| c[a..b].iter().rev().collect::<String>();
    let swapped = |a: usize, b: usize| c[a..b].chunks(2).map(|x| format!("{}{}", x[1], x[0])).collect::<String>();
    Some(format!("{}{}{}{}{}", rev(0, 8), rev(8, 12), rev(12, 16), swapped(16, 20), swapped(20, 32)).to_ascii_uppercase())
}

/// Why the program can not be uninstalled: its uninstaller is gone. `exists` tells whether a
/// file is there (tests give their own); `msi_package` is the cached package Windows Installer
/// needs to remove an MSI product, when it records one.
pub fn broken_reason(e: &UninstallEntry, exists: &dyn Fn(&Path) -> bool, msi_package: Option<&Path>) -> Option<String> {
    if (e.windows_installer || e.uninstall_string.to_ascii_lowercase().contains("msiexec")) && is_guid(&e.key) {
        return msi_package.filter(|p| !exists(p)).map(|_| "Windows Installer's copy of its setup is gone, so it can not uninstall it".to_string());
    }
    let un = uninstaller_file(e)?;
    let low = un.to_string_lossy().to_ascii_lowercase();
    // System programs that run the uninstall (rundll32, msiexec): their target is not checked.
    if low.contains("\\windows\\system32\\") || low.contains("\\windows\\syswow64\\") {
        return None;
    }
    // A network share or a drive that is not there right now (an unplugged disk) does not make
    // the program broken: it is just not reachable at the moment.
    if low.starts_with(r"\\") {
        return None;
    }
    let root: std::path::PathBuf = un.components().take(2).collect();
    if !exists(&root) {
        return None;
    }
    (!exists(&un)).then(|| format!("Its uninstaller {} is missing", un.display()))
}

/// The cached setup Windows Installer keeps for a machine-wide MSI product.
pub fn msi_local_package(product: &str) -> Option<std::path::PathBuf> {
    let packed = pack_guid(product)?;
    let k = super::open(super::Hive::Hklm, &format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Installer\\UserData\\S-1-5-18\\Products\\{packed}\\InstallProperties"))?;
    let v: String = k.get_value("LocalPackage").ok()?;
    (!v.trim().is_empty()).then(|| std::path::PathBuf::from(v.trim()))
}
