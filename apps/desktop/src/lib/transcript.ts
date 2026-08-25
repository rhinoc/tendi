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

export type TranscriptLocatorItem = {
  index: number;
  label: string;
  response: string;
};

export type TranscriptPage = {
  items: TranscriptItem[];
  locatorItems: TranscriptLocatorItem[];
  warnings: string[];
  nextCursor?: string;
  done: boolean;
  sourceVersion: string;
  restartRequired: boolean;
  unchanged: boolean;
};

export type TranscriptLocatorPage = {
  locatorItems: TranscriptLocatorItem[];
  warnings: string[];
  sourceVersion: string;
};

export type TranscriptSearchScopes = {
  user: boolean;
  assistant: boolean;
  system: boolean;
  tool: boolean;
};

export type TranscriptSearchHit = {
  groupIndex: number;
  toolIndex?: number;
};

export type TranscriptSearchResult = {
  hits: TranscriptSearchHit[];
  warnings: string[];
  sourceVersion: string;
};

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

export type JsonObject = Record<string, unknown>;
type ParsedTokenUsage = NonNullable<JsonlTranscriptParseResult["tokenUsage"]>;

type InternalContextMarker = {
  closing?: string;
  label: string;
  prefix: string;
};

const INTERNAL_CONTEXT_MARKERS: InternalContextMarker[] = [
  { prefix: "# AGENTS.md instructions", label: "AGENTS.md", closing: "</INSTRUCTIONS>" },
  { prefix: "<recommended_plugins>", label: "Recommended plugins", closing: "</recommended_plugins>" },
  { prefix: "<environment_context>", label: "Environment", closing: "</environment_context>" },
  { prefix: "<permissions instructions>", label: "Permissions", closing: "</permissions instructions>" },
  { prefix: "<app-context>", label: "App context", closing: "</app-context>" },
  { prefix: "<collaboration_mode>", label: "Collaboration", closing: "</collaboration_mode>" },
  { prefix: "<skills_instructions>", label: "Skills", closing: "</skills_instructions>" },
  { prefix: "<plugins_instructions>", label: "Plugins", closing: "</plugins_instructions>" },
  { prefix: "<system-reminder>", label: "System reminder", closing: "</system-reminder>" },
  { prefix: "<available_subagent_types>", label: "Subagent types", closing: "</available_subagent_types>" },
  { prefix: "<user_instructions>", label: "User instructions", closing: "</user_instructions>" },
  { prefix: "<local-command-caveat>", label: "Local command", closing: "</local-command-caveat>" },
  { prefix: "<command-name>", label: "Command", closing: "</command-name>" },
  { prefix: "<local-command-stdout>", label: "Command output", closing: "</local-command-stdout>" },
  { prefix: "<task-notification>", label: "Task notification", closing: "</task-notification>" },
  { prefix: "<subagent_notification>", label: "Subagent", closing: "</subagent_notification>" },
  { prefix: "<turn_aborted>", label: "Turn aborted", closing: "</turn_aborted>" },
  { prefix: "<in-app-browser-context", label: "Browser context", closing: "</in-app-browser-context>" },
];

export function createLatestRequestAuthority() {
  let revision = 0;
  return {
    begin(): number {
      revision += 1;
      return revision;
    },
    isCurrent(requestRevision: number): boolean {
      return requestRevision === revision;
    },
    invalidate(requestRevision: number): void {
      if (requestRevision === revision) revision += 1;
    },
  };
}

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
    callId: stringValue(item.call_id ?? item.callId) || undefined,
  }));
}

export function mergeTranscriptItems(
  currentItems: TranscriptItem[],
  incomingItems: TranscriptItem[],
): TranscriptItem[] {
  const merged = [...currentItems];
  for (const item of incomingItems) {
    const type = transcriptItemType(item);
    if (type === "tool_result") {
      let targetIndex = -1;
      if (item.callId) {
        for (let index = merged.length - 1; index >= 0; index -= 1) {
          const candidate = merged[index];
          if (transcriptItemType(candidate) === "tool" && candidate.callId === item.callId) {
            targetIndex = index;
            break;
          }
        }
      }
      if (targetIndex >= 0) {
        merged[targetIndex] = {
          ...merged[targetIndex],
          result: item.result || item.body,
          ...(item.durationMs !== undefined ? { durationMs: item.durationMs } : {}),
        };
      }
      continue;
    }
    merged.push(item);
  }
  return merged;
}

