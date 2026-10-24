//! A platform's own page in a window of the app (Steam's "sign in with a Steam window"): shown
//! until the page that ends the sign-in loads, then read. Its cookies stay in the platform's own
//! web profile in the app's data folder, so a hidden window reads the page again later.

use super::Platform;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Longest a sign-in may take before the window gives up.
const MAX_WAIT: Duration = Duration::from_secs(15 * 60);

static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);

/// The web profile of a platform's windows.
pub fn web_dir(p: Platform) -> PathBuf {
    crate::paths::data_dir().join("accounts").join("web").join(p.key())
}

fn close_kind(app: &tauri::AppHandle, kind: &str, p: Platform) {
    let prefix = format!("{kind}-{}-", p.key());
    for (l, w) in tauri::Manager::webview_windows(app) {
        if l.starts_with(&prefix) {
            let _ = w.destroy();
        }
    }
}

/// Close a platform's sign-in window (a canceled sign-in).
pub fn close(app: &tauri::AppHandle, p: Platform) {
    close_kind(app, "signin", p);
}

/// A new window's name; a platform's windows still open from before are closed.
fn label_for(app: &tauri::AppHandle, kind: &str, p: Platform) -> String {
    close_kind(app, kind, p);
    format!("{kind}-{}-{}", p.key(), NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
}

/// Show `start` and let the user sign in; once a page `done` tells is loaded, what `script`
/// returns on it (when not empty). "CANCELED|..." when the user closes the window.
pub async fn run(app: &tauri::AppHandle, p: Platform, start: &str, done: fn(&url::Url) -> bool, script: &'static str) -> Result<String> {
    let label = label_for(app, "signin", p);
    let start_url: url::Url = start.parse().map_err(|_| anyhow!("bad sign-in address"))?;
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<String>>();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let send = {
        let tx = tx.clone();
        move |r: Result<String>| {
            if let Some(t) = tx.lock().ok().and_then(|mut t| t.take()) {
                let _ = t.send(r);
            }
        }
    };
    let load_send = send.clone();
    let w = tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::External(start_url))
        .title(format!("Sign in to {}", p.name()))
        .inner_size(520.0, 760.0)
        .center()
        .data_directory(web_dir(p))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        // Secure pages only (the platform's own).
        .on_navigation(|u| u.scheme() == "https")
        .on_page_load(move |w, pl| {
            if !matches!(pl.event(), tauri::webview::PageLoadEvent::Finished) || !done(pl.url()) {
                return;
            }
            let s = load_send.clone();
            let _ = w.eval_with_callback(script, move |json| {
                let text = serde_json::from_str::<String>(&json).unwrap_or(json);
                if !text.trim().is_empty() {
                    s(Ok(text));
                }
            });
        })
        .build()
        .map_err(|e| anyhow!("Could not open the sign-in window: {e}"))?;
    let closed = send.clone();
    w.on_window_event(move |e| {
        if matches!(e, tauri::WindowEvent::Destroyed) {
            closed(Err(anyhow!("CANCELED|Sign-in closed")));
        }
    });
    let r = match tokio::time::timeout(MAX_WAIT, rx).await {
        Ok(Ok(r)) => r,
        Ok(Err(_)) => Err(anyhow!("CANCELED|Sign-in closed")),
        Err(_) => Err(anyhow!("The sign-in took too long. Try again.")),
    };
    let _ = w.destroy();
    r
}

/// Read a platform's page in a hidden window that has the sign-in's cookies: what `script`
/// returns once a page `done` tells is loaded.
pub async fn read_hidden(app: &tauri::AppHandle, p: Platform, url: &str, done: fn(&url::Url) -> bool, script: &'static str) -> Result<String> {
    let label = label_for(app, "read", p);
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();
    let tx = Arc::new(Mutex::new(Some(tx)));
    let w = tauri::WebviewWindowBuilder::new(app, &label, tauri::WebviewUrl::External(url.parse().map_err(|_| anyhow!("bad address"))?))
        .visible(false)
        .focused(false)
        .skip_taskbar(true)
        .data_directory(web_dir(p))
        .on_new_window(|_, _| tauri::webview::NewWindowResponse::Deny)
        .on_download(|_, _| false)
        .on_navigation(|u| u.scheme() == "https")
        .on_page_load(move |w, pl| {
            if matches!(pl.event(), tauri::webview::PageLoadEvent::Finished) && done(pl.url()) {
                let tx = tx.clone();
                let _ = w.eval_with_callback(script, move |json| {
                    if let Some(t) = tx.lock().ok().and_then(|mut t| t.take()) {
                        let _ = t.send(serde_json::from_str::<String>(&json).unwrap_or(json));
                    }
                });
            }
        })
        .build()
        .map_err(|e| anyhow!("Could not read {}: {e}", p.name()))?;
    let r = tokio::time::timeout(Duration::from_secs(40), rx).await;
    let _ = w.destroy();
    match r {
        Ok(Ok(t)) if !t.trim().is_empty() => Ok(t),
        _ => Err(anyhow!("{} did not answer; sign in again if this goes on", p.name())),
    }
}
