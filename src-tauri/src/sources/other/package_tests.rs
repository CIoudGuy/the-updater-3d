//! App packages (MSIX): matching by package family name, the installer that updates them, and
//! the plans that install, update and remove them.

use super::*;
use super::tests::{entry, inst, installed_from};
use crate::sources::msix::{AppPackage, Sig};
use crate::sources::winget::test_index;

const AFFINITY: &str = "Canva.Affinity_8a0j1tnjnt4a4";
const AFFINITY_FULL: &str = "Canva.Affinity_3.0.1.3808_x64__8a0j1tnjnt4a4";

fn package(full: &str, version: &str, sig: Sig, display: &str, publisher: &str) -> AppPackage {
    AppPackage {
        name: full.split('_').next().unwrap().into(),
        full_name: full.into(),
        family: msix::family_of(full),
        version: version.into(),
        install_dir: Some(PathBuf::from(format!("C:\\Program Files\\WindowsApps\\{full}"))),
        installed_at: Some(std::time::UNIX_EPOCH + Duration::from_secs(1_760_000_000)),
        signature: sig,
        display_name: display.into(),
        publisher: publisher.into(),
        ..Default::default()
    }
}

fn affinity() -> AppPackage {
    package(AFFINITY_FULL, "3.0.1.3808", Sig::Developer, "Affinity", "Canva")
}

/// An index with Affinity, a Store app, a family two packages claim and a fully handled app.
fn index() -> Index {
    let idx = test_index();
    let a = idx.add_test_package("Canva.Affinity", "Affinity", "3.3.1.4940", &[], &[], &[], "Canva");
    idx.add_test_pfn(a, AFFINITY);
    let t = idx.add_test_package("CharlesMilette.TranslucentTB", "TranslucentTB", "2026.2.0.0", &[], &[], &[], "Charles Milette");
    idx.add_test_pfn(t, "28017CharlesMilette.TranslucentTB_v826wp6bftszj");
    for id in ["Twin.One", "Twin.Two"] {
        let r = idx.add_test_package(id, "Twin", "1.0", &[], &[], &[], "Twin");
        idx.add_test_pfn(r, "Twin.App_abc");
    }
    let b = idx.add_test_package("BlenderFoundation.Blender", "Blender", "5.0", &[], &[], &[], "Blender Foundation");
    idx.add_test_pfn(b, "BlenderFoundation.Blender_ppwjx1n5r4v9t");
    let w = idx.add_test_package("Example.Classic", "Classic", "2.0", &["Classic"], &[], &[], "Example");
    idx.add_test_pfn(w, "Example.Classic_abc");
    idx
}

fn msix_installer(arch: &str, family: &str) -> Installer {
    Installer {
        arch: arch.into(),
        kind: "msix".into(),
        url: format!("https://affinity-update.serif.com/windows/3/studio/retail/Affinity-Affinity-Store-{arch}-4940-ce057d0a.msix"),
        sha256: "c598f40af2ec06a1ad7f4280fae1d68a5f2c16d10c7c649e48059370a158f98d".into(),
        family: Some(family.into()),
        signature_sha256: Some("9f91ad9c070d361348f5e07e349039245172a0bebdeec2937334f056c577b66e".into()),
        ..Default::default()
    }
}

/// The catalog's newest Affinity manifest (3.3.1.4940), parsed from its YAML.
fn affinity_manifest() -> Manifest {
    winget::parse_manifest(
        "Installers:
- Architecture: x64
  InstallerSha256: C598F40AF2EC06A1AD7F4280FAE1D68A5F2C16D10C7C649E48059370A158F98D
  InstallerUrl: https://affinity-update.serif.com/windows/3/studio/retail/Affinity-Affinity-Store-x64-4940-ce057d0adfd09febd5029b87f93128d8f0b3b603.msix
  SignatureSha256: 9F91AD9C070D361348F5E07E349039245172A0BEBDEEC2937334F056C577B66E
- Architecture: arm64
  InstallerSha256: BDC30EF7A78866A5DAECA1FE71F181DB8A9DE89AC4CF3B285E7D9595B2E318D9
  InstallerUrl: https://affinity-update.serif.com/windows/3/studio/retail/Affinity-Affinity-Store-arm64-4940-ce057d0adfd09febd5029b87f93128d8f0b3b603.msix
  SignatureSha256: 84A11FD9403B81CF1B8ED7D3A2A0EE89CA2AAD48E13B7B11111673365418301F
InstallerType: msix
ManifestType: merged
PackageFamilyName: Canva.Affinity_8a0j1tnjnt4a4
PackageIdentifier: Canva.Affinity
PackageName: Affinity
PackageVersion: 3.3.1.4940
Publisher: Canva
ShortDescription: Award-winning integrated graphic design, photo-editing and publishing solution.
UpgradeBehavior: install
",
    )
    .unwrap()
}

