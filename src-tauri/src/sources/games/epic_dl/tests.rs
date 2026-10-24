//! Builds, manifests and chunks made here and served from memory; game folders in temp dirs.
//! Every id, token and name is made up.

use super::*;
use parking_lot::Mutex;
use serde_json::json;
use std::sync::atomic::Ordering;

const NS: &str = "madeupnamespace0000000000000000";
const ITEM: &str = "madeupitem000000000000000000000";
const APP: &str = "MadeUpApp";
const TOKEN: &str = "made-up-token";
const B1: &str = "++Made+Release-1.0-CL-100-Windows";
const B2: &str = "++Made+Release-1.1-CL-200-Windows";

fn game() -> EpicRef {
    EpicRef { namespace: NS.into(), catalog_item: ITEM.into(), app_name: APP.into() }
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("u3d-epicdl-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Writes the little-endian numbers and Unreal strings of a manifest or chunk.
#[derive(Default)]
struct W(Vec<u8>);

impl W {
    fn u8(&mut self, v: u8) -> &mut Self {
        self.0.push(v);
        self
    }
    fn u32(&mut self, v: u32) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn u64(&mut self, v: u64) -> &mut Self {
        self.0.extend_from_slice(&v.to_le_bytes());
        self
    }
    fn bytes(&mut self, b: &[u8]) -> &mut Self {
        self.0.extend_from_slice(b);
        self
    }
    fn guid(&mut self, g: Guid) -> &mut Self {
        for n in g.0 {
            self.u32(n);
        }
        self
    }
    /// ASCII with its end zero, UTF-16 (a negative length) for other text, a zero length for "".
    fn fs(&mut self, s: &str) -> &mut Self {
        if s.is_empty() {
            return self.u32(0);
        }
        if s.is_ascii() {
            return self.u32(s.len() as u32 + 1).bytes(s.as_bytes()).u8(0);
        }
        let units: Vec<u16> = s.encode_utf16().chain([0]).collect();
        self.u32((-(units.len() as i32)) as u32);
        for u in units {
            self.bytes(&u.to_le_bytes());
        }
        self
    }
}

fn section(version: u8, body: &[u8]) -> Vec<u8> {
    let mut w = W::default();
    w.u32(body.len() as u32 + 5).u8(version).bytes(body);
    w.0
}

/// The feature level and the data version of each section.
#[derive(Clone, Copy)]
struct Ver {
    level: u32,
    meta: u8,
    files: u8,
    custom: u8,
}

const V: Ver = Ver { level: 21, meta: 2, files: 2, custom: 3 };

/// A manifest written the way Epic writes them: header, then the zlib packed body.
fn binary(m: &Manifest, v: Ver) -> Vec<u8> {
    let mt = &m.meta;
    let mut w = W::default();
    w.u32(v.level).u8(mt.is_file_data as u8).u32(mt.app_id).fs(&mt.app_name).fs(&mt.build_version).fs(&mt.launch_exe).fs(&mt.launch_command).u32(mt.prereq_ids.len() as u32);
    for p in &mt.prereq_ids {
        w.fs(p);
    }
    w.fs(&mt.prereq_name).fs(&mt.prereq_path).fs(&mt.prereq_args);
    if v.meta >= 1 {
        w.fs(&mt.build_id);
    }
    if v.meta >= 2 {
        w.fs("").fs("");
    }
    let mut body = section(v.meta, &w.0);

    let mut w = W::default();
    w.u32(m.chunks.len() as u32);
    for c in &m.chunks {
        w.guid(c.guid);
    }
    for c in &m.chunks {
        w.u64(c.hash);
    }
    for c in &m.chunks {
        w.bytes(&c.sha1);
    }
    for c in &m.chunks {
        w.u8(c.group);
    }
    for c in &m.chunks {
        w.u32(c.window_size);
    }
    for c in &m.chunks {
        w.u64(c.file_size);
    }
    if v.level >= 22 {
        for c in &m.chunks {
            w.guid(c.secret);
        }
        for c in &m.chunks {
            w.u32(c.file_size as u32);
        }
        for _ in &m.chunks {
            w.bytes(&[0; 16]);
        }
    }
    body.extend(section(0, &w.0));

    let mut w = W::default();
    w.u32(m.files.len() as u32);
    for f in &m.files {
        w.fs(&f.name);
    }
    for f in &m.files {
        w.fs(&f.symlink);
    }
    for f in &m.files {
        w.bytes(&f.sha1);
    }
    for f in &m.files {
        w.u8(f.flags);
    }
    for f in &m.files {
        w.u32(f.install_tags.len() as u32);
        for t in &f.install_tags {
            w.fs(t);
        }
    }
    for f in &m.files {
        w.u32(f.parts.len() as u32);
        for p in &f.parts {
            w.u32(28).guid(p.guid).u32(p.offset).u32(p.size);
        }
    }
    if v.files >= 1 {
        for i in 0..m.files.len() {
            if i % 2 == 0 {
                w.u32(1).bytes(&[7; 16]);
            } else {
                w.u32(0);
            }
        }
        for _ in &m.files {
            w.fs("application/octet-stream");
        }
    }
    if v.files >= 2 {
        for _ in &m.files {
            w.bytes(&[9; 32]);
        }
    }
    body.extend(section(v.files, &w.0));

    let mut w = W::default();
    w.u32(m.custom.len() as u32);
    for k in m.custom.keys() {
        w.fs(k);
    }
    for x in m.custom.values() {
        w.fs(x);
    }
    body.extend(section(v.custom, &w.0));

    let packed = miniz_oxide::deflate::compress_to_vec_zlib(&body, 6);
    let mut w = W::default();
    w.u32(MANIFEST_MAGIC).u32(if v.level >= 22 { 73 } else { 41 }).u32(body.len() as u32).u32(packed.len() as u32).bytes(&sha1(&body)).u8(1).u32(v.level);
    if v.level >= 22 {
        // The header's secret id and encryption tag: zero for a plain build.
        w.bytes(&[0; 32]);
    }
    w.bytes(&packed);
    w.0
}

/// The older JSON form of a manifest: numbers as 3-digit bytes, lowest first.
fn json_of(m: &Manifest) -> Vec<u8> {
    let blob = |b: &[u8]| b.iter().map(|x| format!("{x:03}")).collect::<String>();
    let num = |n: u64, width: usize| blob(&n.to_le_bytes()[..width]);
    let (mut hashes, mut shas, mut groups, mut sizes) = (serde_json::Map::new(), serde_json::Map::new(), serde_json::Map::new(), serde_json::Map::new());
    for c in &m.chunks {
        let k = c.guid.hex();
        hashes.insert(k.clone(), num(c.hash, 8).into());
        shas.insert(k.clone(), hex::encode_upper(c.sha1).into());
        groups.insert(k.clone(), num(c.group as u64, 1).into());
        sizes.insert(k, num(c.file_size, 8).into());
    }
    let files: Vec<Value> = m
        .files
        .iter()
        .map(|f| {
            let parts: Vec<Value> = f.parts.iter().map(|p| json!({"Guid": p.guid.hex(), "Offset": num(p.offset as u64, 4), "Size": num(p.size as u64, 4)})).collect();
            json!({"Filename": f.name, "FileHash": blob(&f.sha1), "bIsUnixExecutable": f.flags & 4 != 0, "InstallTags": f.install_tags, "FileChunkParts": parts})
        })
        .collect();
    let mt = &m.meta;
    serde_json::to_vec_pretty(&json!({
        "ManifestFileVersion": num(mt.feature_level as u64, 4),
        "bIsFileData": false,
        "AppID": num(0, 4),
        "AppNameString": mt.app_name,
        "BuildVersionString": mt.build_version,
        "LaunchExeString": mt.launch_exe,
        "LaunchCommand": mt.launch_command,
        "PrereqIds": mt.prereq_ids,
        "PrereqName": mt.prereq_name,
        "PrereqPath": mt.prereq_path,
        "PrereqArgs": mt.prereq_args,
        "FileManifestList": files,
        "ChunkHashList": hashes,
        "ChunkShaList": shas,
        "DataGroupList": groups,
        "ChunkFilesizeList": sizes,
        "CustomFields": m.custom,
    }))
    .unwrap()
}

/// A chunk file: header of `version`, then the data (zlib packed when `pack`).
fn chunk_with(version: u32, guid: Guid, data: &[u8], pack: bool, secret: Guid) -> Vec<u8> {
    let stored = if pack { miniz_oxide::deflate::compress_to_vec_zlib(data, 6) } else { data.to_vec() };
    let header = match version {
        1 => 41,
        2 => 62,
        3 => 66,
        _ => 98,
    };
    let mut w = W::default();
    w.u32(CHUNK_MAGIC).u32(version).u32(header).u32(stored.len() as u32).guid(guid).u64(rolling_hash(data)).u8(pack as u8);
    if version >= 2 {
        w.bytes(&sha1(data)).u8(3);
    }
    if version >= 3 {
        w.u32(data.len() as u32);
    }
    if version >= 4 {
        w.guid(secret).bytes(&[0; 16]);
    }
    w.bytes(&stored);
    w.0
}

fn chunk_file(guid: Guid, data: &[u8], pack: bool) -> Vec<u8> {
    chunk_with(3, guid, data, pack, Guid::default())
}

fn bytes(seed: u8, n: usize) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i / 256) as u8)).collect()
}

