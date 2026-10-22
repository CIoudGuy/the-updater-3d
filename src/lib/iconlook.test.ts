// node --test src/lib/iconlook.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { mostlyDark } from "./iconlook.ts";

const px = (c: number[], n: number) => Array.from({ length: n }, () => c).flat();

test("a dark logo on a clear background gets the light plate, other icons do not", () => {
  // Spotify's: a black circle, the rest clear
  assert.equal(mostlyDark([...px([0, 0, 0, 255], 600), ...px([0, 0, 0, 0], 400)]), true);
  // a dark tile with a white sign on it
  assert.equal(mostlyDark([...px([20, 20, 30, 255], 800), ...px([255, 255, 255, 255], 200)]), false);
  // a green logo
  assert.equal(mostlyDark(px([30, 215, 96, 255], 100)), false);
  // nothing visible
  assert.equal(mostlyDark(px([0, 0, 0, 0], 100)), false);
});
