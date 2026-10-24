//! Epic Games: installing, updating and repairing a game without the Epic Games Launcher. A
//! build is described by its manifest (Epic's binary file list, or the older JSON form): every
//! file of the game, the SHA1 it must have and the parts of chunks it is made of. Chunks are
//! pieces of about 1 MiB on Epic's CDN; each is checked against its own hash as it arrives, and
//! every file written is checked against the manifest's SHA1 before it takes the old one's place.
//!
//! Signing in is done elsewhere: the access token is a parameter. It is only ever sent to Epic's
//! launcher service, never to a CDN, and it is never logged or kept.

use super::epic::{build_meta, version_of_build};
use super::sync::{self, check_cancel, read_state, safe_rel, write_file, write_state, Mode, Progress, Report, State, Undo, OWN_DIR};
use anyhow::{anyhow, bail, Context, Result};
use futures_util::future::BoxFuture;
use futures_util::stream::{BoxStream, StreamExt};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha1::{Digest, Sha1};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Which Epic game: the ids Epic's catalog knows it by.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpicRef {
    pub namespace: String,
    pub catalog_item: String,
    pub app_name: String,
}

/// A chunk's id: four numbers, written as 32 hex digits.
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Guid(pub [u32; 4]);

impl Guid {
    pub fn hex(&self) -> String {
        self.0.iter().map(|n| format!("{n:08X}")).collect()
    }

    /// From its 32 hex digits (the JSON manifest's form).
    pub fn from_hex(s: &str) -> Option<Guid> {
        let s = s.trim();
        if s.len() != 32 || !s.bytes().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let mut g = [0u32; 4];
        for (i, n) in g.iter_mut().enumerate() {
            *n = u32::from_str_radix(&s[i * 8..i * 8 + 8], 16).ok()?;
        }
        Some(Guid(g))
    }

    fn le_bytes(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        for (i, n) in self.0.iter().enumerate() {
            b[i * 4..i * 4 + 4].copy_from_slice(&n.to_le_bytes());
        }
        b
    }

    pub fn is_zero(&self) -> bool {
        self.0 == [0; 4]
    }
}

impl std::fmt::Debug for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

impl std::fmt::Display for Guid {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex())
    }
}

/// The build itself: its app, version and how it starts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// Decides the chunk folders and some of the format
    pub feature_level: u32,
    /// Very old builds were whole files instead of chunks
    pub is_file_data: bool,
    pub app_id: u32,
    pub app_name: String,
    pub build_version: String,
    pub launch_exe: String,
    pub launch_command: String,
    pub prereq_ids: Vec<String>,
    pub prereq_name: String,
    pub prereq_path: String,
    pub prereq_args: String,
    pub build_id: String,
}

/// One chunk of the build on the CDN.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkInfo {
    pub guid: Guid,
    /// Epic's rolling hash of the chunk's data (part of its address)
    pub hash: u64,
    pub sha1: [u8; 20],
    /// The folder number in its address
    pub group: u8,
    /// Its data unpacked
    pub window_size: u32,
    /// The chunk file as downloaded
    pub file_size: u64,
    /// Feature level 22 on: not zero for an encrypted chunk
    pub secret: Guid,
}

/// A piece of a chunk that is part of a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ChunkPart {
    pub guid: Guid,
    pub offset: u32,
    pub size: u32,
}

/// One file of the build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileEntry {
    /// Relative to the game folder, with "/"
    pub name: String,
    pub symlink: String,
    pub sha1: [u8; 20],
    /// 1 read-only, 2 compressed, 4 executable
    pub flags: u8,
    pub install_tags: Vec<String>,
    /// The file is these pieces one after another
    pub parts: Vec<ChunkPart>,
}

impl FileEntry {
    pub fn size(&self) -> u64 {
        self.parts.iter().map(|p| p.size as u64).sum()
    }
}

/// A build manifest.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub meta: Meta,
    pub chunks: Vec<ChunkInfo>,
    pub files: Vec<FileEntry>,
    pub custom: BTreeMap<String, String>,
}

/// The newest feature level whose layout is known. Newer manifests are refused.
const MAX_FEATURE_LEVEL: u32 = 24;
const MANIFEST_MAGIC: u32 = 0x44BE_C00C;
const CHUNK_MAGIC: u32 = 0xB1FE_3AA2;

impl Manifest {
    /// A manifest file as Epic serves it: binary, or JSON (older builds).
    pub fn parse(bytes: &[u8]) -> Result<Manifest> {
        // Not is_ascii_whitespace: that takes in 0x0C, the binary form's first byte.
        let start = bytes.iter().position(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n')).unwrap_or(0);
        let b = &bytes[start..];
        if b.starts_with(b"{") || b.starts_with("\u{feff}".as_bytes()) {
            parse_json(b)
        } else {
            parse_binary(bytes)
        }
    }

    pub fn chunk_index(&self) -> HashMap<Guid, &ChunkInfo> {
        self.chunks.iter().map(|c| (c.guid, c)).collect()
    }

    pub fn custom_field(&self, key: &str) -> Option<&str> {
        self.custom.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.as_str())
    }

    /// Where a chunk is, relative to a CDN base.
    pub fn chunk_path(&self, c: &ChunkInfo) -> String {
        chunk_path(self.meta.feature_level, c)
    }

    /// CDN bases the build names itself (custom fields "BaseUrl" and "CloudDir"), tried after
    /// the ones Epic's service gives.
    pub fn own_bases(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for key in ["BaseUrl", "CloudDir"] {
            for u in self.custom_field(key).unwrap_or("").split(',') {
                let u = u.trim().trim_end_matches('/');
                if (u.starts_with("https://") || u.starts_with("http://")) && !out.iter().any(|o| o == u) {
                    out.push(u.to_string());
                }
            }
        }
        out
    }

    /// The size of the installed game.
    pub fn install_size(&self) -> u64 {
        self.files.iter().map(FileEntry::size).sum()
    }

    /// Every file's path in the game folder. The whole build is refused when one would leave the
    /// folder, a file is a link or is named twice, or a part names a chunk the build does not
    /// list, an encrypted chunk or bytes past a chunk's end.
    pub fn checked_paths(&self) -> Result<Vec<PathBuf>> {
        if self.meta.is_file_data {
            bail!("This build is kept as whole files instead of chunks, which the app does not download");
        }
        let index = self.chunk_index();
        let mut seen = HashSet::new();
        self.files
            .iter()
            .map(|f| {
                let rel = safe_rel(&f.name).ok_or_else(|| anyhow!("The build names a file outside the game folder ({}), so it is refused", f.name))?;
                if !f.symlink.is_empty() {
                    bail!("The build has a link ({}), which the app does not make", f.name);
                }
                if !seen.insert(key(&f.name)) {
                    bail!("The build names {} twice", f.name);
                }
                for p in &f.parts {
                    let c = index.get(&p.guid).ok_or_else(|| anyhow!("{} is made of a chunk the build does not list", f.name))?;
                    if !c.secret.is_zero() {
                        bail!("The files of this game are encrypted; the app cannot install it");
                    }
                    if c.window_size > 0 && p.offset as u64 + p.size as u64 > c.window_size as u64 {
                        bail!("{} names bytes past the end of a chunk", f.name);
                    }
                }
                Ok(rel)
            })
            .collect()
    }
}

