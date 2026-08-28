import assert from "node:assert/strict";
import test from "node:test";

import { backupConfigurationState } from "../src/lib/backup-state.ts";

test("missing response is loading, not unconfigured", () => {
  assert.equal(backupConfigurationState(null), "loading");
});

test("configured response is configured", () => {
  assert.equal(backupConfigurationState({ config: { remoteUrl: "owner/repository" } }), "configured");
});

test("null config response is unconfigured", () => {
  assert.equal(backupConfigurationState({ config: null }), "unconfigured");
});
