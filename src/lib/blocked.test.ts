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

test("apps holding the files are named, because the button closes them and not the app", () => {
  const t = blockedText({ kind: "update", name: "OBS Studio", toVersion: "33.0.1", fromVersion: "33.0.0", blockedBy: ["Discord", "Chrome"], blockedByFiles: true }, 3);
  assert.equal(t.title, "Close Discord, Chrome to update OBS Studio");
  assert.match(t.lead, /Discord, Chrome are using OBS Studio's files/);
  assert.equal(t.info, "Installs by itself when Discord, Chrome close");
  assert.equal(t.action, "Close and update");
  const one = blockedText({ kind: "update", name: "OBS Studio", toVersion: "33.0.1", fromVersion: "33.0.0", blockedBy: ["Chrome"], blockedByFiles: true }, 0);
  assert.match(one.lead, /Chrome is using/);
  assert.equal(one.info, "Installs by itself when Chrome closes");
});

test("an update keeps its wording", () => {
  const t = blockedText(job("update"), 1);
  assert.match(t.lead, /ready to install/);
  assert.match(t.lead, /other updates keep going/);
  assert.equal(t.action, "Close and update");
  assert.equal(t.info, "Installs by itself when launcher closes");
});
