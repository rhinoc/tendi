import assert from "node:assert/strict";
import test from "node:test";

import { groupTranscriptItems, type TranscriptItem } from "../src/lib/transcript.ts";
import { countTextTokens } from "../src/lib/tokenizer.ts";
import {
  buildSessionContext,
  contextShareLabel,
  contextTokenTexts,
  orderSessionFamily,
  sessionUsageParts,
  type ContextSourceItem,
} from "../src/lib/session-context.ts";

function storyFrom(items: TranscriptItem[]) {
  const grouped = groupTranscriptItems(items) as ContextSourceItem[];
  const texts = contextTokenTexts(grouped);
  const counts = new Map(texts.map((text) => [text, countTextTokens(text)]));
  return buildSessionContext(grouped, counts);
}

test("context shares describe the visible window and sum to 100", () => {
  const story = storyFrom([
    { type: "user", body: "look at the token bar" },
    { type: "tool", tag: "Read", command: JSON.stringify({ path: "src/SessionsView.tsx" }), result: "export function SessionsView" },
    { type: "assistant", body: "The bar only shows input and output." },
  ]);

  assert.equal(story.bands.reduce((total, band) => total + band.share, 0), 100);
  assert.deepEqual(story.bands.map((band) => band.band), ["user", "assistant", "tool"]);
  assert.equal(story.steps.length, 1);
  assert.equal(story.steps[0].you, "look at the token bar");
  assert.equal(story.steps[0].incoming, "src/SessionsView.tsx");
  assert.equal(story.files[0]?.path, "src/SessionsView.tsx");
  assert.equal(story.files[0]?.read, 1);
  assert.equal(story.steps[0].key, "assistant-2");
});

test("compaction clears the window and marks the drop", () => {
  const story = storyFrom([
    { type: "user", body: "hello" },
    { type: "assistant", body: "world" },
    { type: "compaction", body: "" },
    { type: "user", body: "again" },
  ]);

  assert.equal(story.steps.length, 2);
  assert.equal(story.steps[1]?.compacted, true);
  assert.equal(story.steps[1]?.total, 0);
  assert.deepEqual(story.bands.map((band) => band.band), ["user"]);
  assert.ok(story.steps[0].total > story.bands[0].tokens);
});

test("shell runs show on the step and are not files", () => {
  const story = storyFrom([
    { type: "tool", tag: "exec", command: "cargo test", result: "exit code 1", durationMs: 4000 },
    { type: "assistant", body: "tests failed" },
  ]);

  assert.equal(story.steps[0]?.incoming, "cargo test");
  assert.equal(story.files.length, 0);
});

test("tool payloads are not treated as shell commands", () => {
  const story = storyFrom([
    { type: "tool", tag: "exec", command: "text((await tools.write_stdin({session_id:1}))", result: "ok", durationMs: 4000 },
    { type: "tool", tag: "exec", command: "text((await tools.write_stdin({session_id:1}))", result: "ok", durationMs: 3000 },
    { type: "assistant", body: "done" },
  ]);

  assert.equal(story.steps[0]?.incoming, "exec");
});

test("context bands use o200k counts", () => {
  const story = storyFrom([
    { type: "user", body: "hello" },
    { type: "assistant", body: "ok" },
  ]);
  const user = story.bands.find((band) => band.band === "user");
  assert.equal(user?.tokens, countTextTokens("hello"));
  assert.notEqual(user?.tokens, Math.ceil("hello".length / 4));
});

test("a tiny message is not reported as zero", () => {
  const story = storyFrom([
    { type: "user", body: "hi" },
    { type: "tool", tag: "Read", command: JSON.stringify({ path: "src/file.tsx" }), result: "x".repeat(4000) },
    { type: "assistant", body: "ok" },
  ]);
  const user = story.bands.find((band) => band.band === "user");
  const assistant = story.bands.find((band) => band.band === "assistant");
  assert.equal(user && contextShareLabel(user.share, user.tokens), "<1%");
  assert.equal(assistant && contextShareLabel(assistant.share, assistant.tokens), "<1%");
});

test("skill context stays out of the tool bucket", () => {
  const story = storyFrom([
    { type: "context", tag: "Skills", body: "<available_skills>review</available_skills>" },
    { type: "assistant", body: "ok" },
  ]);

  assert.deepEqual(story.bands.map((band) => band.band), ["assistant", "skill"]);
});

test("usage mix keeps the billed buckets that the session log reports", () => {
  assert.deepEqual(sessionUsageParts({
    inputTokens: 100,
    cachedInputTokens: 40,
    outputTokens: 30,
    reasoningOutputTokens: 10,
  }), [
    { kind: "cached", tokens: 40 },
    { kind: "uncached", tokens: 60 },
    { kind: "reasoning", tokens: 10 },
    { kind: "answer", tokens: 20 },
  ]);
});

test("family order nests children under their parent", () => {
  const rows = orderSessionFamily([
    { id: "child", parentSessionId: "root" },
    { id: "root" },
    { id: "grandchild", parentSessionId: "child" },
  ]);
  assert.deepEqual(rows.map((row) => [row.session.id, row.depth]), [
    ["root", 0],
    ["child", 1],
    ["grandchild", 2],
  ]);
});
