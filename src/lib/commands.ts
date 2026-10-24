// Every backend command. Names match #[tauri::command] functions in src-tauri/src/commands.rs.
// Arguments are passed as camelCase keys.
import type { AccountInfo, ActivityEntry, BackupInfo, BrowsePage, CatalogEntry, LeftoverResult, LeftoverScan, OtherIcon, Platform, PrefsItem, SelfInfo, ServiceInfo, Settings, SignInStep, Snapshot, VersionInfo } from "./types";
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
  /** Look for a new version of this app only (not for app updates); the newer version or null. */
  check_self_update: { args: {}; ret: string | null };
  /** The Updater 3D itself, for its own page. */
  self_info: { args: {}; ret: SelfInfo };
  /** The Updater 3D's releases, newest first, with their notes. */
  self_versions: { args: {}; ret: VersionInfo[] };
  /** The game platforms the user can sign in to (Steam, Epic Games, GOG), and how each stands. */
  accounts: { args: {}; ret: AccountInfo[] };
  /** Start a sign-in: Steam shows a QR code for its mobile app (account_wait finishes it), or with window signs in in a Steam window; Epic Games and GOG open their page in the browser, which then shows a code for account_code. Rejects with "CANCELED" when stopped. */
  account_sign_in: { args: { platform: Platform; window?: boolean }; ret: SignInStep };
  /** Wait until Steam's QR code is approved in the Steam mobile app. New codes come as the account_qr event. */
  account_wait: { args: { platform: Platform }; ret: AccountInfo };
  /** Finish an Epic Games or GOG sign-in with what the user pasted from the browser (the page's text, its address or the code). */
  account_code: { args: { platform: Platform; text: string }; ret: AccountInfo };
  /** Stop a sign-in that waits (Steam's QR code or window). */
  account_cancel: { args: { platform: Platform }; ret: void };
  /** Forget a platform's sign-in here; its games stop showing. */
  account_sign_out: { args: { platform: Platform }; ret: void };
  /** Read a platform's library again now. */
  account_sync: { args: { platform: Platform }; ret: AccountInfo };
  /** Put back the files a game's last update or repair changed; the version it is at again. */
  game_undo: { args: { appId: string }; ret: string };
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
  /** The Other apps cards on screen and the ones kept around them (their ids): the icons of the
   * cards on screen are searched for first and searches for places that scrolled away are
   * dropped. Returns the icons known right now (iconUrl null: none was found). */
  icons_in_view: { args: { visible: string[]; near: string[] }; ret: OtherIcon[] };
  /** A screenshot in release notes, read from GitHub and kept on this PC: an address the window
   * can load, or null when it is not one GitHub serves or can not be read. */
  note_image: { args: { url: string }; ret: string | null };

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
