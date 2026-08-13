export type TranscriptItem = {
  type?: string;
  kind?: string;
  body?: string;
  tag?: string;
  time?: string;
  command?: string;
  result?: string;
  durationMs?: string | number;
  linkedSessionId?: string;
  model?: string;
  effort?: string;
  [key: string]: unknown;
};

export type TranscriptGroup = TranscriptItem | { type: "toolGroup"; tools: TranscriptItem[] };

export type JsonlTranscriptParseResult = {
  items: TranscriptItem[];
  warnings: string[];
  lineCount: number;
  parsedCount: number;
  startedAt?: string;
  updatedAt?: string;
  title?: string;
  project?: string;
  tokenUsage?: {
    inputTokens: number;
    cachedInputTokens: number;
    outputTokens: number;
    reasoningOutputTokens: number;
    totalTokens: number;
  };
};

type JsonObject = Record<string, unknown>;
type ParsedTokenUsage = NonNullable<JsonlTranscriptParseResult["tokenUsage"]>;

export function normalizeTranscript(items: Array<Record<string, unknown>>): TranscriptItem[] {
  return items.map((item) => ({
    type: `${item.type ?? item.kind ?? ""}`,
    body: `${item.body ?? item.text ?? item.content ?? ""}`,
    tag: `${item.tag ?? item.name ?? item.tool ?? ""}`,
    time: `${item.time ?? item.timestamp ?? ""}`,
    command: `${item.command ?? item.cmd ?? ""}`,
    result: `${item.result ?? item.output ?? item.return ?? ""}`,
    durationMs: (item.duration_ms ?? item.durationMs ?? item.elapsed_ms ?? item.elapsedMs ?? "") as string | number,
    linkedSessionId: stringValue(item.linked_session_id ?? item.linkedSessionId) || undefined,
    model: stringValue(item.model) || undefined,
    effort: stringValue(item.effort) || undefined,
  }));
}

export function transcriptItemType(item: TranscriptItem): string | undefined {
  return item.type ?? item.kind;
}

export function groupTranscriptItems(items: TranscriptItem[]): TranscriptGroup[] {
  const grouped: TranscriptGroup[] = [];
  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    if (transcriptItemType(item) !== "tool") {
      grouped.push(item);
      continue;
    }

    const tools = [item];
    while (index + 1 < items.length && transcriptItemType(items[index + 1]) === "tool") {
      index += 1;
      tools.push(items[index]);
    }

    grouped.push(tools.length > 1 ? { type: "toolGroup", tools } : item);
  }
  return grouped;
}

export function parseJsonlTranscript(text: string): JsonlTranscriptParseResult {
  const items: TranscriptItem[] = [];
  const warnings: string[] = [];
  const meta: Omit<JsonlTranscriptParseResult, "items" | "warnings" | "lineCount" | "parsedCount"> = {};
  const claudeMessageUsage = new Map<string, ParsedTokenUsage>();
  let lineCount = 0;
  let parsedCount = 0;

  text.split(/\r?\n/).forEach((line, index) => {
    if (!line.trim()) return;
    lineCount += 1;
    const beforeCount = items.length;
    let value: unknown;
    try {
      value = JSON.parse(line);
      parsedCount += 1;
    } catch (error) {
      warnings.push(`Line ${index + 1}: ${error instanceof Error ? error.message : `${error}`}`);
      return;
    }
    if (!isJsonObject(value)) return;
    collectJsonlMeta(value, meta, claudeMessageUsage);
    if (value.type === "response_item" || value.type === "event_msg" || value.type === "compacted" || value.type === "turn_context") {
      collectCodexItem(value, items);
      return;
    }
    if (value.type === "user" || value.type === "assistant" || value.toolUseResult !== undefined) {
      collectClaudeItem(value, items);
      return;
    }
    collectGenericItem(value, items);
    if (items.length === beforeCount) collectRawItem(value, items);
  });

  if (!meta.tokenUsage && claudeMessageUsage.size > 0) {
    meta.tokenUsage = sumTokenUsage(claudeMessageUsage.values());
  }

  return { items, warnings, lineCount, parsedCount, ...meta };
}

