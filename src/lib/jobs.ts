import { bytes, bytesParts, speed as fmtSpeed, unitFor } from "./format";
import type { Job } from "./types";

/** "Downloading 1.3 of 2.9 GB at 22 MB/s", "Installing, step 3 of 4", "Waiting" */
export function jobStatus(j: Job, withSpeed = true): string {
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
