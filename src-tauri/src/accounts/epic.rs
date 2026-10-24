//! Epic Games sign-in, as Epic's own launcher does it: the user signs in on epicgames.com, Epic's
//! redirect page then shows a one-time code, which is traded for a session that is refreshed when
//! it runs out. The session reads the library (every game owned, with its newest build) and lets
//! the games be downloaded and started signed in.

use super::{now, vault, Owned, Platform, REFRESH_EARLY};
use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// The Epic launcher's own client (the one every Epic launcher replacement signs in as).
const CLIENT_ID: &str = "34a02cf8f4414e29b15921876da36f9a";
const CLIENT_SECRET: &str = "daafbccc737745039dffe53d94fc76cf";
const OAUTH: &str = "https://account-public-service-prod03.ol.epicgames.com/account/api/oauth";
const ASSETS: &str = "https://launcher-public-service-prod06.ol.epicgames.com/launcher/api/public/assets/Windows?label=Live";
const CATALOG: &str = "https://catalog-public-service-prod06.ol.epicgames.com/catalog/api/shared/namespace";

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    /// Unix seconds
    pub expires_at: i64,
    pub refresh_token: String,
    pub refresh_expires_at: i64,
    pub account_id: String,
    pub display_name: String,
}

pub fn login_url() -> String {
    let redirect = format!("https://www.epicgames.com/id/api/redirect?clientId={CLIENT_ID}&responseType=code");
    format!("https://www.epicgames.com/id/login?redirectUrl={}", url::form_urlencoded::byte_serialize(redirect.as_bytes()).collect::<String>())
}

/// Epic's sign-in codes: 32 hex digits.
fn is_code(c: &str) -> bool {
    c.len() == 32 && c.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The sign-in code in what the user pasted from the browser: the page Epic shows after the
/// sign-in (JSON with `authorizationCode`), the address in it (`...?code=`), or the code alone.
pub fn code_from_text(text: &str) -> Option<String> {
    let t = text.trim().trim_matches('"').trim();
    if is_code(t) {
        return Some(t.to_string());
    }
    let json = t.find('{').zip(t.rfind('}')).filter(|(a, b)| a < b).and_then(|(a, b)| serde_json::from_str::<Value>(&t[a..=b]).ok());
    if let Some(v) = json {
        return v.get("authorizationCode").and_then(Value::as_str).map(str::trim).filter(|c| is_code(c)).map(str::to_string);
    }
    let u: url::Url = t.parse().ok()?;
    u.query_pairs().find(|(k, _)| k == "code").map(|(_, v)| v.trim().to_string()).filter(|c| is_code(c))
}

fn session_of(v: &Value) -> Result<Session> {
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str| v.get(k).and_then(Value::as_i64).unwrap_or(0);
    let access = s("access_token");
    if access.is_empty() {
        let why = v.get("errorMessage").and_then(Value::as_str).unwrap_or("no session given");
        bail!("Epic Games refused the sign-in: {why}");
    }
    let t = now();
    Ok(Session { access_token: access, expires_at: t + n("expires_in"), refresh_token: s("refresh_token"), refresh_expires_at: t + n("refresh_expires"), account_id: s("account_id"), display_name: s("displayName") })
}

async fn token(form: &[(&str, &str)]) -> Result<Session> {
    let r = crate::http::CLIENT.post(format!("{OAUTH}/token")).basic_auth(CLIENT_ID, Some(CLIENT_SECRET)).form(form).send().await.context("Could not reach Epic Games")?;
    let v: Value = r.json().await.context("Epic Games gave an unreadable answer")?;
    session_of(&v)
}

/// Trade the sign-in code for a session.
pub async fn exchange(code: &str) -> Result<Session> {
    token(&[("grant_type", "authorization_code"), ("code", code), ("token_type", "eg1")]).await
}

/// A session that is good for a while yet (refreshed and kept when it was running out).
pub async fn access() -> Result<Session> {
    let s: Session = vault::load(Platform::Epic.key()).ok_or_else(|| anyhow!("SIGNIN|Sign in to Epic Games again"))?;
    if s.expires_at - now() > REFRESH_EARLY.as_secs() as i64 {
        return Ok(s);
    }
    if s.refresh_expires_at <= now() {
        bail!("SIGNIN|Sign in to Epic Games again");
    }
    let fresh = token(&[("grant_type", "refresh_token"), ("refresh_token", &s.refresh_token), ("token_type", "eg1")]).await.map_err(|e| anyhow!("SIGNIN|Sign in to Epic Games again ({e})"))?;
    vault::save(Platform::Epic.key(), &fresh)?;
    Ok(fresh)
}

/// A one-time code a game takes on its command line to start signed in (as Epic's launcher
/// starts it).
pub async fn exchange_code() -> Result<String> {
    let s = access().await?;
    let v: Value = crate::http::CLIENT.get(format!("{OAUTH}/exchange")).bearer_auth(&s.access_token).send().await?.json().await?;
    v.get("code").and_then(Value::as_str).map(str::to_string).ok_or_else(|| anyhow!("Epic Games gave no start code"))
}

async fn get(url: &str, token: &str) -> Result<Value> {
    let r = crate::http::CLIENT.get(url).bearer_auth(token).send().await.with_context(|| format!("Could not reach {}", crate::http::host(url)))?;
    if r.status() == reqwest::StatusCode::UNAUTHORIZED {
        bail!("SIGNIN|Sign in to Epic Games again");
    }
    if !r.status().is_success() {
        bail!("{} answered {}", crate::http::host(url), r.status());
    }
    Ok(r.json().await?)
}

/// An installable build of a game (Epic's "assets" list for Windows).
#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub app_name: String,
    pub namespace: String,
    pub catalog_item: String,
    pub build: String,
}

