// node --test src/lib/browse.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { catalogNote, datedNote, LETTERS, matchRank, mergeEntries, pagesToLoad, patchEntries, patchIcons, placePage, rankEntries, sameKey, sortOptions, windowRows } from "./browse.ts";
import type { CatalogEntry } from "./types.ts";

const e = (id: string, extra: Partial<CatalogEntry> = {}): CatalogEntry => ({
  id, source: "other", name: id, code: "Ab", fg: "#fff", bg: "#000", iconUrl: null, version: "1.0", size: null,
  tagline: id.slice(6), description: null, blocked: null, group: "Other apps", prerelease: false, installed: false, pickVersion: true, ...extra,
});

test("the catalog size is written with separators", () => {
  assert.equal(catalogNote(15724), "15,724 apps from Microsoft's winget catalog.");
  assert.equal(catalogNote(1), "1 app from Microsoft's winget catalog.");
});

test("letters cover A to Z and the rest", () => {
  assert.equal(LETTERS.length, 27);
  assert.equal(LETTERS[0], "#");
});

test("details patch only their own cards and keep a known tagline when they have none", () => {
  const list = [e("other:A.A"), e("other:B.B")];
  const out = patchEntries(list, [
    { id: "other:B.B", tagline: "Archiver", description: "Longer", blocked: null },
    { id: "other:Z.Z", tagline: "x", description: null, blocked: null },
  ])!;
  assert.equal(out[0], list[0]);
  assert.deepEqual([out[1].tagline, out[1].description, out[1].blocked], ["Archiver", "Longer", null]);
  const out2 = patchEntries(out, [{ id: "other:A.A", tagline: null, description: null, blocked: "only a portable copy" }])!;
  assert.deepEqual([out2[0].tagline, out2[0].blocked], ["A.A", "only a portable copy"]);
  assert.equal(patchEntries(out2, [{ id: "other:B.B", tagline: "Archiver", description: "Longer", blocked: null }]), null);
});

test("a page goes in its own places and the list spans every entry from the start", () => {
  const one = placePage([], 7, 0, [e("other:A"), e("other:B")]);
  assert.equal(one.length, 7);
  assert.deepEqual(one.map((x) => x?.id ?? null), ["other:A", "other:B", null, null, null, null, null]);
  // a page far down the list, loaded before the ones between
  const two = placePage(one, 7, 5, [e("other:F"), e("other:G"), e("other:H")]);
  assert.deepEqual(two.map((x) => x?.id ?? null), ["other:A", "other:B", null, null, null, "other:F", "other:G"]);
  assert.equal(two[0], one[0]);
  // the list got shorter on the backend
  assert.equal(placePage(two, 3, 0, []).length, 3);
});

test("only pages with unloaded places in view are asked for, nearest first", () => {
  const list = placePage([], 10, 0, [e("other:A"), e("other:B"), e("other:C")]);
  // pages of 3: [0..3) loaded, [3..6) [6..9) [9..10) not
  assert.deepEqual(pagesToLoad(list, 0, 3, 3), []);
  assert.deepEqual(pagesToLoad(list, 1, 8, 3), [3, 6]);
  assert.deepEqual(pagesToLoad(list, 1, 8, 3, new Set([3])), [6]);
  assert.deepEqual(pagesToLoad(list, 8, 50, 3), [6, 9]);
  assert.deepEqual(pagesToLoad(list, -5, 2, 3), []);
});

test("found icons patch their cards and nothing else", () => {
  const list = [e("other:A"), null, e("other:B")];
  const out = patchIcons(list, [{ id: "other:B", iconUrl: "b.png" }, { id: "other:Z", iconUrl: "z.png" }])!;
  assert.equal(out[0], list[0]);
  assert.equal(out[1], null);
  assert.equal(out[2]?.iconUrl, "b.png");
  assert.equal(patchIcons(out, [{ id: "other:B", iconUrl: "b.png" }]), null);
});

test("the grid keeps the rows in view and a margin around them, from fixed row heights", () => {
  // 100 rows of 200 px, a view of 600 px, 400 px kept above and below
  assert.deepEqual(windowRows(0, 600, 200, 100, 400), { first: 0, last: 5 });
  assert.deepEqual(windowRows(2000, 600, 200, 100, 400), { first: 8, last: 15 });
  // the grid still lower down than the view
  assert.deepEqual(windowRows(-1000, 600, 200, 100, 400), { first: 0, last: 0 });
  // flung to the very end, or past it
  assert.deepEqual(windowRows(19800, 600, 200, 100, 400), { first: 97, last: 100 });
  assert.deepEqual(windowRows(99999, 600, 200, 100, 400), { first: 100, last: 100 });
  assert.deepEqual(windowRows(0, 600, 200, 0, 400), { first: 0, last: 0 });
});

test("curated entries rank like the catalog: exact, start, word starts, inside, line", () => {
  assert.equal(matchRank("Blender 4.5 LTS", null, "blender 4.5 lts"), 0);
  assert.equal(matchRank("7-Zip", null, "7zip"), 0);
  assert.equal(matchRank("Blender 4.5 LTS", null, "blend"), 1);
  assert.equal(matchRank("Adobe Photoshop", null, "photo"), 2);
  assert.equal(matchRank("Adobe Photoshop", null, "shop"), 3);
  assert.equal(matchRank("Krita", "Digital painting", "paint"), 4);
  assert.equal(matchRank("Krita", "Digital painting", "obs"), null);
  assert.equal(matchRank("Krita", null, " -- "), null);
});

test("search results keep the best first and equal ones in order", () => {
  const list = [e("other:A.Shop", { name: "Adobe Photoshop" }), e("other:P", { name: "Photo Tool" }), e("other:X", { name: "Unrelated" }), e("other:Pe", { name: "Photo" })];
  assert.deepEqual(rankEntries(list, "photo").map((x) => x.id), ["other:Pe", "other:P", "other:A.Shop"]);
});

test("icons found later are merged in without changing the list", () => {
  const have = [e("other:A"), e("other:B"), e("other:C")];
  const out = mergeEntries(have, [e("other:B", { iconUrl: "x.png" }), e("other:Z")]);
  assert.deepEqual(out.map((x) => x?.id), ["other:A", "other:B", "other:C"]);
  assert.equal(out[1]?.iconUrl, "x.png");
  assert.equal(out[0], have[0]);
});

test("a list belongs to its search, filters and order", () => {
  const k = { query: "vlc", category: null, letter: null, sort: "popular" as const };
  assert.ok(sameKey(k, { ...k }));
  assert.ok(!sameKey(k, { ...k, sort: "az" }));
  assert.ok(!sameKey(k, { ...k, query: "vl" }));
  assert.ok(!sameKey(k, { ...k, category: "Media" }));
});

test("orders and the note about release dates", () => {
  assert.equal(sortOptions(false)[0].label, "Popular first");
  assert.equal(sortOptions(true)[0].label, "Best match");
  assert.deepEqual(sortOptions(false).map((o) => o.value), ["popular", "az", "za", "updated"]);
  assert.equal(datedNote(10, 10), "Newest release first.");
  assert.match(datedNote(1234, 15724), /known for 1,234 of 15,724 apps so far/);
  assert.match(datedNote(0, 5), /No release dates are known yet/);
});
