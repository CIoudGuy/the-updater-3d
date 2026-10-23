//! Live timing of a full update check and of the startup path, per source, with CPU time and
//! memory. Read-only on this PC (registry, Adobe database); writes only to its own data folder.
//! Not run by default:
//! PROBE_DIR=D:/somewhere cargo test --release --test perf_probe -- --ignored --nocapture --test-threads=1
//! PROBE_ROUNDS picks rounds: "cold,warm,reval" (default) or "start" (a new process on a full cache).

use std::alloc::{GlobalAlloc, Layout, System};
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::time::{Duration, Instant};
use updater_lib::model::Source;
use updater_lib::sources::Installed;
use updater_lib::sources::{adobe, blender, browse, craft, other, tools};
use updater_lib::winutil;

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = System.alloc(l);
        if !p.is_null() {
            let n = LIVE.fetch_add(l.size() as isize, Ordering::Relaxed) + l.size() as isize;
            PEAK.fetch_max(n, Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l);
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
    }
}
#[global_allocator]
static A: Counting = Counting;

fn mb(b: isize) -> String {
    format!("{:.1} MB", b as f64 / 1048576.0)
}

fn cpu_ms() -> u64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u);
    }
    let t = |f: FILETIME| ((f.dwHighDateTime as u64) << 32 | f.dwLowDateTime as u64) / 10_000;
    t(k) + t(u)
}

fn peak_ws() -> usize {
    use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
    use windows::Win32::System::Threading::GetCurrentProcess;
    let mut m = PROCESS_MEMORY_COUNTERS::default();
    unsafe {
        let _ = K32GetProcessMemoryInfo(GetCurrentProcess(), &mut m, std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32);
    }
    m.PeakWorkingSetSize
}

async fn timed<T>(f: impl Future<Output = T>) -> (T, Duration) {
    let t = Instant::now();
    let r = f.await;
    (r, t.elapsed())
}

fn catalog_apps(v: &[Installed]) -> Vec<Installed> {
    v.iter().filter(|i| i.source != Source::Programs).cloned().collect()
}

/// As `Core::rescan`, timing each part.
fn rescan(a: Option<&adobe::Catalog>, o: Option<&other::Feed>, label: &str) -> Vec<Installed> {
    let t = Instant::now();
    let un = winutil::uninstall_entries();
    let t_un = t.elapsed();
    let t1 = Instant::now();
    let mut v = adobe::scan(a, &un);
    let t_adobe = t1.elapsed();
    let t1 = Instant::now();
    v.extend(craft::scan(&un));
    v.extend(blender::scan(&un, &PathBuf::from("C:/Program Files/Blender Foundation")));
    v.extend(tools::scan(&un, &PathBuf::from("C:/Program Files/Godot")));
    let t_rest = t1.elapsed();
    let t1 = Instant::now();
    let found = other::scan(&un, &v, o);
    v.extend(found);
    let t_other = t1.elapsed();
    v.retain(|i| !i.missing);
    let t1 = Instant::now();
    let programs = updater_lib::uninstall::programs::scan(&un, &v);
    v.extend(programs);
    let t_prog = t1.elapsed();
    println!(
        "  rescan[{label}] {:>6.0} ms  (uninstall list {} entries {:.0} ms, adobe {:.0} ms, craft+blender+tools {:.0} ms, other match {:.0} ms, programs {:.0} ms) -> {} apps",
        t.elapsed().as_secs_f64() * 1e3,
        un.len(),
        t_un.as_secs_f64() * 1e3,
        t_adobe.as_secs_f64() * 1e3,
        t_rest.as_secs_f64() * 1e3,
        t_other.as_secs_f64() * 1e3,
        t_prog.as_secs_f64() * 1e3,
        v.len()
    );
    v
}

struct Feeds {
    a: Option<adobe::Catalog>,
    o: Option<other::Feed>,
    _rest: (Option<craft::Feed>, Option<blender::Feed>, Option<tools::Feed>),
}

