//! Small Windows helpers: registry uninstall entries, file versions, disks, processes, shell.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::core::{HSTRING, PCWSTR};
use winreg::enums::*;
use winreg::RegKey;

pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A hidden console-less command.
pub fn cmd(exe: impl AsRef<std::ffi::OsStr>) -> Command {
    let mut c = Command::new(exe);
    c.creation_flags(CREATE_NO_WINDOW);
    c
}

/// NSIS installers read `/D=<dir>` (install folder) and `_?=<dir>` (uninstall in place) from the
/// raw command line: they must come last and must not be quoted, even when the path has spaces.
pub fn is_raw_arg(a: &str) -> bool {
    a.starts_with("/D=") || a.starts_with("_?=")
}

/// Add program arguments, passing the NSIS ones (`is_raw_arg`) through unquoted.
pub fn add_args(c: &mut Command, args: &[String]) {
    for a in args {
        if is_raw_arg(a) {
            c.raw_arg(a);
        } else {
            c.arg(a);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UninstallEntry {
    pub key: String,
    pub hive: &'static str,
    pub display_name: String,
    pub display_version: String,
    pub publisher: String,
    pub install_location: String,
    pub uninstall_string: String,
    pub quiet_uninstall_string: String,
    pub display_icon: String,
    pub install_date: String,
    pub url_update_info: String,
    /// The program's home page (`URLInfoAbout`) and help page (`HelpLink`)
    pub url_info_about: String,
    pub help_link: String,
    pub estimated_size_kb: u64,
    pub windows_installer: bool,
    /// Hidden from Add/Remove Programs (`SystemComponent` = 1): parts of other products.
    pub system_component: bool,
    /// Set for updates and hotfixes of another entry (`ParentKeyName`, `ReleaseType`).
    pub parent_key: String,
    pub release_type: String,
    /// When the entry's key was last written (an installer that rewrites it updates this).
    pub written: Option<std::time::SystemTime>,
    /// A WiX Burn bundle (`BundleProviderKey` or `BundleCachePath`): an installer that runs a
    /// chain of packages, registered with the cached installer as its program.
    pub bundle: bool,
    /// Windows Installer products that are part of this app (a bundle's packages), "{GUID}".
    pub msi_parts: Vec<String>,
}

/// Every Add/Remove Programs entry from HKLM (64 and 32 bit views) and HKCU.
pub fn uninstall_entries() -> Vec<UninstallEntry> {
    const PATH: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall";
    let mut out = Vec::new();
    let views: [(&'static str, RegKey, u32); 3] = [
        ("HKLM64", RegKey::predef(HKEY_LOCAL_MACHINE), KEY_READ | KEY_WOW64_64KEY),
        ("HKLM32", RegKey::predef(HKEY_LOCAL_MACHINE), KEY_READ | KEY_WOW64_32KEY),
        ("HKCU", RegKey::predef(HKEY_CURRENT_USER), KEY_READ),
    ];
    for (hive, root, flags) in views {
        let Ok(base) = root.open_subkey_with_flags(PATH, flags) else { continue };
        for name in base.enum_keys().flatten() {
            let Ok(k) = base.open_subkey_with_flags(&name, flags) else { continue };
            let s = |n: &str| k.get_value::<String, _>(n).unwrap_or_default();
            let size: u32 = k.get_value("EstimatedSize").unwrap_or(0);
            let msi: u32 = k.get_value("WindowsInstaller").unwrap_or(0);
            let system: u32 = k.get_value("SystemComponent").unwrap_or(0);
            out.push(UninstallEntry {
                key: name.clone(),
                hive,
                display_name: s("DisplayName"),
                display_version: s("DisplayVersion"),
                publisher: s("Publisher"),
                install_location: s("InstallLocation"),
                uninstall_string: s("UninstallString"),
                quiet_uninstall_string: s("QuietUninstallString"),
                display_icon: s("DisplayIcon"),
                install_date: s("InstallDate"),
                url_update_info: s("URLUpdateInfo"),
                url_info_about: s("URLInfoAbout"),
                help_link: s("HelpLink"),
                estimated_size_kb: size as u64,
                windows_installer: msi == 1,
                system_component: system == 1,
                parent_key: s("ParentKeyName"),
                release_type: s("ReleaseType"),
                written: k.query_info().ok().and_then(|m| filetime(m.last_write_time.dwLowDateTime, m.last_write_time.dwHighDateTime)),
                bundle: !s("BundleProviderKey").is_empty() || !s("BundleCachePath").is_empty(),
                msi_parts: Vec::new(),
            });
        }
    }
    out
}

/// A Windows FILETIME (100 ns steps since 1601) as a system time.
fn filetime(low: u32, high: u32) -> Option<std::time::SystemTime> {
    let t = ((high as u64) << 32) | low as u64;
    let unix = t.checked_sub(116_444_736_000_000_000)?;
    Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(unix.saturating_mul(100)))
}

/// Upgrade code of each installed Windows Installer product, keyed by product code (both
/// "{GUID}" in upper case). Windows keeps them packed under Installer\UpgradeCodes.
pub fn msi_upgrade_codes() -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    let roots: [(RegKey, &str); 2] = [
        (RegKey::predef(HKEY_LOCAL_MACHINE), "SOFTWARE\\Classes\\Installer\\UpgradeCodes"),
        (RegKey::predef(HKEY_CURRENT_USER), "Software\\Microsoft\\Installer\\UpgradeCodes"),
    ];
    for (root, path) in roots {
        let Ok(base) = root.open_subkey_with_flags(path, KEY_READ | KEY_WOW64_64KEY) else { continue };
        for packed_up in base.enum_keys().flatten() {
            let Some(up) = unpack_guid(&packed_up) else { continue };
            let Ok(k) = base.open_subkey_with_flags(&packed_up, KEY_READ | KEY_WOW64_64KEY) else { continue };
            for (packed_prod, _) in k.enum_values().flatten() {
                if let Some(prod) = unpack_guid(&packed_prod) {
                    out.insert(prod, up.clone());
                }
            }
        }
    }
    out
}

/// Windows Installer's packed form of a GUID ("4B9F...", 32 digits) back to "{GUID}". The
/// first three groups are stored reversed, every byte after them with its two digits swapped.
pub fn unpack_guid(p: &str) -> Option<String> {
    let c: Vec<char> = p.trim().chars().collect();
    if c.len() != 32 || !c.iter().all(|x| x.is_ascii_hexdigit()) {
        return None;
    }
    let rev = |a: usize, b: usize| c[a..b].iter().rev().collect::<String>();
    let swapped = |a: usize, b: usize| c[a..b].chunks(2).map(|x| format!("{}{}", x[1], x[0])).collect::<String>();
    Some(format!("{{{}-{}-{}-{}-{}}}", rev(0, 8), rev(8, 12), rev(12, 16), swapped(16, 20), swapped(20, 32)).to_ascii_uppercase())
}

/// "20261014" -> "2026-10-14T00:00:00Z"
pub fn install_date_iso(d: &str) -> Option<String> {
    // Some installers write the day before the month ("20262408"): not a date.
    if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) && chrono::NaiveDate::parse_from_str(d, "%Y%m%d").is_ok() {
        Some(format!("{}-{}-{}T00:00:00Z", &d[0..4], &d[4..6], &d[6..8]))
    } else {
        None
    }
}

/// Fixed file version of an exe or dll: "12.1.5.5783".
pub fn file_version(path: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::*;
    unsafe {
        let wide = HSTRING::from(path.as_os_str());
        let size = GetFileVersionInfoSizeW(PCWSTR(wide.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(wide.as_ptr()), Some(0), size, buf.as_mut_ptr() as *mut _).ok()?;
        let mut ptr: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut len: u32 = 0;
        let root = HSTRING::from("\\");
        if !VerQueryValueW(buf.as_ptr() as *const _, PCWSTR(root.as_ptr()), &mut ptr, &mut len).as_bool() || ptr.is_null() {
            return None;
        }
        let info = &*(ptr as *const VS_FIXEDFILEINFO);
        Some(format!(
            "{}.{}.{}.{}",
            info.dwFileVersionMS >> 16,
            info.dwFileVersionMS & 0xffff,
            info.dwFileVersionLS >> 16,
            info.dwFileVersionLS & 0xffff
        ))
    }
}

/// (free bytes available to the user, total bytes) of the volume holding `path`.
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let mut p = path.to_path_buf();
    while !p.exists() {
        p = p.parent()?.to_path_buf();
    }
    let wide = HSTRING::from(p.as_os_str());
    let mut free = 0u64;
    let mut total = 0u64;
    unsafe {
        GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut free), Some(&mut total), None).ok()?;
    }
    Some((free, total))
}

