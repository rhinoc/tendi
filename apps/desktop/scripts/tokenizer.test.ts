import assert from "node:assert/strict";
import test from "node:test";

import {
  countTextTokens,
  markdownTokenStats,
  transcriptTokenSegments,
} from "../src/lib/tokenizer.ts";
import {
  markdownTokenParts,
  markdownTokenSegments,
  markdownTokenStatsFromCounts,
  transcriptTokenSegmentsFromCounts,
  transcriptTokenTexts,
} from "../src/lib/tokenizer-shared.ts";
import type { TranscriptTokenItem } from "../src/lib/tokenizer-types.ts";

test("shared markdown tokenization preserves the JS tokenizer result", () => {
  const activePath = "SKILL.md";
  const content = "---\ndescription: A skill for testing\n---\n\n# Body\n\nhello world";
  const selectionText = "hello";
  const expected = markdownTokenStats(activePath, content, selectionText);
  const parts = markdownTokenParts(activePath, content, selectionText);
  const counts = parts.texts.map(countTextTokens);
  assert.deepEqual(markdownTokenStatsFromCounts(parts, counts), expected);
  assert.deepEqual(markdownTokenSegments(expected), [
    { label: "Selection", value: expected.selection },
    { label: "Desc", value: expected.description },
    { label: "Content", value: expected.content },
  ]);
});

test("shared transcript tokenization preserves the JS tokenizer result", () => {
  const items: TranscriptTokenItem[] = [
    { type: "user", body: "hello" },
    { type: "tool", command: "cat README.md", result: "hello world" },
    { type: "assistant", body: "done" },
  ];
  const links = [{ skill_name: "test-skill" }];
  const texts = transcriptTokenTexts(items);
  const counts = new Map(texts.map((text) => [text, countTextTokens(text)]));
  assert.deepEqual(
    transcriptTokenSegmentsFromCounts(items, links, counts),
    transcriptTokenSegments(items, links),
  );
});
