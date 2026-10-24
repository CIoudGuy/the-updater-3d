//! Release notes pages that are built by script: a page that answered the app's own client but
//! has its notes filled in by JavaScript is opened once in a hidden WebView2 window, and what the
//! page shows is read back. Kept light and private:
//! - only for pages that answered normally (a site that refused the app is never opened) and that
//!   look built by script; a page that shows a bot check is dropped, and so is one that leaves its
//!   own site;
//! - one page at a time, in a private (InPrivate) window with no access to the app, no pop-ups,
//!   no downloads, and no images, media, fonts, style sheets or tracker scripts;
//! - each answer (and each miss) is kept on disk for 12 hours, so every version of an app reuses it.

use once_cell::sync::{Lazy, OnceCell};
use std::path::PathBuf;
use std::time::Duration;
use tauri::AppHandle;

static APP: OnceCell<AppHandle> = OnceCell::new();
static ONE_AT_A_TIME: Lazy<tokio::sync::Semaphore> = Lazy::new(|| tokio::sync::Semaphore::new(1));
static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// How long a rendered page (or a miss) is kept.
#[cfg_attr(test, allow(dead_code))]
const KEEP: Duration = Duration::from_secs(12 * 3600);
/// Longest wait for a page to load, and for its script to fill it in.
const LOAD: Duration = Duration::from_secs(15);
const FILL: Duration = Duration::from_millis(1500);
/// The most read back from one page.
const MAX_HTML: usize = 3_000_000;

/// The running app, so the notes code (which has no handle) can open the window.
pub fn set_app(app: AppHandle) {
    let _ = APP.set(app);
}

/// Text a person would read on a page: without scripts, styles and tags.
fn visible_text_len(html: &str) -> usize {
    static DROP: Lazy<regex::Regex> = Lazy::new(|| regex::Regex::new(r"(?is)<(script|style|noscript|svg|template)\b.*?</(script|style|noscript|svg|template)>|<[^>]+>").unwrap());
    DROP.replace_all(html, " ").split_whitespace().map(str::len).sum()
}

/// Does a page look built by script: it runs scripts and has almost no text of its own?
pub fn script_built(html: &str) -> bool {
    let lower = &html[..html.len().min(400_000)].to_ascii_lowercase();
    lower.contains("<script") && visible_text_len(html) < 1500
}

/// Does a page show a bot check or an access block instead of its content?
pub fn challenged(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    const SIGNS: [&str; 10] = ["cf-chl", "challenge-platform", "just a moment...", "attention required", "captcha", "verify you are human", "access denied", "are you a robot", "ddos-guard", "perimeterx"];
    SIGNS.iter().any(|s| lower.contains(s))
}

/// "docs.example.com" and "example.com" are one site; "example.com.evil.net" is not.
pub fn same_site(a: &str, b: &str) -> bool {
    let (a, b) = (a.trim_start_matches("www.").to_ascii_lowercase(), b.trim_start_matches("www.").to_ascii_lowercase());
    a == b || a.ends_with(&format!(".{b}")) || b.ends_with(&format!(".{a}"))
}

/// Requests the hidden page does not need: pictures, media, fonts and style sheets anywhere, and
/// well-known trackers and ad servers.
pub fn needless(uri: &str, kind: Kind) -> bool {
    const TRACKERS: [&str; 20] = [
        "google-analytics.com", "googletagmanager.com", "doubleclick.net", "googlesyndication.com", "googleadservices.com", "facebook.net", "facebook.com", "connect.facebook.net",
        "hotjar.com", "segment.io", "segment.com", "mixpanel.com", "clarity.ms", "hs-analytics.net", "hubspot.com", "intercom.io", "fullstory.com", "amplitude.com", "newrelic.com", "nr-data.net",
    ];
    if matches!(kind, Kind::Image | Kind::Media | Kind::Font | Kind::Style | Kind::Ping) {
        return true;
    }
    let host = url::Url::parse(uri).ok().and_then(|u| u.host_str().map(str::to_ascii_lowercase)).unwrap_or_default();
    TRACKERS.iter().any(|t| host == *t || host.ends_with(&format!(".{t}")))
}

/// What a request of the hidden page is for (WebView2's resource contexts).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Image,
    Media,
    Font,
    Style,
    Ping,
    Other,
}

fn cache_file(url: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let h = Sha256::digest(url.as_bytes());
    let name: String = h.iter().take(12).map(|b| format!("{b:02x}")).collect();
    crate::paths::data_dir().join("rendered").join(name)
}

/// A page kept from an earlier render: Some(page), Some("") for a page that gave nothing, None
/// when it has to be rendered.
#[cfg_attr(test, allow(dead_code))]
fn cached(url: &str) -> Option<String> {
    let f = cache_file(url);
    let age = std::fs::metadata(&f).ok()?.modified().ok()?.elapsed().ok()?;
    if age > KEEP {
        return None;
    }
    std::fs::read_to_string(f).ok()
}