/// Root of the volume ("C:\") for display and grouping.
pub fn volume_root(path: &Path) -> String {
    let s = path.to_string_lossy();
    if s.len() >= 2 && s.as_bytes()[1] == b':' {
        format!("{}\\", &s[..2].to_uppercase())
    } else {
        s.into_owned()
    }
}

pub fn dir_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    total
}

pub fn is_elevated() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin().as_bool() }
}

/// Open a file, folder or URL with its default handler.
pub fn shell_open(target: &str) -> anyhow::Result<()> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let t = HSTRING::from(target);
    let verb = HSTRING::from("open");
    let r = unsafe { ShellExecuteW(None, PCWSTR(verb.as_ptr()), PCWSTR(t.as_ptr()), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL) };
    if r.0 as isize <= 32 {
        anyhow::bail!("Windows could not open {target}");
    }
    Ok(())
}

/// Start a program with arguments, detached, in its own folder.
pub fn launch(exe: &Path, args: &[&str]) -> anyhow::Result<()> {
    let mut c = Command::new(exe);
    c.args(args);
    if let Some(dir) = exe.parent() {
        c.current_dir(dir);
    }
    // DETACHED_PROCESS so the child does not inherit our (absent) console.
    c.creation_flags(0x0000_0008);
    c.spawn()?;
    Ok(())
}

