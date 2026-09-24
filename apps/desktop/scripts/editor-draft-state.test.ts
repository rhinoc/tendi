import assert from "node:assert/strict";
import test from "node:test";
import {
  clearEditorDraft,
  discardEditorDraft,
  getEditorDraft,
  releaseCleanEditorDraft,
  updateEditorDraft,
} from "../src/lib/editor-draft-state.ts";

test("editor drafts are isolated by resource and can be discarded", () => {
  const first = `editor-draft-first-${Date.now()}`;
  const second = `${first}-second`;
  const dirty = { content: "edited", originalContent: "original", sha256: "sha" };

  updateEditorDraft(first, dirty);
  assert.deepEqual(getEditorDraft(first), dirty);
  assert.deepEqual(getEditorDraft(second), { content: "", originalContent: "", sha256: "" });

  discardEditorDraft(first);
  assert.deepEqual(getEditorDraft(first), { ...dirty, content: "original" });
  clearEditorDraft(first);
  assert.deepEqual(getEditorDraft(first), { content: "", originalContent: "", sha256: "" });
});

test("releasing a resource drops clean content but retains unsaved edits", () => {
  const cleanKey = `editor-draft-clean-${Date.now()}`;
  const dirtyKey = `${cleanKey}-dirty`;
  const clean = { content: "disk", originalContent: "disk", sha256: "sha" };
  const dirty = { content: "edited", originalContent: "disk", sha256: "sha" };
  updateEditorDraft(cleanKey, clean);
  updateEditorDraft(dirtyKey, dirty);

  releaseCleanEditorDraft(cleanKey);
  releaseCleanEditorDraft(dirtyKey);

  assert.deepEqual(getEditorDraft(cleanKey), { content: "", originalContent: "", sha256: "" });
  assert.deepEqual(getEditorDraft(dirtyKey), dirty);
});