fn feed_with(idx: &Index, id: &str, m: Manifest) -> Feed {
    let p = idx.by_id(id).unwrap();
    Feed { pkgs: HashMap::from([(id.to_ascii_lowercase(), PkgFeed { pkg: p, versions: vec![], latest: Some(m) })]), popular: vec![], unfinished: false }
}

#[test]
fn the_catalog_manifest_names_the_package_family_and_signature() {
    let m = affinity_manifest();
    assert_eq!(m.installers.len(), 2);
    let x64 = &m.installers[0];
    assert_eq!((x64.kind.as_str(), x64.family.as_deref()), ("msix", Some(AFFINITY)));
    assert_eq!(x64.signature_sha256.as_deref(), Some("9f91ad9c070d361348f5e07e349039245172a0bebdeec2937334f056c577b66e"));
    assert_eq!(usable(x64, "x64"), Ok(()));
}

#[test]
fn app_packages_match_by_their_exact_family_name() {
    let idx = index();
    let pkgs = vec![
        affinity(),
        package("28017CharlesMilette.TranslucentTB_2026.2.0.0_x64__v826wp6bftszj", "2026.2.0.0", Sig::Store, "TranslucentTB", "Charles Milette"),
        // Two packages claim this family: nothing tells them apart.
        package("Twin.App_1.0.0.0_x64__abc", "1.0.0.0", Sig::Developer, "Twin", "Twin"),
        // Not in the catalog, or only a family name that is close.
        package("Some.Other_1.0.0.0_x64__zzz", "1.0.0.0", Sig::Developer, "Other", "Other"),
        package("Canva.Affinity_3.0.1.3808_x64__0000000000000", "3.0.1.3808", Sig::Developer, "Affinity", "Not Canva"),
        // A fully handled app (its Store copy is listed by the Blender source).
        package("BlenderFoundation.Blender_5.0.0.0_x64__ppwjx1n5r4v9t", "5.0.0.0", Sig::Store, "Blender", "Blender Foundation"),
    ];
    let ms = package_matches(&pkgs, &idx, &[], &[]);
    let got: Vec<(&str, How)> = ms.iter().map(|m| (m.pkg.id.as_str(), m.how)).collect();
    assert_eq!(got, vec![("Canva.Affinity", How::PackageFamily), ("CharlesMilette.TranslucentTB", How::PackageFamily)]);
    let e = &ms[0].entry;
    assert_eq!((e.key.as_str(), e.hive, e.display_name.as_str(), e.display_version.as_str(), e.publisher.as_str()), (AFFINITY_FULL, APPX, "Affinity", "3.0.1.3808", "Canva"));
    // The family name in the index is lower case; the package's is not.
    assert_eq!(idx.by_family("CANVA.AFFINITY_8A0J1TNJNT4A4").len(), 1);

    // A package the uninstall list already matched keeps that copy, and a package another source
    // lists by its Store family stays with that source.
    let classic = Match { entry: Default::default(), pkg: idx.by_id("Canva.Affinity").unwrap(), how: How::ProductCode, upgrade_code: None, package: None };
    assert!(package_matches(&[affinity()], &idx, &[classic], &[]).is_empty());
    let mut store_copy = installed_from(Default::default(), "Example.Classic");
    store_copy.source = Source::Tools;
    store_copy.store = Some(StoreRef { family: "Example.Classic_abc".into(), app: None });
    let classic_pkg = package("Example.Classic_2.0.0.0_x64__abc", "2.0.0.0", Sig::Store, "Classic", "Example");
    assert!(package_matches(&[classic_pkg.clone()], &idx, &[], &[store_copy]).is_empty());
    assert_eq!(package_matches(&[classic_pkg], &idx, &[], &[]).len(), 1);
}

