//! Reading and fetching the icons of the makers of drivers and of installed programs and games
//! (what to look at is decided in `makers` and `proglogo`). Everything is kept in the icon cache
//! once; a search that found nothing is remembered through `Memory`, so a site or Steam's store is
//! asked once and then again after a week.

use super::{icons, makers, other, proglogo, winget};
use crate::{paths, winutil};
use std::path::PathBuf;
use std::time::Duration;

/// What the caller remembers between runs: which searches found nothing lately.
pub struct Memory<'a> {
    /// Did the search under this key find nothing in the last days?
    pub recent: &'a (dyn Fn(&str) -> bool + Sync),
    /// Remember that the search under this key found nothing.
    pub miss: &'a (dyn Fn(String) + Sync),
}

/// A short stable name for a file in the icon cache.
fn hash(s: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(&Sha256::digest(s.as_bytes())[..10])
}

/// A small icon kept under this stem (made small already).
fn kept(stem: &str) -> Option<PathBuf> {
    icons::on_disk(stem).filter(|p| icons::is_small(p))
}

/// Save a found picture under this stem, as `how` makes it small.
async fn save(got: icons::Got, stem: String, how: fn(icons::Got) -> icons::Got) -> Option<PathBuf> {
    tauri::async_runtime::spawn_blocking(move || {
        let got = how(got);
        let p = paths::icons().join(icons::saved_name_for(&stem, &got));
        let ok = std::fs::write(&p, &got.bytes).is_ok();
        icons::wrote();
        ok.then_some(p)
    })
    .await
    .ok()
    .flatten()
}

/// A maker's icon as a small file in the icon cache: the one from an earlier run, else read from
/// the maker's site. None: nothing to do now (the last search found none a short while ago, or
/// failed for a passing reason); Err: the maker's site has no icon for us.
pub async fn maker(site: &'static makers::Site, skip: bool) -> Option<Result<PathBuf, ()>> {
    let stem = makers::stem(site.key);
    if let Some(p) = kept(&stem) {
        return Some(Ok(p));
    }
    if skip {
        return None;
    }
    let pages: Vec<String> = site.pages.iter().map(|p| p.to_string()).collect();
    let found = tokio::time::timeout(Duration::from_secs(25), icons::find(&format!("maker:{}", site.key), site.icon, &pages)).await.ok()?;
    match found {
        Ok(got) => Some(save(got, stem, icons::normalize).await.ok_or(())),
        Err(m) if m.passing => None,
        Err(_) => Some(Err(())),
    }
}

/// An icon of a program or game from what this PC has: the icon its entry names (what Windows'
/// own list shows: a program, a library or an icon file), or the one Steam keeps for a Steam
/// game. Saved small in the icon cache. Returns where it came from too.
pub async fn program_here(e: winutil::UninstallEntry) -> Option<(PathBuf, &'static str)> {
    if let Some((file, index)) = proglogo::display_icon(&e.display_icon).filter(|(f, _)| f.is_file()) {
        let out = paths::icons().join(format!("shown-{}.png", hash(&format!("{}-{index}", file.to_string_lossy().to_lowercase()))));
        let made = out.is_file() || tauri::async_runtime::spawn_blocking({
            let out = out.clone();
            move || winutil::extract_icon_png_at(&file, index, &out)
        })
        .await
        .unwrap_or(false);
        if made {
            return Some((out, "its own icon file"));
        }
    }
    let app = proglogo::steam_app_id(&e.key)?;
    let stem = format!("steam-{app}");
    if let Some(p) = kept(&stem) {
        return Some((p, "Steam's library cache"));
    }
    let from = proglogo::steam_dir(&e).and_then(|d| proglogo::steam_cached_icon(&d, app))?;
    tauri::async_runtime::spawn_blocking(move || {
        let bytes = std::fs::read(&from).ok()?;
        let ext = icons::sniff(&bytes)?;
        let got = icons::normalize(icons::Got { bytes, ext, via: "steam", dark: false });
        let p = paths::icons().join(icons::saved_name_for(&stem, &got));
        let ok = std::fs::write(&p, &got.bytes).is_ok();
        icons::wrote();
        ok.then_some(p)
    })
    .await
    .ok()
    .flatten()
    .map(|p| (p, "Steam's library cache"))
}

/// Steam's store picture of an app, cut to a square: kept, or read once. Some(Err): the store has
/// none; None: it failed for a passing reason.
async fn steam_art(app: u32, stem: String) -> Option<Result<PathBuf, ()>> {
    if let Some(p) = kept(&stem) {
        return Some(Ok(p));
    }
    let mut passing = false;
    for url in proglogo::steam_art_urls(app) {
        match icons::picture(&url).await {
            Ok(got) => return Some(save(got, stem, icons::normalize_square).await.ok_or(())),
            Err(p) => passing |= p,
        }
    }
    if passing { None } else { Some(Err(())) }
}

