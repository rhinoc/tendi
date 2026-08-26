import {
  attachToolResult,
  collectMessageContent,
  compactTime,
  durationMs,
  extractContentText,
  extractThinkingText,
  extractToolCommand,
  isJsonObject,
  numberAt,
  pushItem,
  stringAt,
  stringValue,
  summarizeToolCall,
  timestampMs,
} from "../transcript.ts";
import type { JsonObject } from "../transcript.ts";
import type { TranscriptParseContext, TranscriptParser } from "./types.ts";

function collectClaudeMeta(value: JsonObject, context: TranscriptParseContext) {
  if (value.type !== "assistant") return;
  const messageId = stringAt(value, ["message", "id"]);
  const directInputTokens = numberAt(value, ["message", "usage", "input_tokens"]);
  const outputTokens = numberAt(value, ["message", "usage", "output_tokens"]);
  if (!messageId || directInputTokens === undefined || outputTokens === undefined) return;
  const cacheCreationInputTokens = numberAt(value, ["message", "usage", "cache_creation_input_tokens"]) ?? 0;
  const cachedInputTokens = numberAt(value, ["message", "usage", "cache_read_input_tokens"]) ?? 0;
  const inputTokens = directInputTokens + cacheCreationInputTokens + cachedInputTokens;
  if (inputTokens + outputTokens <= 0) return;
  context.messageUsage.set(messageId, {
    inputTokens,
    cachedInputTokens,
    outputTokens,
    reasoningOutputTokens: 0,
    totalTokens: inputTokens + outputTokens,
  });
}

function collectClaudeItem(value: JsonObject, items: TranscriptParseContext["items"]) {
  const kind = stringValue(value.type);
  const time = compactTime(stringValue(value.timestamp));
  const message = isJsonObject(value.message) ? value.message : undefined;
  const content = message?.content;
  const eventMs = timestampMs(value.timestamp);

  if ((kind === "user" || kind === "assistant") && Array.isArray(content) && attachClaudeToolResults(content, value, items, eventMs)) return;
  if (kind === "user" || kind === "assistant") {
    if (kind === "assistant") {
      const thinking = extractThinkingText(content);
      if (thinking) pushItem(items, "thinking", thinking, undefined, time);
    }
    collectMessageContent(content, items, time, kind);
    if (Array.isArray(content)) {
      for (const item of content) {
        if (!isJsonObject(item) || item.type !== "tool_use") continue;
        const tag = stringValue(item.name) || undefined;
        pushItem(items, "tool", summarizeToolCall(item), tag, time, extractToolCommand(item), undefined, durationMs(item), stringValue(item.id), eventMs);
      }
    }
  }

  if (value.toolUseResult !== undefined) {
    const result = extractContentText(value.toolUseResult);
    attachToolResult(items, stringValue(value.toolUseID) || stringValue(value.tool_use_id) || stringValue(value.toolUseId), result, durationMs(value), eventMs);
  }
}

function attachClaudeToolResults(content: unknown[], value: JsonObject, items: TranscriptParseContext["items"], eventMs?: number) {
  let handled = false;
  for (const item of content) {
    if (!isJsonObject(item) || item.type !== "tool_result") continue;
    handled = true;
    const result = extractContentText(item.content) || extractContentText(value.toolUseResult);
    const id = stringValue(item.tool_use_id) || stringValue(item.toolUseID) || stringValue(item.toolUseId);
    attachToolResult(items, id, result, durationMs(value, result), eventMs);
  }
  return handled;
}

export const claudeTranscriptParser: TranscriptParser = (value, context) => {
  const kind = stringValue(value.type);
  if (kind !== "user" && kind !== "assistant" && value.toolUseResult === undefined) return false;
  collectClaudeMeta(value, context);
  collectClaudeItem(value, context.items);
  return true;
};
