use super::*;
use std::io::Write;

fn pkg(family: &str, sig: Sig) -> AppPackage {
    AppPackage { name: family.split('_').next().unwrap().into(), family: family.into(), signature: sig, ..Default::default() }
}

#[test]
fn publisher_ids_are_the_ones_windows_gives() {
    assert_eq!(publisher_hash("CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US"), "8wekyb3d8bbwe");
    assert_eq!(publisher_hash("CN=Canva, O=Canva, S=New South Wales, C=AU"), "8a0j1tnjnt4a4");
    assert_eq!(publisher_hash("CN=04797BBC-C7BB-462F-9B66-331C81E27C0E"), "v826wp6bftszj");
    let id = Identity { name: "Canva.Affinity".into(), publisher: "CN=Canva, O=Canva, S=New South Wales, C=AU".into(), version: "3.3.1.4940".into(), bundle: false };
    assert_eq!(id.family(), "Canva.Affinity_8a0j1tnjnt4a4");
}

#[test]
fn full_names_give_family_and_processor() {
    let full = "Canva.Affinity_3.3.1.4940_x64__8a0j1tnjnt4a4";
    assert_eq!(family_of(full), "Canva.Affinity_8a0j1tnjnt4a4");
    assert_eq!(arch_of(full), "x64");
    assert_eq!(publisher_id(full), "8a0j1tnjnt4a4");
    assert_eq!(arch_of("Microsoft.WindowsTerminal_1.24.12741.0_neutral_~_8wekyb3d8bbwe"), "neutral");
    assert_eq!(family_of("Odd"), "Odd");
    assert_eq!(arch_of("Odd"), "");
}

#[test]
fn only_apps_the_user_installed_are_listed() {
    assert!(listed(&pkg("Canva.Affinity_8a0j1tnjnt4a4", Sig::Developer)));
    assert!(listed(&pkg("Contoso.Tool_abc", Sig::Enterprise)));
    // Store apps are listed (the Store updates them), Windows' own Store-signed parts are not.
    assert!(listed(&pkg("28017CharlesMilette.TranslucentTB_v826wp6bftszj", Sig::Store)));
    assert!(!listed(&pkg("Microsoft.WindowsTerminal_8wekyb3d8bbwe", Sig::Store)));
    assert!(!listed(&pkg("Microsoft.Windows.Photos_8wekyb3d8bbwe", Sig::System)));
    assert!(!listed(&pkg("Unsigned.Thing_x", Sig::None)));
    let mut f = pkg("Microsoft.VCLibs.140.00_8wekyb3d8bbwe", Sig::Store);
    f.framework = true;
    assert!(!listed(&f));
    let mut r = pkg("Canva.Affinity_8a0j1tnjnt4a4", Sig::Developer);
    r.resource = true;
    assert!(!listed(&r));
    let mut d = pkg("Me.MyApp_x", Sig::Developer);
    d.dev_mode = true;
    assert!(!listed(&d));
}

const MANIFEST: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10" xmlns:uap="http://schemas.microsoft.com/appx/manifest/uap/windows10">
  <Identity Name="Canva.Affinity" Publisher="CN=Canva, O=Canva, S=New South Wales, C=AU" Version="3.3.1.4940" ProcessorArchitecture="x64" />
  <Properties><DisplayName>Affinity</DisplayName><Logo>Assets\StoreLogo.png</Logo></Properties>
  <Applications>
    <Application Id="Affinity" Executable="App\Affinity.exe" EntryPoint="Windows.FullTrustApplication">
      <uap:VisualElements DisplayName="Affinity" Square150x150Logo="Assets\Square150x150Logo.png" Square44x44Logo="Assets\Square44x44Logo.png" Description="x" BackgroundColor="transparent" />
    </Application>
  </Applications>
</Package>"#;

const BUNDLE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<Bundle xmlns="http://schemas.microsoft.com/appx/2013/bundle" SchemaVersion="5.0">
  <Identity Name="Microsoft.WindowsTerminal" Publisher="CN=Microsoft Corporation, O=Microsoft Corporation, L=Redmond, S=Washington, C=US" Version="1.25.2733.0" />
  <Packages><Package Type="application" Version="1.25.2733.0" Architecture="x64" FileName="x64.msix" /></Packages>
</Bundle>"#;

#[test]
fn reads_identities_and_the_app_of_manifests() {
    let id = identity_of(MANIFEST).unwrap();
    assert_eq!((id.name.as_str(), id.version.as_str(), id.bundle), ("Canva.Affinity", "3.3.1.4940", false));
    let b = identity_of(BUNDLE).unwrap();
    assert!(b.bundle);
    assert_eq!(b.family(), "Microsoft.WindowsTerminal_8wekyb3d8bbwe");
    assert_eq!(identity_of("<Package/>"), None);
    let v = visual_of(MANIFEST);
    assert_eq!(v.app_id.as_deref(), Some("Affinity"));
    assert_eq!(v.exe.as_deref(), Some("App\\Affinity.exe"));
    assert_eq!(v.logos, vec!["Assets\\Square44x44Logo.png", "Assets\\Square150x150Logo.png", "Assets\\StoreLogo.png"]);
    assert_eq!(package_ext(&b, Path::new("x.msix")), "msixbundle");
    assert_eq!(package_ext(&b, Path::new("x.appxbundle")), "appxbundle");
    assert_eq!(package_ext(&id, Path::new("x.exe")), "msix");
    assert_eq!(package_ext(&id, Path::new("x.appx")), "appx");
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("updater3d-msix-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn write_package(path: &Path, files: &[(&str, &[u8])]) {
    let mut z = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for (n, b) in files {
        z.start_file(*n, zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated)).unwrap();
        z.write_all(b).unwrap();
    }
    z.finish().unwrap();
}

