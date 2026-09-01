import {
  attachToolResult,
  callId,
  collectMessageContent,
  compactTime,
  durationMs,
  extractRawContentText,
  extractThinkingText,
  extractToolCommand,
  extractToolResult,
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

function stripCodexInternalContext(value: string): string {
  return value
    .replace(/(^|\n)<codex_internal_context[\s\S]*?<\/codex_internal_context>\s*/g, "$1")
    .replace(/(^|\n)<codex_internal_context[\s\S]*$/g, "$1")
    .trim();
}

function collectCodexMeta(value: JsonObject, context: TranscriptParseContext) {
  if (stringAt(value, ["payload", "type"]) !== "token_count") return;
  const inputTokens = numberAt(value, ["payload", "info", "total_token_usage", "input_tokens"]);
  const cachedInputTokens = numberAt(value, ["payload", "info", "total_token_usage", "cached_input_tokens"]);
  const outputTokens = numberAt(value, ["payload", "info", "total_token_usage", "output_tokens"]);
  const reasoningOutputTokens = numberAt(value, ["payload", "info", "total_token_usage", "reasoning_output_tokens"]) ?? 0;
  const totalTokens = numberAt(value, ["payload", "info", "total_token_usage", "total_tokens"]);
  if (inputTokens !== undefined && cachedInputTokens !== undefined && outputTokens !== undefined && totalTokens !== undefined && totalTokens > 0) {
    context.meta.tokenUsage = { inputTokens, cachedInputTokens, outputTokens, reasoningOutputTokens, totalTokens };
  }
}

function collectCodexItem(value: JsonObject, items: TranscriptParseContext["items"]) {
  if (value.type === "compacted") {
    pushCodexCompaction(items, value);
    return;
  }
  if (value.type === "event_msg") {
    if (isJsonObject(value.payload) && value.payload.type === "context_compacted") {
      pushCodexCompaction(items, value);
      return;
    }
    if (isJsonObject(value.payload) && value.payload.type === "thread_settings_applied") {
      pushCodexModelConfig(items, value);
      return;
    }
    attachCodexSubagentSession(value, items);
    return;
  }
  if (value.type === "turn_context") {
    pushCodexModelConfig(items, value);
    return;
  }
  if (value.type !== "response_item") return;
  const payload = value.payload;
  if (!isJsonObject(payload)) return;
  const time = compactTime(stringValue(value.timestamp));
  const payloadType = stringValue(payload.type);
  if (payloadType === "message") {
    const role = stringValue(payload.role);
    if (role === "developer" || role === "system") {
      const body = stripCodexInternalContext(extractRawContentText(payload.content));
      if (body) pushItem(items, "context", body, role === "system" ? "System" : "Developer", time);
      return;
    }
    if (role !== "user" && role !== "assistant") return;
    const start = items.length;
    collectMessageContent(payload.content, items, time, role);
    for (let index = items.length - 1; index >= start; index -= 1) {
      const body = stripCodexInternalContext(items[index].body);
      if (body) items[index].body = body;
      else items.splice(index, 1);
    }
    return;
  }
  if (payloadType === "reasoning" || payloadType === "thinking") {
    const body = extractThinkingText(payload.summary ?? payload.content ?? payload);
    if (body) pushItem(items, payloadType === "thinking" ? "thinking" : "reasoning", body, undefined, time);
    return;
  }
  if (payloadType === "function_call" || payloadType === "custom_tool_call" || payloadType === "local_shell_call") {
    const tag = stringValue(payload.name) || stringAt(payload, ["action", "type"]) || undefined;
    pushItem(items, "tool", summarizeToolCall(payload), tag, time, extractToolCommand(payload), undefined, durationMs(payload), callId(payload), timestampMs(value.timestamp));
    return;
  }
  if (payloadType === "function_call_output" || payloadType === "custom_tool_call_output") {
    attachToolResult(items, callId(payload), extractToolResult(payload), durationMs(payload), timestampMs(value.timestamp));
    return;
  }
  if (payloadType === "web_search_call" || payloadType === "image_generation_call") {
    pushItem(items, "tool", summarizeToolCall(payload), payloadType, time, extractToolCommand(payload), undefined, durationMs(payload), callId(payload), timestampMs(value.timestamp));
  }
}

function pushCodexCompaction(items: TranscriptParseContext["items"], value: JsonObject) {
  const time = compactTime(stringValue(value.timestamp));
  const previous = items.at(-1);
  if (previous?.type === "compaction" && previous.time === time) return;
  pushItem(items, "compaction", "Context compacted", undefined, time);
}

function pushCodexModelConfig(items: TranscriptParseContext["items"], value: JsonObject) {
  const payload = isJsonObject(value.payload) ? value.payload : undefined;
  const settings = value.type === "turn_context"
    ? payload
    : payload && isJsonObject(payload.thread_settings) ? payload.thread_settings : undefined;
  const previous = [...items].reverse().find((item) => item.type === "model_config");
  const model = stringValue(settings?.model) || previous?.model || "";
  const effort = stringValue(settings?.effort) || stringValue(settings?.reasoning_effort) || previous?.effort || "";
  if ((!model && !effort) || (previous?.model === model && previous?.effort === effort)) return;
  const body = [model && `Model: ${model}`, effort && `Effort: ${effort}`].filter(Boolean).join("\n");
  pushItem(items, "model_config", body, undefined, compactTime(stringValue(value.timestamp)));
  Object.assign(items.at(-1) ?? {}, { model, effort });
}

function attachCodexSubagentSession(value: JsonObject, items: TranscriptParseContext["items"]) {
  const payload = isJsonObject(value.payload) ? value.payload : undefined;
  if (!payload || payload.type !== "sub_agent_activity" || payload.kind !== "started") return false;
  const eventId = stringValue(payload.event_id);
  const sessionId = stringValue(payload.agent_thread_id);
  if (!eventId || !sessionId) return false;
  const item = [...items].reverse().find((candidate) => candidate.type === "tool" && candidate.tag === "spawn_agent" && candidate.callId === eventId);
  if (!item) return false;
  item.linkedSessionId = sessionId;
  return true;
}

export const codexTranscriptParser: TranscriptParser = (value, context) => {
  if (!(["response_item", "event_msg", "compacted", "turn_context"] as unknown[]).includes(value.type)) return false;
  collectCodexMeta(value, context);
  collectCodexItem(value, context.items);
  return true;
};
