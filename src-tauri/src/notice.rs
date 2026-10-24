//! Windows notifications that open the right page when they are clicked.
//!
//! The app often runs only in the tray, and a toast that moved to the Action Center no longer
//! reaches the process that sent it, so a click can not come back as an event. Instead each toast
//! carries a link (theupdater3d://app/<id>, theupdater3d://queue, ...). Windows opens the link on
//! a click, on screen or later from the Action Center; the link starts this exe, the
//! single-instance plugin hands it to the running copy (a copy that was not running keeps it
//! itself) and the main window opens on that page. The link handler is a small key in the user's
//! own registry, written at start; nothing else is installed.

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

use crate::model::JobKind;

/// The link scheme a notification opens.
pub const SCHEME: &str = "theupdater3d";

/// Windows toasts of a build that is not installed borrow PowerShell's identity.
const POWERSHELL_ID: &str = "{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\\WindowsPowerShell\\v1.0\\powershell.exe";

/// The link handler is in the registry, so a toast may carry a link.
static LINKS_READY: AtomicBool = AtomicBool::new(false);

/// The page a clicked notification opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// One app or driver: its page.
    App(String),
    /// Several updates: Installed, filtered to the updates.
    Updates,
    /// Installed, no filter.
    Installed,
    /// A job waits for the user (an installer, admin rights): the Queue.
    Queue,
    /// Home, which has the restart notice.
    Home,
    /// This app's own page (its update and what is new).
    About,
}

impl Target {
    /// The route the window understands (what "navigate" and `take_route` carry).
    pub fn route(&self) -> String {
        match self {
            Target::App(id) => format!("installed/{}", encode(id)),
            Target::Updates => "updates".into(),
            Target::Installed => "installed".into(),
            Target::Queue => "queue".into(),
            Target::Home => "home".into(),
            Target::About => "updater".into(),
        }
    }

    /// The link a notification carries.
    pub fn link(&self) -> String {
        match self {
            Target::App(id) => format!("{SCHEME}://app/{}", encode(id)),
            Target::Updates => format!("{SCHEME}://updates"),
            Target::Installed => format!("{SCHEME}://installed"),
            Target::Queue => format!("{SCHEME}://queue"),
            Target::Home => format!("{SCHEME}://home"),
            Target::About => format!("{SCHEME}://about"),
        }
    }

    /// Read a link back. Anything that is not exactly one of ours is nothing: the link comes from
    /// outside (any program can open it), so it can only pick a page, never run an action.
    pub fn from_link(text: &str) -> Option<Target> {
        let (scheme, rest) = text.trim().split_once(':')?;
        if !scheme.eq_ignore_ascii_case(SCHEME) {
            return None;
        }
        let rest = rest.split(['?', '#']).next().unwrap_or("").trim_start_matches('/');
        let parts: Vec<&str> = rest.split('/').filter(|p| !p.is_empty()).collect();
        match parts.as_slice() {
            [w] if w.eq_ignore_ascii_case("updates") => Some(Target::Updates),
            [w] if w.eq_ignore_ascii_case("installed") => Some(Target::Installed),
            [w] if w.eq_ignore_ascii_case("queue") => Some(Target::Queue),
            [w] if w.eq_ignore_ascii_case("home") => Some(Target::Home),
            [w] if w.eq_ignore_ascii_case("about") => Some(Target::About),
            [w, id] if w.eq_ignore_ascii_case("app") => {
                let id = decode(id)?;
                (!id.is_empty() && id.len() <= 512 && !id.chars().any(char::is_control)).then_some(Target::App(id))
            }
            _ => None,
        }
    }
}

/// The route a start or a second start asks for through a link in its arguments.
pub fn route_in_args(args: &[String]) -> Option<String> {
    args.iter().find_map(|a| Target::from_link(a)).map(|t| t.route())
}

/// One update opens its app; several open the updates list.
pub fn updates_target(ids: &[&str]) -> Target {
    match ids {
        [one] => Target::App((*one).to_string()),
        _ => Target::Updates,
    }
}

/// A finished job: the app's page, or the list when the app is gone.
pub fn finished_target(kind: JobKind, app_id: &str) -> Target {
    match kind {
        JobKind::Uninstall => Target::Installed,
        _ => Target::App(app_id.to_string()),
    }
}

/// A failed job: the app's page, or the Queue (where the failed job is) when the app was never
/// installed.
pub fn failed_target(kind: JobKind, app_id: &str) -> Target {
    match kind {
        JobKind::Install => Target::Queue,
        _ => Target::App(app_id.to_string()),
    }
}