/// A file's name for comparing builds: Windows names are the same in any case.
fn key(name: &str) -> String {
    name.replace('\\', "/").to_lowercase()
}

fn check_level(fl: u32) -> Result<()> {
    if fl > MAX_FEATURE_LEVEL {
        bail!("This build uses a newer manifest format (feature level {fl}) than the app knows (up to {MAX_FEATURE_LEVEL})");
    }
    Ok(())
}

fn sha1(data: &[u8]) -> [u8; 20] {
    let mut out = [0u8; 20];
    out.copy_from_slice(&Sha1::digest(data)[..]);
    out
}

/// Reads the little-endian numbers and Unreal strings of a manifest or chunk.
struct Rd<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Rd<'a> {
    fn new(b: &'a [u8]) -> Self {
        Rd { b, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.b.len()).ok_or_else(|| anyhow!("The file ends too early"))?;
        let s = &self.b[self.at..end];
        self.at = end;
        Ok(s)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into()?))
    }

    fn sha(&mut self) -> Result<[u8; 20]> {
        Ok(self.take(20)?.try_into()?)
    }

    fn guid(&mut self) -> Result<Guid> {
        Ok(Guid([self.u32()?, self.u32()?, self.u32()?, self.u32()?]))
    }

    /// An Unreal FString: a signed length (with the end zero), negative for UTF-16.
    fn fstring(&mut self) -> Result<String> {
        let n = self.u32()? as i32;
        let s = if n < 0 {
            let units: Vec<u16> = self.take(n.unsigned_abs() as usize * 2)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(self.take(n as usize)?).into_owned()
        };
        Ok(s.trim_end_matches('\0').to_string())
    }

    /// A count of entries that take at least `min` bytes each; refused when the rest could not
    /// hold them (a broken file never makes a huge list).
    fn count(&mut self, min: usize) -> Result<usize> {
        let n = self.u32()? as usize;
        if n.saturating_mul(min.max(1)) > self.b.len() - self.at {
            bail!("The file names more entries than it holds");
        }
        Ok(n)
    }

    /// A section of the manifest body: its size (counted from its start), then its data version.
    /// Returns a reader of just the section, after the version.
    fn section(&mut self, what: &str) -> Result<(Rd<'a>, u8)> {
        let start = self.at;
        let size = self.u32()? as usize;
        let end = start.checked_add(size).filter(|e| size >= 5 && *e <= self.b.len()).ok_or_else(|| anyhow!("The build's {what} is cut short"))?;
        let mut s = Rd { b: &self.b[start..end], at: 4 };
        let v = s.u8()?;
        self.at = end;
        Ok((s, v))
    }
}

fn newer(what: &str, v: u8) -> anyhow::Error {
    anyhow!("This build's {what} uses a newer format (version {v}) than the app knows")
}

fn parse_binary(b: &[u8]) -> Result<Manifest> {
    let mut r = Rd::new(b);
    if r.u32().ok() != Some(MANIFEST_MAGIC) {
        bail!("This is not an Epic build manifest");
    }
    let header_size = r.u32()? as usize;
    let size_raw = r.u32()? as usize;
    let size_packed = r.u32()? as usize;
    let sha = r.sha()?;
    let stored = r.u8()?;
    let version = if header_size >= 41 { r.u32()? } else { 0 };
    check_level(version)?;
    if stored & 2 != 0 {
        bail!("This build's file list is encrypted; the app cannot read it");
    }
    if size_raw > 1 << 30 {
        bail!("This build's file list is too big to be one");
    }
    let rest = b.get(header_size..).ok_or_else(|| anyhow!("The build's file list ends too early"))?;
    let body: std::borrow::Cow<[u8]> = if stored & 1 != 0 {
        let packed = rest.get(..size_packed).ok_or_else(|| anyhow!("The build's file list ends too early"))?;
        std::borrow::Cow::Owned(miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(packed, size_raw).map_err(|e| anyhow!("The build's file list could not be unpacked ({:?})", e.status))?)
    } else {
        std::borrow::Cow::Borrowed(rest.get(..size_raw).ok_or_else(|| anyhow!("The build's file list ends too early"))?)
    };
    if body.len() != size_raw {
        bail!("The build's file list is not the size its header says");
    }
    if sha != [0u8; 20] && sha1(&body) != sha {
        bail!("The build's file list does not match its checksum");
    }
    let mut r = Rd::new(&body);
    let meta = read_meta(&mut r)?;
    let chunks = read_chunks(&mut r, meta.feature_level)?;
    let files = read_files(&mut r)?;
    let custom = read_custom(&mut r)?;
    Ok(Manifest { meta, chunks, files, custom })
}

fn read_meta(r: &mut Rd) -> Result<Meta> {
    let (mut s, v) = r.section("build information")?;
    if v > 2 {
        return Err(newer("build information", v));
    }
    let feature_level = s.u32()?;
    check_level(feature_level)?;
    let is_file_data = s.u8()? == 1;
    let app_id = s.u32()?;
    let app_name = s.fstring()?;
    let build_version = s.fstring()?;
    let launch_exe = s.fstring()?;
    let launch_command = s.fstring()?;
    let n = s.count(4)?;
    let prereq_ids = (0..n).map(|_| s.fstring()).collect::<Result<Vec<_>>>()?;
    let prereq_name = s.fstring()?;
    let prereq_path = s.fstring()?;
    let prereq_args = s.fstring()?;
    let build_id = if v >= 1 { s.fstring()? } else { String::new() };
    if v >= 2 {
        // The program and arguments that uninstall it: not used, the app removes the folder.
        s.fstring()?;
        s.fstring()?;
    }
    Ok(Meta { feature_level, is_file_data, app_id, app_name, build_version, launch_exe, launch_command, prereq_ids, prereq_name, prereq_path, prereq_args, build_id })
}

/// The chunk list: one array per field, each over all chunks.
fn read_chunks(r: &mut Rd, fl: u32) -> Result<Vec<ChunkInfo>> {
    let (mut s, _) = r.section("chunk list")?;
    let per = 16 + 8 + 20 + 1 + 4 + 8 + if fl >= 22 { 36 } else { 0 };
    let n = s.count(per)?;
    let mut c = (0..n).map(|_| s.guid().map(|guid| ChunkInfo { guid, ..Default::default() })).collect::<Result<Vec<_>>>()?;
    for x in &mut c {
        x.hash = s.u64()?;
    }
    for x in &mut c {
        x.sha1 = s.sha()?;
    }
    for x in &mut c {
        x.group = s.u8()?;
    }
    for x in &mut c {
        x.window_size = s.u32()?;
    }
    for x in &mut c {
        x.file_size = s.u64()?;
    }
    if fl >= 22 {
        for x in &mut c {
            x.secret = s.guid()?;
        }
        // Each chunk's packed window size, then its encryption tag: not needed for plain chunks.
        s.take(n * 4)?;
        s.take(n * 16)?;
    }
    Ok(c)
}

