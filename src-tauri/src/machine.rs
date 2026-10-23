//! What this PC can take: its cores, memory and system disk, read once at start, and the tier
//! that sets how much the app does at once. A slow PC gets the lean path (fewer things at once,
//! less kept in memory), a fast one more parallel work and more kept while the window is open.
//! Limits per server (politeness) stay the same on every tier.

use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    Low,
    #[default]
    Normal,
    High,
}

/// This PC, as read at start.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Machine {
    pub cores: usize,
    /// Memory Windows reports, in MB
    pub ram_mb: u64,
    /// The system disk is a solid state drive; None when Windows does not say
    pub ssd: Option<bool>,
    pub tier: Tier,
}

/// How much the app does at once on a tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tuning {
    /// Connections one download may open (all to the same server)
    pub conns_per_file: usize,
    /// Connections a download starts with
    pub start_conns: usize,
    /// Package version lists and manifests read at once in a check (all from Microsoft's catalog server)
    pub manifest_parallel: usize,
    /// Starter list packages read at once (same server)
    pub popular_parallel: usize,
    /// Apps whose icon is searched for at once (many servers, each with its own cap)
    pub icon_parallel: usize,
    /// Get apps details read at once (Microsoft's catalog server)
    pub detail_parallel: usize,
    /// Download size questions at once (many servers)
    pub size_parallel: usize,
    /// Fetched notes pages kept while the window is open
    pub note_pages: usize,
    /// Build the whole-catalog list for Get apps ahead, while the window is open
    pub prebuild_browse: bool,
}

/// Memory below this (Windows reports a bit less than what is fitted) makes a PC low end.
const LOW_RAM_MB: u64 = 6 * 1024;
/// A 16 GB PC reports about 15.8 GB.
const HIGH_RAM_MB: u64 = 15 * 1024;

pub fn tier_of(cores: usize, ram_mb: u64, ssd: Option<bool>) -> Tier {
    if cores <= 2 || ram_mb < LOW_RAM_MB {
        Tier::Low
    } else if cores >= 8 && ram_mb >= HIGH_RAM_MB && ssd != Some(false) {
        Tier::High
    } else {
        Tier::Normal
    }
}

/// The per-server numbers never go above what the app always used (6 connections per download,
/// 8 and 16 requests at once to Microsoft's catalog server, 6 for Get apps details).
pub fn tuning(t: Tier) -> Tuning {
    match t {
        Tier::Low => Tuning { conns_per_file: 3, start_conns: 2, manifest_parallel: 4, popular_parallel: 6, icon_parallel: 6, detail_parallel: 3, size_parallel: 4, note_pages: 8, prebuild_browse: false },
        Tier::Normal => Tuning { conns_per_file: 6, start_conns: 4, manifest_parallel: 8, popular_parallel: 16, icon_parallel: 16, detail_parallel: 6, size_parallel: 8, note_pages: 24, prebuild_browse: true },
        Tier::High => Tuning { conns_per_file: 6, start_conns: 4, manifest_parallel: 8, popular_parallel: 16, icon_parallel: 24, detail_parallel: 6, size_parallel: 12, note_pages: 64, prebuild_browse: true },
    }
}

impl Machine {
    pub fn new(cores: usize, ram_mb: u64, ssd: Option<bool>) -> Machine {
        Machine { cores, ram_mb, ssd, tier: tier_of(cores, ram_mb, ssd) }
    }

    /// "16 cores, 32 GB, SSD": what Settings shows.
    pub fn line(&self) -> String {
        let gb = (self.ram_mb as f64 / 1024.0).round().max(1.0) as u64;
        let disk = match self.ssd {
            Some(true) => ", SSD",
            Some(false) => ", hard disk",
            None => "",
        };
        format!("{} {}, {gb} GB{disk}", self.cores, if self.cores == 1 { "core" } else { "cores" })
    }
}

fn ram_mb() -> u64 {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    match unsafe { GlobalMemoryStatusEx(&mut m) } {
        Ok(()) => m.ullTotalPhys / (1024 * 1024),
        // Unknown: counts as a middle PC.
        Err(_) => 8 * 1024,
    }
}

