//! Signing in to the game platforms (Steam, Epic Games, GOG) to see every game the user owns,
//! installed or not, with its updates, and to install and update Epic and GOG games here.
//!
//! The user signs in on the platform's own sign-in page: Epic Games and GOG in the user's own
//! browser, Steam with a QR code its mobile app approves (or in a Steam window of the app). The
//! app never sees the password. What the sign-in gives back (a session the platform refreshes) is
//! kept encrypted for this Windows user (see `vault`) and is never logged. Signing out forgets it.

pub mod epic;
pub mod games;
pub mod gog;
pub mod steam;
pub mod vault;
pub mod window;

use anyhow::{anyhow, bail, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// A platform the user can sign in to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Platform {
    Steam,
    Epic,
    Gog,
}

impl Platform {
    pub const ALL: [Platform; 3] = [Platform::Steam, Platform::Epic, Platform::Gog];

    pub fn key(self) -> &'static str {
        match self {
            Platform::Steam => "steam",
            Platform::Epic => "epic",
            Platform::Gog => "gog",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Platform::Steam => "Steam",
            Platform::Epic => "Epic Games",
            Platform::Gog => "GOG",
        }
    }

    pub fn from_key(k: &str) -> Option<Platform> {
        Platform::ALL.into_iter().find(|p| p.key().eq_ignore_ascii_case(k))
    }

    /// The launcher this platform's games come from.
    pub fn launcher(self) -> crate::sources::games::Launcher {
        use crate::sources::games::Launcher;
        match self {
            Platform::Steam => Launcher::Steam,
            Platform::Epic => Launcher::Epic,
            Platform::Gog => Launcher::Gog,
        }
    }

    pub fn of_launcher(l: crate::sources::games::Launcher) -> Option<Platform> {
        use crate::sources::games::Launcher;
        match l {
            Launcher::Steam => Some(Platform::Steam),
            Launcher::Epic => Some(Platform::Epic),
            Launcher::Gog => Some(Platform::Gog),
            _ => None,
        }
    }

    /// Can the app install and update this platform's games itself (not only its launcher)?
    pub fn installs_here(self) -> bool {
        matches!(self, Platform::Epic | Platform::Gog)
    }
}

/// A game the user owns on a platform.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Owned {
    pub platform: Platform,
    /// The platform's id: Steam's app id, Epic's app name, GOG's product id
    pub id: String,
    pub name: String,
    /// A picture of it on the platform's own site
    pub image: Option<String>,
    /// Epic: the namespace and catalog item the downloads are asked by
    #[serde(default)]
    pub namespace: String,
    #[serde(default)]
    pub catalog_item: String,
    /// The newest build or version the platform offers, when it says
    #[serde(default)]
    pub latest: Option<String>,
    /// Installed only through the platform's own launcher (Steam, and Epic games another
    /// company's launcher manages)
    #[serde(default)]
    pub hand_off: bool,
}

/// A platform as the window shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    pub platform: Platform,
    pub title: String,
    pub signed_in: bool,
    /// The account's name on the platform, when signed in
    pub name: Option<String>,
    pub games: usize,
    /// When the library was last read (ISO 8601)
    pub synced_at: Option<String>,
    /// Installs and updates its games here (not only in its launcher)
    pub installs_here: bool,
    /// Why the library could not be read the last time ("Sign in to Steam again")
    pub problem: Option<String>,
}

/// A platform's library as last read.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Library {
    pub games: Vec<Owned>,
    pub synced_at: Option<String>,
}

fn library_file(p: Platform) -> PathBuf {
    let d = crate::paths::data_dir().join("accounts");
    let _ = std::fs::create_dir_all(&d);
    d.join(format!("{}-library.json", p.key()))
}

