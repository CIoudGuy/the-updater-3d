// node --test src/lib/markstate.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { isPainted, markShow } from "./markstate.ts";

const shows = (s: ReturnType<typeof markShow>) => [s.painted, s.placeholder, s.shimmer, s.image];

test("an app without an icon shows the default tile, with a shimmer only while its icon is still looked for", () => {
  // the search is over and found none
  assert.deepEqual(shows(markShow({ id: "other:Some.App", iconUrl: null, iconMiss: true }, "none")), [false, true, false, false]);
  // still looking
  assert.deepEqual(shows(markShow({ id: "other:Some.App", iconUrl: null }, "none")), [false, true, true, false]);
  assert.deepEqual(shows(markShow({ id: "prog:hklm64:key", iconUrl: null }, "none")), [false, true, true, false]);
  assert.deepEqual(shows(markShow({ id: "msstore:Pkg", iconUrl: null }, "none")), [false, true, true, false]);
  // nothing looks for these: a driver without its maker's icon, an app the list shows without one
  assert.deepEqual(shows(markShow({ id: "driver:dev:pci", iconUrl: null }, "none")), [false, true, false, false]);
  assert.deepEqual(shows(markShow({ iconUrl: null }, "none")), [false, true, false, false]);
});

test("a picture that is loading shows the tile with the shimmer, a loaded one only the picture", () => {
  for (const id of ["other:Some.App", "prog:x", "driver:nvidia:1", "queue-job-3"]) {
    assert.deepEqual(shows(markShow({ id, iconUrl: "http://asset.localhost/a.png" }, "loading")), [false, true, true, true], id);
    assert.deepEqual(shows(markShow({ id, iconUrl: "http://asset.localhost/a.png" }, "loaded")), [false, false, false, true], id);
  }
});

test("a picture that cannot be loaded gives back the tile, never a broken image or a shimmer that does not end", () => {
  for (const id of ["other:Some.App", "prog:x", "msstore:Pkg", "driver:amd:1"]) {
    assert.deepEqual(shows(markShow({ id, iconUrl: "http://asset.localhost/gone.png" }, "failed")), [false, true, false, false], id);
  }
  // also when the search said it had an icon and then none
  assert.deepEqual(shows(markShow({ id: "other:Some.App", iconUrl: "x.png", iconMiss: true }, "failed")), [false, true, false, false]);
});

test("apps with a painted brand mark keep it, and the tile never shows for them", () => {
  assert.equal(isPainted("adobe:ILST"), true);
  assert.equal(isPainted("blender:5.2"), true);
  assert.equal(isPainted("craft:vectorcraft"), true);
  assert.equal(isPainted("tools:obs"), true);
  assert.equal(isPainted("other:Adobe.Thing"), false);
  assert.equal(isPainted(undefined), false);
  assert.deepEqual(shows(markShow({ id: "adobe:ILST", iconUrl: null }, "none")), [true, false, false, false]);
  assert.deepEqual(shows(markShow({ id: "adobe:ILST", iconUrl: "a.png" }, "loading")), [true, false, false, true]);
  assert.deepEqual(shows(markShow({ id: "adobe:ILST", iconUrl: "a.png" }, "loaded")), [false, false, false, true]);
  assert.deepEqual(shows(markShow({ id: "blender:5.2", iconUrl: "a.png" }, "failed")), [true, false, false, false]);
});
