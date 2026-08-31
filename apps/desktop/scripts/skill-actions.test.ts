import assert from "node:assert/strict";
import test from "node:test";

import { skillActionIds } from "../src/lib/skill-actions.ts";

test("single skill surfaces expose the same complete action set", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 1 }), [
    "open-editor",
    "locations",
    "update",
    "reveal",
    "copy-path",
    "visibility",
    "wrapper",
    "delete",
  ]);
});

test("multi-skill surfaces share batch actions and omit single-path actions", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 2 }), [
    "locations",
    "update",
    "visibility",
    "wrapper",
    "delete",
  ]);
});

test("empty selection has no actions", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 0 }), []);
});