pub fn show_in_folder(path: &Path) -> anyhow::Result<()> {
    // Explorer parses its own command line: the switch must stay outside the quotes.
    let p = path.display().to_string().replace('/', "\\");
    if path.is_dir() {
        Command::new("explorer.exe").raw_arg(format!("\"{p}\"")).spawn()?;
    } else {
        Command::new("explorer.exe").raw_arg(format!("/select,\"{p}\"")).spawn()?;
    }
    Ok(())
}

/// Start `exe` elevated (UAC prompt) and return its process handle as a raw value.
/// Errors with "declined" when the user says no to the prompt.
pub fn shell_execute_elevated(exe: &Path, params: &str, show: bool) -> anyhow::Result<isize> {
    use windows::Win32::UI::Shell::*;
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SW_SHOWNORMAL};
    let file = HSTRING::from(exe.as_os_str());
    let p = HSTRING::from(params);
    let verb = HSTRING::from("runas");
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: PCWSTR(verb.as_ptr()),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(p.as_ptr()),
        nShow: if show { SW_SHOWNORMAL.0 } else { SW_HIDE.0 },
        ..Default::default()
    };
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            // ERROR_CANCELLED
            if e.code().0 as u32 == 0x8007_04C7 {
                anyhow::bail!("declined");
            }
            anyhow::bail!("Could not start the admin helper: {e}");
        }
    }
    Ok(info.hProcess.0 as isize)
}

/// Wait for a process handle from `shell_execute_elevated` and return its exit code.
pub fn wait_process(handle: isize) -> u32 {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    unsafe {
        let h = HANDLE(handle as *mut _);
        WaitForSingleObject(h, INFINITE);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(h, &mut code);
        let _ = CloseHandle(h);
        code
    }
}

/// Save the exe's 256 px (or largest available) icon as PNG. Returns false when it has none.
pub fn extract_icon_png(exe: &Path, out: &Path) -> bool {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::Shell::SHDefExtractIconW;
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, HICON, ICONINFO};
    unsafe {
        let wide = HSTRING::from(exe.as_os_str());
        let mut large = HICON::default();
        // nIconSize: low word = large icon size.
        if SHDefExtractIconW(PCWSTR(wide.as_ptr()), 0, 0, Some(&mut large), None, 256).is_err() || large.is_invalid() {
            return false;
        }
        let mut ii = ICONINFO::default();
        if GetIconInfo(large, &mut ii).is_err() {
            let _ = DestroyIcon(large);
            return false;
        }
        let mut bmp = BITMAP::default();
        GetObjectW(ii.hbmColor.into(), std::mem::size_of::<BITMAP>() as i32, Some(&mut bmp as *mut _ as *mut _));
        let (w, h) = (bmp.bmWidth, bmp.bmHeight);
        let mut ok = false;
        if w > 0 && h > 0 {
            let mut bi = BITMAPINFO::default();
            bi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
            bi.bmiHeader.biWidth = w;
            bi.bmiHeader.biHeight = -h; // top-down
            bi.bmiHeader.biPlanes = 1;
            bi.bmiHeader.biBitCount = 32;
            bi.bmiHeader.biCompression = BI_RGB.0;
            let mut px = vec![0u8; (w * h * 4) as usize];
            let dc = GetDC(None);
            let lines = GetDIBits(dc, ii.hbmColor, 0, h as u32, Some(px.as_mut_ptr() as *mut _), &mut bi, DIB_RGB_COLORS);
            ReleaseDC(None, dc);
            if lines > 0 {
                let has_alpha = px.chunks(4).any(|p| p[3] != 0);
                for p in px.chunks_mut(4) {
                    p.swap(0, 2); // BGRA -> RGBA
                    if !has_alpha {
                        p[3] = 255;
                    }
                }
                ok = write_png(out, w as u32, h as u32, &px).is_ok();
            }
        }
        if !ii.hbmColor.is_invalid() {
            let _ = DeleteObject(ii.hbmColor.into());
        }
        if !ii.hbmMask.is_invalid() {
            let _ = DeleteObject(ii.hbmMask.into());
        }
        let _ = DestroyIcon(large);
        ok
    }
}