/// Is the disk Windows is on a solid state drive?
fn system_ssd() -> Option<bool> {
    use sysinfo::{DiskKind, DiskRefreshKind, Disks};
    let sys = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".into()).to_ascii_uppercase();
    let disks = Disks::new_with_refreshed_list_specifics(DiskRefreshKind::nothing().with_kind());
    let d = disks.list().iter().find(|d| d.mount_point().to_string_lossy().to_ascii_uppercase().starts_with(&sys))?;
    match d.kind() {
        DiskKind::SSD => Some(true),
        DiskKind::HDD => Some(false),
        _ => None,
    }
}

static THIS: Lazy<Machine> = Lazy::new(|| {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    let m = Machine::new(cores, ram_mb(), system_ssd());
    log::info!("This PC: {} ({:?} tier)", m.line(), m.tier);
    m
});

/// This PC.
pub fn this() -> &'static Machine {
    &THIS
}

/// How much to do at once on this PC.
pub fn tuned() -> Tuning {
    tuning(this().tier)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_pcs_get_the_lean_path_and_big_ones_more_at_once() {
        // Two cores or 4 GB: low, whatever else it has.
        assert_eq!(tier_of(2, 16 * 1024, Some(true)), Tier::Low);
        assert_eq!(tier_of(1, 2 * 1024, None), Tier::Low);
        assert_eq!(tier_of(8, 3900, Some(true)), Tier::Low);
        // A usual laptop.
        assert_eq!(tier_of(4, 8 * 1024, Some(true)), Tier::Normal);
        assert_eq!(tier_of(6, 15_900, Some(true)), Tier::Normal);
        // Many cores, 16 GB or more.
        assert_eq!(tier_of(8, 15_800, Some(true)), Tier::High);
        assert_eq!(tier_of(32, 64 * 1024, None), Tier::High);
        // A hard disk keeps a fast PC at normal.
        assert_eq!(tier_of(16, 32 * 1024, Some(false)), Tier::Normal);
    }

    #[test]
    fn every_tier_stays_within_the_limits_per_server() {
        for t in [Tier::Low, Tier::Normal, Tier::High] {
            let n = tuning(t);
            assert!(n.conns_per_file <= 6 && n.start_conns <= n.conns_per_file && n.start_conns >= 1, "{t:?}");
            assert!(n.manifest_parallel <= 8 && n.popular_parallel <= 16 && n.detail_parallel <= 6, "{t:?}");
            assert!(n.icon_parallel >= 1 && n.size_parallel >= 1 && n.note_pages >= 1, "{t:?}");
        }
        let (low, normal, high) = (tuning(Tier::Low), tuning(Tier::Normal), tuning(Tier::High));
        assert!(low.conns_per_file < normal.conns_per_file && low.icon_parallel < normal.icon_parallel && normal.icon_parallel < high.icon_parallel);
        assert!(!low.prebuild_browse && normal.prebuild_browse && high.prebuild_browse);
        assert!(low.note_pages < normal.note_pages && normal.note_pages < high.note_pages);
    }

    #[test]
    fn normal_pcs_keep_the_numbers_the_app_always_used() {
        let n = tuning(Tier::Normal);
        assert_eq!((n.conns_per_file, n.start_conns, n.manifest_parallel, n.popular_parallel, n.icon_parallel, n.detail_parallel, n.size_parallel), (6, 4, 8, 16, 16, 6, 8));
    }

    #[test]
    fn settings_line_names_cores_memory_and_disk() {
        assert_eq!(Machine::new(16, 32 * 1024 - 200, Some(true)).line(), "16 cores, 32 GB, SSD");
        assert_eq!(Machine::new(1, 3800, Some(false)).line(), "1 core, 4 GB, hard disk");
        assert_eq!(Machine::new(4, 8000, None).line(), "4 cores, 8 GB");
    }

    #[test]
    fn this_pc_is_read() {
        let m = this();
        assert!(m.cores >= 1 && m.ram_mb > 0);
    }
}