fn build1() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("Game/Binaries/Win64/Game.exe", bytes(1, 300)),
        ("Game/Content/Paks/data.pak", bytes(2, 1000)),
        ("Engine/readme.txt", bytes(3, 50)),
        ("Game/Content/Straße.txt", bytes(6, 40)),
        ("Game/empty.txt", vec![]),
    ]
}

/// The update: the program changed, a pak added, the readme dropped, the rest the same.
fn build2() -> Vec<(&'static str, Vec<u8>)> {
    vec![
        ("Game/Binaries/Win64/Game.exe", bytes(4, 320)),
        ("Game/Content/Paks/data.pak", bytes(2, 1000)),
        ("Game/Content/Paks/new.pak", bytes(5, 400)),
        ("Game/Content/Straße.txt", bytes(6, 40)),
        ("Game/empty.txt", vec![]),
    ]
}

/// A build: its manifest, the manifest file and its chunk files.
struct Fx {
    tag: u32,
    manifest: Manifest,
    raw: Vec<u8>,
    chunks: Vec<Vec<u8>>,
    files: Vec<(String, Vec<u8>)>,
}

const WINDOW: usize = 256;

fn fixture(tag: u32, build: &str, files: &[(&str, Vec<u8>)]) -> Fx {
    fixture_with(tag, build, files, V)
}

/// The files one after another, cut into chunks of `WINDOW` bytes: files are made of parts of
/// several chunks, and chunks hold parts of several files.
fn fixture_with(tag: u32, build: &str, files: &[(&str, Vec<u8>)], v: Ver) -> Fx {
    let stream: Vec<u8> = files.iter().flat_map(|(_, b)| b.iter().copied()).collect();
    let (mut chunks, mut infos) = (Vec::new(), Vec::new());
    for (i, piece) in stream.chunks(WINDOW).enumerate() {
        let guid = Guid([0xC0FF_EE00 + tag, i as u32, 0x5EED, 0x7E57]);
        let file = chunk_file(guid, piece, i % 2 == 0);
        infos.push(ChunkInfo { guid, hash: rolling_hash(piece), sha1: sha1(piece), group: group_of(guid), window_size: WINDOW as u32, file_size: file.len() as u64, secret: Guid::default() });
        chunks.push(file);
    }
    let mut at = 0;
    let mut entries = Vec::new();
    for (name, b) in files {
        let (mut parts, mut pos, end) = (Vec::new(), at, at + b.len());
        while pos < end {
            let (ci, off) = (pos / WINDOW, pos % WINDOW);
            let n = (WINDOW - off).min(end - pos);
            parts.push(ChunkPart { guid: infos[ci].guid, offset: off as u32, size: n as u32 });
            pos += n;
        }
        at = end;
        entries.push(FileEntry {
            name: name.to_string(),
            sha1: sha1(b),
            flags: if name.ends_with(".exe") { 4 } else { 0 },
            install_tags: if name.ends_with(".pak") { vec!["hd".into()] } else { vec![] },
            parts,
            ..Default::default()
        });
    }
    let manifest = Manifest {
        meta: Meta {
            feature_level: v.level,
            app_name: APP.into(),
            build_version: build.into(),
            launch_exe: "Game/Binaries/Win64/Game.exe".into(),
            launch_command: r#"-nosplash "-ini=a b""#.into(),
            prereq_ids: vec!["made-up-prereq".into()],
            prereq_name: "Made Up Redist".into(),
            prereq_path: "Redist/setup.exe".into(),
            prereq_args: "/quiet".into(),
            build_id: if v.meta >= 1 { "made-up-build-id".into() } else { String::new() },
            ..Default::default()
        },
        chunks: infos,
        files: entries,
        custom: BTreeMap::from([("MadeUpField".to_string(), "x".to_string())]),
    };
    let raw = binary(&manifest, v);
    Fx { tag, manifest, raw, chunks, files: files.iter().map(|(n, b)| (n.to_string(), b.clone())).collect() }
}

