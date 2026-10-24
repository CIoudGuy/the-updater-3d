//! Steam sign-in, two ways:
//! - A QR code the user scans with the Steam mobile app and approves there (Steam's own "sign in
//!   with a QR code"). Steam then gives a session that is renewed by itself; the games are read
//!   with Steam's Web API as the account itself, so a private profile works too.
//! - A Steam window in the app: the user signs in on steamcommunity.com there, and the games list
//!   on the account's own games page is read. The window's cookies stay in its own profile, so
//!   the list is read again later in a hidden window without signing in.
//!
//! Steam games install and update in Steam itself.

use super::{now, vault, window, Owned, Platform, REFRESH_EARLY};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::Duration;

const AUTH: &str = "https://api.steampowered.com/IAuthenticationService";
/// Longest a QR sign-in waits for the phone.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub steam_id: String,
    pub name: String,
    /// From the QR sign-in: renews `access_token`. None: signed in in the Steam window, whose
    /// cookies read the games page.
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub access_token: String,
    /// Unix seconds
    #[serde(default)]
    pub expires_at: i64,
}

// ---- QR sign-in ----

/// A QR sign-in waiting for the phone.
#[derive(Clone, Debug, PartialEq)]
pub struct Qr {
    pub client_id: String,
    pub request_id: String,
    /// What the QR code holds ("https://s.team/q/1/...")
    pub url: String,
    pub interval: u64,
}

/// A call to Steam's sign-in service; its answer and Steam's result code (1: OK).
async fn auth_call(method: &str, form: &[(&str, &str)]) -> Result<(Value, i64)> {
    let r = crate::http::CLIENT.post(format!("{AUTH}/{method}/v1/")).form(form).send().await.context("Could not reach Steam")?;
    let eresult = r.headers().get("x-eresult").and_then(|v| v.to_str().ok()).and_then(|v| v.parse().ok()).unwrap_or(if r.status().is_success() { 1 } else { 2 });
    let v: Value = r.json().await.unwrap_or(Value::Null);
    Ok((v.get("response").cloned().unwrap_or(Value::Null), eresult))
}

pub fn parse_qr(v: &Value) -> Option<Qr> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let url = s("challenge_url").filter(|u| u.starts_with("https://s.team/"))?;
    Some(Qr { client_id: s("client_id")?, request_id: s("request_id")?, url, interval: v.get("interval").and_then(Value::as_u64).unwrap_or(5).clamp(1, 30) })
}

async fn qr_begin() -> Result<Qr> {
    let (v, e) = auth_call("BeginAuthSessionViaQR", &[("device_friendly_name", "The Updater 3D"), ("platform_type", "2")]).await?;
    if e != 1 {
        bail!("Steam could not start a sign-in (code {e}). Try again.");
    }
    parse_qr(&v).ok_or_else(|| anyhow!("Steam gave no QR code. Try again."))
}

/// What a poll of a QR sign-in says.
#[derive(Debug, PartialEq)]
pub enum Poll {
    /// Not approved yet; Steam may have changed the code to show
    Waiting { new_url: Option<String>, new_client_id: Option<String> },
    Approved { refresh_token: String, access_token: String },
    /// The code ran out: a new one is needed
    Expired,
}

pub fn parse_poll(v: &Value, eresult: i64) -> Result<Poll> {
    match eresult {
        1 => {}
        // Expired, timeout, gone (Steam's result codes)
        9 | 16 | 27 => return Ok(Poll::Expired),
        e => bail!("Steam stopped the sign-in (code {e}). Try again."),
    }
    let s = |k: &str| v.get(k).and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string);
    if let (Some(refresh_token), Some(access_token)) = (s("refresh_token"), s("access_token")) {
        return Ok(Poll::Approved { refresh_token, access_token });
    }
    Ok(Poll::Waiting { new_url: s("new_challenge_url").filter(|u| u.starts_with("https://s.team/")), new_client_id: s("new_client_id") })
}

