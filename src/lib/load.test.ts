// node --test src/lib/load.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { errorText, tryLoad } from "./load.ts";

test("a rejected load becomes an error result instead of staying unresolved", async () => {
  const r = await tryLoad(() => Promise.reject(new Error("offline")));
  assert.deepEqual(r, { ok: false, error: "offline" });
  assert.deepEqual(await tryLoad(() => Promise.reject("no network")), { ok: false, error: "no network" });
  assert.deepEqual(await tryLoad(async () => [1, 2]), { ok: true, value: [1, 2] });
  assert.equal(errorText(undefined), "Something went wrong");
});
