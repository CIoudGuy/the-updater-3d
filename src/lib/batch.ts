// Which jobs make up the batch that is running now, for the Home headline and the overall bar.
import type { Job } from "./types";

const OPEN: Job["state"][] = ["waiting", "paused", "blocked", "downloading", "installing", "verifying"];

/**
 * The open jobs plus the finished ones that ran alongside them. Finished jobs stay in the list
 * until cleared, so a job done before this batch started (nothing was open in between) is not
 * part of it. Overlap chains: a job that finished after the batch's earliest start belongs to
 * it, and its own start can pull in jobs that finished earlier still.
 */
export function currentBatch(jobs: Job[]): Job[] {
  const open = jobs.filter((j) => OPEN.includes(j.state));
  if (!open.length) return [];
  const t = (s: string | null) => (s ? Date.parse(s) : NaN);
  const done = jobs.filter((j) => j.state === "done" && !isNaN(t(j.finishedAt)));
  let start = Math.min(Date.now(), ...open.map((j) => t(j.startedAt)).filter((v) => !isNaN(v)));
  const batch = [...open];
  let grew = true;
  while (grew) {
    grew = false;
    for (const j of done) {
      if (!batch.includes(j) && t(j.finishedAt) >= start) {
        batch.push(j);
        const s = t(j.startedAt);
        if (!isNaN(s)) start = Math.min(start, s);
        grew = true;
      }
    }
  }
  return jobs.filter((j) => batch.includes(j));
}