/// The file list: one array per field, each over all files.
fn read_files(r: &mut Rd) -> Result<Vec<FileEntry>> {
    let (mut s, v) = r.section("file list")?;
    if v > 2 {
        return Err(newer("file list", v));
    }
    let n = s.count(4 + 4 + 20 + 1 + 4 + 4)?;
    let mut f = (0..n).map(|_| s.fstring().map(|name| FileEntry { name, ..Default::default() })).collect::<Result<Vec<_>>>()?;
    for x in &mut f {
        x.symlink = s.fstring()?;
    }
    for x in &mut f {
        x.sha1 = s.sha()?;
    }
    for x in &mut f {
        x.flags = s.u8()?;
    }
    for x in &mut f {
        let k = s.count(4)?;
        x.install_tags = (0..k).map(|_| s.fstring()).collect::<Result<Vec<_>>>()?;
    }
    for x in &mut f {
        let k = s.count(28)?;
        for _ in 0..k {
            let size = s.u32()? as usize;
            if size < 28 {
                bail!("A file's part is described in {size} bytes, fewer than a part needs");
            }
            let guid = s.guid()?;
            let offset = s.u32()?;
            let len = s.u32()?;
            s.take(size - 28)?;
            x.parts.push(ChunkPart { guid, offset, size: len });
        }
    }
    if v >= 1 {
        // MD5 (when the file has one), then MIME type: not used.
        for _ in 0..n {
            if s.u32()? != 0 {
                s.take(16)?;
            }
        }
        for _ in 0..n {
            s.fstring()?;
        }
    }
    if v >= 2 {
        // SHA-256: the SHA1 is checked instead.
        s.take(n * 32)?;
    }
    Ok(f)
}

fn read_custom(r: &mut Rd) -> Result<BTreeMap<String, String>> {
    if r.at >= r.b.len() {
        return Ok(BTreeMap::new());
    }
    let (mut s, v) = r.section("custom fields")?;
    // Builds of feature level 21 write version 3 (seen in a real manifest); the layout is the same.
    if v > 3 {
        return Err(newer("custom fields", v));
    }
    let n = s.count(8)?;
    let keys = (0..n).map(|_| s.fstring()).collect::<Result<Vec<_>>>()?;
    let values = (0..n).map(|_| s.fstring()).collect::<Result<Vec<_>>>()?;
    Ok(keys.into_iter().zip(values).collect())
}

/// A number in a JSON manifest: its bytes as 3-digit decimals, lowest byte first.
fn blob_bytes(s: &str) -> Result<Vec<u8>> {
    let s = s.trim();
    if s.len() % 3 != 0 || !s.bytes().all(|c| c.is_ascii_digit()) {
        bail!("A number in the build's file list is not written the way Epic writes them");
    }
    s.as_bytes()
        .chunks(3)
        .map(|c| {
            let n: u32 = std::str::from_utf8(c).unwrap_or("0").parse().unwrap_or(0);
            u8::try_from(n).map_err(|_| anyhow!("A number in the build's file list is not written the way Epic writes them"))
        })
        .collect()
}

fn blob_num(s: &str) -> Result<u64> {
    let b = blob_bytes(s)?;
    if b.len() > 8 {
        bail!("A number in the build's file list is too long");
    }
    Ok(b.iter().rev().fold(0u64, |a, &x| (a << 8) | x as u64))
}

fn get_ci<'v>(o: &'v serde_json::Map<String, Value>, k: &str) -> Option<&'v Value> {
    o.get(k).or_else(|| o.iter().find(|(x, _)| x.eq_ignore_ascii_case(k)).map(|(_, v)| v))
}

fn parse_json(b: &[u8]) -> Result<Manifest> {
    let text = String::from_utf8_lossy(b);
    let v: Value = serde_json::from_str(text.trim_start_matches('\u{feff}').trim()).context("The build's file list is not valid JSON")?;
    let s = |x: Option<&Value>| x.and_then(Value::as_str).unwrap_or("").to_string();
    let num = |x: Option<&Value>, default: u64| -> Result<u64> {
        match x.and_then(Value::as_str) {
            Some(t) => blob_num(t),
            None => Ok(default),
        }
    };
    let num32 = |x: Option<&Value>| -> Result<u32> { u32::try_from(num(x, 0)?).map_err(|_| anyhow!("A number in the build's file list is too big")) };
    let strings = |x: Option<&Value>| -> Vec<String> { x.and_then(Value::as_array).map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default() };
    let feature_level = u32::try_from(num(v.get("ManifestFileVersion"), 13)?).map_err(|_| anyhow!("The build's format number is not one"))?;
    check_level(feature_level)?;
    let meta = Meta {
        feature_level,
        is_file_data: v.get("bIsFileData").and_then(Value::as_bool).unwrap_or(false),
        app_id: num32(v.get("AppID"))?,
        app_name: s(v.get("AppNameString")),
        build_version: s(v.get("BuildVersionString")),
        launch_exe: s(v.get("LaunchExeString")),
        launch_command: s(v.get("LaunchCommand")),
        prereq_ids: strings(v.get("PrereqIds")),
        prereq_name: s(v.get("PrereqName")),
        prereq_path: s(v.get("PrereqPath")),
        prereq_args: s(v.get("PrereqArgs")),
        build_id: String::new(),
    };
    let list = |k: &str| v.get(k).and_then(Value::as_object);
    const LISTS: [&str; 4] = ["ChunkHashList", "ChunkShaList", "DataGroupList", "ChunkFilesizeList"];
    let mut guids = BTreeSet::new();
    for l in LISTS {
        for k in list(l).map(|o| o.keys().collect::<Vec<_>>()).unwrap_or_default() {
            guids.insert(Guid::from_hex(k).ok_or_else(|| anyhow!("The build's file list names a chunk by an id that is not one"))?);
        }
    }
    let mut chunks = Vec::new();
    for guid in guids {
        let hex = guid.hex();
        let get = |l: &str| list(l).and_then(|o| get_ci(o, &hex));
        let sha1 = match get("ChunkShaList").and_then(Value::as_str) {
            Some(h) => hex::decode(h.trim()).ok().and_then(|b| <[u8; 20]>::try_from(b.as_slice()).ok()).ok_or_else(|| anyhow!("A chunk's SHA1 in the build's file list is not one"))?,
            None => [0u8; 20],
        };
        let group = match get("DataGroupList") {
            Some(x) => u8::try_from(num(Some(x), 0)?).map_err(|_| anyhow!("A chunk's group number is not one"))?,
            None => group_of(guid),
        };
        chunks.push(ChunkInfo { guid, hash: num(get("ChunkHashList"), 0)?, sha1, group, window_size: 1 << 20, file_size: num(get("ChunkFilesizeList"), 0)?, secret: Guid::default() });
    }
    let mut files = Vec::new();
    for f in v.get("FileManifestList").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        let hb = match f.get("FileHash").and_then(Value::as_str) {
            Some(t) => blob_bytes(t)?,
            None => Vec::new(),
        };
        if hb.len() > 20 {
            bail!("A file's SHA1 in the build's file list is too long");
        }
        let mut sha = [0u8; 20];
        sha[..hb.len()].copy_from_slice(&hb);
        let mut parts = Vec::new();
        for p in f.get("FileChunkParts").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
            let guid = p.get("Guid").and_then(Value::as_str).and_then(Guid::from_hex).ok_or_else(|| anyhow!("A file's part names no chunk"))?;
            parts.push(ChunkPart { guid, offset: num32(p.get("Offset"))?, size: num32(p.get("Size"))? });
        }
        let flag = |k: &str, bit: u8| if f.get(k).and_then(Value::as_bool).unwrap_or(false) { bit } else { 0 };
        files.push(FileEntry {
            name: s(f.get("Filename")),
            symlink: s(f.get("SymlinkTarget")),
            sha1: sha,
            flags: flag("bIsReadOnly", 1) | flag("bIsCompressed", 2) | flag("bIsUnixExecutable", 4),
            install_tags: strings(f.get("InstallTags")),
            parts,
        });
    }
    let custom = v
        .get("CustomFields")
        .and_then(Value::as_object)
        .map(|o| o.iter().map(|(k, x)| (k.clone(), x.as_str().map(String::from).unwrap_or_else(|| x.to_string()))).collect())
        .unwrap_or_default();
    Ok(Manifest { meta, chunks, files, custom })
}

