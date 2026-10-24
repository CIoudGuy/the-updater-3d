//! Programs no catalog lists whose uninstall entry names their own GitHub project (its home page,
//! help or update link, on github.com or the project's github.io site). The project's releases
//! give the newest version and the changes of each one. When a release has exactly one installer
//! of the type that installed the copy here, for this processor and with GitHub's checksum, the
//! update runs here like any other; otherwise the release page is opened to download it.

use super::{Download, Installed, Latest, Plan, SourceData, Step, Verify};
use crate::http;
use crate::model::{JobKind, Source, VersionInfo};
use crate::paths;
use crate::sources::other::{self, safe_name};
use crate::sources::tools::{cmp_ver, is_pre};
use crate::uninstall::switches::{self, Kind};
use crate::winutil::UninstallEntry;
use anyhow::{anyhow, Result};
use futures_util::StreamExt;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

#[cfg(test)]
mod tests;

/// One release of a project.
#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub tag: String,
    /// The tag without what comes before its number: "v1.1.0" -> "1.1.0"
    pub version: String,
    pub body: String,
    /// The release page
    pub url: String,
    pub date: Option<String>,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    /// Lowercase hex, GitHub's digest of the file
    pub sha256: Option<String>,
}

/// A program whose releases are read: its project and the installer type that installed it.
#[derive(Clone, Debug)]
pub struct Tracked {
    /// "owner/repo" as the entry names it
    pub repo: String,
    pub kind: Kind,
}

/// What the releases of the named projects say.
#[derive(Clone, Debug, Default)]
pub struct Feed {
    /// Program id -> its project
    pub apps: HashMap<String, Tracked>,
    /// "owner/repo" in lowercase -> its releases, newest first. Projects whose releases could
    /// not be read (or that have none) are not here.
    pub repos: HashMap<String, Vec<Release>>,
    /// "owner/repo" in lowercase -> the project's own one-line description
    pub about: HashMap<String, String>,
}

impl Feed {
    /// The project of a program and its releases, when they were read.
    pub fn get(&self, app_id: &str) -> Option<(&Tracked, &[Release])> {
        let t = self.apps.get(app_id)?;
        let rels = self.repos.get(&t.repo.to_ascii_lowercase())?;
        Some((t, rels.as_slice()))
    }

    /// The project's description of a tracked program, for its page and tile.
    pub fn about_of(&self, app_id: &str) -> Option<&str> {
        let t = self.apps.get(app_id)?;
        self.about.get(&t.repo.to_ascii_lowercase()).map(String::as_str)
    }
}

/// A project's description as a short line: one sentence, no emoji codes or links, at most about
/// 140 characters.
pub fn short_about(d: &str) -> Option<String> {
    let t: String = d.split_whitespace().filter(|w| !(w.starts_with(':') && w.ends_with(':') && w.len() > 2) && !w.starts_with("http")).collect::<Vec<_>>().join(" ");
    let t = t.trim().trim_end_matches('.').to_string();
    if t.chars().filter(|c| c.is_alphanumeric()).count() < 4 {
        return None;
    }
    if t.chars().count() <= 140 {
        return Some(t);
    }
    let cut: String = t.chars().take(140).collect();
    Some(match cut.rfind(' ') {
        Some(i) => format!("{}...", cut[..i].trim_end_matches([',', ';', ':'])),
        None => cut,
    })
}

