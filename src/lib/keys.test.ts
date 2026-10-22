// node --test src/lib/keys.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { appShortcut, focusAfterLeave, modalOpen } from "./keys.ts";

const base = { route: "detail", hasJob: false, failedJobIds: [] as string[], allFailedJobIds: [] as string[] };
const app = (status: string, canRepair = true) => ({ id: "a", status, canRepair });

test("R retries a failed app and repairs one with missing files", () => {
  assert.deepEqual(appShortcut("r", { ...base, app: app("failed"), failedJobIds: ["j1"] }), { do: "retry", jobIds: ["j1"] });
  assert.deepEqual(appShortcut("R", { ...base, app: app("missing") }), { do: "repair", appId: "a" });
  assert.equal(appShortcut("r", { ...base, app: app("missing", false) }), null);
  assert.equal(appShortcut("r", { ...base, app: app("current") }), null);
  assert.equal(appShortcut("r", { ...base, app: app("failed"), hasJob: true, failedJobIds: ["j1"] }), null);
});

test("R on Home retries every failed job", () => {
  assert.deepEqual(appShortcut("r", { ...base, route: "home", app: undefined, allFailedJobIds: ["j1", "j2"] }), { do: "retry", jobIds: ["j1", "j2"] });
  assert.equal(appShortcut("r", { ...base, route: "queue", app: undefined, allFailedJobIds: ["j1"] }), null);
});

test("O opens an installed app but not one with missing files or a running job", () => {
  assert.deepEqual(appShortcut("o", { ...base, app: app("current") }), { do: "open", appId: "a" });
  assert.equal(appShortcut("o", { ...base, app: app("missing") }), null);
  assert.equal(appShortcut("o", { ...base, app: app("current"), hasJob: true }), null);
  assert.equal(appShortcut("o", { ...base, route: "home", app: app("current") }), null);
});

test("the app under the pointer is forgotten only when that app is left", () => {
  assert.equal(focusAfterLeave("a", "a"), null);
  assert.equal(focusAfterLeave("b", "a"), "b");
  assert.equal(focusAfterLeave(null, "a"), null);
});

test("a dialog or an undismissed blocked prompt counts as open", () => {
  assert.equal(modalOpen({ dialog: true, blockedIds: [], dismissed: [] }), true);
  assert.equal(modalOpen({ dialog: false, blockedIds: ["j"], dismissed: [] }), true);
  assert.equal(modalOpen({ dialog: false, blockedIds: ["j"], dismissed: ["j"] }), false);
  assert.equal(modalOpen({ dialog: false, blockedIds: [], dismissed: [] }), false);
});