function collectJsonlMeta(
  value: JsonObject,
  meta: Partial<JsonlTranscriptParseResult>,
  claudeMessageUsage: Map<string, ParsedTokenUsage>,
) {
  const timestamp = stringAt(value, ["timestamp"]) || stringAt(value, ["payload", "timestamp"]);
  if (timestamp) {
    if (!meta.startedAt || timestamp < meta.startedAt) meta.startedAt = timestamp;
    if (!meta.updatedAt || timestamp > meta.updatedAt) meta.updatedAt = timestamp;
  }
  meta.project ||= stringAt(value, ["cwd"]) || stringAt(value, ["payload", "cwd"]);
  meta.title ??= extractTitle(value);
  if (stringAt(value, ["payload", "type"]) === "token_count") {
    const inputTokens = numberAt(value, ["payload", "info", "total_token_usage", "input_tokens"]);
    const cachedInputTokens = numberAt(value, ["payload", "info", "total_token_usage", "cached_input_tokens"]);
    const outputTokens = numberAt(value, ["payload", "info", "total_token_usage", "output_tokens"]);
    const reasoningOutputTokens = numberAt(value, ["payload", "info", "total_token_usage", "reasoning_output_tokens"]) ?? 0;
    const totalTokens = numberAt(value, ["payload", "info", "total_token_usage", "total_tokens"]);
    if (inputTokens !== undefined && cachedInputTokens !== undefined && outputTokens !== undefined && totalTokens !== undefined && totalTokens > 0) {
      meta.tokenUsage = { inputTokens, cachedInputTokens, outputTokens, reasoningOutputTokens, totalTokens };
    }
  }
  const claudeUsage = extractClaudeTokenUsage(value);
  if (claudeUsage) claudeMessageUsage.set(claudeUsage.messageId, claudeUsage.tokenUsage);
}

function extractClaudeTokenUsage(value: JsonObject) {
  if (value.type !== "assistant") return undefined;
  const messageId = stringAt(value, ["message", "id"]);
  const directInputTokens = numberAt(value, ["message", "usage", "input_tokens"]);
  const outputTokens = numberAt(value, ["message", "usage", "output_tokens"]);
  if (!messageId || directInputTokens === undefined || outputTokens === undefined) return undefined;
  const cacheCreationInputTokens = numberAt(value, ["message", "usage", "cache_creation_input_tokens"]) ?? 0;
  const cachedInputTokens = numberAt(value, ["message", "usage", "cache_read_input_tokens"]) ?? 0;
  const inputTokens = directInputTokens + cacheCreationInputTokens + cachedInputTokens;
  if (inputTokens + outputTokens <= 0) return undefined;
  return {
    messageId,
    tokenUsage: {
      inputTokens,
      cachedInputTokens,
      outputTokens,
      reasoningOutputTokens: 0,
      totalTokens: inputTokens + outputTokens,
    },
  };
}

function sumTokenUsage(usages: Iterable<ParsedTokenUsage>): ParsedTokenUsage {
  const total = {
    inputTokens: 0,
    cachedInputTokens: 0,
    outputTokens: 0,
    reasoningOutputTokens: 0,
    totalTokens: 0,
  };
  for (const usage of usages) {
    total.inputTokens += usage.inputTokens;
    total.cachedInputTokens += usage.cachedInputTokens;
    total.outputTokens += usage.outputTokens;
    total.reasoningOutputTokens += usage.reasoningOutputTokens;
    total.totalTokens += usage.totalTokens;
  }
  return total;
}

function collectCodexItem(value: JsonObject, items: TranscriptItem[]) {
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
      const body = extractRawContentText(payload.content);
      if (body) pushItem(items, "context", body, role === "system" ? "System" : "Developer", time);
      return;
    }
    if (role !== "user" && role !== "assistant") return;
    if (role === "user") collectInternalContextItems(payload.content, items, time);
    const body = extractContentText(payload.content);
    if (body) {
      const itemType = role === "user" && isSubagentNotification(body) ? "notification" : role;
      pushItem(items, itemType, body, itemType === "notification" ? "Subagent" : undefined, time);
    }
    return;
  }
  if (payloadType === "reasoning" || payloadType === "thinking") {
    const body = extractThinkingText(payload.summary ?? payload.content ?? payload);
    if (body) pushItem(items, payloadType === "thinking" ? "thinking" : "reasoning", body, undefined, time);
    return;
  }
  if (payloadType === "function_call" || payloadType === "custom_tool_call" || payloadType === "local_shell_call") {
    const tag = stringValue(payload.name) || stringAt(payload, ["action", "type"]) || "tool_call";
    pushItem(items, "tool", summarizeToolCall(payload, tag), tag, time, extractToolCommand(payload), undefined, durationMs(payload), callId(payload), timestampMs(value.timestamp));
    return;
  }
  if (payloadType === "function_call_output" || payloadType === "custom_tool_call_output") {
    attachToolResult(items, callId(payload), extractToolResult(payload), durationMs(payload), timestampMs(value.timestamp));
    return;
  }
  if (payloadType === "web_search_call" || payloadType === "image_generation_call") {
    pushItem(items, "tool", payloadType.replaceAll("_", " "), payloadType, time);
  }
}

