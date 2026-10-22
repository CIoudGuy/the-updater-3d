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
    pub estimated_size_kb: u64,
    pub windows_installer: bool,
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
                estimated_size_kb: size as u64,
                windows_installer: msi == 1,
            });
        }
    }
    out
}

/// "20261014" -> "2026-10-14T00:00:00Z"
pub fn install_date_iso(d: &str) -> Option<String> {
    if d.len() == 8 && d.chars().all(|c| c.is_ascii_digit()) {
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
