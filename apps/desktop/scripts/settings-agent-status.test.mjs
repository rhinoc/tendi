import assert from "node:assert/strict";
import test from "node:test";

import { codingAgentsAction, isCodingAgentsInstalled } from "../src/features/settings/settings-agent-status.ts";

function cliStatus(overrides = {}) {
  return {
    state: "unsupported",
    supported: false,
    commandPath: null,
    bundledPath: null,
    pathConfigured: false,
    currentTarget: null,
    detail: "",
    ...overrides,
  };
}

function skillStatus(overrides = {}) {
  return {
    name: "tendi",
    target: "/tmp/.agents/skills/tendi",
    installed: true,
    current: true,
    promptHandled: true,
    shouldPrompt: false,
    ...overrides,
  };
}

test("treats a current skill as installed when CLI registration is unsupported", () => {
  assert.equal(isCodingAgentsInstalled(cliStatus(), skillStatus()), true);
});

test("requires both current skill and healthy CLI when CLI registration is supported", () => {
  assert.equal(
    isCodingAgentsInstalled(cliStatus({ state: "installed", supported: true, pathConfigured: true }), skillStatus()),
    true,
  );
  assert.equal(
    isCodingAgentsInstalled(cliStatus({ state: "not-installed", supported: true }), skillStatus()),
    false,
  );
});

test("does not report setup as installed without the current skill or status", () => {
  assert.equal(isCodingAgentsInstalled(cliStatus(), skillStatus({ current: false })), false);
  assert.equal(isCodingAgentsInstalled(null, skillStatus()), false);
});

test("projects one user action for each coding agents state", () => {
  const healthyCli = cliStatus({ state: "installed", supported: true, pathConfigured: true });
  const missingCli = cliStatus({ state: "not-installed", supported: true });
  const staleCli = cliStatus({ state: "stale", supported: true });

  assert.equal(codingAgentsAction(healthyCli, skillStatus({ installed: false, current: false })), "install");
  assert.equal(codingAgentsAction(healthyCli, skillStatus({ installed: true, current: false })), "repair");
  assert.equal(codingAgentsAction(missingCli, skillStatus()), "install");
  assert.equal(codingAgentsAction(staleCli, skillStatus()), "repair");
  assert.equal(codingAgentsAction(healthyCli, skillStatus()), "remove");
  assert.equal(codingAgentsAction(cliStatus(), skillStatus()), "remove");
  assert.equal(codingAgentsAction(cliStatus({ state: "installed", supported: true }), skillStatus()), "remove");
  assert.equal(codingAgentsAction(cliStatus({ state: "conflict", supported: true }), skillStatus()), null);
});
