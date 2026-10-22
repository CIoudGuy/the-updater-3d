//! The ArtCraft "Crafting Apps" (storytold on GitHub) and the ArtCraft studio app.
//!
//! Latest versions come from the feed the ArtCraft Launcher uses (one request, includes
//! SHA-256 for every file). Apps missing from that feed, and version history, come from
//! GitHub Releases with ETag caching. Installs use each release's MSI (in place upgrade);
//! ArtCraft itself ships a per-user setup.exe.

use super::{Download, Installed, Latest, Plan, SourceData, Step, Verify};
use crate::model::{CatalogEntry, JobKind, Source, VersionInfo};
use crate::{http, paths, version, winutil};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::path::PathBuf;
use std::time::Duration;

const FEED_URL: &str = "https://api.storyteller.ai/v1/craft_apps/release_info";

pub struct AppDef {
    pub key: &'static str,
    pub repo: &'static str,
    pub name: &'static str,
    pub tagline: &'static str,
    pub code: &'static str,
    pub fg: &'static str,
    pub bg: &'static str,
    /// Release tag prefix in a shared repo ("artcraft-v")
    pub tag_prefix: &'static str,
}

pub const APPS: &[AppDef] = &[
    AppDef { key: "artcraft-launcher", repo: "storytold/craft-launcher", name: "ArtCraft Launcher", tagline: "Installs and updates the Craft apps", code: "La", fg: "#EDEDED", bg: "#2E2E2E", tag_prefix: "v" },
    AppDef { key: "photocraft", repo: "storytold/photocraft", name: "PhotoCraft", tagline: "Image editor", code: "Ph", fg: "#7C93D6", bg: "#141C33", tag_prefix: "v" },
    AppDef { key: "vectorcraft", repo: "storytold/vectorcraft", name: "VectorCraft", tagline: "Vector illustration", code: "Vc", fg: "#D9B26A", bg: "#2B2110", tag_prefix: "v" },
    AppDef { key: "filmcraft", repo: "storytold/filmcraft", name: "FilmCraft", tagline: "Video editor", code: "Fm", fg: "#A98BD8", bg: "#1E1530", tag_prefix: "v" },
    AppDef { key: "lightcraft", repo: "storytold/lightcraft", name: "LightCraft", tagline: "Non-destructive photo editor", code: "Lc", fg: "#8FC1E8", bg: "#0F2230", tag_prefix: "v" },
    AppDef { key: "pdfcraft", repo: "storytold/pdfcraft", name: "PdfCraft", tagline: "PDF workbench", code: "Pd", fg: "#D9706A", bg: "#2E1311", tag_prefix: "v" },
    AppDef { key: "effectcraft", repo: "storytold/effectcraft", name: "EffectCraft", tagline: "Motion graphics and VFX", code: "Ef", fg: "#C08BFF", bg: "#22093D", tag_prefix: "v" },
    AppDef { key: "designcraft", repo: "storytold/designcraft", name: "DesignCraft", tagline: "Page layout and publishing", code: "Dc", fg: "#D98BA6", bg: "#2E1320", tag_prefix: "v" },
    AppDef { key: "figcraft", repo: "storytold/figcraft", name: "FigCraft", tagline: "Interface design", code: "Fg", fg: "#7FB77E", bg: "#142614", tag_prefix: "v" },
    AppDef { key: "wordcraft", repo: "storytold/wordcraft", name: "WordCraft", tagline: "Word processing", code: "Wd", fg: "#7FA6E0", bg: "#0F1C30", tag_prefix: "v" },
    AppDef { key: "soundcraft", repo: "storytold/soundcraft", name: "SoundCraft", tagline: "Digital audio workstation", code: "Sd", fg: "#5EC8B8", bg: "#0B2622", tag_prefix: "v" },
    AppDef { key: "deckcraft", repo: "storytold/deckcraft", name: "DeckCraft", tagline: "Presentations and slide shows", code: "Dk", fg: "#E8A15C", bg: "#2E1B0A", tag_prefix: "v" },
    AppDef { key: "cadcraft", repo: "storytold/cadcraft", name: "CADCraft", tagline: "Computer-aided design and drafting", code: "Cd", fg: "#B5C46A", bg: "#22260E", tag_prefix: "v" },
    AppDef { key: "gridcraft", repo: "storytold/gridcraft", name: "GridCraft", tagline: "Spreadsheets and data analysis", code: "Gr", fg: "#6AC48A", bg: "#0E2617", tag_prefix: "v" },
    AppDef { key: "artcraft", repo: "storytold/artcraft", name: "ArtCraft", tagline: "AI image and video studio", code: "Ar", fg: "#EDEDED", bg: "#3A2A1A", tag_prefix: "artcraft-v" },
];

