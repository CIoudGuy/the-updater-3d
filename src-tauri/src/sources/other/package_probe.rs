//! A live, read-only look at this user's app packages (MSIX) the catalog matches, and the plans
//! that would update and remove Affinity (built, never run). Run by hand:
//! `cargo test --release --lib live_app_packages_probe -- --ignored --nocapture`

use super::*;
use sha2::{Digest, Sha256};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn live_app_packages_probe() {
    let pkgs = tokio::task::spawn_blocking(msix::this_users_packages).await.unwrap();
    println!("{} app packages listed for this user", pkgs.len());
    let path = winget::index_path();
    let idx = Index::open(&path).expect("the winget index; start the app once to download it");
    let ms = package_matches(&pkgs, &idx, &[], &[]);
    println!("{} matched to the catalog by package family name:", ms.len());
    let mut feed = Feed::default();
    for m in &ms {
        let p = m.package.as_ref().unwrap();
        let (app, exe, logo) = p.install_dir.as_deref().map(msix::read_visual).unwrap_or_default();
        println!(
            "  {} <- {} {} ({:?}) app={:?} exe={} logo={}",
            m.pkg.id,
            p.family,
            p.version,
            p.signature,
            app,
            exe.as_ref().is_some_and(|e| e.is_file()),
            logo.as_ref().map(|l| l.file_name().unwrap().to_string_lossy().to_string()).unwrap_or_default()
        );
        let (pk, r) = load_pkg(m.pkg.clone()).await;
        match r {
            Ok((versions, man)) => {
                feed.pkgs.insert(pk.id.to_ascii_lowercase(), PkgFeed { pkg: pk, versions, latest: Some(man) });
            }
            Err(e) => println!("    catalog: {e:#}"),
        }
    }
    let list = installed_copies(&ms, Some(&feed));
    for i in &list {
        println!(
            "  listed {} {} read_only={} dir={} date={:?} open={:?} latest={:?} offered={:?}",
            i.id,
            i.version,
            i.read_only,
            i.install_dir.as_ref().map(|d| d.display().to_string()).unwrap_or_default(),
            i.updated_at,
            i.store.as_ref().and_then(|s| s.launch_target()),
            latest(i, &feed).map(|l| l.version),
            offered_update(i, &feed)
        );
    }
    for m in &ms {
        if !list.iter().any(|i| i.id == format!("other:{}", m.pkg.id)) {
            let why = feed.get(&m.pkg.id).and_then(|pf| pf.latest.as_ref()).map(|man| choose(man, &Want::for_package(&m.entry.key)).err().unwrap_or_default());
            println!("  not listed {}: {:?}", m.pkg.id, why);
        }
    }

    let Some(aff) = list.iter().find(|i| i.id == "other:Canva.Affinity") else {
        println!("Affinity is not installed as a package here");
        return;
    };
    // The installed package's own files: its family from its manifest, and its signature file
    // against the catalog's checksum for this version.
    let dir = aff.install_dir.clone().unwrap();
    let xml = std::fs::read_to_string(dir.join("AppxManifest.xml")).unwrap();
    let id = msix::identity_of(&xml).unwrap();
    println!("Affinity manifest: {} {} -> family {}", id.name, id.version, id.family());
    assert!(id.family().eq_ignore_ascii_case(AFFINITY_FAMILY));
    let sig = std::fs::read(dir.join("AppxSignature.p7x")).ok().map(|b| hex::encode(Sha256::digest(&b)));
    let pf = feed.get("Canva.Affinity").unwrap();
    let r = pf.versions.iter().find(|r| version::cmp(&r.version, &aff.version).is_eq()).cloned();
    if let Some(r) = r {
        let m = winget::manifest(&r).await.unwrap();
        let want = choose(&m, &Want::for_installed(aff)).unwrap().signature_sha256.clone();
        println!("signature file {:?}, catalog {:?}", sig, want);
        msix::check_identity(&id, sig.as_deref(), AFFINITY_FAMILY, want.as_deref()).unwrap();
        println!("installed package passes the identity and signature check");
    }

    // The plans, built and never run.
    for kind in [JobKind::Update, JobKind::Repair] {
        let plan = plan_install(&aff.id, None, kind, Some(aff), Some(&feed)).await.unwrap();
        println!("{kind:?} plan: {} -> {} ({:?})", plan.app_id, plan.to_version, plan.kind);
        for d in &plan.downloads {
            println!("  download {} size={:?} sha256={:?}", d.url, d.size, d.sha256);
        }
        for s in &plan.steps {
            println!("  step {s:?}");
        }
        println!("  close first: {:?}; verify: {:?}", plan.block_dirs, plan.verify);
    }
    let un = plan_uninstall(aff).unwrap();
    println!("Uninstall plan: {:?}; close first: {:?}", un.steps, un.block_dirs);
}

const AFFINITY_FAMILY: &str = "Canva.Affinity_8a0j1tnjnt4a4";
