// Shared contract between the Rust backend (src-tauri/src/model.rs) and the UI.
// Field names are camelCase on both sides (serde rename_all = "camelCase").

/** "programs": any other program in Windows' list of installed programs (uninstall only). */
export type Source = "adobe" | "craft" | "blender" | "tools" | "other" | "programs" | "drivers" | "games";

export type AppStatus =
  | "current" // installed and up to date
  | "update" // installed, newer version available
  | "queued" // a job for it is waiting in the queue
  | "updating" // a job for it is running
  | "blocked" // job waiting for the app to close
  | "failed" // last job failed
  | "missing" // registered as installed but its files are gone (Repair reinstalls, Uninstall removes the entry)
  | "unknown"; // installed, latest version unknown (offline, catalog failed)

export interface AppEntry {
  /** Stable id. "adobe:SBSTP", "craft:photocraft", "blender:5.2" (one entry per Blender series), "tools:obs", "tools:godot:4.7.2", "other:7zip.7zip" (a winget package). */
  id: string;
  source: Source;
  /** "Substance 3D Painter", "PhotoCraft", "Blender 5.2" */
  name: string;
  /** Two or three letter mark used when no icon is available: "Pt", "Ph", "Bl" */
  code: string;
  /** Brand colours for the mark and the tile wash. */
  fg: string;
  bg: string;
  /** URL the webview can load (asset protocol or https), or null. */
  iconUrl: string | null;
  /** The icon search is over and found none: the card shows its painted mark, not a shimmer. */
  iconMiss?: boolean;
  version: string;
  latest: string | null;
  status: AppStatus;
  /** Bytes to download for the update, when known. */
  updateSize: number | null;
  sizeOnDisk: number | null;
  installDir: string | null;
  /** ISO date of the last install or update, when known. */
  updatedAt: string | null;
  /** Short tagline: "Paint and texture 3D models". */
  tagline: string | null;
  /** Group shown as a section and filter: "Adobe", "Craft apps", "Blender", "More apps". */
  group: string;
  /** Process is running right now (blocks updates). */
  running: boolean;
  canUninstall: boolean;
  /** Older versions can be installed. */
  canRollback: boolean;
  /** Steam or the Microsoft Store updates and removes it; this app only lists it. */
  readOnly: boolean;
  /** A Microsoft Store copy: its page in the Store app ("ms-windows-store://pdp/?PFN=..."). */
  storeUrl: string | null;
  /** Updated from its maker's own page, not here (a BIOS, or a card whose maker was not reached): the page to open */
  manualUrl?: string | null;
  /** An Other program updated from its GitHub releases: "owner/repo" */
  github?: string | null;
  /** The Updater 3D itself: its page is the app's own page, and it updates itself its own way. */
  isSelf?: boolean;
  /** Blender only: "stable" | "lts" | "beta". */
  channel: string | null;
  /** Human note shown under the version, e.g. "Installs next to 5.2, your 5.2 stays". */
  note: string | null;
  /** Current job for this app, if any. */
  jobId: string | null;
}

export interface CatalogEntry {
  /** Id the app will get once installed, e.g. "adobe:ILST", "craft:vectorcraft", "blender:5.3". */
  id: string;
  source: Source;
  name: string;
  code: string;
  fg: string;
  bg: string;
  iconUrl: string | null;
  /** The icon search is over and found none: the card shows its painted mark, not a shimmer. */
  iconMiss?: boolean;
  version: string;
  size: number | null;
  tagline: string | null;
  /** A few sentences about the app for its page (the card shows the tagline). */
  description: string | null;
  /** Why it can not be installed silently here (Other apps, once its manifest was read). */
  blocked: string | null;
  group: string;
  prerelease: boolean;
  /** Already installed (shown with an "Installed" label and Open instead of Install). */
  installed: boolean;
  /** Offer a version picker (catalog_versions) next to Install. */
  pickVersion: boolean;
}

/** The Updater 3D itself, for its own page. */
export interface SelfInfo {
  version: string;
  installDir: string | null;
  sizeOnDisk: number | null;
  /** when this version was installed */
  updatedAt: string | null;
  /** its public project, with every release */
  projectUrl: string;
}

export interface VersionInfo {
  version: string;
  /** ISO date */
  date: string | null;
  /** Release notes as plain lines (bullets) */
  notes: string[];
  /** Link to full notes */
  notesUrl: string | null;
  size: number | null;
  installed: boolean;
  latest: boolean;
  prerelease: boolean;
}