function pushCodexCompaction(items: TranscriptItem[], value: JsonObject) {
  const time = compactTime(stringValue(value.timestamp));
  const previous = items.at(-1);
  if (previous?.type === "compaction" && previous.time === time) return;
  pushItem(items, "compaction", "Context compacted", undefined, time);
}

function pushCodexModelConfig(items: TranscriptItem[], value: JsonObject) {
  const payload = isJsonObject(value.payload) ? value.payload : undefined;
  const settings = value.type === "turn_context"
    ? payload
    : payload && isJsonObject(payload.thread_settings) ? payload.thread_settings : undefined;
  const previous = [...items].reverse().find((item) => item.type === "model_config");
  const model = stringValue(settings?.model) || previous?.model || "";
  const effort = stringValue(settings?.effort) || stringValue(settings?.reasoning_effort) || previous?.effort || "";
  if ((!model && !effort) || (previous?.model === model && previous?.effort === effort)) return;
  const body = [
    model && `Model: ${model}`,
    effort && `Effort: ${effort}`,
  ].filter(Boolean).join("\n");
  pushItem(items, "model_config", body, undefined, compactTime(stringValue(value.timestamp)));
  Object.assign(items.at(-1) ?? {}, { model, effort });
}

function attachCodexSubagentSession(value: JsonObject, items: TranscriptItem[]) {
  const payload = isJsonObject(value.payload) ? value.payload : undefined;
  if (!payload || payload.type !== "sub_agent_activity" || payload.kind !== "started") return false;
  const eventId = stringValue(payload.event_id);
  const sessionId = stringValue(payload.agent_thread_id);
  if (!eventId || !sessionId) return false;
  const item = [...items].reverse().find((candidate) => (
    candidate.type === "tool"
    && candidate.tag === "spawn_agent"
    && candidate.callId === eventId
  ));
  if (!item) return false;
  item.linkedSessionId = sessionId;
  return true;
}

function collectClaudeItem(value: JsonObject, items: TranscriptItem[]) {
  const kind = stringValue(value.type);
  const time = compactTime(stringValue(value.timestamp));
  const message = isJsonObject(value.message) ? value.message : undefined;
  const content = message?.content;
  const eventMs = timestampMs(value.timestamp);

  if (kind === "user") collectInternalContextItems(content, items, time);
  if ((kind === "user" || kind === "assistant") && Array.isArray(content) && attachClaudeToolResults(content, value, items, time, eventMs)) return;
  if (kind === "user" || kind === "assistant") {
    if (kind === "assistant") {
      const thinking = extractThinkingText(content);
      if (thinking) pushItem(items, "thinking", thinking, undefined, time);
    }
    const body = extractContentText(content);
    if (body) {
      const itemType = kind === "user" && isSubagentNotification(body) ? "notification" : kind;
      pushItem(items, itemType, body, itemType === "notification" ? "Subagent" : undefined, time);
    }
    if (Array.isArray(content)) {
      for (const item of content) {
        if (!isJsonObject(item) || item.type !== "tool_use") continue;
        const tag = stringValue(item.name) || "tool_call";
        pushItem(items, "tool", summarizeToolCall(item, tag), tag, time, extractToolCommand(item), undefined, durationMs(item), stringValue(item.id), eventMs);
      }
    }
  }

  if (value.toolUseResult !== undefined) {
    const result = extractContentText(value.toolUseResult) || "tool result";
    if (!attachToolResult(items, stringValue(value.toolUseID) || stringValue(value.tool_use_id) || stringValue(value.toolUseId), result, durationMs(value), eventMs)) {
      pushItem(items, "tool", result, "tool_result", time);
    }
  }
}

function attachClaudeToolResults(content: unknown[], value: JsonObject, items: TranscriptItem[], time: string, eventMs?: number) {
  let handled = false;
  for (const item of content) {
    if (!isJsonObject(item) || item.type !== "tool_result") continue;
    handled = true;
    const result = extractContentText(item.content) || extractContentText(value.toolUseResult) || "tool result";
    const id = stringValue(item.tool_use_id) || stringValue(item.toolUseID) || stringValue(item.toolUseId);
    if (!attachToolResult(items, id, result, durationMs(value, result), eventMs)) {
      pushItem(items, "tool", result, "tool_result", time);
    }
  }
  return handled;
}

