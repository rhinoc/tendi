import assert from "node:assert/strict";
import test from "node:test";
import {
  discardDirtyDrafts,
  getSkillEditorState,
  hydrateSkillDraft,
  updateSkillEditorField,
} from "../src/features/skills/skill-editor-state.ts";

test("skill editor state is isolated by skill id and retains editor fields", () => {
  const firstSkillId = `skill-editor-state-first-${Date.now()}`;
  const secondSkillId = `${firstSkillId}-second`;

  updateSkillEditorField(firstSkillId, "activePath", "nested/README.md");
  updateSkillEditorField(firstSkillId, "selectedPath", "nested/README.md");
  updateSkillEditorField(firstSkillId, "fileTreeCollapsed", true);
  updateSkillEditorField(firstSkillId, "collapsedFolders", new Set(["nested"]));
  updateSkillEditorField(firstSkillId, "createdPaths", new Set(["nested/README.md"]));
  updateSkillEditorField(firstSkillId, "drafts", {
    "nested/README.md": { content: "edited", originalContent: "original", sha256: "sha" },
  });

  assert.deepEqual(getSkillEditorState(firstSkillId), {
    activePath: "nested/README.md",
    selectedPath: "nested/README.md",
    fileTreeCollapsed: true,
    collapsedFolders: new Set(["nested"]),
    createdPaths: new Set(["nested/README.md"]),
    drafts: {
      "nested/README.md": { content: "edited", originalContent: "original", sha256: "sha" },
    },
  });
  assert.deepEqual(getSkillEditorState(secondSkillId), {
    activePath: "",
    selectedPath: "",
    fileTreeCollapsed: false,
    collapsedFolders: new Set(),
    createdPaths: new Set(),
    drafts: {},
  });
});

test("loading content preserves dirty drafts and refreshes clean drafts", () => {
  const dirtyDraft = {
    content: "edited",
    originalContent: "original",
    sha256: "old-sha",
  };
  const cleanDraft = {
    content: "old",
    originalContent: "old",
    sha256: "old-sha",
  };

  const dirtyResult = hydrateSkillDraft(
    { "SKILL.md": dirtyDraft },
    "SKILL.md",
    "fresh from disk",
    "fresh-sha",
    "SKILL.md",
  );
  const cleanResult = hydrateSkillDraft(
    { "SKILL.md": cleanDraft },
    "SKILL.md",
    "fresh from disk",
    "fresh-sha",
    "SKILL.md",
  );

  assert.deepEqual(dirtyResult["SKILL.md"], dirtyDraft);
  assert.deepEqual(cleanResult["SKILL.md"], {
    content: "fresh from disk",
    originalContent: "fresh from disk",
    sha256: "fresh-sha",
  });
  assert.deepEqual(discardDirtyDrafts({
    dirty: dirtyDraft,
    clean: cleanDraft,
  }), { clean: cleanDraft });
});
