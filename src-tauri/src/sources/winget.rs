//! The public winget catalog, read directly. winget.exe is never run.
//!
//! * `source2.msix` on Microsoft's CDN is a signed package holding one SQLite file,
//!   `Public/index.db`: a row per package (id, name, newest version and a hash that names its
//!   version list) and lookup tables of product codes, upgrade codes, normalized names and
//!   normalized publishers. Kept as `winget/index.db` (about 9 MB) and revalidated with its ETag.
//! * `packages/<id>/<hash>/versionData.mszyml` lists every version of a package with the path and
//!   SHA-256 of its manifest: YAML, compressed in the Windows Compression API's MSZIP format.
//! * `manifests/...` is the merged YAML manifest of one version: its installers (address, SHA-256,
//!   type, scope, switches, product codes) and its release notes.
//!
//! Each step is checked against the hash given by the step before it, and the index itself
//! against Microsoft's signature on the package.

use crate::{http, paths, winutil};
use anyhow::{anyhow, bail, Context, Result};
use fancy_regex::Regex as FRegex;
use once_cell::sync::Lazy;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use yaml_rust2::{Yaml, YamlLoader};

pub const CDN: &str = "https://cdn.winget.microsoft.com/cache";

pub fn dir() -> PathBuf {
    let p = paths::data_dir().join("winget");
    let _ = std::fs::create_dir_all(&p);
    p
}

pub fn index_path() -> PathBuf {
    dir().join("index.db")
}

// ---------------------------------------------------------------- index file

#[derive(Serialize, Deserialize, Default)]
struct IndexMeta {
    etag: Option<String>,
    last_modified: Option<String>,
    fetched: i64,
}

