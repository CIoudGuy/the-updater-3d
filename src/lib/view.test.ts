// node --test src/lib/view.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { allSourcesOff, failedCount, failedSentence, groupOrAll, installedNone, aboutLine, isEmptyLibrary, latestColumn, listedOnly, lowDiskWords, splitMissing, timeLeft } from "./view.ts";

test("a source filter whose last app is gone falls back to all", () => {
  assert.equal(groupOrAll("Blender", ["Adobe", "Craft apps"]), "all");
  assert.equal(groupOrAll("Adobe", ["Adobe", "Craft apps"]), "Adobe");
  assert.equal(groupOrAll("all", []), "all");
});

test("an Other program that names its GitHub project is updated like Other apps", () => {
  assert.equal(listedOnly({ source: "programs", github: null }), true);
  assert.equal(listedOnly({ source: "programs" }), true);
  assert.equal(listedOnly({ source: "programs", github: "CIoudGuy/tidalfast" }), false);
  assert.equal(listedOnly({ source: "other", github: null }), false);
});

test("an app's page shows its short description, not a publisher or a driver kind", () => {
  assert.equal(aboutLine({ source: "programs", github: "o/chat-on-steroids", tagline: "ChatGPT desktop client with projects", note: null }), "ChatGPT desktop client with projects");
  assert.equal(aboutLine({ source: "programs", github: null, tagline: "Mega Limited", note: null }), null);
  assert.equal(aboutLine({ source: "drivers", tagline: "Chipset from AMD", note: null }), null);
  assert.equal(aboutLine({ source: "blender", tagline: "Updated by Steam", note: "Updated by Steam", readOnly: true }), null);
  assert.equal(aboutLine({ source: "other", tagline: "A free source code editor", note: null }), "A free source code editor");
  assert.equal(aboutLine({ source: "other", tagline: "  ", note: null }), null);
});

test("an app whose updates are off shows the version it has, not the newer one", () => {
  const app = { source: "other", status: "current", latest: "2.0", version: "1.0" };
  assert.equal(latestColumn(app, true), "1.0");
  assert.equal(latestColumn(app, false), "2.0");
  assert.equal(latestColumn({ ...app, status: "update" }, false), "2.0");
  assert.equal(latestColumn({ ...app, source: "programs", github: null }, false), "");
  assert.equal(latestColumn({ ...app, source: "programs", github: "o/r", status: "update" }, false), "2.0");
});

test("a search that only the Updates filter hides says so and offers to show all", () => {
  const r = installedNone({ query: "photoshop", filter: "updates", matchesIgnoringFilter: 1 });
  assert.match(r.text, /hidden by the Updates available filter/);
  assert.equal(r.showAll, true);
  const none = installedNone({ query: "zzz", filter: "updates", matchesIgnoringFilter: 0 });
  assert.equal(none.text, 'No apps match "zzz".');
  assert.equal(none.showAll, false);
  assert.equal(installedNone({ query: "", filter: "all", matchesIgnoringFilter: 0 }).text, "Nothing here yet.");
});

test("failed uninstalls and repairs are not called failed installs or updates", () => {
  assert.equal(failedSentence({ kind: "uninstall", name: "Krita", toVersion: "5", fromVersion: "5" }), "Krita 5 could not be uninstalled.");
  assert.equal(failedSentence({ kind: "repair", name: "Krita", toVersion: "5", fromVersion: null }), "Krita 5 could not be repaired.");
  assert.equal(failedSentence({ kind: "update", name: "Krita", toVersion: "6", fromVersion: "5" }), "Krita 6 could not update.");
  assert.equal(failedSentence({ kind: "install", name: "Krita", toVersion: "6", fromVersion: null }), "Krita 6 could not install.");
  assert.equal(failedCount([{ kind: "uninstall" }]), "1 uninstall");
  assert.equal(failedCount([{ kind: "repair" }, { kind: "repair" }]), "2 repairs");
  assert.equal(failedCount([{ kind: "repair" }, { kind: "update" }]), "2 jobs");
});

test("low disk dialog grammar", () => {
  const many = lowDiskWords(Array.from({ length: 10 }, (_, i) => `App ${i}`), "update");
  assert.equal(many.eyebrow, "Before these 10 updates");
  assert.equal(many.name, "10 updates");
  assert.equal(lowDiskWords(["A", "B", "C"], "install").eyebrow, "Before these 3 installs");
  assert.equal(many.needs, "need");
  const two = lowDiskWords(["A", "B"], "update");
  assert.equal(two.needs, "need");
  const one = lowDiskWords(["Blender"], "install");
  assert.equal(one.eyebrow, "Before installing Blender");
  assert.equal(one.needs, "needs");
});

test("time left says why there is no estimate", () => {
  const approx = (s: number) => `About ${Math.round(s / 60)} min`;
  assert.equal(timeLeft(600, true, approx), "About 10 min");
  assert.equal(timeLeft(null, false, approx), "Nothing downloading");
  assert.equal(timeLeft(null, true, approx), "Working it out");
});

test("empty library and all sources off", () => {
  assert.equal(isEmptyLibrary(0), true);
  assert.equal(isEmptyLibrary(3), false);
  assert.equal(allSourcesOff({ adobe: false, craft: false, blender: false, tools: false, other: false }), true);
  assert.equal(allSourcesOff({ adobe: false, craft: true, blender: false, tools: false, other: false }), false);
});

test("the Drivers filter says when no drivers are listed and names itself when it hides a match", () => {
  const empty = installedNone({ query: "", filter: "drivers", matchesIgnoringFilter: 0 });
  assert.match(empty.text, /No drivers listed yet/);
  assert.equal(empty.showAll, true);
  assert.match(installedNone({ query: "blender", filter: "drivers", matchesIgnoringFilter: 2 }).text, /hidden by the Drivers filter/);
});

test("removed programs that left their entry are leftovers, not apps to repair", () => {
  // Seen: 14 removed games (Black Myth: Wukong, Far Cry 6...) showed as "14 apps need repair".
  const apps = [
    { id: "prog:wukong", status: "missing", source: "programs" },
    { id: "adobe:PHSP", status: "missing", source: "adobe" },
    { id: "other:7zip.7zip", status: "current", source: "other" },
  ];
  const { repair, leftovers } = splitMissing(apps);
  assert.deepEqual(repair.map((a) => a.id), ["adobe:PHSP"]);
  assert.deepEqual(leftovers.map((a) => a.id), ["prog:wukong"]);
});
