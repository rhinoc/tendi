import assert from "node:assert/strict";
import test from "node:test";
import {
  collectCursorItemWithTimestamp,
  extractToolCommand,
} from "../src/lib/transcript.ts";
import type { TranscriptItem } from "../src/lib/transcript.ts";

test("keeps Cursor tool calls with non-command input", () => {
  const items: TranscriptItem[] = [];
  collectCursorItemWithTimestamp({
    role: "assistant",
    message: {
      content: [
        { type: "text", text: "I will inspect the file." },
        { type: "tool_use", name: "Read", input: { path: "/tmp/example.txt" } },
        {
          type: "tool_use",
          name: "StrReplace",
          input: {
            path: "/tmp/example.txt",
            old_string: "before",
            new_string: "after",
          },
        },
      ],
    },
  }, items);

  assert.equal(items.length, 3);
  assert.equal(items[0].type, "assistant");
  assert.equal(items[1].tag, "Read");
  assert.deepEqual(JSON.parse(items[1].command ?? ""), { path: "/tmp/example.txt" });
  assert.equal(items[2].tag, "StrReplace");
  assert.deepEqual(JSON.parse(items[2].command ?? ""), {
    path: "/tmp/example.txt",
    old_string: "before",
    new_string: "after",
  });
});

test("does not render Cursor timestamp-only user messages", () => {
  const items: TranscriptItem[] = [];
  collectCursorItemWithTimestamp({
    role: "user",
    message: {
      content: [{ type: "text", text: "<timestamp>Tuesday, Sep 1, 2026, 10:37 AM (UTC+8)</timestamp>" }],
    },
  }, items);

  assert.deepEqual(items, []);
});

test("serializes generic tool arguments for other transcript formats", () => {
  assert.deepEqual(
    JSON.parse(extractToolCommand({ name: "Read", input: { path: "/tmp/example.txt" } })),
    { path: "/tmp/example.txt" },
  );
  assert.deepEqual(
    JSON.parse(extractToolCommand({ name: "Read", arguments: { path: "/tmp/example.txt" } })),
    { path: "/tmp/example.txt" },
  );
  assert.deepEqual(
    JSON.parse(extractToolCommand({ name: "web_search", action: { type: "search", query: "Cursor transcript" } })),
    { type: "search", query: "Cursor transcript" },
  );
});
