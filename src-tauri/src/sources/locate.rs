//! Where an installed app's program file and folder are, when its uninstall entry does not say:
//! Windows Installer's own records, the App Paths list, Start menu shortcuts and the files in
//! its folder. Paths into installer caches, temp and download folders never count.

use super::winget;
use crate::winutil::UninstallEntry;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// A place that holds installers or their leftovers, never the installed program: Burn's and
/// Windows Installer's caches, temp and download folders, and Store packages (another app's).
pub fn bad_place(p: &str) -> bool {
    let low = format!("{}\\", p.trim().trim_matches('"').replace('/', "\\").to_ascii_lowercase());
    ["\\package cache\\", "\\windows\\installer\\", "\\temp\\", "\\tmp\\", "\\downloads\\", "\\installshield installation information\\", "\\windows\\downloaded installations\\", "\\advanced installer\\", "\\windowsapps\\"].iter().any(|b| low.contains(b))
}

/// A program that comes with an app but is not the app: uninstallers, installers, updaters,
/// crash reporters, helpers and runtimes. A word the app's own name has does not count
/// ("Display Driver Uninstaller.exe" is that app).
pub fn helper_exe(p: &Path, app: &str) -> bool {
    let stem = p.file_stem().map(|s| s.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let own: String = app.to_ascii_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    const WORDS: &[&str] = &[
        "unins", "uninst", "setup", "install", "update", "crash", "report", "helper", "elevat", "vc_redist", "vcredist", "dotnet", "redist", "maintenance", "repair", "webengineprocess", "cefsharp", "notification", "service", "squirrel", "createdump", "sendlog", "watchdog", "start", "stop",
    ];
    stem.is_empty() || WORDS.iter().any(|w| stem.contains(w) && !own.contains(&w.replace('_', "")))
}

fn is_exe(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("exe"))
}

fn in_windows(p: &Path) -> bool {
    let w = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into()).to_ascii_lowercase();
    p.to_string_lossy().to_ascii_lowercase().starts_with(&format!("{w}\\"))
}

/// A program file that is there, outside the installer caches and Windows itself.
pub fn real_exe(p: &Path) -> bool {
    is_exe(p) && !bad_place(&p.to_string_lossy()) && !in_windows(p) && p.is_file()
}

/// A program file that can be the app's: there, outside the caches, and not one of its helpers.
pub fn good_exe(p: &Path, app: &str) -> bool {
    real_exe(p) && !helper_exe(p, app)
}

/// How well a name (a shortcut's, a program file's) fits the app's name: 2 the same, 1 one
/// starts or ends with the other, 0 not.
pub fn name_fit(app: &str, other: &str) -> u8 {
    let a = winget::normalize_name(app).name;
    let o = winget::normalize_name(other).name;
    if a.is_empty() || o.is_empty() {
        return 0;
    }
    if a == o {
        return 2;
    }
    let (short, long) = if a.len() < o.len() { (&a, &o) } else { (&o, &a) };
    u8::from(short.len() >= 4 && (long.starts_with(short.as_str()) || long.ends_with(short.as_str())))
}

fn stem_fit(app: &str, p: &Path) -> u8 {
    name_fit(app, &p.file_stem().unwrap_or_default().to_string_lossy())
}

fn size(p: &Path) -> u64 {
    p.metadata().map(|m| m.len()).unwrap_or(0)
}

/// The app's own program in a folder: the one whose name fits the app best (the bigger one when
/// several fit as well), else the only one, else the biggest. Looked for in the folder, then one
/// level below it ("bin", "program").
pub fn main_exe_in(dir: &Path, app: &str) -> Option<PathBuf> {
    if bad_place(&dir.to_string_lossy()) || in_windows(dir) {
        return None;
    }
    let list = |d: &Path| -> Vec<PathBuf> { std::fs::read_dir(d).map(|r| r.flatten().map(|e| e.path()).filter(|p| good_exe(p, app)).collect()).unwrap_or_default() };
    let fitting = |v: &[PathBuf]| v.iter().filter(|p| stem_fit(app, p) > 0).max_by_key(|p| (stem_fit(app, p), size(p))).cloned();
    let top = list(dir);
    if let Some(p) = fitting(&top) {
        return Some(p);
    }
    if top.len() == 1 {
        return top.into_iter().next();
    }
    let subs: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    let below: Vec<PathBuf> = subs.iter().flat_map(|s| list(s)).collect();
    if let Some(p) = fitting(&below) {
        return Some(p);
    }
    top.into_iter().max_by_key(|p| size(p))
}