export function transcriptItemsSize(items: TranscriptItem[]): number {
  return items.reduce((total, item) => total + ([
    item.body,
    item.tag,
    item.time,
    item.command,
    item.result,
    item.linkedSessionId,
    item.model,
    item.effort,
    item.callId,
  ] as unknown[]).reduce<number>((itemTotal, value) => itemTotal + `${value ?? ""}`.length, 0), 0);
}

export function normalizeTranscriptPage(value: unknown): TranscriptPage {
  const page = value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
  const locatorItems = Array.isArray(page.locatorItems)
    ? page.locatorItems.flatMap((value) => {
      if (!value || typeof value !== "object" || Array.isArray(value)) return [];
      const item = value as Record<string, unknown>;
      const index = Number(item.index);
      if (!Number.isSafeInteger(index) || index < 0) return [];
      return [{
        index,
        label: `${item.label ?? ""}`,
        response: `${item.response ?? ""}`,
      }];
    })
    : [];
  return {
    items: Array.isArray(page.items)
      ? normalizeTranscript(page.items as Array<Record<string, unknown>>)
      : [],
    locatorItems,
    warnings: Array.isArray(page.warnings)
      ? page.warnings.map((warning) => `${warning}`)
      : [],
    nextCursor: typeof page.nextCursor === "string" && page.nextCursor
      ? page.nextCursor
      : undefined,
    done: page.done === true,
    sourceVersion: `${page.sourceVersion ?? ""}`,
    restartRequired: page.restartRequired === true,
    unchanged: page.unchanged === true,
  };
}

export function normalizeTranscriptLocatorPage(value: unknown): TranscriptLocatorPage {
  const page = value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
  const locatorItems = Array.isArray(page.locatorItems)
    ? page.locatorItems.flatMap((value) => {
      if (!value || typeof value !== "object" || Array.isArray(value)) return [];
      const item = value as Record<string, unknown>;
      const index = Number(item.index);
      if (!Number.isSafeInteger(index) || index < 0) return [];
      return [{
        index,
        label: `${item.label ?? ""}`,
        response: `${item.response ?? ""}`,
      }];
    })
    : [];
  return {
    locatorItems,
    warnings: Array.isArray(page.warnings)
      ? page.warnings.map((warning) => `${warning}`)
      : [],
    sourceVersion: `${page.sourceVersion ?? ""}`,
  };
}

