// node --test src/lib/batch.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { currentBatch } from "./batch.ts";
import type { Job } from "./types";

const at = (min: number) => new Date(Date.UTC(2020, 0, 1, 5, min)).toISOString();
function job(id: string, state: Job["state"], started: number | null, finished: number | null): Job {
  return {
    id, appId: id, name: id, code: "", fg: "", bg: "", iconUrl: null, kind: "update", fromVersion: null, toVersion: "1",
    state, downloaded: 0, total: 0, speed: 0, eta: null, progress: 0, step: "", blockedBy: [], error: null,
    startedAt: started == null ? null : at(started), finishedAt: finished == null ? null : at(finished),
  };
}
const ids = (js: Job[]) => js.map((j) => j.id).sort();

test("jobs finished before this batch started are not counted", () => {
  // Three updates done earlier, then one new update on its own: "Updating 1 app".
  const jobs = [job("a", "done", 1, 3), job("b", "done", 2, 4), job("c", "done", 4, 6), job("d", "downloading", 20, null)];
  assert.deepEqual(ids(currentBatch(jobs)), ["d"]);
});

test("jobs that ran alongside the open ones are counted", () => {
  // a and b start together, a finishes and c starts, b finishes and d starts: one batch of four.
  const jobs = [job("a", "done", 0, 5), job("b", "done", 0, 8), job("c", "done", 5, 12), job("d", "installing", 8, null)];
  assert.deepEqual(ids(currentBatch(jobs)), ["a", "b", "c", "d"]);
});

test("failed and canceled jobs and an idle list give no batch", () => {
  assert.deepEqual(currentBatch([job("a", "done", 0, 5)]), []);
  const jobs = [job("a", "failed", 10, 11), job("b", "canceled", 10, 12), job("c", "waiting", null, null)];
  assert.deepEqual(ids(currentBatch(jobs)), ["c"]);
});