pub fn parse_assets(v: &Value) -> Vec<Asset> {
    let s = |a: &Value, k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    v.as_array()
        .map(|a| a.iter().map(|a| Asset { app_name: s(a, "appName"), namespace: s(a, "namespace"), catalog_item: s(a, "catalogItemId"), build: s(a, "buildVersion") }).filter(|a| !a.app_name.is_empty() && !a.namespace.is_empty()).collect())
        .unwrap_or_default()
}

/// What the catalog says of an item: its title and picture, and whether it is a game (not a DLC,
/// an engine plugin or a store-only item).
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub title: String,
    pub image: Option<String>,
    pub game: bool,
    /// Installed by another company's launcher (the EA app, Ubisoft Connect)
    pub third_party: bool,
}

pub fn parse_item(v: &Value, id: &str) -> Option<Item> {
    let it = v.get(id)?;
    let title = it.get("title").and_then(Value::as_str)?.trim().to_string();
    let images: Vec<(&str, &str)> = it.get("keyImages").and_then(Value::as_array).map(|a| a.iter().filter_map(|k| Some((k.get("type")?.as_str()?, k.get("url")?.as_str()?))).collect()).unwrap_or_default();
    let image = ["DieselGameBox", "OfferImageWide", "DieselGameBoxTall", "Thumbnail", "DieselStoreFrontWide"].iter().find_map(|t| images.iter().find(|(k, _)| k == t)).map(|(_, u)| u.to_string()).filter(|u| u.starts_with("https://"));
    let dlc = it.get("mainGameItem").is_some_and(|m| !m.is_null());
    let cats: Vec<&str> = it.get("categories").and_then(Value::as_array).map(|a| a.iter().filter_map(|c| c.get("path").and_then(Value::as_str)).collect()).unwrap_or_default();
    let not_game = cats.iter().any(|c| matches!(*c, "addons" | "digitalextras" | "plugins" | "engines" | "assets"));
    let attr = |k: &str| it.get("customAttributes").and_then(|c| c.get(k)).and_then(|a| a.get("value")).and_then(Value::as_str).unwrap_or("").to_string();
    let third_party = !attr("ThirdPartyManagedApp").is_empty() || attr("partnerLinkType").eq_ignore_ascii_case("ubisoft");
    let game = !dlc && !not_game && !title.is_empty();
    Some(Item { title, image, game, third_party })
}

