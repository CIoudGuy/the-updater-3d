// node --test src/lib/format.version.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { shortVersion } from "./format.ts";

test("catalog versions with build hashes or long labels read cleanly", () => {
  assert.equal(shortVersion("1.3.6.1921.g7086c071a617"), "1.3.6.1921");
  assert.equal(shortVersion("1.0.9329126-release.Release - Prod Desktop App_x64"), "1.0.9329126");
});

test("normal versions and short tags stay as they are", () => {
  for (const v of ["1.12.0", "6000.3.27f1", "2.0.0-beta.3", "5.2 LTS", "26.04", "4.7.2", "1.0.108.2970"]) {
    assert.equal(shortVersion(v), v);
  }
});
