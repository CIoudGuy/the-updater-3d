// node --test src/lib/format.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { speed, speedParts } from "./format.ts";
import { failedLine } from "./jobs.ts";

test("a slow download shows KB/s, not 0 MB/s", () => {
  assert.deepEqual(speedParts(300_000), ["300", "KB/s"]);
  assert.equal(speed(300_000), "300 KB/s");
  assert.deepEqual(speedParts(38_400_000), ["38", "MB/s"]);
  assert.deepEqual(speedParts(2_500_000), ["2.5", "MB/s"]);
  assert.deepEqual(speedParts(0), ["0", "MB/s"]);
});

test("a failed row gives the reason, not only the raw code", () => {
  const err = { code: "Error 183", message: "Files in use. Photoshop.exe was running.", detail: "", logPath: null };
  assert.equal(failedLine({ error: err }), "Failed, files in use");
  assert.equal(failedLine({ error: { ...err, message: "" } }), "Failed, Error 183");
  assert.equal(failedLine({ error: null }), "Failed");
});
