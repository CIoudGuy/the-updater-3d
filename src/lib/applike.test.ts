// node --test src/lib/applike.test.ts
import { test } from "node:test";
import assert from "node:assert/strict";
import { keepInApp } from "./applike.ts";

test("middle and modifier clicks on links stay in the app", () => {
  const c = (o: Partial<{ button: number; ctrlKey: boolean; shiftKey: boolean; metaKey: boolean }>) => ({ button: 0, ctrlKey: false, shiftKey: false, metaKey: false, ...o });
  assert.equal(keepInApp(c({ button: 1 })), true);
  assert.equal(keepInApp(c({ ctrlKey: true })), true);
  assert.equal(keepInApp(c({ shiftKey: true })), true);
  assert.equal(keepInApp(c({})), false);
});
