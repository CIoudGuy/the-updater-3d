//! Installing, updating and repairing a GOG game without GOG Galaxy, from GOG's content system
//! (generation 2 builds). A build is a manifest (zlib compressed JSON) that names depots; each
//! depot has its own manifest with the files, and every file is a list of chunks (zlib compressed,
//! named by the md5 of the compressed bytes). Small files are packed into a depot's "small files
//! container", which is a list of chunks of its own; a file then names its place in it.
//!
//! The token the caller passes is sent to GOG's content system for the download addresses and
//! nowhere else. Nothing is written outside the game folder: new files are made whole in
//! `.u3d\parts` first and put in place only when every one of them has been checked, so a failed
//! or canceled sync leaves the installed build as it was.
//!
//! Files marked "support" (GOG Galaxy's install script helpers, not game files) are kept under
//! `gog-support\<product id>\`. Install scripts are not run, and redistributables the build lists
//! (`dependencies`) are not installed; `dependencies` gives their ids.
//!
//! Generation 1 builds (games GOG never moved to generation 2) are installed too: a `repository.json`
//! names the depots, a depot manifest lists each file as a slice of the product's `main.bin`, which
//! is downloaded by byte ranges and checked by the file's md5.

use super::sync::{self, Mode, Progress, Report, State, Undo};
use crate::http::DL_CLIENT;
use anyhow::{anyhow, bail, Context, Result};
use futures_util::stream::FuturesUnordered;
use futures_util::StreamExt;
use md5::Md5;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

const CONTENT_SYSTEM: &str = "content-system.gog.com";
/// The most a chunk may say it holds, before or after packing: a bigger one is a broken manifest.
const MAX_CHUNK: u64 = 256 << 20;
/// The most one answer may be (a chunk, a manifest unpacked).
const MAX_BODY: usize = 600 << 20;
/// Where a game's files are kept while a sync is not finished.
const PARTS: &str = "parts";
const MANIFEST_FILE: &str = "gog-manifest.json";
const PREVIOUS_FILE: &str = "gog-manifest.prev.json";
/// Bytes of a generation 1 file asked for at once.
const V1_PIECE: u64 = 8 << 20;

// ---------------------------------------------------------------------------------------------
// Where the bytes come from

/// Where the bytes come from: the real one asks GOG, tests serve fixtures from memory.
pub trait Fetch: Send + Sync {
    /// The body of `url`. `got` is told about the bytes as they arrive.
    fn get(&self, url: &str, got: &(dyn Fn(u64) + Send + Sync)) -> impl Future<Output = Result<Vec<u8>>> + Send;

    /// `len` bytes of `url` from byte `start` on (exactly those, or an error).
    fn get_range(&self, url: &str, start: u64, len: u64, got: &(dyn Fn(u64) + Send + Sync)) -> impl Future<Output = Result<Vec<u8>>> + Send;
}

/// An answer that was not a success.
#[derive(Debug)]
pub struct HttpStatus(pub u16);

impl std::fmt::Display for HttpStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {}", self.0)
    }
}

impl std::error::Error for HttpStatus {}

fn status_of(e: &anyhow::Error) -> Option<u16> {
    e.downcast_ref::<HttpStatus>().map(|s| s.0)
}

/// GOG says this account may not download the product.
#[derive(Debug)]
struct Denied(String);

impl std::fmt::Display for Denied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "GOG did not allow this account to download product {}", self.0)
    }
}

impl std::error::Error for Denied {}

/// Does this request carry the token? Only GOG's download address requests do.
fn sends_token(url: &str) -> bool {
    crate::http::host(url) == CONTENT_SYSTEM && url.contains("/secure_link")
}

/// The real thing: GOG over HTTPS. The token goes only to the content system's download address
/// requests, never to a CDN or the public lists.
pub struct Http {
    token: parking_lot::Mutex<String>,
    renew: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
}

impl Http {
    /// For GOG's public lists and manifests, which need no sign in.
    pub fn public() -> Http {
        Http { token: parking_lot::Mutex::new(String::new()), renew: None }
    }

    /// With the access token; `renew` is asked for a fresh one when GOG says it ran out.
    pub fn with_token(token: &str, renew: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>) -> Http {
        Http { token: parking_lot::Mutex::new(token.to_string()), renew }
    }
}

impl Http {
    async fn request(&self, url: &str, range: Option<(u64, u64)>, got: &(dyn Fn(u64) + Send + Sync)) -> Result<Vec<u8>> {
        let host = crate::http::host(url);
        let secure = sends_token(url);
        let mut renewed = false;
        loop {
            let mut req = DL_CLIENT.get(url);
            if secure {
                let t = self.token.lock().clone();
                if !t.is_empty() {
                    req = req.bearer_auth(t);
                }
            }
            if let Some((start, len)) = range {
                req = req.header(reqwest::header::RANGE, format!("bytes={start}-{}", start + len.max(1) - 1));
            }
            // The address carries the CDN's token, so errors name the host only.
            let resp = req.send().await.map_err(|e| anyhow!("Could not reach {host}: {}", e.without_url()))?;
            let code = resp.status();
            if code.as_u16() == 401 && secure && !renewed {
                if let Some(fresh) = self.renew.as_ref().and_then(|r| r()) {
                    *self.token.lock() = fresh;
                    renewed = true;
                    continue;
                }
            }
            if !code.is_success() {
                return Err(anyhow::Error::new(HttpStatus(code.as_u16())).context(format!("{host} did not accept the request")));
            }
            // A server that ignores the range sends the whole file: not read.
            if let Some((_, len)) = range {
                if code.as_u16() != 206 && resp.content_length() != Some(len) {
                    bail!("{host} did not give the part of the file that was asked for");
                }
            }
            let mut out = Vec::with_capacity(resp.content_length().map(|l| l.min(64 << 20) as usize).unwrap_or(0));
            let mut body = resp.bytes_stream();
            while let Some(piece) = body.next().await {
                let piece = piece.map_err(|e| anyhow!("The download from {host} broke off: {}", e.without_url()))?;
                got(piece.len() as u64);
                if out.len() + piece.len() > MAX_BODY {
                    bail!("An answer from {host} is bigger than any GOG file");
                }
                out.extend_from_slice(&piece);
            }
            if let Some((_, len)) = range {
                if out.len() as u64 != len {
                    bail!("{host} gave a part of a file of the wrong size");
                }
            }
            return Ok(out);
        }
    }
}

impl Fetch for Http {
    async fn get(&self, url: &str, got: &(dyn Fn(u64) + Send + Sync)) -> Result<Vec<u8>> {
        self.request(url, None, got).await
    }

    async fn get_range(&self, url: &str, start: u64, len: u64, got: &(dyn Fn(u64) + Send + Sync)) -> Result<Vec<u8>> {
        self.request(url, Some((start, len)), got).await
    }
}

// ---------------------------------------------------------------------------------------------
// Small readers for JSON

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.trim().to_string(),
        Value::Number(n) => n.to_string(),
        _ => String::new(),
    }
}

fn num(v: &Value) -> Option<u64> {
    v.as_u64().or_else(|| v.as_str()?.trim().parse().ok())
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().map(text).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

fn is_hex(s: &str, len: usize) -> bool {
    s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn md5_hex(b: &[u8]) -> String {
    hex::encode(Md5::digest(b))
}

/// GOG's meta files are zlib compressed JSON; a plain JSON file is read as it is.
fn unpack_json(raw: &[u8]) -> Result<Value> {
    let plain = raw.iter().find(|b| !b.is_ascii_whitespace()).is_some_and(|b| *b == b'{' || *b == b'[');
    if plain {
        if let Ok(v) = serde_json::from_slice(raw) {
            return Ok(v);
        }
    }
    let inflated = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(raw, MAX_BODY).map_err(|e| anyhow!("A GOG manifest could not be unpacked ({e:?})"))?;
    serde_json::from_slice(&inflated).context("A GOG manifest is not valid JSON")
}

// ---------------------------------------------------------------------------------------------
// The builds list

/// One build in GOG's public list for a game.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildRef {
    pub id: String,
    pub version: String,
    /// None for the default branch
    pub branch: Option<String>,
    /// 1 or 2: the format of the build's manifests
    pub generation: u8,
    pub date: String,
    /// Where the build's manifest is
    pub link: String,
}

impl BuildRef {
    /// The version people know, else the build id.
    pub fn version_or_id(&self) -> String {
        if self.version.is_empty() {
            self.id.clone()
        } else {
            self.version.clone()
        }
    }
}

pub fn builds_url(product_id: u64, generation: u8) -> String {
    format!("https://{CONTENT_SYSTEM}/products/{product_id}/os/windows/builds?generation={generation}")
}

/// The public builds a list names (a build GOG marks private is left out).
pub fn parse_builds(body: &[u8]) -> Result<Vec<BuildRef>> {
    let v: Value = serde_json::from_slice(body).context("GOG's list of builds is not valid JSON")?;
    let items = v["items"].as_array().ok_or_else(|| anyhow!("GOG's list of builds has no items"))?;
    Ok(items
        .iter()
        .filter(|b| b["public"].as_bool() != Some(false))
        .filter_map(|b| {
            let id = text(&b["build_id"]);
            (!id.is_empty()).then(|| BuildRef {
                id,
                version: text(&b["version_name"]),
                branch: Some(text(&b["branch"])).filter(|s| !s.is_empty()),
                generation: num(&b["generation"]).unwrap_or(2) as u8,
                date: text(&b["date_published"]),
                link: text(&b["link"]),
            })
        })
        .collect())
}

/// Which build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick<'a> {
    /// The newest of the default branch
    Newest,
    /// The newest of a named branch
    Branch(&'a str),
    /// A build by its id
    Id(&'a str),
}

/// The build `want` names. A generation 2 build is taken over a generation 1 one.
pub fn pick_build<'a>(builds: &'a [BuildRef], want: Pick<'_>) -> Option<&'a BuildRef> {
    let newest = |gen: u8| {
        builds
            .iter()
            .filter(|b| b.generation == gen)
            .filter(|b| match want {
                Pick::Newest => b.branch.is_none(),
                Pick::Branch(name) => b.branch.as_deref() == Some(name),
                Pick::Id(id) => b.id == id,
            })
            .reduce(|best, b| if b.date > best.date { b } else { best })
    };
    newest(2).or_else(|| newest(1))
}

/// Every public build of the game (the generation 2 answer also holds generation 1 builds that
/// have no newer one; the generation 1 list is asked only when that one is empty).
async fn list_builds<F: Fetch>(f: &F, product_id: u64) -> Result<Vec<BuildRef>> {
    let mut builds = parse_builds(&f.get(&builds_url(product_id, 2), &|_| {}).await.context("Could not read GOG's list of builds")?)?;
    if builds.is_empty() {
        builds = parse_builds(&f.get(&builds_url(product_id, 1), &|_| {}).await.context("Could not read GOG's list of builds")?)?;
    }
    Ok(builds)
}

/// The newest build of the default branch GOG lists for the game: (build id, version). The version
/// is the build id when GOG names none.
pub async fn latest_build(product_id: u64) -> Result<(String, String)> {
    let builds = list_builds(&Http::public(), product_id).await?;
    let b = pick_build(&builds, Pick::Newest).ok_or_else(|| anyhow!("GOG lists no Windows build of this game"))?;
    Ok((b.id.clone(), b.version_or_id()))
}

// ---------------------------------------------------------------------------------------------
// Build and depot manifests

/// One piece of a file (or of a small files container).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    /// md5 of the unpacked bytes
    pub md5: String,
    /// md5 of the packed bytes; also the chunk's name on the CDN
    pub compressed_md5: String,
    pub size: u64,
    pub compressed_size: u64,
}

/// A file of a depot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DepotFile {
    pub path: String,
    pub size: u64,
    pub chunks: Vec<Chunk>,
    pub md5: Option<String>,
    pub sha256: Option<String>,
    /// "executable", "support"
    pub flags: Vec<String>,
    /// (offset, size) in the depot's small files container, when the file is packed in it
    pub sfc: Option<(u64, u64)>,
    /// Where the file is in a generation 1 depot's `main.bin`
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub v1: Option<V1Ref>,
}

/// A generation 1 file: a slice of a product's `main.bin`, as long as the file.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct V1Ref {
    /// The product whose `main.bin` it is in
    pub product: String,
    pub offset: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepotLink {
    pub path: String,
    pub target: String,
}

/// What a depot's manifest lists.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DepotManifest {
    pub files: Vec<DepotFile>,
    pub dirs: Vec<String>,
    pub links: Vec<DepotLink>,
    /// The chunks of the small files container, in order
    pub sfc: Vec<Chunk>,
}

/// A depot a build names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Depot {
    /// The depot manifest's hash
    pub manifest: String,
    pub product_id: String,
    pub languages: Vec<String>,
    pub size: u64,
    pub compressed_size: u64,
    /// "32", "64"; empty for any
    pub bitness: Vec<String>,
    /// GOG's own small depots (the game's `.info` and hash files)
    pub gog_depot: bool,
}

/// How a product is started when the game's `.info` file does not say.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductTask {
    pub product_id: String,
    pub exe: String,
    pub args: String,
}

/// A generation 2 build manifest, without the client keys it also carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildManifest {
    pub build_id: String,
    pub product_id: String,
    pub install_dir: String,
    /// Ids of the redistributables the game needs (VC++, DirectX, ...)
    pub dependencies: Vec<String>,
    pub depots: Vec<Depot>,
    pub tasks: Vec<ProductTask>,
}

pub fn parse_build_manifest(raw: &[u8]) -> Result<BuildManifest> {
    let v = unpack_json(raw)?;
    if let Some(ver) = v.get("version").and_then(num) {
        if ver != 2 {
            bail!("This build's manifest is version {ver}, which is not known");
        }
    }
    let product_id = text(&v["baseProductId"]);
    if product_id.is_empty() {
        bail!("The build's manifest names no product");
    }
    let mut depots = Vec::new();
    for d in v["depots"].as_array().ok_or_else(|| anyhow!("The build's manifest lists no depots"))? {
        let manifest = text(&d["manifest"]).to_lowercase();
        if !is_hex(&manifest, 32) {
            bail!("A depot of the build has a manifest name that is not a hash");
        }
        let pid = text(&d["productId"]);
        depots.push(Depot {
            manifest,
            product_id: if pid.is_empty() { product_id.clone() } else { pid },
            languages: strings(&d["languages"]),
            size: num(&d["size"]).unwrap_or(0),
            compressed_size: num(&d["compressedSize"]).unwrap_or(0),
            bitness: strings(&d["osBitness"]),
            gog_depot: d["isGogDepot"].as_bool().unwrap_or(false),
        });
    }
    let tasks = v["products"]
        .as_array()
        .map(|a| a.iter().map(|p| ProductTask { product_id: text(&p["productId"]), exe: text(&p["temp_executable"]), args: text(&p["temp_arguments"]) }).collect())
        .unwrap_or_default();
    Ok(BuildManifest { build_id: text(&v["buildId"]), product_id, install_dir: text(&v["installDirectory"]), dependencies: strings(&v["dependencies"]), depots, tasks })
}

fn parse_chunk(c: &Value) -> Result<Chunk> {
    let md5 = text(&c["md5"]).to_lowercase();
    let compressed_md5 = text(&c["compressedMd5"]).to_lowercase();
    let size = num(&c["size"]).ok_or_else(|| anyhow!("A chunk of the build has no size"))?;
    let compressed_size = num(&c["compressedSize"]).unwrap_or(0);
    if !is_hex(&md5, 32) || !is_hex(&compressed_md5, 32) {
        bail!("A chunk of the build has no checksum");
    }
    if size > MAX_CHUNK || compressed_size > MAX_CHUNK {
        bail!("A chunk of the build is bigger than any chunk GOG makes");
    }
    Ok(Chunk { md5, compressed_md5, size, compressed_size })
}

fn parse_hash(v: &Value, len: usize) -> Result<Option<String>> {
    let s = text(v).to_lowercase();
    if s.is_empty() {
        return Ok(None);
    }
    if !is_hex(&s, len) {
        bail!("A file of the build has a checksum that cannot be read");
    }
    Ok(Some(s))
}

/// A depot's manifest: files (with their chunks), directories, links and the small files
/// container. An item of a kind that is not known refuses the whole depot.
pub fn parse_depot_manifest(raw: &[u8]) -> Result<DepotManifest> {
    let v = unpack_json(raw)?;
    let depot = v.get("depot").ok_or_else(|| anyhow!("A depot's manifest has no depot"))?;
    let mut out = DepotManifest::default();
    if let Some(chunks) = depot["smallFilesContainer"]["chunks"].as_array() {
        out.sfc = chunks.iter().map(parse_chunk).collect::<Result<_>>()?;
    }
    for it in depot["items"].as_array().ok_or_else(|| anyhow!("A depot's manifest lists no items"))? {
        let path = text(&it["path"]);
        match it["type"].as_str().unwrap_or("") {
            "DepotFile" => {
                if path.is_empty() {
                    bail!("A file of the build has no path");
                }
                let chunks: Vec<Chunk> = it["chunks"].as_array().map(|a| a.iter().map(parse_chunk).collect::<Result<_>>()).transpose()?.unwrap_or_default();
                let sfc = match it.get("sfcRef") {
                    Some(r) if !r.is_null() => Some((num(&r["offset"]).ok_or_else(|| anyhow!("A packed file has no offset"))?, num(&r["size"]).ok_or_else(|| anyhow!("A packed file has no size"))?)),
                    _ => None,
                };
                let sum: u64 = chunks.iter().map(|c| c.size).sum();
                let size = match sfc {
                    Some((_, s)) => {
                        if !chunks.is_empty() && sum != s {
                            bail!("A packed file's size does not match its chunk");
                        }
                        s
                    }
                    None => sum,
                };
                out.files.push(DepotFile { path, size, chunks, md5: parse_hash(&it["md5"], 32)?, sha256: parse_hash(&it["sha256"], 64)?, flags: strings(&it["flags"]), sfc, v1: None });
            }
            "DepotDirectory" => out.dirs.push(path),
            "DepotLink" => {
                let target = text(&it["target"]);
                if path.is_empty() || target.is_empty() {
                    bail!("A link of the build has no path or target");
                }
                out.links.push(DepotLink { path, target });
            }
            other => bail!("The build has an item of a kind that is not known ({other})"),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Generation 1

/// A depot of a generation 1 build's `repository.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V1Depot {
    /// The depot manifest's file name
    pub manifest: String,
    pub languages: Vec<String>,
    pub size: u64,
    /// The products whose files the depot holds
    pub game_ids: Vec<String>,
    /// "Windows", "Mac", "Linux"; empty for any
    pub systems: Vec<String>,
}

/// A generation 1 build's `repository.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V1Repo {
    pub depots: Vec<V1Depot>,
    /// Ids of the redistributables the game needs
    pub redists: Vec<String>,
    pub install_dir: String,
    /// The build's number in the download addresses
    pub timestamp: String,
}

/// A file of a generation 1 depot: a slice of a product's `main.bin`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct V1File {
    pub path: String,
    pub size: u64,
    pub offset: u64,
    /// md5 of the file
    pub hash: Option<String>,
    /// "<product id>/main.bin"
    pub url: String,
    pub support: bool,
    pub executable: bool,
    pub directory: bool,
}

