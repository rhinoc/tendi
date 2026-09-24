import assert from "node:assert/strict";
import test from "node:test";
import {
  discardCleanDrafts,
  hydrateEditorDraft,
  reconcileExternalEditorDraft,
  retainActiveAndDirtyDrafts,
  type EditorDraft,
} from "../src/lib/editor-draft-logic.ts";

test("clean snapshots refresh while dirty drafts survive hydration", () => {
  const dirty: EditorDraft = { content: "local", originalContent: "base", sha256: "base-hash" };
  const clean: EditorDraft = { content: "old", originalContent: "old", sha256: "old-hash" };

  assert.equal(hydrateEditorDraft(dirty, "disk", "disk-hash"), dirty);
  assert.deepEqual(hydrateEditorDraft(clean, "disk", "disk-hash"), {
    content: "disk",
    originalContent: "disk",
    sha256: "disk-hash",
  });
});

test("external snapshots update clean drafts and report conflicts for dirty drafts", () => {
  const clean: EditorDraft = { content: "same", originalContent: "same", sha256: "old-hash" };
  const dirty: EditorDraft = { content: "local", originalContent: "base", sha256: "old-hash" };

  assert.deepEqual(reconcileExternalEditorDraft(clean, { content: "new", sha256: "new-hash" }), {
    kind: "updated",
    draft: { content: "new", originalContent: "new", sha256: "new-hash" },
  });
  assert.deepEqual(reconcileExternalEditorDraft(dirty, { content: "new", sha256: "new-hash" }), {
    kind: "conflict",
    draft: dirty,
    snapshot: { content: "new", sha256: "new-hash" },
  });
  assert.deepEqual(reconcileExternalEditorDraft(clean, { content: "ignored", sha256: "old-hash" }), {
    kind: "unchanged",
    draft: clean,
  });
});

test("draft retention keeps active and dirty resources only", () => {
  const active: EditorDraft = { content: "active", originalContent: "active", sha256: "a" };
  const clean: EditorDraft = { content: "old", originalContent: "old", sha256: "b" };
  const dirty: EditorDraft = { content: "local", originalContent: "base", sha256: "c" };
  const drafts = { active, clean, dirty };

  assert.deepEqual(retainActiveAndDirtyDrafts(drafts, "active"), { active, dirty });
  assert.deepEqual(discardCleanDrafts(drafts), { dirty });
});
