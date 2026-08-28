import assert from "node:assert/strict";
import test from "node:test";

import { actionLabels, copiedPathLabel, copiedValueLabel, copyPathLabel, copyValueLabel, deleteConfirmationDescription, logExportLabels, promptActionLabels, revealPathLabel, selectionCopiedLabel, selectionCopyLabel, selectionDeleteErrorLabel, selectionDeleteLabel, selectionDeleteLoadingLabel } from "../src/lib/action-labels.ts";

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
    checkForUpdates: "Check for updates",
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
  assert.equal(revealPathLabel("workspace"), "Reveal workspace in Finder");
  assert.equal(promptActionLabels.saving, "Saving prompt");
  assert.equal(promptActionLabels.saved, "Prompt saved");
  assert.equal(promptActionLabels.saveFailed, "Could not save prompt.");
  assert.equal(selectionCopiedLabel("prompt", 1), "Prompt copied");
  assert.equal(selectionCopiedLabel("prompt", 2), "Selected prompts copied");
  assert.equal(selectionDeleteLabel("rule", 1), "Delete rule");
  assert.equal(selectionDeleteLabel("rule", 2), "Delete selected rules");
  assert.equal(selectionDeleteLoadingLabel("hook", 1), "Deleting hook");
  assert.equal(selectionDeleteLoadingLabel("hook", 2), "Deleting hooks");
  assert.equal(selectionDeleteErrorLabel("rule", 1), "Could not delete rule.");
  assert.equal(selectionDeleteErrorLabel("rule", 2), "Could not delete rules.");
  assert.equal(deleteConfirmationDescription("config file", 1), "Delete the selected config file? This action cannot be undone.");
  assert.equal(deleteConfirmationDescription("config file", 2), "Delete the selected config files? This action cannot be undone.");
  assert.deepEqual(logExportLabels, {
    idle: "Export logs",
    loading: "Exporting logs",
    success: "Logs exported",
    error: "Export failed",
    retry: "Export logs again",
  });
});