/// Every game the account owns that installs on Windows, with its newest build. Titles and
/// pictures already read are kept from the last time, so only new games ask the catalog.
pub async fn library() -> Result<Vec<Owned>> {
    let s = access().await?;
    let assets = parse_assets(&get(ASSETS, &s.access_token).await?);
    let known: HashMap<String, Owned> = super::library(Platform::Epic).games.into_iter().map(|g| (g.id.clone(), g)).collect();
    // The Unreal Engine itself and its store assets are not games.
    let wanted: Vec<Asset> = {
        let mut seen = std::collections::HashSet::new();
        assets.into_iter().filter(|a| a.namespace != "ue" && seen.insert(a.app_name.clone())).collect()
    };
    // Gathered first: a stream over a borrowing iterator is not Send for the async runtime.
    let lookups: Vec<_> = wanted.iter().filter(|a| !known.contains_key(&a.app_name)).map(|a| {
        let token = s.access_token.clone();
        let a = a.clone();
        async move {
            let url = format!("{CATALOG}/{}/bulk/items?id={}&includeDLCDetails=true&includeMainGameDetails=true&country=US&locale=en-US", a.namespace, a.catalog_item);
            (a.app_name.clone(), get(&url, &token).await.ok().and_then(|v| parse_item(&v, &a.catalog_item)))
        }
    }).collect();
    use futures_util::StreamExt;
    let found: HashMap<String, Option<Item>> = futures_util::stream::iter(lookups).buffer_unordered(8).collect::<Vec<_>>().await.into_iter().collect();
    let mut out = Vec::new();
    for a in wanted {
        if let Some(k) = known.get(&a.app_name) {
            out.push(Owned { latest: Some(a.build.clone()), ..k.clone() });
            continue;
        }
        let Some(Some(it)) = found.get(&a.app_name) else { continue };
        if !it.game {
            continue;
        }
        out.push(Owned { platform: Platform::Epic, id: a.app_name.clone(), name: it.title.clone(), image: it.image.clone(), namespace: a.namespace, catalog_item: a.catalog_item, latest: Some(a.build), hand_off: it.third_party });
    }
    // Owned games with no Windows build are not in the assets list: they cannot be played here.
    out.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sign_in_page_and_its_code_are_known() {
        let u = login_url();
        assert!(u.starts_with("https://www.epicgames.com/id/login?redirectUrl=https%3A%2F%2Fwww.epicgames.com%2Fid%2Fapi%2Fredirect%3FclientId%3D34a02cf8f4414e29b15921876da36f9a"));
        let code = "0123456789abcdef0123456789abcdef";
        let page = format!(r#"{{"warning":"Do not share this code with any 3rd party service.","redirectUrl":"https://localhost/launcher/authorized?code={code}","authorizationCode":"{code}","exchangeCode":null,"sid":null}}"#);
        // The page's text, as copied with or without the browser's "Pretty print" line.
        assert_eq!(code_from_text(&page).as_deref(), Some(code));
        assert_eq!(code_from_text(&format!("Pretty print\n{page}\n")).as_deref(), Some(code));
        // The code alone, with its quotes, or the address in the page.
        assert_eq!(code_from_text(&format!("  \"{code}\" ")).as_deref(), Some(code));
        assert_eq!(code_from_text(&format!("https://localhost/launcher/authorized?code={code}")).as_deref(), Some(code));
        assert_eq!(code_from_text(r#"{"authorizationCode":null}"#), None);
        assert_eq!(code_from_text("<html>sign in</html>"), None);
        assert_eq!(code_from_text("my password"), None);
    }

    #[test]
    fn a_session_answer_is_read_and_a_refusal_is_an_error() {
        let ok: Value = serde_json::from_str(r#"{"access_token":"a","expires_in":7200,"refresh_token":"r","refresh_expires":28800,"account_id":"id","displayName":"Player"}"#).unwrap();
        let s = session_of(&ok).unwrap();
        assert_eq!((s.access_token.as_str(), s.refresh_token.as_str(), s.display_name.as_str()), ("a", "r", "Player"));
        assert!(s.expires_at > now() + 7000 && s.refresh_expires_at > now() + 28000);
        let bad: Value = serde_json::from_str(r#"{"errorCode":"errors.com.epicgames.account.oauth.authorization_code_not_found","errorMessage":"Sorry the authorization code you supplied was not found."}"#).unwrap();
        assert!(session_of(&bad).err().unwrap().to_string().contains("was not found"));
    }

    #[test]
    fn games_are_told_from_dlc_and_store_extras() {
        let assets: Value = serde_json::from_str(r#"[{"appName":"Fortnite","labelName":"Live-Windows","buildVersion":"++Fortnite+Release-42.40-CL-1-Windows","catalogItemId":"4fe7","namespace":"fn","assetId":"Fortnite"},{"appName":"","namespace":"x"}]"#).unwrap();
        assert_eq!(parse_assets(&assets), vec![Asset { app_name: "Fortnite".into(), namespace: "fn".into(), catalog_item: "4fe7".into(), build: "++Fortnite+Release-42.40-CL-1-Windows".into() }]);
        let game: Value = serde_json::from_str(r#"{"4fe7":{"title":"Fortnite","keyImages":[{"type":"Thumbnail","url":"https://cdn1.epicgames.com/thumb.jpg"},{"type":"DieselGameBox","url":"https://cdn1.epicgames.com/box.jpg"}],"categories":[{"path":"games"},{"path":"applications"}],"mainGameItem":null}}"#).unwrap();
        assert_eq!(parse_item(&game, "4fe7"), Some(Item { title: "Fortnite".into(), image: Some("https://cdn1.epicgames.com/box.jpg".into()), game: true, third_party: false }));
        let dlc: Value = serde_json::from_str(r#"{"d1":{"title":"Season Pass","categories":[{"path":"addons"}],"mainGameItem":{"id":"4fe7"}}}"#).unwrap();
        assert!(!parse_item(&dlc, "d1").unwrap().game);
        let ea: Value = serde_json::from_str(r#"{"e1":{"title":"An EA game","customAttributes":{"ThirdPartyManagedApp":{"type":"STRING","value":"Origin"}},"categories":[{"path":"games"}]}}"#).unwrap();
        assert!(parse_item(&ea, "e1").unwrap().third_party);
        assert_eq!(parse_item(&game, "missing"), None);
    }
}