pub fn def(key: &str) -> Option<&'static AppDef> {
    APPS.iter().find(|a| a.key == key)
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub date: Option<String>,
    pub prerelease: bool,
    pub notes: String,
    pub url: Option<String>,
    pub assets: Vec<Asset>,
}

impl Release {
    /// The Windows x64 installer: MSI for the Craft apps, setup.exe for ArtCraft.
    pub fn installer(&self) -> Option<&Asset> {
        let msi = self.assets.iter().find(|a| {
            let n = a.name.to_ascii_lowercase();
            n.ends_with("-windows-x64.msi")
        });
        msi.or_else(|| self.assets.iter().find(|a| a.name.to_ascii_lowercase().ends_with("_x64-setup.exe")))
            .or_else(|| self.assets.iter().find(|a| a.name.to_ascii_lowercase().ends_with("_x64_en-us.msi")))
    }
    pub fn sums(&self) -> Option<&Asset> {
        self.assets.iter().find(|a| a.name.eq_ignore_ascii_case("SHA256SUMS.txt"))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Feed {
    /// key -> newest release (respecting the pre-release setting)
    pub latest: std::collections::HashMap<String, Release>,
}

// ---------------------------------------------------------------- feeds

pub async fn fetch(prereleases: bool) -> Result<Feed> {
    let mut feed = Feed::default();
    match http::get_cached(FEED_URL, &[], Duration::from_secs(10 * 60)).await {
        Ok(f) => {
            if let Ok(v) = serde_json::from_slice::<Value>(&f.body) {
                if let Some(apps) = v["release_info"]["apps"].as_object() {
                    for (key, app) in apps {
                        let lr = &app["latest_release"];
                        if lr.is_null() {
                            continue;
                        }
                        let rel = parse_feed_release(lr);
                        if rel.prerelease && !prereleases {
                            continue;
                        }
                        feed.latest.insert(key.clone(), rel);
                    }
                }
            }
        }
        Err(e) => log::warn!("craft feed: {e}"),
    }
    // Apps the feed does not list (FigCraft, ArtCraft today) come from GitHub.
    for a in APPS {
        if feed.latest.contains_key(a.key) {
            continue;
        }
        match releases(a, prereleases).await {
            Ok(list) => {
                if let Some(r) = list.into_iter().next() {
                    feed.latest.insert(a.key.to_string(), r);
                }
            }
            Err(e) => log::warn!("{}: {e}", a.repo),
        }
    }
    if feed.latest.is_empty() {
        bail!("Could not reach the ArtCraft release feed or GitHub");
    }
    Ok(feed)
}

fn parse_feed_release(lr: &Value) -> Release {
    let assets = lr["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|x| Asset {
                    name: x["name"].as_str().unwrap_or("").to_string(),
                    url: x["download_url"].as_str().unwrap_or("").to_string(),
                    size: x["size_bytes"].as_u64(),
                    sha256: x["sha256"].as_str().map(|s| s.to_ascii_lowercase()),
                })
                .collect()
        })
        .unwrap_or_default();
    Release {
        version: lr["version"].as_str().unwrap_or("").to_string(),
        date: lr["published_at"].as_str().map(String::from),
        prerelease: lr["prerelease"].as_bool().unwrap_or(false),
        notes: lr["notes_markdown"].as_str().unwrap_or("").to_string(),
        url: lr["html_url"].as_str().map(String::from),
        assets,
    }
}

