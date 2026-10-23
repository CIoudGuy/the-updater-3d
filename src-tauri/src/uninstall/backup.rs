//! Registry keys, startup values, services and scheduled tasks are saved before they are deleted:
//! keys as .reg files (what Windows' Registry Editor imports), tasks as their XML. Kept 30 days in
//! the app's data folder; "Restore" in Activity puts them back.

use super::{Hive, Target};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use winreg::enums::RegType;
use winreg::RegKey;

/// How long a backup is kept.
pub const KEEP_DAYS: i64 = 30;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "k")]
pub enum SavedItem {
    /// A .reg file; `hklm` when it writes to HKEY_LOCAL_MACHINE (restoring needs admin rights).
    Reg { file: String, hklm: bool, label: String },
    /// A scheduled task's XML.
    Task { path: String, file: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Saved {
    pub id: String,
    pub created: String,
    pub app: String,
    pub items: Vec<SavedItem>,
    #[serde(default)]
    pub restored: bool,
}

/// Where the backups are.
pub fn root() -> PathBuf {
    crate::paths::data_dir().join("uninstall-backups")
}

// ---------------------------------------------------------------- .reg files

fn escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(",")
}

fn type_num(t: &RegType) -> u32 {
    match t {
        RegType::REG_NONE => 0,
        RegType::REG_SZ => 1,
        RegType::REG_EXPAND_SZ => 2,
        RegType::REG_BINARY => 3,
        RegType::REG_DWORD => 4,
        RegType::REG_DWORD_BIG_ENDIAN => 5,
        RegType::REG_LINK => 6,
        RegType::REG_MULTI_SZ => 7,
        RegType::REG_RESOURCE_LIST => 8,
        RegType::REG_FULL_RESOURCE_DESCRIPTOR => 9,
        RegType::REG_RESOURCE_REQUIREMENTS_LIST => 10,
        RegType::REG_QWORD => 11,
    }
}

/// One value as a .reg line.
pub fn value_line(name: &str, vtype: &RegType, bytes: &[u8]) -> String {
    let n = if name.is_empty() { "@".to_string() } else { format!("\"{}\"", escape(name)) };
    let data = match vtype {
        RegType::REG_SZ if bytes.len() % 2 == 0 => {
            let w: Vec<u16> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let mut s = String::from_utf16_lossy(&w);
            while s.ends_with('\0') {
                s.pop();
            }
            if s.contains(['\r', '\n', '\0']) || String::from_utf16(&w).is_err() {
                format!("hex(1):{}", hex(bytes))
            } else {
                format!("\"{}\"", escape(&s))
            }
        }
        RegType::REG_DWORD if bytes.len() == 4 => format!("dword:{:08x}", u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
        RegType::REG_BINARY => format!("hex:{}", hex(bytes)),
        t => format!("hex({:x}):{}", type_num(t), hex(bytes)),
    };
    format!("{n}={data}")
}

fn write_key(k: &RegKey, full: &str, out: &mut String) -> Result<()> {
    out.push_str(&format!("[{full}]\r\n"));
    for v in k.enum_values() {
        let (name, val) = v?;
        out.push_str(&value_line(&name, &val.vtype, &val.bytes[..]));
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    for sub in k.enum_keys() {
        let sub = sub?;
        let sk = k.open_subkey_with_flags(&sub, winreg::enums::KEY_READ | winreg::enums::KEY_WOW64_64KEY)?;
        write_key(&sk, &format!("{full}\\{sub}"), out)?;
    }
    Ok(())
}

const HEADER: &str = "Windows Registry Editor Version 5.00\r\n\r\n";

/// A key and everything under it, as .reg text.
pub fn export_key(hive: Hive, path: &str) -> Result<String> {
    let k = super::open(hive, path).ok_or_else(|| anyhow!("{}\\{path} is not there", hive.short()))?;
    let mut out = HEADER.to_string();
    write_key(&k, &format!("{}\\{path}", hive.long()), &mut out)?;
    Ok(out)
}

/// One value, as .reg text.
pub fn export_value(hive: Hive, path: &str, name: &str) -> Result<String> {
    let k = super::open(hive, path).ok_or_else(|| anyhow!("{}\\{path} is not there", hive.short()))?;
    let v = k.get_raw_value(name)?;
    Ok(format!("{HEADER}[{}\\{path}]\r\n{}\r\n\r\n", hive.long(), value_line(name, &v.vtype, &v.bytes[..])))
}

/// Write .reg text as Registry Editor does (UTF-16 with a byte order mark).
pub fn write_reg(file: &Path, text: &str) -> Result<()> {
    let mut b = vec![0xFF, 0xFE];
    for u in text.encode_utf16() {
        b.extend_from_slice(&u.to_le_bytes());
    }
    std::fs::write(file, b).with_context(|| format!("write {}", file.display()))
}

// ---------------------------------------------------------------- backups

/// Save what these targets would delete (keys, values, services, tasks) in a new backup folder
/// under `root`. Folders and files are not saved. None when nothing needed saving.
pub fn save(root: &Path, app: &str, targets: &[Target]) -> Result<Option<Saved>> {
    let id = format!("{}-{}", chrono::Utc::now().format("%Y%m%d%H%M%S"), &uuid::Uuid::new_v4().simple().to_string()[..6]);
    let dir = root.join(&id);
    let mut items = Vec::new();
    let mut n = 0;
    let mut file = |ext: &str| {
        n += 1;
        dir.join(format!("{n:03}.{ext}"))
    };
    std::fs::create_dir_all(&dir)?;
    for t in targets {
        match t {
            Target::Key { hive, path } => {
                if !super::key_exists(*hive, path) {
                    continue;
                }
                let f = file("reg");
                write_reg(&f, &export_key(*hive, path)?)?;
                items.push(SavedItem::Reg { file: f.display().to_string(), hklm: *hive == Hive::Hklm, label: format!("{}\\{path}", hive.short()) });
            }
            Target::Value { hive, path, name } => {
                // Gone since the scan (the uninstaller removed it): nothing to save or delete.
                if super::open(*hive, path).is_none_or(|k| k.get_raw_value(name).is_err()) {
                    continue;
                }
                let f = file("reg");
                write_reg(&f, &export_value(*hive, path, name)?)?;
                items.push(SavedItem::Reg { file: f.display().to_string(), hklm: *hive == Hive::Hklm, label: format!("{}\\{path} ({name})", hive.short()) });
            }
            Target::Service { name } => {
                let path = format!("SYSTEM\\CurrentControlSet\\Services\\{name}");
                if super::key_exists(Hive::Hklm, &path) {
                    let f = file("reg");
                    write_reg(&f, &export_key(Hive::Hklm, &path)?)?;
                    items.push(SavedItem::Reg { file: f.display().to_string(), hklm: true, label: format!("Service {name}") });
                }
            }
            Target::Task { path } => {
                if let Some(xml) = task_xml_at(path) {
                    let f = file("xml");
                    std::fs::write(&f, xml)?;
                    items.push(SavedItem::Task { path: path.clone(), file: f.display().to_string() });
                }
            }
            Target::Dir { .. } | Target::File { .. } => {}
        }
    }
    if items.is_empty() {
        let _ = std::fs::remove_dir_all(&dir);
        return Ok(None);
    }
    let saved = Saved { id, created: chrono::Utc::now().to_rfc3339(), app: app.to_string(), items, restored: false };
    write_manifest(root, &saved)?;
    Ok(Some(saved))
}

pub fn write_manifest(root: &Path, s: &Saved) -> Result<()> {
    std::fs::write(root.join(&s.id).join("backup.json"), serde_json::to_vec_pretty(s)?)?;
    Ok(())
}

pub fn load(root: &Path, id: &str) -> Option<Saved> {
    if id.contains(['\\', '/', '.']) {
        return None;
    }
    serde_json::from_slice(&std::fs::read(root.join(id).join("backup.json")).ok()?).ok()
}

/// The backups that can still be restored, newest first.
pub fn list(root: &Path) -> Vec<Saved> {
    let mut v: Vec<Saved> = std::fs::read_dir(root).map(|r| r.flatten().filter_map(|e| load(root, &e.file_name().to_string_lossy())).collect()).unwrap_or_default();
    v.retain(|s| !s.restored && !expired(s, chrono::Utc::now()));
    v.sort_by(|a, b| b.created.cmp(&a.created));
    v
}

fn expired(s: &Saved, now: chrono::DateTime<chrono::Utc>) -> bool {
    chrono::DateTime::parse_from_rfc3339(&s.created).map(|t| now.signed_duration_since(t) > chrono::Duration::days(KEEP_DAYS)).unwrap_or(true)
}

/// Delete backups older than `KEEP_DAYS` (and folders that are not backups of ours).
pub fn prune(root: &Path, now: chrono::DateTime<chrono::Utc>) {
    let Ok(rd) = std::fs::read_dir(root) else { return };
    for e in rd.flatten() {
        let id = e.file_name().to_string_lossy().into_owned();
        let old = match load(root, &id) {
            Some(s) => expired(&s, now),
            None => e.metadata().and_then(|m| m.modified()).map(|t| now.signed_duration_since(chrono::DateTime::<chrono::Utc>::from(t)) > chrono::Duration::days(KEEP_DAYS)).unwrap_or(false),
        };
        if old {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
}

fn system32(exe: &str) -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32").join(exe)
}

/// The steps that put a backup back.
pub fn restore_steps(s: &Saved) -> Vec<crate::sources::Step> {
    s.items
        .iter()
        .map(|i| match i {
            SavedItem::Reg { file, hklm, label } => crate::sources::Step::Run { exe: system32("reg.exe"), args: vec!["import".into(), file.clone(), "/reg:64".into()], cwd: None, ok_codes: vec![0], elevated: *hklm, label: format!("Restoring {label}"), log: None },
            SavedItem::Task { path, file } => crate::sources::Step::Run { exe: system32("schtasks.exe"), args: vec!["/create".into(), "/tn".into(), path.clone(), "/xml".into(), file.clone(), "/f".into()], cwd: None, ok_codes: vec![0], elevated: true, label: format!("Restoring the task {path}"), log: None },
        })
        .collect()
}

/// A scheduled task's XML, by its full path.
fn task_xml_at(path: &str) -> Option<String> {
    let (folder, name) = path.rsplit_once('\\').unwrap_or(("", path));
    let folder = if folder.is_empty() { "\\" } else { folder };
    use windows::core::BSTR;
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    use windows::Win32::System::TaskScheduler::{ITaskService, TaskScheduler};
    use windows::Win32::System::Variant::VARIANT;
    unsafe {
        // Joins the multithreaded apartment and stays (see `locate::shortcut_target`).
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        (|| -> windows::core::Result<String> {
            let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
            let none = VARIANT::default();
            svc.Connect(&none, &none, &none, &none)?;
            Ok(svc.GetFolder(&BSTR::from(folder))?.GetTask(&BSTR::from(name))?.Xml()?.to_string())
        })()
        .ok()
    }
}
