import assert from "node:assert/strict";
import test from "node:test";

const { applySkillUpdateEvent } = await import("../src/controllers/skill-update-controller.ts");

test("releases the active guard after a completed update check", () => {
    const reports: unknown[] = [];
    const errors: string[] = [];
    const checking: boolean[] = [];
    const active: boolean[] = [];

    applySkillUpdateEvent({
      status: "completed",
      updates: [{ id: "skill-1", name: "Skill 1", status: "update-available", source_kind: "git" }],
      error: null,
    }, {
      setSkillUpdateReports: (updates) => reports.push(...updates),
      setSkillUpdateError: (message) => errors.push(message),
      setCheckingSkillUpdates: (value) => checking.push(value),
      setSkillUpdateCheckActive: (value) => active.push(value),
    });

    assert.equal(reports.length, 1);
    assert.deepEqual(errors, [""]);
    assert.deepEqual(checking, [false]);
    assert.deepEqual(active, [false]);
});

test("releases the active guard after a failed update check", () => {
    const errors: string[] = [];
    const checking: boolean[] = [];
    const active: boolean[] = [];

    applySkillUpdateEvent({
      status: "failed",
      updates: [],
      error: "remote fetch failed",
    }, {
      setSkillUpdateReports: () => assert.fail("failed checks must not replace reports"),
      setSkillUpdateError: (message) => errors.push(message),
      setCheckingSkillUpdates: (value) => checking.push(value),
      setSkillUpdateCheckActive: (value) => active.push(value),
    });

    assert.deepEqual(errors, ["remote fetch failed"]);
    assert.deepEqual(checking, [false]);
    assert.deepEqual(active, [false]);
});