/// CRC-32 (the zip one).
fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// A chunk's folder number when the manifest has none: CRC-32 of its id, modulo 100.
fn group_of(g: Guid) -> u8 {
    (crc32(&g.le_bytes()) % 100) as u8
}

/// Epic's rolling hash of a chunk's data: a CRC-64 table, the hash turned one bit left per byte.
fn rolling_hash(data: &[u8]) -> u64 {
    static TABLE: Lazy<[u64; 256]> = Lazy::new(|| {
        let mut t = [0u64; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut v = i as u64;
            for _ in 0..8 {
                v = if v & 1 != 0 { (v >> 1) ^ 0xC96C_5795_D787_0F42 } else { v >> 1 };
            }
            *e = v;
        }
        t
    });
    data.iter().fold(0u64, |h, &b| h.rotate_left(1) ^ TABLE[b as usize])
}

/// The chunk folder of a feature level.
pub fn chunk_dir(feature_level: u32) -> &'static str {
    match feature_level {
        22.. => "ChunksV5",
        15.. => "ChunksV4",
        6.. => "ChunksV3",
        3.. => "ChunksV2",
        _ => "Chunks",
    }
}

/// Where a chunk is, relative to a CDN base: "ChunksV4/07/<16-hex hash>_<32-hex id>.chunk", and
/// from feature level 22 "ChunksV5/plain/07/<hash>_<id>.chunk" with both in URL-safe base64.
pub fn chunk_path(feature_level: u32, c: &ChunkInfo) -> String {
    let dir = chunk_dir(feature_level);
    let group = c.group;
    if feature_level >= 22 {
        use base64::Engine;
        let b64 = &base64::engine::general_purpose::URL_SAFE_NO_PAD;
        let secret = if c.secret.is_zero() { "plain".to_string() } else { b64.encode(c.secret.le_bytes()) };
        format!("{dir}/{secret}/{group:02}/{}_{}.chunk", b64.encode(c.hash.to_le_bytes()), b64.encode(c.guid.le_bytes()))
    } else {
        format!("{dir}/{group:02}/{:016X}_{}.chunk", c.hash, c.guid.hex())
    }
}

pub fn chunk_url(base: &str, feature_level: u32, c: &ChunkInfo) -> String {
    format!("{}/{}", base.trim_end_matches('/'), chunk_path(feature_level, c))
}

/// A chunk file's header.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ChunkHeader {
    pub version: u32,
    pub header_size: u32,
    pub compressed_size: u32,
    pub guid: Guid,
    pub hash: u64,
    /// 1 zlib packed, 2 encrypted
    pub stored_as: u8,
    /// Header version 2 on
    pub sha1: [u8; 20],
    /// 1 rolling hash, 2 SHA1, 3 both
    pub hash_type: u8,
    /// Header version 3 on (else 0)
    pub uncompressed_size: u32,
}

/// The biggest chunk unpacked when its header does not say its size.
const MAX_CHUNK: usize = 64 << 20;

/// A chunk file as served: its header and its data unpacked, checked against the chunk's own
/// hash (its SHA1 when it has one, else its rolling hash).
pub fn read_chunk(bytes: &[u8]) -> Result<(ChunkHeader, Vec<u8>)> {
    let mut r = Rd::new(bytes);
    if r.u32().ok() != Some(CHUNK_MAGIC) {
        bail!("A downloaded chunk is not an Epic chunk");
    }
    let version = r.u32()?;
    if !(1..=4).contains(&version) {
        bail!("A chunk uses a newer format (version {version}) than the app knows");
    }
    let mut h = ChunkHeader { version, header_size: r.u32()?, compressed_size: r.u32()?, guid: r.guid()?, hash: r.u64()?, stored_as: r.u8()?, hash_type: 1, ..Default::default() };
    if version >= 2 {
        h.sha1 = r.sha()?;
        h.hash_type = r.u8()?;
    }
    if version >= 3 {
        h.uncompressed_size = r.u32()?;
    }
    let mut encrypted = h.stored_as & 2 != 0;
    if version >= 4 {
        encrypted |= !r.guid()?.is_zero();
        r.take(16)?;
    }
    if encrypted {
        bail!("A chunk of this game is encrypted; the app cannot install it");
    }
    if r.at > h.header_size as usize {
        bail!("A chunk's header is shorter than its fields");
    }
    let data = bytes.get(h.header_size as usize..).and_then(|d| d.get(..h.compressed_size as usize)).ok_or_else(|| anyhow!("A chunk ends too early"))?;
    let limit = if h.uncompressed_size > 0 { h.uncompressed_size as usize } else { MAX_CHUNK };
    let data = if h.stored_as & 1 != 0 {
        miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(data, limit).map_err(|e| anyhow!("A chunk could not be unpacked ({:?})", e.status))?
    } else {
        data.to_vec()
    };
    if h.uncompressed_size > 0 && data.len() != h.uncompressed_size as usize {
        bail!("A chunk unpacked to another size than its header says");
    }
    if h.hash_type & 2 != 0 {
        if sha1(&data) != h.sha1 {
            bail!("A chunk does not match its hash");
        }
    } else if h.hash_type & 1 != 0 && rolling_hash(&data) != h.hash {
        bail!("A chunk does not match its hash");
    }
    Ok((h, data))
}

/// Downloads. Behind a trait so tests serve manifests and chunks from memory.
trait Fetch: Send + Sync {
    /// The body of `url`. `bearer` goes in the Authorization header when given.
    fn get<'a>(&'a self, url: &'a str, bearer: Option<&'a str>) -> BoxFuture<'a, Result<Vec<u8>>>;
}

