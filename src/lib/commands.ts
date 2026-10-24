// Every backend command. Names match #[tauri::command] functions in src-tauri/src/commands.rs.
// Arguments are passed as camelCase keys.
import type { ActivityEntry, BackupInfo, BrowsePage, CatalogEntry, LeftoverResult, LeftoverScan, PrefsItem, ServiceInfo, Settings, Snapshot, VersionInfo } from "./types";
import type { OtherSort } from "./browse";

export interface Commands {
  /** Everything the UI needs on start. */
  get_snapshot: { args: {}; ret: Snapshot };
  /** Re-scan installed apps and fetch latest versions. Resolves when done. */
  check_now: { args: {}; ret: void };

  /** Queue updates for these apps (latest version). */
  update_apps: { args: { appIds: string[] }; ret: void };
  /** Queue every available update. */
  update_all: { args: {}; ret: void };
  /** Install a catalog entry (Get apps). version null = latest. */
  install_app: { args: { id: string; version: string | null }; ret: void };
  /** Install a specific version of an installed app (roll back or forward). */
  install_version: { args: { appId: string; version: string }; ret: void };
  uninstall_app: { args: { appId: string; removePrefs: boolean }; ret: void };
  prefs_preview: { args: { appId: string }; ret: PrefsItem[] };
  /** What an uninstalled (or broken) program left behind; nothing is deleted. */
  leftover_scan: { args: { appId: string }; ret: LeftoverScan };
  /** Delete the ticked items of a scan (registry entries, services and tasks are saved first). */
  leftover_delete: { args: { scanId: string; ids: string[] }; ret: LeftoverResult };
  /** Put back what a cleanup saved. */
  leftover_restore: { args: { backupId: string }; ret: void };
  /** Cleanups that can still be restored. */
  leftover_backups: { args: {}; ret: BackupInfo[] };
  window_ready: { args: {}; ret: void };
  /** Ask for admin rights once more and continue the jobs waiting for them. */
  allow_admin: { args: {}; ret: void };
  /** The screen the tray asked for, once; null when there is none. */
  take_route: { args: {}; ret: string | null };
  open_logs_folder: { args: {}; ret: void };
  repair_app: { args: { appId: string }; ret: void };

  open_app: { args: { appId: string }; ret: void };
  open_store_page: { args: { appId: string }; ret: void };
  /** A driver's maker page: where a BIOS is updated from, or the maker's own updater for a card. */
  open_driver_page: { args: { id: string }; ret: void };
  /** Restart the PC in a minute to finish installing drivers. */
  restart_pc: { args: {}; ret: void };
  show_in_folder: { args: { appId: string }; ret: void };
  get_versions: { args: { appId: string }; ret: VersionInfo[] };
  get_catalog: { args: {}; ret: CatalogEntry[] };
  get_catalog_entry: { args: { id: string }; ret: CatalogEntry | null };
  /** Versions that can be installed for a catalog entry (newest first). */
  catalog_versions: { args: { id: string }; ret: VersionInfo[] };
  /** A page of the whole winget catalog (Other apps on only): a search through it or null, a
   * category and first letter ("A" to "Z", "#") or null for all, and an order. */
  browse_other_apps: { args: { query: string | null; category: string | null; letter: string | null; sort: OtherSort; offset: number; limit: number }; ret: BrowsePage };

  pause_job: { args: { jobId: string }; ret: void };
  resume_job: { args: { jobId: string }; ret: void };
  cancel_job: { args: { jobId: string }; ret: void };
  retry_job: { args: { jobId: string }; ret: void };
  /** New order of waiting job ids. */
  reorder_jobs: { args: { jobIds: string[] }; ret: void };
  pause_all: { args: {}; ret: void };
  resume_all: { args: {}; ret: void };
  /** Remove finished, failed and canceled jobs from the queue view. */
  clear_finished_jobs: { args: {}; ret: void };
  /** Close the processes blocking this job, then continue it. */
  close_and_continue: { args: { jobId: string }; ret: void };

  get_activity: { args: {}; ret: ActivityEntry[] };
  /** Writes the log to a file the user picks; returns the path or null when canceled. */
  export_activity: { args: {}; ret: string | null };
  open_log: { args: { path: string }; ret: void };

  get_services: { args: {}; ret: ServiceInfo[] };
  stop_service: { args: { name: string }; ret: void };
  start_service: { args: { name: string }; ret: void };
  stop_safe_services: { args: {}; ret: void };
  /** Starts Creative Cloud desktop (or opens it). */
  open_creative_cloud: { args: {}; ret: void };

  set_settings: { args: { patch: Partial<Settings> }; ret: Settings };
  /** Folder picker for install locations; returns the chosen folder or null. */
  pick_folder: { args: { current: string }; ret: string | null };
  clear_cache: { args: {}; ret: number };

  /** Tray popup -> show main window, optionally on a route ("home", "installed", "queue"...). */
  show_main: { args: { route: string | null }; ret: void };
  quit_app: { args: {}; ret: void };
  /** Shows a Windows notification so the user can check notifications work. */
  test_notification: { args: {}; ret: void };
  /** Downloads and installs the newer The Updater 3D (status.selfUpdate), then restarts it. */
  install_self_update: { args: {}; ret: void };
}