/// A QR code as a picture the window can show (an SVG data address).
pub fn qr_image(url: &str) -> Result<String> {
    use base64::Engine;
    let code = qrcode::QrCode::new(url.as_bytes()).map_err(|e| anyhow!("Could not draw the QR code: {e}"))?;
    let svg = code.render::<qrcode::render::svg::Color>().min_dimensions(232, 232).quiet_zone(true).dark_color(qrcode::render::svg::Color("#000000")).light_color(qrcode::render::svg::Color("#ffffff")).build();
    Ok(format!("data:image/svg+xml;base64,{}", base64::engine::general_purpose::STANDARD.encode(svg)))
}

/// A sign-in waiting for the phone or the window, which "Cancel" stops.
static WAITING: once_cell::sync::Lazy<parking_lot::Mutex<Option<tokio::sync::oneshot::Sender<()>>>> = once_cell::sync::Lazy::new(Default::default);
/// The QR sign-in started by `qr_start`, for `qr_wait`.
static QR: once_cell::sync::Lazy<parking_lot::Mutex<Option<Qr>>> = once_cell::sync::Lazy::new(Default::default);

/// Stop a Steam sign-in that waits.
pub fn cancel() {
    QR.lock().take();
    if let Some(tx) = WAITING.lock().take() {
        let _ = tx.send(());
    }
}

/// Start a QR sign-in: the code to show.
pub async fn qr_start() -> Result<String> {
    cancel();
    let q = qr_begin().await?;
    let img = qr_image(&q.url)?;
    *QR.lock() = Some(q);
    Ok(img)
}

/// Wait until the QR code is approved in the Steam mobile app. `show` gets each new code (Steam
/// changes it now and then, and a code that ran out is replaced).
pub async fn qr_wait(show: impl Fn(String) + Send) -> Result<Session> {
    let (tx, mut rx) = tokio::sync::oneshot::channel();
    *WAITING.lock() = Some(tx);
    let t0 = std::time::Instant::now();
    let r = async {
        loop {
            let Some(mut q) = QR.lock().clone() else { bail!("CANCELED|Sign-in canceled") };
            tokio::select! {
                _ = &mut rx => bail!("CANCELED|Sign-in canceled"),
                _ = tokio::time::sleep(Duration::from_secs(q.interval)) => {}
            }
            if t0.elapsed() > MAX_WAIT {
                bail!("The Steam sign-in took too long. Try again.");
            }
            let (v, e) = auth_call("PollAuthSessionStatus", &[("client_id", &q.client_id), ("request_id", &q.request_id)]).await?;
            match parse_poll(&v, e)? {
                Poll::Approved { refresh_token, access_token } => return session_of(refresh_token, access_token).await,
                Poll::Expired => {
                    q = qr_begin().await?;
                    show(qr_image(&q.url)?);
                }
                Poll::Waiting { new_url, new_client_id } => {
                    if let Some(id) = new_client_id {
                        q.client_id = id;
                    }
                    if let Some(u) = new_url {
                        show(qr_image(&u)?);
                        q.url = u;
                    }
                }
            }
            let mut cur = QR.lock();
            if cur.is_none() {
                bail!("CANCELED|Sign-in canceled");
            }
            *cur = Some(q);
        }
    }
    .await;
    WAITING.lock().take();
    QR.lock().take();
    r
}

/// The fields of a Steam session token (a JWT): its account and when it runs out.
pub fn token_claims(token: &str) -> Option<(String, i64)> {
    use base64::Engine;
    let body = token.split('.').nth(1)?;
    let v: Value = serde_json::from_slice(&base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(body.trim_end_matches('=')).ok()?).ok()?;
    let sub = v.get("sub").and_then(Value::as_str).filter(|s| s.len() == 17 && s.bytes().all(|b| b.is_ascii_digit()))?.to_string();
    Some((sub, v.get("exp").and_then(Value::as_i64).unwrap_or(0)))
}

async fn session_of(refresh_token: String, access_token: String) -> Result<Session> {
    let (steam_id, expires_at) = token_claims(&access_token).or_else(|| token_claims(&refresh_token)).ok_or_else(|| anyhow!("Steam gave an unreadable sign-in"))?;
    let name = persona_name(&steam_id).await.unwrap_or_default();
    Ok(Session { steam_id, name, refresh_token: Some(refresh_token), access_token, expires_at })
}