// ---------------------------------------------------------------- what Windows knows

/// Windows Installer's install folder of a product ("{GUID}"), when the product recorded one.
pub fn msi_install_location(product: &str) -> Option<PathBuf> {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::System::ApplicationInstallationAndServicing::{MsiGetProductInfoW, INSTALLPROPERTY_INSTALLLOCATION};
    let code = HSTRING::from(product.trim());
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let r = unsafe { MsiGetProductInfoW(&code, INSTALLPROPERTY_INSTALLLOCATION, Some(PWSTR(buf.as_mut_ptr())), Some(&mut len)) };
    if r != 0 || len == 0 {
        return None;
    }
    let s = String::from_utf16_lossy(&buf[..len as usize]);
    let d = PathBuf::from(s.trim().trim_end_matches('\\'));
    (d.is_dir() && !bad_place(&d.to_string_lossy())).then_some(d)
}

/// The program an advertised (Windows Installer) shortcut starts: its component's key file.
fn msi_shortcut_target(lnk: &Path) -> Option<PathBuf> {
    use windows::core::{HSTRING, PWSTR};
    use windows::Win32::System::ApplicationInstallationAndServicing::{MsiGetComponentPathW, MsiGetShortcutTargetW, INSTALLSTATE_LOCAL};
    let mut product = [0u16; 39];
    let mut component = [0u16; 39];
    let r = unsafe { MsiGetShortcutTargetW(&HSTRING::from(lnk.as_os_str()), Some(PWSTR(product.as_mut_ptr())), None, Some(PWSTR(component.as_mut_ptr()))) };
    if r != 0 {
        return None;
    }
    let mut buf = vec![0u16; 1024];
    let mut len = buf.len() as u32;
    let st = unsafe { MsiGetComponentPathW(PWSTR(product.as_mut_ptr()), PWSTR(component.as_mut_ptr()), Some(PWSTR(buf.as_mut_ptr())), Some(&mut len)) };
    if st != INSTALLSTATE_LOCAL || len == 0 {
        return None;
    }
    Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
}

/// The program a shortcut starts (read only; the shortcut is not "resolved", which could search).
pub fn shortcut_target(lnk: &Path) -> Option<PathBuf> {
    use windows::core::{Interface, HSTRING};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, STGM_READ};
    use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
    if let Some(p) = msi_shortcut_target(lnk) {
        return Some(p);
    }
    unsafe {
        // Joins the thread to the process's multithreaded apartment and stays there: tearing it
        // down again could end that apartment under other threads that use it without joining
        // (WinRT calls such as the Store package list), which crashes them.
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let r = (|| -> windows::core::Result<PathBuf> {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.cast::<IPersistFile>()?.Load(&HSTRING::from(lnk.as_os_str()), STGM_READ)?;
            let mut buf = [0u16; 1024];
            link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
            let n = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
            Ok(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
        })();
        r.ok().filter(|p| !p.as_os_str().is_empty())
    }
}

/// A Start menu shortcut: its name and the program it starts.
#[derive(Clone, Debug)]
pub struct Shortcut {
    pub name: String,
    pub target: PathBuf,
    /// The shortcut file
    pub lnk: PathBuf,
}

/// The Start menu folders for all users and for this user.
pub fn start_menu_dirs() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(p) = std::env::var_os("ProgramData") {
        v.push(PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    if let Some(p) = std::env::var_os("APPDATA") {
        v.push(PathBuf::from(p).join("Microsoft\\Windows\\Start Menu\\Programs"));
    }
    v
}

/// Shortcut targets already read, by shortcut file and its time stamp.
static LNK_MEMO: Lazy<Mutex<HashMap<PathBuf, (Option<SystemTime>, Option<PathBuf>)>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// The shortcuts in these folders (three levels deep) that start a usable program file.
pub fn shortcuts_in(dirs: &[PathBuf]) -> Vec<Shortcut> {
    fn walk(d: &Path, depth: u8, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(d) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, out);
                }
            } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lnk")) {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    for d in dirs {
        walk(d, 3, &mut files);
    }
    let mut memo = LNK_MEMO.lock();
    files
        .into_iter()
        .filter_map(|f| {
            let stamp = f.metadata().and_then(|m| m.modified()).ok();
            let target = match memo.get(&f) {
                Some((s, t)) if *s == stamp => t.clone(),
                _ => {
                    let t = shortcut_target(&f);
                    memo.insert(f.clone(), (stamp, t.clone()));
                    t
                }
            };
            let target = target.filter(|t| real_exe(t))?;
            Some(Shortcut { name: f.file_stem()?.to_string_lossy().into_owned(), target, lnk: f })
        })
        .collect()
}

