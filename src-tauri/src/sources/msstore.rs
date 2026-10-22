//! Apps installed from the Microsoft Store (MSIX packages) for this user. They are listed so the
//! copy shows up with its version, but the Store updates and removes them: nothing here changes
//! a Store package, it is only read.

use super::StoreRef;
use std::path::PathBuf;

/// One package from the Microsoft Store installed for this user.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Package {
    /// Identity name: "BlenderFoundation.Blender4.5LTS"
    pub name: String,
    /// Package family name: "BlenderFoundation.Blender4.5LTS_ppwjx1n5r4v9t"
    pub family: String,
    /// Package version, always four numbers: "4.5.3.0"
    pub version: String,
    pub install_dir: Option<PathBuf>,
    /// ISO date it was installed or last updated
    pub installed_at: Option<String>,
}

impl Package {
    /// The publisher part of the family name ("ppwjx1n5r4v9t"), which only the signer has.
    pub fn publisher_id(&self) -> &str {
        self.family.rsplit_once('_').map(|(_, p)| p).unwrap_or("")
    }
}

/// The first application of a package manifest (AppxManifest.xml): its id and program file.
pub fn manifest_app(xml: &str) -> Option<(String, String)> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    let app = doc.descendants().find(|n| n.tag_name().name() == "Application")?;
    Some((app.attribute("Id")?.to_string(), app.attribute("Executable").unwrap_or("").to_string()))
}

/// "4.5.3.0" shows as "4.5.3": the fourth number of a package version is only kept when it is set.
pub fn display_version(v: &str) -> String {
    let parts: Vec<&str> = v.split('.').collect();
    if parts.len() == 4 && parts[3] == "0" {
        parts[..3].join(".")
    } else {
        v.to_string()
    }
}

/// What a source lists for a Store package: its reference, program file and display version.
pub struct Copy {
    pub store: StoreRef,
    pub exe: Option<PathBuf>,
    pub version: String,
}

/// Read the package's manifest for its application and program file.
pub fn copy_of(p: &Package) -> Copy {
    let app = p.install_dir.as_ref().and_then(|d| std::fs::read_to_string(d.join("AppxManifest.xml")).ok()).and_then(|x| manifest_app(&x));
    let exe = match (&p.install_dir, &app) {
        (Some(d), Some((_, exe))) if !exe.is_empty() => Some(d.join(exe.replace('/', "\\"))),
        _ => None,
    };
    Copy { store: StoreRef { family: p.family.clone(), app: app.map(|a| a.0) }, exe, version: display_version(&p.version) }
}

/// Microsoft Store packages installed for this user (not frameworks, resources or sideloaded
/// packages). Empty when Windows can not list them.
pub fn packages() -> Vec<Package> {
    match read_packages() {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Store packages: {e}");
            vec![]
        }
    }
}

fn read_packages() -> windows::core::Result<Vec<Package>> {
    use windows::ApplicationModel::PackageSignatureKind;
    use windows::Management::Deployment::PackageManager;
    // Join the multithreaded apartment (and stay): without it the WinRT objects below live in
    // the implicit one, which another thread ending its own COM use can tear down mid-call.
    let _ = unsafe { windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED) };
    let pm = PackageManager::new()?;
    // An empty user id is the current user, which needs no admin rights.
    let list = pm.FindPackagesByUserSecurityId(&windows::core::HSTRING::new())?;
    let mut out = Vec::new();
    for p in list {
        let skip = p.IsFramework().unwrap_or(true) || p.IsResourcePackage().unwrap_or(false) || p.SignatureKind().map(|k| k != PackageSignatureKind::Store).unwrap_or(true);
        if skip {
            continue;
        }
        let Ok(id) = p.Id() else { continue };
        let (Ok(name), Ok(family), Ok(v)) = (id.Name(), id.FamilyName(), id.Version()) else { continue };
        let install_dir = p.InstalledPath().ok().map(|s| PathBuf::from(s.to_string())).filter(|d: &PathBuf| !d.as_os_str().is_empty());
        out.push(Package {
            name: name.to_string(),
            family: family.to_string(),
            version: format!("{}.{}.{}.{}", v.Major, v.Minor, v.Build, v.Revision),
            install_dir,
            installed_at: p.InstalledDate().ok().and_then(|d| filetime_iso(d.UniversalTime)),
        });
    }
    Ok(out)
}

/// A Windows date (100 ns steps since 1601) as an ISO date.
fn filetime_iso(t: i64) -> Option<String> {
    if t <= 0 {
        return None;
    }
    let secs = t / 10_000_000 - 11_644_473_600;
    chrono::DateTime::<chrono::Utc>::from_timestamp(secs, 0).map(|d| d.to_rfc3339())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_application_and_the_version() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="BlenderFoundation.Blender4.5LTS" Publisher="CN=X" Version="4.5.3.0" />
  <Applications>
    <Application Id="BLENDER" Executable="Blender/blender-launcher.exe" EntryPoint="Windows.FullTrustApplication" />
  </Applications>
</Package>"#;
        assert_eq!(manifest_app(xml), Some(("BLENDER".into(), "Blender/blender-launcher.exe".into())));
        assert_eq!(manifest_app("<Package/>"), None);
        assert_eq!(display_version("4.5.3.0"), "4.5.3");
        assert_eq!(display_version("5.2.9.1"), "5.2.9.1");
        assert_eq!(display_version("5.2"), "5.2");
        let p = Package { family: "49800KritaProject.Krita_n3kgb906j1zjg".into(), ..Default::default() };
        assert_eq!(p.publisher_id(), "n3kgb906j1zjg");
        assert_eq!(filetime_iso(133_000_000_000_000_000).as_deref(), Some("2022-06-18T04:26:40+00:00"));
        let s = StoreRef { family: "BlenderFoundation.Blender_ppwjx1n5r4v9t".into(), app: Some("BLENDER".into()) };
        assert_eq!(s.page_url(), "ms-windows-store://pdp/?PFN=BlenderFoundation.Blender_ppwjx1n5r4v9t");
        assert_eq!(s.launch_target().as_deref(), Some("shell:AppsFolder\\BlenderFoundation.Blender_ppwjx1n5r4v9t!BLENDER"));
    }
}
