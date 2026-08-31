import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("session resume menu tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  mock.module("../src/lib/agent/index.ts", {
    namedExports: { agentDefinition: () => undefined },
  });
  mock.module("../src/lib/agent/catalog.ts", {
    namedExports: { agentIcons: {} },
  });

  const {
    sessionResumeErrorMessage,
    sessionResumeLabel,
    sessionResumeTargetForMenu,
    sessionResumeTargetsForMenu,
  } = await import("../src/lib/session-resume.ts");

  test("offers both explicit resume targets only when both are supported", () => {
    assert.deepEqual(sessionResumeTargetsForMenu({ terminal: true, app: true }), ["terminal", "app"]);
    assert.deepEqual(sessionResumeTargetsForMenu({ terminal: true, app: false }), ["terminal"]);
    assert.deepEqual(sessionResumeTargetsForMenu({ terminal: false, app: false }), []);
  });

  test("uses the inferred target when the session resume setting is auto", () => {
    assert.equal(sessionResumeTargetForMenu("auto", "app"), "app");
    assert.equal(sessionResumeTargetForMenu("auto", "terminal"), "terminal");
  });

  test("keeps explicit targets and leaves unresolved auto unchanged", () => {
    assert.equal(sessionResumeTargetForMenu("app", "terminal"), "app");
    assert.equal(sessionResumeTargetForMenu("terminal", "app"), "terminal");
    assert.equal(sessionResumeTargetForMenu("auto"), "auto");
  });

  test("labels every resume state from the shared copy helper", () => {
    assert.equal(sessionResumeLabel("idle", "app"), "Resume in app");
    assert.equal(sessionResumeLabel("idle", "terminal"), "Resume in terminal");
    assert.equal(sessionResumeLabel("idle", "auto"), "Resume");
    assert.equal(sessionResumeLabel("loading", "app"), "Opening session in app");
    assert.equal(sessionResumeLabel("success", "terminal"), "Session opened in terminal");
    assert.equal(sessionResumeLabel("error", "auto"), "Could not open session");
    assert.equal(sessionResumeErrorMessage(), "Could not open session. Try again.");
  });
}
