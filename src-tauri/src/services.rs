//! Adobe background processes and Windows services (the Services screen).

use crate::model::ServiceInfo;
use crate::procs::{self, Proc};
use crate::sources::Step;
use crate::winutil;
use std::path::PathBuf;

struct Known {
    name: &'static str,
    display: &'static str,
    description: &'static str,
    service: Option<&'static str>,
    required: bool,
}

const KNOWN: &[Known] = &[
    Known { name: "Adobe Desktop Service.exe", display: "Adobe Desktop Service", description: "Sign-in, licensing and installs", service: None, required: true },
    Known { name: "AdobeIPCBroker.exe", display: "AdobeIPCBroker", description: "Connects Adobe apps to your sign-in", service: None, required: true },
    Known { name: "Creative Cloud.exe", display: "Creative Cloud", description: "The Creative Cloud window, not needed with this app", service: None, required: false },
    Known { name: "Creative Cloud UI Helper.exe", display: "Creative Cloud UI Helper", description: "Draws the Creative Cloud window", service: None, required: false },
    Known { name: "CoreSync.exe", display: "CoreSync", description: "Syncs Creative Cloud files", service: None, required: false },
    Known { name: "CCXProcess.exe", display: "CCXProcess", description: "Extensions and in-app content", service: None, required: false },
    Known { name: "CCLibrary.exe", display: "CCLibrary", description: "Syncs Libraries", service: None, required: false },
    Known { name: "Adobe CEF Helper.exe", display: "Adobe CEF Helper", description: "Web views inside Creative Cloud", service: None, required: false },
    Known { name: "AdobeNotificationClient.exe", display: "Adobe Notification Client", description: "Adobe pop-up notifications", service: None, required: false },
    Known { name: "AdobeCollabSync.exe", display: "AdobeCollabSync", description: "Acrobat shared reviews", service: None, required: false },
    Known { name: "AdobeUpdateService.exe", display: "AdobeUpdateService", description: "Adobe update checker, this app does its job", service: Some("AdobeUpdateService"), required: false },
    Known { name: "AGSService.exe", display: "AGSService", description: "Genuine software check", service: Some("AGSService"), required: false },
    Known { name: "AGMService.exe", display: "AGMService", description: "Genuine software monitor", service: Some("AGMService"), required: false },
    Known { name: "armsvc.exe", display: "AdobeARMservice", description: "Acrobat update service", service: Some("AdobeARMservice"), required: false },
];

fn adobe_dirs() -> Vec<PathBuf> {
    vec![
        crate::paths::program_files_x86().join("Adobe"),
        crate::paths::program_files_x86().join("Common Files\\Adobe"),
        crate::paths::program_files().join("Common Files\\Adobe"),
    ]
}

pub fn list() -> Vec<ServiceInfo> {
    let procs = procs::list();
    let mut out = Vec::new();
    for k in KNOWN {
        let mine: Vec<&Proc> = procs.iter().filter(|p| p.name.eq_ignore_ascii_case(k.name)).collect();
        let svc_state = k.service.and_then(winutil::service_running);
        if mine.is_empty() && svc_state.is_none() {
            continue; // not on this PC
        }
        let running = !mine.is_empty() || svc_state == Some(true);
        out.push(ServiceInfo {
            name: k.service.unwrap_or(k.name).to_string(),
            display_name: k.display.to_string(),
            description: k.description.to_string(),
            kind: if k.service.is_some() { "service".into() } else { "process".into() },
            running,
            cpu: mine.iter().map(|p| p.cpu).sum(),
            memory: mine.iter().map(|p| p.memory).sum(),
            required: k.required,
            safe_to_stop: !k.required,
        });
    }
    // Other Adobe background programs not in the list above.
    let dirs = adobe_dirs();
    let mut extra: std::collections::BTreeMap<String, (f32, u64)> = std::collections::BTreeMap::new();
    for p in procs::blocking(&procs, &dirs, &[]) {
        if KNOWN.iter().any(|k| k.name.eq_ignore_ascii_case(&p.name)) || p.name.eq_ignore_ascii_case("node.exe") {
            continue;
        }
        let e = extra.entry(p.name.clone()).or_default();
        e.0 += p.cpu;
        e.1 += p.memory;
    }
    for (name, (cpu, mem)) in extra {
        out.push(ServiceInfo {
            display_name: procs::friendly(&name),
            name,
            description: "Adobe background program".into(),
            kind: "process".into(),
            running: true,
            cpu,
            memory: mem,
            required: false,
            safe_to_stop: false,
        });
    }
    out
}

pub enum Action {
    /// Close processes in this process (no admin needed).
    Close(Vec<u32>),
    /// Needs admin rights.
    Elevated(Step),
    /// Start Creative Cloud.
    OpenCc,
}

pub fn stop(name: &str) -> anyhow::Result<Action> {
    if let Some(k) = KNOWN.iter().find(|k| k.service == Some(name) || k.name.eq_ignore_ascii_case(name)) {
        if k.required {
            anyhow::bail!("{} is needed for sign-in and licensing", k.display);
        }
        if let Some(svc) = k.service {
            return Ok(Action::Elevated(Step::Run {
                exe: "sc.exe".into(),
                args: vec!["stop".into(), svc.into()],
                cwd: None,
                ok_codes: vec![0, 1062],
                elevated: true,
                label: format!("Stopping {}", k.display),
                log: None,
            }));
        }
    }
    let pids: Vec<u32> = procs::list().into_iter().filter(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.pid).collect();
    Ok(Action::Close(pids))
}

pub fn start(name: &str) -> anyhow::Result<Action> {
    if let Some(k) = KNOWN.iter().find(|k| k.service == Some(name)) {
        return Ok(Action::Elevated(Step::Run {
            exe: "sc.exe".into(),
            args: vec!["start".into(), k.service.unwrap().into()],
            cwd: None,
            ok_codes: vec![0, 1056],
            elevated: true,
            label: format!("Starting {}", k.display),
            log: None,
        }));
    }
    Ok(Action::OpenCc)
}

pub fn safe_names() -> Vec<String> {
    list().into_iter().filter(|s| s.running && s.safe_to_stop).map(|s| s.name).collect()
}