fn write_png(out: &Path, w: u32, h: u32, rgba: &[u8]) -> anyhow::Result<()> {
    let f = std::fs::File::create(out)?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    let mut wr = enc.write_header()?;
    wr.write_image_data(rgba)?;
    Ok(())
}

/// State of a Windows service from `sc query`: Some(true) running, Some(false) stopped, None missing.
pub fn service_running(name: &str) -> Option<bool> {
    let out = cmd("sc.exe").args(["query", name]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with("STATE") {
            // "STATE              : 4  RUNNING"
            let num = l.split(':').nth(1)?.trim().split_whitespace().next()?;
            return Some(num == "4" || num == "2");
        }
    }
    None
}

/// Split a Windows command line into the program and its arguments (CommandLineToArgvW rules).
/// Split an uninstall command from the registry. Vendors sometimes leave the program path
/// unquoted even with spaces in it (Unity: `C:\Program Files\Unity\...\Uninstall.exe`); Windows
/// still runs those, reading the program up to its `.exe`, so do the same.
pub fn split_program_line(s: &str) -> Vec<String> {
    let t = s.trim();
    if t.starts_with('"') {
        return split_command_line(t);
    }
    let low = t.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = low[from..].find(".exe") {
        let end = from + i + 4;
        if end == t.len() || matches!(t.as_bytes()[end], b' ' | b'\t' | b',') {
            let mut v = vec![t[..end].to_string()];
            v.extend(split_command_line(&t[end..]));
            return v;
        }
        from = end;
    }
    split_command_line(t)
}

pub fn split_command_line(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    let mut any = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\\' => {
                let mut n = 0;
                while i < chars.len() && chars[i] == '\\' {
                    n += 1;
                    i += 1;
                }
                if i < chars.len() && chars[i] == '"' {
                    cur.extend(std::iter::repeat_n('\\', n / 2));
                    if n % 2 == 1 {
                        cur.push('"');
                        i += 1;
                    }
                } else {
                    cur.extend(std::iter::repeat_n('\\', n));
                }
                any = true;
                continue;
            }
            '"' => {
                in_q = !in_q;
                any = true;
            }
            ' ' | '\t' if !in_q => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            _ => {
                cur.push(c);
                any = true;
            }
        }
        i += 1;
    }
    if any {
        out.push(cur);
    }
    out
}

/// Quote one argument for a Windows command line.
pub fn quote_arg(a: &str) -> String {
    if !a.is_empty() && !a.contains([' ', '\t', '"']) {
        return a.to_string();
    }
    let mut s = String::from("\"");
    let mut bs = 0;
    for c in a.chars() {
        match c {
            '\\' => bs += 1,
            '"' => {
                s.extend(std::iter::repeat_n('\\', bs * 2 + 1));
                s.push('"');
                bs = 0;
            }
            _ => {
                s.extend(std::iter::repeat_n('\\', bs));
                bs = 0;
                s.push(c);
            }
        }
    }
    s.extend(std::iter::repeat_n('\\', bs * 2));
    s.push('"');
    s
}

pub fn join_args(args: &[String]) -> String {
    args.iter().map(|a| quote_arg(a)).collect::<Vec<_>>().join(" ")
}

pub fn exists(p: &str) -> bool {
    !p.is_empty() && Path::new(p).exists()
}

pub fn pathbuf(s: &str) -> PathBuf {
    PathBuf::from(s.trim().trim_matches('"').trim_end_matches(['\\', '/']))
}

// ---------------------------------------------------------------- no helper programs
// Antivirus heuristics distrust an unsigned app that quietly starts schtasks, whoami or
// taskkill, so these talk to Windows directly instead.

/// "4.5.14" from the exe's version resource (the numeric file version, not the display text).
pub fn exe_version(exe: &Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW, VS_FIXEDFILEINFO};
    let name = HSTRING::from(exe.as_os_str());
    unsafe {
        let len = GetFileVersionInfoSizeW(&name, None);
        if len == 0 {
            return None;
        }
        let mut buf = vec![0u8; len as usize];
        GetFileVersionInfoW(&name, None, len, buf.as_mut_ptr().cast()).ok()?;
        let mut info: *mut std::ffi::c_void = std::ptr::null_mut();
        let mut size = 0u32;
        if !VerQueryValueW(buf.as_ptr().cast(), &HSTRING::from("\\"), &mut info, &mut size).as_bool() || info.is_null() || (size as usize) < std::mem::size_of::<VS_FIXEDFILEINFO>() {
            return None;
        }
        let f = &*(info as *const VS_FIXEDFILEINFO);
        if f.dwSignature != 0xFEEF04BD {
            return None;
        }
        let (a, b, c) = (f.dwFileVersionMS >> 16, f.dwFileVersionMS & 0xFFFF, f.dwFileVersionLS >> 16);
        if a == 0 && b == 0 {
            return None;
        }
        Some(format!("{a}.{b}.{c}"))
    }
}

