//! Live timing of the Other apps icon search for one screenful of Get apps. Not run by default:
//! PROBE_LETTER=K cargo test --release --test icon_probe -- --ignored --nocapture

use futures_util::StreamExt;
use std::collections::HashSet;
use std::time::Instant;
use updater_lib::sources::{browse, icons, other};

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn time_until_a_screenful_of_icons_is_found() {
    let letter = std::env::var("PROBE_LETTER").unwrap_or_else(|_| "K".into());
    let n: usize = std::env::var("PROBE_N").ok().and_then(|s| s.parse().ok()).unwrap_or(24);
    // A fresh data folder with only the winget index in it, so the first pass is cold every time.
    let real = updater_lib::sources::winget::index_path();
    let tmp = std::env::temp_dir().join(format!("icon-probe-{}", std::process::id()));
    std::env::set_var("LOCALAPPDATA", &tmp);
    let fresh = updater_lib::sources::winget::index_path();
    std::fs::copy(&real, &fresh).expect("copy the winget index");
    let list = browse::list().expect("winget index");
    let req = browse::Request { letter: Some(letter.clone()), sort: Some("az".into()), offset: 0, limit: n, ..Default::default() };
    let (page, _) = browse::page_for(&list, &req, &HashSet::new(), true);
    let ids: Vec<String> = page.entries.iter().filter_map(|e| e.id.strip_prefix("other:").map(String::from)).collect();
    for pass in ["first", "second"] {
        let t0 = Instant::now();
        let mut s = futures_util::stream::iter(ids.clone().into_iter().map(|id| async move {
            let r = tokio::time::timeout(std::time::Duration::from_secs(60), async {
                let src = other::icon_source_of(None, &id).await?;
                icons::find(&id, src.icon.as_deref(), &src.pages).await.ok().map(icons::normalize)
            })
            .await
            .ok()
            .flatten();
            (id, r.map(|g| (g.via, g.bytes.len())))
        }))
        .buffer_unordered(icons::PARALLEL);
        let (mut first, mut hits, mut sizes) = (None, 0, 0usize);
        while let Some((_id, r)) = s.next().await {
            if let Some((_, len)) = r {
                first.get_or_insert(t0.elapsed());
                hits += 1;
                sizes += len;
            }
        }
        println!(
            "{pass} pass, letter {letter}: {} apps, {hits} icons, first after {:?}, all done after {:?}, {} KB of icons",
            ids.len(),
            first.unwrap_or_default(),
            t0.elapsed(),
            sizes / 1024
        );
    }
    let _ = std::fs::remove_dir_all(&tmp);
}

/// PROBE_IDS=EpicGames.EpicGamesLauncher,Cisco.Webex cargo test --release --test icon_probe explain -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn explain() {
    for id in std::env::var("PROBE_IDS").unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
        let t0 = Instant::now();
        let src = other::icon_source_of(None, id).await;
        println!("{id}: icon {:?}, pages {:?}", src.as_ref().and_then(|s| s.icon.clone()), src.as_ref().map(|s| s.pages.clone()));
        if let Some(s) = src {
            let r = icons::find(id, s.icon.as_deref(), &s.pages).await;
            println!("  -> {:?} after {:?}", r.map(|g| (g.via, g.ext, g.bytes.len())).map_err(|m| m.passing), t0.elapsed());
        }
    }
}