/// The internet. Errors name the host only: an address can hold a signed token.
struct Net;

impl Fetch for Net {
    fn get<'a>(&'a self, url: &'a str, bearer: Option<&'a str>) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let host = crate::http::host(url).to_string();
            let req = match bearer {
                Some(t) => {
                    let mut v = reqwest::header::HeaderValue::from_str(&format!("bearer {t}")).map_err(|_| anyhow!("The sign-in token is not one"))?;
                    v.set_sensitive(true);
                    crate::http::CLIENT.get(url).header(reqwest::header::AUTHORIZATION, v)
                }
                None => crate::http::DL_CLIENT.get(url),
            };
            let resp = req.timeout(Duration::from_secs(300)).send().await.map_err(|e| anyhow!("Could not reach {host}: {}", e.without_url()))?;
            let status = resp.status();
            if !status.is_success() {
                bail!("{host} answered HTTP {}", status.as_u16());
            }
            Ok(resp.bytes().await.map_err(|e| anyhow!("The download from {host} broke off: {}", e.without_url()))?.to_vec())
        })
    }
}

/// Epic's launcher service, which says where a game's newest build is.
const LAUNCHER_HOST: &str = "https://launcher-public-service-prod06.ol.epicgames.com";

/// Percent-encodes everything but letters, digits and "-_.~".
fn enc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn assets_url(namespace: &str, catalog_item: &str, app_name: &str) -> String {
    format!("{LAUNCHER_HOST}/launcher/api/public/assets/v2/platform/Windows/namespace/{}/catalogItem/{}/app/{}/label/Live", enc(namespace), enc(catalog_item), enc(app_name))
}

/// What the launcher service says about a game's newest build.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Assets {
    build: String,
    /// SHA1 of the manifest file
    hash: String,
    /// Where the manifest is, with the query the service gave
    manifests: Vec<String>,
    /// Where its chunks are
    bases: Vec<String>,
}

fn parse_assets(json: &[u8], app_name: &str) -> Result<Assets> {
    let v: Value = serde_json::from_slice(json).context("Epic's answer about the game's build is not JSON")?;
    let els = v.get("elements").and_then(Value::as_array).filter(|a| !a.is_empty()).ok_or_else(|| anyhow!("Epic's service lists no build of this game"))?;
    let el = els.iter().find(|e| e.get("appName").and_then(Value::as_str).is_some_and(|a| a.eq_ignore_ascii_case(app_name))).unwrap_or(&els[0]);
    let s = |k: &str| el.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string();
    let mut a = Assets { build: s("buildVersion"), hash: s("hash"), ..Default::default() };
    for m in el.get("manifests").and_then(Value::as_array).map(Vec::as_slice).unwrap_or(&[]) {
        let Some(uri) = m.get("uri").and_then(Value::as_str).map(str::trim) else { continue };
        if !(uri.starts_with("https://") || uri.starts_with("http://")) {
            continue;
        }
        let params: Vec<String> = m
            .get("queryParams")
            .and_then(Value::as_array)
            .map(|ps| ps.iter().filter_map(|p| Some(format!("{}={}", p.get("name")?.as_str()?, p.get("value")?.as_str()?))).collect())
            .unwrap_or_default();
        let url = if params.is_empty() { uri.to_string() } else { format!("{uri}{}{}", if uri.contains('?') { '&' } else { '?' }, params.join("&")) };
        let path = uri.split('?').next().unwrap_or(uri);
        if let Some((base, _)) = path.rsplit_once('/') {
            if !a.bases.iter().any(|b| b == base) {
                a.bases.push(base.to_string());
            }
        }
        a.manifests.push(url);
    }
    Ok(a)
}

async fn assets(f: &dyn Fetch, token: &str, namespace: &str, catalog_item: &str, app_name: &str) -> Result<Assets> {
    let body = f.get(&assets_url(namespace, catalog_item, app_name), Some(token)).await.context("Epic's launcher service did not say where the game's build is")?;
    parse_assets(&body, app_name)
}

/// A build as Epic serves it now.
struct Live {
    manifest: Manifest,
    /// The manifest file as downloaded (kept in the game folder)
    raw: Vec<u8>,
    bases: Vec<String>,
}

async fn live_manifest(f: &dyn Fetch, token: &str, namespace: &str, catalog_item: &str, app_name: &str) -> Result<Live> {
    let a = assets(f, token, namespace, catalog_item, app_name).await?;
    if a.manifests.is_empty() {
        bail!("Epic's service gave no address for the game's build");
    }
    let mut last = None;
    for url in &a.manifests {
        match f.get(url, None).await {
            Ok(raw) => {
                if !a.hash.is_empty() && !hex::encode(sha1(&raw)).eq_ignore_ascii_case(&a.hash) {
                    last = Some(anyhow!("The build's file list from {} does not match its checksum", crate::http::host(url)));
                    continue;
                }
                let manifest = Manifest::parse(&raw)?;
                let mut bases = a.bases.clone();
                for b in manifest.own_bases() {
                    if !bases.contains(&b) {
                        bases.push(b);
                    }
                }
                return Ok(Live { manifest, raw, bases });
            }
            Err(e) => last = Some(e),
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("No address worked")).context("Could not download the build's file list"))
}

/// The newest build of a game as Epic serves it, and the CDN bases its chunks are on.
pub async fn manifest_for(token: &str, namespace: &str, catalog_item: &str, app_name: &str) -> Result<(Manifest, Vec<String>)> {
    let live = live_manifest(&Net, token, namespace, catalog_item, app_name).await?;
    Ok((live.manifest, live.bases))
}

/// The newest build version of a game (Epic's build string), for update checks.
pub async fn latest_build(token: &str, namespace: &str, catalog_item: &str, app_name: &str) -> Result<String> {
    latest_with(&Net, token, namespace, catalog_item, app_name).await
}

async fn latest_with(f: &dyn Fetch, token: &str, namespace: &str, catalog_item: &str, app_name: &str) -> Result<String> {
    let a = assets(f, token, namespace, catalog_item, app_name).await?;
    if a.build.is_empty() {
        bail!("Epic's service names no build of this game");
    }
    Ok(a.build)
}

/// How a sync downloads and keeps chunks.
struct Tune {
    /// Chunks downloaded at once
    parallel: usize,
    /// Chunks kept in memory for files further on; the rest wait in `.u3d\chunks`
    cache_bytes: usize,
    /// Files up to this size are put together in memory, bigger ones in a part file
    in_memory: u64,
    report_every: Duration,
}

const TUNE: Tune = Tune { parallel: 8, cache_bytes: 64 << 20, in_memory: 32 << 20, report_every: Duration::from_secs(1) };

/// Room on the drive beyond the files themselves (chunks waiting in `.u3d\chunks`).
const HEADROOM: u64 = 256 << 20;

