import assert from "node:assert/strict";
import test from "node:test";

import { desktopStore } from "../src/store/desktop-store.ts";

test("inventory count can arrive before the asynchronous skill update result", () => {
  desktopStore.actions.resetInventory();
  desktopStore.actions.setInventoryCount("skills", 4, 1);
  assert.equal(desktopStore.getSnapshot().inventory.counts.skills, 4);
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 1);
});

test("fresh skill updates become the shared count authority", () => {
  desktopStore.actions.updateData((current) => ({
    ...current,
    skills: [{ name: "demo", agents: ["Codex"] }] as typeof current.skills,
  }));
  desktopStore.actions.setSkillUpdateReports([{ name: "demo", status: "update-available" }]);
  assert.equal(desktopStore.getSnapshot().catalogs.data.skills[0]?.updateStatus, "update-available");
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 1);

  desktopStore.actions.setInventoryCount("skills", 1, 0);
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 1);
});

test("resetting overview inventory preserves fresh skill update counts", () => {
  desktopStore.actions.resetInventory("All");
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 1);

  desktopStore.actions.resetInventory("Claude Code");
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 0);

  desktopStore.actions.resetInventory("Codex");
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 1);
});

test("skill update count follows the selected agent without another scan", () => {
  desktopStore.actions.refreshSkillUpdateCount("Claude Code");
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 0);
  desktopStore.actions.refreshSkillUpdateCount("Codex");
  assert.equal(desktopStore.getSnapshot().inventory.skillUpdateCount, 1);
});
