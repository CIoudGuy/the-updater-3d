//! Tray icon, its popup window and its right-click menu.

use crate::core::Core;
use crate::model::AppStatus;
use std::sync::Arc;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalPosition, Rect, WebviewUrl, WebviewWindowBuilder};

const POPUP_W: f64 = 360.0;
const POPUP_H: f64 = 480.0;

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, "open", "Open The Updater 3D", true, None::<&str>)?;
    let check = MenuItem::with_id(app, "check", "Check for updates", true, None::<&str>)?;
    let update = MenuItem::with_id(app, "update_all", "Update all", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &check, &update, &PredefinedMenuItem::separator(app)?, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/32x32.png")).ok().or_else(|| app.default_window_icon().cloned()).expect("icon"))
        .tooltip("The Updater 3D")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, ev| {
            let core = app.state::<Arc<Core>>().inner().clone();
            match ev.id.as_ref() {
                "open" => show_main(app),
                "check" => {
                    tauri::async_runtime::spawn(async move {
                        let _ = core.check_now(true).await;
                    });
                }
                "update_all" => {
                    let ids: Vec<String> = core.apps().into_iter().filter(|a| a.status == AppStatus::Update).map(|a| a.id).collect();
                    for id in ids {
                        let _ = core.enqueue(crate::core::JobRequest::Update { app_id: id, version: None });
                    }
                }
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, ev| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = ev {
                toggle_popup(tray.app_handle(), rect);
            }
        })
        .build(app)?;
    Ok(())
}

/// Show the main window, creating it if needed. In the background the app runs without any
/// window (no web view, a few MB), so the window is built on demand and destroyed on close.
/// Make the web view behave like an app, not a browser: no link address bubble on hover, no
/// browser shortcuts (find bar, reload, print, zoom), no "open in new window".
pub fn app_like(w: &tauri::WebviewWindow) {
    let _ = w.with_webview(|wv| unsafe {
        let Ok(core) = wv.controller().CoreWebView2() else { return };
        if let Ok(s) = core.Settings() {
            let _ = s.SetIsStatusBarEnabled(false);
            use windows::core::Interface;
            if let Ok(s3) = s.cast::<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Settings3>() {
                let _ = s3.SetAreBrowserAcceleratorKeysEnabled(false);
            }
            let _ = s.SetIsZoomControlEnabled(false);
            let _ = s.SetAreDefaultContextMenusEnabled(false);
        }
    });
}

pub fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
        return;
    }
    let core = app.state::<Arc<Core>>().inner().clone();
    let geo = core.persisted.lock().window;
    let zoom = core.settings.read().ui_zoom;
    let mut b = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
        .title("The Updater 3D")
        .inner_size(1120.0, 720.0)
        .min_inner_size(900.0, 600.0)
        .decorations(false)
        .resizable(true)
        .visible(false)
        .zoom_hotkeys_enabled(false)
        .disable_drag_drop_handler()
        .background_color(tauri::window::Color(0x14, 0x14, 0x14, 0xFF));
    let mut placed = false;
    if let Some((x, y, w, h, _)) = geo {
        // Only reuse the old spot if it is still on a connected screen.
        let on_screen = app.available_monitors().unwrap_or_default().iter().any(|m| {
            let (mx, my) = (m.position().x, m.position().y);
            let (mw, mh) = (m.size().width as i32, m.size().height as i32);
            x + 80 > mx && x < mx + mw - 80 && y >= my - 10 && y < my + mh - 80
        });
        if on_screen && w >= 600 && h >= 400 {
            b = b.position(0.0, 0.0);
            placed = true;
        }
    }
    if !placed {
        b = b.center();
    }
    let Ok(w) = b.build() else { return };
    app_like(&w);
    if let (true, Some((x, y, width, height, max))) = (placed, geo) {
        let _ = w.set_size(tauri::PhysicalSize::new(width, height));
        let _ = w.set_position(PhysicalPosition::new(x, y));
        if max {
            let _ = w.maximize();
        }
    }
    if (zoom - 1.0).abs() > 0.001 {
        let _ = w.set_zoom(zoom);
    }
    let w2 = w.clone();
    let core2 = core.clone();
    w.on_window_event(move |e| match e {
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => remember(&w2, &core2),
        _ => {}
    });
    let _ = w.show();
    let _ = w.set_focus();
}