/// Releases from GitHub, newest first, only ones with a Windows installer.
pub async fn releases(a: &AppDef, prereleases: bool) -> Result<Vec<Release>> {
    let url = format!("https://api.github.com/repos/{}/releases?per_page=40", a.repo);
    let f = http::get_cached(&url, &[("Accept", "application/vnd.github+json")], Duration::from_secs(10 * 60)).await?;
    let v: Value = serde_json::from_slice(&f.body).context("GitHub answer")?;
    let arr = v.as_array().ok_or_else(|| anyhow!("GitHub: {}", v["message"].as_str().unwrap_or("unexpected answer")))?;
    let mut out = Vec::new();
    for r in arr {
        if r["draft"].as_bool() == Some(true) {
            continue;
        }
        let tag = r["tag_name"].as_str().unwrap_or("");
        if !tag.starts_with(a.tag_prefix) {
            continue;
        }
        let pre = r["prerelease"].as_bool().unwrap_or(false);
        if pre && !prereleases {
            continue;
        }
        let assets: Vec<Asset> = r["assets"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(|x| Asset {
                        name: x["name"].as_str().unwrap_or("").to_string(),
                        url: x["browser_download_url"].as_str().unwrap_or("").to_string(),
                        size: x["size"].as_u64(),
                        sha256: x["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|s| s.to_ascii_lowercase()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        let rel = Release {
            version: tag.trim_start_matches(a.tag_prefix).to_string(),
            date: r["published_at"].as_str().map(String::from),
            prerelease: pre,
            notes: r["body"].as_str().unwrap_or("").to_string(),
            url: r["html_url"].as_str().map(String::from),
            assets,
        };
        if rel.installer().is_some() {
            out.push(rel);
        }
    }
    out.sort_by(|x, y| version::cmp(&y.version, &x.version));
    Ok(out)
}

/// Release notes markdown to plain lines: its section headings as "## " / "### " lines (see
/// `notes`), every bullet as a line.
pub fn note_lines(md: &str) -> Vec<String> {
    let link = regex::Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap();
    let mut out = Vec::new();
    for line in md.lines() {
        let l = line.trim();
        if let Some(h) = markdown_heading(l) {
            out.push(h);
            continue;
        }
        let Some(item) = l.strip_prefix("* ").or_else(|| l.strip_prefix("- ")) else { continue };
        let mut t = link.replace_all(item, "$1").to_string();
        if let Some(i) = t.find(" by @") {
            t.truncate(i);
        }
        let t = t.replace("**", "").replace('`', "").replace(" \u{2014} ", ", ").replace(" \u{2013} ", ", ").replace('\u{2014}', ", ").replace('\u{2013}', "-");
        let t = t.trim().trim_end_matches('.').to_string();
        if !t.is_empty() && !t.starts_with("@") {
            out.push(t);
        }
    }
    crate::notes::tidy_headings(out)
}

/// The change list of a release: its bullets, or for a release written as prose its sentences.
/// GitHub's stock text ("See the assets below to download this version and install") and a lone
/// "Full Changelog" link are no notes, so those show that none is published.
pub fn release_lines(md: &str) -> Vec<String> {
    let mut lines = note_lines(md);
    if !lines.iter().any(|l| !l.starts_with('#')) {
        lines = crate::notes::text_lines(md)
            .into_iter()
            .filter(|l| {
                let low = l.to_ascii_lowercase();
                !(low.contains("see the assets") || low.contains("download this version") || low.starts_with("full changelog"))
            })
            .collect();
        lines = crate::notes::tidy_headings(lines);
    }
    if lines.iter().any(|l| !l.starts_with('#')) {
        lines
    } else {
        vec![super::other::NO_NOTES.to_string()]
    }
}

/// "## What's Changed" as a heading line; boilerplate sections (contributors, the full
/// changelog link, downloads) are not headings worth showing.
pub fn markdown_heading(l: &str) -> Option<String> {
    let level = l.chars().take_while(|c| *c == '#').count();
    if level == 0 || l.chars().nth(level) != Some(' ') {
        return None;
    }
    let link = regex::Regex::new(r"\[([^\]]+)\]\([^)]+\)").unwrap();
    let text = link.replace_all(l[level..].trim(), "$1").replace("**", "").replace('`', "").trim().trim_end_matches(':').to_string();
    let low = text.to_ascii_lowercase();
    if text.is_empty() || ["contributor", "full changelog", "download", "checksum", "sha256", "assets", "installation"].iter().any(|w| low.contains(w)) {
        return None;
    }
    Some(format!("{} {text}", if level <= 2 { "##" } else { "###" }))
}

// ---------------------------------------------------------------- installed apps

pub fn scan(uninstall: &[winutil::UninstallEntry]) -> Vec<Installed> {
    let mut out = Vec::new();
    for a in APPS {
        let entry = uninstall.iter().find(|e| {
            e.display_name.eq_ignore_ascii_case(a.name)
                && (e.publisher.contains("Learning Machines") || e.publisher.to_ascii_lowercase().contains("storyteller") || e.url_update_info.contains("storytold") || e.url_update_info.contains("artcraft") || e.publisher.is_empty() || a.key == "artcraft")
        });
        let Some(e) = entry else { continue };
        let exe_name = if a.key == "artcraft" { "ArtCraft.exe".to_string() } else { format!("{}.exe", a.key) };
        let mut candidates: Vec<PathBuf> = Vec::new();
        if !e.install_location.is_empty() {
            candidates.push(winutil::pathbuf(&e.install_location));
        }
        let icon = e.display_icon.split(',').next().unwrap_or("").trim_matches('"').to_string();
        if icon.to_ascii_lowercase().ends_with(".exe") {
            if let Some(p) = PathBuf::from(&icon).parent() {
                candidates.push(p.to_path_buf());
            }
        }
        candidates.push(paths::program_files().join(a.name));
        candidates.push(paths::local_app_data().join("Programs").join(a.name));
        candidates.push(paths::local_app_data().join(a.name));
        let dir = candidates.into_iter().find(|d| d.join(&exe_name).exists() || (d.exists() && a.key == "artcraft"));
        let exe = dir.as_ref().map(|d| d.join(&exe_name)).filter(|p| p.exists());
        let msi_code = if e.key.starts_with('{') { Some(e.key.clone()) } else { None };
        let mut uninstall_cmd = if let Some(code) = &msi_code {
            Some(vec!["msiexec.exe".to_string(), "/x".into(), code.clone(), "/qn".into(), "/norestart".into()])
        } else if !e.quiet_uninstall_string.is_empty() {
            Some(winutil::split_program_line(&e.quiet_uninstall_string))
        } else if !e.uninstall_string.is_empty() {
            let mut v = winutil::split_program_line(&e.uninstall_string);
            v.push("/S".into());
            Some(v)
        } else {
            None
        };
        if uninstall_cmd.as_ref().is_some_and(|v| v.is_empty()) {
            uninstall_cmd = None;
        }
        let updated_at = winutil::install_date_iso(&e.install_date)
            .or_else(|| exe.as_ref().and_then(|p| p.metadata().ok()).and_then(|m| m.modified().ok()).map(|t| chrono::DateTime::<chrono::Utc>::from(t).to_rfc3339()));
        out.push(Installed {
            id: format!("craft:{}", a.key),
            source: Source::Craft,
            name: a.name.to_string(),
            code: a.code.into(),
            fg: a.fg.into(),
            bg: a.bg.into(),
            version: e.display_version.clone(),
            missing: exe.is_none() && dir.is_none(),
            install_dir: dir,
            exe,
            uninstall: uninstall_cmd,
            updated_at,
            size_hint: Some(e.estimated_size_kb * 1024).filter(|s| *s > 0),
            tagline: Some(a.tagline.into()),
            channel: None,
            read_only: false,
            icon_remote: None,
            data: SourceData::Craft { key: a.key.to_string(), msi_product_code: msi_code },
        });
    }
    out
}

pub fn latest(inst: &Installed, feed: &Feed) -> Option<Latest> {
    let SourceData::Craft { key, .. } = &inst.data else { return None };
    let r = feed.latest.get(key)?;
    Some(Latest { version: r.version.clone(), size: r.installer().and_then(|a| a.size), note: None })
}

pub fn catalog_entries(feed: &Feed, installed: &[Installed]) -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    for a in APPS {
        // Installed ones stay listed (marked installed, with Open), so the list is the whole family.
        let is_installed = installed.iter().any(|i| i.id == format!("craft:{}", a.key));
        let Some(r) = feed.latest.get(a.key) else { continue };
        out.push(CatalogEntry {
            id: format!("craft:{}", a.key),
            source: Source::Craft,
            name: a.name.into(),
            code: a.code.into(),
            fg: a.fg.into(),
            bg: a.bg.into(),
            icon_url: None,
            version: r.version.clone(),
            size: r.installer().and_then(|x| x.size),
            tagline: Some(a.tagline.into()),
            description: super::about::craft(a.key).map(String::from),
            blocked: None,
            group: Source::Craft.group().into(),
            prerelease: r.prerelease,
            installed: is_installed,
            pick_version: true,
        });
    }
    out
}

pub async fn versions(key: &str, installed: Option<&str>, prereleases: bool) -> Result<Vec<VersionInfo>> {
    let a = def(key).ok_or_else(|| anyhow!("unknown app"))?;
    let list = releases(a, prereleases).await?;
    let newest = list.first().map(|r| r.version.clone());
    Ok(list
        .into_iter()
        .take(25)
        .map(|r| VersionInfo {
            installed: installed.is_some_and(|v| version::cmp(v, &r.version).is_eq()),
            latest: newest.as_deref() == Some(r.version.as_str()),
            notes: release_lines(&r.notes),
            notes_url: r.url.clone(),
            size: r.installer().and_then(|x| x.size),
            date: r.date.clone(),
            prerelease: r.prerelease,
            version: r.version,
        })
        .collect())
}

async fn find_release(a: &AppDef, ver: Option<&str>, feed: Option<&Feed>, prereleases: bool) -> Result<Release> {
    if ver.is_none() {
        if let Some(r) = feed.and_then(|f| f.latest.get(a.key)) {
            return Ok(r.clone());
        }
    }
    let list = releases(a, true).await?;
    match ver {
        Some(v) => list.into_iter().find(|r| version::cmp(&r.version, v).is_eq()).ok_or_else(|| anyhow!("{} {} is not on GitHub", a.name, v)),
        None => list.into_iter().find(|r| prereleases || !r.prerelease).ok_or_else(|| anyhow!("{} has no Windows release yet", a.name)),
    }
}

/// SHA-256 for an asset: from the feed/GitHub digest, or the release's SHA256SUMS.txt.
async fn sha_for(r: &Release, asset: &Asset) -> Option<String> {
    if let Some(s) = &asset.sha256 {
        return Some(s.clone());
    }
    let sums = r.sums()?;
    let f = http::get_cached(&sums.url, &[], Duration::from_secs(24 * 3600)).await.ok()?;
    let text = String::from_utf8_lossy(&f.body);
    text.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        let h = it.next()?;
        let n = it.next()?.trim_start_matches('*');
        (n == asset.name).then(|| h.to_ascii_lowercase())
    })
}

