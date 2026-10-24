//! GOG sign-in, as GOG Galaxy does it: the user signs in on gog.com, which sends the window to
//! GOG's "login success" page with a one-time code in its address; the code is traded for a
//! session that is refreshed when it runs out. The session reads the library and downloads the
//! games' builds.

use super::{now, vault, Owned, Platform, REFRESH_EARLY};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// GOG Galaxy's own client (the one every Galaxy replacement signs in as).
const CLIENT_ID: &str = "46899977096215655";
const CLIENT_SECRET: &str = "9d85c43b1482497dbbce61f6e4aa173a433796eeae2ca8c5f6129f2dc4de46d9";
const REDIRECT: &str = "https://embed.gog.com/on_login_success?origin=client";

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub expires_at: i64,
    pub refresh_token: String,
    pub user_id: String,
    pub name: String,
}

fn enc(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

pub fn login_url() -> String {
    format!("https://auth.gog.com/auth?client_id={CLIENT_ID}&redirect_uri={}&response_type=code&layout=client2", enc(REDIRECT))
}

/// The sign-in code in what the user pasted from the browser: the address of GOG's "login
/// success" page the sign-in ends on (`...on_login_success?origin=client&code=...`), or the code
/// alone.
pub fn code_from_text(text: &str) -> Option<String> {
    let t = text.trim().trim_matches('"').trim();
    let plain = |c: &str| c.len() >= 20 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if plain(t) {
        return Some(t.to_string());
    }
    let u: url::Url = t.parse().ok()?;
    if !u.host_str().is_some_and(|h| h.eq_ignore_ascii_case("embed.gog.com")) || !u.path().eq_ignore_ascii_case("/on_login_success") {
        return None;
    }
    u.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.trim().to_string()).filter(|c| plain(c))
}

fn session_of(v: &Value, name: String) -> Result<Session> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let access = s("access_token");
    if access.is_empty() {
        let why = v.get("error_description").or_else(|| v.get("error")).and_then(Value::as_str).unwrap_or("no session given");
        bail!("GOG refused the sign-in: {why}");
    }
    Ok(Session { access_token: access, expires_at: now() + v.get("expires_in").and_then(Value::as_i64).unwrap_or(3600), refresh_token: s("refresh_token"), user_id: s("user_id"), name })
}

async fn token(query: &str, name: String) -> Result<Session> {
    let url = format!("https://auth.gog.com/token?client_id={CLIENT_ID}&client_secret={CLIENT_SECRET}&{query}");
    let v: Value = crate::http::CLIENT.get(url).send().await.context("Could not reach GOG")?.json().await.context("GOG gave an unreadable answer")?;
    session_of(&v, name)
}

/// Trade the sign-in code for a session (with the account's name).
pub async fn exchange(code: &str) -> Result<Session> {
    let mut s = token(&format!("grant_type=authorization_code&code={}&redirect_uri={}", enc(code), enc(REDIRECT)), String::new()).await?;
    if let Ok(v) = get("https://embed.gog.com/userData.json", &s.access_token).await {
        s.name = v.get("username").and_then(Value::as_str).unwrap_or("").to_string();
    }
    Ok(s)
}

/// A session that is good for a while yet (refreshed and kept when it was running out).
pub async fn access() -> Result<Session> {
    let s: Session = vault::load(Platform::Gog.key()).ok_or_else(|| anyhow!("SIGNIN|Sign in to GOG again"))?;
    if s.expires_at - now() > REFRESH_EARLY.as_secs() as i64 {
        return Ok(s);
    }
    refresh(s).await
}

/// A new session now, for when GOG said the one in hand ran out (a long download).
pub async fn renewed() -> Result<Session> {
    refresh(vault::load(Platform::Gog.key()).ok_or_else(|| anyhow!("SIGNIN|Sign in to GOG again"))?).await
}

async fn refresh(s: Session) -> Result<Session> {
    let fresh = token(&format!("grant_type=refresh_token&refresh_token={}", enc(&s.refresh_token)), s.name.clone()).await.map_err(|e| anyhow!("SIGNIN|Sign in to GOG again ({e})"))?;
    vault::save(Platform::Gog.key(), &fresh)?;
    Ok(fresh)
}

async fn get(url: &str, token: &str) -> Result<Value> {
    let r = crate::http::CLIENT.get(url).bearer_auth(token).send().await.with_context(|| format!("Could not reach {}", crate::http::host(url)))?;
    if r.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("SIGNIN|Sign in to GOG again");
    }
    if !r.status().is_success() {
        bail!("{} answered {}", crate::http::host(url), r.status());
    }
    Ok(r.json().await?)
}

pub fn parse_owned(v: &Value) -> Vec<u64> {
    v.get("owned").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default()
}