/// The depots, the redistributable ids and the install folder name of a `repository.json`.
pub fn parse_v1_repository(raw: &[u8]) -> Result<V1Repo> {
    let v = unpack_json(raw)?;
    let p = v.get("product").ok_or_else(|| anyhow!("A generation 1 repository has no product"))?;
    let mut depots = Vec::new();
    let mut redists = Vec::new();
    for d in p["depots"].as_array().ok_or_else(|| anyhow!("A generation 1 repository has no depots"))? {
        if let Some(r) = d["redist"].as_str() {
            redists.push(r.to_string());
        } else if !text(&d["manifest"]).is_empty() {
            depots.push(V1Depot { manifest: text(&d["manifest"]), languages: strings(&d["languages"]), size: num(&d["size"]).unwrap_or(0), game_ids: strings(&d["gameIDs"]), systems: strings(&d["systems"]) });
        }
    }
    let timestamp = text(&p["timestamp"]);
    if timestamp.is_empty() || !timestamp.bytes().all(|b| b.is_ascii_digit()) {
        bail!("A generation 1 repository has no build number");
    }
    Ok(V1Repo { depots, redists, install_dir: text(&p["installDirectory"]), timestamp })
}

/// The files of a generation 1 depot manifest.
pub fn parse_v1_depot(raw: &[u8]) -> Result<Vec<V1File>> {
    let v = unpack_json(raw)?;
    let files = v["depot"]["files"].as_array().ok_or_else(|| anyhow!("A generation 1 depot lists no files"))?;
    files
        .iter()
        .map(|f| {
            let path = text(&f["path"]);
            if path.is_empty() {
                bail!("A file of the build has no path");
            }
            Ok(V1File {
                path,
                size: num(&f["size"]).unwrap_or(0),
                offset: num(&f["offset"]).unwrap_or(0),
                hash: parse_hash(&f["hash"], 32)?,
                url: text(&f["url"]),
                support: f["support"].as_bool().unwrap_or(false),
                executable: f["executable"].as_bool().unwrap_or(false),
                directory: f["directory"].as_bool().unwrap_or(false),
            })
        })
        .collect()
}

/// A generation 1 depot's files in the form the rest of the engine works with. `product` is the
/// product the depot belongs to, for a file whose address names none.
fn v1_manifest(files: Vec<V1File>, product: &str) -> Result<DepotManifest> {
    let mut out = DepotManifest::default();
    for f in files {
        if f.directory {
            out.dirs.push(f.path);
            continue;
        }
        if f.size > 0 && f.hash.is_none() {
            bail!("A file of the build has no checksum to check it by");
        }
        let owner = f.url.split('/').next().filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit())).unwrap_or(product).to_string();
        let mut flags = Vec::new();
        if f.support {
            flags.push("support".to_string());
        }
        if f.executable {
            flags.push("executable".to_string());
        }
        out.files.push(DepotFile { path: f.path, size: f.size, chunks: vec![], md5: f.hash, sha256: None, flags, sfc: None, v1: Some(V1Ref { product: owner, offset: f.offset }) });
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// Languages and depots

const LANG_NAMES: &[(&str, &str)] = &[
    ("english", "en"),
    ("german", "de"),
    ("deutsch", "de"),
    ("french", "fr"),
    ("spanish", "es"),
    ("italian", "it"),
    ("polish", "pl"),
    ("russian", "ru"),
    ("portuguese", "pt"),
    ("brazilian portuguese", "pt-br"),
    ("japanese", "ja"),
    ("korean", "ko"),
    ("chinese", "zh"),
    ("simplified chinese", "zh-hans"),
    ("traditional chinese", "zh-hant"),
    ("czech", "cs"),
    ("swedish", "sv"),
    ("turkish", "tr"),
    ("dutch", "nl"),
    ("hungarian", "hu"),
    ("arabic", "ar"),
    ("ukrainian", "uk"),
    ("finnish", "fi"),
    ("norwegian", "no"),
    ("danish", "da"),
    ("greek", "el"),
    ("romanian", "ro"),
    ("bulgarian", "bg"),
    ("thai", "th"),
];

/// A language written the way a locale code is: "en-US" and "English" and "en_us" all come out
/// as "en-us" or "en". "*" and "Neutral" (a depot for every language) come out as "*".
fn lang_norm(s: &str) -> String {
    let l = s.trim().to_lowercase().replace('_', "-");
    if l == "*" || l == "neutral" {
        return "*".into();
    }
    LANG_NAMES.iter().find(|(n, _)| *n == l).map(|(_, c)| c.to_string()).unwrap_or(l)
}

fn lang_primary(l: &str) -> &str {
    l.split('-').next().unwrap_or(l)
}

/// The language of the build to install for the wanted one, among the languages its depots have:
/// the same locale, else the same language, else English, else the first the build has. None when
/// the depots have no language of their own.
pub fn pick_language(available: &[String], wanted: &str) -> Option<String> {
    let avail: Vec<String> = available.iter().map(|l| lang_norm(l)).filter(|l| l != "*" && !l.is_empty()).collect();
    let w = lang_norm(wanted);
    let by_exact = |x: &str| avail.iter().find(|a| *a == x).cloned();
    let by_primary = |x: &str| avail.iter().find(|a| lang_primary(a) == lang_primary(x)).cloned();
    if w != "*" && !w.is_empty() {
        if let Some(l) = by_exact(&w).or_else(|| by_primary(&w)) {
            return Some(l);
        }
    }
    by_exact("en-us").or_else(|| by_primary("en")).or_else(|| avail.first().cloned())
}

fn depot_in_language(languages: &[String], chosen: Option<&str>) -> bool {
    let langs: Vec<String> = languages.iter().map(|l| lang_norm(l)).collect();
    langs.is_empty() || langs.iter().any(|l| l == "*") || chosen.is_some_and(|c| langs.iter().any(|l| l == c))
}

/// The depots to install: for the products allowed, 64 bit ones (32 bit ones only when a product
/// has nothing else), and those for the language (and the ones for every language). Returns the
/// language chosen.
pub fn pick_depots<'a>(m: &'a BuildManifest, wanted_language: &str, products: &HashSet<String>) -> (Vec<&'a Depot>, Option<String>) {
    let mut cand: Vec<&Depot> = m.depots.iter().filter(|d| products.contains(&d.product_id)).collect();
    let any64 = |d: &Depot| d.bitness.is_empty() || d.bitness.iter().any(|b| b == "64");
    let with64: HashSet<String> = cand.iter().filter(|d| any64(d)).map(|d| d.product_id.clone()).collect();
    cand.retain(|d| !with64.contains(&d.product_id) || any64(d));
    let available: Vec<String> = cand.iter().flat_map(|d| d.languages.iter().cloned()).collect();
    let chosen = pick_language(&available, wanted_language);
    cand.retain(|d| depot_in_language(&d.languages, chosen.as_deref()));
    (cand, chosen)
}

// ---------------------------------------------------------------------------------------------
// Download addresses

/// One CDN GOG offers for a product's chunks.
#[derive(Clone, Debug)]
struct Endpoint {
    url_format: String,
    params: Vec<(String, String)>,
    max_age: u64,
}

/// The CDNs in `secure_link`'s answer, in the order GOG lists them (the first is the preferred).
fn parse_secure_link(body: &[u8]) -> Result<Vec<Endpoint>> {
    let v: Value = serde_json::from_slice(body).context("GOG's download addresses are not valid JSON")?;
    let eps: Vec<Endpoint> = v["urls"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|u| {
                    let url_format = u["url_format"].as_str()?.to_string();
                    let params = u["parameters"].as_object()?.iter().map(|(k, v)| (k.clone(), text(v))).collect();
                    Some(Endpoint { url_format, params, max_age: num(&u["max_age"]).unwrap_or(0) })
                })
                .collect()
        })
        .unwrap_or_default();
    if eps.is_empty() {
        bail!("GOG gave no download addresses");
    }
    Ok(eps)
}

/// Collapses runs of slashes in the path of an address (not after "https:").
fn tidy_url(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else { return url.to_string() };
    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };
    let (host, tail) = match path.find('/') {
        Some(i) => path.split_at(i),
        None => (path, ""),
    };
    let mut tail = tail.to_string();
    while tail.contains("//") {
        tail = tail.replace("//", "/");
    }
    match query {
        Some(q) => format!("{scheme}://{host}{tail}?{q}"),
        None => format!("{scheme}://{host}{tail}"),
    }
}

/// Where a file is on a CDN: the endpoint's address template with `tail` (a chunk's place, or
/// "main.bin") after the path it names.
fn chunk_url(ep: &Endpoint, tail: &str) -> Result<String> {
    let base = ep.params.iter().find(|(k, _)| k == "path").map(|(_, v)| v.trim_end_matches('/').to_string()).unwrap_or_default();
    let mut params: Vec<(String, String)> = ep.params.iter().filter(|(k, _)| k != "path").cloned().collect();
    params.push(("path".into(), format!("{base}/{tail}")));
    let mut url = ep.url_format.clone();
    // Every name in braces must be one the endpoint gave.
    let mut rest = ep.url_format.as_str();
    while let Some(i) = rest.find('{') {
        let j = rest[i..].find('}').ok_or_else(|| anyhow!("GOG's download address is not understood"))? + i;
        if !params.iter().any(|(k, _)| k == &rest[i + 1..j]) {
            bail!("GOG's download address is not understood");
        }
        rest = &rest[j + 1..];
    }
    for (k, v) in &params {
        url = url.replace(&format!("{{{k}}}"), v);
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        bail!("GOG's download address is not understood");
    }
    Ok(tidy_url(&url))
}

/// A chunk's place on the CDN, from the md5 of its packed bytes.
fn galaxy_path(compressed_md5: &str) -> Result<String> {
    if !is_hex(compressed_md5, 32) {
        bail!("A chunk has no usable name");
    }
    let m = compressed_md5.to_lowercase();
    Ok(format!("{}/{}/{}", &m[0..2], &m[2..4], m))
}

#[derive(Default)]
struct Links {
    eps: Vec<Endpoint>,
    at: Option<Instant>,
    /// Counts the times the addresses were asked for again
    generation: u64,
}

/// A product's download addresses, asked for when first needed and again when they run out.
struct Cdn {
    product: String,
    /// What to ask for the addresses
    ask_url: String,
    links: parking_lot::RwLock<Links>,
    renew: tokio::sync::Mutex<()>,
}

impl Cdn {
    /// For a generation 2 build.
    fn new(product: &str) -> Cdn {
        Cdn::with_url(product, format!("https://{CONTENT_SYSTEM}/products/{product}/secure_link?_version=2&generation=2&path=/"))
    }

    /// For a generation 1 build, named by its number.
    fn v1(product: &str, timestamp: &str) -> Cdn {
        Cdn::with_url(product, format!("https://{CONTENT_SYSTEM}/products/{product}/secure_link?_version=2&type=depot&path=/windows/{timestamp}/"))
    }

    /// For the generation of a saved build.
    fn for_saved(product: &str, saved: &Saved) -> Cdn {
        if saved.generation == 1 {
            Cdn::v1(product, &saved.timestamp)
        } else {
            Cdn::new(product)
        }
    }

    fn with_url(product: &str, ask_url: String) -> Cdn {
        Cdn { product: product.to_string(), ask_url, links: parking_lot::RwLock::new(Links::default()), renew: tokio::sync::Mutex::new(()) }
    }

    async fn ask<F: Fetch>(&self, f: &F) -> Result<Vec<Endpoint>> {
        match f.get(&self.ask_url, &|_| {}).await {
            Ok(body) => parse_secure_link(&body),
            Err(e) if matches!(status_of(&e), Some(401 | 403)) => Err(anyhow::Error::new(Denied(self.product.clone()))),
            Err(e) => Err(e.context("Could not get GOG's download addresses")),
        }
    }

    /// Makes sure the addresses are there (this is also how it is found out that the account may
    /// download the product).
    async fn ensure<F: Fetch>(&self, f: &F) -> Result<()> {
        if self.links.read().at.is_some() {
            return Ok(());
        }
        let _one = self.renew.lock().await;
        if self.links.read().at.is_some() {
            return Ok(());
        }
        let eps = self.ask(f).await?;
        *self.links.write() = Links { eps, at: Some(Instant::now()), generation: 1 };
        Ok(())
    }

    /// Asks again, unless another download did already since `seen`.
    async fn refresh<F: Fetch>(&self, f: &F, seen: u64) -> Result<()> {
        let _one = self.renew.lock().await;
        if self.links.read().generation != seen {
            return Ok(());
        }
        let eps = self.ask(f).await?;
        let mut l = self.links.write();
        l.eps = eps;
        l.at = Some(Instant::now());
        l.generation += 1;
        Ok(())
    }

    /// The address of a chunk (or of "main.bin") on the CDN `attempt` selects (they are tried in
    /// turn), and the generation of the addresses it came from.
    async fn url<F: Fetch>(&self, f: &F, tail: &str, attempt: usize) -> Result<(String, u64)> {
        self.ensure(f).await?;
        let (stale, seen) = {
            let l = self.links.read();
            let max_age = l.eps.first().map(|e| e.max_age).unwrap_or(0);
            let life = if max_age > 0 { Duration::from_secs((max_age * 4 / 5).clamp(30, 2700)) } else { Duration::from_secs(1200) };
            (l.at.is_some_and(|t| t.elapsed() > life), l.generation)
        };
        if stale {
            self.refresh(f, seen).await?;
        }
        let l = self.links.read();
        let ep = &l.eps[attempt % l.eps.len()];
        Ok((chunk_url(ep, tail)?, l.generation))
    }
}

// ---------------------------------------------------------------------------------------------
// What is installed: the saved manifests

/// The depot manifests of the build a folder has, kept in `.u3d` to know what an update changes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Saved {
    pub format: u32,
    /// 1 or 2: the format of the build's manifests
    #[serde(default = "two")]
    pub generation: u8,
    /// A generation 1 build's number in the download addresses
    #[serde(default)]
    pub timestamp: String,
    pub build_id: String,
    pub version: String,
    pub product_id: String,
    /// The language the depots were chosen for
    pub language: String,
    pub dependencies: Vec<String>,
    pub depots: Vec<SavedDepot>,
    pub tasks: Vec<ProductTask>,
}