#[test]
fn a_sideloaded_package_is_updated_here_and_a_store_one_by_the_store() {
    let idx = index();
    let pkgs = vec![affinity(), package("28017CharlesMilette.TranslucentTB_2026.2.0.0_x64__v826wp6bftszj", "2026.2.0.0", Sig::Store, "TranslucentTB", "Charles Milette")];
    let ms = package_matches(&pkgs, &idx, &[], &[]);
    let list = installed_copies(&ms, None);
    assert_eq!(list.len(), 2);
    let a = &list[0];
    assert_eq!((a.id.as_str(), a.version.as_str(), a.read_only), ("other:Canva.Affinity", "3.0.1.3808", false));
    assert_eq!(a.install_dir, Some(PathBuf::from(format!("C:\\Program Files\\WindowsApps\\{AFFINITY_FULL}"))));
    assert_eq!(a.store.as_ref().map(|s| s.family.as_str()), Some(AFFINITY));
    assert_eq!(a.updated_at.as_deref(), Some("2025-10-09T08:53:20Z"));
    assert_eq!(package_family(a).as_deref(), Some(AFFINITY));
    assert!(a.uninstall.is_none());
    let w = Want::for_installed(a);
    assert_eq!((w.package.as_deref(), w.arch, w.scope), (Some(AFFINITY), Some("x64"), Some("user")));
    let t = &list[1];
    assert!(t.read_only);
    assert_eq!(t.tagline.as_deref(), Some("Updated by the Microsoft Store"));

    // Once the catalog is read: Affinity is offered its newest package version.
    let feed = feed_with(&idx, "Canva.Affinity", affinity_manifest());
    let list = installed_copies(&ms, Some(&feed));
    let a = list.iter().find(|i| i.id == "other:Canva.Affinity").unwrap();
    assert_eq!(offered_update(a, &feed).as_deref(), Some("3.3.1.4940"));
    assert_eq!(update_urls(&list, &feed).len(), 1);
    let mut current = a.clone();
    current.version = "3.3.1.4940".into();
    assert_eq!(offered_update(&current, &feed), None);

    // A sideloaded package the catalog only has a normal installer for is not listed (it can
    // not be updated here); a Store one still is.
    let mut exe = inst("x64", "exe", None);
    exe.silent = Some("/S".into());
    let feed = feed_with(&idx, "Canva.Affinity", Manifest { id: "Canva.Affinity".into(), version: "3.3.1.4940".into(), installers: vec![exe.clone()], ..Default::default() });
    assert!(installed_copies(&ms, Some(&feed)).iter().all(|i| i.id != "other:Canva.Affinity"));
    let feed = feed_with(&idx, "CharlesMilette.TranslucentTB", Manifest { id: "CharlesMilette.TranslucentTB".into(), version: "2026.2.0.0".into(), installers: vec![exe], ..Default::default() });
    assert!(installed_copies(&ms, Some(&feed)).iter().any(|i| i.id == "other:CharlesMilette.TranslucentTB"));
}

#[test]
fn only_a_package_of_the_same_family_updates_an_app_package() {
    let w = Want::for_package(AFFINITY_FULL);
    let mut exe = inst("x64", "exe", None);
    exe.silent = Some("/S".into());
    let mine = msix_installer("x64", AFFINITY);
    let arm = msix_installer("arm64", AFFINITY);
    let theirs = msix_installer("x64", "Canva.Affinity_0000000000000");
    let m = |v: Vec<Installer>| Manifest { id: "Canva.Affinity".into(), version: "3.3.1.4940".into(), installers: v, ..Default::default() };
    assert_eq!(choose(&m(vec![exe.clone(), arm.clone(), mine.clone()]), &w).unwrap().url, mine.url);
    assert_eq!(choose(&m(vec![exe.clone()]), &w).unwrap_err(), "no MSIX package, this copy is an app package");
    assert_eq!(choose(&m(vec![theirs]), &w).unwrap_err(), "only an MSIX package of another family");
    // A copy from a normal installer is never updated by a package (a second copy).
    let classic = Want::for_entry(&entry("Affinity", "HKLM64", "Affinity", "3.0", "Canva"), None);
    assert_eq!(choose(&m(vec![mine.clone()]), &classic).unwrap_err(), "only an MSIX package, this copy did not come from one");
    assert_eq!(choose(&m(vec![mine.clone(), exe.clone()]), &classic).unwrap().kind, "exe");
    // A new install takes the package for this processor.
    let fresh = Want { machine: "x64", ..Want::fresh() };
    assert_eq!(choose(&m(vec![arm, mine.clone()]), &fresh).unwrap().url, mine.url);
    // A Store-only entry stays out.
    let store = Installer { kind: "msstore".into(), ..inst("x64", "msstore", None) };
    assert_eq!(choose(&m(vec![store]), &fresh).unwrap_err(), STORE_ONLY);
}