pub async fn plan_install(key: &str, ver: Option<&str>, kind: JobKind, current: Option<&Installed>, feed: Option<&Feed>, prereleases: bool) -> Result<Plan> {
    let a = def(key).ok_or_else(|| anyhow!("unknown Craft app {key}"))?;
    let r = find_release(a, ver, feed, prereleases).await?;
    let asset = r.installer().cloned().ok_or_else(|| anyhow!("{} {} has no Windows installer", a.name, r.version))?;
    let sha = sha_for(&r, &asset).await;
    let dest = paths::downloads().join("craft").join(&asset.name);
    let dl = Download { url: asset.url.clone(), dest: dest.clone(), size: asset.size, sha256: sha, md5: None, signer: None, sha512: None, headers: vec![] };
    super::require_checked(&dl, &format!("{} {}", a.name, r.version))?;
    let log = paths::logs().join(format!("{}-{}.log", a.key, r.version));
    let mut steps = Vec::new();
    // MSI will not downgrade over a newer copy, so a roll back removes the current one first.
    let downgrade = current.is_some_and(|c| version::newer(&c.version, &r.version));
    if downgrade || kind == JobKind::Repair {
        if let Some(c) = current {
            if let Some(u) = &c.uninstall {
                steps.push(Step::Run {
                    exe: PathBuf::from(&u[0]),
                    args: u[1..].to_vec(),
                    cwd: None,
                    ok_codes: vec![0, 1605, 3010],
                    elevated: u[0].to_ascii_lowercase().contains("msiexec"),
                    label: format!("Removing {} {}", a.name, c.version),
                    log: None,
                });
            }
        }
    }
    let is_msi = asset.name.to_ascii_lowercase().ends_with(".msi");
    if is_msi {
        steps.push(Step::Run {
            exe: PathBuf::from("msiexec.exe"),
            args: vec!["/i".into(), dest.to_string_lossy().into(), "/qn".into(), "/norestart".into(), "/l*v".into(), log.to_string_lossy().into()],
            cwd: None,
            ok_codes: vec![0, 3010, 1641],
            elevated: true,
            label: format!("Installing {} {}", a.name, r.version),
            log: Some(log),
        });
    } else {
        steps.push(Step::Run {
            exe: dest.clone(),
            args: vec!["/S".into()],
            cwd: None,
            ok_codes: vec![0],
            elevated: false,
            label: format!("Installing {} {}", a.name, r.version),
            log: None,
        });
    }
    Ok(Plan {
        app_id: format!("craft:{}", a.key),
        kind,
        name: a.name.to_string(),
        to_version: r.version.clone(),
        downloads: vec![dl],
        steps,
        block_dirs: current.and_then(|c| c.install_dir.clone()).into_iter().collect(),
        block_names: vec![format!("{}.exe", a.key)],
        verify: Verify::Version { app_id: format!("craft:{}", a.key), version: r.version.clone() },
        fallback: None,
        cleanup: vec![dest],
    })
}