/// Percent-encode everything but letters, digits and "-_.~", so the text survives a link, a
/// command line and `decodeURIComponent` in the window's router.
fn encode(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

fn decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = s.get(i + 1..i + 3).filter(|h| h.bytes().all(|c| c.is_ascii_hexdigit()))?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

// ------------------------------------------------------------ the toast

fn escape(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '&' => "&amp;".to_string(),
            '<' => "&lt;".to_string(),
            '>' => "&gt;".to_string(),
            '"' => "&quot;".to_string(),
            '\'' => "&apos;".to_string(),
            // Control characters are not allowed in XML at all.
            c if (c as u32) < 0x20 && !matches!(c, '\n' | '\t') => String::new(),
            c => c.to_string(),
        })
        .collect()
}

/// The sound element for a Windows sound name. "Default" is the system's own sound (no element);
/// an unknown name falls back to it, so a toast is never silent by accident.
fn sound_element(name: &str) -> Option<String> {
    ["IM", "Mail", "Reminder", "SMS"].contains(&name).then(|| format!("<audio src=\"ms-winsoundevent:Notification.{name}\"/>"))
}

/// The toast as Windows reads it. With a link, a click on the toast opens that link.
pub fn toast_xml(title: &str, body: &str, sound: &str, link: Option<&str>) -> String {
    let mut x = String::from("<toast");
    if let Some(l) = link {
        x.push_str(&format!(" launch=\"{}\" activationType=\"protocol\"", escape(l)));
    }
    x.push_str("><visual><binding template=\"ToastGeneric\">");
    x.push_str(&format!("<text>{}</text>", escape(title)));
    if !body.is_empty() {
        x.push_str(&format!("<text>{}</text>", escape(body)));
    }
    x.push_str("</binding></visual>");
    if let Some(a) = sound_element(sound) {
        x.push_str(&a);
    }
    x.push_str("</toast>");
    x
}

/// A build run from cargo's output folder (target\debug or target\release) is not installed:
/// it has no Start menu entry to show a toast under, and it must not take over the link handler.
fn is_dev_build(exe_dir: &str) -> bool {
    let d = exe_dir.trim_end_matches('\\');
    d.ends_with("\\target\\debug") || d.ends_with("\\target\\release")
}

fn app_user_model_id(identifier: &str, exe_dir: &str) -> String {
    if is_dev_build(exe_dir) { POWERSHELL_ID.to_string() } else { identifier.to_string() }
}

/// Show a notification. The page it opens when clicked is `target`.
pub fn show(app: &AppHandle, title: &str, body: &str, sound: &str, target: &Target) {
    let exe_dir = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.display().to_string())).unwrap_or_default();
    let id = app_user_model_id(&app.config().identifier, &exe_dir);
    let link = LINKS_READY.load(Ordering::SeqCst).then(|| target.link());
    let xml = toast_xml(title, body, sound, link.as_deref());
    let (app, title, body, sound) = (app.clone(), title.to_string(), body.to_string(), sound.to_string());
    tauri::async_runtime::spawn_blocking(move || {
        if let Err(e) = show_xml(&id, &xml) {
            // Still tell the user, just without the click.
            log::warn!("notification: {e}");
            use tauri_plugin_notification::NotificationExt;
            let _ = app.notification().builder().title(&title).body(&body).sound(&sound).show();
        }
    });
}

fn show_xml(app_id: &str, xml: &str) -> windows::core::Result<()> {
    use windows::{
        core::HSTRING,
        Data::Xml::Dom::XmlDocument,
        UI::Notifications::{ToastNotification, ToastNotificationManager},
    };
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(app_id))?.Show(&toast)
}

// ------------------------------------------------------------ the link handler

/// What the link handler is, as (key under HKCU\Software\Classes, value name, value). An empty
/// name is the key's default value.
fn handler_entries(exe: &str) -> Vec<(String, &'static str, String)> {
    handler_entries_for(SCHEME, exe)
}

fn handler_entries_for(scheme: &str, exe: &str) -> Vec<(String, &'static str, String)> {
    vec![
        (scheme.to_string(), "", "URL:The Updater 3D".to_string()),
        (scheme.to_string(), "URL Protocol", String::new()),
        (format!("{scheme}\\DefaultIcon"), "", format!("\"{exe}\",0")),
        (format!("{scheme}\\shell\\open\\command"), "", format!("\"{exe}\" \"%1\"")),
    ]
}