pub fn library(p: Platform) -> Library {
    std::fs::read(library_file(p)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_library(p: Platform, games: Vec<Owned>) {
    let lib = Library { games, synced_at: Some(chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()) };
    if let Ok(b) = serde_json::to_vec(&lib) {
        let _ = std::fs::write(library_file(p), b);
    }
}

/// Is the user signed in to this platform here?
pub fn signed_in(p: Platform) -> bool {
    vault::has(p.key())
}

/// Does a launcher's game show? Steam, Epic Games and GOG games once the user signed in to that
/// platform here; the other launchers have no sign-in and their games always show.
pub fn shows(l: crate::sources::games::Launcher) -> bool {
    Platform::of_launcher(l).is_none_or(signed_in)
}

/// The account name kept with the sign-in.
fn account_name(p: Platform) -> Option<String> {
    match p {
        Platform::Steam => vault::load::<steam::Session>(p.key()).map(|s| s.name),
        Platform::Epic => vault::load::<epic::Session>(p.key()).map(|s| s.display_name),
        Platform::Gog => vault::load::<gog::Session>(p.key()).map(|s| s.name),
    }
    .filter(|n| !n.trim().is_empty())
}

/// Why each platform's library could not be read the last time (this run of the app).
static PROBLEMS: once_cell::sync::Lazy<parking_lot::Mutex<std::collections::HashMap<Platform, String>>> = once_cell::sync::Lazy::new(Default::default);

/// Every platform, signed in or not.
pub fn list() -> Vec<AccountInfo> {
    let problems = PROBLEMS.lock().clone();
    Platform::ALL
        .into_iter()
        .map(|p| {
            let on = signed_in(p);
            let lib = if on { library(p) } else { Library::default() };
            let problem = problems.get(&p).filter(|_| on).cloned();
            AccountInfo { platform: p, title: p.name().into(), signed_in: on, name: if on { account_name(p) } else { None }, games: lib.games.len(), synced_at: lib.synced_at, installs_here: p.installs_here(), problem }
        })
        .collect()
}

/// The words of an error for the window ("SIGNIN|Sign in to GOG again" -> "Sign in to GOG again").
pub fn problem_text(e: &anyhow::Error) -> String {
    let t = format!("{e:#}");
    t.split_once('|').map(|(_, r)| r.to_string()).unwrap_or(t)
}

/// Read the libraries that are due again: Epic Games and GOG at every check, Steam twice a day.
/// A failure keeps the last library and is shown in Settings.
pub async fn refresh_due(app: &tauri::AppHandle) {
    for p in Platform::ALL {
        if !signed_in(p) {
            continue;
        }
        if p == Platform::Steam {
            let fresh = library(p).synced_at.and_then(|t| chrono::DateTime::parse_from_rfc3339(&t).ok()).is_some_and(|t| chrono::Utc::now().signed_duration_since(t) < chrono::Duration::hours(12));
            if fresh {
                continue;
            }
        }
        if let Err(e) = sync(app, p).await {
            log::warn!("{} library: {}", p.name(), problem_text(&e));
        }
    }
}

/// Forget the sign-in and the library of a platform (and the Steam window's cookies).
pub fn sign_out(p: Platform) {
    cancel(p);
    PROBLEMS.lock().remove(&p);
    vault::remove(p.key());
    let _ = std::fs::remove_file(library_file(p));
    if p == Platform::Steam {
        let _ = std::fs::remove_dir_all(window::web_dir(p));
    }
}

/// Read a platform's library again (signed in only), and keep it.
pub async fn sync(app: &tauri::AppHandle, p: Platform) -> Result<Library> {
    if !signed_in(p) {
        return Err(anyhow!("Sign in to {} first", p.name()));
    }
    let games = match p {
        Platform::Epic => epic::library().await,
        Platform::Gog => gog::library().await,
        Platform::Steam => steam::library(app).await,
    };
    match games {
        Ok(g) => {
            PROBLEMS.lock().remove(&p);
            save_library(p, g);
            Ok(library(p))
        }
        Err(e) => {
            PROBLEMS.lock().insert(p, problem_text(&e));
            Err(e)
        }
    }
}

fn info(p: Platform) -> Result<AccountInfo> {
    list().into_iter().find(|a| a.platform == p).ok_or_else(|| anyhow!("Unknown platform"))
}

/// Kept signed in: read the library right away. A failure leaves the sign-in (Settings shows why,
/// and the next check reads it again).
async fn signed_in_now(app: &tauri::AppHandle, p: Platform) -> Result<AccountInfo> {
    PROBLEMS.lock().remove(&p);
    if let Err(e) = sync(app, p).await {
        log::warn!("{} library: {}", p.name(), problem_text(&e));
    }
    info(p)
}

/// The first step of a sign-in.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum SignInStep {
    /// Signed in already (the Steam window)
    Done { account: AccountInfo },
    /// The platform's page is open in the browser; it shows a code to paste (`finish`)
    Paste,
    /// A QR code to scan with the Steam mobile app (an image address); `wait` finishes it
    Qr { image: String },
}

/// Start signing in to a platform: Steam with a QR code for its mobile app (or, `window`, in a
/// Steam window of the app); Epic Games and GOG on their page in the user's own browser.
pub async fn sign_in(app: &tauri::AppHandle, p: Platform, window: bool) -> Result<SignInStep> {
    match p {
        Platform::Steam if window => {
            let (s, games) = steam::window_sign_in(app).await?;
            vault::save(p.key(), &s)?;
            save_library(p, games);
            PROBLEMS.lock().remove(&p);
            Ok(SignInStep::Done { account: info(p)? })
        }
        Platform::Steam => Ok(SignInStep::Qr { image: steam::qr_start().await? }),
        Platform::Epic => {
            crate::winutil::shell_open(&epic::login_url())?;
            Ok(SignInStep::Paste)
        }
        Platform::Gog => {
            crate::winutil::shell_open(&gog::login_url())?;
            Ok(SignInStep::Paste)
        }
    }
}

/// Wait for a QR sign-in to be approved on the phone (Steam). `show` gets each new code.
pub async fn wait(app: &tauri::AppHandle, p: Platform, show: impl Fn(String) + Send) -> Result<AccountInfo> {
    if p != Platform::Steam {
        bail!("{} has no QR sign-in", p.name());
    }
    let s = steam::qr_wait(show).await?;
    vault::save(p.key(), &s)?;
    signed_in_now(app, p).await
}

/// Finish an Epic Games or GOG sign-in with what the user pasted from the browser: the page's
/// text, its address or the code alone.
pub async fn finish(app: &tauri::AppHandle, p: Platform, pasted: &str) -> Result<AccountInfo> {
    match p {
        Platform::Epic => {
            let code = epic::code_from_text(pasted).ok_or_else(|| anyhow!("That is not the code Epic Games shows. After you sign in, copy all the text on its page and paste it here."))?;
            vault::save(p.key(), &epic::exchange(&code).await?)?;
        }
        Platform::Gog => {
            let code = gog::code_from_text(pasted).ok_or_else(|| anyhow!("That is not GOG's sign-in address. After you sign in, copy the address of the empty page GOG shows and paste it here."))?;
            vault::save(p.key(), &gog::exchange(&code).await?)?;
        }
        Platform::Steam => bail!("Steam signs in with a QR code or a Steam window"),
    }
    signed_in_now(app, p).await
}

/// Stop a sign-in that waits (Steam's QR code or window).
pub fn cancel(p: Platform) {
    if p == Platform::Steam {
        steam::cancel();
    }
}

/// How old a session's access may get before it is refreshed.
pub(crate) const REFRESH_EARLY: Duration = Duration::from_secs(5 * 60);

/// Now, in Unix seconds.
pub(crate) fn now() -> i64 {
    chrono::Utc::now().timestamp()
}