/// The signed-in Windows account's SID, "S-1-5-21-...".
pub fn current_user_sid() -> Option<String> {
    use windows::Win32::Foundation::{CloseHandle, LocalFree, HANDLE, HLOCAL};
    use windows::Win32::Security::Authorization::ConvertSidToStringSidW;
    use windows::Win32::Security::{GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    unsafe {
        let mut token = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).ok()?;
        let mut len = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut len);
        let mut buf = vec![0u8; len.max(1) as usize];
        let got = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len);
        let _ = CloseHandle(token);
        got.ok()?;
        let user = &*(buf.as_ptr() as *const TOKEN_USER);
        let mut s = windows::core::PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut s).ok()?;
        let out = s.to_string().ok();
        let _ = LocalFree(Some(HLOCAL(s.0.cast())));
        out.filter(|v| v.starts_with("S-1-"))
    }
}

/// Run `f` with the Task Scheduler service, on this thread (COM set up and torn down here).
fn with_task_service<T>(f: impl FnOnce(&windows::Win32::System::TaskScheduler::ITaskService) -> windows::core::Result<T>) -> windows::core::Result<T> {
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::TaskScheduler::{ITaskService, TaskScheduler};
    use windows::Win32::System::Variant::VARIANT;
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let r = (|| {
            let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let none = VARIANT::default();
            svc.Connect(&none, &none, &none, &none)?;
            f(&svc)
        })();
        if init.is_ok() {
            CoUninitialize();
        }
        r
    }
}

/// Is a scheduled task with this name registered in the root folder?
pub fn task_exists(name: &str) -> bool {
    with_task_service(|svc| unsafe { svc.GetFolder(&windows::core::BSTR::from("\\"))?.GetTask(&windows::core::BSTR::from(name)) }).is_ok()
}

/// The XML definition of a scheduled task in the root folder.
pub fn task_xml(name: &str) -> Option<String> {
    with_task_service(|svc| unsafe { svc.GetFolder(&windows::core::BSTR::from("\\"))?.GetTask(&windows::core::BSTR::from(name))?.Xml() }).ok().map(|b| b.to_string())
}

/// `task_xml`, telling a task that is not there (Ok(None)) from one that could not be read
/// (the Task Scheduler not answering yet right after sign-in, say).
pub fn task_xml_checked(name: &str) -> Result<Option<String>, String> {
    match with_task_service(|svc| unsafe { svc.GetFolder(&windows::core::BSTR::from("\\"))?.GetTask(&windows::core::BSTR::from(name))?.Xml() }) {
        Ok(b) => Ok(Some(b.to_string())),
        // ERROR_FILE_NOT_FOUND / ERROR_PATH_NOT_FOUND
        Err(e) if matches!(e.code().0 as u32, 0x8007_0002 | 0x8007_0003) => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Start a scheduled task now.
pub fn run_task(name: &str) -> bool {
    use windows::Win32::System::Variant::VARIANT;
    with_task_service(|svc| unsafe { svc.GetFolder(&windows::core::BSTR::from("\\"))?.GetTask(&windows::core::BSTR::from(name))?.Run(&VARIANT::default()) }).is_ok()
}

/// The signer's name ("Adobe Inc.") when Windows trusts the file's Authenticode signature (the
/// check behind "Verified publisher"), for downloads whose vendor publishes no checksum.
pub fn authenticode_signer(file: &Path) -> Option<String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Security::Cryptography::{CertGetNameStringW, CERT_NAME_SIMPLE_DISPLAY_TYPE};
    use windows::Win32::Security::WinTrust::*;
    let path = HSTRING::from(file.as_os_str());
    let mut info = WINTRUST_FILE_INFO { cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32, pcwszFilePath: PCWSTR(path.as_ptr()), ..Default::default() };
    let mut data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        dwStateAction: WTD_STATEACTION_VERIFY,
        ..Default::default()
    };
    data.Anonymous.pFile = &mut info;
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    unsafe {
        let trusted = WinVerifyTrust(HWND(-1isize as *mut _), &mut action, (&mut data as *mut WINTRUST_DATA).cast()) == 0;
        let mut name = None;
        if trusted {
            let prov = WTHelperProvDataFromStateData(data.hWVTStateData);
            if !prov.is_null() {
                let sgnr = WTHelperGetProvSignerFromChain(prov, 0, false, 0);
                if !sgnr.is_null() && (*sgnr).csCertChain > 0 && !(*sgnr).pasCertChain.is_null() {
                    let cert = (*(*sgnr).pasCertChain).pCert;
                    let mut buf = [0u16; 256];
                    let n = CertGetNameStringW(cert, CERT_NAME_SIMPLE_DISPLAY_TYPE, 0, None, Some(&mut buf));
                    if n > 1 {
                        name = Some(String::from_utf16_lossy(&buf[..n as usize - 1]));
                    }
                }
            }
        }
        data.dwStateAction = WTD_STATEACTION_CLOSE;
        let _ = WinVerifyTrust(HWND(-1isize as *mut _), &mut action, (&mut data as *mut WINTRUST_DATA).cast());
        name
    }
}

