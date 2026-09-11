import assert from "node:assert/strict";
import test from "node:test";

import {
  assistantAskRequest,
  assistantPromptSuggestions,
  buildAssistantContext,
  canMarkAssistantReplyRead,
  truncateAssistantText,
  persistAssistantAgent,
  readAssistantAgent,
} from "../src/lib/assistant.ts";

test("persists and restores the selected assistant agent", () => {
  const storage = new Map<string, string>();
  const globalWithWindow = globalThis as typeof globalThis & { window?: unknown };
  const previousWindow = globalWithWindow.window;
  Object.defineProperty(globalWithWindow, "window", {
    configurable: true,
    value: {
      localStorage: {
        getItem: (key: string) => storage.get(key) ?? null,
        setItem: (key: string, value: string) => storage.set(key, value),
      },
    },
  });
  try {
    persistAssistantAgent(" Claude ");
    assert.equal(readAssistantAgent(), "Claude");
  } finally {
    Object.defineProperty(globalWithWindow, "window", { configurable: true, value: previousWindow });
  }
});

test("marks a reply read only while the visible panel follows the latest messages", () => {
  assert.equal(canMarkAssistantReplyRead(true, true, true, true), true);
  assert.equal(canMarkAssistantReplyRead(false, true, true, true), false);
  assert.equal(canMarkAssistantReplyRead(true, false, true, true), false);
  assert.equal(canMarkAssistantReplyRead(true, true, false, true), false);
  assert.equal(canMarkAssistantReplyRead(true, true, true, false), false);
});

test("builds bounded context with selection and selected rows", () => {
  const context = buildAssistantContext({
    pageId: "sessions",
    pageTitle: "Sessions",
    filters: { agent: "Codex", ignored: undefined },
    selection: " selected text ",
    selectedContent: ["selected text", "row one", "row two"],
    skill: null,
    session: { id: "session-1", tokenUsage: { totalTokens: 42 } },
    tendi: { counts: { sessions: 2 } },
  });

  assert.equal(context.selection, "selected text");
  assert.deepEqual(context.selectedContent, ["selected text", "row one", "row two"]);
  assert.deepEqual(context.filters, { agent: "Codex" });
  assert.equal(context.skill, null);
  assert.deepEqual(context.session, { id: "session-1", tokenUsage: { totalTokens: 42 } });
});

test("truncates context text deterministically", () => {
  assert.equal(truncateAssistantText("  short  ", 20), "short");
  const truncated = truncateAssistantText("1234567890abcdefghijkl", 20);
  assert.equal(truncated.length, 20);
  assert.ok(truncated.endsWith("[truncated]"));
});

test("builds page-aware suggestions from real page rows", () => {
  const context = buildAssistantContext({
    pageId: "hooks",
    pageTitle: "Hooks",
    pageData: [{ agent: "codex", event: "after_turn", enabled: false, needsReview: true }],
  });
  const suggestions = assistantPromptSuggestions(context);

  assert.equal(suggestions[0]?.label, "Find hooks to review");
  assert.match(suggestions[0]?.detail ?? "", /1 hooks in view/);
  assert.match(suggestions[0]?.message ?? "", /Hooks rows/);
  assert.deepEqual(context.tendi.currentPage, {
    id: "hooks",
    rowCount: 1,
    shownRowCount: 1,
    truncated: false,
    rows: [{ agent: "codex", event: "after_turn", enabled: false, needsReview: true }],
  });
});

test("prioritizes selected content over page-level suggestions", () => {
  const context = buildAssistantContext({
    pageId: "sessions",
    pageTitle: "Sessions",
    selection: "session-1",
  });
  const suggestions = assistantPromptSuggestions(context);

  assert.equal(suggestions[0]?.label, "Review the selected items");
  assert.match(suggestions[0]?.message ?? "", /selected items/);
});

test("keeps assistant requests bounded to recent history", () => {
  const history = Array.from({ length: 21 }, (_, index) => ({
    role: index % 2 === 0 ? "user" as const : "assistant" as const,
    content: `message-${index}`,
  }));
  const context = buildAssistantContext({ pageId: "overview", pageTitle: "Overview" });
  const request = assistantAskRequest(" question ", history, context, "codex", "/repo", "conversation-1", true);
  assert.equal(request.message, "question");
  assert.equal(request.conversationId, "conversation-1");
  assert.equal(request.persistUserMessage, true);
  assert.equal(request.history.length, 20);
});