#[test]
fn a_downloaded_package_must_be_the_family_and_signature_the_catalog_names() {
    let d = temp("check");
    let file = d.join("Affinity.msix");
    write_package(&file, &[("AppxManifest.xml", MANIFEST.as_bytes()), ("AppxSignature.p7x", b"PKCX signature bytes")]);
    let (id, sig) = read_package_file(&file).unwrap();
    assert_eq!(id.family(), "Canva.Affinity_8a0j1tnjnt4a4");
    let want = hex::encode(Sha256::digest(b"PKCX signature bytes"));
    assert_eq!(sig.as_deref(), Some(want.as_str()));
    check_identity(&id, sig.as_deref(), "canva.affinity_8a0j1tnjnt4a4", Some(&want.to_uppercase())).unwrap();
    check_identity(&id, sig.as_deref(), "Canva.Affinity_8a0j1tnjnt4a4", None).unwrap();
    // Another publisher's package of the same name, or another signature: refused.
    assert!(check_identity(&id, sig.as_deref(), "Canva.Affinity_0000000000000", None).is_err());
    assert!(check_identity(&id, sig.as_deref(), "Canva.Affinity_8a0j1tnjnt4a4", Some(&"0".repeat(64))).is_err());
    // An unsigned package when the catalog gives a signature checksum: refused.
    let bare = d.join("bare.msix");
    write_package(&bare, &[("AppxManifest.xml", MANIFEST.as_bytes())]);
    let (id2, sig2) = read_package_file(&bare).unwrap();
    assert_eq!(sig2, None);
    assert!(check_identity(&id2, None, "Canva.Affinity_8a0j1tnjnt4a4", Some(&want)).is_err());
    // A bundle is read from its bundle manifest.
    let bundle = d.join("t.msixbundle");
    write_package(&bundle, &[("AppxMetadata/AppxBundleManifest.xml", BUNDLE.as_bytes()), ("x64.msix", b"inner"), ("AppxSignature.p7x", b"s")]);
    let (b, _) = read_package_file(&bundle).unwrap();
    assert!(b.bundle && b.family() == "Microsoft.WindowsTerminal_8wekyb3d8bbwe");
    // Not a package at all.
    let junk = d.join("junk.msix");
    std::fs::write(&junk, b"MZ not a zip").unwrap();
    assert!(read_package_file(&junk).is_err());
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn picks_the_largest_plain_logo() {
    let d = temp("logo");
    let assets = d.join("Assets");
    std::fs::create_dir_all(&assets).unwrap();
    for n in [
        "Square44x44Logo.scale-100.png",
        "Square44x44Logo.scale-200.png",
        "Square44x44Logo.targetsize-48.png",
        "Square44x44Logo.targetsize-256.png",
        "Square44x44Logo.targetsize-256_altform-unplated.png",
        "Square44x44Logo.targetsize-256_altform-lightunplated.png",
        "Square44x44Logo.targetsize-256_contrast-black.png",
        "StoreLogo.png",
        "Other.scale-400.png",
    ] {
        std::fs::write(assets.join(n), b"png").unwrap();
    }
    let got = logo_file(&d, "Assets\\Square44x44Logo.png").unwrap();
    assert_eq!(got.file_name().unwrap(), "Square44x44Logo.targetsize-256_altform-unplated.png");
    assert_eq!(logo_file(&d, "Assets\\StoreLogo.png").unwrap().file_name().unwrap(), "StoreLogo.png");
    assert_eq!(logo_file(&d, "Assets\\Missing.png"), None);
    std::fs::write(d.join("AppxManifest.xml"), MANIFEST).unwrap();
    let (app, exe, logo) = read_visual(&d);
    assert_eq!(app.as_deref(), Some("Affinity"));
    assert_eq!(exe, Some(d.join("App\\Affinity.exe")));
    assert_eq!(logo.unwrap().file_name().unwrap(), "Square44x44Logo.targetsize-256_altform-unplated.png");
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn the_package_list_is_kept_while_the_package_names_stay_the_same() {
    let t0 = std::time::Instant::now();
    let held: Held = Some((7, t0, vec![pkg("Canva.Affinity_8a0j1tnjnt4a4", Sig::Developer)]));
    // Same names a moment later: the kept list.
    assert_eq!(still_current(&held, Some(7), t0 + std::time::Duration::from_secs(5)).map(|v| v.len()), Some(1));
    // A package added, updated or removed: asked again.
    assert!(still_current(&held, Some(8), t0 + std::time::Duration::from_secs(5)).is_none());
    // The names could not be read: asked again.
    assert!(still_current(&held, None, t0).is_none());
    // Kept only for a while, whatever the names.
    assert!(still_current(&held, Some(7), t0 + HELD_FOR).is_none());
    assert!(still_current(&None, Some(7), t0).is_none());
}