/// One check as `Core::check_once` runs it: rescan, all feeds at once, rescan.
async fn check(label: &str, prev: Option<&Feeds>) -> Feeds {
    println!("{label}:");
    let (cpu0, live0, t0) = (cpu_ms(), LIVE.load(Ordering::Relaxed), Instant::now());
    PEAK.store(live0, Ordering::Relaxed);
    let installed = rescan(prev.and_then(|p| p.a.as_ref()), prev.and_then(|p| p.o.as_ref()), "first");
    let cat = catalog_apps(&installed);
    let prev_o = prev.and_then(|p| p.o.as_ref());
    let ((a, ta), (c, tc), (b, tb), (t, tt), (o, to)) = tokio::join!(
        timed(adobe::fetch_catalog()),
        timed(craft::fetch(false)),
        timed(blender::fetch(&installed, false)),
        timed(tools::fetch(&installed)),
        timed(other::fetch(&cat, prev_o)),
    );
    let tf = t0.elapsed();
    for (n, d, ok) in [("adobe", ta, a.is_ok()), ("craft", tc, c.is_ok()), ("blender", tb, b.is_ok()), ("more apps", tt, t.is_ok()), ("other apps", to, o.is_ok())] {
        println!("  fetch {n:<11} {:>6.0} ms {}", d.as_secs_f64() * 1e3, if ok { "" } else { "FAILED" });
    }
    if let Ok(o) = &o {
        println!("  other feed: {} matched packages, {} popular, unfinished={}", o.pkgs.len(), o.popular.len(), o.unfinished);
    }
    let f = Feeds { a: a.ok(), o: o.ok(), _rest: (c.ok(), b.ok(), t.ok()) };
    let _ = rescan(f.a.as_ref(), f.o.as_ref(), "second");
    let live = LIVE.load(Ordering::Relaxed);
    println!(
        "  TOTAL {:.0} ms (feeds done at {:.0} ms), CPU {} ms, heap peak during check {} (+{}), heap held after {} (+{}), process peak working set {}",
        t0.elapsed().as_secs_f64() * 1e3,
        tf.as_secs_f64() * 1e3,
        cpu_ms() - cpu0,
        mb(PEAK.load(Ordering::Relaxed)),
        mb(PEAK.load(Ordering::Relaxed) - live0),
        mb(live),
        mb(live - live0),
        mb(peak_ws() as isize)
    );
    f
}

