// node --test src/lib/jobs.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { jobDetail } from "./jobs.ts";

test("the detail line shows for running and waiting jobs only", () => {
  assert.equal(jobDetail({ state: "downloading", detail: "Downloading over 6 connections from github.com" }), "Downloading over 6 connections from github.com");
  assert.equal(jobDetail({ state: "installing", detail: " Unpacking 1,234 files " }), "Unpacking 1,234 files");
  assert.equal(jobDetail({ state: "paused", detail: "120 MB kept, continues where it stopped" }), "120 MB kept, continues where it stopped");
  // Older backends send no detail.
  assert.equal(jobDetail({ state: "downloading" }), "");
  for (const state of ["done", "failed", "canceled"] as const) assert.equal(jobDetail({ state, detail: "left over" }), "");
});