/// Epic's service and CDN, from memory. Notes every address asked and whether the token went
/// with it.
#[derive(Default)]
struct Mem {
    served: Mutex<HashMap<String, Vec<u8>>>,
    asked: Mutex<Vec<(String, bool)>>,
}

impl Fetch for Mem {
    fn get<'a>(&'a self, url: &'a str, bearer: Option<&'a str>) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.asked.lock().push((url.to_string(), bearer.is_some()));
            if bearer.is_some_and(|t| t != TOKEN) {
                bail!("HTTP 401");
            }
            self.served.lock().get(url).cloned().ok_or_else(|| anyhow!("HTTP 404"))
        })
    }
}

impl Mem {
    fn put(&self, url: &str, body: Vec<u8>) {
        self.served.lock().insert(url.to_string(), body);
    }
    fn chunk_urls(&self) -> BTreeSet<String> {
        self.asked.lock().iter().filter(|(u, _)| u.contains("/Chunks")).map(|(u, _)| u.clone()).collect()
    }
    fn chunk_requests(&self) -> usize {
        self.asked.lock().iter().filter(|(u, _)| u.contains("/Chunks")).count()
    }
    fn clear(&self) {
        self.asked.lock().clear();
    }
}

fn base(fx: &Fx) -> String {
    format!("https://cdn.example.test/Builds/Org/o-madeup/b{}/default", fx.tag)
}

fn manifest_url(fx: &Fx) -> String {
    format!("{}/{}.manifest?f_token=made-up&exp=1", base(fx), fx.tag)
}

/// Makes `fx` the build Epic serves now.
fn serve(mem: &Mem, fx: &Fx) {
    let b = base(fx);
    mem.put(&manifest_url(fx), fx.raw.clone());
    for (c, file) in fx.manifest.chunks.iter().zip(&fx.chunks) {
        mem.put(&chunk_url(&b, fx.manifest.meta.feature_level, c), file.clone());
    }
    let answer = json!({"elements": [{
        "appName": APP,
        "labelName": "Live-Windows",
        "buildVersion": fx.manifest.meta.build_version,
        "hash": hex::encode(sha1(&fx.raw)),
        "useSignedUrl": true,
        "manifests": [{"uri": format!("{b}/{}.manifest", fx.tag), "queryParams": [{"name": "f_token", "value": "made-up"}, {"name": "exp", "value": "1"}]}]
    }]});
    mem.put(&assets_url(NS, ITEM, APP), serde_json::to_vec(&answer).unwrap());
}

/// The chunk addresses of the named files of `fx`.
fn chunks_of(fx: &Fx, names: &[&str]) -> BTreeSet<String> {
    let index = fx.manifest.chunk_index();
    fx.manifest.files.iter().filter(|f| names.iter().any(|n| f.name.ends_with(n))).flat_map(|f| f.parts.iter()).map(|p| chunk_url(&base(fx), fx.manifest.meta.feature_level, index[&p.guid])).collect()
}

const QUICK: Tune = Tune { parallel: 3, cache_bytes: 64 << 20, in_memory: 32 << 20, report_every: Duration::ZERO };

async fn run(mem: &Arc<Mem>, dest: &Path, mode: Mode, tune: &Tune) -> Result<Report> {
    let cancel = AtomicBool::new(false);
    sync_with(mem.clone(), TOKEN, &game(), "Made Up Game", dest, mode, &|_| {}, &cancel, tune).await
}

/// Every file in the game folder but the app's own, with its bytes.
fn on_disk(dest: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !(dir == root && e.file_name().eq_ignore_ascii_case(OWN_DIR)) {
                    walk(root, &p, out);
                }
            } else {
                out.insert(p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/"), std::fs::read(&p).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(dest, dest, &mut out);
    out
}

fn expected(fx: &Fx) -> BTreeMap<String, Vec<u8>> {
    fx.files.iter().map(|(n, b)| (n.clone(), b.clone())).collect()
}

/// No part file left anywhere in the folder, the app's own included.
fn no_part_files(dest: &Path) -> bool {
    fn walk(dir: &Path) -> bool {
        let Ok(rd) = std::fs::read_dir(dir) else { return true };
        rd.flatten().all(|e| {
            let p = e.path();
            let n = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                walk(&p)
            } else {
                !n.contains("u3dpart") && !n.ends_with(".part")
            }
        })
    }
    walk(dest)
}

#[test]
fn a_binary_manifest_reads_whole() {
    for v in [V, Ver { level: 18, meta: 1, files: 1, custom: 0 }, Ver { level: 17, meta: 0, files: 0, custom: 0 }, Ver { level: 22, ..V }] {
        let fx = fixture_with(1, B1, &build1(), v);
        assert_eq!(Manifest::parse(&fx.raw).unwrap(), fx.manifest, "level {}", v.level);
    }
    let fx = fixture(1, B1, &build1());
    let m = Manifest::parse(&fx.raw).unwrap();
    assert_eq!(m.files.len(), 5);
    assert_eq!(m.meta.prereq_ids, ["made-up-prereq"]);
    assert_eq!(m.meta.build_id, "made-up-build-id");
    assert_eq!(m.custom_field("madeupfield"), Some("x"));
    assert_eq!(m.files[3].name, "Game/Content/Straße.txt");
    // Files of several chunks, and a chunk with parts of two files.
    assert!(m.files[1].parts.len() >= 4);
    assert!(m.files[0].parts.last().map(|p| p.guid) == m.files[1].parts.first().map(|p| p.guid));
    assert_eq!(m.install_size(), 1390);
    assert_eq!(m.files[4].size(), 0);
    // The start of the file still reads as before.
    let meta = build_meta(&fx.raw).unwrap();
    assert_eq!((meta.app_name.as_str(), meta.build.as_str(), meta.launch_exe.as_str()), (APP, B1, "Game/Binaries/Win64/Game.exe"));
}

