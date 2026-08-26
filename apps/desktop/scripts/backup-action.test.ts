import assert from "node:assert/strict";
import test from "node:test";

import { backupSkillsForSelection, backupStatusForSkill } from "../src/features/skills/backup-action.ts";

const skill = { id: "demo", paths: [{ path: "/skills/demo" }] };
const statuses = new Map([
  ["/skills/demo", { skillPath: "/skills/demo", state: "unmanaged" }],
]);
const getStatus = (selectedSkill: typeof skill) => backupStatusForSkill(selectedSkill, statuses);

test("unconfigured backup never produces adopt candidates", () => {
  assert.deepEqual(backupSkillsForSelection([skill], getStatus, false), []);
});

test("configured backup keeps unmanaged skills with a path adoptable", () => {
  assert.deepEqual(backupSkillsForSelection([skill], getStatus, true), [skill]);
});