async fn about(repo: &str) -> Option<String> {
    let url = format!("https://api.github.com/repos/{repo}");
    let f = tokio::time::timeout(Duration::from_secs(15), http::get_cached(&url, &[("Accept", "application/vnd.github+json")], Duration::from_secs(24 * 3600))).await.ok()?.ok()?;
    let v: Value = serde_json::from_slice(&f.body).ok()?;
    v["description"].as_str().and_then(short_about)
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The program's name without the version some installers add to it ("Tidalfast 1.0.2-alpha.3").
fn base_name(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().take_while(|w| !w.trim_start_matches(['v', 'V']).starts_with(|c: char| c.is_ascii_digit())).collect();
    words.join(" ")
}

/// Is the project `repo` the program itself? Their names agree ("TidaLuna" and "TidaLunar").
fn same_project(repo: &str, name: &str) -> bool {
    let r = norm(repo);
    let b = base_name(name);
    let n = norm(if b.is_empty() { name } else { &b });
    r.len() >= 3 && n.len() >= 3 && (n.contains(&r) || r.contains(&n))
}

/// Owner and project names as GitHub allows them (nothing that could leave the address).
fn valid(owner: &str, repo: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 39
        && owner.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        && !repo.is_empty()
        && repo.len() <= 100
        && repo.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && repo.chars().any(|c| c.is_ascii_alphanumeric())
}

/// "https://owner.github.io/repo/..." -> (owner, repo): a project's own site on GitHub Pages.
fn pages_repo(url: &str) -> Option<(String, String)> {
    let url = url.trim();
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let owner = host.to_ascii_lowercase().strip_suffix(".github.io")?.to_string();
    if owner.is_empty() || owner.contains('.') {
        return None;
    }
    let repo = path.split(['/', '?', '#']).next().filter(|s| !s.is_empty())?;
    Some((owner, repo.to_string()))
}

/// Programs whose entry links another project than the one that builds them: their name (as
/// `norm` writes it, without a version) and their own project. TidaLunar's entry links TidaLuna,
/// the mod it is built on; it comes from the Brskt/TidaLuna fork.
const KNOWN: &[(&str, &str)] = &[("tidalunar", "Brskt/TidaLuna")];

/// The GitHub project ("owner", "repo") an uninstall entry names as its own.
pub fn repo_of(e: &UninstallEntry) -> Option<(String, String)> {
    let name = norm(&base_name(&e.display_name));
    if let Some((_, repo)) = KNOWN.iter().find(|(n, _)| *n == name) {
        let (o, r) = repo.split_once('/')?;
        return Some((o.to_string(), r.to_string()));
    }
    let links = [&e.url_info_about, &e.help_link, &e.url_update_info];
    let on_github = links.iter().filter_map(|u| other::github_repo(u.trim()));
    let on_pages = links.iter().filter_map(|u| pages_repo(u));
    on_github.chain(on_pages).find(|(o, r)| valid(o, r) && same_project(r, &e.display_name))
}

/// The version a release tag names: "v1.1.0", "tidalfast-v1.1.0" and "1.1.0" -> "1.1.0".
pub fn version_of_tag(tag: &str) -> Option<String> {
    let i = tag.find(|c: char| c.is_ascii_digit())?;
    Some(tag[i..].trim().to_string()).filter(|v| !v.is_empty())
}

/// The releases in GitHub's answer, newest first. Drafts and tags without a version are left out;
/// a file counts only when it is a GitHub release download.
pub fn parse_releases(v: &Value) -> Vec<Release> {
    let mut out: Vec<Release> = v
        .as_array()
        .into_iter()
        .flatten()
        .filter(|r| r["draft"].as_bool() != Some(true))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?.to_string();
            let version = version_of_tag(&tag)?;
            let assets = r["assets"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|a| {
                    let url = a["browser_download_url"].as_str()?;
                    if !url.starts_with("https://github.com/") || !url.contains("/releases/download/") {
                        return None;
                    }
                    let sha256 = a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|s| s.to_ascii_lowercase()).filter(|s| s.len() == 64 && s.chars().all(|c| c.is_ascii_hexdigit()));
                    Some(Asset { name: a["name"].as_str().unwrap_or("").to_string(), url: url.to_string(), size: a["size"].as_u64(), sha256 })
                })
                .collect();
            Some(Release {
                tag,
                version,
                body: r["body"].as_str().unwrap_or("").to_string(),
                url: r["html_url"].as_str().filter(|u| u.starts_with("https://github.com/")).unwrap_or("").to_string(),
                date: r["published_at"].as_str().map(String::from),
                prerelease: r["prerelease"].as_bool().unwrap_or(false),
                assets,
            })
        })
        .collect();
    out.sort_by(|a, b| cmp_ver(&b.version, &a.version));
    out
}

/// Does the project name its normal releases like pre-releases ("0.0.19-alpha") and mark its
/// real pre-releases with GitHub's flag only (TidaLunar's dev builds)?
fn flag_only(rels: &[Release]) -> bool {
    let normal: Vec<&Release> = rels.iter().filter(|r| !r.prerelease).take(3).collect();
    !normal.is_empty() && normal.iter().all(|r| is_pre(&r.version))
}