#[test]
fn unknown_or_broken_manifests_are_refused_rather_than_guessed() {
    for (v, word) in [(Ver { meta: 3, ..V }, "build information"), (Ver { files: 3, ..V }, "file list"), (Ver { custom: 4, ..V }, "custom fields"), (Ver { level: 25, ..V }, "feature level 25")] {
        let fx = fixture_with(1, B1, &build1(), v);
        let e = Manifest::parse(&fx.raw).unwrap_err().to_string();
        assert!(e.contains(word) && e.contains("newer"), "{e}");
    }
    let fx = fixture(1, B1, &build1());
    let mut enc = fx.raw.clone();
    enc[36] |= 2;
    assert!(Manifest::parse(&enc).unwrap_err().to_string().contains("encrypted"));
    let mut broken = fx.raw.clone();
    let n = broken.len();
    broken[n - 20] ^= 0x55;
    assert!(Manifest::parse(&broken).is_err());
    assert!(Manifest::parse(&fx.raw[..100]).is_err());
    assert!(Manifest::parse(b"garbage").unwrap_err().to_string().contains("not an Epic build manifest"));
    assert!(Manifest::parse(b"\x0c{}").unwrap_err().to_string().contains("not an Epic build manifest"));
    assert!(Manifest::parse(b" \r\n{}").is_ok());
    assert!(Manifest::parse(&[]).is_err());
}