/// The account's public name (its profile's XML, which Steam shows to anyone).
async fn persona_name(steam_id: &str) -> Option<String> {
    let xml = crate::http::CLIENT.get(format!("https://steamcommunity.com/profiles/{steam_id}/?xml=1")).timeout(Duration::from_secs(15)).send().await.ok()?.text().await.ok()?;
    let doc = roxmltree::Document::parse(&xml).ok()?;
    doc.descendants().find(|n| n.has_tag_name("steamID")).and_then(|n| n.text()).map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

/// A QR session whose access is good for a while yet (renewed and kept when it ran out).
async fn access() -> Result<Session> {
    let s: Session = vault::load(Platform::Steam.key()).ok_or_else(|| anyhow!("SIGNIN|Sign in to Steam again"))?;
    let Some(refresh) = s.refresh_token.clone() else { return Ok(s) };
    if !s.access_token.is_empty() && s.expires_at - now() > REFRESH_EARLY.as_secs() as i64 {
        return Ok(s);
    }
    let (v, e) = auth_call("GenerateAccessTokenForApp", &[("refresh_token", &refresh), ("steamid", &s.steam_id)]).await?;
    let token = v.get("access_token").and_then(Value::as_str).filter(|t| !t.is_empty());
    let Some(token) = token.filter(|_| e == 1) else { bail!("SIGNIN|Sign in to Steam again") };
    let fresh = Session {
        expires_at: token_claims(token).map(|c| c.1).unwrap_or(now() + 3600),
        access_token: token.to_string(),
        refresh_token: Some(v.get("refresh_token").and_then(Value::as_str).filter(|t| !t.is_empty()).map(str::to_string).unwrap_or(refresh)),
        ..s
    };
    vault::save(Platform::Steam.key(), &fresh)?;
    Ok(fresh)
}

fn owned(id: u64, title: &str) -> Option<Owned> {
    let title = title.trim();
    (!title.is_empty()).then(|| Owned {
        platform: Platform::Steam,
        id: id.to_string(),
        name: title.to_string(),
        image: Some(format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{id}/header.jpg")),
        namespace: String::new(),
        catalog_item: String::new(),
        latest: None,
        hand_off: true,
    })
}

fn sorted(mut games: Vec<Owned>) -> Vec<Owned> {
    games.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    games
}

/// The games in Steam's Web API answer (`IPlayerService/GetOwnedGames` with app info).
pub fn parse_owned_games(v: &Value) -> Option<Vec<Owned>> {
    let games = v.get("response")?.get("games").and_then(Value::as_array).cloned().unwrap_or_default();
    Some(sorted(games.iter().filter_map(|g| owned(g.get("appid")?.as_u64()?, g.get("name")?.as_str()?)).collect()))
}

async fn owned_games(s: &Session) -> Result<Vec<Owned>> {
    let url = format!("https://api.steampowered.com/IPlayerService/GetOwnedGames/v1/?steamid={}&include_appinfo=true&include_played_free_games=true&access_token={}", s.steam_id, s.access_token);
    let r = crate::http::CLIENT.get(url).send().await.context("Could not reach Steam")?;
    if matches!(r.status().as_u16(), 401 | 403) {
        bail!("SIGNIN|Sign in to Steam again");
    }
    if !r.status().is_success() {
        bail!("Steam answered {}", r.status());
    }
    parse_owned_games(&r.json().await?).ok_or_else(|| anyhow!("Steam gave an unreadable games list"))
}

// ---- Steam window ----

/// The account's games page; Steam sends a signed-out window to its sign-in page first.
pub const GAMES_URL: &str = "https://steamcommunity.com/my/games/?tab=all";

/// Reads the games list the page carries (or says the page is Steam's sign-in page).
pub const READ_GAMES: &str = "(() => { try { if (/^\\/login/i.test(location.pathname)) return 'LOGIN'; const t = document.querySelector('[data-profile-gameslist]'); return t ? (t.getAttribute('data-profile-gameslist') || '') : ''; } catch (e) { return ''; } })()";

fn steam_host(u: &url::Url) -> bool {
    u.host_str().is_some_and(|h| h.eq_ignore_ascii_case("steamcommunity.com"))
}

/// The account's own games page (/id/<name>/games or /profiles/<id>/games).
pub fn is_games_page(u: &url::Url) -> bool {
    let parts: Vec<&str> = u.path().split('/').filter(|p| !p.is_empty()).collect();
    steam_host(u) && parts.len() == 3 && matches!(parts[0], "id" | "profiles") && parts[2].eq_ignore_ascii_case("games")
}

fn read_done(u: &url::Url) -> bool {
    is_games_page(u) || (steam_host(u) && u.path().to_ascii_lowercase().starts_with("/login"))
}

/// The account, its name and its games, from the list the games page carries.
pub fn parse_games_page(json: &str) -> Option<(String, String, Vec<Owned>)> {
    let v: Value = serde_json::from_str(json.trim()).ok()?;
    if v.get("bViewingOwnProfile").and_then(Value::as_bool) == Some(false) {
        return None;
    }
    let steam_id = v.get("strSteamId").and_then(Value::as_str).filter(|s| s.len() == 17)?.to_string();
    let name = v.get("strProfileName").and_then(Value::as_str).unwrap_or("").to_string();
    let games = v.get("rgGames")?.as_array()?.iter().filter_map(|g| owned(g.get("appid")?.as_u64()?, g.get("name")?.as_str()?)).collect();
    Some((steam_id, name, sorted(games)))
}

/// Sign in in a Steam window of the app; the account and its games.
pub async fn window_sign_in(app: &tauri::AppHandle) -> Result<(Session, Vec<Owned>)> {
    cancel();
    let (tx, rx) = tokio::sync::oneshot::channel();
    *WAITING.lock() = Some(tx);
    let r = tokio::select! {
        r = window::run(app, Platform::Steam, GAMES_URL, is_games_page, READ_GAMES) => r,
        _ = rx => {
            window::close(app, Platform::Steam);
            Err(anyhow!("CANCELED|Sign-in canceled"))
        }
    };
    WAITING.lock().take();
    let (steam_id, name, games) = parse_games_page(&r?).ok_or_else(|| anyhow!("Steam did not show your games list. Try again."))?;
    Ok((Session { steam_id, name, refresh_token: None, access_token: String::new(), expires_at: 0 }, games))
}

/// The games again, for the account signed in.
pub async fn library(app: &tauri::AppHandle) -> Result<Vec<Owned>> {
    let s = access().await?;
    if s.refresh_token.is_some() {
        return owned_games(&s).await;
    }
    let page = window::read_hidden(app, Platform::Steam, GAMES_URL, read_done, READ_GAMES).await?;
    if page.trim() == "LOGIN" {
        bail!("SIGNIN|Sign in to Steam again");
    }
    let (_, name, games) = parse_games_page(&page).ok_or_else(|| anyhow!("Steam did not show your games list"))?;
    if !name.is_empty() && name != s.name {
        let _ = vault::save(Platform::Steam.key(), &Session { name, ..s });
    }
    Ok(games)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_qr_sign_in_is_read() {
        let v: Value = serde_json::from_str(r#"{"client_id":"123","challenge_url":"https://s.team/q/1/456","request_id":"cmVxdWVzdA==","interval":5,"allowed_confirmations":[{"confirmation_type":4}],"version":1}"#).unwrap();
        assert_eq!(parse_qr(&v), Some(Qr { client_id: "123".into(), request_id: "cmVxdWVzdA==".into(), url: "https://s.team/q/1/456".into(), interval: 5 }));
        // A code that leads anywhere else is not shown.
        let bad: Value = serde_json::from_str(r#"{"client_id":"1","challenge_url":"https://evil.example/q","request_id":"x"}"#).unwrap();
        assert_eq!(parse_qr(&bad), None);
        assert!(qr_image("https://s.team/q/1/456").unwrap().starts_with("data:image/svg+xml;base64,"));
    }

    #[test]
    fn a_poll_tells_waiting_approved_and_expired() {
        let empty: Value = serde_json::from_str("{}").unwrap();
        assert_eq!(parse_poll(&empty, 1).unwrap(), Poll::Waiting { new_url: None, new_client_id: None });
        let new: Value = serde_json::from_str(r#"{"new_challenge_url":"https://s.team/q/1/789","new_client_id":"124","had_remote_interaction":true}"#).unwrap();
        assert_eq!(parse_poll(&new, 1).unwrap(), Poll::Waiting { new_url: Some("https://s.team/q/1/789".into()), new_client_id: Some("124".into()) });
        let ok: Value = serde_json::from_str(r#"{"refresh_token":"r.t.x","access_token":"a.t.x","account_name":"someone"}"#).unwrap();
        assert_eq!(parse_poll(&ok, 1).unwrap(), Poll::Approved { refresh_token: "r.t.x".into(), access_token: "a.t.x".into() });
        assert_eq!(parse_poll(&empty, 27).unwrap(), Poll::Expired);
        assert!(parse_poll(&empty, 5).is_err());
    }

    #[test]
    fn a_session_token_names_its_account() {
        use base64::Engine;
        let body = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(r#"{"iss":"steam","sub":"76561190000000000","aud":["web"],"exp":1800000000}"#);
        assert_eq!(token_claims(&format!("eyJhbGciOiJFZERTQSJ9.{body}.c2ln")), Some(("76561190000000000".into(), 1800000000)));
        assert_eq!(token_claims("not a token"), None);
    }

    #[test]
    fn the_web_apis_games_list_is_read() {
        let v: Value = serde_json::from_str(r#"{"response":{"game_count":3,"games":[{"appid":431960,"name":"Wallpaper Engine","playtime_forever":5},{"appid":1174180,"name":"Red Dead Redemption 2"},{"appid":5,"name":" "}]}}"#).unwrap();
        let g = parse_owned_games(&v).unwrap();
        assert_eq!(g.iter().map(|g| (g.id.as_str(), g.name.as_str())).collect::<Vec<_>>(), vec![("1174180", "Red Dead Redemption 2"), ("431960", "Wallpaper Engine")]);
        assert!(g.iter().all(|g| g.hand_off && g.image.as_deref().is_some_and(|u| u.starts_with("https://cdn.cloudflare.steamstatic.com/"))));
        // A private list is an empty answer, not an error.
        assert_eq!(parse_owned_games(&serde_json::from_str(r#"{"response":{}}"#).unwrap()).unwrap().len(), 0);
    }

    #[test]
    fn the_games_page_is_known_and_read() {
        assert!(is_games_page(&"https://steamcommunity.com/id/player/games/?tab=all".parse().unwrap()));
        assert!(is_games_page(&"https://steamcommunity.com/profiles/76561190000000000/games".parse().unwrap()));
        assert!(!is_games_page(&"https://steamcommunity.com/login/home/?goto=my%2Fgames".parse().unwrap()));
        assert!(!is_games_page(&"https://evil.example/id/player/games".parse().unwrap()));
        assert!(read_done(&"https://steamcommunity.com/login/home/?goto=my%2Fgames".parse().unwrap()));
        let page = r#"{"strProfileName":"Player","bViewingOwnProfile":true,"strSteamId":"76561190000000000","rgGames":[{"appid":1174180,"name":"Red Dead Redemption 2"},{"appid":431960,"name":"Wallpaper Engine"}]}"#;
        let (id, name, g) = parse_games_page(page).unwrap();
        assert_eq!((id.as_str(), name.as_str(), g.len()), ("76561190000000000", "Player", 2));
        // Someone else's profile is not the account's own list.
        assert!(parse_games_page(r#"{"bViewingOwnProfile":false,"strSteamId":"76561190000000000","rgGames":[]}"#).is_none());
    }

    /// Run with `cargo test live_steam_qr -- --ignored --nocapture`: Steam starts a QR sign-in
    /// (nobody approves it; it runs out by itself).
    #[tokio::test]
    #[ignore]
    async fn live_steam_qr() {
        let q = qr_begin().await.unwrap();
        assert!(q.url.starts_with("https://s.team/q/"));
        let (v, e) = auth_call("PollAuthSessionStatus", &[("client_id", &q.client_id), ("request_id", &q.request_id)]).await.unwrap();
        eprintln!("poll: eresult {e}, {:?}", parse_poll(&v, e));
    }
}