fn two() -> u8 {
    2
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedDepot {
    pub product_id: String,
    pub manifest: String,
    #[serde(flatten)]
    pub data: DepotManifest,
}

fn own_file(dest: &Path, name: &str) -> PathBuf {
    dest.join(sync::OWN_DIR).join(name)
}

fn read_saved(p: &Path) -> Option<Saved> {
    serde_json::from_slice::<Saved>(&std::fs::read(p).ok()?).ok().filter(|s| s.format == 1)
}

/// The saved manifests of the build the folder's state names (the one before the last update is
/// kept too, for when that update was put back).
fn load_saved(dest: &Path, state: Option<&State>) -> Option<Saved> {
    let build = &state?.build;
    [MANIFEST_FILE, PREVIOUS_FILE].iter().filter_map(|n| read_saved(&own_file(dest, n))).find(|s| &s.build_id == build)
}

fn save_manifest(dest: &Path, s: &Saved) -> Result<()> {
    let current = own_file(dest, MANIFEST_FILE);
    if read_saved(&current).is_some_and(|c| c.build_id != s.build_id) {
        let _ = std::fs::remove_file(own_file(dest, PREVIOUS_FILE));
        std::fs::rename(&current, own_file(dest, PREVIOUS_FILE)).context("Could not keep the manifest of the build before")?;
    }
    sync::write_file(dest, &Path::new(sync::OWN_DIR).join(MANIFEST_FILE), &serde_json::to_vec(s)?)
}

/// Ids of the redistributables (VC++, DirectX, ...) the installed build lists. They are not
/// installed; the caller can tell the user.
pub fn dependencies(dest: &Path) -> Vec<String> {
    read_saved(&own_file(dest, MANIFEST_FILE)).map(|s| s.dependencies).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------
// The files of a build

/// A file of the build to put in the game folder.
#[derive(Clone)]
struct Entry {
    rel: PathBuf,
    size: u64,
    product: String,
    /// Index in the depot list the entry came from
    depot: usize,
    file: Arc<DepotFile>,
}

struct Merged {
    entries: Vec<Entry>,
    dirs: Vec<PathBuf>,
}

fn path_key(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// The path of a build's file in the game folder. A leading slash is dropped (GOG writes some
/// paths that way); support files go to their own folder. Refused when it is anything but a plain
/// path below the game folder.
fn rel_of(path: &str, flags: &[String], product: &str) -> Result<PathBuf> {
    let p = path.trim().trim_start_matches(['/', '\\']);
    let full = if flags.iter().any(|f| f == "support") { format!("gog-support\\{product}\\{p}") } else { p.to_string() };
    sync::safe_rel(&full).ok_or_else(|| anyhow!("The build names a file path that is not allowed: {path}"))
}

/// The files of the depots as one list: a path in two depots is one file (the later depot wins,
/// the case of a name does not matter). Every path is checked; one that is not safe refuses the
/// build.
fn merge(depots: &[SavedDepot]) -> Result<Merged> {
    let mut entries: Vec<Entry> = Vec::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut seen_dirs: HashSet<String> = HashSet::new();
    for (di, d) in depots.iter().enumerate() {
        for f in &d.data.files {
            let rel = rel_of(&f.path, &f.flags, &d.product_id)?;
            // A generation 1 file belongs to the product whose main.bin it is in.
            let product = f.v1.as_ref().map(|v| v.product.clone()).unwrap_or_else(|| d.product_id.clone());
            let e = Entry { rel: rel.clone(), size: f.size, product, depot: di, file: Arc::new(f.clone()) };
            match at.get(&path_key(&rel)) {
                Some(&i) => entries[i] = e,
                None => {
                    at.insert(path_key(&rel), entries.len());
                    entries.push(e);
                }
            }
        }
        for dir in &d.data.dirs {
            let t = dir.trim().trim_matches(['/', '\\']);
            if t.is_empty() {
                continue;
            }
            let rel = sync::safe_rel(t).ok_or_else(|| anyhow!("The build names a folder path that is not allowed: {dir}"))?;
            if seen_dirs.insert(path_key(&rel)) {
                dirs.push(rel);
            }
        }
        for l in &d.data.links {
            rel_of(&l.path, &[], &d.product_id)?;
        }
    }
    Ok(Merged { entries, dirs })
}

/// Is this the same content? (Same chunks; a file hash only counts when both sides have one.)
fn same_content(a: &DepotFile, b: &DepotFile) -> bool {
    // A file with no chunks (generation 1) is known by its md5 alone.
    if a.chunks.is_empty() && a.size > 0 && (a.md5.is_none() || a.md5 != b.md5) {
        return false;
    }
    a.size == b.size
        && a.chunks.len() == b.chunks.len()
        && a.chunks.iter().zip(&b.chunks).all(|(x, y)| x.md5 == y.md5 && x.size == y.size)
        && (a.md5.is_none() || b.md5.is_none() || a.md5 == b.md5)
        && (a.sha256.is_none() || b.sha256.is_none() || a.sha256 == b.sha256)
}

/// What an update changes, from the saved manifest of the installed build: the files (indices in
/// `new`) that are new, different or missing, and the files the new build dropped.
fn diff_update(dest: &Path, new: &[Entry], old: &[Entry]) -> (Vec<usize>, Vec<PathBuf>) {
    let old_by: HashMap<String, &Entry> = old.iter().map(|e| (path_key(&e.rel), e)).collect();
    let todo = new
        .iter()
        .enumerate()
        .filter(|(_, e)| {
            let unchanged = old_by.get(&path_key(&e.rel)).is_some_and(|o| same_content(&o.file, &e.file));
            let on_disk = std::fs::metadata(dest.join(&e.rel)).ok().filter(|m| m.is_file()).map(|m| m.len());
            !(unchanged && on_disk == Some(e.size))
        })
        .map(|(i, _)| i)
        .collect();
    let kept: HashSet<String> = new.iter().map(|e| path_key(&e.rel)).collect();
    let removals = old.iter().filter(|e| !kept.contains(&path_key(&e.rel))).map(|e| e.rel.clone()).collect();
    (todo, removals)
}

// ---------------------------------------------------------------------------------------------
// Checking files on disk

/// Reads `left` bytes of `f` into `h`; false when the file is shorter or the sync was stopped.
fn feed(f: &mut std::fs::File, mut left: u64, h: &mut Md5, buf: &mut [u8], stop: &AtomicBool, counted: &AtomicU64) -> bool {
    while left > 0 {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        let want = left.min(buf.len() as u64) as usize;
        match f.read(&mut buf[..want]) {
            Ok(0) | Err(_) => return false,
            Ok(n) => {
                h.update(&buf[..n]);
                left -= n as u64;
                counted.fetch_add(n as u64, Ordering::Relaxed);
            }
        }
    }
    true
}

/// Does the file at `path` hold exactly what the build says: its chunks, else (a generation 1
/// file) its md5?
fn file_matches(path: &Path, size: u64, chunks: &[Chunk], md5: Option<&str>, stop: &AtomicBool, counted: &AtomicU64) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    if f.metadata().ok().map(|m| m.len()) != Some(size) {
        return false;
    }
    let mut buf = vec![0u8; 1 << 20];
    if chunks.iter().map(|c| c.size).sum::<u64>() != size {
        let Some(want) = md5 else { return true };
        let mut h = Md5::new();
        return feed(&mut f, size, &mut h, &mut buf, stop, counted) && hex::encode(h.finalize()) == want;
    }
    for c in chunks {
        let mut h = Md5::new();
        if !feed(&mut f, c.size, &mut h, &mut buf, stop, counted) || hex::encode(h.finalize()) != c.md5 {
            return false;
        }
    }
    true
}

/// Checks a file that was just written: its size, and its md5 and sha256 when the build gives them.
fn verify_part(path: &Path, size: u64, md5: Option<&str>, sha256: Option<&str>) -> Result<()> {
    if std::fs::metadata(path)?.len() != size {
        bail!("it has the wrong size");
    }
    if md5.is_none() && sha256.is_none() {
        return Ok(());
    }
    let mut f = std::fs::File::open(path)?;
    let (mut hm, mut hs) = (Md5::new(), Sha256::new());
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if md5.is_some() {
            hm.update(&buf[..n]);
        }
        if sha256.is_some() {
            hs.update(&buf[..n]);
        }
    }
    if md5.is_some_and(|m| hex::encode(hm.finalize()) != m) || sha256.is_some_and(|s| hex::encode(hs.finalize()) != s) {
        bail!("it does not match the checksum of the build");
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Downloading

#[derive(Clone)]
struct Cfg {
    /// Chunks downloaded at once
    fetchers: usize,
    /// Files checked at once
    hashers: usize,
    tries: usize,
    wait: Duration,
    /// How often cancel is looked at
    tick: Duration,
    /// How often progress is reported
    report: Duration,
    /// Most bytes of a generation 1 file asked for at once
    piece: u64,
    /// (free, total) instead of asking the drive (tests)
    space: Option<(u64, u64)>,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { fetchers: 8, hashers: 3, tries: 5, wait: Duration::from_secs(2), tick: Duration::from_millis(250), report: Duration::from_secs(1), piece: V1_PIECE, space: None }
    }
}

fn canceled() -> anyhow::Error {
    anyhow!("CANCELED|Canceled")
}

fn emit(p: &(dyn Fn(Progress) + Send + Sync), step: &str, done: u64, total: u64, files_done: usize, files_total: usize) {
    p(Progress { done_bytes: done.min(total), total_bytes: total, files_done, files_total, step: step.to_string() });
}

/// A file being made: its bytes arrive piece by piece, in any order, into a part file.
struct Slot {
    rel: PathBuf,
    part: PathBuf,
    size: u64,
    /// Bytes still to arrive
    left: AtomicU64,
    file: parking_lot::Mutex<Option<Arc<std::fs::File>>>,
    md5: Option<String>,
    sha256: Option<String>,
}

impl Slot {
    fn handle(&self) -> std::io::Result<Arc<std::fs::File>> {
        let mut g = self.file.lock();
        if let Some(f) = g.as_ref() {
            return Ok(f.clone());
        }
        if let Some(dir) = self.part.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let f = std::fs::OpenOptions::new().create(true).truncate(true).write(true).open(&self.part)?;
        f.set_len(self.size)?;
        let f = Arc::new(f);
        *g = Some(f.clone());
        Ok(f)
    }

    /// All bytes have been written: close the file and check it.
    fn finish(&self) -> Result<()> {
        drop(self.file.lock().take());
        verify_part(&self.part, self.size, self.md5.as_deref(), self.sha256.as_deref()).with_context(|| format!("{} did not come out right", self.rel.display()))
    }
}

fn write_at(f: &std::fs::File, mut buf: &[u8], mut offset: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !buf.is_empty() {
        let n = f.seek_write(buf, offset)?;
        if n == 0 {
            return Err(std::io::ErrorKind::WriteZero.into());
        }
        buf = &buf[n..];
        offset += n as u64;
    }
    Ok(())
}

/// Where some of a chunk's unpacked bytes go.
#[derive(Clone, Copy)]
struct Place {
    slot: usize,
    src: u64,
    len: u64,
    dst: u64,
}

/// Where a piece of a file comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    /// A packed chunk on the CDN
    Chunk(Chunk),
    /// A range of a product's `main.bin` (generation 1), as it is
    Range { offset: u64, len: u64 },
}

/// A piece to download, and the files its bytes go to (several, for a container chunk or for
/// content that two files share).
struct Unit {
    product: String,
    source: Source,
    /// Bytes to download
    packed: u64,
    places: Vec<Place>,
}

/// The pieces to download for `todo`, each once, in the order the files come. A generation 1 file
/// is asked for in pieces of at most `piece` bytes.
fn plan_units(todo: &[&Entry], depots: &[SavedDepot], piece: u64) -> Result<Vec<Unit>> {
    let mut units: Vec<Unit> = Vec::new();
    let mut at: HashMap<(String, String), usize> = HashMap::new();
    let mut add = |product: &str, source: Source, packed: u64, p: Place| -> Result<()> {
        let name = match &source {
            Source::Chunk(c) => c.compressed_md5.clone(),
            Source::Range { offset, len } => format!("{offset}+{len}"),
        };
        match at.get(&(product.to_string(), name.clone())) {
            Some(&i) => {
                let alike = match (&units[i].source, &source) {
                    (Source::Chunk(a), Source::Chunk(b)) => a.size == b.size && a.md5 == b.md5,
                    (a, b) => a == b,
                };
                if !alike {
                    bail!("The build names two different pieces with the same name");
                }
                units[i].places.push(p);
            }
            None => {
                at.insert((product.to_string(), name), units.len());
                units.push(Unit { product: product.to_string(), source, packed, places: vec![p] });
            }
        }
        Ok(())
    };
    let mut slot = 0;
    for e in todo {
        if e.size == 0 {
            continue;
        }
        let f = &e.file;
        if let Some((offset, size)) = f.sfc {
            let container = &depots[e.depot].data.sfc;
            let mut start = 0u64;
            let mut covered = 0u64;
            for c in container {
                let (lo, hi) = (start.max(offset), (start + c.size).min(offset + size));
                if lo < hi {
                    add(&e.product, Source::Chunk(c.clone()), c.compressed_size, Place { slot, src: lo - start, len: hi - lo, dst: lo - offset })?;
                    covered += hi - lo;
                }
                start += c.size;
            }
            if covered != size {
                bail!("A packed file lies outside its container");
            }
        } else if let Some(v) = &f.v1 {
            let mut at_byte = 0;
            while at_byte < f.size {
                let len = (f.size - at_byte).min(piece.max(1));
                add(&e.product, Source::Range { offset: v.offset + at_byte, len }, len, Place { slot, src: 0, len, dst: at_byte })?;
                at_byte += len;
            }
        } else {
            let mut dst = 0;
            for c in &f.chunks {
                add(&e.product, Source::Chunk(c.clone()), c.compressed_size, Place { slot, src: 0, len: c.size, dst })?;
                dst += c.size;
            }
        }
        slot += 1;
    }
    Ok(units)
}

/// Checks and unpacks a downloaded chunk.
fn unpack(bytes: &[u8], c: &Chunk) -> Result<Vec<u8>> {
    if md5_hex(bytes) != c.compressed_md5 {
        bail!("A piece of the game did not match its checksum");
    }
    let plain = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(bytes, c.size as usize).map_err(|e| anyhow!("A piece of the game could not be unpacked ({e:?})"))?;
    if plain.len() as u64 != c.size || md5_hex(&plain) != c.md5 {
        bail!("A piece of the game did not match its checksum once unpacked");
    }
    Ok(plain)
}

/// Checks a downloaded piece: a chunk is unpacked, a range of a generation 1 file is as long as
/// asked (the file's md5 is checked when it is whole).
fn check_piece(bytes: Vec<u8>, source: &Source) -> Result<Vec<u8>> {
    match source {
        Source::Chunk(c) => unpack(&bytes, c),
        Source::Range { len, .. } if bytes.len() as u64 == *len => Ok(bytes),
        Source::Range { .. } => bail!("A piece of the game came in the wrong size"),
    }
}

/// Writes an unpacked chunk where it goes, and checks every file it completes. How many files
/// were completed.
fn place_all(places: Vec<(Arc<Slot>, Place)>, plain: &[u8]) -> Result<usize> {
    let mut finished = 0;
    for (slot, p) in places {
        let f = slot.handle().with_context(|| format!("Could not write {}", slot.rel.display()))?;
        write_at(&f, &plain[p.src as usize..(p.src + p.len) as usize], p.dst).with_context(|| format!("Could not write {}", slot.rel.display()))?;
        drop(f);
        let before = slot.left.fetch_sub(p.len, Ordering::SeqCst);
        if before < p.len {
            bail!("More of a file arrived than it holds");
        }
        if before == p.len {
            slot.finish()?;
            finished += 1;
        }
    }
    Ok(finished)
}

struct Ctx<'a, F: Fetch> {
    f: &'a F,
    cfg: &'a Cfg,
    cdns: &'a HashMap<String, Cdn>,
    slots: Vec<Arc<Slot>>,
    /// Packed bytes received so far
    received: AtomicU64,
    stop: Arc<AtomicBool>,
}

/// Downloads one piece (the other CDNs in turn when one fails, new addresses when they ran out),
/// checks it and writes it where it goes. How many files it completed.
async fn run_unit<F: Fetch>(ctx: &Ctx<'_, F>, u: &Unit) -> Result<usize> {
    let tail = match &u.source {
        Source::Chunk(c) => galaxy_path(&c.compressed_md5)?,
        Source::Range { .. } => "main.bin".to_string(),
    };
    let cdn = ctx.cdns.get(&u.product).ok_or_else(|| anyhow!("The build has a piece of a product it does not list"))?;
    let mut last: Option<anyhow::Error> = None;
    for attempt in 0..ctx.cfg.tries.max(1) {
        if ctx.stop.load(Ordering::Relaxed) {
            return Err(canceled());
        }
        if attempt > 0 {
            tokio::time::sleep(ctx.cfg.wait * attempt as u32).await;
        }
        let (url, generation) = match cdn.url(ctx.f, &tail, attempt).await {
            Ok(x) => x,
            Err(e) => {
                if e.downcast_ref::<Denied>().is_some() {
                    return Err(e);
                }
                last = Some(e);
                continue;
            }
        };
        let tally = AtomicU64::new(0);
        let count = |n: u64| {
            tally.fetch_add(n, Ordering::Relaxed);
            ctx.received.fetch_add(n, Ordering::Relaxed);
        };
        let got = match &u.source {
            Source::Chunk(_) => ctx.f.get(&url, &count).await,
            Source::Range { offset, len } => ctx.f.get_range(&url, *offset, *len, &count).await,
        };
        match got {
            Ok(bytes) => {
                let source = u.source.clone();
                match tokio::task::spawn_blocking(move || check_piece(bytes, &source)).await? {
                    Ok(plain) => {
                        let places = u.places.iter().map(|p| (ctx.slots[p.slot].clone(), *p)).collect();
                        return tokio::task::spawn_blocking(move || place_all(places, &plain)).await?;
                    }
                    Err(e) => {
                        ctx.received.fetch_sub(tally.load(Ordering::Relaxed), Ordering::Relaxed);
                        last = Some(e);
                    }
                }
            }
            Err(e) => {
                ctx.received.fetch_sub(tally.load(Ordering::Relaxed), Ordering::Relaxed);
                if matches!(status_of(&e), Some(401 | 403 | 410)) {
                    cdn.refresh(ctx.f, generation).await?;
                }
                last = Some(e);
            }
        }
    }
    Err(last.unwrap_or_else(|| anyhow!("A piece of the game could not be downloaded")))
}

fn parts_dir(dest: &Path) -> PathBuf {
    dest.join(sync::OWN_DIR).join(PARTS)
}

fn clean_parts(dest: &Path) {
    let d = parts_dir(dest);
    for _ in 0..5 {
        if !d.exists() || std::fs::remove_dir_all(&d).is_ok() {
            return;
        }
        // A write that was in flight when the sync stopped may still hold a file.
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Downloads the chunks for `todo` into part files. The part files, by the path they are for.
#[allow(clippy::too_many_arguments)]
async fn download<F: Fetch>(
    f: &F,
    cfg: &Cfg,
    dest: &Path,
    todo: &[&Entry],
    depots: &[SavedDepot],
    cdns: &HashMap<String, Cdn>,
    progress: &(dyn Fn(Progress) + Send + Sync),
    cancel: &AtomicBool,
    stop: &Arc<AtomicBool>,
) -> Result<(Vec<(PathBuf, PathBuf)>, u64)> {
    let dir = parts_dir(dest);
    std::fs::create_dir_all(&dir).with_context(|| format!("Could not make {}", dir.display()))?;
    let units = plan_units(todo, depots, cfg.piece)?;
    let total: u64 = units.iter().map(|u| u.packed).sum();
    let mut slots = Vec::new();
    let mut parts = Vec::new();
    let mut files_done = 0;
    for (i, e) in todo.iter().enumerate() {
        let part = dir.join(format!("{i}.part"));
        parts.push((e.rel.clone(), part.clone()));
        if e.size == 0 {
            std::fs::File::create(&part).with_context(|| format!("Could not write {}", e.rel.display()))?;
            files_done += 1;
            continue;
        }
        // A packed file with one chunk can be checked by that chunk's md5, which the container
        // alone does not give.
        let own_md5 = e.file.md5.clone().or_else(|| (e.file.sfc.is_some() && e.file.chunks.len() == 1).then(|| e.file.chunks[0].md5.clone()));
        slots.push(Arc::new(Slot { rel: e.rel.clone(), part, size: e.size, left: AtomicU64::new(e.size), file: parking_lot::Mutex::new(None), md5: own_md5, sha256: e.file.sha256.clone() }));
    }
    let files_total = todo.len();
    let ctx = Ctx { f, cfg, cdns, slots, received: AtomicU64::new(0), stop: stop.clone() };
    emit(progress, "Downloading", 0, total, files_done, files_total);
    {
        // A few chunks at a time, the next one started as one is done.
        let mut pending = FuturesUnordered::new();
        let mut next = units.iter();
        let mut tick = tokio::time::interval(cfg.tick);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        let mut last_emit = Instant::now();
        loop {
            while pending.len() < cfg.fetchers.max(1) {
                match next.next() {
                    Some(u) => pending.push(run_unit(&ctx, u)),
                    None => break,
                }
            }
            if pending.is_empty() {
                break;
            }
            tokio::select! {
                r = pending.next() => match r {
                    Some(Ok(n)) => files_done += n,
                    Some(Err(e)) => {
                        stop.store(true, Ordering::Relaxed);
                        return Err(e);
                    }
                    None => {}
                },
                _ = tick.tick() => {
                    if cancel.load(Ordering::Relaxed) {
                        stop.store(true, Ordering::Relaxed);
                        return Err(canceled());
                    }
                    if last_emit.elapsed() >= cfg.report {
                        last_emit = Instant::now();
                        emit(progress, "Downloading", ctx.received.load(Ordering::Relaxed), total, files_done, files_total);
                    }
                }
            }
        }
    }
    if ctx.slots.iter().any(|s| s.left.load(Ordering::SeqCst) != 0) {
        bail!("A file of the build was not completed by its chunks");
    }
    let received = ctx.received.load(Ordering::Relaxed);
    emit(progress, "Downloading", received, total, files_total, files_total);
    Ok((parts, received))
}

/// Which of the entries are missing or wrong on disk (true), by hashing them against their chunks.
async fn find_bad(dest: &Path, entries: &[Entry], cfg: &Cfg, progress: &(dyn Fn(Progress) + Send + Sync), cancel: &AtomicBool, stop: &Arc<AtomicBool>) -> Result<Vec<bool>> {
    let total: u64 = entries.iter().map(|e| e.size).sum();
    let counted = Arc::new(AtomicU64::new(0));
    let mut bad = vec![false; entries.len()];
    let mut files_done = 0;
    emit(progress, "Checking files", 0, total, 0, entries.len());
    let mut pending = FuturesUnordered::new();
    let mut next = 0;
    let mut tick = tokio::time::interval(cfg.tick);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_emit = Instant::now();
    loop {
        while pending.len() < cfg.hashers.max(1) && next < entries.len() {
            let (i, e) = (next, &entries[next]);
            next += 1;
            let (path, size, file, stop, counted) = (dest.join(&e.rel), e.size, e.file.clone(), stop.clone(), counted.clone());
            pending.push(tokio::task::spawn_blocking(move || (i, !file_matches(&path, size, &file.chunks, file.md5.as_deref(), &stop, &counted))));
        }
        if pending.is_empty() {
            break;
        }
        tokio::select! {
            r = pending.next() => {
                if let Some(r) = r {
                    let (i, wrong) = r?;
                    bad[i] = wrong;
                    files_done += 1;
                }
            },
            _ = tick.tick() => {
                if cancel.load(Ordering::Relaxed) {
                    stop.store(true, Ordering::Relaxed);
                    return Err(canceled());
                }
                if last_emit.elapsed() >= cfg.report {
                    last_emit = Instant::now();
                    emit(progress, "Checking files", counted.load(Ordering::Relaxed), total, files_done, entries.len());
                }
            }
        }
    }
    Ok(bad)
}

fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1e9)
}

