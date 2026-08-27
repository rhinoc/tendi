import assert from "node:assert/strict";
import test from "node:test";

import { sessionResumeErrorMessage, sessionResumeLabel, sessionResumeTargetForMenu } from "../src/lib/session-resume.ts";

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