/// Is this release a pre-release of its project: flagged so, or named so when the project's normal
/// releases are not.
fn pre(rels: &[Release], r: &Release) -> bool {
    r.prerelease || (!flag_only(rels) && is_pre(&r.version))
}

/// Is this release the installed version? The same text, or the same number for a normal
/// release ("1.1.0" and "1.1.0.0"; "0.0.18-alpha.dev.4" reads like "0.0.18-alpha" but is not it).
fn is_version(r: &Release, installed: &str) -> bool {
    r.version.eq_ignore_ascii_case(installed.trim()) || (!r.prerelease && cmp_ver(&r.version, installed).is_eq())
}

/// Is the installed copy a pre-release? Named so, unless the project names its normal releases
/// so too; then only when it is one of the flagged ones.
fn pre_copy(rels: &[Release], installed: &str) -> bool {
    if flag_only(rels) {
        rels.iter().any(|r| r.prerelease && is_version(r, installed))
    } else {
        is_pre(installed)
    }
}

/// The releases numbered like the project numbers them now, in the same order: an old tag
/// numbered another way (ninja's "release-120715" from 2012, next to its 1.13) is not newer than
/// what came after it. Nothing goes past the first number of the release published last.
fn in_line(rels: &[Release]) -> Vec<&Release> {
    let last = rels.iter().filter(|r| r.date.is_some()).max_by(|a, b| a.date.cmp(&b.date)).or(rels.first());
    let top = last.and_then(|r| major(&r.version));
    rels.iter().filter(|r| top.is_none_or(|t| major(&r.version).is_none_or(|m| m <= t))).collect()
}

/// The newest release the installed copy may move to: a pre-release only for a copy that is a
/// pre-release itself.
pub fn newest<'a>(rels: &'a [Release], installed: &str, choice: Option<bool>) -> Option<&'a Release> {
    // The user's choice for the app, else what the copy here is (a pre-release keeps getting them).
    let pre_ok = choice.unwrap_or_else(|| pre_copy(rels, installed));
    in_line(rels)
        .into_iter()
        .filter(|r| pre_ok || !pre(rels, r))
        // A dev build that reads like its release ("0.0.19-alpha.dev.3" next to "0.0.19-alpha") is not newer.
        .max_by(|a, b| cmp_ver(&a.version, &b.version).then((!a.prerelease).cmp(&!b.prerelease)))
}

/// The processor a file is built for, when its name says.
fn arch_in(name: &str) -> Option<&'static str> {
    let low = name.to_ascii_lowercase();
    let words: Vec<&str> = low.split(|c: char| !c.is_ascii_alphanumeric()).collect();
    let has = |w: &[&str]| words.iter().any(|x| w.contains(x));
    if has(&["arm64", "aarch64"]) {
        Some("arm64")
    } else if low.contains("x86_64") || low.contains("x86-64") || has(&["x64", "amd64", "win64", "64bit"]) {
        Some("x64")
    } else if has(&["x86", "i386", "i686", "win32", "ia32", "32bit"]) {
        Some("x86")
    } else {
        None
    }
}

/// The release's installer for a copy that `kind` installed on a `machine` PC: the one file of
/// that installer type for this processor, with GitHub's checksum. None when there is none, or
/// several that nothing tells apart.
pub fn installer_for<'a>(rel: &'a Release, kind: Kind, machine: &str) -> Option<&'a Asset> {
    let ext = match kind {
        Kind::Msi => ".msi",
        Kind::Inno | Kind::Nsis | Kind::Burn => ".exe",
        // Squirrel apps update themselves; any other installer has no switch known to be silent.
        Kind::Squirrel | Kind::Unknown => return None,
    };
    const NOT: &[&str] = &["portable", "debug", "symbols", "uninstall", "delta"];
    let fits: Vec<&Asset> = rel
        .assets
        .iter()
        .filter(|a| {
            let n = a.name.to_ascii_lowercase();
            n.ends_with(ext) && !NOT.iter().any(|w| n.contains(w)) && arch_in(&n).is_none_or(|x| x == machine) && a.sha256.is_some()
        })
        .collect();
    let one = |v: Vec<&'a Asset>| -> Option<&'a Asset> {
        if v.len() == 1 {
            return v.first().copied();
        }
        // An installer for this processor over one that does not say.
        let exact: Vec<&Asset> = v.into_iter().filter(|a| arch_in(&a.name) == Some(machine)).collect();
        (exact.len() == 1).then(|| exact[0])
    };
    let named: Vec<&Asset> = fits.iter().copied().filter(|a| a.name.to_ascii_lowercase().contains("setup") || a.name.to_ascii_lowercase().contains("install")).collect();
    one(named).or_else(|| one(fits))
}

