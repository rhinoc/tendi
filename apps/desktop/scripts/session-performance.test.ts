import assert from "node:assert/strict";
import test from "node:test";

import { resolveInitialSession, resolveInitialSessionId } from "../src/lib/session-selection.ts";
import { createLatestRequestAuthority, mergeTranscriptItems, normalizeTranscriptSearchResult, transcriptItemsSize } from "../src/lib/transcript.ts";
import { parseJsonlTranscript } from "../src/lib/agent/transcript.ts";
import { selectAnalyticsGranularity, stepAnalyticsGranularity } from "../src/lib/analytics.ts";
import { trackpadZoomDirection, trackpadZoomFactor } from "../src/lib/zoom-gesture.ts";

test("external session target wins on the first selection", () => {
  const sessions = [
    { id: "default", agent: "Codex" },
    { id: "requested", agent: "Claude Code" },
  ];

  assert.equal(
    resolveInitialSessionId(sessions, "claude:requested"),
    "requested",
  );
});

test("external session target resolves the requested agent when ids overlap", () => {
  const sessions = [
    { id: "shared-id", agent: "Codex" },
    { id: "shared-id", agent: "Claude" },
  ];

  assert.equal(resolveInitialSession(sessions, "claude:shared-id"), sessions[1]);
});

test("latest transcript request is the only current request", () => {
  const authority = createLatestRequestAuthority();
  const first = authority.begin();
  const second = authority.begin();

  assert.equal(authority.isCurrent(first), false);
  assert.equal(authority.isCurrent(second), true);
  authority.invalidate(second);
  assert.equal(authority.isCurrent(second), false);
});

test("cross-page tool result merges by stable call id", () => {
  const merged = mergeTranscriptItems(
    [{ type: "tool", body: "cargo test", callId: "call-1" }],
    [{ type: "tool_result", body: "Tool result", result: "passed", callId: "call-1" }],
  );

  assert.equal(merged.length, 1);
  assert.equal(merged[0].result, "passed");
});

test("unmatched tool result is dropped during merge", () => {
  const merged = mergeTranscriptItems(
    [{ type: "tool", body: "cargo test", callId: "call-1" }],
    [{ type: "tool_result", body: "Tool result", result: "orphaned", callId: "missing" }],
  );

  assert.equal(merged.length, 1);
  assert.equal(merged[0].result, undefined);
});

test("unmatched tool results are omitted from local transcript parsing", () => {
  const parsed = parseJsonlTranscript([
    JSON.stringify({
      type: "response_item",
      payload: { type: "function_call_output", call_id: "missing-codex-call", output: "orphaned" },
    }),
    JSON.stringify({
      type: "user",
      message: {
        content: [{ type: "tool_result", tool_use_id: "missing-claude-call", content: "orphaned" }],
      },
    }),
    JSON.stringify({
      type: "user",
      toolUseID: "missing-claude-legacy-call",
      toolUseResult: { stdout: "orphaned" },
    }),
  ].join("\n"));

  assert.equal(parsed.items.length, 0);
});

test("transcript cache size includes large bodies and tool results", () => {
  assert.equal(transcriptItemsSize([{ body: "abc", result: "12345" }]), 8);
});

test("transcript search results normalize logical group and tool indices", () => {
  assert.deepEqual(
    normalizeTranscriptSearchResult({
      hits: [
        { groupIndex: 4 },
        { groupIndex: 8, toolIndex: 2 },
        { groupIndex: -1 },
        { groupIndex: "invalid" },
      ],
      warnings: ["one warning"],
      sourceVersion: "v1-test",
    }),
    {
      hits: [{ groupIndex: 4 }, { groupIndex: 8, toolIndex: 2 }],
      warnings: ["one warning"],
      sourceVersion: "v1-test",
    },
  );
});

test("analytics chart density adapts to the loaded span", () => {
  assert.equal(selectAnalyticsGranularity(30), "day");
  assert.equal(selectAnalyticsGranularity(90), "week");
  assert.equal(selectAnalyticsGranularity(365), "week");
  assert.equal(selectAnalyticsGranularity(730), "month");
});

test("analytics chart zoom steps through adjacent granularities", () => {
  assert.equal(stepAnalyticsGranularity("week", -1), "day");
  assert.equal(stepAnalyticsGranularity("week", 1), "month");
  assert.equal(stepAnalyticsGranularity("day", -1), "day");
  assert.equal(stepAnalyticsGranularity("month", 1), "month");
});

test("charts share trackpad zoom sensitivity and thresholds", () => {
  assert.equal(trackpadZoomDirection(trackpadZoomFactor(-40)), -1);
  assert.equal(trackpadZoomDirection(trackpadZoomFactor(40)), 1);
  assert.equal(trackpadZoomDirection(1), 0);
});