#[cfg_attr(test, allow(dead_code))]
fn keep(url: &str, html: &str) {
    let f = cache_file(url);
    if let Some(d) = f.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(f, html);
}

/// The page as the browser shows it, read in the running app's hidden window. None when the app
/// has no window system (a test), the page is not https, it blocks or challenges, or it does
/// not load in time.
pub async fn page(url: &str) -> Option<String> {
    // Test programs have no window system (and no manifest for it): the window code stays out.
    #[cfg(test)]
    {
        let _ = url;
        None
    }
    #[cfg(not(test))]
    {
        let app = APP.get()?.clone();
        if let Some(hit) = cached(url) {
            return (!hit.is_empty()).then_some(hit);
        }
        let got = render_in(&app, url, None).await;
        keep(url, got.as_deref().unwrap_or(""));
        got
    }
}

/// Render one page in a hidden window of `app`. `data_dir`: the browser's own data folder (None:
/// the app's).
pub async fn render_in(app: &AppHandle, url: &str, data_dir: Option<PathBuf>) -> Option<String> {
    let target = url::Url::parse(url).ok().filter(|u| u.scheme() == "https")?;
    let site = target.host_str()?.to_string();
    let _turn = ONE_AT_A_TIME.acquire().await.ok()?;
    let label = format!("notes-render-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let (loaded_tx, loaded_rx) = tokio::sync::oneshot::channel::<()>();
    let loaded_tx = std::sync::Mutex::new(Some(loaded_tx));
    let nav_site = site.clone();
    let load_site = site.clone();
    let mut b = tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::External("about:blank".parse().ok()?))
        .visible(false)
        .focused(false)
        .skip_taskbar(true)
        .inner_size(1280.0, 900.0)
        .incognito(true)
        // Only the page's own site: a hand-off to a bot check or another site ends the read.
        .on_navigation(move |u| u.as_str() == "about:blank" || (u.scheme() == "https" && u.host_str().is_some_and(|h| same_site(h, &nav_site))))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .on_page_load(move |_, p| {
            if matches!(p.event(), tauri::webview::PageLoadEvent::Finished) && p.url().host_str().is_some_and(|h| same_site(h, &load_site)) {
                if let Some(tx) = loaded_tx.lock().ok().and_then(|mut t| t.take()) {
                    let _ = tx.send(());
                }
            }
        });
    if let Some(d) = data_dir {
        b = b.data_directory(d);
    }
    let w = b.build().map_err(|e| log::warn!("notes render window: {e}")).ok()?;
    block_needless(&w);
    let result = async {
        w.navigate(target.clone()).ok()?;
        tokio::time::timeout(LOAD, loaded_rx).await.ok()?.ok()?;
        // Its script fills the page in after it loads: read it, and once more if still empty.
        let mut html = String::new();
        for wait in [FILL, FILL * 2] {
            tokio::time::sleep(wait).await;
            html = read_back(&w).await.unwrap_or_default();
            if visible_text_len(&html) >= 600 {
                break;
            }
        }
        (!html.is_empty() && !challenged(&html)).then_some(html)
    }
    .await;
    let _ = w.destroy();
    result
}

/// The page's HTML as the browser has it now.
async fn read_back(w: &tauri::WebviewWindow) -> Option<String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = std::sync::Mutex::new(Some(tx));
    let js = format!("(() => {{ try {{ return document.documentElement.outerHTML.slice(0, {MAX_HTML}); }} catch (e) {{ return ''; }} }})()");
    w.eval_with_callback(js, move |json| {
        if let Some(t) = tx.lock().ok().and_then(|mut t| t.take()) {
            let _ = t.send(json);
        }
    })
    .ok()?;
    let json = tokio::time::timeout(Duration::from_secs(5), rx).await.ok()?.ok()?;
    serde_json::from_str::<String>(&json).ok()
}

/// Answer the page's needless requests (see `needless`) with an empty "blocked" response.
fn block_needless(w: &tauri::WebviewWindow) {
    let _ = w.with_webview(|pw| unsafe {
        use webview2_com::Microsoft::Web::WebView2::Win32::*;
        use webview2_com::{take_pwstr, WebResourceRequestedEventHandler};
        use windows::core::{HSTRING, PWSTR};
        let Ok(core) = pw.controller().CoreWebView2() else { return };
        let env = pw.environment();
        let all = HSTRING::from("*");
        if core.AddWebResourceRequestedFilter(&all, COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL).is_err() {
            return;
        }
        let mut token = 0i64;
        let _ = core.add_WebResourceRequested(
            &WebResourceRequestedEventHandler::create(Box::new(move |_, args| {
                let Some(args) = args else { return Ok(()) };
                let mut ctx = COREWEBVIEW2_WEB_RESOURCE_CONTEXT_ALL;
                args.ResourceContext(&mut ctx)?;
                let kind = match ctx {
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_IMAGE => Kind::Image,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_MEDIA => Kind::Media,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_FONT => Kind::Font,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_STYLESHEET => Kind::Style,
                    COREWEBVIEW2_WEB_RESOURCE_CONTEXT_PING => Kind::Ping,
                    _ => Kind::Other,
                };
                let mut uri = PWSTR::null();
                args.Request()?.Uri(&mut uri)?;
                if needless(&take_pwstr(uri), kind) {
                    let resp = env.CreateWebResourceResponse(None, 403, &HSTRING::from("Blocked"), &HSTRING::from(""))?;
                    args.SetResponse(&resp)?;
                }
                Ok(())
            })),
            &mut token,
        );
    });
}

