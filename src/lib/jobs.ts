import { bytes, bytesParts, speed as fmtSpeed, unitFor } from "./format.ts";
import type { Job } from "./types";

/** "Downloading 1.3 of 2.9 GB at 22 MB/s", "Installing, step 3 of 4", "Waiting" */
export function jobStatus(j: Job, withSpeed = true): string {
  if (queueGroup(j) === "ready") return "Downloaded, ready to install";
  switch (j.state) {
    case "downloading": {
      const u = unitFor(j.total);
      const base = `Downloading ${bytesParts(j.downloaded, u)[0]} of ${bytes(j.total)}`;
      return withSpeed && j.speed > 0 ? `${base} at ${fmtSpeed(j.speed)}` : base;
    }
    case "paused":
      return j.total > 0 ? `Paused at ${bytesParts(j.downloaded, unitFor(j.total))[0]} of ${bytes(j.total)}` : "Paused";
    case "waiting":
      return "Waiting";
    case "done":
      return "Done";
    case "failed":
      return j.error ? `Failed, ${j.error.code}` : "Failed";
    case "canceled":
      return "Canceled";
    default:
      return j.step || "Installing";
  }
}

/** "Update 27.9.1 → 28.0", "Install 5.3.0, 390 MB" */
export function jobWhat(j: Job): string {
  switch (j.kind) {
    case "update":
      return `Update ${j.fromVersion} → ${j.toVersion}`;
    case "rollback":
      return `Roll back ${j.fromVersion} → ${j.toVersion}`;
    case "repair":
      return `Repair ${j.toVersion}`;
    case "uninstall":
      return `Uninstall ${j.fromVersion ?? j.toVersion}`;
    default:
      return `Install ${j.toVersion}${j.total ? ", " + bytes(j.total) : ""}`;
  }
}

export const pct = (j: Job) => Math.round(j.progress * 100);

/** The small line under a job that is running or waiting its turn: what it does right now. Nothing for finished jobs. */
export function jobDetail(j: Pick<Job, "state" | "detail" | "ready">): string {
  if (j.state === "done" || j.state === "failed" || j.state === "canceled") return "";
  // Downloaded and waiting its turn: the status line says what it waits for.
  if (queueGroup(j) === "ready") return "";
  return (j.detail ?? "").trim();
}

/** Tooltip for a job control that does nothing right now. */
export const NO_STOP_TIP = "Installing now. It can't be stopped safely.";

/** The backend only pauses a job that is downloading (or one that already is paused or still waiting). */
export function canPause(j: Pick<Job, "state">): boolean {
  return j.state === "downloading" || j.state === "paused" || j.state === "waiting";
}
/** The backend refuses to cancel while the installer runs, unless the job is only waiting. */
export function canCancel(j: Pick<Job, "state" | "cancelable">): boolean {
  return (j.state !== "installing" && j.state !== "verifying") || !!j.cancelable;
}
/** Tooltip for the pause or resume button: the reason when it is disabled, else the action with its key. */
export function pauseTip(j: Pick<Job, "state">, keys = ""): string {
  if (!canPause(j)) return j.state === "blocked" ? "Waiting for the app to close." : NO_STOP_TIP;
  return (j.state === "paused" ? "Resume" : "Pause") + keys;
}
/** Tooltip for the cancel button. */
export function cancelTip(j: Pick<Job, "state" | "cancelable">, label = "Cancel", keys = ""): string {
  return canCancel(j) ? label + keys : NO_STOP_TIP;
}

/** Status of a failed job in a list row: "Failed, files in use" from the message, the code only if there is no message. */
export function failedLine(j: Pick<Job, "error">): string {
  if (!j.error) return "Failed";
  const first = j.error.message.trim().split(/\.\s/)[0].replace(/\.$/, "");
  if (!first) return `Failed, ${j.error.code}`;
  const lower = /^[A-Z][a-z]/.test(first) ? first[0].toLowerCase() + first.slice(1) : first;
  return `Failed, ${lower}`;
}

// ----- queue sections -----

/** The Queue in the order things happen: the one installing, the downloaded ones waiting their
 * turn (installs run one at a time), the downloads, and the jobs that have not started. */
export interface QueueGroups {
  installing: Job[];
  ready: Job[];
  downloading: Job[];
  waiting: Job[];
}

type QJob = Pick<Job, "state" | "ready">;

/** Which section of the Queue a job that is not finished goes in, null for a finished one. */
export function queueGroup(j: QJob): keyof QueueGroups | null {
  if (j.ready && (j.state === "installing" || j.state === "verifying")) return "ready";
  switch (j.state) {
    case "installing":
    case "verifying":
    case "blocked":
      return "installing";
    case "downloading":
    case "paused":
      return "downloading";
    case "waiting":
      return "waiting";
    default:
      return null;
  }
}

/** Open jobs split into the Queue sections, each in queue order (ready ones in install order). */
export function groupQueue<T extends QJob>(jobs: readonly T[]): { installing: T[]; ready: T[]; downloading: T[]; waiting: T[] } {
  const g = { installing: [] as T[], ready: [] as T[], downloading: [] as T[], waiting: [] as T[] };
  for (const j of jobs) {
    const k = queueGroup(j);
    if (k) g[k].push(j);
  }
  return g;
}

/** "1 installing, 21 ready to install, 2 downloading, 3 waiting"; empty sections are left out. */
export function queueSummary(g: { installing: unknown[]; ready: unknown[]; downloading: unknown[]; waiting: unknown[] }): string {
  const parts: string[] = [];
  if (g.installing.length) parts.push(`${g.installing.length} installing`);
  if (g.ready.length) parts.push(`${g.ready.length} ready to install`);
  if (g.downloading.length) parts.push(`${g.downloading.length} downloading`);
  if (g.waiting.length) parts.push(`${g.waiting.length} waiting`);
  return parts.join(", ");
}

/** Status of the ready job at `index` in install order: "Downloaded, installs after Photoshop".
 * The first one waits for the installer that holds the turn (not one only waiting for Creative Cloud). */
export function readyLine(g: { installing: Pick<Job, "name" | "cancelable">[]; ready: Pick<Job, "name">[] }, index: number): string {
  const before = index > 0 ? g.ready[index - 1]?.name : g.installing.find((j) => !j.cancelable)?.name;
  return before ? `Downloaded, installs after ${before}` : "Downloaded, installs next";
}

export const QUEUE_LABELS: Record<keyof QueueGroups, string> = {
  installing: "Installing now",
  ready: "Ready to install",
  downloading: "Downloading",
  waiting: "Waiting",
};

/** The sections that have jobs, in Queue order, with their headings. */
export function queueSections<T>(g: { installing: T[]; ready: T[]; downloading: T[]; waiting: T[] }): { key: keyof QueueGroups; label: string; jobs: T[] }[] {
  return (["installing", "ready", "downloading", "waiting"] as const).filter((k) => g[k].length > 0).map((k) => ({ key: k, label: QUEUE_LABELS[k], jobs: g[k] }));
}
