import assert from "node:assert/strict";
import test from "node:test";

import {
  configSelectionActionIds,
  hookSelectionActionIds,
  mcpSelectionActionIds,
  promptSelectionActionIds,
  ruleSelectionActionIds,
  visibleSelectionActionCount,
} from "../src/lib/table-selection-actions.ts";

test("bulk-capable tabs expose only operations that support multiple rows", () => {
  assert.deepEqual(hookSelectionActionIds(2), ["enable", "disable", "delete"]);
  assert.deepEqual(mcpSelectionActionIds(2), ["enable", "disable"]);
  assert.deepEqual(promptSelectionActionIds(2), ["delete"]);
  assert.deepEqual(ruleSelectionActionIds(2), ["delete"]);
  assert.deepEqual(configSelectionActionIds(2), ["delete"]);
});

test("single-row tabs include their row actions in the shared surface", () => {
  assert.deepEqual(hookSelectionActionIds(1), ["open-editor", "reveal", "copy-path", "enable", "disable", "delete"]);
  assert.deepEqual(mcpSelectionActionIds(1), ["enable", "disable", "open-editor", "reveal", "copy-path"]);
  assert.deepEqual(promptSelectionActionIds(1), ["copy", "edit", "delete"]);
  assert.deepEqual(ruleSelectionActionIds(1), ["open-editor", "reveal", "copy-path", "delete"]);
  assert.deepEqual(configSelectionActionIds(1), ["open-editor", "reveal", "copy-path", "delete"]);
});

test("empty selection has no actions", () => {
  assert.deepEqual(hookSelectionActionIds(0), []);
  assert.deepEqual(mcpSelectionActionIds(0), []);
  assert.deepEqual(promptSelectionActionIds(0), []);
  assert.deepEqual(ruleSelectionActionIds(0), []);
  assert.deepEqual(configSelectionActionIds(0), []);
});

test("toolbar keeps actions in the overflow menu when the available width is too small", () => {
  assert.equal(visibleSelectionActionCount(320, [100, 100, 100], 28), 3);
  assert.equal(visibleSelectionActionCount(240, [100, 100, 100], 28), 2);
  assert.equal(visibleSelectionActionCount(150, [100, 100, 100], 28), 1);
  assert.equal(visibleSelectionActionCount(20, [100, 100, 100], 28), 0);
});