export type JobKind = "update" | "install" | "uninstall" | "rollback" | "repair";
export type JobState =
  | "waiting"
  | "downloading"
  | "paused"
  | "installing"
  | "verifying"
  | "blocked"
  | "done"
  | "failed"
  | "canceled";

export interface JobError {
  /** Short code: "Error 183", "Checksum mismatch", "Exit code 1603" */
  code: string;
  /** One line: "Files in use. Photoshop.exe was running." */
  message: string;
  /** Multi-line detail for the expandable box */
  detail: string;
  logPath: string | null;
}

/** One thing "Also delete settings and add-ons" would remove */
export interface PrefsItem {
  /** "AppData\Roaming\obs-studio", or a registry key "HKCU\Software\..." */
  path: string;
  /** bytes on disk, null for a registry key */
  size: number | null;
  registry: boolean;
}

export interface Job {
  id: string;
  appId: string;
  name: string;
  code: string;
  fg: string;
  bg: string;
  iconUrl: string | null;
  kind: JobKind;
  fromVersion: string | null;
  toVersion: string;
  state: JobState;
  downloaded: number;
  total: number;
  /** bytes per second, 0 when not downloading */
  speed: number;
  /** seconds, null when unknown */
  eta: number | null;
  /** 0..1 overall progress (download + install) */
  progress: number;
  /** "Downloading", "Installing, step 2 of 4", "Waiting for Photoshop to close" */
  step: string;
  /** What it does right now, one short line: "Downloading over 6 connections from github.com", "Unpacking 1,234 files" */
  detail?: string;
  /** Processes that must close before this job continues */
  blockedBy: string[];
  error: JobError | null;
  startedAt: string | null;
  finishedAt: string | null;
  /** installing, but only waiting (for its turn or for Creative Cloud), so Cancel is safe */
  cancelable?: boolean;
  /** downloaded, waiting for its turn to run the installer (one installer runs at a time) */
  ready?: boolean;
  /** waiting for the user to allow admin rights ("Allow admin rights" in the Queue); nothing ran yet */
  needsAdmin?: boolean;
}

export type ActivityKind = "update" | "install" | "uninstall" | "rollback" | "check" | "failure" | "service";

export interface ActivityEntry {
  id: string;
  /** ISO time */
  time: string;
  kind: ActivityKind;
  title: string;
  /** right-aligned meta: "312 MB, 1 min 4 s", "Error 183", "Kept version, no download" */
  meta: string | null;
  appId: string | null;
  error: JobError | null;
  /** A cleanup's saved registry entries and tasks, which "Restore" puts back (30 days). */
  restoreId?: string | null;
}

/** What an uninstalled program left behind (leftover_scan). */
export type LeftoverKind = "folder" | "entry" | "shortcut" | "startup" | "registry" | "service" | "task" | "appPath" | "fileType" | "settings";
export interface LeftoverItem {
  id: string;
  kind: LeftoverKind;
  /** The path or registry key as shown. */
  label: string;
  /** Why it is listed: "Starts C:\...pp.exe, which is gone". */
  detail: string;
  size: number | null;
  /** Ticked when the list opens. */
  checked: boolean;
  /** Always removed ("Remove from list" removes the entry). */
  required: boolean;
  /** Needs admin rights. */
  elevated: boolean;
}
export interface LeftoverScan {
  scanId: string;
  appId: string;
  name: string;
  /** The program's uninstaller is missing: this list is for "Remove from list". */
  broken: boolean;
  items: LeftoverItem[];
}
export interface LeftoverResult {
  removed: number;
  failed: string[];
  freed: number;
  backupId: string | null;
}
export interface BackupInfo {
  id: string;
  created: string;
  app: string;
  items: number;
}

export interface ServiceInfo {
  /** process image or Windows service name */
  name: string;
  displayName: string;
  description: string;
  kind: "process" | "service";
  running: boolean;
  /** percent of total CPU */
  cpu: number;
  /** bytes */
  memory: number;
  /** needed for sign-in and licensing, do not stop */
  required: boolean;
  safeToStop: boolean;
}

export type AutoMode = "off" | "notify" | "install";