function collectGenericItem(value: JsonObject, items: TranscriptItem[]) {
  const kind = stringValue(value.role) || stringValue(value.type);
  const time = compactTime(stringValue(value.timestamp));
  const message = isJsonObject(value.message) ? value.message.content : value.message;
  const content = message ?? value.content;
  if (kind === "developer" || kind === "system") {
    const body = extractRawContentText(content);
    if (body) pushItem(items, "context", body, kind === "system" ? "System" : "Developer", time);
    return;
  }
  if (kind !== "user" && kind !== "assistant") return;
  if (kind === "user") collectInternalContextItems(content, items, time);
  const body = extractContentText(content);
  if (body) {
    const itemType = kind === "user" && isSubagentNotification(body) ? "notification" : kind;
    pushItem(items, itemType, body, itemType === "notification" ? "Subagent" : undefined, time);
  }
}

function collectRawItem(value: JsonObject, items: TranscriptItem[]) {
  const label = stringValue(value.type) || stringValue(value.kind) || stringValue(value.event) || "jsonl";
  const body = extractContentText(value.message) || extractContentText(value.content) || JSON.stringify(value, null, 2);
  if (body && !isInternalContext(body)) pushItem(items, "tool", truncateText(body, 12_000), label, compactTime(stringValue(value.timestamp)));
}

function pushItem(
  items: TranscriptItem[],
  type: string,
  body: string,
  tag?: string,
  time = "",
  command?: string,
  result?: string,
  durationMs?: string | number,
  callId?: string,
  startedAtMs?: number,
) {
  items.push({ type, body, tag, time, command, result, durationMs, callId, startedAtMs });
}

function attachToolResult(items: TranscriptItem[], id: string, result = "", duration?: number, endedAtMs?: number) {
  if (!result) return false;
  const item = [...items].reverse().find((candidate) => (
    candidate.type === "tool" && (id ? candidate.callId === id : !candidate.result)
  ));
  if (!item) return false;
  item.result = truncateText(result.trim(), 12_000);
  const elapsed = typeof item.startedAtMs === "number" && typeof endedAtMs === "number"
    ? Math.max(0, endedAtMs - item.startedAtMs)
    : undefined;
  if (!item.durationMs) item.durationMs = duration || elapsed || "";
  return true;
}

function extractContentText(value: unknown): string {
  if (typeof value === "string") return cleanBody(value);
  if (Array.isArray(value)) {
    return cleanBody(value.map((item) => {
      if (typeof item === "string") return item;
      if (!isJsonObject(item)) return "";
      if (isThinkingContentItem(item)) return "";
      return stringValue(item.text) || extractContentText(item.content);
    }).filter(Boolean).join("\n"));
  }
  if (isJsonObject(value)) {
    if (isThinkingContentItem(value)) return "";
    return cleanBody(stringValue(value.text) || extractContentText(value.content) || stringValue(value.message) || JSON.stringify(value));
  }
  return "";
}

function extractRawContentText(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (Array.isArray(value)) {
    return value.map((item) => extractRawContentText(item)).filter(Boolean).join("\n").trim();
  }
  if (!isJsonObject(value) || isThinkingContentItem(value)) return "";
  return (stringValue(value.text) || extractRawContentText(value.content) || stringValue(value.message)).trim();
}

function collectInternalContextItems(value: unknown, items: TranscriptItem[], time: string) {
  const contentItems = Array.isArray(value) ? value : [value];
  for (const contentItem of contentItems) {
    const body = extractRawContentText(contentItem);
    const label = internalContextLabel(body);
    if (body && label) pushItem(items, "context", body, label, time);
  }
}

function extractThinkingText(value: unknown): string {
  if (typeof value === "string") return cleanBody(value);
  if (Array.isArray(value)) {
    return cleanBody(value.map((item) => {
      if (!isJsonObject(item) || !isThinkingContentItem(item)) return "";
      return stringValue(item.thinking) || stringValue(item.text) || extractThinkingText(item.content);
    }).filter(Boolean).join("\n"));
  }
  if (!isJsonObject(value)) return "";
  return cleanBody(
    stringValue(value.thinking) ||
    stringValue(value.text) ||
    extractThinkingText(value.summary) ||
    extractThinkingText(value.content),
  );
}

function cleanBody(value: string) {
  let text = value.trim();
  if (!text || isInternalContext(text)) return "";
  const userQuery = text.match(/<user_query>\s*([\s\S]*?)\s*<\/user_query>/);
  if (userQuery?.[1]?.trim()) text = userQuery[1].trim();
  if (
    text.startsWith("<local-command-caveat>") ||
    text.startsWith("<command-name>") ||
    text.startsWith("<local-command-stdout>") ||
    text.startsWith("<task-notification>")
  ) return "";
  return text;
}