/// The App Paths list (programs registered to start by name): program file of each.
pub fn app_paths() -> Vec<PathBuf> {
    use winreg::enums::*;
    use winreg::RegKey;
    const PATH: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths";
    let mut out = Vec::new();
    let views = [(HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_64KEY), (HKEY_LOCAL_MACHINE, KEY_READ | KEY_WOW64_32KEY), (HKEY_CURRENT_USER, KEY_READ)];
    for (root, flags) in views {
        let Ok(base) = RegKey::predef(root).open_subkey_with_flags(PATH, flags) else { continue };
        for name in base.enum_keys().flatten() {
            let Ok(k) = base.open_subkey_with_flags(&name, flags) else { continue };
            let Ok(v) = k.get_value::<String, _>("") else { continue };
            let p = crate::winutil::pathbuf(v.trim().trim_matches('"'));
            if real_exe(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// What is read once per scan for every app: shortcuts and App Paths.
#[derive(Clone, Debug, Default)]
pub struct Hints {
    pub shortcuts: Vec<Shortcut>,
    pub app_paths: Vec<PathBuf>,
}

impl Hints {
    pub fn read() -> Hints {
        Hints { shortcuts: shortcuts_in(&start_menu_dirs()), app_paths: app_paths() }
    }
}

// ---------------------------------------------------------------- putting it together

/// The program file the entry's icon names, when it is a usable one.
pub fn icon_exe(e: &UninstallEntry) -> Option<PathBuf> {
    let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"');
    let p = crate::winutil::pathbuf(icon);
    good_exe(&p, &e.display_name).then_some(p)
}

/// The entry's install folder, when it names a real one outside the installer caches.
pub fn location_dir(e: &UninstallEntry) -> Option<PathBuf> {
    let l = e.install_location.trim().trim_matches('"');
    if l.is_empty() || bad_place(l) {
        return None;
    }
    let d = crate::winutil::pathbuf(l);
    (d.is_dir() && d.components().count() >= 3 && !in_windows(&d)).then_some(d)
}

/// The folder of the program the entry's icon or uninstaller names (an uninstaller usually sits
/// in the app's folder), outside the installer caches and Windows.
pub fn named_dir(e: &UninstallEntry) -> Option<PathBuf> {
    let icon = e.display_icon.split(',').next().unwrap_or("").trim().trim_matches('"').to_string();
    let un = crate::winutil::split_program_line(&e.uninstall_string).into_iter().next().unwrap_or_default();
    [icon, un].into_iter().find_map(|f| {
        let p = crate::winutil::pathbuf(&f);
        if !is_exe(&p) || bad_place(&f) || in_windows(&p) || !p.is_file() {
            return None;
        }
        p.parent().map(Path::to_path_buf).filter(|d| d.components().count() >= 3)
    })
}

fn under(p: &Path, dir: &Path) -> bool {
    let n = |x: &Path| format!("{}\\", x.to_string_lossy().trim_end_matches('\\').to_ascii_lowercase());
    n(p).starts_with(&n(dir))
}

/// Does a shortcut fit this version of the app? A shortcut that names a version ("Python 3.10")
/// only fits that version.
fn version_fits(shortcut: &str, version: &str) -> bool {
    let v = version.trim().trim_start_matches(['v', 'V']);
    shortcut.split(|c: char| !(c.is_ascii_digit() || c == '.')).filter(|t| t.contains('.') && t.split('.').all(|x| !x.is_empty())).all(|t| v == t || v.starts_with(&format!("{t}.")))
}

/// The app's program file and install folder. `msi` are the Windows Installer products the app
/// is made of (the entry's own, a bundle's packages). In order: the program the entry's icon
/// names; in the install folder (the entry's, Windows Installer's, or its uninstaller's), the
/// program a Start menu shortcut or App Paths names, else the one that fits the app's name;
/// with no folder known, the program of the shortcut or App Paths entry named like the app.
/// A web app a Chromium browser (Chrome, Edge, Brave, Thorium) installed: its entry's uninstall
/// command is the browser with "--uninstall-app-id=<id>". (the browser's program, the app's id)
pub fn web_app(e: &UninstallEntry) -> Option<(PathBuf, String)> {
    let args = crate::winutil::split_program_line(&e.uninstall_string);
    let id = args.iter().find_map(|a| a.strip_prefix("--uninstall-app-id=")).filter(|id| !id.is_empty())?.to_string();
    Some((crate::winutil::pathbuf(args.first()?), id))
}

pub fn locate(e: &UninstallEntry, msi: &[String], hints: &Hints) -> (Option<PathBuf>, Option<PathBuf>) {
    // A web app has no program or folder of its own: the browser's are the browser's.
    if web_app(e).is_some() {
        return (None, None);
    }
    let name = e.display_name.as_str();
    let mut dir = location_dir(e).or_else(|| msi.iter().find_map(|c| msi_install_location(c))).or_else(|| named_dir(e));
    let mut exe = icon_exe(e).filter(|x| dir.as_ref().is_none_or(|d| under(x, d)));
    let fit = |s: &Shortcut| name_fit(name, &s.name).max(stem_fit(name, &s.target));
    if exe.is_none() {
        if let Some(d) = &dir {
            exe = hints.shortcuts.iter().filter(|s| under(&s.target, d) && !helper_exe(&s.target, name) && !helper_exe(Path::new(&s.name), name)).max_by_key(|s| (fit(s), size(&s.target))).map(|s| s.target.clone());
            exe = exe.or_else(|| hints.app_paths.iter().filter(|p| under(p, d) && !helper_exe(p, name)).max_by_key(|p| (stem_fit(name, p), size(p))).cloned());
            exe = exe.or_else(|| main_exe_in(d, name));
        }
    }
    if exe.is_none() && dir.is_none() {
        // Only a shortcut named like the app, for this version of it, and only when that leaves
        // one program.
        let mut found: Vec<&PathBuf> = hints.shortcuts.iter().filter(|s| fit(s) == 2 && version_fits(&s.name, &e.display_version) && !helper_exe(&s.target, name)).map(|s| &s.target).collect();
        found.extend(hints.app_paths.iter().filter(|p| stem_fit(name, p) == 2 && !helper_exe(p, name)));
        found.sort_by_key(|p| p.to_string_lossy().to_ascii_lowercase());
        found.dedup_by_key(|p| p.to_string_lossy().to_ascii_lowercase());
        if found.len() == 1 {
            exe = Some(found[0].clone());
        }
    }
    if dir.is_none() {
        dir = exe.as_ref().and_then(|x| x.parent()).map(Path::to_path_buf).filter(|d| d.components().count() >= 3);
    }
    (exe, dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        // Next to the test program: a folder under Temp never counts as an install folder.
        let d = std::env::current_exe().unwrap().parent().unwrap().join(format!("locate-unit-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn installer_caches_temp_and_downloads_are_never_the_program() {
        assert!(bad_place(r"C:\ProgramData\Package Cache\{eeba99ce-6267-4545-834f-be6fa6de0e47}\Player-1.12.0-windows-x64.exe"));
        assert!(bad_place(r"C:\Windows\Installer\{D935BD04-09A0-4D85-A13F-63FBC9B6000F}\icon.exe"));
        assert!(bad_place(r"C:\Users\A\AppData\Local\Temp\setup\app.exe"));
        assert!(bad_place(r"C:\Users\A\Downloads\app.exe"));
        assert!(!bad_place(r"C:\Program Files\Player\Player.exe"));
        assert!(helper_exe(Path::new(r"C:\Program Files\Player\Uninstall.exe"), "Player"));
        assert!(helper_exe(Path::new(r"C:\Program Files\Player\QtWebEngineProcess.exe"), "Player"));
        assert!(!helper_exe(Path::new(r"C:\Program Files\Player\Player.exe"), "Player"));
        // The app's own name decides: this uninstaller is the app.
        assert!(!helper_exe(Path::new(r"C:\Program Files\DDU\Display Driver Uninstaller.exe"), "Display Driver Uninstaller"));
    }

    #[test]
    fn the_main_program_is_picked_over_helpers() {
        let d = tmp("main");
        for f in ["MediaPlayer.exe", "QtWebEngineProcess.exe", "vc_redist.x64.exe", "unins000.exe"] {
            std::fs::write(d.join(f), b"MZ").unwrap();
        }
        assert_eq!(main_exe_in(&d, "Media Player").unwrap().file_name().unwrap(), "MediaPlayer.exe");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn names_fit_without_versions() {
        assert_eq!(name_fit("Jellyfin Media Player", "Jellyfin Media Player"), 2);
        assert_eq!(name_fit("Jellyfin Media Player", "JellyfinMediaPlayer"), 2);
        assert_eq!(name_fit("Zipper 26.04 (x64)", "Zipper File Manager"), 1);
        assert_eq!(name_fit("Media Player", "Paint"), 0);
    }
}
