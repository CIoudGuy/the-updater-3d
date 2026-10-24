//! How long each part of the first scan takes on this PC. Not run by default:
//! cargo test --test scan_timing -- --ignored --nocapture

use std::time::Instant;
use updater_lib::sources::{adobe, blender, craft, drivers, other, tools};
use updater_lib::winutil;

fn t<T>(what: &str, f: impl FnOnce() -> T) -> T {
    let s = Instant::now();
    let v = f();
    println!("{what:<28} {:>7} ms", s.elapsed().as_millis());
    v
}

#[tokio::test(flavor = "multi_thread")]
#[ignore]
async fn first_scan_timing() {
    let all = Instant::now();
    let un = t("uninstall entries", winutil::uninstall_entries);
    let mut v = t("adobe scan", || adobe::scan(None, &un));
    v.extend(t("craft scan", || craft::scan(&un)));
    v.extend(t("blender scan", || blender::scan(&un, std::path::Path::new("C:\\Program Files\\Blender Foundation"))));
    v.extend(t("tools scan", || tools::scan(&un, std::path::Path::new("C:\\none"))));
    t("other scan (no feed)", || other::scan(&un, &v, None));
    t("drivers present", drivers::present);
    println!("local scan total             {:>7} ms", all.elapsed().as_millis());

    macro_rules! time {
        ($name:expr, $f:expr) => {
            async {
                let s = Instant::now();
                let ok = $f.await.is_ok();
                println!("{:<28} {:>7} ms ok={ok}", $name, s.elapsed().as_millis());
            }
        };
    }
    let net = Instant::now();
    tokio::join!(
        time!("adobe catalog", adobe::fetch_catalog()),
        time!("craft feed", craft::fetch(false)),
        time!("blender feed", blender::fetch(&v, true)),
        time!("tools feed", tools::fetch(&v)),
        time!("other feed (winget)", other::fetch(&v, None)),
        time!("windows update drivers", drivers::fetch()),
    );
    println!("network checks (parallel)   {:>7} ms", net.elapsed().as_millis());
}
