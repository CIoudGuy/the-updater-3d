//! "Also delete settings and add-ons" on uninstall: where each app keeps them.
//!
//! Only per-user settings, caches and add-ons are listed. Never the user's own work: Documents,
//! Godot's signing keystores and the save data of games made with it, Substance assets. Folders
//! that every installed version shares go only with the last version.

use crate::sources::{Installed, SourceData, Step};
use crate::version;
use std::path::{Path, PathBuf};

/// Where per-user data lives (tests point these at a temp folder).
pub struct Roots {
    pub roaming: PathBuf,
    pub local: PathBuf,
    pub program_data: PathBuf,
}

impl Roots {
    pub fn system() -> Roots {
        Roots {
            roaming: crate::paths::app_data(),
            local: crate::paths::local_app_data(),
            program_data: PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())),
        }
    }
}

/// Settings and add-ons of `inst` that exist on disk, as remove steps. `others` are the other
/// installed apps (to keep what another version still uses).
pub fn steps(inst: &Installed, others: &[Installed], r: &Roots) -> Vec<Step> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    let mut files: Vec<PathBuf> = Vec::new();
    let mut elevated_dirs: Vec<PathBuf> = Vec::new();
    let mut reg_keys: Vec<String> = Vec::new();
    let same_tool = |key: &str| others.iter().any(|o| matches!(&o.data, SourceData::Tools { key: k, .. } if k == key));
    match &inst.data {
        SourceData::Adobe { sap, .. } => {
            // Creative Cloud is never uninstalled here; its data is shared by every Adobe app.
            if sap != "KCCC" {
                if let Some(name) = inst.install_dir.as_deref().and_then(Path::file_name) {
                    dirs.push(r.roaming.join("Adobe").join(name));
                    dirs.push(r.local.join("Adobe").join(name));
                }
            }
        }
        SourceData::Craft { key, .. } => {
            let squashed: String = inst.name.chars().filter(|c| !c.is_whitespace()).collect();
            for base in [&r.roaming, &r.local] {
                dirs.push(base.join(&inst.name));
                dirs.push(base.join(&squashed));
                dirs.push(base.join(format!("ai.storyteller.{key}")));
            }
        }
        SourceData::Blender { series, .. } => {
            // Settings and add-ons are per series; a beta copy of a series shares them with the
            // release of that series, so they stay while either is installed.
            let shared = others.iter().any(|o| matches!(&o.data, SourceData::Blender { series: s, .. } if s == series));
            if !shared {
                dirs.push(r.roaming.join("Blender Foundation").join("Blender").join(series));
                dirs.push(r.local.join("Blender Foundation").join("Blender").join(series));
            }
        }
        SourceData::Tools { key, series } => match key.as_str() {
            "obs" => {
                dirs.push(r.roaming.join("obs-studio"));
                elevated_dirs.push(r.program_data.join("obs-studio"));
            }
            "krita" => {
                dirs.push(r.roaming.join("krita"));
                dirs.push(r.local.join("krita"));
                files.push(r.local.join("kritarc"));
                files.push(r.local.join("kritadisplayrc"));
            }
            "freecad" => {
                // FreeCAD 1.1 and later keep each version apart ("v1-1"); older ones share one folder.
                let p = version::parts(series);
                let own = r.roaming.join("FreeCAD").join(format!("v{}-{}", p.first().unwrap_or(&0), p.get(1).unwrap_or(&0)));
                if own.exists() {
                    dirs.push(own);
                    dirs.push(r.local.join("FreeCAD").join(format!("v{}-{}", p.first().unwrap_or(&0), p.get(1).unwrap_or(&0))));
                } else if !same_tool("freecad") {
                    dirs.push(r.roaming.join("FreeCAD"));
                    dirs.push(r.local.join("FreeCAD"));
                }
            }
            "godot" => {
                let g = r.roaming.join("Godot");
                let ver = &inst.version;
                files.push(g.join(format!("editor_settings-{}.tres", version::trim_like(ver, "1.2"))));
                if let Ok(rd) = std::fs::read_dir(g.join("export_templates")) {
                    for e in rd.flatten() {
                        let n = e.file_name().to_string_lossy().to_string();
                        if n.split('.').take(3).collect::<Vec<_>>().join(".") == *ver || n.starts_with(&format!("{ver}.")) {
                            dirs.push(e.path());
                        }
                    }
                }
                if !same_tool("godot") {
                    for d in ["feature_profiles", "script_templates", "text_editor_themes", "shader_cache"] {
                        dirs.push(g.join(d));
                    }
                    files.push(g.join("projects.cfg"));
                    dirs.push(r.local.join("Godot"));
                }
            }
            "unityhub" => {
                dirs.push(r.roaming.join("UnityHub"));
            }
            "unity" => {
                // Editor preferences and the Asset Store download cache are shared by all editors.
                if !same_tool("unity") {
                    reg_keys.push("Software\\Unity Technologies\\Unity Editor 5.x".into());
                    dirs.push(r.roaming.join("Unity").join("Editor-5.x"));
                    dirs.push(r.roaming.join("Unity").join("Asset Store-5.x"));
                    dirs.push(r.local.join("Unity").join("Editor"));
                }
            }
            _ => {}
        },
    }
    // Never the install folder itself (the uninstall handles that) and nothing that is missing.
    let not_install = |p: &PathBuf| inst.install_dir.as_deref() != Some(p.as_path());
    let mut out = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for d in dirs.into_iter().filter(|d| d.is_dir() && not_install(d)) {
        if seen.iter().any(|s| s.as_os_str().eq_ignore_ascii_case(d.as_os_str())) {
            continue;
        }
        seen.push(d.clone());
        out.push(Step::RemoveDir { path: d, elevated: false, label: format!("Deleting {} settings", inst.name) });
    }
    for d in elevated_dirs.into_iter().filter(|d| d.is_dir()) {
        out.push(Step::RemoveDir { path: d, elevated: true, label: format!("Deleting {} add-ons", inst.name) });
    }
    for f in files.into_iter().filter(|f| f.is_file()) {
        out.push(Step::RemoveFile { path: f });
    }
    for k in reg_keys {
        out.push(Step::RemoveUserRegKey { path: k, label: format!("Deleting {} settings", inst.name) });
    }
    out
}

#[cfg(test)]
#[path = "prefs_tests.rs"]
mod tests;