/// After an uninstall: is nothing left in `dir` but the uninstaller itself (uninstall.exe,
/// uninst.exe, unins000.exe and its .dat) and logs? A few files at most, so a folder that still
/// holds anything else, such as the user's own files, is never taken for a leftover.
pub fn only_uninstaller_left(dir: &Path) -> bool {
    fn walk(d: &Path, files: &mut Vec<String>, depth: u8) -> bool {
        let Ok(rd) = std::fs::read_dir(d) else { return false };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth >= 2 || !walk(&p, files, depth + 1) {
                    return false;
                }
            } else {
                files.push(e.file_name().to_string_lossy().to_ascii_lowercase());
                if files.len() > 8 {
                    return false;
                }
            }
        }
        true
    }
    let mut files = Vec::new();
    if !walk(dir, &mut files, 0) {
        return false;
    }
    files.iter().all(|f| {
        let uninstaller = (f.starts_with("unins") || f.starts_with("uninst")) && [".exe", ".dat", ".msg", ".ini"].iter().any(|x| f.ends_with(x));
        uninstaller || f.ends_with(".log")
    })
}

/// Ask a process to close, like clicking its X: WM_CLOSE to each of its top-level windows.
pub fn close_process_windows(pid: u32) {
    use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE};
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> windows::core::BOOL {
        let mut owner = 0u32;
        unsafe {
            GetWindowThreadProcessId(hwnd, Some(&mut owner));
            if owner == lp.0 as u32 {
                let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            }
        }
        true.into()
    }
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(pid as isize));
    }
}

/// Does the process have a visible top-level window (something a WM_CLOSE can close)?
pub fn has_windows(pid: u32) -> bool {
    use windows::Win32::Foundation::{HWND, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible};
    struct Ask {
        pid: u32,
        found: bool,
    }
    unsafe extern "system" fn each(hwnd: HWND, lp: LPARAM) -> windows::core::BOOL {
        let ask = unsafe { &mut *(lp.0 as *mut Ask) };
        let mut owner = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut owner)) };
        if owner == ask.pid && unsafe { IsWindowVisible(hwnd) }.as_bool() {
            ask.found = true;
            return false.into();
        }
        true.into()
    }
    let mut ask = Ask { pid, found: false };
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut ask as *mut Ask as isize));
    }
    ask.found
}

/// End a process at once.
pub fn kill_process(pid: u32) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    unsafe {
        if let Ok(h) = OpenProcess(PROCESS_TERMINATE, false, pid) {
            let _ = TerminateProcess(h, 1);
            let _ = CloseHandle(h);
        }
    }
}

/// Full path of a running process's exe.
pub fn process_image(pid: u32) -> Option<PathBuf> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = vec![0u16; 32768];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, windows::core::PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(&buf[..len as usize])))
    }
}

/// The process id on the server end of a named pipe this process is a client of.
pub fn pipe_server_pid(handle: isize) -> Option<u32> {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetNamedPipeServerProcessId(pipe: isize, pid: *mut u32) -> i32;
    }
    let mut pid = 0u32;
    (unsafe { GetNamedPipeServerProcessId(handle, &mut pid) } != 0).then_some(pid)
}

/// Same file path, ignoring case and the \\?\ prefix.
pub fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| {
        let p = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
        let s = p.to_string_lossy().to_ascii_lowercase();
        s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s)
    };
    norm(a) == norm(b)
}

/// "arm64" or "x64" for an IMAGE_FILE_MACHINE value.
pub fn arch_of_machine(machine: u16) -> &'static str {
    // IMAGE_FILE_MACHINE_ARM64
    if machine == 0xAA64 {
        "arm64"
    } else {
        "x64"
    }
}