/// The notes for `ver` on a page that answered the app with `body`: read as it came, and when it
/// is built by script, as the browser shows it.
pub async fn page_notes(url: &str, body: &str, ver: &str) -> Vec<String> {
    let lines = crate::notes::page_notes_at(url, body, ver);
    if !lines.is_empty() || !script_built(body) {
        return lines;
    }
    match page(url).await {
        Some(html) => crate::notes::page_notes_at(url, &html, ver),
        None => vec![],
    }
}

/// `--render-page <url> <out file>`: render one page in a hidden window and write what the
/// browser shows (empty when it gave nothing). For checking the real path from a build; uses its
/// own browser data folder, so the installed app's is never touched.
pub fn render_main(args: &[String]) -> i32 {
    let mut it = args.iter().skip_while(|a| *a != "--render-page").skip(1);
    let (Some(url), Some(out)) = (it.next().cloned(), it.next().cloned()) else {
        eprintln!("--render-page <url> <out file>");
        return 2;
    };
    let data = std::env::temp_dir().join(format!("u3d-render-{}", std::process::id()));
    let code = std::sync::Arc::new(std::sync::atomic::AtomicI32::new(1));
    let code2 = code.clone();
    let data2 = data.clone();
    let built = tauri::Builder::default().setup(move |app| {
        let h = app.handle().clone();
        let (url, out, data, code) = (url.clone(), out.clone(), data2.clone(), code2.clone());
        tauri::async_runtime::spawn(async move {
            let got = render_in(&h, &url, Some(data)).await;
            let _ = std::fs::write(&out, got.as_deref().unwrap_or(""));
            code.store(if got.is_some() { 0 } else { 3 }, std::sync::atomic::Ordering::SeqCst);
            h.exit(0);
        });
        Ok(())
    });
    if let Err(e) = built.run(crate::context()) {
        eprintln!("{e}");
    }
    let _ = std::fs::remove_dir_all(&data);
    code.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_built_by_script_is_told_from_a_page_with_its_text() {
        let spa = r#"<html><head><script src="/app.js"></script></head><body><div id="root"></div><noscript>You need to enable JavaScript to run this app.</noscript></body></html>"#;
        assert!(script_built(spa));
        let real = format!("<html><body><script>var a=1;</script><h2>Version 2.1</h2><ul>{}</ul></body></html>", "<li>Fixed a crash when opening large files from the network</li>".repeat(40));
        assert!(!script_built(&real));
        assert!(!script_built("<html><body>short page, no scripts</body></html>"));
    }

    #[test]
    fn bot_checks_and_blocks_are_not_read_as_notes() {
        assert!(challenged("<title>Just a moment...</title><div id=\"cf-chl-widget\"></div>"));
        assert!(challenged("<h1>Access denied</h1>"));
        assert!(!challenged("<h2>What's new in 5.2</h2><ul><li>Faster sync</li></ul>"));
    }

    #[test]
    fn the_hidden_page_stays_on_its_site_and_loads_only_what_it_needs() {
        assert!(same_site("docs.example.com", "example.com") && same_site("www.example.com", "example.com"));
        assert!(!same_site("example.com.evil.net", "example.com") && !same_site("notexample.com", "example.com"));
        assert!(needless("https://example.com/logo.png", Kind::Image));
        assert!(needless("https://www.google-analytics.com/collect", Kind::Other));
        assert!(needless("https://x.clarity.ms/tag", Kind::Other));
        assert!(!needless("https://api.example.com/releases.json", Kind::Other));
        assert!(!needless("https://cdn.example-cdn.net/app.js", Kind::Other));
    }

    #[tokio::test]
    async fn without_the_app_nothing_is_rendered() {
        // Tests have no window system: nothing is opened, and nothing is written to the app's data.
        assert!(page("https://example.org/release-notes-test-page").await.is_none());
        assert!(!cache_file("https://example.org/release-notes-test-page").exists());
        let spa = r#"<html><head><script src="/app.js"></script></head><body><div id="root"></div></body></html>"#;
        assert!(super::page_notes("https://example.org/release-notes-test-page", spa, "1.0").await.is_empty());
    }
}
