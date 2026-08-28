import assert from "node:assert/strict";
import test from "node:test";

import type { SkillChangeCommand } from "../src/lib/skills.ts";
import { skillChangeActionLabel, skillChangeBusyLabel, skillChangeDescription, skillChangeLoadingCopy, skillChangeTitle } from "../src/lib/skill-change-copy.ts";

const deleteMany = "skills_delete_many" as SkillChangeCommand;
const updateMany = "skills_update_many" as SkillChangeCommand;
const setVisibility = "skills_set" as SkillChangeCommand;
const wrap = "skills_wrap" as SkillChangeCommand;

test("keeps the skill change fallback and dialog copy aligned", () => {
  assert.equal(skillChangeTitle(deleteMany), "Delete selected skills?");
  assert.equal(skillChangeDescription(deleteMany), "Delete the selected skills from their installed locations.");
  assert.equal(skillChangeTitle(updateMany), "Confirm skill changes");
  assert.equal(skillChangeDescription(updateMany), "Apply available updates for the selected skills.");
});

test("keeps skill change action and loading copy shared", () => {
  assert.equal(skillChangeActionLabel(deleteMany), "Delete skills");
  assert.equal(skillChangeActionLabel(updateMany), "Apply updates");
  assert.equal(skillChangeActionLabel(setVisibility), "Apply visibility");
  assert.equal(skillChangeActionLabel(wrap), "Create skill");
  assert.equal(skillChangeActionLabel(null), "Apply changes");
  assert.equal(skillChangeBusyLabel(deleteMany), "Deleting…");
  assert.equal(skillChangeBusyLabel(updateMany), "Updating…");
  assert.equal(skillChangeBusyLabel(wrap), "Creating…");
  assert.equal(skillChangeBusyLabel(setVisibility), "Applying…");
  assert.deepEqual(skillChangeLoadingCopy, {
    description: "Preparing skill change preview.",
    previewLabel: "Preparing update preview",
  });
});