export interface Settings {
  /** hours between background checks, 0 = never */
  checkInterval: number;
  autoMode: AutoMode;
  /** "02:00" */
  windowStart: string;
  windowEnd: string;
  /** appId -> mode, overrides autoMode */
  exceptions: Record<string, AutoMode>;
  /** apps and drivers whose updates are turned off: not shown, counted, notified or installed */
  ignored?: string[];
  keepPreviousVersions: boolean;
  /** bytes per second, 0 = unlimited */
  bandwidthLimit: number;
  bandwidthLimitEnabled: boolean;
  launchAtStartup: boolean;
  startMinimized: boolean;
  closeToTray: boolean;
  notifyUpdates: boolean;
  notifyFinished: boolean;
  notifyFailed: boolean;
  /** Adobe install language, "en_US" */
  language: string;
  /** Adobe install root, "C:\\Program Files\\Adobe" */
  adobeInstallDir: string;
  /** Root that new Blender versions go into */
  blenderInstallDir: string;
  /** Root that new Godot versions are unpacked into (one folder per version) */
  godotInstallDir: string;
  /** "beta" (default) also lists Blender alpha, beta and release candidate builds; "stable" hides them */
  blenderChannel: "stable" | "beta";
  /** Bug-fix releases (5.2.2 -> 5.2.3) keep the old copy instead of replacing it */
  blenderKeepPatchVersions: boolean;
  /** Show Craft app pre-releases */
  craftPrereleases: boolean;
  /** Get apps also lists apps that are already installed */
  showInstalledInGetApps: boolean;
  /** Which sources are enabled */
  /** `other`: every other app, matched from the public winget catalog (off by default) */
  sources: { adobe: boolean; craft: boolean; blender: boolean; tools: boolean; other: boolean; drivers?: boolean; games?: boolean };
  /** Number of parallel downloads */
  parallelDownloads: number;
  /** One UAC prompt ever instead of one per batch (registers an elevated task) */
  silentElevation: boolean;
  /** Page zoom of the main window, 1 = 100%. Ctrl +, Ctrl -, Ctrl 0. */
  uiZoom: number;
}

export interface DiskInfo {
  path: string;
  free: number;
  total: number;
}

export interface Status {
  online: boolean;
  checking: boolean;
  /** ISO time of last finished check */
  lastCheck: string | null;
  /** Creative Cloud desktop services running (needed for Adobe installs) */
  ccRunning: boolean;
  ccInstalled: boolean;
  signedIn: boolean;
  /** Free space on the drives we install to */
  disks: DiskInfo[];
  /** Total bytes in the download cache */
  cacheSize: number;
  /** "Checking Adobe", "Reading installed apps" while scanning; null otherwise */
  scanStep: string | null;
  /** true until the first scan finished */
  firstScan: boolean;
  /** app version of The Updater 3D */
  appVersion: string;
  /** set when a newer The Updater 3D is available */
  selfUpdate: string | null;
  /** "16 cores, 32 GB, SSD": the PC the app tuned itself for */
  machine?: string;
  /** How much the app does at once on this PC */
  machineTier?: "low" | "normal" | "high";
  /** a driver installed since the app started needs a restart to finish */
  restartNeeded?: boolean;
}

export interface Snapshot {
  apps: AppEntry[];
  jobs: Job[];
  settings: Settings;
  status: Status;
}

/** Events emitted by the backend */
export interface EventMap {
  /** full app list changed */
  apps: AppEntry[];
  /** job list or progress changed (sent at most ~8 times a second) */
  jobs: Job[];
  status: Status;
  settings: Settings;
  activity: ActivityEntry;
  /** short message for a toast */
  toast: { kind: "ok" | "warn" | "err" | "info"; text: string; appId?: string };
  /** the tray or a clicked notification asked the main window to navigate (see route.ts) */
  navigate: string;
  /** a cached version history (get_versions, catalog_versions) was refreshed in the background */
  "versions-changed": { id: string };
  /** Get apps icons or lists changed in the background (found icons, the starter list finished) */
  catalog_changed: null;
  /** descriptions and install verdicts of winget packages read in the background */
  other_details: OtherDetail[];
  /** Other apps icons found in the background, a few at a time */
  other_icons: OtherIcon[];
}

/** A found icon for a Get apps entry. */
export interface OtherIcon {
  id: string;
  /** null: the search is over and found no icon */
  iconUrl: string | null;
}

/** What a winget package's manifest told about it, for a card already shown. */
export interface OtherDetail {
  id: string;
  tagline: string | null;
  description: string | null;
  blocked: string | null;
}

/** One page of the whole winget catalog (Get apps, Other apps, Browse all). */
export interface BrowsePage {
  /** entries of this category and letter in all */
  total: number;
  entries: CatalogEntry[];
  /** packages in the whole winget catalog */
  packages: number;
  categories: { name: string; count: number }[];
  /** entries of this selection with a known release date */
  dated: number;
}
