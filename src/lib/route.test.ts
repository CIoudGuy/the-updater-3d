// node --test src/lib/route.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { landing } from "./route.ts";

// The routes the backend builds for a clicked notification (notice.rs, Target::route).
const ROUTES = {
  app: (id: string) => "installed/" + encodeURIComponent(id),
  updates: "updates",
  installed: "installed",
  queue: "queue",
  home: "home",
  about: "settings/about",
};

test("a notification about one app opens that app's page", () => {
  assert.deepEqual(landing(ROUTES.app("adobe:PHSP")), { path: "installed/adobe%3APHSP" });
  // the same text the Rust test pins for this id
  assert.equal(ROUTES.app("driver:PCI\\VEN_10DE&DEV_2684 x/y"), "installed/driver%3APCI%5CVEN_10DE%26DEV_2684%20x%2Fy");
  // the router splits the path and decodes each part: the id comes back whole
  const parts = landing(ROUTES.app("driver:PCI\\VEN_10DE&DEV_2684 x/y")).path.split("/").map(decodeURIComponent);
  assert.deepEqual(parts, ["installed", "driver:PCI\\VEN_10DE&DEV_2684 x/y"]);
});

test("a notification about several updates opens Installed with the Updates filter", () => {
  assert.deepEqual(landing(ROUTES.updates), { path: "installed", filter: "updates" });
});

test("the other notifications open Installed, the Queue, Home and Settings > About", () => {
  assert.deepEqual(landing(ROUTES.installed), { path: "installed" });
  assert.deepEqual(landing(ROUTES.queue), { path: "queue" });
  assert.deepEqual(landing(ROUTES.home), { path: "home" });
  assert.deepEqual(landing(ROUTES.about), { path: "settings/about" });
});

test("the tray's Update all opens Home and starts the update", () => {
  assert.deepEqual(landing("update-all"), { path: "home", updateAll: true });
});

test("the tray's Settings button and routes with a leading # or / still work", () => {
  assert.deepEqual(landing("settings"), { path: "settings" });
  assert.deepEqual(landing("#/queue"), { path: "queue" });
  assert.deepEqual(landing("/queue"), { path: "queue" });
  assert.deepEqual(landing("#updates"), { path: "installed", filter: "updates" });
});

test("only the exact word 'updates' means the filter, an app named that way is an app", () => {
  assert.deepEqual(landing("installed/updates"), { path: "installed/updates" });
});
