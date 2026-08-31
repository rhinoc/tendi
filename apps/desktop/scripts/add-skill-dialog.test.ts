import assert from "node:assert/strict";
import test from "node:test";

import {
  captureSkillSourcePage,
  isSkillSourceActionReady,
  normalizeSkillAddPlan,
  resolveSkillInstallTarget,
  restoreSkillSourcePage,
  shouldShowSkillQuickSelect,
  skillSourceErrorMessage,
} from "../src/lib/add-skill-dialog.ts";

test("keeps a root-level skill whose relative path is empty", () => {
  const rootSkill = {
    name: "tendi",
    description: "Bundled Tendi skill",
    path: "/tmp/tendi-bundled",
    relative_path: "",
    dependencies: [],
  };
  const normalized = normalizeSkillAddPlan({
    source: "/tmp/tendi-bundled",
    source_kind: "local",
    source_ref: null,
    source_root: "/tmp/tendi-bundled",
    target: "shared",
    scope: "global",
    mode: "symlink",
    available: [rootSkill],
    selected: [rootSkill],
    operations: [{
      name: "tendi",
      source: "/tmp/tendi-bundled",
      target: "/tmp/.agents/skills/tendi",
      mode: "symlink",
      status: "planned",
      message: null,
    }],
  });

  assert.deepEqual(normalized?.available.map((skill) => skill.name), ["tendi"]);
  assert.deepEqual(normalized?.selected.map((skill) => skill.name), ["tendi"]);
  assert.equal(normalized?.available[0]?.relative_path, "");
});

test("hides the skill quick-select when there are no existing skills", () => {
  assert.equal(shouldShowSkillQuickSelect(7, 0), false);
});

test("keeps the skill quick-select when existing skills are available", () => {
  assert.equal(shouldShowSkillQuickSelect(7, 2), true);
});

test("does not show the skill quick-select without available skills", () => {
  assert.equal(shouldShowSkillQuickSelect(0, 0), false);
});

test("restores the matching source page after a preview failure", () => {
  const results = [{ source: "owner/skill" }];
  const page = captureSkillSourcePage("owner", results);

  assert.deepEqual(restoreSkillSourcePage(page), {
    source: "owner",
    marketplaceQuery: "owner",
    marketplaceResults: results,
  });
});

test("restores the recommended source page after a preview failure", () => {
  const results = [{ source: "owner/skill" }];
  const page = captureSkillSourcePage("", results);

  assert.deepEqual(restoreSkillSourcePage(page), {
    source: "",
    marketplaceQuery: "",
    marketplaceResults: results,
  });
});

test("summarizes a missing repository clone error", () => {
  assert.equal(
    skillSourceErrorMessage(new Error("git clone failed for https://github.com/owner/missing.git: remote: Repository not found. fatal: repository not found")),
    "Repository not found.",
  );
});

test("keeps non-missing-repository source errors readable", () => {
  assert.equal(
    skillSourceErrorMessage(new Error("git clone failed for https://github.com/owner/skill.git: Could not resolve host")),
    "git clone failed for https://github.com/owner/skill.git: Could not resolve host",
  );
});

test("does not enable direct source actions without a target", () => {
  assert.equal(isSkillSourceActionReady("owner/skill", true, ""), false);
  assert.equal(isSkillSourceActionReady("owner/skill", true, "shared"), true);
});

test("keeps marketplace search available without a target", () => {
  assert.equal(isSkillSourceActionReady("react", false, ""), true);
});

test("resolves the first available install target when the selection is empty", () => {
  const options = [{ id: "shared" }, { id: "claude" }];
  assert.equal(resolveSkillInstallTarget("", options), "shared");
});

test("preserves a valid install target and replaces a stale selection", () => {
  const options = [{ id: "shared" }, { id: "claude" }];
  assert.equal(resolveSkillInstallTarget("claude", options), "claude");
  assert.equal(resolveSkillInstallTarget("cursor", options), "shared");
});