/// Keep the window's place and size (not while minimized; maximized keeps the normal size).
fn remember(w: &tauri::WebviewWindow, core: &Arc<Core>) {
    if w.is_minimized().unwrap_or(false) {
        return;
    }
    let max = w.is_maximized().unwrap_or(false);
    {
        let mut p = core.persisted.lock();
        if max {
            if let Some(g) = p.window.as_mut() {
                g.4 = true;
            } else if let (Ok(pos), Ok(size)) = (w.outer_position(), w.inner_size()) {
                p.window = Some((pos.x, pos.y, size.width, size.height, true));
            }
        } else if let (Ok(pos), Ok(size)) = (w.outer_position(), w.inner_size()) {
            if size.width >= 400 && size.height >= 300 {
                p.window = Some((pos.x, pos.y, size.width, size.height, false));
            }
        }
    }
    save_soon(core);
}

/// Write the window placement to disk half a second after the last move or resize, so it
/// survives a crash, a forced close or an update restart, without writing on every pixel.
fn save_soon(core: &Arc<Core>) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static GEN: AtomicU64 = AtomicU64::new(0);
    let gen = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let core = core.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if GEN.load(Ordering::SeqCst) == gen {
            crate::store::save_state(&core.persisted.lock());
        }
    });
}

/// When the popup last closed itself: a click on the tray icon is what took its focus, and that
/// same click must close it, not open it again.
static POPUP_CLOSED: parking_lot::Mutex<Option<std::time::Instant>> = parking_lot::Mutex::new(None);

fn toggle_popup(app: &AppHandle, rect: Rect) {
    if POPUP_CLOSED.lock().is_some_and(|t| t.elapsed() < std::time::Duration::from_millis(400)) {
        return;
    }
    let w = match app.get_webview_window("tray") {
        Some(w) => {
            if w.is_visible().unwrap_or(false) {
                let _ = w.close();
                return;
            }
            w
        }
        None => {
            let Ok(w) = WebviewWindowBuilder::new(app, "tray", WebviewUrl::App("tray.html".into()))
                .title("The Updater 3D")
                .inner_size(POPUP_W, POPUP_H)
                .resizable(false)
                .decorations(false)
                .always_on_top(true)
                .skip_taskbar(true)
                .visible(false)
                .focused(true)
                .background_color(tauri::window::Color(0x1A, 0x1A, 0x1A, 0xFF))
                .build()
            else {
                return;
            };
            app_like(&w);
            let w2 = w.clone();
            // Closed, not hidden: a hidden popup keeps a whole web view (and its memory) alive.
            w.on_window_event(move |e| {
                if let tauri::WindowEvent::Focused(false) = e {
                    *POPUP_CLOSED.lock() = Some(std::time::Instant::now());
                    let _ = w2.close();
                }
            });
            w
        }
    };
    // Sit just above (or below) the tray icon, inside the screen.
    let scale = w.scale_factor().unwrap_or(1.0);
    let (pos, size) = match (rect.position, rect.size) {
        (tauri::Position::Physical(p), tauri::Size::Physical(s)) => ((p.x as f64, p.y as f64), (s.width as f64, s.height as f64)),
        (tauri::Position::Logical(p), tauri::Size::Logical(s)) => ((p.x * scale, p.y * scale), (s.width * scale, s.height * scale)),
        (p, s) => {
            let p = p.to_physical::<f64>(scale);
            let s = s.to_physical::<f64>(scale);
            ((p.x, p.y), (s.width, s.height))
        }
    };
    let (pw, ph) = (POPUP_W * scale, POPUP_H * scale);
    let mut x = pos.0 + size.0 / 2.0 - pw / 2.0;
    let mut y = pos.1 - ph - 8.0 * scale;
    if let Ok(Some(m)) = w.current_monitor().or_else(|_| w.primary_monitor()) {
        let mp = m.position();
        let ms = m.size();
        if y < mp.y as f64 {
            y = pos.1 + size.1 + 8.0 * scale;
        }
        x = x.clamp(mp.x as f64 + 8.0, (mp.x as f64 + ms.width as f64 - pw - 8.0).max(mp.x as f64));
    }
    let _ = w.set_position(PhysicalPosition::new(x as i32, y as i32));
    let _ = w.show();
    let _ = w.set_focus();
}

/// Keep the tooltip in step with the number of updates.
pub fn set_count(app: &AppHandle, n: usize) {
    if let Some(t) = app.tray_by_id("main") {
        let tip = match n {
            0 => "The Updater 3D".to_string(),
            1 => "The Updater 3D, 1 update ready".to_string(),
            n => format!("The Updater 3D, {n} updates ready"),
        };
        let _ = t.set_tooltip(Some(tip));
    }
}