#[test]
fn the_json_form_reads_the_same() {
    let mut fx = fixture(1, B1, &build1());
    fx.manifest.meta.feature_level = 13;
    let m = Manifest::parse(&json_of(&fx.manifest)).unwrap();
    assert_eq!(m.meta.feature_level, 13);
    assert_eq!((m.meta.app_name.as_str(), m.meta.build_version.as_str(), m.meta.launch_exe.as_str()), (APP, B1, "Game/Binaries/Win64/Game.exe"));
    assert_eq!(m.meta.launch_command, fx.manifest.meta.launch_command);
    assert_eq!(m.meta.prereq_ids, ["made-up-prereq"]);
    assert_eq!(m.files, fx.manifest.files);
    assert_eq!(m.custom, fx.manifest.custom);
    let strip = |c: &ChunkInfo| (c.guid, c.hash, c.sha1, c.group, c.file_size);
    assert_eq!(m.chunks.iter().map(strip).collect::<Vec<_>>(), fx.manifest.chunks.iter().map(strip).collect::<Vec<_>>());
    assert!(m.chunks.iter().all(|c| c.window_size == 1 << 20));
    assert_eq!(blob_num("013000000000").unwrap(), 13);
    assert_eq!(blob_num("255001").unwrap(), 511);
    assert!(Manifest::parse(br#"{"ManifestFileVersion":"01x000000000"}"#).is_err());
    assert!(Manifest::parse(br#"{"ManifestFileVersion":"999000000000"}"#).is_err());
    assert!(Manifest::parse(br#"{"FileManifestList":[{"Filename":"a","FileChunkParts":[{"Guid":"nope"}]}]}"#).is_err());
}

#[test]
fn chunk_addresses_follow_the_feature_level() {
    let c = ChunkInfo { guid: Guid([1, 2, 3, 0xA]), hash: 0xDEAD_BEEF, group: 7, ..Default::default() };
    assert_eq!(chunk_path(21, &c), "ChunksV4/07/00000000DEADBEEF_0000000100000002000000030000000A.chunk");
    for (level, dir) in [(15, "ChunksV4/"), (14, "ChunksV3/"), (6, "ChunksV3/"), (5, "ChunksV2/"), (3, "ChunksV2/"), (2, "Chunks/")] {
        assert!(chunk_path(level, &c).starts_with(dir), "{level}");
    }
    // From feature level 22: plain chunks under "plain", hash and id in URL-safe base64.
    assert_eq!(chunk_path(22, &c), "ChunksV5/plain/07/776t3gAAAAA_AQAAAAIAAAADAAAACgAAAA.chunk");
    let secret = ChunkInfo { secret: Guid([1, 0, 0, 0]), ..c.clone() };
    assert!(chunk_path(22, &secret).starts_with("ChunksV5/AQAAAAAAAAAAAAAAAAAAAA/07/"));
    assert_eq!(chunk_url("https://cdn.example.test/b/", 21, &c), "https://cdn.example.test/b/ChunksV4/07/00000000DEADBEEF_0000000100000002000000030000000A.chunk");
    // The folder number when a manifest has none: CRC-32 of the id.
    assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    assert!(group_of(c.guid) < 100);
    assert_eq!(Guid::from_hex(&c.guid.hex()), Some(c.guid));
    assert_eq!(Guid::from_hex("0000000100000002000000030000000a"), Some(c.guid));
    assert_eq!(Guid::from_hex("123"), None);
}

#[test]
fn a_build_can_name_more_cdns_itself() {
    let mut m = Manifest::default();
    m.custom.insert("BaseUrl".into(), "https://x.example.test/a/, ftp://no.example.test, https://y.example.test/b".into());
    m.custom.insert("CloudDir".into(), "https://z.example.test/c".into());
    assert_eq!(m.own_bases(), ["https://x.example.test/a", "https://y.example.test/b", "https://z.example.test/c"]);
    assert!(Manifest::default().own_bases().is_empty());
}

#[test]
fn a_chunk_is_unpacked_and_checked_against_its_hash() {
    let data = bytes(9, 700);
    let g = Guid([1, 2, 3, 4]);
    for pack in [true, false] {
        let (h, d) = read_chunk(&chunk_file(g, &data, pack)).unwrap();
        assert_eq!(d, data);
        assert_eq!((h.guid, h.version, h.hash_type, h.uncompressed_size), (g, 3, 3, 700));
    }
    // Older headers (version 1 has only the rolling hash) and the newest (a zero secret is plain).
    for v in [1, 2, 4] {
        assert_eq!(read_chunk(&chunk_with(v, g, &data, true, Guid::default())).unwrap().1, data, "version {v}");
    }
    let mut v1 = chunk_with(1, g, &data, false, Guid::default());
    let n = v1.len();
    v1[n - 1] ^= 1;
    assert!(read_chunk(&v1).unwrap_err().to_string().contains("does not match its hash"));
    // Data that is not what its SHA1 says.
    let mut bad = chunk_file(g, &data, false);
    let n = bad.len();
    bad[n - 1] ^= 1;
    assert!(read_chunk(&bad).unwrap_err().to_string().contains("does not match its hash"));
    // Encrypted, newer, cut short, not a chunk.
    let mut enc = chunk_file(g, &data, false);
    enc[40] |= 2;
    assert!(read_chunk(&enc).unwrap_err().to_string().contains("encrypted"));
    assert!(read_chunk(&chunk_with(4, g, &data, false, Guid([5, 0, 0, 0]))).unwrap_err().to_string().contains("encrypted"));
    let mut newer = chunk_file(g, &data, false);
    newer[4] = 9;
    assert!(read_chunk(&newer).unwrap_err().to_string().contains("newer"));
    let whole = chunk_file(g, &data, true);
    assert!(read_chunk(&whole[..whole.len() - 5]).is_err());
    assert!(read_chunk(b"nope").is_err());
    // The manifest's SHA1 is checked when the chunk has none of its own; the id always.
    let job = |sha| Job { guid: g, urls: vec![], sha1: Some(sha) };
    let v1 = chunk_with(1, g, &data, false, Guid::default());
    assert!(check_chunk(&v1, &job(sha1(&data))).is_ok());
    assert!(check_chunk(&v1, &job([0; 20])).unwrap_err().to_string().contains("does not match its hash"));
    let other = Job { guid: Guid([9, 9, 9, 9]), urls: vec![], sha1: None };
    assert!(check_chunk(&chunk_file(g, &data, true), &other).is_err());
}

#[tokio::test]
async fn an_install_writes_every_file_and_checks_it() {
    let dir = tmp("install");
    let fx = fixture(1, B1, &build1());
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    let r = run(&mem, &dir, Mode::Install, &QUICK).await.unwrap();
    assert_eq!(on_disk(&dir), expected(&fx));
    for f in &fx.manifest.files {
        assert_eq!(sha1(&std::fs::read(dir.join(safe_rel(&f.name).unwrap())).unwrap()), f.sha1, "{}", f.name);
    }
    assert_eq!((r.version.as_str(), r.files_written, r.files_repaired), ("1.0", 5, 0));
    assert_eq!(r.bytes_downloaded, fx.chunks.iter().map(|c| c.len() as u64).sum::<u64>());
    let s = read_state(&dir).unwrap();
    assert_eq!((s.platform.as_str(), s.game.as_str(), s.name.as_str(), s.version.as_str(), s.build.as_str()), ("epic", APP, "Made Up Game", "1.0", B1));
    assert_eq!(s.launch, r"Game\Binaries\Win64\Game.exe");
    assert_eq!(s.launch_args, ["-nosplash", "-ini=a b"]);
    assert_eq!(std::fs::read(saved_path(&dir)).unwrap(), fx.raw);
    assert!(!chunks_dir(&dir).exists());
    assert!(no_part_files(&dir));
    // Every chunk once; the token went to Epic's launcher service only.
    assert_eq!((mem.chunk_requests(), mem.chunk_urls().len()), (fx.chunks.len(), fx.chunks.len()));
    assert!(mem.asked.lock().iter().all(|(u, bearer)| *bearer == u.starts_with(LAUNCHER_HOST)));
    // Installing again finds every file right and downloads nothing.
    mem.clear();
    let r = run(&mem, &dir, Mode::Install, &QUICK).await.unwrap();
    assert_eq!(r.files_written, 0);
    assert!(mem.chunk_urls().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_tiny_chunk_cache_and_part_files_give_the_same_files() {
    let dir = tmp("tiny");
    let fx = fixture(1, B1, &build1());
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    // No chunk kept in memory and every file put together in a part file.
    let tune = Tune { parallel: 2, cache_bytes: 0, in_memory: 0, report_every: Duration::ZERO };
    let waited = AtomicBool::new(false);
    let seen = |_: Progress| {
        if chunks_dir(&dir).exists() {
            waited.store(true, Ordering::Relaxed);
        }
    };
    let cancel = AtomicBool::new(false);
    sync_with(mem.clone(), TOKEN, &game(), "Made Up Game", &dir, Mode::Install, &seen, &cancel, &tune).await.unwrap();
    assert!(waited.load(Ordering::Relaxed), "chunks needed later waited on disk");
    assert_eq!(on_disk(&dir), expected(&fx));
    assert_eq!(mem.chunk_requests(), fx.chunks.len());
    assert!(!chunks_dir(&dir).exists());
    assert!(no_part_files(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_update_downloads_only_what_changed_and_can_be_put_back() {
    let dir = tmp("update");
    let (b1, b2) = (fixture(1, B1, &build1()), fixture(2, B2, &build2()));
    let mem = Arc::new(Mem::default());
    serve(&mem, &b1);
    run(&mem, &dir, Mode::Install, &QUICK).await.unwrap();
    serve(&mem, &b2);
    mem.clear();
    let r = run(&mem, &dir, Mode::Update, &QUICK).await.unwrap();
    assert_eq!(on_disk(&dir), expected(&b2));
    assert_eq!((r.version.as_str(), r.files_written), ("1.1", 2));
    assert!(r.undo.is_some());
    // Only the chunks of the changed and the new file came.
    let wanted = chunks_of(&b2, &["Game.exe", "new.pak"]);
    assert_eq!(mem.chunk_urls(), wanted);
    assert!(wanted.len() < b2.chunks.len());
    assert!(chunks_of(&b2, &["data.pak"]).difference(&wanted).count() > 0);
    assert_eq!(read_state(&dir).unwrap().build, B2);
    // Put back: the first build exactly.
    let back = sync::undo(&dir).unwrap().unwrap();
    assert_eq!(back.build, B1);
    assert_eq!(on_disk(&dir), expected(&b1));
    assert_eq!(read_state(&dir).unwrap().build, B1);
    // Updating again compares with the first build's manifest, which was kept.
    mem.clear();
    let r = run(&mem, &dir, Mode::Update, &QUICK).await.unwrap();
    assert_eq!(on_disk(&dir), expected(&b2));
    assert_eq!(mem.chunk_urls(), wanted);
    assert_eq!(r.files_written, 2);
    // Nothing new: nothing downloaded or written, and the update can still be put back.
    mem.clear();
    let r = run(&mem, &dir, Mode::Update, &QUICK).await.unwrap();
    assert_eq!((r.files_written, r.bytes_downloaded), (0, 0));
    assert!(mem.chunk_urls().is_empty());
    assert!(sync::can_undo(&dir).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_repair_finds_and_rewrites_broken_files() {
    let dir = tmp("repair");
    let fx = fixture(1, B1, &build1());
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    run(&mem, &dir, Mode::Install, &QUICK).await.unwrap();
    // One file changed (same size), one gone.
    let pak = dir.join(r"Game\Content\Paks\data.pak");
    let mut b = std::fs::read(&pak).unwrap();
    b[500] ^= 0xFF;
    std::fs::write(&pak, b).unwrap();
    std::fs::remove_file(dir.join(r"Engine\readme.txt")).unwrap();
    mem.clear();
    let r = run(&mem, &dir, Mode::Repair, &QUICK).await.unwrap();
    assert_eq!((r.files_repaired, r.files_written), (2, 2));
    assert_eq!(on_disk(&dir), expected(&fx));
    assert_eq!(mem.chunk_urls(), chunks_of(&fx, &["data.pak", "readme.txt"]));
    // Nothing wrong: nothing written.
    mem.clear();
    let r = run(&mem, &dir, Mode::Repair, &QUICK).await.unwrap();
    assert_eq!((r.files_repaired, r.files_written), (0, 0));
    assert!(mem.chunk_urls().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_build_with_a_path_out_of_the_folder_is_refused() {
    let dir = tmp("evil");
    let dest = dir.join("game");
    let fx = fixture(1, B1, &[("Game/ok.txt", bytes(1, 10)), ("../evil.dll", bytes(2, 10))]);
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    let e = run(&mem, &dest, Mode::Install, &QUICK).await.unwrap_err();
    assert!(format!("{e:#}").contains("outside the game folder"), "{e:#}");
    assert!(!dir.join("evil.dll").exists());
    assert!(on_disk(&dest).is_empty());
    assert!(mem.chunk_urls().is_empty());
    for bad in ["C:/Windows/evil.dll", "/abs.dll", ".u3d/game.json", "a/../../b", ""] {
        assert!(fixture(1, B1, &[(bad, bytes(1, 5))]).manifest.checked_paths().is_err(), "{bad}");
    }
    // A link, a file named twice, a part past its chunk, an encrypted chunk.
    let ok = || fixture(1, B1, &build1()).manifest;
    assert!(ok().checked_paths().is_ok());
    let mut m = ok();
    m.files[0].symlink = "elsewhere".into();
    assert!(m.checked_paths().is_err());
    let mut m = ok();
    m.files[1].name = "game/binaries/WIN64/game.exe".into();
    assert!(m.checked_paths().unwrap_err().to_string().contains("twice"));
    let mut m = ok();
    m.files[0].parts[0].offset = 250;
    assert!(m.checked_paths().unwrap_err().to_string().contains("past the end"));
    let mut m = ok();
    m.chunks[0].secret = Guid([1, 0, 0, 0]);
    assert!(m.checked_paths().unwrap_err().to_string().contains("encrypted"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_chunk_that_does_not_match_its_hash_is_refused() {
    let dir = tmp("badchunk");
    let fx = fixture(1, B1, &build1());
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    // The CDN serves a broken copy of a chunk the first two files share.
    let c = &fx.manifest.chunks[1];
    let mut bad = fx.chunks[1].clone();
    let n = bad.len();
    bad[n - 10] ^= 1;
    mem.put(&chunk_url(&base(&fx), 21, c), bad);
    let e = format!("{:#}", run(&mem, &dir, Mode::Install, &QUICK).await.unwrap_err());
    assert!(e.contains("Could not download chunk") && e.contains("does not match its hash"), "{e}");
    for f in fx.manifest.files.iter().filter(|f| f.parts.iter().any(|p| p.guid == c.guid)) {
        assert!(!dir.join(safe_rel(&f.name).unwrap()).exists(), "{}", f.name);
    }
    assert!(read_state(&dir).is_none());
    assert!(no_part_files(&dir));
    assert!(!chunks_dir(&dir).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_canceled_update_leaves_the_build_before() {
    let dir = tmp("cancel");
    let (b1, b2) = (fixture(1, B1, &build1()), fixture(2, B2, &build2()));
    let mem = Arc::new(Mem::default());
    serve(&mem, &b1);
    run(&mem, &dir, Mode::Install, &QUICK).await.unwrap();
    serve(&mem, &b2);
    // Canceled once the first file of the update is written.
    let cancel = AtomicBool::new(false);
    let stop = |p: Progress| {
        if p.step == "Downloading" && p.files_done >= 1 {
            cancel.store(true, Ordering::Relaxed);
        }
    };
    let e = sync_with(mem.clone(), TOKEN, &game(), "Made Up Game", &dir, Mode::Update, &stop, &cancel, &QUICK).await.unwrap_err();
    assert!(e.to_string().starts_with("CANCELED|"), "{e:#}");
    assert_eq!(on_disk(&dir), expected(&b1));
    assert_eq!(read_state(&dir).unwrap().build, B1);
    assert_eq!(std::fs::read(saved_path(&dir)).unwrap(), b1.raw);
    assert!(sync::can_undo(&dir).is_none());
    assert!(no_part_files(&dir));
    assert!(!chunks_dir(&dir).exists());
    // The next update runs through.
    run(&mem, &dir, Mode::Update, &QUICK).await.unwrap();
    assert_eq!(on_disk(&dir), expected(&b2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn only_folders_the_app_installed_are_updated_or_repaired() {
    let dir = tmp("foreign");
    let fx = fixture(1, B1, &build1());
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    for mode in [Mode::Update, Mode::Repair] {
        assert!(run(&mem, &dir, mode, &QUICK).await.unwrap_err().to_string().contains("did not install"));
    }
    let other = State { platform: "gog".into(), game: "1".into(), name: "Other".into(), version: "1".into(), build: "1".into(), launch: String::new(), launch_args: vec![] };
    write_state(&dir, &other).unwrap();
    assert!(run(&mem, &dir, Mode::Install, &QUICK).await.unwrap_err().to_string().contains("another game"));
    assert!(mem.chunk_urls().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_build_with_a_json_manifest_installs_too() {
    let dir = tmp("json");
    let mut fx = fixture(3, "1.2.3", &build1());
    fx.manifest.meta.feature_level = 13;
    for c in &mut fx.manifest.chunks {
        c.window_size = 1 << 20;
    }
    fx.raw = json_of(&fx.manifest);
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    let r = run(&mem, &dir, Mode::Install, &QUICK).await.unwrap();
    assert_eq!(r.version, "1.2.3");
    assert_eq!(on_disk(&dir), expected(&fx));
    assert!(mem.chunk_urls().iter().all(|u| u.contains("/ChunksV3/")));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_launcher_service_gives_the_build_and_where_it_is() {
    let fx = fixture(1, B1, &build1());
    let mem = Arc::new(Mem::default());
    serve(&mem, &fx);
    assert_eq!(latest_with(&*mem, TOKEN, NS, ITEM, APP).await.unwrap(), B1);
    let live = live_manifest(&*mem, TOKEN, NS, ITEM, APP).await.unwrap();
    assert_eq!((&live.manifest, &live.raw), (&fx.manifest, &fx.raw));
    assert_eq!(live.bases, [base(&fx)]);
    // The service with the token; the manifest with its query and without the token.
    let asked = mem.asked.lock().clone();
    assert_eq!(asked[0], (assets_url(NS, ITEM, APP), true));
    assert!(asked.contains(&(manifest_url(&fx), false)));
    assert!(latest_with(&*mem, "wrong-token", NS, ITEM, APP).await.is_err());
    // A manifest that does not match the service's checksum is not used.
    mem.put(&manifest_url(&fx), fixture(2, B2, &build2()).raw);
    assert!(format!("{:#}", live_manifest(&*mem, TOKEN, NS, ITEM, APP).await.err().unwrap()).contains("checksum"));
    // Several manifest addresses: each with its query, the CDN bases once each.
    let a = parse_assets(
        br#"{"elements":[{"appName":"MadeUpApp","buildVersion":"1.0","hash":"ab","manifests":[
            {"uri":"https://a.example.test/B/x/m.manifest","queryParams":[{"name":"t","value":"1"}]},
            {"uri":"https://b.example.test/B/x/m.manifest"},
            {"uri":"https://a.example.test/B/x/m.manifest"}]}]}"#,
        APP,
    )
    .unwrap();
    assert_eq!(a.manifests, ["https://a.example.test/B/x/m.manifest?t=1", "https://b.example.test/B/x/m.manifest", "https://a.example.test/B/x/m.manifest"]);
    assert_eq!(a.bases, ["https://a.example.test/B/x", "https://b.example.test/B/x"]);
    assert_eq!(a.build, "1.0");
    assert!(parse_assets(br#"{"elements":[]}"#, APP).is_err());
    assert!(parse_assets(b"<html>", APP).is_err());
    assert_eq!(assets_url("ns", "item", "App Name"), format!("{LAUNCHER_HOST}/launcher/api/public/assets/v2/platform/Windows/namespace/ns/catalogItem/item/app/App%20Name/label/Live"));
}

#[test]
fn a_game_starts_with_its_own_command_and_the_sign_in_set() {
    let s = State { platform: "epic".into(), game: APP.into(), name: "Made Up Game".into(), version: "1.0".into(), build: B1.into(), launch: r"Game\Game.exe".into(), launch_args: vec!["-nosplash".into()] };
    let a = launch_args(&s, Some("made-up-code"), "made-up-account", "Made Up Name");
    assert_eq!(
        a[..9],
        ["-nosplash", "-AUTH_LOGIN=unused", "-AUTH_PASSWORD=made-up-code", "-AUTH_TYPE=exchangecode", "-epicapp=MadeUpApp", "-epicenv=Prod", "-EpicPortal", "-epicusername=Made Up Name", "-epicuserid=made-up-account"]
    );
    assert_eq!(a.len(), 10);
    let locale = a[9].strip_prefix("-epiclocale=").unwrap();
    assert!(!locale.is_empty() && locale.chars().all(|c| c.is_ascii_lowercase()), "{locale}");
    // Offline: no exchange code.
    assert!(launch_args(&s, None, "a", "n").iter().any(|x| x == "-AUTH_PASSWORD="));
    assert_eq!(split_command(r#"-a  "b c" -d="e f" "" g"#), ["-a", "b c", "-d=e f", "", "g"]);
    assert!(split_command("  ").is_empty());
}

#[test]
fn a_sync_needs_room_for_its_files() {
    let dir = tmp("space");
    assert!(check_space(&dir, u64::MAX).unwrap_err().to_string().contains("Not enough free space"));
    assert!(check_space(&dir, 1).is_ok());
    assert_eq!(size_text(3 << 20), "3 MB");
    assert_eq!(size_text(5 << 30), "5.0 GB");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_sync_can_run_on_any_thread() {
    fn send<T: Send>(_: &T) {}
    let cancel = AtomicBool::new(false);
    let g = game();
    let quiet = |_: Progress| {};
    let f = sync(TOKEN, &g, "n", Path::new("unused"), Mode::Install, &quiet, &cancel);
    send(&f);
}

/// Reads a real build manifest, without network or account:
/// `EPIC_MANIFEST=<path of a .manifest file> cargo test --lib epic_dl -- --ignored --nocapture`
/// With `EPIC_CHUNKS=<folder of .chunk files>` (the launcher's `.egstore\chunk_cache`) it also
/// reads those chunks and checks them against the manifest.
#[test]
#[ignore]
fn a_real_manifest_reads() {
    let Some(p) = std::env::var_os("EPIC_MANIFEST") else {
        eprintln!("EPIC_MANIFEST is not set");
        return;
    };
    let raw = std::fs::read(p).unwrap();
    let m = Manifest::parse(&raw).unwrap();
    let meta = build_meta(&raw).unwrap();
    assert_eq!((meta.app_name.as_str(), meta.build.as_str()), (m.meta.app_name.as_str(), m.meta.build_version.as_str()));
    assert_eq!(m.checked_paths().unwrap().len(), m.files.len());
    let index = m.chunk_index();
    assert!(m.files.iter().flat_map(|f| &f.parts).all(|p| index.contains_key(&p.guid)));
    let other_group = m.chunks.iter().filter(|c| c.group != group_of(c.guid)).count();
    let untagged: u64 = m.files.iter().filter(|f| f.install_tags.iter().all(|t| t.is_empty())).map(FileEntry::size).sum();
    println!(
        "feature level {}, {} files, {} chunks, {} installed ({} in files without an install tag), {} to download, {} chunks in another folder than their CRC says, first chunk at {}",
        m.meta.feature_level,
        m.files.len(),
        m.chunks.len(),
        size_text(m.install_size()),
        size_text(untagged),
        size_text(m.chunks.iter().map(|c| c.file_size).sum()),
        other_group,
        m.chunks.first().map(|c| m.chunk_path(c)).unwrap_or_default()
    );
    let Some(dir) = std::env::var_os("EPIC_CHUNKS") else { return };
    let (mut files, mut chunk_files, mut listed) = (0, 0, 0);
    let mut kinds = BTreeSet::new();
    let by_sha: HashMap<[u8; 20], &ChunkInfo> = m.chunks.iter().map(|c| (c.sha1, c)).collect();
    for e in std::fs::read_dir(dir).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "chunk")) {
        files += 1;
        let raw = std::fs::read(e.path()).unwrap();
        let (guid, data) = match read_chunk(&raw) {
            Ok((h, data)) => {
                chunk_files += 1;
                kinds.insert((h.version, h.hash_type, h.stored_as, h.header_size));
                (Some(h.guid), data)
            }
            // The launcher's own cache keeps chunks unpacked, named by their id.
            Err(_) => (e.path().file_stem().and_then(|s| Guid::from_hex(&s.to_string_lossy())), raw),
        };
        let sum = sha1(&data);
        if let Some(c) = guid.and_then(|g| index.get(&g).copied()).or_else(|| by_sha.get(&sum).copied()) {
            listed += 1;
            assert_eq!((sum, rolling_hash(&data), data.len() as u32), (c.sha1, c.hash, c.window_size), "{}", c.guid);
        }
    }
    println!("{files} files, {chunk_files} of them CDN chunk files (version, hash type, stored as, header size: {kinds:?}), {listed} in the manifest with its SHA1, rolling hash and size");
}

/// Rebuilds chunks of a real build from its installed files (read only) and checks them against
/// the manifest's SHA1 and rolling hash, and checks the SHA1 of a few small files:
/// `EPIC_MANIFEST=<the game's .egstore\*.manifest> EPIC_GAME_DIR=<the game's folder> cargo test
/// --lib epic_dl -- --ignored --nocapture`
#[test]
#[ignore]
fn a_real_build_matches_its_installed_files() {
    use std::io::Seek;
    let (Some(p), Some(game)) = (std::env::var_os("EPIC_MANIFEST"), std::env::var_os("EPIC_GAME_DIR")) else {
        eprintln!("EPIC_MANIFEST and EPIC_GAME_DIR are not set");
        return;
    };
    let game = PathBuf::from(game);
    let m = Manifest::parse(&std::fs::read(p).unwrap()).unwrap();
    let rels = m.checked_paths().unwrap();
    // Where each chunk's bytes are in the files: (offset in the chunk, file, offset in the file, size).
    let mut places: HashMap<Guid, Vec<(u32, usize, u64, u32)>> = HashMap::new();
    for (i, f) in m.files.iter().enumerate() {
        let mut at = 0u64;
        for p in &f.parts {
            places.entry(p.guid).or_default().push((p.offset, i, at, p.size));
            at += p.size as u64;
        }
    }
    let (mut ok, mut bad) = (0, 0);
    for c in &m.chunks {
        if ok + bad >= 40 {
            break;
        }
        let Some(ps) = places.get_mut(&c.guid) else { continue };
        ps.sort();
        ps.dedup_by_key(|p| p.0);
        // Only chunks whose whole window is in the files, one piece after another.
        let mut end = 0u32;
        if !ps.iter().all(|p| {
            let next = p.0 == end;
            end += p.3;
            next
        }) || end != c.window_size
        {
            continue;
        }
        let mut data = Vec::new();
        for &(_, i, at, size) in ps.iter() {
            let Ok(mut f) = std::fs::File::open(game.join(&rels[i])) else { break };
            f.seek(std::io::SeekFrom::Start(at)).unwrap();
            let mut piece = vec![0u8; size as usize];
            if f.read_exact(&mut piece).is_err() {
                break;
            }
            data.extend(piece);
        }
        if data.len() != c.window_size as usize {
            continue;
        }
        if sha1(&data) == c.sha1 && rolling_hash(&data) == c.hash {
            ok += 1;
        } else {
            bad += 1;
        }
    }
    let mut small: Vec<usize> = (0..m.files.len()).filter(|&i| m.files[i].size() > 0 && game.join(&rels[i]).is_file()).collect();
    small.sort_by_key(|&i| m.files[i].size());
    let same = small.iter().take(5).filter(|&&i| sha1(&std::fs::read(game.join(&rels[i])).unwrap()) == m.files[i].sha1).count();
    println!("{ok} chunks rebuilt from the installed files match the manifest's SHA1 and rolling hash, {bad} do not; {same} of the 5 smallest files match their SHA1");
    assert!(ok > 0 && bad == 0);
}
