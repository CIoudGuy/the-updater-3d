// Reactive snapshot of the backend plus derived values. Events from the backend patch it.
import { call, on, send, setErrorHandler } from "./api";
import type { ActivityEntry, AppEntry, AppStatus, Job, Settings, Status } from "./types";
import { driveOf } from "./format";
import { currentBatch } from "./batch";
import { tryLoad } from "./load";
import { splitMissing } from "./view";

const defaultSettings: Settings = {
  checkInterval: 4,
  autoMode: "notify",
  windowStart: "02:00",
  windowEnd: "06:00",
  exceptions: {},
  keepPreviousVersions: true,
  bandwidthLimit: 0,
  bandwidthLimitEnabled: false,
  launchAtStartup: true,
  startMinimized: false,
  closeToTray: true,
  notifyUpdates: true,
  notifyFinished: true,
  notifyFailed: true,
  language: "en_US",
  adobeInstallDir: "",
  blenderInstallDir: "",
  godotInstallDir: "",
  blenderChannel: "beta",
  blenderKeepPatchVersions: false,
  craftPrereleases: false,
  showInstalledInGetApps: false,
  sources: { adobe: true, craft: true, blender: true, tools: true, other: true, drivers: true },
  parallelDownloads: 3,
  silentElevation: true,
  uiZoom: 1,
};
const defaultStatus: Status = {
  online: true,
  checking: false,
  lastCheck: null,
  ccRunning: true,
  ccInstalled: true,
  signedIn: true,
  disks: [],
  cacheSize: 0,
  scanStep: null,
  firstScan: true,
  appVersion: "",
  selfUpdate: null,
};

export interface Toast {
  id: number;
  kind: "ok" | "warn" | "err" | "info";
  text: string;
  appId?: string;
  /** a toast with a key is updated in place instead of stacking (zoom) */
  key?: string;
}

/**
 * Merge an incoming list into the current one in place when it has the same ids in the same order.
 * Only fields that really changed are written, so a progress tick wakes only the elements that show it
 * instead of re-rendering every row.
 */
function sameValue(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (typeof a !== "object" || typeof b !== "object" || !a || !b) return false;
  return JSON.stringify(a) === JSON.stringify(b);
}
function syncList<T extends { id: string }>(cur: T[], next: T[]): T[] {
  if (cur.length !== next.length) return next;
  for (let i = 0; i < cur.length; i++) if (cur[i].id !== next[i].id) return next;
  for (let i = 0; i < cur.length; i++) {
    const c = cur[i] as Record<string, unknown>;
    const n = next[i] as Record<string, unknown>;
    for (const k in n) if (!sameValue(c[k], n[k])) c[k] = n[k];
  }
  return cur;
}

export const GROUPS = ["Adobe", "Craft apps", "Blender", "More apps", "Other apps", "Games", "Drivers", "Other programs"] as const;

/** Shown wherever Other apps are listed. */

export function isOpenState(s: Job["state"]) {
  return s === "waiting" || s === "paused" || s === "blocked" || s === "downloading" || s === "installing" || s === "verifying";
}
export function isRunningState(s: Job["state"]) {
  return s === "downloading" || s === "installing" || s === "verifying";
}

class Store {
  ready = $state(false);
  /** the first snapshot could not be read; the page shows this with a Retry button */
  loadError = $state<string | null>(null);
  apps = $state<AppEntry[]>([]);
  jobs = $state<Job[]>([]);
  settings = $state<Settings>(defaultSettings);
  status = $state<Status>(defaultStatus);
  activity = $state<ActivityEntry[]>([]);
  toasts = $state<Toast[]>([]);
  /** ticks every 20 s so "3 minutes ago" stays right */
  now = $state(Date.now());

  // ---- derived ----
  /** Updates that install here (a BIOS is updated from its maker's page) */
  updates = $derived(this.apps.filter((a) => a.status === "update" && !a.manualUrl));
  updatesBytes = $derived(this.updates.reduce((n, a) => n + (a.updateSize ?? 0), 0));
  /** Apps whose files are gone that a reinstall brings back. A removed program whose entry was left behind is a leftover, not a repair. */
  missing = $derived(splitMissing(this.apps).repair);
  /** Programs that were removed but left their entry in Windows' list (their files and uninstaller are gone) */
  leftovers = $derived(splitMissing(this.apps).leftovers);
  openJobs = $derived(this.jobs.filter((j) => isOpenState(j.state)));
  runningJobs = $derived(this.jobs.filter((j) => isRunningState(j.state)));
  finishedJobs = $derived(this.jobs.filter((j) => j.state === "done" || j.state === "failed" || j.state === "canceled"));
  failedJobs = $derived(this.jobs.filter((j) => j.state === "failed"));
  activeJobs = $derived(this.jobs.filter((j) => isRunningState(j.state) || j.state === "paused" || j.state === "blocked"));
  waitingJobs = $derived(this.jobs.filter((j) => j.state === "waiting"));
  blockedJobs = $derived(this.jobs.filter((j) => j.state === "blocked"));
  /** jobs of the batch running now, for the overall numbers (see currentBatch) */
  batchJobs = $derived(currentBatch(this.jobs));
  speed = $derived(this.jobs.reduce((n, j) => n + (j.state === "downloading" ? j.speed : 0), 0));
  downloaded = $derived(this.batchJobs.reduce((n, j) => n + Math.min(j.downloaded, j.total), 0));
  totalBytes = $derived(this.batchJobs.reduce((n, j) => n + j.total, 0));
  eta = $derived.by(() => {
    const open = this.openJobs.filter((j) => j.eta != null && j.state !== "paused" && j.state !== "blocked");
    if (!open.length) return null;
    // downloads run in parallel: the batch is done when the slowest finishes
    return Math.max(...open.map((j) => j.eta ?? 0));
  });
  /** 0..1 over the current batch */
  overall = $derived.by(() => {
    const b = this.batchJobs;
    if (!b.length) return 0;
    return b.reduce((n, j) => n + j.progress, 0) / b.length;
  });
  /** Adobe apps are looked after (Settings > Sources). Off: nothing about Adobe or Creative Cloud shows. */
  adobeOn = $derived(this.settings.sources.adobe);
  hasAdobe = $derived(this.apps.some((a) => a.source === "adobe") && this.settings.sources.adobe);
  ccDown = $derived(this.hasAdobe && this.status.ccInstalled && !this.status.ccRunning);
  signedOut = $derived(this.hasAdobe && this.status.ccInstalled && this.status.ccRunning && !this.status.signedIn);
  jobByApp = $derived(new Map(this.jobs.filter((j) => isOpenState(j.state)).map((j) => [j.appId, j])));

