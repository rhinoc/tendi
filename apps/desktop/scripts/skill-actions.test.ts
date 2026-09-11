import assert from "node:assert/strict";
import test from "node:test";

import { skillActionIds } from "../src/lib/skill-actions.ts";

test("single skill surfaces expose the same complete action set", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 1 }), [
    "visibility",
    "update",
    "open-editor",
    "reveal",
    "copy-path",
    "wrapper",
    "locations",
    "delete",
  ]);
});

test("multi-skill surfaces share batch actions and omit single-path actions", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 2 }), [
    "visibility",
    "update",
    "wrapper",
    "locations",
    "delete",
  ]);
});

test("empty selection has no actions", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 0 }), []);
});
