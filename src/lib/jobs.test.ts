// node --test src/lib/jobs.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { failedLine, finishedLine, groupQueue, jobDetail, jobStatus, queueGroup, queueSections, queueSummary, readyLine } from "./jobs.ts";
import type { Job } from "./types";

test("a failure reads as one sentence and names keep their capitals", () => {
  const line = (message: string) => failedLine({ error: { code: "Error 1603", message, detail: "", logPath: null } });
  assert.equal(line("Windows Installer hit a fatal error. The log has the details."), "Failed, Windows Installer hit a fatal error");
  assert.equal(line("Photoshop is running. Close it."), "Failed, Photoshop is running");
  assert.equal(line("OBS Studio is running."), "Failed, OBS Studio is running");
  assert.equal(line("The installer finished, but Krita reports 5.2 instead of 5.3"), "Failed, the installer finished, but Krita reports 5.2 instead of 5.3");
  assert.equal(line("Another app is using its files."), "Failed, another app is using its files");
  assert.equal(line("Files are in use."), "Failed, files are in use");
  assert.equal(failedLine({ error: { code: "Error 5", message: " ", detail: "", logPath: null } }), "Failed, Error 5");
});

test("a job that ended with nothing done says why, without Failed or Retry wording", () => {
  assert.equal(finishedLine({ state: "canceled", step: "Adobe has no update for this PC yet", error: null }), "Adobe has no update for this PC yet");
  assert.equal(finishedLine({ state: "canceled", step: "Canceled", error: null }), "Canceled");
  assert.equal(finishedLine({ state: "canceled", step: "", error: null }), "Canceled");
  assert.equal(finishedLine({ state: "done", step: "Done", error: null }), "Finished");
  assert.equal(finishedLine({ state: "failed", step: "x", error: null }), "Failed");
});

test("the detail line shows for running and waiting jobs only", () => {
  assert.equal(jobDetail({ state: "downloading", detail: "Downloading over 6 connections from github.com" }), "Downloading over 6 connections from github.com");
  assert.equal(jobDetail({ state: "installing", detail: " Unpacking 1,234 files " }), "Unpacking 1,234 files");
  assert.equal(jobDetail({ state: "paused", detail: "120 MB kept, continues where it stopped" }), "120 MB kept, continues where it stopped");
  // Older backends send no detail.
  assert.equal(jobDetail({ state: "downloading" }), "");
  for (const state of ["done", "failed", "canceled"] as const) assert.equal(jobDetail({ state, detail: "left over" }), "");
});

const J = (id: string, state: Job["state"], extra: Partial<Job> = {}) =>
  ({ id, name: id, state, ready: false, cancelable: false, detail: "", step: "", downloaded: 0, total: 0, speed: 0, ...extra }) as Job;

test("downloaded jobs waiting for their install turn get their own section", () => {
  const jobs = [
    J("Photoshop", "installing"),
    ...Array.from({ length: 21 }, (_, i) => J(`Ready ${i + 1}`, "installing", { ready: true, cancelable: true, step: "Waiting for another install to finish" })),
    J("Blender", "downloading"),
    J("Krita", "paused"),
    J("OBS", "waiting"),
    J("Old", "done"),
  ];
  const g = groupQueue(jobs);
  assert.deepEqual(g.installing.map((j) => j.id), ["Photoshop"]);
  assert.equal(g.ready.length, 21);
  assert.deepEqual(g.downloading.map((j) => j.id), ["Blender", "Krita"]);
  assert.deepEqual(g.waiting.map((j) => j.id), ["OBS"]);
  assert.equal(queueSummary(g), "1 installing, 21 ready to install, 2 downloading, 1 waiting");
  assert.equal(queueSummary(groupQueue([])), "");
});

test("a job waiting for an app to close holds the install turn", () => {
  assert.equal(queueGroup(J("a", "blocked")), "installing");
  assert.equal(queueGroup(J("a", "verifying")), "installing");
  assert.equal(queueGroup(J("a", "failed")), null);
});

test("ready jobs say what they install after, and nothing looks stuck", () => {
  const g = groupQueue([J("Photoshop", "installing"), J("Blender", "installing", { ready: true, cancelable: true }), J("Krita", "installing", { ready: true, cancelable: true })]);
  assert.equal(readyLine(g, 0), "Downloaded, installs after Photoshop");
  assert.equal(readyLine(g, 1), "Downloaded, installs after Blender");
  // Only Creative Cloud is busy (that job let the turn go): the next one goes next.
  const cc = groupQueue([J("Illustrator", "installing", { cancelable: true }), J("Blender", "installing", { ready: true, cancelable: true })]);
  assert.equal(readyLine(cc, 0), "Downloaded, installs next");
  const r = J("Blender", "installing", { ready: true, cancelable: true, step: "Waiting for another install to finish", detail: "Photoshop is installing now" });
  assert.equal(jobStatus(r, false), "Downloaded, ready to install");
  assert.equal(jobDetail(r), "");
});

test("sections without jobs are left out, the rest keep the Queue order", () => {
  const g = groupQueue([J("Blender", "waiting"), J("Krita", "downloading"), J("OBS", "installing")]);
  assert.deepEqual(queueSections(g).map((x) => [x.label, x.jobs.map((j) => j.id)]), [
    ["Installing now", ["OBS"]],
    ["Downloading", ["Krita"]],
    ["Waiting", ["Blender"]],
  ]);
});
