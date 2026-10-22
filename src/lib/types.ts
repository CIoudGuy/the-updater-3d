// Shared contract between the Rust backend (src-tauri/src/model.rs) and the UI.
// Field names are camelCase on both sides (serde rename_all = "camelCase").

export type Source = "adobe" | "craft" | "blender" | "tools";

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
  /** Stable id. "adobe:SBSTP", "craft:photocraft", "blender:5.2" (one entry per Blender series), "tools:obs", "tools:godot:4.7.2". */
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
  version: string;
  size: number | null;
  tagline: string | null;
  group: string;
  prerelease: boolean;
  /** Already installed (shown with an "Installed" label and Open instead of Install). */
  installed: boolean;
  /** Offer a version picker (catalog_versions) next to Install. */
  pickVersion: boolean;
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
  /** Processes that must close before this job continues */
  blockedBy: string[];
  error: JobError | null;
  startedAt: string | null;
  finishedAt: string | null;
  /** installing, but only waiting (for its turn or for Creative Cloud), so Cancel is safe */
  cancelable?: boolean;
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
  sources: { adobe: boolean; craft: boolean; blender: boolean; tools: boolean };
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
  /** tray asked the main window to navigate */
  navigate: string;
  /** a cached version history (get_versions, catalog_versions) was refreshed in the background */
  "versions-changed": { id: string };
}