/// Installs, updates or repairs an Epic game in `dest` to the build Epic serves now. Only whole
/// files that match the build's SHA1 replace old ones; the replaced files are kept in the game's
/// undo folder. A canceled or failed sync puts back what it replaced, so the build installed
/// before keeps working. `.u3d\game.json` is written only when the whole sync succeeded.
pub async fn sync(token: &str, game: &EpicRef, name: &str, dest: &Path, mode: Mode, progress: &(dyn Fn(Progress) + Send + Sync), cancel: &AtomicBool) -> Result<Report> {
    sync_with(Arc::new(Net), token, game, name, dest, mode, progress, cancel, &TUNE).await
}

/// The manifest of the last sync, and the one before (for an update that was put back).
fn saved_path(dest: &Path) -> PathBuf {
    dest.join(OWN_DIR).join("manifest.bin")
}

fn prev_path(dest: &Path) -> PathBuf {
    dest.join(OWN_DIR).join("manifest.prev.bin")
}

fn chunks_dir(dest: &Path) -> PathBuf {
    dest.join(OWN_DIR).join("chunks")
}

/// The saved manifest of the build installed now (`state.build`).
fn saved_manifest(dest: &Path, state: Option<&State>) -> Option<Manifest> {
    let s = state?;
    [saved_path(dest), prev_path(dest)].iter().find_map(|p| {
        let raw = std::fs::read(p).ok()?;
        (build_meta(&raw)?.build == s.build).then(|| Manifest::parse(&raw).ok()).flatten()
    })
}

/// Keeps the new build's manifest; the one before stays as the previous when it was another
/// build.
fn save_manifest(dest: &Path, raw: &[u8], build: &str) -> Result<()> {
    let cur = saved_path(dest);
    std::fs::create_dir_all(cur.parent().unwrap_or(dest))?;
    if let Ok(old) = std::fs::read(&cur) {
        if build_meta(&old).is_some_and(|m| m.build != build) {
            let prev = prev_path(dest);
            let _ = std::fs::remove_file(&prev);
            std::fs::rename(&cur, &prev)?;
        }
    }
    let part = cur.with_extension("bin.part");
    std::fs::write(&part, raw)?;
    std::fs::rename(&part, &cur)?;
    Ok(())
}

fn size_text(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{} MB", b.div_ceil(1 << 20))
    }
}

fn check_space(dest: &Path, need: u64) -> Result<()> {
    if let Some((free, _)) = crate::winutil::disk_space(dest) {
        if free < need {
            bail!("Not enough free space: the game needs {} and the drive has {}", size_text(need), size_text(free));
        }
    }
    Ok(())
}

/// Splits a command line the way Windows programs read it: spaces part arguments, except in
/// double quotes.
fn split_command(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for c in s.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        out.push(cur);
    }
    out
}

/// Sends progress at the start of each step and then at most every `every`.
struct Meter<'a> {
    f: &'a (dyn Fn(Progress) + Send + Sync),
    p: Progress,
    last: Option<Instant>,
    every: Duration,
}

impl Meter<'_> {
    fn start(&mut self, step: &str, total_bytes: u64, files_total: usize) {
        self.p = Progress { step: step.to_string(), total_bytes, files_total, ..Default::default() };
        self.send();
    }

    fn add(&mut self, n: u64) {
        self.p.done_bytes += n;
        self.tick();
    }

    fn file_done(&mut self) {
        self.p.files_done += 1;
        self.tick();
    }

    fn tick(&mut self) {
        if self.last.is_none_or(|t| t.elapsed() >= self.every) {
            self.send();
        }
    }

    fn send(&mut self) {
        (self.f)(self.p.clone());
        self.last = Some(Instant::now());
    }
}

/// SHA1 of a file on disk, read in pieces (cancel and progress between them).
async fn sha1_file(path: &Path, cancel: &AtomicBool, meter: &mut Meter<'_>) -> Result<[u8; 20]> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha1::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        check_cancel(cancel)?;
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        meter.add(n as u64);
        tokio::task::yield_now().await;
    }
    let mut out = [0u8; 20];
    out.copy_from_slice(&h.finalize()[..]);
    Ok(out)
}

/// The undo folder of this sync, started when the first file is replaced (until then the one of
/// the update before stays).
struct Changes<'a> {
    dest: &'a Path,
    before: Option<State>,
    undo: Option<Undo>,
    begun: bool,
}

impl Changes<'_> {
    fn undo(&mut self) -> Result<&mut Undo> {
        if self.undo.is_none() {
            self.undo = Some(Undo::begin(self.dest, self.before.clone())?);
            self.begun = true;
        }
        Ok(self.undo.as_mut().expect("just set"))
    }

    fn keep(&mut self, rel: &Path) -> Result<()> {
        self.undo()?.keep(rel)
    }

    fn remove(&mut self, rel: &Path) -> Result<()> {
        if self.dest.join(rel).is_file() {
            self.undo()?.remove(rel)?;
        }
        Ok(())
    }

    fn finish(&mut self) -> Result<Option<PathBuf>> {
        match self.undo.take() {
            Some(u) => u.finish(),
            None => Ok(None),
        }
    }
}

/// After a failed or canceled sync: what it replaced is put back. A failed install keeps the
/// whole files it wrote (they are checked), so a new install goes on from there.
fn roll_back(dest: &Path, mode: Mode, begun: bool) -> Result<()> {
    if !begun {
        return Ok(());
    }
    match sync::can_undo(dest) {
        Some(log) if mode != Mode::Install || !log.replaced.is_empty() => sync::undo(dest).map(|_| ()),
        _ => {
            let _ = std::fs::remove_dir_all(sync::undo_dir(dest));
            Ok(())
        }
    }
}

/// The part file a big file is put together in, beside it (the name `write_file` uses). Removed
/// unless it became the file.
struct PartFile {
    path: PathBuf,
    out: Option<std::io::BufWriter<std::fs::File>>,
    done: bool,
}