#[test]
fn package_plans_install_update_and_remove_through_windows() {
    let idx = index();
    let p = idx.by_id("Canva.Affinity").unwrap();
    let man = affinity_manifest();
    let i = &man.installers[0];
    // New install: downloaded with its checksum, then added for this user without admin rights.
    let plan = build_plan(&p, &man, i, JobKind::Install, None, Some(1 << 30)).unwrap();
    assert_eq!(plan.app_id, "other:Canva.Affinity");
    assert_eq!(plan.downloads.len(), 1);
    let d = &plan.downloads[0];
    assert_eq!(d.sha256.as_deref(), Some("c598f40af2ec06a1ad7f4280fae1d68a5f2c16d10c7c649e48059370a158f98d"));
    assert!(d.dest.to_string_lossy().ends_with("Affinity-Affinity-Store-x64-4940-ce057d0adfd09febd5029b87f93128d8f0b3b603.msix"), "{}", d.dest.display());
    match &plan.steps[..] {
        [Step::AddPackage { path, family, signature_sha256, update: false, any_version: false, .. }] => {
            assert_eq!(path, &d.dest);
            assert_eq!(family, AFFINITY);
            assert_eq!(signature_sha256.as_deref(), Some("9f91ad9c070d361348f5e07e349039245172a0bebdeec2937334f056c577b66e"));
        }
        s => panic!("{s:?}"),
    }
    assert!(plan.steps.iter().all(|s| !s.elevated()));
    assert!(matches!(&plan.verify, Verify::Version { app_id, version } if app_id == "other:Canva.Affinity" && version == "3.3.1.4940"));
    assert!(plan.fallback.is_none() && plan.cleanup == vec![d.dest.clone()]);

    // Update of the installed copy: the app is closed first, then Windows replaces the package.
    let ms = package_matches(&[affinity()], &idx, &[], &[]);
    let cur = installed_copies(&ms, None).remove(0);
    let plan = build_plan(&p, &man, i, JobKind::Update, Some(&cur), None).unwrap();
    assert!(matches!(&plan.steps[..], [Step::AddPackage { update: true, any_version: false, .. }]));
    assert_eq!(plan.block_dirs, vec![PathBuf::from(format!("C:\\Program Files\\WindowsApps\\{AFFINITY_FULL}"))]);
    let back = build_plan(&p, &man, i, JobKind::Rollback, Some(&cur), None).unwrap();
    assert!(matches!(&back.steps[..], [Step::AddPackage { update: true, any_version: true, .. }]));

    // Uninstall: Windows removes the package of that family.
    let un = plan_uninstall(&cur).unwrap();
    assert!(matches!(&un.steps[..], [Step::RemovePackage { family, .. }] if family == AFFINITY));
    assert!(matches!(&un.verify, Verify::Removed { app_id } if app_id == "other:Canva.Affinity"));
    assert_eq!(un.block_dirs, plan.block_dirs);
}

#[test]
fn package_downloads_keep_an_extension_windows_reads() {
    let mut i = msix_installer("x64", AFFINITY);
    i.url = "https://example.com/download?id=7".into();
    assert_eq!(file_name(&i, "Canva.Affinity", "3.3.1.4940"), "Canva.Affinity-3.3.1.4940.msix");
    i.url = "https://example.com/Terminal_1.25.msixbundle".into();
    assert_eq!(file_name(&i, "X", "1"), "Terminal_1.25.msixbundle");
    i.kind = "appx".into();
    i.url = "https://example.com/a".into();
    assert_eq!(file_name(&i, "X", "1"), "X-1.appx");
}