export function normalizeTranscriptSearchResult(value: unknown): TranscriptSearchResult {
  const result = value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {};
  const hits = Array.isArray(result.hits)
    ? result.hits.flatMap((value) => {
      if (!value || typeof value !== "object" || Array.isArray(value)) return [];
      const hit = value as Record<string, unknown>;
      const groupIndex = Number(hit.groupIndex);
      if (!Number.isSafeInteger(groupIndex) || groupIndex < 0) return [];
      const numericToolIndex = hit.toolIndex === undefined ? undefined : Number(hit.toolIndex);
      const validToolIndex = numericToolIndex !== undefined
        && Number.isSafeInteger(numericToolIndex)
        && numericToolIndex >= 0
        ? numericToolIndex
        : undefined;
      return [{
        groupIndex,
        ...(validToolIndex === undefined ? {} : { toolIndex: validToolIndex }),
      }];
    })
    : [];
  return {
    hits,
    warnings: Array.isArray(result.warnings) ? result.warnings.map((warning) => `${warning}`) : [],
    sourceVersion: `${result.sourceVersion ?? ""}`,
  };
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

export function collectGenericItem(value: JsonObject, items: TranscriptItem[]) {
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
  collectMessageContent(content, items, time, kind);
}

function collectRawItem(value: JsonObject, items: TranscriptItem[]) {
  const label = stringValue(value.type) || stringValue(value.kind) || stringValue(value.event) || "jsonl";
  const body = extractContentText(value.message) || extractContentText(value.content) || JSON.stringify(value, null, 2);
  if (body && !isInternalContext(body)) pushItem(items, "tool", truncateText(body, 12_000), label, compactTime(stringValue(value.timestamp)));
}

export function pushItem(
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

export function attachToolResult(items: TranscriptItem[], id: string, result = "", duration?: number, endedAtMs?: number) {
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

export function extractContentText(value: unknown): string {
  if (typeof value === "string") return cleanBody(value);
  if (Array.isArray(value)) {
    return cleanBody(value.map((item) => {
      if (typeof item === "string") return cleanBody(item);
      if (!isJsonObject(item)) return "";
      if (isThinkingContentItem(item)) return "";
      const text = stringValue(item.text) || extractContentText(item.content);
      return cleanBody(text);
    }).filter(Boolean).join("\n"));
  }
  if (isJsonObject(value)) {
    if (isThinkingContentItem(value)) return "";
    return cleanBody(stringValue(value.text) || extractContentText(value.content) || stringValue(value.message) || JSON.stringify(value));
  }
  return "";
}

export function extractRawContentText(value: unknown): string {
  if (typeof value === "string") return value.trim();
  if (Array.isArray(value)) {
    return value.map((item) => extractRawContentText(item)).filter(Boolean).join("\n").trim();
  }
  if (!isJsonObject(value) || isThinkingContentItem(value)) return "";
  return (stringValue(value.text) || extractRawContentText(value.content) || stringValue(value.message)).trim();
}

export function collectMessageContent(value: unknown, items: TranscriptItem[], time: string, role: string) {
  const contentItems = Array.isArray(value) ? value : [value];
  let pendingBody: string[] = [];
  const flushBody = () => {
    if (pendingBody.length === 0) return;
    const body = pendingBody.join("\n");
    pendingBody = [];
    const itemType = role === "user" && isSubagentNotification(body) ? "notification" : role;
    pushItem(items, itemType, body, itemType === "notification" ? "Subagent" : undefined, time);
  };

  for (const contentItem of contentItems) {
    if (!isMessageContentItem(contentItem)) continue;
    const rawBody = extractRawContentText(contentItem);
    for (const segment of splitInternalContextSegments(rawBody)) {
      if (segment.label) {
        flushBody();
        pushItem(items, "context", segment.body, segment.label, time);
      } else {
        const body = cleanBody(segment.body);
        if (body) pendingBody.push(body);
      }
    }
  }
  flushBody();
}

function isMessageContentItem(value: unknown) {
  if (typeof value === "string") return true;
  if (!isJsonObject(value)) return false;
  if (isThinkingContentItem(value)) return false;
  return ![
    "tool_result",
    "tool_use",
    "function_call",
    "function_call_output",
    "custom_tool_call",
    "custom_tool_call_output",
  ].includes(stringValue(value.type));
}

export function extractThinkingText(value: unknown): string {
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
  let text = splitInternalContextSegments(value)
    .filter((segment) => !segment.label)
    .map((segment) => segment.body)
    .join("\n")
    .trim();
  if (!text) return "";
  const userQuery = text.match(/<user_query>\s*([\s\S]*?)\s*<\/user_query>/);
  if (userQuery?.[1]?.trim()) text = userQuery[1].trim();
  return text;
}

function splitInternalContextSegments(value: string) {
  const segments: Array<{ body: string; label?: string }> = [];
  let cursor = 0;
  while (true) {
    const marker = findInternalContextMarker(value, cursor);
    if (!marker) break;
    if (marker.start > cursor) segments.push({ body: value.slice(cursor, marker.start) });
    const closingStart = marker.marker.closing
      ? value.indexOf(marker.marker.closing, marker.start)
      : -1;
    const closingEnd = closingStart >= marker.start
      ? closingStart + marker.marker.closing!.length
      : -1;
    const nextMarker = findInternalContextMarker(value, marker.start + marker.marker.prefix.length);
    const end = closingEnd > marker.start
      ? closingEnd
      : nextMarker?.start ?? value.length;
    if (end <= marker.start) break;
    segments.push({ body: value.slice(marker.start, end).trim(), label: marker.marker.label });
    cursor = end;
  }
  if (cursor < value.length) segments.push({ body: value.slice(cursor) });
  if (segments.length === 0 && value.trim()) segments.push({ body: value.trim() });
  return segments;
}

function findInternalContextMarker(value: string, offset: number) {
  let match: { marker: InternalContextMarker; start: number } | undefined;
  for (const marker of INTERNAL_CONTEXT_MARKERS) {
    let searchFrom = offset;
    while (searchFrom <= value.length) {
      const relativeStart = value.indexOf(marker.prefix, searchFrom);
      if (relativeStart < 0) break;
      if (relativeStart === 0 || value[relativeStart - 1] === "\n") {
        if (!match || relativeStart < match.start) match = { marker, start: relativeStart };
        break;
      }
      searchFrom = relativeStart + marker.prefix.length;
    }
  }
  return match;
}

function isSubagentNotification(text: string) {
  const normalized = text.replace(/\s+/g, " ").trim();
  return normalized === "Briefly inform the user about the task result and perform any follow-up actions (if needed)."
    || normalized.startsWith("The beginning of the above subagent result is already visible to the user. Perform any follow-up actions (if needed).");
}

export function isInternalContext(text: string) {
  return splitInternalContextSegments(text).some((segment) => Boolean(segment.label));
}

function isThinkingContentItem(value: JsonObject) {
  const type = stringValue(value.type);
  return type === "thinking" || type === "reasoning" || type === "summary_text";
}

export function summarizeToolCall(value: JsonObject, fallback: string) {
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

export function extractToolCommand(value: JsonObject) {
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

export function extractToolResult(value: JsonObject) {
  const output = value.output ?? value.result ?? value.content;
  if (typeof output === "string") return truncateText(output.trim(), 12_000);
  if (isJsonObject(output) || Array.isArray(output)) return truncateText(JSON.stringify(output, null, 2), 12_000);
  return "";
}

export function durationMs(value: JsonObject, output?: string) {
  for (const key of ["duration_ms", "durationMs", "elapsed_ms", "elapsedMs"]) {
    const raw = value[key];
    if (typeof raw === "number" && Number.isFinite(raw)) return Math.max(0, Math.round(raw));
  }
  const match = output?.match(/Wall time:\s*([0-9.]+)\s*seconds/);
  return match ? Math.round(Number(match[1]) * 1000) : undefined;
}

export function callId(value: JsonObject) {
  return stringValue(value.call_id) || stringValue(value.callId) || stringValue(value.id);
}

export function extractTitle(value: JsonObject) {
  const body = extractContentText(value.message ?? value.content ?? (isJsonObject(value.payload) ? value.payload.content : undefined));
  return body ? body.split(/\r?\n/)[0].trim().slice(0, 80) : undefined;
}

export function compactTime(value: string) {
  return value.split("T")[1]?.slice(0, 5) || value;
}

export function timestampMs(value: unknown) {
  const time = Date.parse(stringValue(value));
  return Number.isFinite(time) ? time : undefined;
}

export function stringAt(value: JsonObject, path: string[]) {
  let current: unknown = value;
  for (const key of path) {
    if (!isJsonObject(current)) return "";
    current = current[key];
  }
  return stringValue(current);
}

export function numberAt(value: JsonObject, path: string[]) {
  let current: unknown = value;
  for (const key of path) {
    if (!isJsonObject(current)) return undefined;
    current = current[key];
  }
  return typeof current === "number" && Number.isFinite(current) && current >= 0 ? current : undefined;
}

export function stringValue(value: unknown) {
  return typeof value === "string" ? value : "";
}

export function truncateText(value: string, limit: number) {
  return value.length > limit ? `${value.slice(0, limit)}\n... truncated` : value;
}

export function isJsonObject(value: unknown): value is JsonObject {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}