/// The installer type of a program, read from its entry and its uninstaller.
fn kind_of(e: &UninstallEntry) -> Kind {
    let head = switches::uninstaller_file(e).map(|p| switches::head_of(&p)).unwrap_or_default();
    switches::detect(e, &head)
}

/// The first number of a version: "1.0.2-alpha.3" -> 1.
fn major(v: &str) -> Option<u64> {
    let d: String = v.trim().trim_start_matches(['v', 'V']).chars().take_while(|c| c.is_ascii_digit()).collect();
    d.parse().ok()
}

/// Are these releases of the copy that is installed? One of them is its version, or shares its
/// first number (a year, for projects numbered by date). A sibling project the entry links to
/// (TidaLunar's entry names TidaLuna, numbered 1.17 next to its 0.0.18) is not it.
pub fn fits(installed: &str, rels: &[Release]) -> bool {
    let year = |m: u64| (1990..=2200).contains(&m);
    let mine = major(installed);
    rels.iter().any(|r| {
        cmp_ver(&r.version, installed).is_eq()
            || match (mine, major(&r.version)) {
                (Some(a), Some(b)) => a == b || (year(a) && year(b) && a.abs_diff(b) <= 3),
                _ => false,
            }
    })
}

/// Most projects one check reads: GitHub answers this many questions an hour for apps that do
/// not sign in, and an unchanged release list (most of them) does not count.
const MAX_REPOS: usize = 40;

pub(crate) async fn releases(repo: &str) -> Vec<Release> {
    let url = format!("https://api.github.com/repos/{repo}/releases?per_page=30");
    let Ok(Ok(f)) = tokio::time::timeout(Duration::from_secs(15), http::get_cached(&url, &[("Accept", "application/vnd.github+json")], Duration::from_secs(30 * 60))).await else { return vec![] };
    parse_releases(&serde_json::from_slice(&f.body).unwrap_or(Value::Null))
}

/// Reads the releases of the projects the listed programs name.
pub async fn fetch(installed: &[Installed]) -> Feed {
    let mut apps = HashMap::new();
    let mut versions = HashMap::new();
    for i in installed.iter().filter(|i| i.source == Source::Programs && !i.missing) {
        let SourceData::Other { entry, .. } = &i.data else { continue };
        let Some((o, r)) = repo_of(entry) else { continue };
        apps.insert(i.id.clone(), Tracked { repo: format!("{o}/{r}"), kind: kind_of(entry) });
        versions.insert(i.id.clone(), i.version.clone());
    }
    let mut keys: Vec<String> = apps.values().map(|t| t.repo.to_ascii_lowercase()).collect();
    keys.sort();
    keys.dedup();
    keys.truncate(MAX_REPOS);
    let repos: HashMap<String, Vec<Release>> = futures_util::stream::iter(keys)
        .map(|k| async move {
            let rels = releases(&k).await;
            (k, rels)
        })
        .buffer_unordered(4)
        .filter(|(_, r)| std::future::ready(!r.is_empty()))
        .collect()
        .await;
    apps.retain(|id, t| repos.get(&t.repo.to_ascii_lowercase()).is_some_and(|rels| versions.get(id).is_some_and(|v| fits(v, rels))));
    // Each tracked project's description (kept a day; an unchanged answer costs nothing).
    let mut wanted: Vec<String> = apps.values().map(|t| t.repo.to_ascii_lowercase()).collect();
    wanted.sort();
    wanted.dedup();
    let about: HashMap<String, String> = futures_util::stream::iter(wanted)
        .map(|k| async move { about(&k).await.map(|d| (k, d)) })
        .buffer_unordered(4)
        .filter_map(std::future::ready)
        .collect()
        .await;
    Feed { apps, repos, about }
}

