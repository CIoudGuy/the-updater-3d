// node --test src/lib/browse.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { appendPage, catalogNote, LETTERS, patchEntries } from "./browse.ts";
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

test("pages are added without repeating entries", () => {
  assert.deepEqual(appendPage([e("other:A.A")], [e("other:A.A"), e("other:B.B")]).map((x) => x.id), ["other:A.A", "other:B.B"]);
});