/// Fails when the drive of the game folder has less free space than `bytes` and some room to spare.
fn need_space(dest: &Path, bytes: u64, cfg: &Cfg) -> Result<()> {
    if bytes == 0 {
        return Ok(());
    }
    let Some((free, _)) = cfg.space.or_else(|| crate::winutil::disk_space(dest)) else { return Ok(()) };
    let need = bytes + (bytes / 100).max(64 << 20);
    if free < need {
        bail!("There is not enough free space on the drive for this game: {} needed, {} free", gb(need), gb(free));
    }
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// Which build, which depots

struct Target {
    generation: u8,
    timestamp: String,
    build_id: String,
    version: String,
    product_id: String,
    language: String,
    dependencies: Vec<String>,
    depots: Vec<SavedDepot>,
    tasks: Vec<ProductTask>,
    cdns: HashMap<String, Cdn>,
}

/// The products whose pieces a list of depots holds.
fn products_of(depots: &[SavedDepot]) -> HashSet<String> {
    let mut out = HashSet::new();
    for d in depots {
        out.insert(d.product_id.clone());
        out.extend(d.data.files.iter().filter_map(|f| f.v1.as_ref().map(|v| v.product.clone())));
    }
    out
}

impl Target {
    fn from_saved(s: &Saved) -> Target {
        let cdns = products_of(&s.depots).into_iter().map(|p| (p.clone(), Cdn::for_saved(&p, s))).collect();
        Target { generation: s.generation, timestamp: s.timestamp.clone(), build_id: s.build_id.clone(), version: s.version.clone(), product_id: s.product_id.clone(), language: s.language.clone(), dependencies: s.dependencies.clone(), depots: s.depots.clone(), tasks: s.tasks.clone(), cdns }
    }

    fn saved(&self) -> Saved {
        Saved {
            format: 1,
            generation: self.generation,
            timestamp: self.timestamp.clone(),
            build_id: self.build_id.clone(),
            version: self.version.clone(),
            product_id: self.product_id.clone(),
            language: self.language.clone(),
            dependencies: self.dependencies.clone(),
            depots: self.depots.clone(),
            tasks: self.tasks.clone(),
        }
    }
}

/// The folder part of a URL: "https://host/content-system/v2/meta" for a build link.
fn meta_base(link: &str) -> String {
    match link.find("/content-system/v2/meta/") {
        Some(i) => link[..i + "/content-system/v2/meta".len()].to_string(),
        None => "https://cdn.gog.com/content-system/v2/meta".to_string(),
    }
}

fn depot_url(base: &str, hash: &str) -> String {
    format!("{base}/{}/{}/{hash}", &hash[0..2], &hash[2..4])
}

async fn fetch_depot<F: Fetch>(f: &F, url: String, product_id: String, manifest: String) -> Result<SavedDepot> {
    let raw = f.get(&url, &|_| {}).await.context("Could not read a depot's manifest")?;
    Ok(SavedDepot { product_id, manifest, data: parse_depot_manifest(&raw)? })
}

async fn fetch_v1_depot<F: Fetch>(f: &F, url: String, product_id: String, manifest: String) -> Result<SavedDepot> {
    let raw = f.get(&url, &|_| {}).await.context("Could not read a depot's manifest")?;
    let data = v1_manifest(parse_v1_depot(&raw)?, &product_id)?;
    Ok(SavedDepot { product_id, manifest, data })
}

/// Finds out which of the products (the game and its add-ons) this account may download. The game
/// itself must be; an add-on counts when GOG gives its addresses, or when the build now installed
/// has it.
async fn allowed_products<F: Fetch>(f: &F, base: &str, cdns: &HashMap<String, Cdn>, opts: &Options, saved: Option<&Saved>) -> Result<HashSet<String>> {
    let own = cdns.get(base).ok_or_else(|| anyhow!("This build has no files of the game itself"))?;
    own.ensure(f).await.map_err(|e| {
        if e.downcast_ref::<Denied>().is_some() {
            anyhow!("GOG did not allow the download. Sign in to GOG again, and check that this account owns the game")
        } else {
            e
        }
    })?;
    let mut allowed: HashSet<String> = HashSet::from([base.to_string()]);
    if !opts.no_dlc {
        for (pid, cdn) in cdns {
            if pid == base {
                continue;
            }
            match cdn.ensure(f).await {
                Ok(()) => {
                    allowed.insert(pid.clone());
                }
                Err(e) if e.downcast_ref::<Denied>().is_some() => {
                    if saved.is_some_and(|s| products_of(&s.depots).contains(pid)) {
                        allowed.insert(pid.clone());
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    Ok(allowed)
}

/// Picks the build, reads its manifest and the manifests of the depots to install, and finds out
/// which of the products (the game and its add-ons) this account may download.
async fn resolve<F: Fetch>(f: &F, cfg: &Cfg, product_id: u64, opts: &Options, language: &str, saved: Option<&Saved>) -> Result<Target> {
    let builds = list_builds(f, product_id).await?;
    let want = match (&opts.build, &opts.branch) {
        (Some(id), _) => Pick::Id(id),
        (None, Some(branch)) => Pick::Branch(branch),
        (None, None) => Pick::Newest,
    };
    let build = pick_build(&builds, want).ok_or_else(|| match want {
        Pick::Id(id) => anyhow!("GOG does not list build {id} of this game"),
        Pick::Branch(b) => anyhow!("GOG lists no build of this game on the branch {b}"),
        Pick::Newest => anyhow!("GOG lists no Windows build of this game"),
    })?;
    if build.link.is_empty() {
        bail!("GOG's list names no manifest for this build");
    }
    if build.generation == 1 {
        return resolve_v1(f, cfg, product_id, build, opts, language, saved).await;
    }
    let raw = f.get(&build.link, &|_| {}).await.context("Could not read the build's manifest")?;
    let manifest = parse_build_manifest(&raw)?;
    let base = product_id.to_string();
    if manifest.product_id != base {
        bail!("This build belongs to another product");
    }
    let mut cdns: HashMap<String, Cdn> = HashMap::new();
    for d in &manifest.depots {
        cdns.entry(d.product_id.clone()).or_insert_with(|| Cdn::new(&d.product_id));
    }
    let allowed = allowed_products(f, &base, &cdns, opts, saved).await?;
    let (picked, chosen) = pick_depots(&manifest, language, &allowed);
    if picked.is_empty() {
        bail!("This build has no files for this game");
    }
    let origin = meta_base(&build.link);
    let mut depots = Vec::new();
    for group in picked.chunks(cfg.fetchers.clamp(1, 4)) {
        let mut fetching = Vec::new();
        for d in group {
            fetching.push(fetch_depot(f, depot_url(&origin, &d.manifest), d.product_id.clone(), d.manifest.clone()));
        }
        for r in futures_util::future::join_all(fetching).await {
            depots.push(r?);
        }
    }
    let cdns = cdns.into_iter().filter(|(pid, _)| allowed.contains(pid)).collect();
    Ok(Target { generation: 2, timestamp: String::new(), build_id: build.id.clone(), version: build.version_or_id(), product_id: base, language: chosen.unwrap_or_default(), dependencies: manifest.dependencies, depots, tasks: manifest.tasks, cdns })
}

/// `resolve` for a generation 1 build: its `repository.json` and the manifests of its depots.
async fn resolve_v1<F: Fetch>(f: &F, cfg: &Cfg, product_id: u64, build: &BuildRef, opts: &Options, language: &str, saved: Option<&Saved>) -> Result<Target> {
    let raw = f.get(&build.link, &|_| {}).await.context("Could not read the build's repository")?;
    let repo = parse_v1_repository(&raw)?;
    let base = product_id.to_string();
    let for_windows = |d: &V1Depot| d.systems.is_empty() || d.systems.iter().any(|s| s.eq_ignore_ascii_case("windows"));
    let usable: Vec<&V1Depot> = repo.depots.iter().filter(|d| for_windows(d)).collect();
    let mut cdns: HashMap<String, Cdn> = HashMap::new();
    cdns.insert(base.clone(), Cdn::v1(&base, &repo.timestamp));
    for g in usable.iter().flat_map(|d| d.game_ids.iter()) {
        cdns.entry(g.clone()).or_insert_with(|| Cdn::v1(g, &repo.timestamp));
    }
    let allowed = allowed_products(f, &base, &cdns, opts, saved).await?;
    let mut picked: Vec<&V1Depot> = usable.into_iter().filter(|d| d.game_ids.iter().any(|g| allowed.contains(g))).collect();
    let available: Vec<String> = picked.iter().flat_map(|d| d.languages.iter().cloned()).collect();
    let chosen = pick_language(&available, language);
    picked.retain(|d| depot_in_language(&d.languages, chosen.as_deref()));
    if picked.is_empty() {
        bail!("This build has no files for this game");
    }
    let dir = build.link.rsplit_once('/').map(|(d, _)| d.to_string()).ok_or_else(|| anyhow!("GOG's list names no manifest for this build"))?;
    let mut depots = Vec::new();
    for group in picked.chunks(cfg.fetchers.clamp(1, 4)) {
        let mut fetching = Vec::new();
        for d in group {
            if d.manifest.is_empty() || !d.manifest.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.') {
                bail!("A depot of the build has a manifest name that is not understood");
            }
            let owner = d.game_ids.iter().find(|g| allowed.contains(*g)).cloned().unwrap_or_else(|| base.clone());
            fetching.push(fetch_v1_depot(f, format!("{dir}/{}", d.manifest), owner, d.manifest.clone()));
        }
        for r in futures_util::future::join_all(fetching).await {
            depots.push(r?);
        }
    }
    // Files may name another product's main.bin.
    for p in products_of(&depots) {
        cdns.entry(p.clone()).or_insert_with(|| Cdn::v1(&p, &repo.timestamp));
    }
    Ok(Target { generation: 1, timestamp: repo.timestamp, build_id: build.id.clone(), version: build.version_or_id(), product_id: base, language: chosen.unwrap_or_default(), dependencies: repo.redists, depots, tasks: vec![], cdns })
}

// ---------------------------------------------------------------------------------------------
// How the game is started

/// Splits an arguments text into arguments, honoring double quotes.
fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let (mut quoted, mut any) = (false, false);
    for ch in s.chars() {
        match ch {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// The program that starts the game and its arguments: the main play task of the game's
/// `goggame-<id>.info`, else the build manifest's own entry for the product. Empty when neither
/// names a path inside the game folder.
fn launch_of(dest: &Path, product_id: &str, language: &str, tasks: &[ProductTask]) -> (String, Vec<String>) {
    let from_info = || -> Option<(String, String)> {
        let bytes = std::fs::read(dest.join(format!("goggame-{product_id}.info"))).ok()?;
        let v: Value = serde_json::from_str(super::decode_text(&bytes).trim_start_matches('\u{feff}')).ok()?;
        let all: Vec<&Value> = v["playTasks"].as_array()?.iter().filter(|t| t["type"].as_str().unwrap_or("FileTask") == "FileTask" && !text(&t["path"]).is_empty()).collect();
        let lang = lang_norm(language);
        let for_language = |t: &Value| {
            let l = strings(&t["languages"]);
            l.is_empty() || l.iter().any(|x| lang_norm(x) == lang || lang_primary(&lang_norm(x)) == lang_primary(&lang))
        };
        let primary = |t: &Value| t["isPrimary"].as_bool() == Some(true);
        let t = all
            .iter()
            .copied()
            .find(|t| primary(t) && for_language(t))
            .or_else(|| all.iter().copied().find(|t| primary(t)))
            .or_else(|| all.iter().copied().find(|t| t["category"].as_str() == Some("game")))
            .or_else(|| all.first().copied())?;
        Some((text(&t["path"]), text(&t["arguments"])))
    };
    let from_manifest = || tasks.iter().find(|t| t.product_id == product_id && !t.exe.is_empty()).map(|t| (t.exe.clone(), t.args.clone()));
    let Some((path, args)) = from_info().or_else(from_manifest) else { return (String::new(), vec![]) };
    match sync::safe_rel(path.trim_start_matches(['/', '\\'])) {
        Some(p) => (p.to_string_lossy().replace('/', "\\"), split_args(&args)),
        None => (String::new(), vec![]),
    }
}

// ---------------------------------------------------------------------------------------------
// The sync

/// Choices beyond the plain install.
#[derive(Clone, Default)]
pub struct Options {
    /// A build by its id, instead of the newest
    pub build: Option<String>,
    /// The newest build of a named branch, instead of the default branch
    pub branch: Option<String>,
    /// Leave out the add-ons (DLC) the account has
    pub no_dlc: bool,
    /// Asked for a fresh access token when GOG says the one given ran out (a download can take
    /// longer than a token lasts)
    pub fresh_token: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
}

/// Brings `dest` to the newest build of the game: Install everything, Update what changed since
/// the last sync, Repair what is missing or wrong. `language` is a locale ("en-US") or a language
/// name ("English"); the build's depots for it are installed, else English, else the first it has.
/// Add-ons the account has are installed with the game. The game's state is written only when the
/// whole sync succeeded, and a failed or canceled sync leaves the installed build as it was.
#[allow(clippy::too_many_arguments)]
pub async fn sync(token: &str, product_id: u64, name: &str, dest: &Path, mode: Mode, language: &str, progress: &(dyn Fn(Progress) + Send + Sync), cancel: &AtomicBool) -> Result<Report> {
    sync_with(token, product_id, name, dest, mode, language, &Options::default(), progress, cancel).await
}

/// `sync` with choices: a given build or branch, no add-ons, a way to get a fresh token.
#[allow(clippy::too_many_arguments)]
pub async fn sync_with(token: &str, product_id: u64, name: &str, dest: &Path, mode: Mode, language: &str, opts: &Options, progress: &(dyn Fn(Progress) + Send + Sync), cancel: &AtomicBool) -> Result<Report> {
    let http = Http::with_token(token, opts.fresh_token.clone());
    sync_via(&http, &Cfg::default(), product_id, name, dest, mode, language, opts, progress, cancel).await
}

#[allow(clippy::too_many_arguments)]
async fn sync_via<F: Fetch>(f: &F, cfg: &Cfg, product_id: u64, name: &str, dest: &Path, mode: Mode, language: &str, opts: &Options, progress: &(dyn Fn(Progress) + Send + Sync), cancel: &AtomicBool) -> Result<Report> {
    sync::check_cancel(cancel)?;
    let stop = Arc::new(AtomicBool::new(false));
    let result = sync_inner(f, cfg, product_id, name, dest, mode, language, opts, progress, cancel, &stop).await;
    stop.store(true, Ordering::Relaxed);
    clean_parts(dest);
    result
}

#[allow(clippy::too_many_arguments)]
async fn sync_inner<F: Fetch>(f: &F, cfg: &Cfg, product_id: u64, name: &str, dest: &Path, mode: Mode, language: &str, opts: &Options, progress: &(dyn Fn(Progress) + Send + Sync), cancel: &AtomicBool, stop: &Arc<AtomicBool>) -> Result<Report> {
    let pid = product_id.to_string();
    emit(progress, "Preparing", 0, 0, 0, 0);
    clean_parts(dest);
    let before = sync::read_state(dest).filter(|s| s.platform == "gog" && s.game == pid);
    let saved = load_saved(dest, before.as_ref());

    // The build to bring the folder to: a repair checks the installed build, anything else
    // brings it to the newest (or the one asked for).
    let target = match (mode, &saved) {
        (Mode::Repair, Some(s)) => Target::from_saved(s),
        _ => resolve(f, cfg, product_id, opts, language, saved.as_ref()).await?,
    };
    sync::check_cancel(cancel)?;
    let new = merge(&target.depots)?;

    // What to write, and what the new build drops.
    let (todo, removals): (Vec<usize>, Vec<PathBuf>) = match mode {
        Mode::Install => ((0..new.entries.len()).collect(), vec![]),
        Mode::Update if saved.is_some() => {
            let old = merge(&saved.as_ref().map(|s| s.depots.clone()).unwrap_or_default())?.entries;
            let (dest2, new2) = (dest.to_path_buf(), new.entries.clone());
            tokio::task::spawn_blocking(move || diff_update(&dest2, &new2, &old)).await?
        }
        _ => {
            // Nothing to compare with (or a repair): the files on disk are checked against the build.
            let bad = find_bad(dest, &new.entries, cfg, progress, cancel, stop).await?;
            ((0..bad.len()).filter(|&i| bad[i]).collect(), vec![])
        }
    };
    sync::check_cancel(cancel)?;
    let todo_entries: Vec<&Entry> = todo.iter().map(|&i| &new.entries[i]).collect();
    need_space(dest, todo_entries.iter().map(|e| e.size).sum(), cfg)?;

    // Everything is made whole in part files before the installed build is touched.
    let (parts, received) = download(f, cfg, dest, &todo_entries, &target.depots, &target.cdns, progress, cancel, stop).await?;
    sync::check_cancel(cancel)?;

    emit(progress, "Writing files", 0, 0, 0, parts.len());
    let kept_launch = before.as_ref().map(|s| (s.launch.clone(), s.launch_args.clone()));
    let changes = !parts.is_empty() || !removals.is_empty();
    let mut undo = if changes { Some(Undo::begin(dest, before.clone())?) } else { None };
    let committed = (|| -> Result<()> {
        if let Some(undo) = undo.as_mut() {
            for rel in &removals {
                undo.remove(rel)?;
            }
            for (rel, part) in &parts {
                undo.keep(rel)?;
                let to = dest.join(rel);
                if let Some(dir) = to.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                std::fs::rename(part, &to).or_else(|_| std::fs::copy(part, &to).map(|_| ()).and_then(|_| std::fs::remove_file(part))).with_context(|| format!("Could not put {} in place; is the game running?", rel.display()))?;
            }
        }
        for dir in &new.dirs {
            std::fs::create_dir_all(dest.join(dir))?;
        }
        let (launch, launch_args) = {
            let (l, a) = launch_of(dest, &pid, &target.language, &target.tasks);
            if l.is_empty() { kept_launch.clone().unwrap_or_default() } else { (l, a) }
        };
        save_manifest(dest, &target.saved())?;
        sync::write_state(dest, &State { platform: "gog".into(), game: pid.clone(), name: name.to_string(), version: target.version.clone(), build: target.build_id.clone(), launch, launch_args })
    })();
    if let Err(e) = committed {
        // Put back what this sync replaced (the state was not written). Without an undo of its
        // own there is nothing to put back, and the one an update before left is not touched.
        if undo.is_some() {
            let _ = sync::undo(dest);
        }
        return Err(e);
    }
    let undo_dir = match undo {
        Some(u) => u.finish()?,
        None => None,
    };
    emit(progress, "Writing files", 0, 0, parts.len(), parts.len());
    Ok(Report { version: target.version, bytes_downloaded: received, files_written: parts.len(), files_repaired: if mode == Mode::Repair { parts.len() } else { 0 }, undo: undo_dir })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicUsize;

    const PID: u64 = 4242;
    const DLC: u64 = 4343;
    const CHUNK: usize = 1000;
    const STORE: &str = "https://cdn.test/store";
    const V1STORE: &str = "https://cdn.test/v1store";
    /// The number of the generation 1 builds in the fixtures.
    const TS: &str = "64924992";

    struct Tmp(PathBuf);

    impl Tmp {
        fn new(name: &str) -> Tmp {
            let d = std::env::temp_dir().join(format!("u3d-gogdl-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Tmp(d)
        }

        /// The game folder inside (it does not exist yet).
        fn game(&self) -> PathBuf {
            self.0.join("game")
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    // ----- a GOG that lives in memory -----

    struct Fake {
        files: parking_lot::Mutex<HashMap<String, Vec<u8>>>,
        builds: parking_lot::Mutex<Vec<Value>>,
        /// The compressed md5 of every chunk asked for
        chunk_log: parking_lot::Mutex<Vec<String>>,
        denied: parking_lot::Mutex<HashSet<u64>>,
        cancel_after: parking_lot::Mutex<Option<(usize, Arc<AtomicBool>)>>,
        /// Chunk -> times it answers 500 before it works
        flaky: parking_lot::Mutex<HashMap<String, usize>>,
        /// What the download addresses say as their token, one answer after the other
        tokens: parking_lot::Mutex<Vec<String>>,
        valid_token: parking_lot::Mutex<String>,
        link_asks: AtomicUsize,
        delay: parking_lot::Mutex<Duration>,
    }

    impl Fake {
        fn new() -> Fake {
            Fake {
                files: Default::default(),
                builds: Default::default(),
                chunk_log: Default::default(),
                denied: Default::default(),
                cancel_after: Default::default(),
                flaky: Default::default(),
                tokens: parking_lot::Mutex::new(vec!["ok".into()]),
                valid_token: parking_lot::Mutex::new("ok".into()),
                link_asks: AtomicUsize::new(0),
                delay: Default::default(),
            }
        }

        fn next_token(&self) -> String {
            let mut t = self.tokens.lock();
            if t.len() > 1 {
                t.remove(0)
            } else {
                t[0].clone()
            }
        }

        fn asked(&self) -> HashSet<String> {
            self.chunk_log.lock().iter().cloned().collect()
        }

        /// A piece was asked for: logged, and the cancel flag set when it is time.
        fn note(&self, name: String) -> usize {
            let n = {
                let mut l = self.chunk_log.lock();
                l.push(name);
                l.len()
            };
            if let Some((after, flag)) = self.cancel_after.lock().as_ref() {
                if n >= *after {
                    flag.store(true, Ordering::SeqCst);
                }
            }
            n
        }
    }

    impl Fetch for Fake {
        async fn get(&self, url: &str, got: &(dyn Fn(u64) + Send + Sync)) -> Result<Vec<u8>> {
            let delay = *self.delay.lock();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let not_found = || anyhow::Error::new(HttpStatus(404));
            if let Some(rest) = url.strip_prefix(&format!("{STORE}/")) {
                let (path, token) = rest.split_once('?').unwrap_or((rest, ""));
                let md5 = path.rsplit('/').next().unwrap_or("").to_string();
                self.note(md5.clone());
                if token != *self.valid_token.lock() {
                    return Err(anyhow::Error::new(HttpStatus(403)));
                }
                if let Some(left) = self.flaky.lock().get_mut(&md5) {
                    if *left > 0 {
                        *left -= 1;
                        return Err(anyhow::Error::new(HttpStatus(500)));
                    }
                }
                let body = self.files.lock().get(&format!("{STORE}/{path}")).cloned().ok_or_else(not_found)?;
                got(body.len() as u64);
                return Ok(body);
            }
            if url.contains("/secure_link") {
                let pid: u64 = url.split("/products/").nth(1).and_then(|r| r.split('/').next()).and_then(|p| p.parse().ok()).ok_or_else(not_found)?;
                self.link_asks.fetch_add(1, Ordering::SeqCst);
                if self.denied.lock().contains(&pid) {
                    return Err(anyhow::Error::new(HttpStatus(403)));
                }
                let token = self.next_token();
                let body = if url.contains("type=depot") {
                    // Generation 1: the build's number is the path asked for.
                    if !url.ends_with(&format!("path=/windows/{TS}/")) {
                        return Err(not_found());
                    }
                    json!({"product_id": pid, "urls": [{"endpoint_name": "test", "url_format": "{base_url}{path}?{token}", "parameters": {"base_url": format!("{V1STORE}/{pid}"), "path": format!("/windows/{TS}"), "token": token}, "max_age": 3600}]})
                } else {
                    json!({"product_id": pid, "urls": [{"endpoint_name": "test", "url_format": "{base_url}/{path}?{token}", "parameters": {"base_url": format!("{STORE}/{pid}"), "path": "/", "token": token}, "priority": 1, "max_age": 3600}]})
                };
                return Ok(body.to_string().into_bytes());
            }
            if url == builds_url(PID, 2) {
                let items: Vec<Value> = self.builds.lock().clone();
                return Ok(json!({"total_count": items.len(), "count": items.len(), "items": items}).to_string().into_bytes());
            }
            let body = self.files.lock().get(url).cloned().ok_or_else(not_found)?;
            got(body.len() as u64);
            Ok(body)
        }

        async fn get_range(&self, url: &str, start: u64, len: u64, got: &(dyn Fn(u64) + Send + Sync)) -> Result<Vec<u8>> {
            let delay = *self.delay.lock();
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let (path, token) = url.split_once('?').unwrap_or((url, ""));
            self.note(format!("range:{start}+{len}"));
            if token != *self.valid_token.lock() {
                return Err(anyhow::Error::new(HttpStatus(403)));
            }
            let body = self.files.lock().get(path).cloned().ok_or_else(|| anyhow::Error::new(HttpStatus(404)))?;
            let end = (start + len) as usize;
            if end > body.len() {
                return Err(anyhow::Error::new(HttpStatus(416)));
            }
            let part = body[start as usize..end].to_vec();
            got(part.len() as u64);
            Ok(part)
        }
    }

    // ----- building a game for it -----

    struct TFile {
        path: String,
        data: Vec<u8>,
        flags: Vec<&'static str>,
        /// Give the file's own md5 and sha256
        hashes: bool,
        /// Pack it in the depot's small files container
        packed: bool,
        /// Say a wrong md5 for its chunks
        wrong_md5: bool,
        /// Say a wrong sha256 for the file (with `hashes`)
        wrong_hash: bool,
    }

    struct TDepot {
        product: u64,
        langs: Vec<&'static str>,
        files: Vec<TFile>,
        dirs: Vec<&'static str>,
    }

    fn bytes(seed: u8, len: usize) -> Vec<u8> {
        let mut x = 0x9E37_79B9u32 ^ (seed as u32).wrapping_mul(2_654_435_761);
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x >> 8) as u8
            })
            .collect()
    }

    fn tf(path: &str, seed: u8, len: usize) -> TFile {
        TFile { path: path.into(), data: bytes(seed, len), flags: vec![], hashes: false, packed: false, wrong_md5: false, wrong_hash: false }
    }

    fn info(exe: &str) -> TFile {
        let v = json!({"gameId": PID.to_string(), "name": "Test Game", "playTasks": [
            {"category": "game", "isPrimary": true, "languages": ["en-US"], "name": "Test Game", "path": exe, "arguments": "-windowed \"two words\"", "type": "FileTask"},
            {"category": "document", "name": "Manual", "path": "manual.pdf", "type": "FileTask"}], "rootGameId": PID.to_string(), "version": 1});
        TFile { path: format!("goggame-{PID}.info"), data: v.to_string().into_bytes(), flags: vec![], hashes: false, packed: false, wrong_md5: false, wrong_hash: false }
    }

    fn zlib(v: &Value) -> Vec<u8> {
        miniz_oxide::deflate::compress_to_vec_zlib(v.to_string().as_bytes(), 6)
    }

    /// The compressed md5s of the chunks a file is cut into.
    fn chunk_names(data: &[u8]) -> HashSet<String> {
        data.chunks(CHUNK).map(|c| md5_hex(&miniz_oxide::deflate::compress_to_vec_zlib(c, 6))).collect()
    }

    fn chunk_json(plain: &[u8]) -> (Value, Vec<u8>, String) {
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(plain, 6);
        let name = md5_hex(&packed);
        (json!({"compressedMd5": name, "compressedSize": packed.len(), "md5": md5_hex(plain), "size": plain.len()}), packed, name)
    }

    /// Puts a chunk in the store (as the CDN of `product` serves it) and returns its manifest entry.
    fn store_chunk(fake: &Fake, product: u64, plain: &[u8], wrong_md5: bool) -> Value {
        let (mut j, packed, name) = chunk_json(plain);
        if wrong_md5 {
            j["md5"] = json!("00000000000000000000000000000000");
        }
        fake.files.lock().insert(format!("{STORE}/{product}/{}/{}/{name}", &name[0..2], &name[2..4]), packed);
        j
    }

    fn meta_url(hash: &str) -> String {
        format!("https://cdn.gog.com/content-system/v2/meta/{}/{}/{hash}", &hash[0..2], &hash[2..4])
    }

    /// Publishes a build of the game on the fake GOG: its depots and chunks, its manifest and its
    /// place in the list of builds.
    fn publish(fake: &Fake, build_id: &str, version: &str, date: &str, depots: &[TDepot], deps: &[&str]) {
        let mut depot_list = Vec::new();
        for d in depots {
            let mut items = Vec::new();
            let mut container: Vec<u8> = Vec::new();
            for f in &d.files {
                let mut it;
                if f.packed {
                    let (own, _, _) = chunk_json(&f.data);
                    it = json!({"type": "DepotFile", "path": f.path, "chunks": [own], "sfcRef": {"offset": container.len(), "size": f.data.len()}});
                    container.extend_from_slice(&f.data);
                } else {
                    let chunks: Vec<Value> = f.data.chunks(CHUNK).map(|c| store_chunk(fake, d.product, c, f.wrong_md5)).collect();
                    it = json!({"type": "DepotFile", "path": f.path, "chunks": chunks});
                }
                if !f.flags.is_empty() {
                    it["flags"] = json!(f.flags);
                }
                if f.hashes {
                    it["md5"] = json!(md5_hex(&f.data));
                    it["sha256"] = json!(if f.wrong_hash { "11".repeat(32) } else { hex::encode(Sha256::digest(&f.data)) });
                }
                items.push(it);
            }
            for dir in &d.dirs {
                items.push(json!({"type": "DepotDirectory", "path": dir}));
            }
            let mut depot = json!({"items": items});
            if !container.is_empty() {
                let chunks: Vec<Value> = container.chunks(CHUNK).map(|c| store_chunk(fake, d.product, c, false)).collect();
                depot["smallFilesContainer"] = json!({"chunks": chunks});
            }
            let raw = zlib(&json!({"depot": depot, "version": 2}));
            let hash = md5_hex(&raw);
            let size: usize = d.files.iter().map(|f| f.data.len()).sum();
            fake.files.lock().insert(meta_url(&hash), raw);
            depot_list.push(json!({"compressedSize": size, "languages": d.langs, "manifest": hash, "osBitness": ["64"], "productId": d.product.to_string(), "size": size}));
        }
        let manifest = json!({"baseProductId": PID.to_string(), "buildId": build_id, "clientId": "0000000000", "clientSecret": "made-up-secret-value", "dependencies": deps, "depots": depot_list,
            "installDirectory": "Test Game", "platform": "windows", "productId": PID.to_string(),
            "products": [{"name": "Test Game", "productId": PID.to_string(), "temp_arguments": "", "temp_executable": ""}], "version": 2});
        let raw = zlib(&manifest);
        let link = meta_url(&md5_hex(&raw));
        fake.files.lock().insert(link.clone(), raw);
        let mut builds = fake.builds.lock();
        builds.insert(0, json!({"build_id": build_id, "product_id": PID.to_string(), "os": "windows", "branch": null, "version_name": version, "tags": [], "public": true, "date_published": date, "generation": 2, "link": link}));
    }

    fn game_v1() -> Vec<TDepot> {
        vec![TDepot { product: PID, langs: vec!["*"], files: vec![tf("bin\\game.exe", 1, 2500), tf("data\\level1.pak", 2, 1500), tf("readme.txt", 3, 300), info("bin\\game.exe")], dirs: vec!["saves"] }]
    }

    /// Build 2: game.exe changed, readme.txt dropped, level2.pak added, level1.pak the same.
    fn game_v2() -> Vec<TDepot> {
        vec![TDepot { product: PID, langs: vec!["*"], files: vec![tf("bin\\game.exe", 9, 2600), tf("data\\level1.pak", 2, 1500), tf("data\\level2.pak", 5, 700), info("bin\\game.exe")], dirs: vec!["saves"] }]
    }

    fn cfg() -> Cfg {
        Cfg { fetchers: 4, hashers: 2, tries: 2, wait: Duration::from_millis(1), tick: Duration::from_millis(10), report: Duration::from_secs(1), piece: 1000, space: None }
    }

    async fn run_with(fake: &Fake, dest: &Path, mode: Mode, lang: &str, opts: &Options, cancel: &AtomicBool) -> Result<(Report, Vec<Progress>)> {
        let seen = parking_lot::Mutex::new(Vec::new());
        let r = sync_via(fake, &cfg(), PID, "Test Game", dest, mode, lang, opts, &|p| seen.lock().push(p), cancel).await?;
        Ok((r, seen.into_inner()))
    }

    async fn run(fake: &Fake, dest: &Path, mode: Mode) -> Result<(Report, Vec<Progress>)> {
        run_with(fake, dest, mode, "en-US", &Options::default(), &AtomicBool::new(false)).await
    }

    fn assert_file(dest: &Path, rel: &str, want: &[u8]) {
        let got = std::fs::read(dest.join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert_eq!((got.len(), md5_hex(&got)), (want.len(), md5_hex(want)), "{rel}");
    }

    fn assert_v1(dest: &Path) {
        assert_file(dest, "bin\\game.exe", &bytes(1, 2500));
        assert_file(dest, "data\\level1.pak", &bytes(2, 1500));
        assert_file(dest, "readme.txt", &bytes(3, 300));
    }

    fn no_parts(dest: &Path) {
        assert!(!parts_dir(dest).exists(), "part files were left behind");
    }

    // ----- installing -----

    #[tokio::test]
    async fn a_game_is_installed_file_for_file_and_its_state_written() {
        let t = Tmp::new("install");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &["MSVC2015", "DirectX"]);
        let (r, progress) = run(&fake, &dest, Mode::Install).await.unwrap();
        assert_v1(&dest);
        assert!(dest.join("saves").is_dir(), "empty folders of the build are made");
        let s = sync::read_state(&dest).unwrap();
        assert_eq!((s.platform.as_str(), s.game.as_str(), s.name.as_str(), s.version.as_str(), s.build.as_str()), ("gog", "4242", "Test Game", "1.0", "100"));
        assert_eq!((s.launch.as_str(), s.launch_args.as_slice()), ("bin\\game.exe", ["-windowed".to_string(), "two words".to_string()].as_slice()));
        assert_eq!((r.version.as_str(), r.files_written, r.files_repaired), ("1.0", 4, 0));
        assert!(r.bytes_downloaded > 0 && r.undo.is_some());
        assert_eq!(dependencies(&dest), vec!["MSVC2015".to_string(), "DirectX".to_string()]);
        no_parts(&dest);
        // The client keys the build manifest carries are not kept.
        let kept = std::fs::read_to_string(own_file(&dest, MANIFEST_FILE)).unwrap();
        assert!(!kept.contains("made-up-secret-value") && !kept.contains("clientSecret") && !kept.contains("clientId"));
        // Progress ends complete, in steps the queue knows.
        let last = progress.iter().rev().find(|p| p.step == "Downloading").unwrap();
        assert!(last.total_bytes > 0 && last.done_bytes == last.total_bytes && last.files_done == last.files_total);
        assert!(progress.iter().all(|p| ["Preparing", "Downloading", "Checking files", "Writing files"].contains(&p.step.as_str())));
    }

    #[tokio::test]
    async fn small_files_packed_in_the_container_come_out_whole_across_chunk_edges() {
        let t = Tmp::new("sfc");
        let dest = t.game();
        let fake = Fake::new();
        let mut files = vec![tf("big.bin", 4, 2300)];
        let mut want = Vec::new();
        for i in 0..9u8 {
            let mut f = tf(&format!("small\\f{i}.dat"), 20 + i, 130 + i as usize * 111);
            f.packed = true;
            want.push((f.path.clone(), f.data.clone()));
            files.push(f);
        }
        let mut hashed = tf("small\\hashed.dat", 40, 777);
        hashed.packed = true;
        hashed.hashes = true;
        want.push((hashed.path.clone(), hashed.data.clone()));
        files.push(hashed);
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files, dirs: vec![] }], &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        assert_file(&dest, "big.bin", &bytes(4, 2300));
        for (p, d) in &want {
            assert_file(&dest, p, d);
        }
        // Only the container's chunks were fetched for the small files (their own chunks are not on the CDN).
        let container: Vec<u8> = want.iter().flat_map(|(_, d)| d.clone()).collect();
        let mut expected = chunk_names(&bytes(4, 2300));
        expected.extend(chunk_names(&container));
        assert_eq!(fake.asked(), expected);
    }

    #[tokio::test]
    async fn empty_files_support_files_and_paths_with_a_leading_slash() {
        let t = Tmp::new("odd");
        let dest = t.game();
        let fake = Fake::new();
        let mut support = tf("app\\settings.conf", 8, 90);
        support.flags = vec!["support"];
        let files = vec![TFile { data: vec![], ..tf("logs\\empty.log", 0, 0) }, support, tf("/Data/lead.bin", 6, 1200)];
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files, dirs: vec![] }], &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        assert_file(&dest, "logs\\empty.log", b"");
        assert_file(&dest, "gog-support\\4242\\app\\settings.conf", &bytes(8, 90));
        assert_file(&dest, "Data\\lead.bin", &bytes(6, 1200));
        assert!(sync::read_state(&dest).unwrap().launch.is_empty(), "no info file and no entry in the manifest: no launch path");
    }

    #[tokio::test]
    async fn the_same_content_in_two_files_is_downloaded_once() {
        let t = Tmp::new("dedupe");
        let dest = t.game();
        let fake = Fake::new();
        let a = tf("a.bin", 7, 1800);
        let b = TFile { path: "copy\\b.bin".into(), ..tf("x", 7, 1800) };
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files: vec![a, b], dirs: vec![] }], &[]);
        let (r, _) = run(&fake, &dest, Mode::Install).await.unwrap();
        assert_file(&dest, "a.bin", &bytes(7, 1800));
        assert_file(&dest, "copy\\b.bin", &bytes(7, 1800));
        assert_eq!(fake.chunk_log.lock().len(), 2, "two chunks of 1000 and 800 bytes, each fetched once");
        assert_eq!(r.files_written, 2);
    }

    #[tokio::test]
    async fn a_file_with_a_hash_is_checked_against_it() {
        let t = Tmp::new("hashed");
        let dest = t.game();
        let fake = Fake::new();
        let mut f = tf("pak\\hashed.pak", 3, 2400);
        f.hashes = true;
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files: vec![f], dirs: vec![] }], &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        assert_file(&dest, "pak\\hashed.pak", &bytes(3, 2400));
        // The same file, with a sha256 that is not its own: refused.
        let t2 = Tmp::new("hashed-bad");
        let fake2 = Fake::new();
        let mut g = tf("pak\\hashed.pak", 3, 2400);
        g.hashes = true;
        g.wrong_hash = true;
        publish(&fake2, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files: vec![g], dirs: vec![] }], &[]);
        let err = run(&fake2, &t2.game(), Mode::Install).await.unwrap_err();
        assert!(format!("{err:#}").contains("did not come out right"), "{err:#}");
        assert!(sync::read_state(&t2.game()).is_none());
        no_parts(&t2.game());
    }

    // ----- updating -----

    #[tokio::test]
    async fn an_update_fetches_only_what_changed_and_can_be_put_back_exactly() {
        let t = Tmp::new("update");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &["MSVC2015"]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &game_v2(), &["MSVC2015"]);
        fake.chunk_log.lock().clear();
        let (r, _) = run(&fake, &dest, Mode::Update).await.unwrap();
        // Only game.exe (changed) and level2.pak (new) came over the wire.
        let mut want = chunk_names(&bytes(9, 2600));
        want.extend(chunk_names(&bytes(5, 700)));
        assert_eq!(fake.asked(), want);
        assert_eq!(fake.chunk_log.lock().len(), want.len(), "every chunk once");
        assert_eq!((r.version.as_str(), r.files_written, r.files_repaired), ("2.0", 2, 0));
        assert_file(&dest, "bin\\game.exe", &bytes(9, 2600));
        assert_file(&dest, "data\\level1.pak", &bytes(2, 1500));
        assert_file(&dest, "data\\level2.pak", &bytes(5, 700));
        assert!(!dest.join("readme.txt").exists(), "a file the new build dropped is gone");
        let s = sync::read_state(&dest).unwrap();
        assert_eq!((s.version.as_str(), s.build.as_str()), ("2.0", "200"));
        no_parts(&dest);

        // Back to 1.0, exactly.
        let back = sync::undo(&dest).unwrap().unwrap();
        assert_eq!((back.version.as_str(), back.build.as_str()), ("1.0", "100"));
        assert_v1(&dest);
        assert!(!dest.join("data").join("level2.pak").exists());
        assert_eq!(sync::read_state(&dest).unwrap().version, "1.0");

        // Updating again after the undo still knows which build the folder has (from the manifest
        // kept for it, so the files are not all checked again): the same two files.
        fake.chunk_log.lock().clear();
        let (_, progress) = run(&fake, &dest, Mode::Update).await.unwrap();
        assert_eq!(fake.asked(), want);
        assert!(progress.iter().all(|p| p.step != "Checking files"));
        assert_file(&dest, "bin\\game.exe", &bytes(9, 2600));
    }

    #[tokio::test]
    async fn an_update_notices_a_file_that_changed_but_kept_its_size() {
        let t = Tmp::new("samesize");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        let mut v2 = game_v1();
        v2[0].files[1] = tf("data\\level1.pak", 12, 1500);
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &v2, &[]);
        fake.chunk_log.lock().clear();
        let (r, _) = run(&fake, &dest, Mode::Update).await.unwrap();
        assert_eq!(fake.asked(), chunk_names(&bytes(12, 1500)));
        assert_eq!(r.files_written, 1);
        assert_file(&dest, "data\\level1.pak", &bytes(12, 1500));
        assert_file(&dest, "bin\\game.exe", &bytes(1, 2500));
    }

    #[tokio::test]
    async fn an_update_to_the_build_already_installed_changes_nothing_and_keeps_the_undo() {
        let t = Tmp::new("same");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        let log_before = sync::can_undo(&dest);
        fake.chunk_log.lock().clear();
        let (r, _) = run(&fake, &dest, Mode::Update).await.unwrap();
        assert!(fake.chunk_log.lock().is_empty());
        assert_eq!((r.files_written, r.bytes_downloaded, r.undo.is_none()), (0, 0, true));
        assert_eq!(sync::can_undo(&dest), log_before);
    }

    #[tokio::test]
    async fn a_failed_sync_that_changed_nothing_does_not_put_an_older_update_back() {
        let t = Tmp::new("noundo");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &game_v2(), &[]);
        run(&fake, &dest, Mode::Update).await.unwrap();
        assert!(sync::can_undo(&dest).is_some());
        // The manifest cannot be written this time (a folder has its name); nothing else is to change.
        let m = own_file(&dest, MANIFEST_FILE);
        std::fs::remove_file(&m).unwrap();
        std::fs::create_dir_all(&m).unwrap();
        assert!(run(&fake, &dest, Mode::Update).await.is_err());
        // The update before is still there to put back, and was not put back by the failure.
        assert!(sync::can_undo(&dest).is_some());
        assert_file(&dest, "bin\\game.exe", &bytes(9, 2600));
        assert_eq!(sync::read_state(&dest).unwrap().version, "2.0");
    }

    #[tokio::test]
    async fn an_update_of_a_folder_with_no_saved_manifest_checks_its_files_and_fetches_only_the_differences() {
        let t = Tmp::new("nomanifest");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        for n in [MANIFEST_FILE, PREVIOUS_FILE] {
            let _ = std::fs::remove_file(own_file(&dest, n));
        }
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &game_v2(), &[]);
        fake.chunk_log.lock().clear();
        let (_, progress) = run(&fake, &dest, Mode::Update).await.unwrap();
        let mut want = chunk_names(&bytes(9, 2600));
        want.extend(chunk_names(&bytes(5, 700)));
        assert_eq!(fake.asked(), want);
        assert!(progress.iter().any(|p| p.step == "Checking files"));
        assert_file(&dest, "data\\level1.pak", &bytes(2, 1500));
        // Nothing says readme.txt was the game's, so it is left alone.
        assert!(dest.join("readme.txt").exists());
    }

    // ----- repairing -----

    #[tokio::test]
    async fn a_repair_rewrites_the_bad_and_missing_files_and_nothing_else() {
        let t = Tmp::new("repair");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        // One byte wrong in the middle of game.exe, readme.txt gone, level1.pak untouched.
        let exe = dest.join("bin").join("game.exe");
        let mut b = std::fs::read(&exe).unwrap();
        b[1700] ^= 0xFF;
        std::fs::write(&exe, &b).unwrap();
        std::fs::remove_file(dest.join("readme.txt")).unwrap();
        // A newer build exists, but a repair stays on the installed one.
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &game_v2(), &[]);
        fake.chunk_log.lock().clear();
        let (r, progress) = run(&fake, &dest, Mode::Repair).await.unwrap();
        let mut want = chunk_names(&bytes(1, 2500));
        want.extend(chunk_names(&bytes(3, 300)));
        assert_eq!(fake.asked(), want);
        assert_v1(&dest);
        assert_eq!((r.version.as_str(), r.files_written, r.files_repaired), ("1.0", 2, 2));
        assert_eq!(sync::read_state(&dest).unwrap().build, "100");
        assert!(progress.iter().any(|p| p.step == "Checking files" && p.total_bytes > 0));
        no_parts(&dest);
        // And a repair of a sound folder fetches nothing.
        fake.chunk_log.lock().clear();
        let (r, _) = run(&fake, &dest, Mode::Repair).await.unwrap();
        assert!(fake.chunk_log.lock().is_empty());
        assert_eq!((r.files_written, r.files_repaired), (0, 0));
    }

    #[tokio::test]
    async fn a_file_cut_short_is_repaired_too() {
        let t = Tmp::new("short");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        std::fs::write(dest.join("data").join("level1.pak"), &bytes(2, 1500)[..900]).unwrap();
        let (r, _) = run(&fake, &dest, Mode::Repair).await.unwrap();
        assert_eq!(r.files_repaired, 1);
        assert_v1(&dest);
    }

    // ----- refusals -----

    #[tokio::test]
    async fn paths_that_leave_the_game_folder_refuse_the_whole_build() {
        for bad in ["../evil.txt", "..\\evil.txt", "a/../../evil.txt", "C:\\Windows\\evil.dll", ".u3d\\game.json", ".U3D\\undo\\undo.json"] {
            let t = Tmp::new("dots");
            let dest = t.game();
            let fake = Fake::new();
            let files = vec![tf("fine.txt", 1, 100), tf(bad, 2, 100)];
            publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files, dirs: vec![] }], &[]);
            let err = run(&fake, &dest, Mode::Install).await.unwrap_err();
            assert!(format!("{err:#}").contains("not allowed"), "{bad}: {err:#}");
            assert!(!t.0.join("evil.txt").exists(), "{bad}");
            assert!(!dest.join("fine.txt").exists(), "nothing of a refused build is written ({bad})");
            assert!(sync::read_state(&dest).is_none());
            assert!(fake.chunk_log.lock().is_empty(), "nothing is downloaded for a refused build ({bad})");
        }
        // A folder path and a link path are checked as well.
        for (dirs, label) in [(vec!["../up"], "dir"), (vec!["ok", "C:\\x"], "dir2")] {
            let t = Tmp::new("dotdir");
            let fake = Fake::new();
            publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files: vec![tf("a.txt", 1, 10)], dirs }], &[]);
            assert!(run(&fake, &t.game(), Mode::Install).await.is_err(), "{label}");
        }
    }

    #[tokio::test]
    async fn a_chunk_that_does_not_match_its_md5_is_refused_and_nothing_is_installed() {
        // The bytes on the CDN are not the ones the manifest names.
        let t = Tmp::new("badpacked");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        let victim = chunk_names(&bytes(2, 1500)).into_iter().next().unwrap();
        let key = fake.files.lock().keys().find(|k| k.ends_with(&victim)).cloned().unwrap();
        fake.files.lock().insert(key, miniz_oxide::deflate::compress_to_vec_zlib(b"something else entirely", 6));
        let err = run(&fake, &dest, Mode::Install).await.unwrap_err();
        assert!(format!("{err:#}").contains("did not match its checksum"), "{err:#}");
        assert!(sync::read_state(&dest).is_none());
        assert!(!dest.join("bin").join("game.exe").exists() && !dest.join("data").join("level1.pak").exists());
        no_parts(&dest);
        assert_eq!(fake.chunk_log.lock().iter().filter(|m| **m == victim).count(), cfg().tries, "asked again before giving up");

        // The packed bytes are right but what they unpack to is not what the manifest says.
        let t = Tmp::new("badplain");
        let fake = Fake::new();
        let mut f = tf("x.bin", 3, 1500);
        f.wrong_md5 = true;
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files: vec![f], dirs: vec![] }], &[]);
        let err = run(&fake, &t.game(), Mode::Install).await.unwrap_err();
        assert!(format!("{err:#}").contains("once unpacked"), "{err:#}");
        assert!(sync::read_state(&t.game()).is_none() && !t.game().join("x.bin").exists());
        no_parts(&t.game());
    }

    #[test]
    fn a_chunk_is_unpacked_only_to_what_its_manifest_says() {
        let plain = bytes(3, 900);
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(&plain, 6);
        let chunk = |size: u64, md5: &str| Chunk { md5: md5.into(), compressed_md5: md5_hex(&packed), size, compressed_size: packed.len() as u64 };
        assert_eq!(unpack(&packed, &chunk(900, &md5_hex(&plain))).unwrap(), plain);
        // The manifest says it is shorter or longer than it is, or names another md5.
        assert!(unpack(&packed, &chunk(899, &md5_hex(&plain))).is_err());
        assert!(unpack(&packed, &chunk(901, &md5_hex(&plain))).is_err());
        assert!(unpack(&packed, &chunk(900, &md5_hex(b"other"))).is_err());
        // The packed bytes are not the ones the manifest names.
        let mut other = chunk(900, &md5_hex(&plain));
        other.compressed_md5 = md5_hex(b"other");
        assert!(unpack(&packed, &other).is_err());
        // A packed bomb is stopped at the size the manifest gives.
        let bomb = miniz_oxide::deflate::compress_to_vec_zlib(&vec![0u8; 10_000_000], 6);
        let c = Chunk { md5: md5_hex(&vec![0u8; 900]), compressed_md5: md5_hex(&bomb), size: 900, compressed_size: bomb.len() as u64 };
        assert!(unpack(&bomb, &c).is_err());
    }

    // ----- generation 1 -----

    struct V1Item {
        path: &'static str,
        data: Vec<u8>,
        support: bool,
    }

    fn v1(path: &'static str, seed: u8, len: usize) -> V1Item {
        V1Item { path, data: bytes(seed, len), support: false }
    }

    /// Publishes a generation 1 build: one `main.bin` for the product (equal files share a place
    /// in it), a depot per language and the repository. `depots` is (language, files).
    fn publish_v1(fake: &Fake, build_id: &str, date: &str, depots: &[(&str, Vec<V1Item>)], dirs: &[&str]) {
        let dir = format!("https://gog-cdn-fastly.gog.com/content-system/v1/manifests/{PID}/windows/{TS}");
        // A newer build adds to the same main.bin, so the older build's ranges stay valid.
        let blob_key = format!("{V1STORE}/{PID}/windows/{TS}/main.bin");
        let mut blob: Vec<u8> = fake.files.lock().get(&blob_key).cloned().unwrap_or_default();
        let mut placed: HashMap<String, usize> = HashMap::new();
        let mut depot_list = Vec::new();
        for (i, (lang, items)) in depots.iter().enumerate() {
            let mut files = Vec::new();
            for it in items {
                let key = md5_hex(&it.data);
                let offset = *placed.entry(key.clone()).or_insert_with(|| {
                    let at = blob.len();
                    blob.extend_from_slice(&it.data);
                    at
                });
                let mut f = json!({"offset": offset, "path": format!("/{}", it.path), "size": it.data.len(), "hash": key, "url": format!("{PID}/main.bin")});
                if it.support {
                    f["support"] = json!(true);
                }
                files.push(f);
            }
            if i == 0 {
                for d in dirs {
                    files.push(json!({"path": format!("/{d}"), "size": 0, "directory": true, "url": format!("{PID}/main.bin"), "offset": 0}));
                }
            }
            let name = format!("{i:08x}-0000-4000-8000-000000000000.json");
            fake.files.lock().insert(format!("{dir}/{name}"), json!({"version": 1, "depot": {"name": "Test Game", "files": files}}).to_string().into_bytes());
            depot_list.push(json!({"languages": [lang], "manifest": name, "gameIDs": [PID.to_string()], "size": "1", "systems": ["Windows"]}));
        }
        depot_list.push(json!({"redist": "XNA_40", "executable": "__redist/XNA_40/xnafx40_redist.msi", "argument": "/quiet"}));
        let repo = json!({"product": {"rootGameID": PID.to_string(), "timestamp": TS.parse::<u64>().unwrap(), "depots": depot_list, "installDirectory": "Test Game", "projectName": "Test Game"}, "version": 1});
        let link = format!("{dir}/repository.json");
        fake.files.lock().insert(link.clone(), format!("\n{repo}").into_bytes());
        fake.files.lock().insert(blob_key, blob);
        fake.builds.lock().insert(0, json!({"build_id": build_id, "product_id": PID.to_string(), "os": "windows", "branch": null, "version_name": "", "tags": [], "public": true, "date_published": date, "generation": 1, "link": link, "legacy_build_id": TS.parse::<u64>().unwrap()}));
    }

    fn v1_game() -> Vec<(&'static str, Vec<V1Item>)> {
        let mut support = v1("galaxy_setup.exe", 7, 100);
        support.support = true;
        vec![
            ("Neutral", vec![v1("Data/a.bin", 1, 2500), v1("readme.txt", 3, 300), v1("Data/copy_of_a.bin", 1, 2500), support, V1Item { path: "empty.txt", data: vec![], support: false }]),
            ("English", vec![v1("voice_en.bin", 4, 1200)]),
            ("German", vec![v1("voice_de.bin", 5, 1200)]),
        ]
    }

    #[tokio::test]
    async fn a_generation_1_game_is_installed_from_ranges_of_its_main_bin() {
        let t = Tmp::new("gen1");
        let dest = t.game();
        let fake = Fake::new();
        publish_v1(&fake, "3161", "2016-01-01T10:00:00+0000", &v1_game(), &["saves"]);
        let (r, _) = run(&fake, &dest, Mode::Install).await.unwrap();
        assert_file(&dest, "Data\\a.bin", &bytes(1, 2500));
        assert_file(&dest, "Data\\copy_of_a.bin", &bytes(1, 2500));
        assert_file(&dest, "readme.txt", &bytes(3, 300));
        assert_file(&dest, "voice_en.bin", &bytes(4, 1200));
        assert_file(&dest, "empty.txt", b"");
        assert_file(&dest, "gog-support\\4242\\galaxy_setup.exe", &bytes(7, 100));
        assert!(!dest.join("voice_de.bin").exists(), "the language of the user only");
        assert!(dest.join("saves").is_dir());
        // The shared file is asked for once: 3 pieces of a.bin, 1 of readme, 1 of the support file, 2 of the voice file.
        assert_eq!(fake.chunk_log.lock().len(), 7);
        assert!(fake.chunk_log.lock().iter().all(|n| n.starts_with("range:")));
        let s = sync::read_state(&dest).unwrap();
        assert_eq!((s.version.as_str(), s.build.as_str()), ("3161", "3161"), "no version name: the build id");
        assert_eq!(r.files_written, 6);
        assert_eq!(dependencies(&dest), vec!["XNA_40".to_string()]);
        let kept = std::fs::read_to_string(own_file(&dest, MANIFEST_FILE)).unwrap();
        assert!(kept.contains("\"generation\":1") && kept.contains(TS));
        no_parts(&dest);
    }

    #[tokio::test]
    async fn a_generation_1_game_updates_and_repairs_by_the_md5_of_its_files() {
        let t = Tmp::new("gen1up");
        let dest = t.game();
        let fake = Fake::new();
        publish_v1(&fake, "3161", "2016-01-01T10:00:00+0000", &v1_game(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        // A newer build: a.bin changed, the rest the same.
        let mut v2 = v1_game();
        v2[0].1[0] = v1("Data/a.bin", 9, 2500);
        v2[0].1.remove(2);
        publish_v1(&fake, "3200", "2016-02-01T10:00:00+0000", &v2, &[]);
        fake.chunk_log.lock().clear();
        let (r, _) = run(&fake, &dest, Mode::Update).await.unwrap();
        assert_eq!(fake.chunk_log.lock().len(), 3, "only the 3 pieces of the changed file");
        assert_eq!(r.files_written, 1);
        assert_file(&dest, "Data\\a.bin", &bytes(9, 2500));
        assert!(!dest.join("Data").join("copy_of_a.bin").exists(), "a file the new build dropped is gone");
        assert_file(&dest, "readme.txt", &bytes(3, 300));
        // Put the update back, then break a file and a repair of the installed build fixes only it.
        sync::undo(&dest).unwrap();
        assert_file(&dest, "Data\\copy_of_a.bin", &bytes(1, 2500));
        let mut b = std::fs::read(dest.join("readme.txt")).unwrap();
        b[10] ^= 1;
        std::fs::write(dest.join("readme.txt"), &b).unwrap();
        fake.chunk_log.lock().clear();
        let (r, _) = run(&fake, &dest, Mode::Repair).await.unwrap();
        assert_eq!(r.files_repaired, 1);
        assert_eq!(fake.chunk_log.lock().len(), 1);
        assert_file(&dest, "readme.txt", &bytes(3, 300));
        assert_file(&dest, "Data\\a.bin", &bytes(1, 2500));
    }

    #[tokio::test]
    async fn a_generation_1_file_that_does_not_match_its_md5_is_refused() {
        let t = Tmp::new("gen1bad");
        let dest = t.game();
        let fake = Fake::new();
        publish_v1(&fake, "3161", "2016-01-01T10:00:00+0000", &v1_game(), &[]);
        // The main.bin on the CDN holds other bytes than the manifest says.
        let key = format!("{V1STORE}/{PID}/windows/{TS}/main.bin");
        let mut blob = fake.files.lock().get(&key).cloned().unwrap();
        blob[1200] ^= 0xFF;
        fake.files.lock().insert(key, blob);
        let err = run(&fake, &dest, Mode::Install).await.unwrap_err();
        assert!(format!("{err:#}").contains("did not come out right"), "{err:#}");
        assert!(sync::read_state(&dest).is_none() && !dest.join("readme.txt").exists());
        no_parts(&dest);
    }

    #[tokio::test]
    async fn a_generation_1_game_with_no_checksum_for_a_file_is_refused() {
        let t = Tmp::new("gen1nohash");
        let fake = Fake::new();
        publish_v1(&fake, "3161", "2016-01-01T10:00:00+0000", &v1_game(), &[]);
        let key = fake.files.lock().keys().find(|k| k.ends_with("00000000-0000-4000-8000-000000000000.json")).cloned().unwrap();
        let text = String::from_utf8(fake.files.lock().get(&key).cloned().unwrap()).unwrap();
        let mut v: Value = serde_json::from_str(&text).unwrap();
        v["depot"]["files"][0].as_object_mut().unwrap().remove("hash");
        fake.files.lock().insert(key, v.to_string().into_bytes());
        let err = run(&fake, &t.game(), Mode::Install).await.unwrap_err();
        assert!(format!("{err:#}").contains("no checksum"), "{err:#}");
    }

    #[tokio::test]
    async fn not_enough_free_space_stops_before_anything_is_downloaded() {
        let t = Tmp::new("space");
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        let mut c = cfg();
        c.space = Some((1_000, 10_000));
        let err = sync_via(&fake, &c, PID, "Test Game", &t.game(), Mode::Install, "en-US", &Options::default(), &|_| {}, &AtomicBool::new(false)).await.unwrap_err();
        assert!(err.to_string().contains("not enough free space"), "{err:#}");
        assert!(fake.chunk_log.lock().is_empty());
        // With room it goes through.
        c.space = Some((100 << 30, 200 << 30));
        sync_via(&fake, &c, PID, "Test Game", &t.game(), Mode::Install, "en-US", &Options::default(), &|_| {}, &AtomicBool::new(false)).await.unwrap();
    }

    // ----- cancel, failure, retries -----

    #[tokio::test(flavor = "multi_thread", worker_threads = 3)]
    async fn a_canceled_update_keeps_the_old_build_working() {
        let t = Tmp::new("cancel");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        let undo_before = sync::can_undo(&dest);
        // Build 2 changes a lot; the cancel comes after the second chunk, while the rest is on its way.
        let mut v2 = game_v2();
        v2[0].files.push(tf("data\\huge.pak", 12, 12_000));
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &v2, &[]);
        let cancel = Arc::new(AtomicBool::new(false));
        *fake.cancel_after.lock() = Some((2, cancel.clone()));
        *fake.delay.lock() = Duration::from_millis(15);
        fake.chunk_log.lock().clear();
        let err = run_with(&fake, &dest, Mode::Update, "en-US", &Options::default(), &cancel).await.unwrap_err();
        assert!(err.to_string().contains("CANCELED"), "{err:#}");
        assert!(fake.chunk_log.lock().len() < 16, "the download stopped early");
        assert_v1(&dest);
        let s = sync::read_state(&dest).unwrap();
        assert_eq!((s.version.as_str(), s.build.as_str()), ("1.0", "100"));
        assert!(!dest.join("data").join("level2.pak").exists() && !dest.join("data").join("huge.pak").exists());
        assert_eq!(sync::can_undo(&dest), undo_before, "the undo of the install is still there");
        no_parts(&dest);
        // The old build still updates fine afterwards.
        *fake.cancel_after.lock() = None;
        *fake.delay.lock() = Duration::ZERO;
        run(&fake, &dest, Mode::Update).await.unwrap();
        assert_file(&dest, "data\\huge.pak", &bytes(12, 12_000));
    }

    #[tokio::test]
    async fn a_cancel_before_the_start_does_nothing() {
        let t = Tmp::new("cancel0");
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        let err = run_with(&fake, &t.game(), Mode::Install, "en-US", &Options::default(), &AtomicBool::new(true)).await.unwrap_err();
        assert!(err.to_string().contains("CANCELED"));
        assert!(fake.chunk_log.lock().is_empty() && !t.game().exists());
    }

    #[tokio::test]
    async fn a_failure_after_files_were_put_in_place_restores_them() {
        let t = Tmp::new("restore");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &game_v2(), &[]);
        // level2.pak cannot be put in place: a folder with its name is in the way.
        std::fs::create_dir_all(dest.join("data").join("level2.pak")).unwrap();
        let err = run(&fake, &dest, Mode::Update).await.unwrap_err();
        assert!(format!("{err:#}").contains("level2.pak"), "{err:#}");
        assert_v1(&dest);
        assert_eq!(sync::read_state(&dest).unwrap().version, "1.0");
        no_parts(&dest);
    }

    #[tokio::test]
    async fn a_chunk_that_fails_once_is_asked_for_again() {
        let t = Tmp::new("flaky");
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        let victim = chunk_names(&bytes(1, 2500)).into_iter().next().unwrap();
        fake.flaky.lock().insert(victim.clone(), 1);
        run(&fake, &t.game(), Mode::Install).await.unwrap();
        assert_v1(&t.game());
        assert_eq!(fake.chunk_log.lock().iter().filter(|m| **m == victim).count(), 2);
    }

    #[tokio::test]
    async fn run_out_download_addresses_are_asked_for_again() {
        let t = Tmp::new("expired");
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        // The first addresses carry a token the CDN no longer takes; the second ones work.
        *fake.tokens.lock() = vec!["old".into(), "ok".into()];
        run(&fake, &t.game(), Mode::Install).await.unwrap();
        assert_v1(&t.game());
        assert_eq!(fake.link_asks.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_download_with_progress_reports_while_it_runs() {
        let t = Tmp::new("progress");
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &[TDepot { product: PID, langs: vec!["*"], files: vec![tf("big.bin", 5, 12_000)], dirs: vec![] }], &[]);
        *fake.delay.lock() = Duration::from_millis(25);
        let mut c = cfg();
        c.fetchers = 2;
        c.report = Duration::from_millis(30);
        let seen = parking_lot::Mutex::new(Vec::new());
        sync_via(&fake, &c, PID, "Test Game", &t.game(), Mode::Install, "en-US", &Options::default(), &|p| seen.lock().push(p), &AtomicBool::new(false)).await.unwrap();
        let downloading: Vec<Progress> = seen.into_inner().into_iter().filter(|p| p.step == "Downloading").collect();
        assert!(downloading.len() >= 4, "{} progress reports", downloading.len());
        assert!(downloading.windows(2).all(|w| w[0].done_bytes <= w[1].done_bytes));
    }

    // ----- builds, depots, languages -----

    const CYBERPUNK: &str = r#"{"total_count":265,"count":6,"items":[
        {"build_id":"60152385347545148","product_id":"1423049311","os":"windows","branch":null,"version_name":"2.5_RedModFix","public":true,"date_published":"2026-10-22T15:28:42+0000","generation":2,"link":"https://gog-cdn-fastly-cp77.gog.com/content-system/v2/meta/18/32/1832ea3a875f806203b92644e9edb617"},
        {"build_id":"60148683633405433","product_id":"1423049311","os":"windows","branch":null,"version_name":"2.5","public":true,"date_published":"2026-10-21T08:54:08+0000","generation":2,"link":"https://gog-cdn-fastly-cp77.gog.com/content-system/v2/meta/4e/4d/4e4d1ffdf6d9f0db3bff8644e753c479"},
        {"build_id":"60300000000000000","product_id":"1423049311","os":"windows","branch":"beta","version_name":"2.6 beta","public":true,"date_published":"2026-10-23T08:54:08+0000","generation":2,"link":"https://x.gog.com/b"},
        {"build_id":"60400000000000000","product_id":"1423049311","os":"windows","branch":null,"version_name":"3.0 hidden","public":false,"date_published":"2026-10-24T08:54:08+0000","generation":2,"link":"https://x.gog.com/c"},
        {"build_id":"56624919703786522","product_id":"1423049311","os":"windows","branch":"1_63_legacy_patch","version_name":"1.63_Hotfix","public":true,"date_published":"2023-06-23T21:32:34+0000","generation":2,"link":"https://x.gog.com/d"}]}"#;

    #[test]
    fn the_newest_public_build_of_the_default_branch_is_picked_unless_another_is_asked_for() {
        let b = parse_builds(CYBERPUNK.as_bytes()).unwrap();
        assert_eq!(b.len(), 4, "a private build is left out");
        let newest = pick_build(&b, Pick::Newest).unwrap();
        assert_eq!((newest.id.as_str(), newest.version.as_str(), newest.branch.clone()), ("60152385347545148", "2.5_RedModFix", None));
        assert_eq!(pick_build(&b, Pick::Branch("beta")).unwrap().version, "2.6 beta");
        assert_eq!(pick_build(&b, Pick::Id("60148683633405433")).unwrap().version, "2.5");
        assert_eq!(pick_build(&b, Pick::Id("1")), None);
        assert_eq!(pick_build(&b, Pick::Branch("nope")), None);
        assert!(parse_builds(b"not json").is_err());
    }

    #[test]
    fn a_generation_2_build_is_taken_over_a_generation_1_one() {
        let json = r#"{"items":[
            {"build_id":"3161","branch":null,"version_name":"","public":true,"date_published":"2016-01-01T00:00:00+0000","generation":1,"link":"https://x.gog.com/repository.json"},
            {"build_id":"500","branch":null,"version_name":"1.0","public":true,"date_published":"2015-01-01T00:00:00+0000","generation":2,"link":"https://x.gog.com/m"}]}"#;
        let b = parse_builds(json.as_bytes()).unwrap();
        assert_eq!(pick_build(&b, Pick::Newest).unwrap().id, "500");
        // With only generation 1, that one is named (and refused later); the version is the build id.
        let only1 = &b[..1];
        let p = pick_build(only1, Pick::Newest).unwrap();
        assert_eq!((p.generation, p.version_or_id()), (1, "3161".to_string()));
    }

    #[test]
    fn languages_are_chosen_like_galaxy_does_with_english_then_anything_as_fallbacks() {
        let langs = |l: &[&str]| l.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let all = langs(&["en-US", "pl-PL", "de-DE", "zh-Hans", "pt-BR", "*"]);
        assert_eq!(pick_language(&all, "de-DE").as_deref(), Some("de-de"));
        assert_eq!(pick_language(&all, "German").as_deref(), Some("de-de"));
        assert_eq!(pick_language(&all, "de").as_deref(), Some("de-de"));
        assert_eq!(pick_language(&all, "zh_Hans").as_deref(), Some("zh-hans"));
        assert_eq!(pick_language(&all, "Brazilian Portuguese").as_deref(), Some("pt-br"));
        // Not there: English.
        assert_eq!(pick_language(&all, "ja-JP").as_deref(), Some("en-us"));
        assert_eq!(pick_language(&all, "").as_deref(), Some("en-us"));
        // No English either: the first the build has.
        assert_eq!(pick_language(&langs(&["fr-FR", "de-DE"]), "ja").as_deref(), Some("fr-fr"));
        // Old builds write "en" for it; "English" and "Neutral" are old names too.
        assert_eq!(pick_language(&langs(&["en"]), "en-US").as_deref(), Some("en"));
        assert_eq!(pick_language(&langs(&["English", "Neutral"]), "en-US").as_deref(), Some("en"));
        assert_eq!(pick_language(&langs(&["*"]), "en-US"), None);
    }

    fn depot(product: &str, langs: &[&str], bits: &[&str], manifest: &str) -> Depot {
        Depot { manifest: manifest.into(), product_id: product.into(), languages: langs.iter().map(|s| s.to_string()).collect(), size: 1, compressed_size: 1, bitness: bits.iter().map(|s| s.to_string()).collect(), gog_depot: false }
    }

    #[test]
    fn depots_are_picked_by_product_language_and_bitness() {
        let m = BuildManifest {
            build_id: "1".into(),
            product_id: "10".into(),
            install_dir: "G".into(),
            dependencies: vec![],
            tasks: vec![],
            depots: vec![
                depot("10", &["en-US", "de-DE", "fr-FR"], &["64"], "a"),
                depot("10", &["de-DE"], &["64"], "b"),
                depot("10", &["fr-FR"], &["64"], "c"),
                depot("10", &["*"], &["64"], "d"),
                depot("10", &["*"], &["32"], "e"),
                depot("20", &["*"], &[], "f"),
            ],
        };
        fn names<'a>(ds: &[&'a Depot]) -> Vec<&'a str> {
            ds.iter().map(|d| d.manifest.as_str()).collect()
        }
        let only10: HashSet<String> = ["10".to_string()].into();
        let both: HashSet<String> = ["10".to_string(), "20".to_string()].into();
        let (d, l) = pick_depots(&m, "de-DE", &only10);
        assert_eq!((names(&d), l.as_deref()), (vec!["a", "b", "d"], Some("de-de")));
        let (d, l) = pick_depots(&m, "en-US", &both);
        assert_eq!((names(&d), l.as_deref()), (vec!["a", "d", "f"], Some("en-us")));
        // Spanish is not there: English, with the depots for every language.
        let (d, _) = pick_depots(&m, "es-ES", &only10);
        assert_eq!(names(&d), vec!["a", "d"]);
        // A product that has only 32 bit depots gets them.
        let m32 = BuildManifest { depots: vec![depot("10", &["*"], &["32"], "e")], ..m.clone() };
        assert_eq!(names(&pick_depots(&m32, "en", &only10).0), vec!["e"]);
    }

    #[tokio::test]
    async fn the_depots_for_the_users_language_are_installed() {
        let fake = Fake::new();
        let depots = vec![
            TDepot { product: PID, langs: vec!["*"], files: vec![tf("common.bin", 1, 1100)], dirs: vec![] },
            TDepot { product: PID, langs: vec!["en-US"], files: vec![tf("voice_en.pak", 2, 1100)], dirs: vec![] },
            TDepot { product: PID, langs: vec!["de-DE"], files: vec![tf("voice_de.pak", 3, 1100)], dirs: vec![] },
        ];
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &depots, &[]);
        for (lang, has_en, has_de) in [("de-DE", false, true), ("English", true, false), ("fr-FR", true, false)] {
            let t = Tmp::new("lang");
            let dest = t.game();
            run_with(&fake, &dest, Mode::Install, lang, &Options::default(), &AtomicBool::new(false)).await.unwrap();
            assert_file(&dest, "common.bin", &bytes(1, 1100));
            assert_eq!((dest.join("voice_en.pak").exists(), dest.join("voice_de.pak").exists()), (has_en, has_de), "{lang}");
        }
    }

    // ----- add-ons and ownership -----

    fn with_dlc() -> Vec<TDepot> {
        vec![
            TDepot { product: PID, langs: vec!["*"], files: vec![tf("base.bin", 1, 1100)], dirs: vec![] },
            TDepot { product: DLC, langs: vec!["*"], files: vec![tf("dlc\\extra.pak", 2, 1100)], dirs: vec![] },
        ]
    }

    #[tokio::test]
    async fn add_ons_the_account_has_are_installed_and_kept_by_later_updates() {
        let t = Tmp::new("dlc");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &with_dlc(), &[]);
        run(&fake, &dest, Mode::Install).await.unwrap();
        assert_file(&dest, "dlc\\extra.pak", &bytes(2, 1100));
        // A later build; GOG now refuses the add-on's addresses (a hiccup, or a refund): its files stay.
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &[with_dlc().remove(0), TDepot { product: DLC, langs: vec!["*"], files: vec![tf("dlc\\extra.pak", 2, 1100)], dirs: vec![] }], &[]);
        fake.denied.lock().insert(DLC);
        run(&fake, &dest, Mode::Update).await.unwrap();
        assert_file(&dest, "dlc\\extra.pak", &bytes(2, 1100));
        assert_eq!(sync::read_state(&dest).unwrap().version, "2.0");
    }

    #[tokio::test]
    async fn add_ons_the_account_does_not_have_are_left_out_and_so_are_all_when_asked() {
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &with_dlc(), &[]);
        fake.denied.lock().insert(DLC);
        let t = Tmp::new("nodlc");
        run(&fake, &t.game(), Mode::Install).await.unwrap();
        assert_file(&t.game(), "base.bin", &bytes(1, 1100));
        assert!(!t.game().join("dlc").exists());
        fake.denied.lock().clear();
        let t2 = Tmp::new("nodlc2");
        run_with(&fake, &t2.game(), Mode::Install, "en-US", &Options { no_dlc: true, ..Default::default() }, &AtomicBool::new(false)).await.unwrap();
        assert!(t2.game().join("base.bin").exists() && !t2.game().join("dlc").exists());
    }

    #[tokio::test]
    async fn a_game_the_account_does_not_own_says_so() {
        let t = Tmp::new("denied");
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        fake.denied.lock().insert(PID);
        let err = run(&fake, &t.game(), Mode::Install).await.unwrap_err();
        assert!(err.to_string().contains("Sign in to GOG again"), "{err:#}");
        assert!(!t.game().exists() || sync::read_state(&t.game()).is_none());
    }

    #[tokio::test]
    async fn a_given_build_is_installed_instead_of_the_newest() {
        let t = Tmp::new("pick");
        let dest = t.game();
        let fake = Fake::new();
        publish(&fake, "100", "1.0", "2026-10-01T10:00:00+0000", &game_v1(), &[]);
        publish(&fake, "200", "2.0", "2026-10-02T10:00:00+0000", &game_v2(), &[]);
        run_with(&fake, &dest, Mode::Install, "en-US", &Options { build: Some("100".into()), ..Default::default() }, &AtomicBool::new(false)).await.unwrap();
        assert_v1(&dest);
        assert_eq!(sync::read_state(&dest).unwrap().build, "100");
        let err = run_with(&fake, &dest, Mode::Update, "en-US", &Options { build: Some("999".into()), ..Default::default() }, &AtomicBool::new(false)).await.unwrap_err();
        assert!(err.to_string().contains("does not list build 999"), "{err:#}");
    }

    // ----- parsing -----

    #[test]
    fn a_depot_manifest_is_read_with_files_folders_links_and_the_container() {
        let c = |n: &str| json!({"compressedMd5": md5_hex(n.as_bytes()), "compressedSize": 5, "md5": md5_hex(format!("p{n}").as_bytes()), "size": 100});
        let v = json!({"version": 2, "depot": {
            "smallFilesContainer": {"chunks": [c("sfc")]},
            "items": [
                {"type": "DepotFile", "path": "a\\b.exe", "chunks": [c("1"), c("2")], "flags": ["executable"], "md5": md5_hex(b"x"), "sha256": "ab".repeat(32)},
                {"type": "DepotFile", "path": "tiny.txt", "chunks": [c("3")], "sfcRef": {"offset": 40, "size": 100}},
                {"type": "DepotFile", "path": "empty", "chunks": []},
                {"type": "DepotDirectory", "path": "saves"},
                {"type": "DepotLink", "path": "lnk", "target": "a/b.exe"}]}});
        for raw in [zlib(&v), v.to_string().into_bytes()] {
            let m = parse_depot_manifest(&raw).unwrap();
            assert_eq!((m.files.len(), m.dirs.clone(), m.links.len(), m.sfc.len()), (3, vec!["saves".to_string()], 1, 1));
            assert_eq!((m.files[0].size, m.files[0].flags.clone(), m.files[0].md5.is_some(), m.files[0].sha256.is_some()), (200, vec!["executable".to_string()], true, true));
            assert_eq!((m.files[1].size, m.files[1].sfc), (100, Some((40, 100))));
            assert_eq!(m.files[2].size, 0);
        }
        // Things that cannot be trusted refuse the depot.
        let mut bad = v.clone();
        bad["depot"]["items"][0]["chunks"][0]["md5"] = json!("zz");
        assert!(parse_depot_manifest(&zlib(&bad)).is_err());
        let mut unknown = v.clone();
        unknown["depot"]["items"][3]["type"] = json!("DepotSomethingNew");
        assert!(parse_depot_manifest(&zlib(&unknown)).is_err());
        let mut packed_wrong = v.clone();
        packed_wrong["depot"]["items"][1]["sfcRef"]["size"] = json!(99);
        assert!(parse_depot_manifest(&zlib(&packed_wrong)).is_err());
        let mut huge = v.clone();
        huge["depot"]["items"][0]["chunks"][0]["size"] = json!(u64::MAX / 2);
        assert!(parse_depot_manifest(&zlib(&huge)).is_err());
        assert!(parse_depot_manifest(b"garbage").is_err());
    }

    #[test]
    fn a_build_manifest_keeps_depots_and_dependencies_and_not_the_client_keys() {
        let hash = "e7126b6c0cd4550a4b850dc1f5aa23ca";
        let v = json!({"baseProductId": "1423049311", "buildId": "6015", "clientId": "1", "clientSecret": "s", "dependencies": ["MSVC2015", "MSVC2013_x64"], "installDirectory": "Cyberpunk 2077",
            "depots": [{"compressedSize": 5, "languages": ["en-US", "de-DE"], "manifest": hash, "osBitness": ["64"], "productId": "1423049311", "size": 9}, {"languages": ["*"], "manifest": hash, "productId": "1597316373", "isGogDepot": true, "size": "7"}],
            "products": [{"name": "C", "productId": "1423049311", "temp_arguments": "-x", "temp_executable": "bin\\c.exe"}], "version": 2});
        let m = parse_build_manifest(&zlib(&v)).unwrap();
        assert_eq!((m.product_id.as_str(), m.build_id.as_str(), m.install_dir.as_str()), ("1423049311", "6015", "Cyberpunk 2077"));
        assert_eq!(m.dependencies, vec!["MSVC2015".to_string(), "MSVC2013_x64".to_string()]);
        assert_eq!((m.depots.len(), m.depots[0].bitness.clone(), m.depots[1].gog_depot, m.depots[1].size), (2, vec!["64".to_string()], true, 7));
        assert_eq!(m.tasks[0], ProductTask { product_id: "1423049311".into(), exe: "bin\\c.exe".into(), args: "-x".into() });
        let mut v3 = v.clone();
        v3["version"] = json!(3);
        assert!(parse_build_manifest(&zlib(&v3)).is_err());
        let mut nohash = v.clone();
        nohash["depots"][0]["manifest"] = json!("../../x");
        assert!(parse_build_manifest(&zlib(&nohash)).is_err());
    }

    #[test]
    fn the_generation_1_formats_seen_on_gogs_servers_are_read() {
        let repo = r#"{"product":{"rootGameID":"1453375253","timestamp":64924992,"gameIDs":[{"standalone":true,"name":{"en":"Stardew Valley"},"gameID":"1453375253","dependencies":[]}],
            "depots":[{"languages":["Neutral"],"manifest":"bd1d0fc3-d5cf-4a1c-8124-0e5068c33a68.json","gameIDs":["1453375253"],"size":"1264696","systems":["Windows"]},
            {"languages":["English"],"manifest":"d974f986-1679-4562-8b31-ddcc6d7d793f.json","gameIDs":["1453375253"],"size":"389740549","systems":["Windows"]},
            {"redist":"XNA_40","executable":"__redist/XNA_40/xnafx40_redist.msi","argument":"/quiet"}],"installDirectory":"Stardew Valley","projectName":"Stardew Valley"},"version":1}"#;
        let r = parse_v1_repository(repo.as_bytes()).unwrap();
        assert_eq!((r.depots.len(), r.redists.clone(), r.install_dir.as_str(), r.timestamp.as_str()), (2, vec!["XNA_40".to_string()], "Stardew Valley", "64924992"));
        assert_eq!((r.depots[1].size, r.depots[1].languages.clone(), r.depots[1].game_ids.clone(), r.depots[1].systems.clone()), (389740549, vec!["English".to_string()], vec!["1453375253".to_string()], vec!["Windows".to_string()]));
        let depot = r#"{"version":1,"depot":{"name":"Stardew Valley","files":[
            {"offset":0,"path":"1453375253/galaxy.exe","size":1264696,"support":true,"hash":"5f2a1c44693eda3c80f72b3eed29449a","url":"1453375253/main.bin"},
            {"offset":384862540,"hash":"eddf3ff02539dcef1dba524bfc3675ea","url":"1453375253/main.bin","path":"/Content/Characters/Grandpa.xnb","size":191}]}}"#;
        let files = parse_v1_depot(depot.as_bytes()).unwrap();
        assert_eq!((files.len(), files[0].support, files[1].offset, files[1].size, files[1].path.as_str()), (2, true, 384862540, 191, "/Content/Characters/Grandpa.xnb"));
        // In the form the engine works with: the product named in the address owns the file.
        let m = v1_manifest(files, "1453375253").unwrap();
        assert_eq!((m.files.len(), m.files[0].flags.clone(), m.files[1].v1.clone()), (2, vec!["support".to_string()], Some(V1Ref { product: "1453375253".into(), offset: 384862540 })));
        assert_eq!(rel_of(&m.files[1].path, &m.files[1].flags, "1453375253").unwrap(), PathBuf::from("Content\\Characters\\Grandpa.xnb"));
    }

    // ----- download addresses -----

    fn ep(format: &str, params: &[(&str, &str)]) -> Endpoint {
        Endpoint { url_format: format.into(), params: params.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), max_age: 0 }
    }

    #[test]
    fn a_chunk_is_found_at_the_endpoints_template_after_its_path() {
        let g = galaxy_path("ABCDEF0123456789abcdef0123456789").unwrap();
        assert_eq!(g, "ab/cd/abcdef0123456789abcdef0123456789");
        // Path "/" with a slash around it, and a path that names the product's folder.
        let a = ep("{base_url}/{path}?{token}", &[("base_url", "https://cdn.example.com/content-system/v2/store/7"), ("path", "/"), ("token", "__token__=exp=1~hmac=2")]);
        assert_eq!(chunk_url(&a, &g).unwrap(), format!("https://cdn.example.com/content-system/v2/store/7/{g}?__token__=exp=1~hmac=2"));
        let b = ep("{base_url}{path}?{token}", &[("base_url", "https://cdn.example.com"), ("path", "/content-system/v2/store/7"), ("token", "t")]);
        assert_eq!(chunk_url(&b, &g).unwrap(), format!("https://cdn.example.com/content-system/v2/store/7/{g}?t"));
        let c = ep("{base_url}/{path}", &[("base_url", "https://cdn.example.com/store"), ("path", "/tok/en/")]);
        assert_eq!(chunk_url(&c, &g).unwrap(), format!("https://cdn.example.com/store/tok/en/{g}"));
        // A template that names something the endpoint did not give, or is not a web address, is refused.
        assert!(chunk_url(&ep("{base_url}/{nope}", &[("base_url", "https://x")]), &g).is_err());
        assert!(chunk_url(&ep("{base_url}/{path}", &[("base_url", "file:///c")]), &g).is_err());
        assert!(galaxy_path("../../etc").is_err());
    }

    #[test]
    fn the_download_addresses_are_read_in_the_order_gog_lists_them() {
        let body = json!({"product_id": 1, "urls": [
            {"endpoint_name": "a", "url_format": "{base_url}/{path}?{token}", "parameters": {"base_url": "https://a.example.com", "path": "/", "token": "x"}, "priority": 1},
            {"endpoint_name": "b", "url_format": "{base_url}/{path}", "parameters": {"base_url": "https://b.example.com", "path": "/"}, "priority": 2, "max_age": 600}]});
        let eps = parse_secure_link(body.to_string().as_bytes()).unwrap();
        assert_eq!((eps[0].url_format.contains("token"), eps[1].max_age), (true, 600));
        assert!(parse_secure_link(b"{\"urls\":[]}").is_err());
        assert!(parse_secure_link(b"nope").is_err());
    }

    #[test]
    fn slashes_in_an_address_are_tidied_but_not_the_scheme_or_the_query() {
        assert_eq!(tidy_url("https://h.com/a//b///c?x=//y"), "https://h.com/a/b/c?x=//y");
        assert_eq!(tidy_url("https://h.com"), "https://h.com");
    }

    #[test]
    fn the_token_goes_only_to_the_content_systems_address_requests() {
        assert!(sends_token("https://content-system.gog.com/products/1/secure_link?_version=2&generation=2&path=/"));
        for url in [
            "https://content-system.gog.com/products/1/os/windows/builds?generation=2",
            "https://cdn.gog.com/content-system/v2/meta/aa/bb/aabb",
            "https://gog-cdn-fastly.gog.com/content-system/v2/store/1/aa/bb/aabb?token",
            "https://content-system.gog.com.example.net/products/1/secure_link",
            "https://example.net/content-system.gog.com/secure_link",
        ] {
            assert!(!sends_token(url), "{url}");
        }
    }

    // ----- how the game is started -----

    #[test]
    fn arguments_are_split_like_a_command_line() {
        assert_eq!(split_args(r#"-windowed "two words" --x=1 """#), vec!["-windowed", "two words", "--x=1", ""]);
        assert!(split_args("   ").is_empty());
    }

    #[test]
    fn the_main_play_task_of_the_info_file_starts_the_game() {
        let t = Tmp::new("launch");
        let d = t.game();
        std::fs::create_dir_all(&d).unwrap();
        let tasks = vec![ProductTask { product_id: "9".into(), exe: "from_manifest.exe".into(), args: "-m".into() }];
        // No info file: the manifest's entry.
        assert_eq!(launch_of(&d, "9", "en-US", &tasks), ("from_manifest.exe".to_string(), vec!["-m".to_string()]));
        // The info file wins; a task for another language is passed over; a byte order mark is fine.
        let info = json!({"playTasks": [
            {"category": "tool", "name": "Config", "path": "config.exe", "type": "FileTask"},
            {"category": "game", "isPrimary": true, "languages": ["de-DE"], "path": "de.exe", "type": "FileTask"},
            {"category": "game", "isPrimary": true, "languages": ["en-US"], "path": "sub/en.exe", "arguments": "-a -b", "type": "FileTask"},
            {"category": "game", "isPrimary": true, "path": "https://example.com", "type": "URLTask"}]});
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice(info.to_string().as_bytes());
        std::fs::write(d.join("goggame-9.info"), &bytes).unwrap();
        assert_eq!(launch_of(&d, "9", "en-US", &tasks), ("sub\\en.exe".to_string(), vec!["-a".to_string(), "-b".to_string()]));
        assert_eq!(launch_of(&d, "9", "German", &tasks).0, "de.exe");
        // A path that leaves the folder is not used.
        std::fs::write(d.join("goggame-9.info"), json!({"playTasks": [{"isPrimary": true, "path": "..\\evil.exe", "type": "FileTask"}]}).to_string()).unwrap();
        assert_eq!(launch_of(&d, "9", "en-US", &[]), (String::new(), vec![]));
    }

    // ----- the real HTTP client -----

    #[tokio::test]
    async fn the_http_client_returns_bodies_counts_bytes_and_reports_statuses() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let sent_auth = Arc::new(AtomicBool::new(false));
        let flag = sent_auth.clone();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                let flag = flag.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 4096];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_ascii_lowercase();
                    if req.contains("authorization") {
                        flag.store(true, Ordering::SeqCst);
                    }
                    let crlf = "\r\n";
                    let resp = if req.starts_with("get /missing") {
                        format!("HTTP/1.1 404 Not Found{crlf}Content-Length: 0{crlf}Connection: close{crlf}{crlf}")
                    } else if req.starts_with("get /ranged") && req.contains("range: bytes=2-5") {
                        format!("HTTP/1.1 206 Partial Content{crlf}Content-Length: 4{crlf}Content-Range: bytes 2-5/11{crlf}Connection: close{crlf}{crlf}llo ")
                    } else {
                        format!("HTTP/1.1 200 OK{crlf}Content-Length: 11{crlf}Connection: close{crlf}{crlf}hello world")
                    };
                    let _ = s.write_all(resp.as_bytes()).await;
                });
            }
        });
        let http = Http::with_token("a-made-up-token", None);
        let counted = AtomicU64::new(0);
        let body = http.get(&format!("http://{addr}/secure_link?x=1"), &|n| {
            counted.fetch_add(n, Ordering::SeqCst);
        })
        .await
        .unwrap();
        assert_eq!((body.as_slice(), counted.load(Ordering::SeqCst)), (b"hello world".as_slice(), 11));
        let err = http.get(&format!("http://{addr}/missing?token=secret-cdn-token"), &|_| {}).await.unwrap_err();
        assert_eq!(status_of(&err), Some(404));
        assert!(!format!("{err:#}").contains("secret-cdn-token"), "an address with a token is not put in an error: {err:#}");
        assert!(!sent_auth.load(Ordering::SeqCst), "the token is not sent to a host that is not GOG's content system");
        // A range comes back as exactly those bytes; a server that ignores the range and sends it all is not believed.
        let part = http.get_range(&format!("http://{addr}/ranged"), 2, 4, &|_| {}).await.unwrap();
        assert_eq!(part, b"llo ");
        assert!(http.get_range(&format!("http://{addr}/whole"), 2, 4, &|_| {}).await.is_err());
    }

    // ----- plumbing -----

    #[test]
    fn the_sync_futures_can_be_spawned_onto_a_runtime() {
        fn is_send<T: Send>(_: &T) {}
        let cancel = AtomicBool::new(false);
        let progress = |_p: Progress| {};
        let real = sync("t", 1, "n", Path::new("x"), Mode::Install, "en", &progress, &cancel);
        is_send(&real);
        let fake = Fake::new();
        let (config, options) = (cfg(), Options::default());
        let fixture = sync_via(&fake, &config, 1, "n", Path::new("x"), Mode::Install, "en", &options, &progress, &cancel);
        is_send(&fixture);
        let latest = latest_build(1);
        is_send(&latest);
    }

    /// GOG's real public lists and manifests, no sign in and no chunks:
    /// `cargo test gog_dl::tests::live -- --ignored --nocapture`.
    #[tokio::test]
    #[ignore]
    async fn live_public_manifests() {
        let http = Http::public();
        for (pid, lang) in [(1423049311u64, "en-US"), (1207658786, "de-DE"), (1207664643, "English"), (1207659026, "ja-JP")] {
            let (id, version) = latest_build(pid).await.unwrap();
            let builds = list_builds(&http, pid).await.unwrap();
            let b = pick_build(&builds, Pick::Newest).unwrap();
            assert_eq!(b.id, id);
            assert_eq!(b.generation, 2, "{pid}");
            let raw = http.get(&b.link, &|_| {}).await.unwrap();
            let m = parse_build_manifest(&raw).unwrap();
            assert_eq!(m.product_id, pid.to_string());
            let allowed: HashSet<String> = [pid.to_string()].into();
            let (picked, chosen) = pick_depots(&m, lang, &allowed);
            let origin = meta_base(&b.link);
            let mut depots = Vec::new();
            for d in &picked {
                let raw = http.get(&depot_url(&origin, &d.manifest), &|_| {}).await.unwrap();
                depots.push(SavedDepot { product_id: d.product_id.clone(), manifest: d.manifest.clone(), data: parse_depot_manifest(&raw).unwrap() });
            }
            let merged = merge(&depots).unwrap();
            let todo: Vec<&Entry> = merged.entries.iter().collect();
            let units = plan_units(&todo, &depots, V1_PIECE).unwrap();
            let total: u64 = merged.entries.iter().map(|e| e.size).sum();
            let placed: u64 = units.iter().flat_map(|u| u.places.iter()).map(|p| p.len).sum();
            assert_eq!(placed, total, "{pid}: every byte of every file has a chunk to come from");
            let packed = merged.entries.iter().filter(|e| e.file.sfc.is_some()).count();
            let download: u64 = units.iter().map(|u| u.packed).sum();
            eprintln!(
                "{pid} {version:?} build {id}: {} of {} depots for {chosen:?}, {} files ({packed} packed), {} folders, {} downloads of {} MB for {} MB, dependencies {:?}, launch fallback {:?}",
                picked.len(),
                m.depots.len(),
                merged.entries.len(),
                merged.dirs.len(),
                units.len(),
                download >> 20,
                total >> 20,
                m.dependencies,
                m.tasks.iter().find(|t| t.product_id == pid.to_string()).map(|t| t.exe.clone())
            );
            assert!(!merged.entries.is_empty());
        }
        // A game with only generation 1 builds on GOG's lists: the repository and its depots are read.
        let body = http.get(&builds_url(1207659139, 1), &|_| {}).await.unwrap();
        let builds = parse_builds(&body).unwrap();
        let b = pick_build(&builds, Pick::Newest).unwrap();
        assert_eq!(b.generation, 1);
        let repo = http.get(&b.link, &|_| {}).await.unwrap();
        let r = parse_v1_repository(&repo).unwrap();
        let base = b.link.rsplit_once('/').unwrap().0.to_string();
        let mut saved = Vec::new();
        for d in r.depots.iter().filter(|d| d.systems.iter().any(|s| s.eq_ignore_ascii_case("windows"))) {
            let files = parse_v1_depot(&http.get(&format!("{base}/{}", d.manifest), &|_| {}).await.unwrap()).unwrap();
            saved.push(SavedDepot { product_id: "1207659139".into(), manifest: d.manifest.clone(), data: v1_manifest(files, "1207659139").unwrap() });
        }
        let merged = merge(&saved).unwrap();
        let todo: Vec<&Entry> = merged.entries.iter().collect();
        let units = plan_units(&todo, &saved, V1_PIECE).unwrap();
        let total: u64 = merged.entries.iter().map(|e| e.size).sum();
        let placed: u64 = units.iter().flat_map(|u| u.places.iter()).map(|p| p.len).sum();
        assert_eq!(placed, total);
        eprintln!(
            "generation 1 build {} of 1207659139 (Windows depots {} of {}): {} files, {} folders, {} range requests of at most {} MB for {} MB, timestamp {}, redistributables {:?}",
            b.id,
            saved.len(),
            r.depots.len(),
            merged.entries.len(),
            merged.dirs.len(),
            units.len(),
            V1_PIECE >> 20,
            total >> 20,
            r.timestamp,
            r.redists
        );
        assert!(!merged.entries.is_empty());
    }
}