/// The newest version of a program from its releases. `choice`: the user's choice of
/// pre-releases for it (None: as the copy here is).
pub fn latest(i: &Installed, feed: &Feed, choice: Option<bool>) -> Option<Latest> {
    let (t, rels) = feed.get(&i.id)?;
    let r = newest(rels, &i.version, choice)?;
    let size = installer_for(r, t.kind, other::machine_arch()).and_then(|a| a.size);
    Some(Latest { version: r.version.clone(), size, note: None })
}

/// The release page to download the newest version from, when it can not be installed here.
pub fn manual_page(i: &Installed, feed: &Feed, choice: Option<bool>) -> Option<String> {
    let (t, rels) = feed.get(&i.id)?;
    let r = newest(rels, &i.version, choice)?;
    installer_for(r, t.kind, other::machine_arch()).is_none().then(|| r.url.clone()).filter(|u| !u.is_empty())
}

/// Does the program's project publish pre-releases? Then the user can choose them for it: what
/// it follows now (`choice`, else as the copy here is). None when it has none.
pub fn follows_pre(i: &Installed, feed: &Feed, choice: Option<bool>) -> Option<bool> {
    let (_, rels) = feed.get(&i.id)?;
    in_line(rels).into_iter().any(|r| pre(rels, r)).then(|| choice.unwrap_or_else(|| pre_copy(rels, &i.version)))
}

/// Can an older release be put back? Windows Installer refuses to replace a newer version.
pub fn can_roll_back(i: &Installed, feed: &Feed) -> bool {
    feed.get(&i.id).is_some_and(|(t, _)| matches!(t.kind, Kind::Inno | Kind::Nsis))
}

/// The version history of a program: its releases, newest first, with their change lists.
/// Pre-releases older than the newest final release are left out.
pub fn versions(i: &Installed, feed: &Feed, choice: Option<bool>) -> Vec<VersionInfo> {
    let Some((t, rels)) = feed.get(&i.id) else { return vec![] };
    let top = newest(rels, &i.version, choice).map(|r| r.version.clone());
    let stable = in_line(rels).into_iter().find(|r| !pre(rels, r)).map(|r| r.version.clone());
    let machine = other::machine_arch();
    in_line(rels)
        .into_iter()
        .filter(|r| !pre(rels, r) || stable.as_ref().is_none_or(|s| cmp_ver(&r.version, s).is_gt()) || is_version(r, &i.version))
        .take(15)
        .map(|r| VersionInfo {
            version: r.version.clone(),
            date: r.date.clone(),
            notes: crate::notes::text_lines(&r.body),
            notes_url: Some(r.url.clone()).filter(|u| !u.is_empty()),
            size: installer_for(r, t.kind, machine).and_then(|a| a.size),
            installed: is_version(r, &i.version),
            latest: top.as_ref() == Some(&r.version),
            prerelease: pre(rels, r),
        })
        .collect()
}

/// `versions`, with the change list of a release whose text only links its changelog page read
/// from that page (see `other::release_text_notes`).
pub async fn versions_read(i: &Installed, feed: &Feed, choice: Option<bool>) -> Vec<VersionInfo> {
    let mut list = versions(i, feed, choice);
    let Some((_, rels)) = feed.get(&i.id) else { return list };
    let linked: Vec<(usize, String)> = list
        .iter()
        .enumerate()
        .filter_map(|(k, v)| rels.iter().find(|r| r.version == v.version).and_then(|r| crate::notes::linked_changelog(&r.body)).map(|u| (k, u)))
        .collect();
    let read = futures_util::future::join_all(linked.iter().map(|(k, u)| other::linked_notes(u, &list[*k].version))).await;
    for ((k, u), lines) in linked.into_iter().zip(read) {
        list[k].notes = if lines.is_empty() { vec![other::SEE_NOTES.to_string()] } else { lines };
        list[k].notes_url = Some(u);
    }
    list
}