  app(id: string) {
    return this.apps.find((a) => a.id === id);
  }
  job(id: string | null | undefined) {
    return id ? this.jobs.find((j) => j.id === id) : undefined;
  }

  /** free bytes of a drive "C:" */
  freeOn(drive: string | null): number | null {
    if (!drive) return null;
    const d = this.status.disks.find((x) => driveOf(x.path) === drive);
    return d ? d.free : null;
  }

  private toastTimers = new Map<number, ReturnType<typeof setTimeout>>();
  toast(kind: Toast["kind"], text: string, appId?: string, opts: { key?: string; ms?: number } = {}) {
    const ms = opts.ms ?? (kind === "err" ? 9000 : 5200);
    const same = opts.key ? this.toasts.find((t) => t.key === opts.key) : undefined;
    const id = same ? same.id : Math.random();
    if (same) {
      same.kind = kind;
      same.text = text;
      same.appId = appId;
      clearTimeout(this.toastTimers.get(id));
    } else {
      this.toasts = [...this.toasts, { id, kind, text, appId, key: opts.key }].slice(-3);
    }
    this.toastTimers.set(id, setTimeout(() => this.dismissToast(id), ms));
  }
  dismissToast(id: number) {
    clearTimeout(this.toastTimers.get(id));
    this.toastTimers.delete(id);
    this.toasts = this.toasts.filter((t) => t.id !== id);
  }

  async init(onNavigate?: (route: string) => void) {
    setErrorHandler((m) => this.toast("err", m));
    const unsub: Array<() => void> = [];
    // Subscribe and ask for the snapshot at the same time; events that arrive first are simply overwritten.
    const subs = Promise.all([
      on("apps", (a) => (this.apps = syncList(this.apps, a))),
      on("jobs", (j) => (this.jobs = syncList(this.jobs, j))),
      on("status", (s) => (this.status = s)),
      on("settings", (s) => (this.settings = s)),
      on("activity", (e) => (this.activity = [e, ...this.activity.filter((x) => x.id !== e.id)])),
      on("toast", (t) => this.toast(t.kind, t.text, t.appId)),
      on("navigate", (r) => onNavigate?.(r)),
    ]).then((u) => unsub.push(...u));
    await this.loadSnapshot();
    await subs;
    const t = setInterval(() => (this.now = Date.now()), 20000);
    unsub.push(() => clearInterval(t));
    return () => unsub.forEach((u) => u());
  }

  /** First read of the backend state. On failure `loadError` is set instead of staying on the skeleton forever. */
  async loadSnapshot() {
    this.loadError = null;
    const r = await tryLoad(() => call("get_snapshot", {}));
    if (!r.ok) {
      this.loadError = r.error;
      return;
    }
    const snap = r.value;
    this.apps = snap.apps;
    this.jobs = snap.jobs;
    this.settings = snap.settings;
    this.status = snap.status;
    this.ready = true;
    // the activity log is only needed on one screen, so it must not delay the first paint
    call("get_activity", {}).then((a) => (this.activity = [...this.activity.filter((x) => !a.some((y) => y.id === x.id)), ...a]));
  }

  patchSettings(patch: Partial<Settings>) {
    // optimistic, then confirmed by the backend answer
    this.settings = { ...this.settings, ...patch, sources: { ...this.settings.sources, ...(patch.sources ?? {}) } };
    send("set_settings", { patch }).then((s) => {
      if (s) this.settings = s;
    });
  }
}

export const store = new Store();

// ---- display helpers for statuses ----
export function statusLabel(s: AppStatus, long = false): string {
  switch (s) {
    case "current":
      return long ? "Up to date" : "Current";
    case "update":
      return long ? "Update available" : "Update";
    case "queued":
      return "Queued";
    case "updating":
      return "Updating";
    case "blocked":
      return "Close the app";
    case "failed":
      return "Failed";
    case "missing":
      return "Files missing";
    default:
      return "Unknown";
  }
}
export function statusColor(s: AppStatus): string {
  switch (s) {
    case "current":
      return "var(--ok)";
    case "update":
    case "blocked":
      return "var(--warn)";
    case "failed":
    case "missing":
      return "var(--err)";
    case "updating":
      return "var(--text)";
    case "queued":
      return "var(--text-2)";
    default:
      return "var(--text-3)";
  }
}