/// The icon of the catalog app that has the program's name, when exactly one has it: the same
/// icon Get apps shows for it. Some(Err): it has none; None: nothing to do now.
async fn catalog_same_name(norm: &str, later: &str) -> Option<Result<PathBuf, ()>> {
    let (path, n) = (winget::index_path(), norm.to_string());
    let pkg = tauri::async_runtime::spawn_blocking(move || {
        let idx = winget::Index::open(&path).ok()?;
        match idx.by_name(&n).as_slice() {
            [row] => idx.by_row(*row),
            _ => None,
        }
    })
    .await
    .ok()
    .flatten()?;
    let stem = icons::file_stem(&pkg.id);
    if let Some(p) = kept(&stem) {
        return Some(Ok(p));
    }
    let src = other::icon_source_of(None, &pkg.id).await?;
    match icons::find(&pkg.id, src.icon.as_deref(), &src.pages).await {
        Ok(got) => Some(save(got, stem, icons::normalize).await.ok_or(())),
        Err(m) if m.passing => {
            icons::retry_later(later);
            None
        }
        Err(_) => Some(Err(())),
    }
}

/// The icon of a program or game from the web, when its own files gave none: for a Steam game its
/// store picture, for a game Steam's store picture found by its exact name, else the icon of the
/// publisher's own site, else the catalog app of the same name, else its maker's icon. Only a public app id, a game's name or a site's own
/// address goes into a request. `id` is the program's id, to try again later after a failure.
pub async fn program_web(id: &str, e: winutil::UninstallEntry, mem: &Memory<'_>) -> Option<(PathBuf, &'static str)> {
    let later = format!("prog:{id}");
    if !icons::may_search(&later) {
        return None;
    }
    // 1. a Steam game: its store picture by its app id
    if let Some(app) = proglogo::steam_app_id(&e.key) {
        let key = format!("steam:{app}");
        if !(mem.recent)(&key) {
            match steam_art(app, format!("steam-{app}-art")).await {
                Some(Ok(p)) => return Some((p, "Steam's store, by app id")),
                Some(Err(())) => (mem.miss)(key),
                None => icons::retry_later(&later),
            }
        }
    }
    // 2. a game: Steam's store picture by the game's exact name
    let plain: String = proglogo::plain(&e.display_name).chars().take(60).collect();
    if proglogo::looks_like_game(&e) && !plain.is_empty() {
        let stem = format!("steamname-{plain}");
        if let Some(p) = kept(&stem) {
            return Some((p, "Steam's store, by name"));
        }
        let key = format!("steamname:{plain}");
        if !(mem.recent)(&key) {
            match icons::text(&proglogo::steam_search_url(&e.display_name)).await {
                Some(json) => match proglogo::steam_pick(&json, &e.display_name).or_else(|| proglogo::steam_pick_longer(&json, &e.display_name)) {
                    Some(app) => match steam_art(app, stem).await {
                        Some(Ok(p)) => return Some((p, "Steam's store, by name")),
                        Some(Err(())) => (mem.miss)(key),
                        None => icons::retry_later(&later),
                    },
                    None => (mem.miss)(key),
                },
                None => icons::retry_later(&later),
            }
        }
    }
    // 3. the publisher's own site, once for all its programs
    let pages = proglogo::site_pages(&e);
    if let Some(site) = proglogo::site_key(&pages) {
        let stem = format!("site-{}", hash(&site));
        if let Some(p) = kept(&stem) {
            return Some((p, "the publisher's site"));
        }
        let key = format!("site:{site}");
        if !(mem.recent)(&key) {
            match tokio::time::timeout(Duration::from_secs(25), icons::find(&key, None, &pages)).await {
                Ok(Ok(got)) => {
                    if let Some(p) = save(got, stem, icons::normalize).await {
                        return Some((p, "the publisher's site"));
                    }
                }
                Ok(Err(m)) if m.passing => icons::retry_later(&later),
                Ok(Err(_)) => (mem.miss)(key),
                Err(_) => icons::retry_later(&later),
            }
        }
    }
    // 4. the catalog app with the program's name (one app only)
    let norm = winget::normalize_name(&e.display_name).name;
    if !norm.is_empty() {
        let key = format!("catalogname:{norm}");
        if !(mem.recent)(&key) {
            match catalog_same_name(&norm, &later).await {
                Some(Ok(p)) => return Some((p, "the catalog app of the same name")),
                Some(Err(())) => (mem.miss)(key),
                None => {}
            }
        }
    }
    // 5. the publisher is a maker whose icon is known (Microsoft's parts of Visual Studio and the
    // Windows SDK, Intel's tools): its icon, read once for all its programs
    if let Some(m) = makers::site_of(&e.publisher) {
        let key = format!("maker:{}", m.key);
        match maker(m, (mem.recent)(&key)).await {
            Some(Ok(p)) => return Some((p, "its maker's icon")),
            Some(Err(())) => (mem.miss)(key),
            None => {}
        }
    }
    None
}