/// Make Windows open the links of the notifications with this exe. Called at every start (it
/// keeps the path right after an update or a move) and only writes when something differs. A
/// build run from cargo's output is left out. Without the handler, toasts carry no link.
pub fn register_link_handler() {
    let Ok(exe) = std::env::current_exe() else { return };
    if is_dev_build(&exe.parent().map(|p| p.display().to_string()).unwrap_or_default()) {
        return;
    }
    match write_handler(&handler_entries(&exe.display().to_string())) {
        Ok(()) => LINKS_READY.store(true, Ordering::SeqCst),
        Err(e) => log::warn!("link handler: {e}"),
    }
}

fn write_handler(entries: &[(String, &'static str, String)]) -> std::io::Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let (classes, _) = RegKey::predef(HKEY_CURRENT_USER).create_subkey("Software\\Classes")?;
    let same = entries.iter().all(|(key, name, value)| classes.open_subkey(key).and_then(|k| k.get_value::<String, _>(name)).is_ok_and(|v| &v == value));
    if same {
        return Ok(());
    }
    for (key, name, value) in entries {
        let (k, _) = classes.create_subkey(key)?;
        k.set_value(name, value)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str) -> Target {
        Target::App(id.to_string())
    }

    #[test]
    fn every_target_has_its_route_for_the_window() {
        assert_eq!(app("adobe:PHSP").route(), "installed/adobe%3APHSP");
        assert_eq!(Target::Updates.route(), "updates");
        assert_eq!(Target::Installed.route(), "installed");
        assert_eq!(Target::Queue.route(), "queue");
        assert_eq!(Target::Home.route(), "home");
        assert_eq!(Target::About.route(), "updater");
    }

    #[test]
    fn an_app_route_matches_what_the_router_decodes() {
        // The window builds app links with encodeURIComponent and reads them with decodeURIComponent.
        assert_eq!(app("other:VideoLAN.VLC").route(), "installed/other%3AVideoLAN.VLC");
        assert_eq!(app("driver:PCI\\VEN_10DE&DEV_2684 x/y").route(), "installed/driver%3APCI%5CVEN_10DE%26DEV_2684%20x%2Fy");
    }

    #[test]
    fn a_link_comes_back_as_the_same_target() {
        let all = [app("adobe:PHSP"), app("driver:PCI\\VEN_10DE&DEV_2684 x/y?z#w%"), app("tools:godot:4.7.2"), app("Ünïcode ✓"), Target::Updates, Target::Installed, Target::Queue, Target::Home, Target::About];
        for t in all {
            assert_eq!(Target::from_link(&t.link()), Some(t.clone()), "{}", t.link());
        }
    }

    #[test]
    fn links_are_read_the_way_windows_hands_them_over() {
        // A trailing slash, other letter case, spaces around, a query or a fragment.
        assert_eq!(Target::from_link("theupdater3d://queue/"), Some(Target::Queue));
        assert_eq!(Target::from_link("TheUpdater3D://Queue"), Some(Target::Queue));
        assert_eq!(Target::from_link("  theupdater3d://home  "), Some(Target::Home));
        assert_eq!(Target::from_link("theupdater3d:queue"), Some(Target::Queue));
        assert_eq!(Target::from_link("theupdater3d:///queue"), Some(Target::Queue));
        assert_eq!(Target::from_link("theupdater3d://about?x=1#y"), Some(Target::About));
        assert_eq!(Target::from_link("theupdater3d://app/blender/"), Some(app("blender")));
    }

    #[test]
    fn a_link_can_only_open_a_page() {
        for bad in [
            "",
            "theupdater3d",
            "theupdater3d:",
            "theupdater3d://",
            "theupdater3d://update-all",
            "theupdater3d://settings",
            "theupdater3d://queue/extra",
            "theupdater3d://app",
            "theupdater3d://app/",
            "theupdater3d://app/a/b",
            "theupdater3d://app/%",
            "theupdater3d://app/%zz",
            "theupdater3d://app/%+1",
            "theupdater3d://app/%FF",
            "theupdater3d://app/%00",
            "theupdater3d://app/a%0Ab",
            "other://queue",
            "xtheupdater3d://queue",
            "--minimized",
            "C:\\Program Files\\x.exe",
        ] {
            assert_eq!(Target::from_link(bad), None, "{bad}");
        }
        assert_eq!(Target::from_link(&format!("theupdater3d://app/{}", "a".repeat(600))), None);
    }

    #[test]
    fn a_start_with_a_link_asks_for_that_page() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(route_in_args(&args(&["the-updater-3d.exe", "theupdater3d://app/adobe%3APHSP"])).as_deref(), Some("installed/adobe%3APHSP"));
        assert_eq!(route_in_args(&args(&["the-updater-3d.exe", "--minimized", "theupdater3d://queue"])).as_deref(), Some("queue"));
        assert_eq!(route_in_args(&args(&["the-updater-3d.exe"])), None);
        assert_eq!(route_in_args(&args(&["the-updater-3d.exe", "--minimized"])), None);
        assert_eq!(route_in_args(&args(&["the-updater-3d.exe", "theupdater3d://nonsense"])), None);
    }

    /// Which notification opens which page.
    #[test]
    fn one_update_opens_its_app_and_several_open_the_updates() {
        assert_eq!(updates_target(&["adobe:PHSP"]), app("adobe:PHSP"));
        assert_eq!(updates_target(&["adobe:PHSP", "blender"]), Target::Updates);
        assert_eq!(updates_target(&["a", "b", "c", "d"]), Target::Updates);
        assert_eq!(updates_target(&[]), Target::Updates);
    }

    #[test]
    fn a_finished_job_opens_its_app_and_an_uninstall_opens_the_list() {
        for k in [JobKind::Update, JobKind::Install, JobKind::Rollback, JobKind::Repair] {
            assert_eq!(finished_target(k, "blender"), app("blender"));
        }
        assert_eq!(finished_target(JobKind::Uninstall, "blender"), Target::Installed);
    }

    #[test]
    fn a_failed_job_opens_its_app_or_the_queue_for_an_app_that_is_not_installed() {
        for k in [JobKind::Update, JobKind::Uninstall, JobKind::Rollback, JobKind::Repair] {
            assert_eq!(failed_target(k, "blender"), app("blender"));
        }
        assert_eq!(failed_target(JobKind::Install, "other:VideoLAN.VLC"), Target::Queue);
    }

    #[test]
    fn a_toast_with_a_link_opens_it_on_a_click() {
        let x = toast_xml("3 updates ready", "Blender, Krita and OBS Studio", "Default", Some("theupdater3d://updates"));
        let d = roxmltree::Document::parse(&x).unwrap();
        let toast = d.root_element();
        assert_eq!(toast.tag_name().name(), "toast");
        assert_eq!(toast.attribute("launch"), Some("theupdater3d://updates"));
        assert_eq!(toast.attribute("activationType"), Some("protocol"));
        let texts: Vec<&str> = d.descendants().filter(|n| n.has_tag_name("text")).filter_map(|n| n.text()).collect();
        assert_eq!(texts, ["3 updates ready", "Blender, Krita and OBS Studio"]);
        let binding = d.descendants().find(|n| n.has_tag_name("binding")).unwrap();
        assert_eq!(binding.attribute("template"), Some("ToastGeneric"));
        assert!(!d.descendants().any(|n| n.has_tag_name("audio")), "the default sound is the system's own");
    }

    #[test]
    fn a_toast_without_a_link_has_no_click_action() {
        let x = toast_xml("t", "b", "Default", None);
        let d = roxmltree::Document::parse(&x).unwrap();
        assert_eq!(d.root_element().attribute("launch"), None);
        assert_eq!(d.root_element().attribute("activationType"), None);
    }

    #[test]
    fn names_with_markup_stay_text() {
        let x = toast_xml("Updated A & B <1>", "say \"hi\" 'there'\u{1}", "Default", Some(&app("x\"y&z<").link()));
        let d = roxmltree::Document::parse(&x).unwrap();
        let texts: Vec<&str> = d.descendants().filter(|n| n.has_tag_name("text")).filter_map(|n| n.text()).collect();
        assert_eq!(texts, ["Updated A & B <1>", "say \"hi\" 'there'"]);
        assert_eq!(d.root_element().attribute("launch"), Some("theupdater3d://app/x%22y%26z%3C"));
    }

    #[test]
    fn a_toast_with_no_body_has_only_a_title() {
        let x = toast_xml("t", "", "Default", None);
        assert_eq!(roxmltree::Document::parse(&x).unwrap().descendants().filter(|n| n.has_tag_name("text")).count(), 1);
    }

    #[test]
    fn the_known_sounds_play_and_the_default_is_the_systems() {
        for name in ["IM", "Mail", "Reminder", "SMS"] {
            let x = toast_xml("t", "b", name, None);
            let d = roxmltree::Document::parse(&x).unwrap();
            let a = d.descendants().find(|n| n.has_tag_name("audio")).unwrap();
            assert_eq!(a.attribute("src"), Some(format!("ms-winsoundevent:Notification.{name}").as_str()));
        }
        // Not a silent toast: the sound element is left out and Windows plays its own.
        assert!(!toast_xml("t", "b", "Nonsense", None).contains("audio"));
    }

    /// Building the toast object is what Windows checks before showing; nothing is shown.
    #[test]
    fn windows_accepts_the_toast_we_build() {
        use windows::{core::HSTRING, Data::Xml::Dom::XmlDocument, UI::Notifications::ToastNotification};
        for x in [toast_xml("Photoshop 27.2 is ready", "Open the app", "Default", Some(&app("adobe:PHSP").link())), toast_xml("t", "", "Reminder", None)] {
            let doc = XmlDocument::new().unwrap();
            doc.LoadXml(&HSTRING::from(x.as_str())).unwrap();
            ToastNotification::CreateToastNotification(&doc).unwrap();
        }
    }

    #[test]
    fn an_installed_copy_shows_toasts_as_itself_and_a_cargo_build_as_powershell() {
        assert_eq!(app_user_model_id("app.theupdater3d", "C:\\Users\\Alex\\AppData\\Local\\The Updater 3D"), "app.theupdater3d");
        assert_eq!(app_user_model_id("app.theupdater3d", "D:\\cargo-target\\the-updater-3d\\release"), "app.theupdater3d");
        assert_eq!(app_user_model_id("app.theupdater3d", "C:\\src\\the-updater-3d\\src-tauri\\target\\debug"), POWERSHELL_ID);
        assert_eq!(app_user_model_id("app.theupdater3d", "C:\\src\\x\\target\\release\\"), POWERSHELL_ID);
        assert!(!is_dev_build("C:\\Users\\Alex\\AppData\\Local\\The Updater 3D"));
    }

    #[test]
    fn the_link_handler_starts_this_exe_with_the_link() {
        let exe = "C:\\Users\\Alex\\AppData\\Local\\The Updater 3D\\the-updater-3d.exe";
        let e = handler_entries(exe);
        let get = |key: &str, name: &str| e.iter().find(|(k, n, _)| k == key && *n == name).map(|(_, _, v)| v.as_str());
        assert_eq!(get("theupdater3d", "URL Protocol"), Some(""));
        assert_eq!(get("theupdater3d\\shell\\open\\command", ""), Some("\"C:\\Users\\Alex\\AppData\\Local\\The Updater 3D\\the-updater-3d.exe\" \"%1\""));
        // Only the scheme's own keys under Classes.
        assert!(e.iter().all(|(k, _, _)| k == "theupdater3d" || k.starts_with("theupdater3d\\")));
    }

    /// Writes a throwaway scheme (never the real one) under the user's classes and removes it.
    #[test]
    fn the_link_handler_is_written_and_kept_up_to_date_in_the_registry() {
        use winreg::{enums::HKEY_CURRENT_USER, RegKey};
        let scheme = format!("theupdater3d-unit-{}", std::process::id());
        let command = || {
            RegKey::predef(HKEY_CURRENT_USER)
                .open_subkey(format!("Software\\Classes\\{scheme}\\shell\\open\\command"))
                .and_then(|k| k.get_value::<String, _>(""))
        };
        let first = write_handler(&handler_entries_for(&scheme, "C:\\a b\\one.exe"));
        let after_first = command();
        // Written again as is (nothing to change), then for an exe that moved.
        let again = write_handler(&handler_entries_for(&scheme, "C:\\a b\\one.exe"));
        let moved = write_handler(&handler_entries_for(&scheme, "D:\\two.exe"));
        let after_move = command();
        let protocol = RegKey::predef(HKEY_CURRENT_USER).open_subkey(format!("Software\\Classes\\{scheme}")).and_then(|k| k.get_value::<String, _>("URL Protocol"));
        let _ = RegKey::predef(HKEY_CURRENT_USER).delete_subkey_all(format!("Software\\Classes\\{scheme}"));
        first.unwrap();
        again.unwrap();
        moved.unwrap();
        assert_eq!(after_first.unwrap(), "\"C:\\a b\\one.exe\" \"%1\"");
        assert_eq!(after_move.unwrap(), "\"D:\\two.exe\" \"%1\"");
        assert_eq!(protocol.unwrap(), "");
        assert!(command().is_err(), "the throwaway scheme was removed");
    }

    /// Every notification goes through this module, or its click would be lost.
    #[test]
    fn nothing_else_sends_a_notification() {
        for (name, src) in [("core.rs", include_str!("core.rs")), ("commands.rs", include_str!("commands.rs")), ("lib.rs", include_str!("lib.rs")), ("tray.rs", include_str!("tray.rs"))] {
            assert!(!src.contains(concat!(".notification", "()")), "{name} sends a notification itself");
        }
    }
}
