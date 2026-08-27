import assert from "node:assert/strict";
import test from "node:test";

import { skillActionIds } from "../src/lib/skill-actions.ts";

test("single skill surfaces expose the same complete action set", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 1, backupConfigured: true }), [
    "open-editor",
    "locations",
    "update",
    "reveal",
    "copy-path",
    "visibility",
    "backup",
    "wrapper",
    "delete",
  ]);
});

test("multi-skill surfaces share batch actions and omit single-path actions", () => {
  assert.deepEqual(skillActionIds({ selectionCount: 2, backupConfigured: true }), [
    "locations",
    "update",
    "visibility",
    "backup",
    "wrapper",
    "delete",
  ]);
});

test("backup is absent from every surface when it is not configured", () => {
  assert.equal(skillActionIds({ selectionCount: 1, backupConfigured: false }).includes("backup"), false);
  assert.equal(skillActionIds({ selectionCount: 2, backupConfigured: false }).includes("backup"), false);
});
