import assert from "node:assert/strict";
import test from "node:test";

import {
  applySkillUpdateReports,
  clearSkillUpdateAvailability,
  mergeSkillListPreservingUpdates,
  normalizeSkill,
  type NormalizedSkill,
} from "../src/lib/skills.ts";
import { applySkillUpdateReportsToData, countSkillUpdates } from "../src/lib/skill-updates.ts";
import type { RuntimeData } from "../src/lib/data.ts";

function rawSkill(trackingStatus = "checkable"): Record<string, unknown> {
  return {
    name: "demo",
    agents: ["shared"],
    tags: [],
    dependencies: [],
    dependents: [],
    paths: [{
      path: "/tmp/demo",
      root: "/tmp",
      scope: "user",
      agent: "shared",
      install_target: "shared:/tmp",
      source_kind: "local",
    }],
    install_targets: ["shared:/tmp"],
    visibility: "Auto",
    source_summary: "local",
    update_status: trackingStatus,
    is_system: false,
  };
}

function normalizedSkill(trackingStatus = "checkable"): NormalizedSkill {
  const skill = normalizeSkill(rawSkill(trackingStatus));
  assert.ok(skill);
  return skill;
}

test("normalize separates tracking status from remote update availability", () => {
  const skill = normalizedSkill("tracked");

  assert.equal(skill.trackingStatus, "tracked");
  assert.equal(skill.updateAvailability, "unknown");
  assert.equal("updateStatus" in skill, false);
  assert.equal("meta" in skill, false);
});

test("remote update reports only change update availability", () => {
  const report = applySkillUpdateReports(
    { skills: [normalizedSkill("checkable")] },
    [{ name: "demo", status: "update-available" }],
  );
  const skill = report.skills[0];

  assert.equal(skill.trackingStatus, "checkable");
  assert.equal(skill.updateAvailability, "update-available");
  assert.equal("meta" in skill, false);
});

test("refresh merge preserves remote availability but takes the new tracking status", () => {
  const previous = applySkillUpdateReports(
    { skills: [normalizedSkill("tracked")] },
    [{ name: "demo", status: "update-available" }],
  ).skills;
  const next = [normalizedSkill("local")];

  const [skill] = mergeSkillListPreservingUpdates(previous, next);
  assert.equal(skill.trackingStatus, "local");
  assert.equal(skill.updateAvailability, "update-available");
});

test("clearing an update changes availability without changing tracking status", () => {
  const report = applySkillUpdateReports(
    { skills: [normalizedSkill("tracked")] },
    [{ name: "demo", status: "update-available" }],
  );

  const cleared = clearSkillUpdateAvailability(report, ["demo"]);
  assert.equal(cleared.skills[0].trackingStatus, "tracked");
  assert.equal(cleared.skills[0].updateAvailability, "up-to-date");
  assert.equal("meta" in cleared.skills[0], false);
});

test("update count uses remote availability, not tracking status", () => {
  const available = applySkillUpdateReports(
    { skills: [normalizedSkill("checkable")] },
    [{ name: "demo", status: "update-available" }],
  ).skills[0];
  const data = {
    agents: [],
    skills: [available, { ...normalizedSkill("update-available"), name: "local-only" }],
    prompts: [],
    sessions: [],
    rules: [],
    hooks: [],
    mcp: [],
    sources: [],
  } as RuntimeData;

  assert.equal(countSkillUpdates(data, "All"), 1);

  const applied = applySkillUpdateReportsToData(data, [{ name: "local-only", status: "up-to-date" }]);
  assert.equal(applied.skills[1].trackingStatus, "update-available");
  assert.equal(applied.skills[1].updateAvailability, "up-to-date");
});