/// The processor Windows itself runs on, "arm64" or "x64". An x64 build of this app running
/// emulated on ARM64 sees "AMD64" in its environment, so the kernel is asked instead.
pub fn native_arch() -> &'static str {
    use once_cell::sync::Lazy;
    static ARCH: Lazy<&'static str> = Lazy::new(|| {
        use windows::Win32::System::SystemInformation::IMAGE_FILE_MACHINE;
        use windows::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};
        let mut process = IMAGE_FILE_MACHINE(0);
        let mut native = IMAGE_FILE_MACHINE(0);
        if unsafe { IsWow64Process2(GetCurrentProcess(), &mut process, Some(&mut native)) }.is_ok() && native.0 != 0 {
            return arch_of_machine(native.0);
        }
        let a = std::env::var("PROCESSOR_ARCHITEW6432").or_else(|_| std::env::var("PROCESSOR_ARCHITECTURE")).unwrap_or_default();
        if a.eq_ignore_ascii_case("ARM64") { "arm64" } else { "x64" }
    });
    *ARCH
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths_x86() -> std::path::PathBuf {
        crate::paths::program_files_x86()
    }

    #[test]
    fn reads_versions_and_account_without_helper_programs() {
        // notepad.exe always carries a numeric version resource.
        let np = std::path::PathBuf::from(std::env::var("WINDIR").unwrap()).join("System32").join("notepad.exe");
        let v = exe_version(&np).expect("notepad version");
        assert!(v.split('.').count() == 3 && v.split('.').all(|p| p.parse::<u32>().is_ok()), "{v}");
        assert!(current_user_sid().unwrap().starts_with("S-1-5-"));
        assert!(!task_exists("The Updater 3D test task that does not exist"));
        // Edge carries an embedded Microsoft signature (notepad's is in a Windows catalog, not the
        // file); an unsigned file has none.
        let edge = paths_x86().join(r"Microsoft\Edge\Application\msedge.exe");
        if edge.exists() {
            assert_eq!(authenticode_signer(&edge).as_deref().map(|n| n.starts_with("Microsoft")), Some(true));
        }
        let plain = std::env::temp_dir().join(format!("u3d-unsigned-{}.exe", std::process::id()));
        std::fs::write(&plain, b"MZ not signed").unwrap();
        assert_eq!(authenticode_signer(&plain), None);
        let _ = std::fs::remove_file(&plain);
    }

    #[test]
    fn splits_adobe_uninstall_string() {
        let s = r#""C:\Program Files (x86)\Common Files\Adobe\Adobe Desktop Common\HDBox\Uninstaller.exe" --uninstall=1 --sapCode=SBSTP --productVersion=12.1.5 --productName="Substance 3D Painter" --mode=2"#;
        let v = split_command_line(s);
        assert_eq!(v[0], r"C:\Program Files (x86)\Common Files\Adobe\Adobe Desktop Common\HDBox\Uninstaller.exe");
        assert!(v.contains(&"--productName=Substance 3D Painter".to_string()));
        assert_eq!(v.last().unwrap(), "--mode=2");
    }

    #[test]
    fn quotes_round_trip() {
        let args = vec!["--a=1".to_string(), "C:\\Program Files\\x y\\".to_string(), "say \"hi\"".to_string()];
        assert_eq!(split_command_line(&join_args(&args)), args);
    }

    #[test]
    fn only_a_folder_holding_just_its_uninstaller_counts_as_left_over() {
        let d = std::env::temp_dir().join(format!("u3d-leftover-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("logs")).unwrap();
        std::fs::write(d.join("Uninstall.exe"), b"x").unwrap();
        std::fs::write(d.join("logs").join("install.log"), b"x").unwrap();
        assert!(only_uninstaller_left(&d));
        std::fs::write(d.join("unins000.dat"), b"x").unwrap();
        assert!(only_uninstaller_left(&d));
        // Anything else, the user's own files included, keeps the folder.
        std::fs::write(d.join("my project.blend"), b"x").unwrap();
        assert!(!only_uninstaller_left(&d));
        let _ = std::fs::remove_dir_all(&d);
        assert!(!only_uninstaller_left(&d), "a folder that is gone is not removed again");
    }

    #[test]
    fn unquoted_program_paths_with_spaces_stay_whole() {
        // Unity's uninstall entry, exactly as it is in the registry.
        let v = split_program_line(r"C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Uninstall.exe");
        assert_eq!(v, vec![r"C:\Program Files\Unity\Hub\Editor\6000.3.27f1\Editor\Uninstall.exe".to_string()]);
        let v = split_program_line(r"C:\Program Files\OBS Studio\uninstall.exe /S");
        assert_eq!(v, vec![r"C:\Program Files\OBS Studio\uninstall.exe".to_string(), "/S".to_string()]);
        // Icon paths: "file.exe,0".
        assert_eq!(split_program_line(r"C:\Program Files\X Y\x.exe,0")[0], r"C:\Program Files\X Y\x.exe");
        // Quoted lines are unchanged.
        assert_eq!(split_program_line(r#""C:\Program Files\A\u.exe" --mode=3"#), vec![r"C:\Program Files\A\u.exe".to_string(), "--mode=3".to_string()]);
        assert_eq!(split_program_line("MsiExec.exe /X{ABC}"), vec!["MsiExec.exe".to_string(), "/X{ABC}".to_string()]);
    }

    #[test]
    fn native_arch_comes_from_the_kernel() {
        assert_eq!(super::arch_of_machine(0xAA64), "arm64");
        assert_eq!(super::arch_of_machine(0x8664), "x64");
        let env = std::env::var("PROCESSOR_ARCHITEW6432").or_else(|_| std::env::var("PROCESSOR_ARCHITECTURE")).unwrap_or_default();
        assert_eq!(super::native_arch(), if env.eq_ignore_ascii_case("ARM64") { "arm64" } else { "x64" });
    }
}

// ---------------------------------------------------------------- installer process trees

/// A Windows job object holding an installer and every process it starts, so the app can wait
/// for an installer that hands the work to a child (a bootstrapper, a self-elevated copy, the
/// real setup unpacked to a temp folder) and exits first. Breaking away is not allowed: Windows'
/// shell asks for it whenever it may when a program is started with ShellExecute (as NSIS and
/// most installers do), and falls back to staying in the job when it may not. Closing the job
/// leaves its processes running.
pub struct InstallerJob(windows::Win32::Foundation::HANDLE);

// The handle is only used through the job functions, which may be called from any thread.
unsafe impl Send for InstallerJob {}
unsafe impl Sync for InstallerJob {}

impl InstallerJob {
    pub fn new() -> Option<InstallerJob> {
        use windows::Win32::System::JobObjects::CreateJobObjectW;
        unsafe {
            let h = CreateJobObjectW(None, PCWSTR::null()).ok()?;
            let job = InstallerJob(h);
            Some(job)
        }
    }

    /// Put a process (started suspended) into the job.
    pub fn add(&self, child: &std::process::Child) -> bool {
        use std::os::windows::io::AsRawHandle;
        use windows::Win32::System::JobObjects::AssignProcessToJobObject;
        unsafe { AssignProcessToJobObject(self.0, windows::Win32::Foundation::HANDLE(child.as_raw_handle())).is_ok() }
    }

    /// Ids of the processes in the job that are still running.
    pub fn pids(&self) -> Vec<u32> {
        use windows::Win32::System::JobObjects::*;
        // Two u32 counts (assigned, listed), then the ids as ULONG_PTR.
        let mut buf = vec![0usize; 1 + 1024];
        unsafe {
            if QueryInformationJobObject(Some(self.0), JobObjectBasicProcessIdList, buf.as_mut_ptr() as *mut _, (buf.len() * std::mem::size_of::<usize>()) as u32, None).is_err() {
                return vec![];
            }
        }
        let n = ((buf[0] as u64 >> 32) as usize).min(buf.len() - 1);
        buf[1..1 + n].iter().map(|p| *p as u32).collect()
    }
}

impl Drop for InstallerJob {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

/// Start the threads of a process created suspended.
pub fn resume_process(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Diagnostics::ToolHelp::*;
    use windows::Win32::System::Threading::{OpenThread, ResumeThread, THREAD_SUSPEND_RESUME};
    let mut resumed = false;
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0) else { return false };
        let mut te = THREADENTRY32 { dwSize: std::mem::size_of::<THREADENTRY32>() as u32, ..Default::default() };
        let mut more = Thread32First(snap, &mut te).is_ok();
        while more {
            if te.th32OwnerProcessID == pid {
                if let Ok(t) = OpenThread(THREAD_SUSPEND_RESUME, false, te.th32ThreadID) {
                    resumed |= ResumeThread(t) != u32::MAX;
                    let _ = CloseHandle(t);
                }
            }
            more = Thread32Next(snap, &mut te).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    resumed
}

/// Give memory the app no longer uses back to Windows: compact the heap and empty the working
/// set (pages come back on their own when needed). For when the window has closed.
pub fn give_back_memory() {
    use windows::Win32::System::Memory::{GetProcessHeap, HeapCompact, HEAP_FLAGS};
    use windows::Win32::System::ProcessStatus::K32EmptyWorkingSet;
    use windows::Win32::System::Threading::GetCurrentProcess;
    unsafe {
        if let Ok(h) = GetProcessHeap() {
            let _ = HeapCompact(h, HEAP_FLAGS(0));
        }
        let _ = K32EmptyWorkingSet(GetCurrentProcess());
    }
}
