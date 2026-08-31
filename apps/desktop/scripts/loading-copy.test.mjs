import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = async (path) => readFile(new URL(`../src/${path}`, import.meta.url), "utf8");

test("background skill refresh uses an icon-only status", async () => {
  const sessionsView = await source("views/SessionsView.tsx");
  assert.match(
    sessionsView,
    /<div className="sessionSkillsStatusOverlay" role="status" aria-live="polite" aria-label="Refreshing skills">\s*<LoadingIcon size=\{15\} \/>/,
  );
  assert.doesNotMatch(sessionsView, /sessionSkillsStatusOverlay[\s\S]{0,240}<LoadingInline/);
});

test("linked session indexing uses an icon-only status", async () => {
  const linkedSessions = await source("features/sessions/linked-sessions.tsx");
  assert.match(
    linkedSessions,
    /<span role="status" aria-label="Indexing"><LoadingIcon size=\{14\} \/><\/span>/,
  );
  assert.doesNotMatch(linkedSessions, /<span>Indexing<\/span>/);
});

test("button loading states do not render action copy", async () => {
  const [configView, promptsView, skillsView] = await Promise.all([
    source("views/ConfigView.tsx"),
    source("views/PromptsView.tsx"),
    source("views/SkillsView.tsx"),
  ]);

  assert.doesNotMatch(configView, /loadingContent=\{<LoadingInline/);
  assert.doesNotMatch(promptsView, /loadingContent=\{<LoadingInline/);
  assert.doesNotMatch(skillsView, /skillPreviewBusy[\s\S]{0,160}<LoadingInline/);
  assert.match(configView, /loadingContent=\{<LoadingIcon size=\{16\} \/>\}/);
  assert.match(promptsView, /loadingContent=\{<LoadingIcon size=\{16\} \/>\}/);
  assert.match(skillsView, /skillPreviewBusy[\s\S]{0,160}<LoadingIcon size=\{14\} \/>/);
});
