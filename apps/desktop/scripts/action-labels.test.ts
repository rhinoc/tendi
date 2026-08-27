import assert from "node:assert/strict";
import test from "node:test";

import { actionLabels, copiedPathLabel, copiedValueLabel, copyPathLabel, copyValueLabel, selectionCopiedLabel, selectionCopyLabel, selectionDeleteLabel } from "../src/lib/action-labels.ts";

test("keeps shared file action labels in one source", () => {
  assert.deepEqual(actionLabels, {
    openInEditor: "Open in editor",
    revealInFinder: "Reveal in Finder",
    copyPath: "Copy path",
    pathCopied: "Path copied",
    deleteSelected: "Delete selected",
    copy: "Copy",
    copied: "Copied",
    copyFailed: "Copy failed",
    saveFailed: "Save failed",
    enable: "Enable",
    disable: "Disable",
  });
});

test("builds consistent single and multi-selection labels", () => {
  assert.equal(selectionCopyLabel("prompt", 1), "Copy prompt");
  assert.equal(selectionCopyLabel("prompt", 2), "Copy selected prompts");
  assert.equal(copyValueLabel("workspace"), "Copy workspace");
  assert.equal(copiedValueLabel("workspace"), "Workspace copied");
  assert.equal(copyPathLabel("transcript"), "Copy transcript path");
  assert.equal(copiedPathLabel("transcript"), "Transcript path copied");
  assert.equal(selectionCopiedLabel("prompt", 1), "Prompt copied");
  assert.equal(selectionCopiedLabel("prompt", 2), "Selected prompts copied");
  assert.equal(selectionDeleteLabel("rule", 1), "Delete rule");
  assert.equal(selectionDeleteLabel("rule", 2), "Delete selected rules");
});