/// The games among GOG's product answers (not DLC or bundles), which run on Windows.
pub fn parse_products(v: &Value) -> Vec<Owned> {
    let Some(a) = v.as_array() else { return Vec::new() };
    a.iter()
        .filter_map(|p| {
            let id = p.get("id").and_then(Value::as_u64)?;
            let title = p.get("title").and_then(Value::as_str)?.trim().to_string();
            let kind = p.get("game_type").and_then(Value::as_str).unwrap_or("game");
            let windows = p.get("content_system_compatibility").and_then(|c| c.get("windows")).and_then(Value::as_bool).unwrap_or(true);
            if kind != "game" || !windows || title.is_empty() {
                return None;
            }
            let image = p.get("images").and_then(|i| i.get("logo2x").or_else(|| i.get("logo"))).and_then(Value::as_str).map(|u| if u.starts_with("//") { format!("https:{u}") } else { u.to_string() }).filter(|u| u.starts_with("https://"));
            Some(Owned { platform: Platform::Gog, id: id.to_string(), name: title, image, namespace: String::new(), catalog_item: String::new(), latest: None, hand_off: false })
        })
        .collect()
}

/// Every game the account owns. Titles and pictures already read are kept from the last time,
/// so only new games ask GOG's catalog.
pub async fn library() -> Result<Vec<Owned>> {
    let s = access().await?;
    let owned = parse_owned(&get("https://embed.gog.com/user/data/games", &s.access_token).await?);
    let known: HashMap<String, Owned> = super::library(Platform::Gog).games.into_iter().map(|g| (g.id.clone(), g)).collect();
    let new: Vec<u64> = owned.iter().copied().filter(|id| !known.contains_key(&id.to_string())).collect();
    let mut found = Vec::new();
    for chunk in new.chunks(50) {
        let ids = chunk.iter().map(u64::to_string).collect::<Vec<_>>().join(",");
        found.extend(parse_products(&get(&format!("https://api.gog.com/products?ids={ids}"), &s.access_token).await?));
    }
    let mut out: Vec<Owned> = owned.iter().filter_map(|id| known.get(&id.to_string()).cloned()).collect();
    out.extend(found);
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_page_and_its_code_are_known() {
        assert_eq!(login_url(), "https://auth.gog.com/auth?client_id=46899977096215655&redirect_uri=https%3A%2F%2Fembed.gog.com%2Fon_login_success%3Forigin%3Dclient&response_type=code&layout=client2");
        let code = "made-up_code-0123456789abcdefABCDEF";
        assert_eq!(code_from_text(&format!("https://embed.gog.com/on_login_success?origin=client&code={code}")).as_deref(), Some(code));
        assert_eq!(code_from_text(&format!(" {code} ")).as_deref(), Some(code));
        assert_eq!(code_from_text("https://embed.gog.com/on_login_success?origin=client"), None);
        assert_eq!(code_from_text(&format!("https://evil.example/on_login_success?code={code}")), None);
        assert_eq!(code_from_text("my password"), None);
    }

    #[test]
    fn a_session_answer_is_read_and_a_refusal_is_an_error() {
        let ok: Value = serde_json::from_str(r#"{"expires_in":3600,"access_token":"a","user_id":"u","refresh_token":"r","session_id":"s","token_type":"bearer"}"#).unwrap();
        let s = session_of(&ok, "Player".into()).unwrap();
        assert_eq!((s.access_token.as_str(), s.refresh_token.as_str(), s.user_id.as_str(), s.name.as_str()), ("a", "r", "u", "Player"));
        let bad: Value = serde_json::from_str(r#"{"error":"invalid_grant","error_description":"The authorization code has expired"}"#).unwrap();
        assert!(session_of(&bad, String::new()).err().unwrap().to_string().contains("has expired"));
    }

    #[test]
    fn games_are_told_from_dlc_and_mac_only_ones() {
        assert_eq!(parse_owned(&serde_json::from_str(r#"{"owned":[1207658924,2,"x"]}"#).unwrap()), vec![1207658924, 2]);
        let v: Value = serde_json::from_str(r#"[
            {"id":1207658924,"title":"Unreal Tournament 2004","game_type":"game","content_system_compatibility":{"windows":true,"osx":false},"images":{"logo2x":"//images-1.gog-statics.com/ut.png"}},
            {"id":2,"title":"A soundtrack","game_type":"dlc"},
            {"id":3,"title":"Mac only","game_type":"game","content_system_compatibility":{"windows":false,"osx":true}}
        ]"#).unwrap();
        let g = parse_products(&v);
        assert_eq!(g.len(), 1);
        assert_eq!((g[0].id.as_str(), g[0].name.as_str(), g[0].image.as_deref()), ("1207658924", "Unreal Tournament 2004", Some("https://images-1.gog-statics.com/ut.png")));
    }
}
