// node --test src/lib/blocked.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { blockedText } from "./blocked.ts";

const job = (kind: "update" | "uninstall") => ({ kind, name: "Superposition", toVersion: "1.1", fromVersion: "1.1", blockedBy: ["launcher"] });

test("an uninstall waiting for the app to close does not talk about installing an update", () => {
  const t = blockedText(job("uninstall"), 0);
  const all = `${t.title} ${t.lead} ${t.info} ${t.action}`;
  assert.doesNotMatch(all, /update|downloaded|ready to install|\binstalls\b/i);
  assert.match(t.title, /uninstall/);
  assert.equal(t.action, "Close and uninstall");
});

test("an update keeps its wording", () => {
  const t = blockedText(job("update"), 1);
  assert.match(t.lead, /ready to install/);
  assert.match(t.lead, /other updates keep going/);
  assert.equal(t.action, "Close and update");
  assert.equal(t.info, "Installs by itself when launcher closes");
});
