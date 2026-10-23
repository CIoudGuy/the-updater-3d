// Shared view logic for app tiles and rows.
import { send } from "./api";
import { dateShort, bytes } from "./format";
import { jobStatus } from "./jobs";
import { router, appHref } from "./router.svelte";
import { statusLabel, store } from "./store.svelte";
import type { AppEntry } from "./types";
import { menuItemsFor, ui } from "./ui.svelte";
import { runAppMenu } from "./actions";

export function openMenuFor(app: AppEntry, anchor: HTMLElement, at?: { x: number; y: number }) {
  const r = at ? new DOMRect(at.x, at.y, 0, 0) : anchor.getBoundingClientRect();
  ui.openMenu({
    rect: r,
    items: menuItemsFor(app),
    align: at ? "start" : "end",
    label: `${app.name} actions`,
    anchor,
    onpick: (id) => runAppMenu(app, id),
  });
}

/** The status word on a tile or row. Other programs have no catalog: "Installed", not "Current". */
export function stateLabel(app: AppEntry, long = false): string {
  if (app.source === "programs" && app.status === "current") return "Installed";
  if (app.source === "programs" && app.status === "missing") return long ? "Uninstaller missing" : "Broken";
  return statusLabel(app.status, long);
}

/** Subline under the version: "28.0 available", "Up to date", "Downloading 1.3 of 2.9 GB" */
export function subline(app: AppEntry): string {
  const j = store.jobByApp.get(app.id);
  if (app.source === "programs" && (app.status === "current" || app.status === "missing")) return app.status === "missing" ? "" : (app.tagline ?? "");
  switch (app.status) {
    case "updating":
      return j ? jobStatus(j, false) : "Updating";
    case "queued":
      return j?.state === "paused" ? jobStatus(j, false) : "Waiting in the queue";
    case "blocked":
      return j?.step ?? "Close the app to continue";
    case "update":
      return `${app.latest} available`;
    case "failed": {
      const f = store.failedJobs.find((x) => x.appId === app.id);
      return f?.error ? f.error.code : "Last update failed";
    }
    case "missing":
      return "";
    case "unknown":
      return "Latest version unknown";
    default:
      return "Up to date";
  }
}
export function subColor(app: AppEntry): string {
  switch (app.status) {
    case "update":
    case "blocked":
      return "var(--warn)";
    case "failed":
    case "missing":
      return "var(--err)";
    case "updating":
    case "queued":
      return "var(--text-2)";
    default:
      return "var(--text-3)";
  }
}
export function metaLine(app: AppEntry): string {
  const parts = [app.sizeOnDisk ? bytes(app.sizeOnDisk) : "", app.updatedAt ? dateShort(app.updatedAt) : ""].filter(Boolean);
  return parts.join(", ");
}
/**
 * The installed app behind a Get apps entry: the entry's own id, or a side-by-side install such as
 * "tools:godot:4.7.2" for the entry "tools:godot". Prefers an exact match, then one with an update waiting.
 */
export function installedFor(entry: { id: string }): AppEntry | undefined {
  const exact = store.app(entry.id);
  if (exact) return exact;
  const prefix = entry.id + ":";
  const hits = store.apps.filter((a) => a.id.startsWith(prefix));
  return hits.find((a) => a.status === "update") ?? hits[0];
}
export function go(app: AppEntry) {
  router.go(appHref(app.id));
}
export function retryFor(app: AppEntry) {
  const f = store.failedJobs.find((x) => x.appId === app.id);
  if (f) void send("retry_job", { jobId: f.id });
}