pub fn plan_uninstall(inst: &Installed) -> Result<Plan> {
    let u = inst.uninstall.clone().filter(|u| !u.is_empty()).ok_or_else(|| anyhow!("{} has no uninstaller registered", inst.name))?;
    let elevated = u[0].to_ascii_lowercase().contains("msiexec");
    Ok(Plan {
        app_id: inst.id.clone(),
        kind: JobKind::Uninstall,
        name: inst.name.clone(),
        to_version: inst.version.clone(),
        downloads: vec![],
        steps: vec![Step::Run {
            exe: PathBuf::from(&u[0]),
            args: u[1..].to_vec(),
            cwd: None,
            ok_codes: vec![0, 1605, 3010],
            elevated,
            label: format!("Uninstalling {}", inst.name),
            log: None,
        }],
        block_dirs: inst.install_dir.iter().cloned().collect(),
        block_names: vec![],
        verify: Verify::Removed { app_id: inst.id.clone() },
        fallback: None,
        cleanup: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_become_plain_bullets() {
        let md = "## What's Changed\n* AUDIT and RECOVER: fix database errors (part of #406) by @vangel76 in https://github.com/x/pull/411\n* Add [docking](https://x) to CADCraft by @sky in https://x\n\n**Full Changelog**: https://x";
        let v = note_lines(md);
        assert_eq!(v, vec!["AUDIT and RECOVER: fix database errors (part of #406)", "Add docking to CADCraft"]);
        // No em or en dashes anywhere in the UI.
        let v = note_lines("* Scenarios P20 (4\u{2013}5 GB PSB) \u{2014} not run yet");
        assert_eq!(v, vec!["Scenarios P20 (4-5 GB PSB), not run yet"]);
    }

    #[test]
    fn prose_releases_keep_their_text_and_empty_ones_say_so() {
        assert_eq!(release_lines("This is a beta release. It contains the new FAL API Key support. "), vec!["This is a beta release. It contains the new FAL API Key support"]);
        // GitHub's stock text and a lone compare link are no notes.
        for md in ["See the assets below to download this version and install.", "Production build of ArtCraft. See the assets below to download and install.", "**Full Changelog**: https://github.com/storytold/photocraft/compare/v0.1.0...v0.1.1", ""] {
            assert_eq!(release_lines(md), vec![super::super::other::NO_NOTES], "{md}");
        }
        // Bullets as before.
        assert_eq!(release_lines("## What's Changed\n* Add docking by @sky in https://x\n"), vec!["Add docking"]);
    }

    #[test]
    fn picks_x64_msi() {
        let r = Release {
            version: "0.12.0".into(),
            date: None,
            prerelease: false,
            notes: String::new(),
            url: None,
            assets: vec![
                Asset { name: "photocraft-0.12.0-windows-arm64.msi".into(), url: String::new(), size: None, sha256: None },
                Asset { name: "photocraft-0.12.0-windows-x64-portable.zip".into(), url: String::new(), size: None, sha256: None },
                Asset { name: "photocraft-0.12.0-windows-x64.msi".into(), url: String::new(), size: None, sha256: None },
            ],
        };
        assert_eq!(r.installer().unwrap().name, "photocraft-0.12.0-windows-x64.msi");
    }
}
