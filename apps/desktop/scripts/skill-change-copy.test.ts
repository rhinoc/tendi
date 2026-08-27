import assert from "node:assert/strict";
import test from "node:test";

import type { SkillChangeCommand } from "../src/lib/skills.ts";
import { skillChangeDescription, skillChangeTitle } from "../src/lib/skill-change-copy.ts";

const deleteMany = "skills_delete_many" as SkillChangeCommand;
const updateMany = "skills_update_many" as SkillChangeCommand;

test("keeps the skill change fallback and dialog copy aligned", () => {
  assert.equal(skillChangeTitle(deleteMany), "Delete selected skills?");
  assert.equal(skillChangeDescription(deleteMany), "Delete the selected skills from their installed locations.");
  assert.equal(skillChangeTitle(updateMany), "Confirm skill changes");
  assert.equal(skillChangeDescription(updateMany), "Apply available updates for the selected skills.");
});
