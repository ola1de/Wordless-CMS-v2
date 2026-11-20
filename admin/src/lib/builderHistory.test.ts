// Node tests for the builder undo/redo history (P11). Run with:
//   node --test admin/src/lib/builderHistory.test.ts
// (Node 24 type-strips .ts files; this module uses only erasable
// syntax, so no build step is needed.)
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  canRedo,
  canUndo,
  createHistory,
  historyPush,
  historyRedo,
  historyUndo,
} from "./builderHistory.ts";

test("undo/redo walks the stack and clears redo on push", () => {
  let h = createHistory(["a"]);
  assert.equal(canUndo(h), false);
  assert.equal(canRedo(h), false);
  h = historyPush(h, ["a", "b"]);
  h = historyPush(h, ["a", "b", "c"]);
  assert.equal(canUndo(h), true);
  h = historyUndo(h);
  assert.deepEqual(h.present, ["a", "b"]);
  assert.equal(canRedo(h), true);
  h = historyUndo(h);
  assert.deepEqual(h.present, ["a"]);
  assert.equal(canUndo(h), false);
  h = historyRedo(h);
  assert.deepEqual(h.present, ["a", "b"]);
  // A new push clears the redo stack.
  h = historyPush(h, ["a", "b", "x"]);
  assert.equal(canRedo(h), false);
  assert.deepEqual(h.present, ["a", "b", "x"]);
});

test("undo/redo at the ends is a no-op", () => {
  const h = createHistory(1);
  assert.equal(historyUndo(h), h);
  assert.equal(historyRedo(h), h);
});

test("past is capped at 100 entries", () => {
  let h = createHistory(0);
  for (let i = 1; i <= 150; i++) h = historyPush(h, i);
  assert.equal(h.past.length, 100);
  assert.equal(h.present, 150);
});
