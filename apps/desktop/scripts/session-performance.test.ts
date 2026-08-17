import assert from "node:assert/strict";
import test from "node:test";

import { resolveInitialSession, resolveInitialSessionId } from "../src/lib/session-selection.ts";
import { applySessionProjectDelta } from "../src/lib/session-project-delta.ts";
import { createLatestRequestAuthority, mergeTranscriptItems, normalizeTranscriptSearchResult, transcriptItemsSize } from "../src/lib/transcript.ts";
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

test("project merge updates only affected session rows", () => {
  const rows = [
    { id: "target", title: "Target", agent: "Codex", path: "/target", logicalProjectId: "p1", logicalProjectName: "Target" },
    { id: "source", title: "Source", agent: "Claude", path: "/source", logicalProjectId: "p2", logicalProjectName: "Source" },
    { id: "other", title: "Other", agent: "Cursor", path: "/other", logicalProjectId: "p3", logicalProjectName: "Other" },
  ];

  const next = applySessionProjectDelta(rows, {
    kind: "merge",
    projectId: "p1",
    projectName: "Target",
    mergedProjectIds: ["p1", "p2"],
  });

  assert.equal(next[0], rows[0]);
  assert.notEqual(next[1], rows[1]);
  assert.equal(next[1].logicalProjectId, "p1");
  assert.equal(next[2], rows[2]);
});

test("project split matches normalized agent identity", () => {
  const rows = [
    { id: "selected", title: "Selected", agent: "Claude", path: "/selected", logicalProjectId: "old", logicalProjectName: "Old" },
    { id: "other", title: "Other", agent: "Claude", path: "/other", logicalProjectId: "old", logicalProjectName: "Old" },
  ];

  const next = applySessionProjectDelta(rows, {
    kind: "split",
    projectId: "new",
    projectName: "Focused",
    sessions: [{ id: "selected", agent: "claude-code", path: "/selected" }],
  });

  assert.equal(next[0].logicalProjectId, "new");
  assert.equal(next[1], rows[1]);
});

test("cross-page tool result merges by stable call id", () => {
  const merged = mergeTranscriptItems(
    [{ type: "tool", body: "cargo test", callId: "call-1" }],
    [{ type: "tool_result", body: "Tool result", result: "passed", callId: "call-1" }],
  );

  assert.equal(merged.length, 1);
  assert.equal(merged[0].result, "passed");
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