/// The plan that installs a release of a program (`version`, else the newest) over the copy here.
pub fn plan_install(i: &Installed, version: Option<&str>, kind: JobKind, feed: Option<&Feed>) -> Result<Plan> {
    let name = &i.name;
    let (t, rels) = feed.and_then(|f| f.get(&i.id)).ok_or_else(|| anyhow!("{name}: its GitHub releases could not be read. Check your connection."))?;
    let rel = match version {
        Some(v) => rels.iter().find(|r| r.version == v),
        None => newest(rels, &i.version, None),
    }
    .ok_or_else(|| anyhow!("{name}: that release is no longer on GitHub"))?;
    let SourceData::Other { entry, .. } = &i.data else { return Err(anyhow!("{name} is not a program from Windows' list")) };
    // Read again: the copy may have been reinstalled another way since the check.
    let how = kind_of(entry);
    let a = installer_for(rel, how, other::machine_arch()).ok_or_else(|| anyhow!("{name} {}: its release has no installer this app can run silently. Its release page has the download.", rel.version))?;
    let sha = a.sha256.clone().ok_or_else(|| anyhow!("{name}: GitHub has no checksum for {}", a.name))?;
    let tail = safe_name(&t.repo.replace('/', "-"));
    let base = paths::downloads().join("github").join(&tail);
    let dest = base.join(safe_name(&a.name));
    let log = paths::logs().join(format!("github-{tail}-{}.log", safe_name(&rel.version)));
    let (exe, args, ok_codes): (PathBuf, Vec<String>, Vec<u32>) = match how {
        Kind::Msi => (
            PathBuf::from("msiexec.exe"),
            vec!["/i".into(), dest.to_string_lossy().to_string(), "/qn".into(), "/norestart".into(), "MSIFASTINSTALL=7".into(), "/l*".into(), log.to_string_lossy().to_string()],
            vec![0, 3010, 1641],
        ),
        Kind::Inno => (dest.clone(), vec!["/VERYSILENT".into(), "/SUPPRESSMSGBOXES".into(), "/NORESTART".into(), "/SP-".into(), format!("/LOG={}", log.display())], vec![0]),
        Kind::Nsis => (dest.clone(), vec!["/S".into()], vec![0]),
        Kind::Burn => (dest.clone(), vec!["/quiet".into(), "/norestart".into()], vec![0, 3010]),
        Kind::Squirrel | Kind::Unknown => return Err(anyhow!("{name}: its installer has no switch known to run it silently")),
    };
    let logged = matches!(how, Kind::Msi | Kind::Inno);
    let mut cleanup = vec![dest.clone()];
    if logged {
        cleanup.push(log.clone());
    }
    // A copy in the user's own list installs without admin rights.
    let admin = entry.hive != "HKCU";
    let label = format!("Installing {name} {}", rel.version);
    let run = |elevated: bool| Step::Run { exe: exe.clone(), args: args.clone(), cwd: None, ok_codes: ok_codes.clone(), elevated, label: label.clone(), log: logged.then(|| log.clone()) };
    let block_dirs: Vec<PathBuf> = i.install_dir.clone().filter(|d| other::own_dir(d)).into_iter().collect();
    let block_names: Vec<String> = if block_dirs.is_empty() { i.exe.as_ref().and_then(|e| e.file_name()).map(|n| n.to_string_lossy().to_ascii_lowercase()).filter(|n| n.ends_with(".exe")).into_iter().collect() } else { vec![] };
    let make = |elevated: bool| Plan {
        app_id: i.id.clone(),
        kind,
        name: name.clone(),
        to_version: rel.version.clone(),
        downloads: vec![Download { url: a.url.clone(), dest: dest.clone(), size: a.size, sha256: Some(sha.clone()), md5: None, signer: None, sha512: None, headers: vec![] }],
        steps: vec![run(elevated)],
        block_dirs: block_dirs.clone(),
        block_names: block_names.clone(),
        verify: Verify::Version { app_id: i.id.clone(), version: rel.version.clone() },
        fallback: None,
        cleanup: cleanup.clone(),
    };
    let mut plan = make(admin);
    // A per-user installer that Windows only starts with admin rights gets them on a second try.
    if !admin {
        plan.fallback = Some(Box::new(make(true)));
    }
    Ok(plan)
}