impl Drop for PartFile {
    fn drop(&mut self) {
        self.out.take();
        if !self.done {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Where a file being written goes until it is whole and checked.
enum Sink {
    Mem(Vec<u8>),
    Part(PartFile),
}

impl Sink {
    fn new(dest: &Path, rel: &Path, size: u64, in_memory: u64) -> Result<Sink> {
        if size <= in_memory {
            return Ok(Sink::Mem(Vec::with_capacity(size as usize)));
        }
        let to = dest.join(rel);
        std::fs::create_dir_all(to.parent().unwrap_or(dest))?;
        let path = to.with_extension(format!("{}u3dpart", to.extension().map(|e| format!("{}.", e.to_string_lossy())).unwrap_or_default()));
        let f = std::fs::File::create(&path)?;
        Ok(Sink::Part(PartFile { path, out: Some(std::io::BufWriter::with_capacity(1 << 20, f)), done: false }))
    }

    fn write(&mut self, b: &[u8]) -> Result<()> {
        match self {
            Sink::Mem(v) => v.extend_from_slice(b),
            Sink::Part(p) => p.out.as_mut().expect("open").write_all(b)?,
        }
        Ok(())
    }

    /// The file takes its place in the game folder.
    fn finish(self, dest: &Path, rel: &Path) -> Result<()> {
        match self {
            Sink::Mem(v) => write_file(dest, rel, &v),
            Sink::Part(mut p) => {
                p.out.take().expect("open").into_inner().map_err(|e| e.into_error())?;
                let to = dest.join(rel);
                if to.exists() {
                    std::fs::remove_file(&to)?;
                }
                std::fs::rename(&p.path, &to)?;
                p.done = true;
                Ok(())
            }
        }
    }
}

/// A spawned download, stopped when dropped (a canceled sync leaves none running).
struct Task<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for Task<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl<T> std::future::Future for Task<T> {
    type Output = std::result::Result<T, tokio::task::JoinError>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
        std::future::Future::poll(Pin::new(&mut self.0), cx)
    }
}

/// A chunk to download: its addresses on each CDN, and the SHA1 the manifest gives it.
struct Job {
    guid: Guid,
    urls: Vec<String>,
    sha1: Option<[u8; 20]>,
}

/// A chunk downloaded and checked: its data unpacked, and how many bytes came.
struct Got {
    guid: Guid,
    data: Vec<u8>,
    size: u64,
}

fn check_chunk(raw: &[u8], job: &Job) -> Result<Vec<u8>> {
    let (h, data) = read_chunk(raw)?;
    if h.guid != job.guid {
        bail!("The server sent another chunk than the one asked for");
    }
    // A chunk without its own SHA1 is checked against the manifest's.
    if h.hash_type & 2 == 0 {
        if let Some(want) = job.sha1 {
            if sha1(&data) != want {
                bail!("A chunk does not match its hash");
            }
        }
    }
    Ok(data)
}

/// One chunk, from the first CDN that serves it whole; three rounds over them all.
async fn get_chunk(f: Arc<dyn Fetch>, job: Job) -> Result<Got> {
    let mut last = None;
    for round in 0..3u64 {
        if round > 0 {
            tokio::time::sleep(Duration::from_millis(500 * round)).await;
        }
        for url in &job.urls {
            let got = match f.get(url, None).await {
                Ok(raw) => check_chunk(&raw, &job).map(|data| Got { guid: job.guid, data, size: raw.len() as u64 }),
                Err(e) => Err(e),
            };
            match got {
                Ok(g) => return Ok(g),
                Err(e) => last = Some(e),
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("There is no address to download it from")).context(format!("Could not download chunk {}", job.guid)))
}

/// The chunks files are made of: downloaded several at once in the order they are first needed,
/// kept while files further on need them (in memory up to the budget, else in `.u3d\chunks`),
/// and forgotten after their last use.
struct Cache {
    dir: PathBuf,
    budget: usize,
    mem: HashMap<Guid, Arc<Vec<u8>>>,
    mem_bytes: usize,
    disk: HashSet<Guid>,
    /// The places in the write order where each chunk is still needed
    uses: HashMap<Guid, VecDeque<usize>>,
    stream: BoxStream<'static, Result<Got>>,
    downloaded: u64,
}

impl Cache {
    async fn take(&mut self, g: Guid, cancel: &AtomicBool, meter: &mut Meter<'_>) -> Result<Arc<Vec<u8>>> {
        if let Some(d) = self.mem.get(&g) {
            return Ok(d.clone());
        }
        let d = if self.disk.contains(&g) {
            Arc::new(std::fs::read(self.dir.join(g.hex()))?)
        } else {
            check_cancel(cancel)?;
            let got = loop {
                match tokio::time::timeout(Duration::from_millis(500), self.stream.next()).await {
                    Ok(Some(r)) => break r?,
                    Ok(None) => bail!("The chunk list ended before the files did"),
                    Err(_) => {
                        check_cancel(cancel)?;
                        meter.tick();
                    }
                }
            };
            if got.guid != g {
                bail!("The chunks came in another order than asked");
            }
            self.downloaded += got.size;
            Arc::new(got.data)
        };
        self.mem_bytes += d.len();
        self.mem.insert(g, d.clone());
        Ok(d)
    }

    /// One use of `g` is done.
    fn used(&mut self, g: Guid) -> Result<()> {
        let left = self.uses.get_mut(&g).map(|q| {
            q.pop_front();
            q.len()
        });
        if left.unwrap_or(0) == 0 {
            self.uses.remove(&g);
            if let Some(d) = self.mem.remove(&g) {
                self.mem_bytes -= d.len();
            }
            if self.disk.remove(&g) {
                let _ = std::fs::remove_file(self.dir.join(g.hex()));
            }
        }
        // Over the budget: the chunks needed last wait on disk.
        while self.mem_bytes > self.budget {
            let Some(victim) = self.mem.keys().copied().max_by_key(|k| self.uses.get(k).and_then(|q| q.front()).copied().unwrap_or(usize::MAX)) else { break };
            let d = self.mem.remove(&victim).expect("listed");
            self.mem_bytes -= d.len();
            if self.disk.insert(victim) {
                std::fs::create_dir_all(&self.dir)?;
                std::fs::write(self.dir.join(victim.hex()), &*d)?;
            }
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
async fn sync_with(fetch: Arc<dyn Fetch>, token: &str, game: &EpicRef, name: &str, dest: &Path, mode: Mode, progress: &(dyn Fn(Progress) + Send + Sync), cancel: &AtomicBool, tune: &Tune) -> Result<Report> {
    check_cancel(cancel)?;
    let before = read_state(dest);
    match &before {
        Some(s) if s.platform != "epic" || !s.game.eq_ignore_ascii_case(&game.app_name) => bail!("This folder holds another game"),
        None if mode != Mode::Install => bail!("The app did not install this game here, so it cannot update or repair it"),
        _ => {}
    }
    let mut meter = Meter { f: progress, p: Progress::default(), last: None, every: tune.report_every };
    meter.start("Checking files", 0, 0);
    let live = live_manifest(&*fetch, token, &game.namespace, &game.catalog_item, &game.app_name).await?;
    check_cancel(cancel)?;
    let m = &live.manifest;
    let rels = m.checked_paths()?;
    let build = m.meta.build_version.clone();

    // What to write: the files that are missing, have another size, or (in an update) changed
    // since the build installed; the rest are checked by their SHA1 when there is no saved
    // manifest to compare with, or in a repair.
    let old = if mode == Mode::Install { None } else { saved_manifest(dest, before.as_ref()) };
    let old_files: HashMap<String, &FileEntry> = old.iter().flat_map(|o| o.files.iter()).map(|f| (key(&f.name), f)).collect();
    let check_all = mode != Mode::Update || old.is_none();
    let mut todo = Vec::new();
    let mut to_hash = Vec::new();
    for (i, f) in m.files.iter().enumerate() {
        let on_disk = std::fs::metadata(dest.join(&rels[i])).ok().filter(|md| md.is_file()).map(|md| md.len());
        if on_disk != Some(f.size()) {
            todo.push(i);
        } else if check_all {
            to_hash.push(i);
        } else if old_files.get(&key(&f.name)).is_none_or(|o| o.sha1 != f.sha1) {
            todo.push(i);
        }
    }
    meter.start("Checking files", to_hash.iter().map(|&i| m.files[i].size()).sum(), to_hash.len());
    for &i in &to_hash {
        if sha1_file(&dest.join(&rels[i]), cancel, &mut meter).await? != m.files[i].sha1 {
            todo.push(i);
        }
        meter.file_done();
    }
    todo.sort_unstable();
    let new_keys: HashSet<String> = m.files.iter().map(|f| key(&f.name)).collect();
    let dropped: Vec<PathBuf> = old.iter().flat_map(|o| o.files.iter()).filter(|f| !new_keys.contains(&key(&f.name))).filter_map(|f| safe_rel(&f.name)).filter(|r| dest.join(r).is_file()).collect();
    let need: u64 = todo.iter().map(|&i| m.files[i].size()).sum();
    if need > 0 {
        check_space(dest, need + HEADROOM)?;
    }
    log::info!("Epic {}: {} of {} files to write, {} to remove", game.app_name, todo.len(), m.files.len(), dropped.len());

    let state = State {
        platform: "epic".into(),
        game: game.app_name.clone(),
        name: name.to_string(),
        version: version_of_build(&build),
        build: build.clone(),
        launch: safe_rel(&m.meta.launch_exe).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
        launch_args: split_command(&m.meta.launch_command),
    };
    let mut changes = Changes { dest, before, undo: None, begun: false };
    let res = apply(&fetch, &live, &rels, &todo, &dropped, &mut changes, &mut meter, cancel, tune, &state).await;
    let _ = std::fs::remove_dir_all(chunks_dir(dest));
    match res {
        Ok((bytes_downloaded, files_written, undo)) => Ok(Report {
            version: state.version,
            bytes_downloaded,
            files_written,
            files_repaired: if mode == Mode::Repair { todo.len() } else { 0 },
            undo,
        }),
        Err(e) => {
            drop(changes.undo.take());
            match roll_back(dest, mode, changes.begun) {
                Ok(()) => Err(e),
                Err(u) => Err(anyhow!("{e:#}. Putting the game folder back failed too: {u:#}")),
            }
        }
    }
}

/// Downloads and writes the files to write, removes the dropped ones, then keeps the manifest
/// and writes the folder's state.
#[allow(clippy::too_many_arguments)]
async fn apply(fetch: &Arc<dyn Fetch>, live: &Live, rels: &[PathBuf], todo: &[usize], dropped: &[PathBuf], changes: &mut Changes<'_>, meter: &mut Meter<'_>, cancel: &AtomicBool, tune: &Tune, state: &State) -> Result<(u64, usize, Option<PathBuf>)> {
    let m = &live.manifest;
    let dest = changes.dest;
    let index = m.chunk_index();
    let mut uses: HashMap<Guid, VecDeque<usize>> = HashMap::new();
    let mut order = Vec::new();
    let mut at = 0usize;
    for &i in todo {
        for p in m.files[i].parts.iter().filter(|p| p.size > 0) {
            let q = uses.entry(p.guid).or_default();
            if q.is_empty() {
                order.push(p.guid);
            }
            q.push_back(at);
            at += 1;
        }
    }
    if !order.is_empty() && live.bases.is_empty() {
        bail!("Epic's service gave no address for the game's chunks");
    }
    let fl = m.meta.feature_level;
    let jobs: Vec<Job> = order
        .iter()
        .map(|g| {
            let c = index[g];
            Job { guid: *g, urls: live.bases.iter().map(|b| chunk_url(b, fl, c)).collect(), sha1: (c.sha1 != [0u8; 20]).then_some(c.sha1) }
        })
        .collect();
    let f = fetch.clone();
    let stream = futures_util::stream::iter(jobs)
        .map(move |j| Task(tokio::spawn(get_chunk(f.clone(), j))))
        .buffered(tune.parallel.max(1))
        .map(|r| r.map_err(|e| anyhow!("A download stopped: {e}")).and_then(|x| x))
        .boxed();
    let mut cache = Cache { dir: chunks_dir(dest), budget: tune.cache_bytes, mem: HashMap::new(), mem_bytes: 0, disk: HashSet::new(), uses, stream, downloaded: 0 };

    meter.start("Downloading", todo.iter().map(|&i| m.files[i].size()).sum(), todo.len());
    let mut written = 0;
    for &i in todo {
        check_cancel(cancel)?;
        let (f, rel) = (&m.files[i], &rels[i]);
        let mut out = Sink::new(dest, rel, f.size(), tune.in_memory)?;
        let mut h = Sha1::new();
        for p in f.parts.iter().filter(|p| p.size > 0) {
            let data = cache.take(p.guid, cancel, meter).await?;
            let piece = data.get(p.offset as usize..p.offset as usize + p.size as usize).ok_or_else(|| anyhow!("{} names bytes past the end of a chunk", rel.display()))?;
            out.write(piece)?;
            h.update(piece);
            drop(data);
            cache.used(p.guid)?;
            meter.add(p.size as u64);
        }
        if h.finalize()[..] != f.sha1[..] {
            bail!("{} does not match its hash after downloading", rel.display());
        }
        changes.keep(rel)?;
        out.finish(dest, rel)?;
        written += 1;
        meter.file_done();
    }
    let downloaded = cache.downloaded;
    drop(cache);

    meter.start("Writing files", 0, dropped.len());
    for rel in dropped {
        changes.remove(rel)?;
        meter.file_done();
    }
    save_manifest(dest, &live.raw, &m.meta.build_version)?;
    let undo = changes.finish()?;
    write_state(dest, state)?;
    Ok((downloaded, written, undo))
}

/// The arguments an Epic game is started with: the build's own launch command, then what the
/// launcher passes (the exchange code is the game's sign-in; without one the game starts
/// offline). None of these are logged.
pub fn launch_args(state: &State, exchange_code: Option<&str>, account_id: &str, display_name: &str) -> Vec<String> {
    let mut a = state.launch_args.clone();
    a.extend([
        "-AUTH_LOGIN=unused".to_string(),
        format!("-AUTH_PASSWORD={}", exchange_code.unwrap_or("")),
        "-AUTH_TYPE=exchangecode".to_string(),
        format!("-epicapp={}", state.game),
        "-epicenv=Prod".to_string(),
        "-EpicPortal".to_string(),
        format!("-epicusername={display_name}"),
        format!("-epicuserid={account_id}"),
        format!("-epiclocale={}", epic_locale()),
    ]);
    a
}

/// The language of Windows for this user ("en"), as the launcher passes it.
fn epic_locale() -> String {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let name: String = RegKey::predef(HKEY_CURRENT_USER).open_subkey(r"Control Panel\International").and_then(|k| k.get_value("LocaleName")).unwrap_or_default();
    let lang = name.split(['-', '_']).next().unwrap_or("").trim().to_ascii_lowercase();
    if lang.is_empty() || !lang.chars().all(|c| c.is_ascii_alphabetic()) {
        "en".to_string()
    } else {
        lang
    }
}

#[cfg(test)]
mod tests;