/// Make every cached answer old, so the next check asks the servers again (conditional requests).
fn age_cache(dir: &std::path::Path) {
    let mut n = 0;
    for sub in ["app.theupdater3d/data/http-cache", "app.theupdater3d/data/winget"] {
        let Ok(rd) = std::fs::read_dir(dir.join(sub)) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x == "json") {
                if let Ok(mut v) = serde_json::from_slice::<serde_json::Value>(&std::fs::read(&p).unwrap_or_default()) {
                    if v.get("fetched").is_some() {
                        v["fetched"] = serde_json::json!(0);
                        let _ = std::fs::write(&p, serde_json::to_vec(&v).unwrap());
                        n += 1;
                    }
                }
            }
        }
    }
    println!("(aged {n} cached answers)");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn perf_probe() {
    let dir = PathBuf::from(std::env::var("PROBE_DIR").unwrap_or_else(|_| std::env::temp_dir().join("perf-probe").to_string_lossy().into()));
    let rounds = std::env::var("PROBE_ROUNDS").unwrap_or_else(|_| "cold,warm,reval".into());
    std::env::set_var("LOCALAPPDATA", &dir);
    println!("data folder {}, cores {}", dir.display(), std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0));
    let mut prev: Option<Feeds> = None;
    for r in rounds.split(',') {
        match r {
            "cold" => {
                let _ = std::fs::remove_dir_all(&dir);
                prev = Some(check("COLD check (empty caches)", prev.as_ref()).await);
            }
            "warm" => prev = Some(check("WARM check (caches younger than their max age)", prev.as_ref()).await),
            "reval" => {
                age_cache(&dir);
                prev = Some(check("REVALIDATE check (every answer asked again, 304s)", prev.as_ref()).await);
            }
            "start" => {
                // As `Core::preload`: a new process with every answer on disk.
                updater_lib::http::PREFER_CACHE.store(true, Ordering::Relaxed);
                prev = Some(check("STARTUP preload (new process, everything from disk)", None).await);
                updater_lib::http::PREFER_CACHE.store(false, Ordering::Relaxed);
                let (l, d) = {
                    let live0 = LIVE.load(Ordering::Relaxed);
                    let t = Instant::now();
                    let _l = browse::list().unwrap();
                    (LIVE.load(Ordering::Relaxed) - live0, t.elapsed())
                };
                println!("  browse list built in {:.0} ms, holds {}", d.as_secs_f64() * 1e3, mb(l));
                browse::drop_list();
            }
            "peaks" => {
                // Each step alone: how much memory it needs at its peak.
                macro_rules! peak {
                    ($n:expr, $e:expr) => {{
                        let l0 = LIVE.load(Ordering::Relaxed);
                        PEAK.store(l0, Ordering::Relaxed);
                        let t = Instant::now();
                        let r = $e;
                        println!("  {:<16} peak +{:<9} held +{:<9} {:.0} ms", $n, mb(PEAK.load(Ordering::Relaxed) - l0), mb(LIVE.load(Ordering::Relaxed) - l0), t.elapsed().as_secs_f64() * 1e3);
                        r
                    }};
                }
                let un = peak!("uninstall list", winutil::uninstall_entries());
                let a = peak!("adobe catalog", adobe::fetch_catalog().await.ok());
                let v = peak!("adobe scan", adobe::scan(a.as_ref(), &un));
                let _c = peak!("craft", craft::fetch(false).await.ok());
                let _b = peak!("blender", blender::fetch(&v, false).await.ok());
                let _t = peak!("more apps", tools::fetch(&v).await.ok());
                let o = peak!("other apps", other::fetch(&v, None).await.ok());
                let _o2 = peak!("other again", other::fetch(&v, o.as_ref()).await.ok());
                let w = peak!("other scan", other::scan(&un, &v, o.as_ref()));
                let _p = peak!("programs scan", updater_lib::uninstall::programs::scan(&un, &w));
            }
            "parts" => {
                use updater_lib::sources::{locate, msix, winget};
                let un = winutil::uninstall_entries();
                let ms = |t: Instant| t.elapsed().as_secs_f64() * 1e3;
                let t = Instant::now();
                let idx = winget::Index::open(&winget::index_path()).unwrap();
                println!("  index open {:.0} ms", ms(t));
                let t = Instant::now();
                let r = other::match_on_disk(&un, &idx, &winget::index_path(), &[]);
                println!("  match_on_disk {:.0} ms ({} matches)", ms(t), r.matches.len());
                let t = Instant::now();
                let r = other::match_on_disk(&un, &idx, &winget::index_path(), &[]);
                println!("  match_on_disk again {:.0} ms ({} matches)", ms(t), r.matches.len());
                for _ in 0..2 {
                    let t = Instant::now();
                    let p = msix::this_users_packages();
                    println!("  app packages {:.0} ms ({})", ms(t), p.len());
                    let t = Instant::now();
                    let h = locate::Hints::read();
                    println!("  start menu + app paths {:.0} ms ({} shortcuts)", ms(t), h.shortcuts.len());
                    let t = Instant::now();
                    let _ = locate::shortcuts_in(&locate::start_menu_dirs());
                    println!("    of which shortcuts {:.0} ms", ms(t));
                    let t = Instant::now();
                    let v = other::scan(&un, &[], None);
                    println!("  other::scan {:.0} ms ({})", ms(t), v.len());
                    let t = Instant::now();
                    let v = updater_lib::uninstall::programs::scan(&un, &v);
                    println!("  programs::scan {:.0} ms ({})", ms(t), v.len());
                }
                let t = Instant::now();
                let _ = winutil::msi_upgrade_codes();
                println!("  msi upgrade codes {:.0} ms", ms(t));
            }
            _ => {}
        }
    }
    if let Some(p) = prev {
        let live0 = LIVE.load(Ordering::Relaxed);
        drop(p);
        println!("feeds held by the checker: {}", mb(live0 - LIVE.load(Ordering::Relaxed)));
    }
}