/// A current copy of the package index on disk. The copy already there is used when it is
/// younger than `max_age`, when the CDN says it has not changed, or when the CDN can not be reached.
pub async fn refresh_index(max_age: Duration) -> Result<PathBuf> {
    // One refresh at a time: a check and an opened version list may ask together.
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    let _turn = LOCK.lock().await;
    let db = index_path();
    let meta_path = dir().join("index.json");
    let meta: IndexMeta = std::fs::read(&meta_path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let now = chrono::Utc::now().timestamp();
    let have = db.is_file();
    if have && (http::PREFER_CACHE.load(std::sync::atomic::Ordering::Relaxed) || now - meta.fetched < max_age.as_secs() as i64) {
        return Ok(db);
    }
    let mut req = http::CLIENT.get(format!("{CDN}/source2.msix")).timeout(Duration::from_secs(180));
    if have {
        if let Some(e) = &meta.etag {
            req = req.header(reqwest::header::IF_NONE_MATCH, e);
        }
        if let Some(l) = &meta.last_modified {
            req = req.header(reqwest::header::IF_MODIFIED_SINCE, l);
        }
    }
    let save = |m: &IndexMeta| {
        let _ = std::fs::write(&meta_path, serde_json::to_vec(m).unwrap_or_default());
    };
    match req.send().await {
        Ok(r) if r.status() == reqwest::StatusCode::NOT_MODIFIED && have => {
            save(&IndexMeta { fetched: now, ..meta });
            Ok(db)
        }
        Ok(r) if r.status().is_success() => {
            let header = |h: reqwest::header::HeaderName| r.headers().get(h).and_then(|v| v.to_str().ok()).map(String::from);
            let (etag, last_modified) = (header(reqwest::header::ETAG), header(reqwest::header::LAST_MODIFIED));
            let got: Result<()> = async {
                let body = r.bytes().await.context("winget index download")?.to_vec();
                let target = db.clone();
                tokio::task::spawn_blocking(move || install_index(&body, &target)).await.map_err(|e| anyhow!("{e}"))?
            }
            .await;
            if got.is_ok() {
                save(&IndexMeta { etag, last_modified, fetched: now });
            }
            keep_old_on_error(got, have, db)
        }
        Ok(r) if have => {
            log::warn!("winget index: HTTP {}, using the copy on disk", r.status());
            Ok(db)
        }
        Ok(r) => bail!("HTTP {} from cdn.winget.microsoft.com", r.status()),
        Err(e) if have => {
            log::warn!("winget index: {e}, using the copy on disk");
            Ok(db)
        }
        Err(e) => Err(e).context("Could not reach cdn.winget.microsoft.com"),
    }
}

/// A refresh that broke off (connection dropped mid-download, the file still locked) keeps the
/// index already on disk instead of failing the whole check.
fn keep_old_on_error(got: Result<()>, have: bool, db: PathBuf) -> Result<PathBuf> {
    match got {
        Ok(()) => Ok(db),
        Err(e) if have => {
            log::warn!("winget index: {e:#}, using the copy on disk");
            Ok(db)
        }
        Err(e) => Err(e),
    }
}

/// Check Microsoft's signature on the downloaded package and put its index in place.
fn install_index(msix: &[u8], db: &Path) -> Result<()> {
    // Windows checks a package's signature by its extension: it has to end in .msix.
    let pkg = db.with_file_name("source2-download.msix");
    let part = db.with_extension("db.part");
    let r = (|| -> Result<()> {
        std::fs::write(&pkg, msix)?;
        let signer = winutil::authenticode_signer(&pkg).unwrap_or_default();
        if !signer.starts_with("Microsoft") {
            bail!("The winget index is not signed by Microsoft (signer: {})", if signer.is_empty() { "none" } else { &signer });
        }
        let mut z = zip::ZipArchive::new(std::io::BufReader::new(std::fs::File::open(&pkg)?))?;
        let mut e = z.by_name("Public/index.db").context("index.db is missing from the winget package")?;
        let mut out = std::fs::File::create(&part)?;
        std::io::copy(&mut e, &mut out)?;
        drop(out);
        Index::open(&part)?;
        // A scan may have the old copy open for a moment.
        let mut last = None;
        for _ in 0..10 {
            match std::fs::rename(&part, db) {
                Ok(()) => return Ok(()),
                Err(e) => last = Some(e),
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        Err(last.map(anyhow::Error::from).unwrap_or_else(|| anyhow!("rename failed")))
    })();
    let _ = std::fs::remove_file(&pkg);
    let _ = std::fs::remove_file(&part);
    r
}

// ---------------------------------------------------------------- index queries

pub struct Index {
    conn: Connection,
}

/// One package row of the index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pkg {
    pub row: i64,
    /// "7zip.7zip"
    pub id: String,
    pub name: String,
    pub moniker: Option<String>,
    pub latest: String,
    /// Range of the versions the apps write to the uninstall list, when they differ from the
    /// package versions.
    pub arp_min: Option<String>,
    pub arp_max: Option<String>,
    /// Lowercase hex SHA-256 of the version list
    pub hash: String,
}

impl Pkg {
    pub fn version_data_url(&self) -> String {
        format!("{CDN}/packages/{}/{}/versionData.mszyml", self.id, &self.hash[..self.hash.len().min(8)])
    }
}

const PKG_COLS: &str = "rowid, id, name, moniker, latest_version, arp_min_version, arp_max_version, hash";

fn pkg_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Pkg> {
    let hash: Option<Vec<u8>> = r.get(7)?;
    Ok(Pkg { row: r.get(0)?, id: r.get(1)?, name: r.get(2)?, moniker: r.get(3)?, latest: r.get(4)?, arp_min: r.get(5)?, arp_max: r.get(6)?, hash: hash.map(hex::encode).unwrap_or_default() })
}

impl Index {
    pub fn open(path: &Path) -> Result<Index> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).with_context(|| format!("open {}", path.display()))?;
        Index::from_connection(conn)
    }

    /// An index in a connection (tests build one in memory).
    pub fn from_connection(conn: Connection) -> Result<Index> {
        let major: String = conn.query_row("SELECT value FROM metadata WHERE name = 'majorVersion'", [], |r| r.get(0)).context("not a winget index")?;
        if major != "2" {
            bail!("winget index version {major} is not supported");
        }
        Ok(Index { conn })
    }

    pub fn count(&self) -> usize {
        self.conn.query_row("SELECT count(*) FROM packages", [], |r| r.get::<_, i64>(0)).unwrap_or(0) as usize
    }

    pub fn by_row(&self, row: i64) -> Option<Pkg> {
        self.conn.query_row(&format!("SELECT {PKG_COLS} FROM packages WHERE rowid = ?1"), [row], pkg_row).ok()
    }

    pub fn by_id(&self, id: &str) -> Option<Pkg> {
        self.conn.query_row(&format!("SELECT {PKG_COLS} FROM packages WHERE id = ?1 COLLATE NOCASE"), [id], pkg_row).ok()
    }

    fn rows(&self, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Vec<i64> {
        let Ok(mut st) = self.conn.prepare_cached(sql) else { return vec![] };
        let mut v: Vec<i64> = st.query_map(args, |r| r.get(0)).map(|m| m.flatten().collect()).unwrap_or_default();
        v.sort_unstable();
        v.dedup();
        v
    }

    fn strings(&self, sql: &str, row: i64) -> Vec<String> {
        let Ok(mut st) = self.conn.prepare_cached(sql) else { return vec![] };
        st.query_map([row], |r| r.get(0)).map(|m| m.flatten().collect()).unwrap_or_default()
    }

    /// Packages that install this product code (an MSI "{GUID}" or the uninstall key name of
    /// any other installer). The index keeps them in lower case, a few with leading spaces.
    pub fn by_product_code(&self, code: &str) -> Vec<i64> {
        let c = code.trim().to_lowercase();
        if c.is_empty() {
            return vec![];
        }
        self.rows("SELECT package FROM productcodes2 WHERE productcode IN (?1, ' ' || ?1, '  ' || ?1)", &[&c])
    }

    pub fn by_upgrade_code(&self, code: &str) -> Vec<i64> {
        let c = code.trim().to_lowercase();
        if c.is_empty() || c == "{*}" {
            return vec![];
        }
        self.rows("SELECT package FROM upgradecodes2 WHERE upgradecode = ?1", &[&c])
    }

    /// Packages with this normalized name, with or without the architecture the index adds.
    pub fn by_name(&self, norm: &str) -> Vec<i64> {
        if norm.is_empty() {
            return vec![];
        }
        self.rows("SELECT package FROM norm_names2 WHERE norm_name IN (?1, ?1 || '(X64)', ?1 || '(X86)')", &[&norm])
    }

    pub fn by_publisher(&self, norm: &str) -> Vec<i64> {
        if norm.is_empty() {
            return vec![];
        }
        self.rows("SELECT package FROM norm_publishers2 WHERE norm_publisher = ?1", &[&norm])
    }

    pub fn publishers_of(&self, row: i64) -> Vec<String> {
        self.strings("SELECT norm_publisher FROM norm_publishers2 WHERE package = ?1", row)
    }

    /// Normalized names of a package, architecture marks removed.
    pub fn names_of(&self, row: i64) -> Vec<String> {
        let mut v: Vec<String> = self.strings("SELECT norm_name FROM norm_names2 WHERE package = ?1", row).into_iter().map(|n| n.trim_end_matches("(X64)").trim_end_matches("(X86)").to_string()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Packages offered as MSIX (they have a package family name).
    pub fn is_msix(&self, row: i64) -> bool {
        self.conn.query_row("SELECT count(*) FROM pfns2 WHERE package = ?1", [row], |r| r.get::<_, i64>(0)).unwrap_or(0) > 0
    }

    /// Packages whose id, name or moniker contains every word of `q`, best first.
    pub fn search(&self, q: &str, limit: usize) -> Vec<Pkg> {
        let words: Vec<String> = q.split_whitespace().map(|w| w.to_lowercase().replace(['%', '_'], "")).filter(|w| !w.is_empty()).take(4).collect();
        if words.is_empty() {
            return vec![];
        }
        let cond: Vec<String> = (1..=words.len()).map(|i| format!("(lower(name) LIKE ?{i} OR lower(id) LIKE ?{i} OR lower(ifnull(moniker,'')) LIKE ?{i})")).collect();
        let sql = format!("SELECT {PKG_COLS} FROM packages WHERE {} LIMIT 400", cond.join(" AND "));
        let pats: Vec<String> = words.iter().map(|w| format!("%{w}%")).collect();
        let Ok(mut st) = self.conn.prepare(&sql) else { return vec![] };
        let args: Vec<&dyn rusqlite::ToSql> = pats.iter().map(|p| p as &dyn rusqlite::ToSql).collect();
        let mut v: Vec<Pkg> = st.query_map(args.as_slice(), pkg_row).map(|m| m.flatten().collect()).unwrap_or_default();
        let ql = q.trim().to_lowercase();
        let rank = |p: &Pkg| {
            let n = p.name.to_lowercase();
            if n == ql || p.moniker.as_deref().is_some_and(|m| m.eq_ignore_ascii_case(&ql)) {
                0
            } else if n.starts_with(&ql) {
                1
            } else if n.contains(&ql) {
                2
            } else {
                3
            }
        };
        v.sort_by(|a, b| rank(a).cmp(&rank(b)).then_with(|| a.name.len().cmp(&b.name.len())).then_with(|| a.name.cmp(&b.name)));
        v.truncate(limit);
        v
    }
}

/// An empty index with the same tables as the real one, for tests.
#[cfg(test)]
pub fn test_index() -> Index {
    let c = Connection::open_in_memory().unwrap();
    c.execute_batch(
        "CREATE TABLE metadata(name TEXT PRIMARY KEY NOT NULL, value TEXT NOT NULL);
         INSERT INTO metadata VALUES('majorVersion','2'),('minorVersion','0');
         CREATE TABLE packages(rowid INTEGER PRIMARY KEY, id TEXT NOT NULL, name TEXT NOT NULL, moniker TEXT, latest_version TEXT NOT NULL, arp_min_version TEXT, arp_max_version TEXT, hash BLOB);
         CREATE TABLE productcodes2(productcode TEXT NOT NULL, package INT64 NOT NULL, PRIMARY KEY(productcode, package));
         CREATE TABLE upgradecodes2(upgradecode TEXT NOT NULL, package INT64 NOT NULL, PRIMARY KEY(upgradecode, package));
         CREATE TABLE norm_names2(norm_name TEXT NOT NULL, package INT64 NOT NULL, PRIMARY KEY(norm_name, package));
         CREATE TABLE norm_publishers2(norm_publisher TEXT NOT NULL, package INT64 NOT NULL, PRIMARY KEY(norm_publisher, package));
         CREATE TABLE pfns2(pfn TEXT NOT NULL, package INT64 NOT NULL, PRIMARY KEY(pfn, package));",
    )
    .unwrap();
    Index { conn: c }
}

#[cfg(test)]
impl Index {
    /// Write a test index to a file, as the real one is kept.
    pub fn save_to(&self, path: &Path) {
        let _ = std::fs::remove_file(path);
        self.conn.execute("VACUUM INTO ?1", [path.to_string_lossy().to_string()]).unwrap();
    }

    /// Add a package to a test index: its product codes, upgrade codes, display names and publisher.
    #[allow(clippy::too_many_arguments)]
    pub fn add_test_package(&self, id: &str, name: &str, latest: &str, product_codes: &[&str], upgrade_codes: &[&str], names: &[&str], publisher: &str) -> i64 {
        let hash = Sha256::digest(id.as_bytes()).to_vec();
        self.conn.execute("INSERT INTO packages(id, name, latest_version, hash) VALUES(?1, ?2, ?3, ?4)", rusqlite::params![id, name, latest, hash]).unwrap();
        let row = self.conn.last_insert_rowid();
        for p in product_codes {
            self.conn.execute("INSERT OR IGNORE INTO productcodes2 VALUES(?1, ?2)", rusqlite::params![p.to_lowercase(), row]).unwrap();
        }
        for u in upgrade_codes {
            self.conn.execute("INSERT OR IGNORE INTO upgradecodes2 VALUES(?1, ?2)", rusqlite::params![u.to_lowercase(), row]).unwrap();
        }
        for n in names.iter().copied().chain([name]) {
            let nn = normalize_name(n);
            let key = match nn.arch {
                Some(a) => format!("{}({a})", nn.name),
                None => nn.name,
            };
            self.conn.execute("INSERT OR IGNORE INTO norm_names2 VALUES(?1, ?2)", rusqlite::params![key, row]).unwrap();
        }
        self.conn.execute("INSERT OR IGNORE INTO norm_publishers2 VALUES(?1, ?2)", rusqlite::params![normalize_publisher(publisher), row]).unwrap();
        row
    }
}

// ---------------------------------------------------------------- MSZIP

/// Decompress a Windows Compression API MSZIP buffer: a 24 byte header (signature, then the
/// uncompressed size at offset 8), then chunks of a 4 byte length and "CK" deflate blocks. Each
/// block may refer back to what the blocks before it produced.
pub fn mszip(data: &[u8]) -> Result<Vec<u8>> {
    use miniz_oxide::inflate::core::{decompress, inflate_flags, DecompressorOxide};
    use miniz_oxide::inflate::TINFLStatus;
    if data.len() < 24 || data[..4] != [0x0A, 0x51, 0xE5, 0xC0] {
        bail!("not an MSZIP buffer");
    }
    let total = u64::from_le_bytes(data[8..16].try_into().unwrap()) as usize;
    if total > 64 * 1024 * 1024 {
        bail!("MSZIP buffer too large");
    }
    let mut out = vec![0u8; total];
    let mut at = 0usize;
    let mut pos = 24usize;
    while pos + 4 <= data.len() && at < total {
        let n = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        let mut chunk = data.get(pos..pos + n).ok_or_else(|| anyhow!("MSZIP chunk past the end"))?;
        pos += n;
        while chunk.len() > 2 && at < total {
            if &chunk[..2] != b"CK" {
                bail!("MSZIP block without its CK mark");
            }
            let mut r = DecompressorOxide::new();
            let (status, used, wrote) = decompress(&mut r, &chunk[2..], &mut out, at, inflate_flags::TINFL_FLAG_USING_NON_WRAPPING_OUTPUT_BUF);
            if status != TINFLStatus::Done {
                bail!("MSZIP block: {status:?}");
            }
            at += wrote;
            chunk = &chunk[2 + used..];
        }
    }
    if at != total {
        bail!("MSZIP: {at} of {total} bytes");
    }
    Ok(out)
}

// ---------------------------------------------------------------- versions

/// One version of a package and where its manifest is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerRef {
    pub version: String,
    /// "manifests/7/7zip/7zip/26.05/8249"
    pub path: String,
    pub sha256: String,
}

/// Text of a YAML scalar. Versions such as `26.05` are numbers to YAML; their text is kept.
pub fn text(y: &Yaml) -> Option<String> {
    match y {
        Yaml::String(s) | Yaml::Real(s) => Some(s.clone()),
        Yaml::Integer(i) => Some(i.to_string()),
        Yaml::Boolean(b) => Some(b.to_string()),
        _ => None,
    }
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
}

fn load(yaml: &str) -> Result<Yaml> {
    let docs = YamlLoader::load_from_str(yaml.trim_start_matches('\u{feff}')).map_err(|e| anyhow!("YAML: {e}"))?;
    docs.into_iter().next().ok_or_else(|| anyhow!("empty YAML"))
}

/// The versions of a version list, newest first.
pub fn parse_version_data(yaml: &str) -> Result<Vec<VerRef>> {
    let doc = load(yaml)?;
    let mut v: Vec<VerRef> = doc["vD"]
        .as_vec()
        .map(|l| {
            l.iter()
                .filter_map(|e| Some(VerRef { version: text(&e["v"])?, path: text(&e["rP"])?, sha256: text(&e["s256H"])?.to_ascii_lowercase() }))
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| crate::sources::tools::cmp_ver(&b.version, &a.version));
    Ok(v)
}

fn sha_hex(b: &[u8]) -> String {
    hex::encode(Sha256::digest(b))
}

fn files_dir() -> PathBuf {
    let p = dir().join("files");
    let _ = std::fs::create_dir_all(&p);
    p
}

/// A file whose address names its content (version lists and manifests never change), kept in
/// `winget/files` under its SHA-256 and checked against it.
async fn immutable(url: &str, sha256: &str) -> Result<Vec<u8>> {
    let sha = sha256.to_ascii_lowercase();
    if sha.len() != 64 || !sha.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("{url}: the winget index gives no checksum for it");
    }
    let path = files_dir().join(&sha);
    if let Ok(b) = std::fs::read(&path) {
        if sha_hex(&b) == sha {
            // Marks it as used, so pruning keeps it.
            let _ = std::fs::File::options().write(true).open(&path).and_then(|f| f.set_modified(std::time::SystemTime::now()));
            return Ok(b);
        }
    }
    let body = http::CLIENT.get(url).timeout(Duration::from_secs(60)).send().await?.error_for_status()?.bytes().await?.to_vec();
    if sha_hex(&body) != sha {
        bail!("{url} does not match the checksum in the winget index");
    }
    let _ = std::fs::write(&path, &body);
    Ok(body)
}

/// Delete version lists and manifests not used for `max_age` (older versions of packages).
pub fn prune_files(max_age: Duration) {
    let Ok(rd) = std::fs::read_dir(files_dir()) else { return };
    for e in rd.flatten() {
        let old = e.metadata().ok().and_then(|m| m.modified().ok()).and_then(|t| t.elapsed().ok()).is_some_and(|age| age > max_age);
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Every version of a package, newest first.
pub async fn versions(p: &Pkg) -> Result<Vec<VerRef>> {
    let body = immutable(&p.version_data_url(), &p.hash).await?;
    let yaml = mszip(&body)?;
    parse_version_data(&String::from_utf8_lossy(&yaml))
}

pub async fn manifest(r: &VerRef) -> Result<Manifest> {
    let body = immutable(&format!("{CDN}/{}", r.path), &r.sha256).await?;
    parse_manifest(&String::from_utf8_lossy(&body))
}

// ---------------------------------------------------------------- manifests

/// What an installer writes to the uninstall list.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ArpEntry {
    pub display_name: Option<String>,
    pub publisher: Option<String>,
    pub display_version: Option<String>,
    pub product_code: Option<String>,
    pub upgrade_code: Option<String>,
    pub installer_type: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Installer {
    /// x64, x86, arm64, arm, neutral
    pub arch: String,
    /// msi, wix, nsis (winget's "nullsoft"), inno, burn, exe, msix, appx, zip, portable, pwa, font
    pub kind: String,
    /// Inside a zip: the installer type and its path in the zip
    pub nested_kind: Option<String>,
    pub nested_path: Option<String>,
    pub url: String,
    /// Lowercase hex
    pub sha256: String,
    /// user or machine; None when the installer decides
    pub scope: Option<String>,
    pub locale: Option<String>,
    pub silent: Option<String>,
    pub silent_progress: Option<String>,
    pub custom: Option<String>,
    pub upgrade: Option<String>,
    pub install_modes: Vec<String>,
    pub product_code: Option<String>,
    pub arp: Vec<ArpEntry>,
    /// install, uninstallPrevious, deny
    pub upgrade_behavior: Option<String>,
    /// elevationRequired, elevatesSelf, elevationProhibited
    pub elevation: Option<String>,
    /// Exit codes that mean success besides 0
    pub success_codes: Vec<u32>,
    pub release_date: Option<String>,
    pub default_install_location: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Manifest {
    pub id: String,
    pub version: String,
    pub name: String,
    pub publisher: String,
    pub publisher_url: Option<String>,
    pub package_url: Option<String>,
    pub description: Option<String>,
    pub release_notes: Option<String>,
    pub release_notes_url: Option<String>,
    pub release_date: Option<String>,
    pub icon_url: Option<String>,
    pub installers: Vec<Installer>,
}

/// winget's installer type names, with NSIS ("nullsoft") under its usual name.
fn installer_type(t: String) -> String {
    let t = t.to_ascii_lowercase();
    if t == "nullsoft" {
        "nsis".into()
    } else {
        t
    }
}

fn has(y: &Yaml) -> bool {
    !y.is_badvalue() && !y.is_null()
}

/// Exit codes that winget treats as a finished install.
fn ok_returns(y: &Yaml) -> Vec<u32> {
    y.as_vec()
        .map(|l| {
            l.iter()
                .filter(|e| matches!(text(&e["ReturnResponse"]).as_deref(), Some("rebootRequiredToFinish" | "rebootInitiated" | "alreadyInstalled")))
                .filter_map(|e| e["InstallerReturnCode"].as_i64())
                .map(|c| c as u32)
                .collect()
        })
        .unwrap_or_default()
}

fn arp_entries(y: &Yaml) -> Vec<ArpEntry> {
    y.as_vec()
        .map(|l| {
            l.iter()
                .map(|e| ArpEntry {
                    display_name: text(&e["DisplayName"]),
                    publisher: text(&e["Publisher"]),
                    display_version: text(&e["DisplayVersion"]),
                    product_code: text(&e["ProductCode"]),
                    upgrade_code: text(&e["UpgradeCode"]),
                    installer_type: text(&e["InstallerType"]).map(|s| s.to_ascii_lowercase()),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A merged manifest. Values given at the top apply to every installer that does not set its own.
pub fn parse_manifest(yaml: &str) -> Result<Manifest> {
    let root = load(yaml)?;
    let mut m = Manifest {
        id: text(&root["PackageIdentifier"]).unwrap_or_default(),
        version: text(&root["PackageVersion"]).unwrap_or_default(),
        name: text(&root["PackageName"]).unwrap_or_default(),
        publisher: text(&root["Publisher"]).unwrap_or_default(),
        publisher_url: text(&root["PublisherUrl"]),
        package_url: text(&root["PackageUrl"]),
        description: text(&root["ShortDescription"]),
        release_notes: text(&root["ReleaseNotes"]),
        release_notes_url: text(&root["ReleaseNotesUrl"]),
        release_date: text(&root["ReleaseDate"]),
        icon_url: root["Icons"].as_vec().and_then(|l| l.iter().find_map(|i| text(&i["IconUrl"]))),
        installers: vec![],
    };
    if m.id.is_empty() || m.version.is_empty() {
        bail!("manifest without an id or version");
    }
    for inst in root["Installers"].as_vec().map(|v| v.as_slice()).unwrap_or(&[]) {
        let get = |k: &str| if has(&inst[k]) { &inst[k] } else { &root[k] };
        let switch = |k: &str| text(&inst["InstallerSwitches"][k]).or_else(|| text(&root["InstallerSwitches"][k]));
        let nested = get("NestedInstallerFiles");
        let mut success: Vec<u32> = get("InstallerSuccessCodes").as_vec().map(|l| l.iter().filter_map(|c| c.as_i64()).map(|c| c as u32).collect()).unwrap_or_default();
        success.extend(ok_returns(get("ExpectedReturnCodes")));
        let url = text(&inst["InstallerUrl"]).unwrap_or_default();
        if url.is_empty() {
            continue;
        }
        m.installers.push(Installer {
            arch: text(get("Architecture")).unwrap_or_else(|| "neutral".into()).to_ascii_lowercase(),
            kind: installer_type(text(get("InstallerType")).unwrap_or_default()),
            nested_kind: text(get("NestedInstallerType")).map(installer_type),
            nested_path: nested.as_vec().and_then(|l| l.iter().find_map(|f| text(&f["RelativeFilePath"]))),
            url,
            sha256: text(&inst["InstallerSha256"]).unwrap_or_default().to_ascii_lowercase(),
            scope: text(get("Scope")).map(|s| s.to_ascii_lowercase()),
            locale: text(get("InstallerLocale")),
            silent: switch("Silent"),
            silent_progress: switch("SilentWithProgress"),
            custom: switch("Custom"),
            upgrade: switch("Upgrade"),
            install_modes: get("InstallModes").as_vec().map(|l| l.iter().filter_map(text).collect()).unwrap_or_default(),
            product_code: text(get("ProductCode")),
            arp: arp_entries(get("AppsAndFeaturesEntries")),
            upgrade_behavior: text(get("UpgradeBehavior")),
            elevation: text(get("ElevationRequirement")),
            success_codes: success,
            release_date: text(get("ReleaseDate")),
            default_install_location: text(&get("InstallationMetadata")["DefaultInstallLocation"]),
        });
    }
    if m.release_date.is_none() {
        m.release_date = m.installers.iter().find_map(|i| i.release_date.clone());
    }
    Ok(m)
}

// ---------------------------------------------------------------- name normalization
//
// The index keys installed apps by name and publisher after winget's own normalization
// (NameNormalization.cpp, "Initial" version): versions, architectures, locales, bracketed
// parts and legal entity words are removed, then everything but letters and digits.

fn rx(p: &str) -> FRegex {
    FRegex::new(&format!("(?i){p}")).expect("normalization pattern")
}

const VER_WORDS: &str = "V|VER|VERSI(?:O|\u{d3})N|VERS\u{c3}O|VERSIE|WERSJA|BUILD|RELEASE|RC|SP";

static ARCH_32_OR_64: Lazy<FRegex> = Lazy::new(|| rx(r"(?<![\p{L}\p{Nd}])((64[\\/]32|32[\\/]64)[\p{Pd}\p{Pc}\p{Z}]?BIT)S?(?:\sEDITION)?"));
static ARCH_X64: Lazy<FRegex> = Lazy::new(|| rx(r"(?<![\p{L}\p{Nd}])(X64|AMD64|X86([\p{Pd}\p{Pc}]64))(?=\P{Nd}|$)(?:\sEDITION)?"));
static ARCH_64BIT: Lazy<FRegex> = Lazy::new(|| rx(r"(?<![\p{L}\p{Nd}])(64[\p{Pd}\p{Pc}\p{Z}]?BIT)S?(?:\sEDITION)?"));
static ARCH_X32: Lazy<FRegex> = Lazy::new(|| rx(r"(?<![\p{L}\p{Nd}])(X32|X86)(?=\P{Nd}|$)(?:\sEDITION)?"));
static ARCH_32BIT: Lazy<FRegex> = Lazy::new(|| rx(r"(?<![\p{L}\p{Nd}])(32[\p{Pd}\p{Pc}\p{Z}]?BIT)S?(?:\sEDITION)?"));
static LOCALE: Lazy<FRegex> = Lazy::new(|| rx(r"(?<![A-Z])((?:\p{Lu}{2,3}(-(CANS|CYRL|LATN|MONG))?-\p{Lu}{2})(?![A-Z])(?:-VALENCIA)?)"));
static KB: Lazy<FRegex> = Lazy::new(|| rx(r"\((KB\d+)\)"));
static ROBLOX: Lazy<FRegex> = Lazy::new(|| rx(r"^(ROBLOX\s(?:PLAYER|STUDIO))\sFOR\s.*"));

static PROGRAM_NAME: Lazy<Vec<FRegex>> = Lazy::new(|| {
    [
        r"^\(.*?\)".to_string(),
        r#"(\(\s*\)|\[\s*\]|"\s*")"#.to_string(),
        r"\([CDEF]:\\(.+?\\)*[^\s]*\\?\)".to_string(),
        r#""[CDEF]:\\(.+?\\)*[^\s]*\\?""#.to_string(),
        r"((INSTALLED\sAT|IN)\s)?[CDEF]:\\(.+?\\)*[^\s]*\\?".to_string(),
        format!(r"(?<!\p{{L}})(?:(?:{VER_WORDS})\P{{L}})?\p{{Lu}}\p{{Nd}}+(?:[\p{{Po}}\p{{Pd}}\p{{Pc}}]\p{{Nd}}+)+"),
        format!(r"((?<!\p{{L}})(?:{VER_WORDS})\P{{L}}?)?\p{{Nd}}+([\p{{Po}}\p{{Pd}}\p{{Pc}}]\p{{Nd}}?(RC|B|A|R|SP|K)?\p{{Nd}}+)+([\p{{Po}}\p{{Pd}}\p{{Pc}}]?[\p{{L}}\p{{Nd}}]+)*"),
        format!(r"(FOR\s)?(?<!\p{{L}})(?:P|{VER_WORDS})(?:\P{{L}}|\P{{L}}\p{{L}})?(\p{{Nd}}|\.\p{{Nd}})+(?:RC|B|A|R|V|SP)?\p{{Nd}}?"),
        r"\sEN\s*$".to_string(),
        r"\([^\(\)]*\)|\[[^\[\]]*\]".to_string(),
        r#"(?:\p{Ps}.*\p{Pe}|".*")"#.to_string(),
        r"(?<!\p{L})(?:http[s]?|ftp)://".to_string(),
        r"^[^\p{L}\p{Nd}]+".to_string(),
        r"[^\p{L}\p{Nd}]+$".to_string(),
    ]
    .iter()
    .map(|p| rx(p))
    .collect()
});

static PUBLISHER: Lazy<Vec<FRegex>> = Lazy::new(|| {
    [
        format!(r"((?<!\p{{L}})(?:{VER_WORDS})\P{{L}}?)?\p{{Nd}}+([\p{{Po}}\p{{Pd}}\p{{Pc}}]\p{{Nd}}?(RC|B|A|R|SP|K)?\p{{Nd}}+)+([\p{{Po}}\p{{Pd}}\p{{Pc}}]?[\p{{L}}\p{{Nd}}]+)*"),
        format!(r"(FOR\s)?(?<!\p{{L}})(?:P|{VER_WORDS})(?:\P{{L}}|\P{{L}}\p{{L}})?(\p{{Nd}}|\.\p{{Nd}})+(?:RC|B|A|R|V|SP)?\p{{Nd}}?"),
        r"\([^\(\)]*\)|\[[^\[\]]*\]".to_string(),
        r#"(?:\p{Ps}.*\p{Pe}|".*")"#.to_string(),
        r"(?<!\p{L})(?:http[s]?|ftp)://".to_string(),
        r"(?<!\S)[^\p{L}]+(?=\s|$)".to_string(),
        r"\P{L}+$".to_string(),
        r"(?:(?<=^\p{L})|(?<=\P{L}\p{L}))(\.|/)(?=\p{L}(?:\P{L}|$))".to_string(),
    ]
    .iter()
    .map(|p| rx(p))
    .collect()
});

const LOCALES: &str = "af-za am-et ar-ae ar-bh ar-dz ar-eg ar-iq ar-jo ar-kw ar-lb ar-ly ar-ma arn-cl ar-om ar-qa ar-sa ar-sy ar-tn ar-ye as-in ba-ru be-by bg-bg bn-bd bn-in bo-cn br-fr ca-es ca-es-valencia co-fr cs-cz cy-gb da-dk de-at de-ch de-de de-li de-lu dsb-de dv-mv el-gr en-au en-bz en-ca en-gb en-ie en-in en-jm en-my en-nz en-ph en-sg en-tt en-us en-za en-zw es-ar es-bo es-cl es-co es-cr es-do es-ec es-es es-gt es-hn es-mx es-ni es-pa es-pe es-pr es-py es-sv es-us es-uy es-ve et-ee eu-es fa-ir fi-fi fil-ph fo-fo fr-be fr-ca fr-ch fr-fr fr-lu fr-mc fy-nl ga-ie gd-db gl-es gsw-fr gu-in he-il hi-in hr-ba hr-hr hsb-de hu-hu hy-am id-id ig-ng ii-cn is-is it-ch it-it ja-jp ka-ge kk-kz kl-gl km-kh kn-in kok-in ko-kr ky-kg lb-lu lo-la lt-lt lv-lv mi-nz mk-mk ml-in mn-mn moh-ca mr-in ms-bn ms-my mt-mt nb-no ne-np nl-be nl-nl nn-no nso-za oc-fr or-in pa-in pl-pl prs-af ps-af pt-br pt-pt qut-gt quz-bo quz-ec quz-pe rm-ch ro-ro ru-ru rw-rw sah-ru sa-in se-fi se-no se-se si-lk sk-sk sl-si sma-no sma-se smj-no smj-se smn-fi sms-fi sq-al sv-fi sv-se sw-ke syr-sy ta-in te-in th-th tk-tm tn-za tr-tr tt-ru ug-cn uk-ua ur-pk vi-vn wo-sn xh-za yo-ng zh-cn zh-hk zh-mo zh-sg zh-tw zu-za az-cyrl-az az-latn-az bs-cyrl-ba bs-latn-ba ha-latn-ng iu-cans-ca iu-latn-ca mn-mong-cn sr-cyrl-ba sr-cyrl-cs sr-cyrl-me sr-cyrl-rs sr-latn-ba sr-latn-cs sr-latn-me sr-latn-rs tg-cyrl-tj tzm-latn-dz uz-cyrl-uz uz-latn-uz";

const LEGAL: &str = "ab ad ag aps as asa bv co cv doo ev ges gesmbh gmbh inc kg ks ps llc lp ltd ltda mbh nv plc sl pty pvt sa sarl sc sca sp spa srl sro company corp corporation holding holdings incorporated limited subsidiary";

fn is_word(list: &str, w: &str) -> bool {
    let w = w.to_lowercase();
    list.split(' ').any(|x| x == w)
}

/// Every match of `re` removed. Says whether anything was.
fn remove(re: &FRegex, s: &mut String) -> bool {
    let mut out = String::with_capacity(s.len());
    let mut last = 0;
    for m in re.find_iter(s) {
        let Ok(m) = m else { break };
        if m.end() == m.start() {
            continue;
        }
        out.push_str(&s[last..m.start()]);
        last = m.end();
    }
    if last == 0 {
        return false;
    }
    out.push_str(&s[last..]);
    let changed = out != *s;
    *s = out;
    changed
}

fn prepare(v: &str) -> String {
    let mut s = v.trim().to_string();
    if let Some(i) = s.char_indices().skip(3).find(|(i, _)| s[*i..].starts_with("@@")).map(|(i, _)| i) {
        s.truncate(i);
    }
    // Remove wrapping quotes or parentheses.
    loop {
        let b = s.as_bytes();
        if s.len() >= 2 && ((b[0] == b'"' && b[s.len() - 1] == b'"') || (b[0] == b'(' && b[s.len() - 1] == b')')) {
            s = s[1..s.len() - 1].to_string();
        } else {
            break;
        }
    }
    s
}

/// Words split at anything but letters, digits, `+` and `&` (names) or letters and digits
/// (publishers). Legal entity words after the first are dropped, and a publisher ends at one.
fn join_words(s: &str, keep_plus: bool, stop: bool) -> String {
    let sep = |c: char| !(c.is_alphanumeric() || (keep_plus && (c == '+' || c == '&')));
    let mut out = String::new();
    let mut first = true;
    for w in s.split(sep).filter(|w| !w.is_empty()) {
        if !first && is_word(LEGAL, w) {
            if stop {
                break;
            }
            continue;
        }
        first = false;
        out.push_str(w);
    }
    out.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NormName {
    pub name: String,
    /// "X64" or "X86" when the name said so
    pub arch: Option<&'static str>,
    /// "en-us" when the name carried a locale
    pub locale: Option<String>,
}

pub fn normalize_name(raw: &str) -> NormName {
    let mut s = prepare(raw);
    let arch = if remove(&ARCH_32_OR_64, &mut s) {
        None
    } else if remove(&ARCH_X64, &mut s) || remove(&ARCH_64BIT, &mut s) {
        Some("X64")
    } else if remove(&ARCH_X32, &mut s) || remove(&ARCH_32BIT, &mut s) {
        Some("X86")
    } else {
        None
    };
    // Locales: only real ones are removed; the first one found is kept as the name's locale.
    let mut locale = None;
    let mut kept = String::with_capacity(s.len());
    let mut last = 0;
    for m in LOCALE.find_iter(&s) {
        let Ok(m) = m else { break };
        let t = m.as_str().to_lowercase();
        if LOCALES.split(' ').any(|l| l == t) {
            kept.push_str(&s[last..m.start()]);
            last = m.end();
            locale.get_or_insert(t);
        }
    }
    kept.push_str(&s[last..]);
    s = kept;
    if let Ok(t) = KB.try_replacen(&s, 0, "$1") {
        s = t.into_owned();
    }
    if let Ok(t) = ROBLOX.try_replacen(&s, 0, "$1") {
        s = t.into_owned();
    }
    for _ in 0..20 {
        let mut any = false;
        for re in PROGRAM_NAME.iter() {
            any |= remove(re, &mut s);
        }
        if !any {
            break;
        }
    }
    NormName { name: join_words(&s, true, false), arch, locale }
}

pub fn normalize_publisher(raw: &str) -> String {
    let mut s = prepare(raw);
    for _ in 0..20 {
        let mut any = false;
        for re in PUBLISHER.iter() {
            any |= remove(re, &mut s);
        }
        if !any {
            break;
        }
    }
    join_words(&s, false, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refresh_that_breaks_off_keeps_the_index_on_disk() {
        let db = PathBuf::from("index.db");
        let broke = || Err(anyhow!("error decoding response body: connection closed"));
        assert_eq!(keep_old_on_error(broke(), true, db.clone()).unwrap(), db);
        // Without a copy on disk there is nothing to fall back on.
        assert!(keep_old_on_error(broke(), false, db.clone()).is_err());
        assert_eq!(keep_old_on_error(Ok(()), false, db.clone()).unwrap(), db);
    }

    #[test]
    fn normalizes_names_like_winget() {
        let n = |s: &str| normalize_name(s).name;
        assert_eq!(n("7-Zip 26.04 (x64)"), "7zip");
        assert_eq!(normalize_name("7-Zip 26.04 (x64)").arch, Some("X64"));
        assert_eq!(n("Notepad++"), "notepad");
        assert_eq!(n("VLC media player"), "vlcmediaplayer");
        let ff = normalize_name("Mozilla Firefox (x64 en-US)");
        assert_eq!((ff.name.as_str(), ff.arch, ff.locale.as_deref()), ("mozillafirefox", Some("X64"), Some("en-us")));
        assert_eq!(n("Python 3.12.3 (64-bit)"), "python");
        assert_eq!(n("WinRAR 7.23 (64-bit)"), "winrar");
        assert_eq!(n("FileZilla 3.70.6"), "filezilla");
        assert_eq!(n("Inno Setup version 6.7.1"), "innosetup");
        assert_eq!(n("Windhawk v1.7.3"), "windhawk");
        assert_eq!(n("Microsoft Visual C++ 2013 Redistributable (x64) - 12.0.40664"), "microsoftvisualc++2013redistributable".replace('+', ""));
        assert_eq!(n("Roblox Studio for someone"), "robloxstudio");
        assert_eq!(n("Git"), "git");
    }

    #[test]
    fn normalizes_publishers_like_winget() {
        assert_eq!(normalize_publisher("Igor Pavlov"), "igorpavlov");
        assert_eq!(normalize_publisher("Microsoft Corporation"), "microsoft");
        assert_eq!(normalize_publisher("Notepad++ Team"), "notepadteam");
        assert_eq!(normalize_publisher("Discord Inc."), "discord");
        assert_eq!(normalize_publisher("win.rar GmbH"), "winrar");
        assert_eq!(normalize_publisher("Valve Corporation"), "valve");
        assert_eq!(normalize_publisher("The Git Development Community"), "thegitdevelopmentcommunity");
    }

    #[test]
    fn unpacks_mszip_with_back_references() {
        // Header, then one chunk per block: "CK" and raw deflate. The second block starts with a
        // match that copies from the first block's output, as real lists do.
        let first: &[u8] = b"sV: 1.0
vD:
- v: 26.05
";
        let rest: &[u8] = b"  rP: manifests/7/7zip/7zip/26.05/8249
";
        let mut want = first.to_vec();
        want.extend_from_slice(&first[..3]);
        want.extend_from_slice(rest);
        let mut buf = vec![0x0A, 0x51, 0xE5, 0xC0, 0x18, 0, 0x8A, 0x02];
        buf.extend((want.len() as u64).to_le_bytes());
        buf.extend((want.len() as u64).to_le_bytes());
        for b in [miniz_oxide::deflate::compress_to_vec(first, 6), deflate_with_back_reference(first.len() as u32, rest)] {
            buf.extend(((b.len() + 2) as u32).to_le_bytes());
            buf.extend(b"CK");
            buf.extend(b);
        }
        assert_eq!(mszip(&buf).unwrap(), want);
        assert!(mszip(b"not compressed at all, no header").is_err());
        let mut short = buf.clone();
        short[8] += 1;
        assert!(mszip(&short).is_err(), "a size that does not add up is an error");
    }

    #[test]
    fn reads_version_lists() {
        let yaml = "sV: 1.0
vD:
- v: 26.04
  rP: manifests/7/7zip/7zip/26.04/54ea
  s256H: 71D2
- v: 26.05
  rP: manifests/7/7zip/7zip/26.05/8249
  s256H: bc27
- v: 9.20
  rP: manifests/7/7zip/7zip/9.20/0001
  s256H: aa
";
        let v = parse_version_data(yaml).unwrap();
        assert_eq!(v.iter().map(|r| r.version.as_str()).collect::<Vec<_>>(), vec!["26.05", "26.04", "9.20"]);
        assert_eq!(v[1], VerRef { version: "26.04".into(), path: "manifests/7/7zip/7zip/26.04/54ea".into(), sha256: "71d2".into() });
    }

    /// Raw deflate (one fixed Huffman block) of a 3 byte match at `distance`, then `data` as literals.
    fn deflate_with_back_reference(distance: u32, data: &[u8]) -> Vec<u8> {
        struct Bits {
            out: Vec<u8>,
            acc: u32,
            n: u32,
        }
        impl Bits {
            fn put(&mut self, v: u32, len: u32) {
                for i in 0..len {
                    self.acc |= ((v >> i) & 1) << self.n;
                    self.n += 1;
                    if self.n == 8 {
                        self.out.push(self.acc as u8);
                        self.acc = 0;
                        self.n = 0;
                    }
                }
            }
            /// Huffman codes go most significant bit first.
            fn code(&mut self, c: u32, len: u32) {
                for i in (0..len).rev() {
                    self.put((c >> i) & 1, 1);
                }
            }
        }
        let mut w = Bits { out: vec![], acc: 0, n: 0 };
        w.put(1, 1); // last block
        w.put(1, 2); // fixed codes
        w.code(1, 7); // length 3 (symbol 257)
        let bases: [(u32, u32); 30] = [(1, 0), (2, 0), (3, 0), (4, 0), (5, 1), (7, 1), (9, 2), (13, 2), (17, 3), (25, 3), (33, 4), (49, 4), (65, 5), (97, 5), (129, 6), (193, 6), (257, 7), (385, 7), (513, 8), (769, 8), (1025, 9), (1537, 9), (2049, 10), (3073, 10), (4097, 11), (6145, 11), (8193, 12), (12289, 12), (16385, 13), (24577, 13)];
        let d = bases.iter().rposition(|(b, _)| *b <= distance).unwrap();
        w.code(d as u32, 5);
        w.put(distance - bases[d].0, bases[d].1);
        for b in data {
            let b = *b as u32;
            if b < 144 {
                w.code(0x30 + b, 8);
            } else {
                w.code(0x190 + b - 144, 9);
            }
        }
        w.code(0, 7); // end of block
        if w.n > 0 {
            w.out.push(w.acc as u8);
        }
        w.out
    }

    #[test]
    fn reads_merged_manifests() {
        let y = r#"PackageIdentifier: Example.Tool
PackageVersion: 2.10
PackageName: Example Tool
Publisher: Example Ltd
ReleaseNotes: '- Fixed a crash.

  - Faster start.'
ReleaseNotesUrl: https://github.com/example/tool/releases/tag/v2.10
InstallerType: nullsoft
Scope: machine
InstallerSwitches:
  Silent: /S
  Custom: /NOICONS
Installers:
- Architecture: x64
  InstallerUrl: https://github.com/example/tool/releases/download/v2.10/tool-x64.exe
  InstallerSha256: ABCDEF
  ProductCode: Example Tool
  AppsAndFeaturesEntries:
  - DisplayVersion: 2.10.0.0
- Architecture: x64
  InstallerType: wix
  Scope: user
  InstallerSwitches:
    Custom: ALLUSERS=2
  InstallerUrl: https://example.com/tool.msi
  InstallerSha256: '012345'
  ProductCode: '{11111111-2222-3333-4444-555555555555}'
  ExpectedReturnCodes:
  - InstallerReturnCode: 3010
    ReturnResponse: rebootRequiredToFinish
  - InstallerReturnCode: 1638
    ReturnResponse: alreadyInstalled
  - InstallerReturnCode: 1602
    ReturnResponse: cancelledByUser
ManifestType: merged
"#;
        let m = parse_manifest(y).unwrap();
        assert_eq!((m.id.as_str(), m.version.as_str(), m.publisher.as_str()), ("Example.Tool", "2.10", "Example Ltd"));
        assert_eq!(m.release_notes.as_deref(), Some("- Fixed a crash.\n- Faster start."));
        assert_eq!(m.installers.len(), 2);
        let a = &m.installers[0];
        assert_eq!((a.kind.as_str(), a.scope.as_deref(), a.silent.as_deref(), a.custom.as_deref()), ("nsis", Some("machine"), Some("/S"), Some("/NOICONS")));
        assert_eq!(a.sha256, "abcdef");
        assert_eq!(a.arp[0].display_version.as_deref(), Some("2.10.0.0"));
        let b = &m.installers[1];
        assert_eq!((b.kind.as_str(), b.scope.as_deref(), b.silent.as_deref(), b.custom.as_deref()), ("wix", Some("user"), Some("/S"), Some("ALLUSERS=2")));
        assert_eq!(b.product_code.as_deref(), Some("{11111111-2222-3333-4444-555555555555}"));
        assert_eq!(b.success_codes, vec![3010, 1638]);
        assert!(parse_manifest("PackageName: no id").is_err());
    }

    #[test]
    fn reads_the_index_tables() {
        let idx = test_index();
        let row = idx.add_test_package("Example.Tool", "Example Tool", "2.10", &["{AAAA0000-0000-0000-0000-000000000001}", "Example Tool_is1"], &["{BBBB0000-0000-0000-0000-000000000001}"], &["Example Tool 2.10 (x64)"], "Example Ltd");
        assert_eq!(idx.count(), 1);
        assert_eq!(idx.by_product_code("{aaaa0000-0000-0000-0000-000000000001}"), vec![row]);
        assert_eq!(idx.by_product_code("example tool_IS1"), vec![row]);
        assert_eq!(idx.by_upgrade_code("{BBBB0000-0000-0000-0000-000000000001}"), vec![row]);
        assert_eq!(idx.by_name("exampletool"), vec![row]);
        assert_eq!(idx.by_publisher("example"), vec![row]);
        assert!(idx.by_upgrade_code("{*}").is_empty());
        let p = idx.by_id("example.tool").unwrap();
        assert_eq!((p.latest.as_str(), p.hash.len()), ("2.10", 64));
        assert!(p.version_data_url().starts_with("https://cdn.winget.microsoft.com/cache/packages/Example.Tool/"));
        assert_eq!(idx.search("example", 5).len(), 1);
        assert!(idx.search("nothing like it", 5).is_empty());
    }
}