function isSubagentNotification(text: string) {
  const normalized = text.replace(/\s+/g, " ").trim();
  return normalized === "Briefly inform the user about the task result and perform any follow-up actions (if needed)."
    || normalized.startsWith("The beginning of the above subagent result is already visible to the user. Perform any follow-up actions (if needed).");
}

function isInternalContext(text: string) {
  return Boolean(internalContextLabel(text));
}

function internalContextLabel(text: string) {
  const contextTypes: Array<[string, string]> = [
    ["# AGENTS.md instructions", "AGENTS.md"],
    ["<codex_internal_context", "Codex internal"],
    ["<environment_context>", "Environment"],
    ["<permissions instructions>", "Permissions"],
    ["<app-context>", "App context"],
    ["<collaboration_mode>", "Collaboration"],
    ["<skills_instructions>", "Skills"],
    ["<plugins_instructions>", "Plugins"],
    ["<system-reminder>", "System reminder"],
    ["<available_subagent_types>", "Subagent types"],
  ];
  return contextTypes.find(([prefix]) => text.startsWith(prefix))?.[1] ?? "";
}

function isThinkingContentItem(value: JsonObject) {
  const type = stringValue(value.type);
  return type === "thinking" || type === "reasoning" || type === "summary_text";
}

function summarizeToolCall(value: JsonObject, fallback: string) {
  const command = extractToolCommand(value);
  if (command) return truncateText(command, 220);
  const args = value.arguments;
  if (typeof args === "string") {
    try {
      const parsed = JSON.parse(args);
      if (isJsonObject(parsed)) return truncateText(stringValue(parsed.cmd) || stringValue(parsed.command) || args, 220);
    } catch {
      return truncateText(args, 220);
    }
  }
  if (value.input !== undefined) return truncateText(JSON.stringify(value.input), 220);
  return fallback;
}

function extractToolCommand(value: JsonObject) {
  const command =
    stringAt(value, ["arguments", "cmd"]) ||
    stringAt(value, ["arguments", "command"]) ||
    stringAt(value, ["action", "command"]) ||
    stringAt(value, ["input", "command"]) ||
    stringAt(value, ["input", "cmd"]);
  if (command) return truncateText(command.trim(), 4_000);
  if (typeof value.arguments !== "string") return "";
  try {
    const parsed = JSON.parse(value.arguments);
    return isJsonObject(parsed) ? truncateText((stringValue(parsed.cmd) || stringValue(parsed.command)).trim(), 4_000) : "";
  } catch {
    return "";
  }
}

function extractToolResult(value: JsonObject) {
  const output = value.output ?? value.result ?? value.content;
  if (typeof output === "string") return truncateText(output.trim(), 12_000);
  if (isJsonObject(output) || Array.isArray(output)) return truncateText(JSON.stringify(output, null, 2), 12_000);
  return "";
}

function durationMs(value: JsonObject, output?: string) {
  for (const key of ["duration_ms", "durationMs", "elapsed_ms", "elapsedMs"]) {
    const raw = value[key];
    if (typeof raw === "number" && Number.isFinite(raw)) return Math.max(0, Math.round(raw));
  }
  const match = output?.match(/Wall time:\s*([0-9.]+)\s*seconds/);
  return match ? Math.round(Number(match[1]) * 1000) : undefined;
}

function callId(value: JsonObject) {
  return stringValue(value.call_id) || stringValue(value.callId) || stringValue(value.id);
}

function extractTitle(value: JsonObject) {
  const body = extractContentText(value.message ?? value.content ?? (isJsonObject(value.payload) ? value.payload.content : undefined));
  return body ? body.split(/\r?\n/)[0].trim().slice(0, 80) : undefined;
}

function compactTime(value: string) {
  return value.split("T")[1]?.slice(0, 5) || value;
}

function timestampMs(value: unknown) {
  const time = Date.parse(stringValue(value));
  return Number.isFinite(time) ? time : undefined;
}

function stringAt(value: JsonObject, path: string[]) {
  let current: unknown = value;
  for (const key of path) {
    if (!isJsonObject(current)) return "";
    current = current[key];
  }
  return stringValue(current);
}

function numberAt(value: JsonObject, path: string[]) {
  let current: unknown = value;
  for (const key of path) {
    if (!isJsonObject(current)) return undefined;
    current = current[key];
  }
  return typeof current === "number" && Number.isFinite(current) && current >= 0 ? current : undefined;
}

function stringValue(value: unknown) {
  return typeof value === "string" ? value : "";
}

function truncateText(value: string, limit: number) {
  return value.length > limit ? `${value.slice(0, limit)}\n... truncated` : value;
}

function isJsonObject(value: unknown): value is JsonObject {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
