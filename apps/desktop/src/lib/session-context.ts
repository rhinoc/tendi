export const CONTEXT_BANDS = ["context", "user", "assistant", "reasoning", "tool", "skill"] as const;

export type ContextBand = (typeof CONTEXT_BANDS)[number];

export const CONTEXT_BAND_LABEL: Record<ContextBand, string> = {
  context: "context",
  user: "user",
  assistant: "asst",
  reasoning: "think",
  tool: "tool",
  skill: "skill",
};

export type ContextSourceItem = {
  type: string;
  body?: string;
  tag?: string;
  command?: string;
  result?: string;
  durationMs?: string | number;
  tools?: ContextSourceItem[];
};

export type ContextFileKind = "read" | "write" | "search";

export type ContextFileRow = {
  path: string;
  read: number;
  write: number;
  search: number;
  key: string;
};

export type ContextStep = {
  key: string;
  turn: number;
  total: number;
  compacted: boolean;
  you: string;
  youKey: string;
  incoming: string;
  incomingKey: string;
  reply: string;
  bands: Record<ContextBand, number>;
};

export type ContextBandShare = {
  band: ContextBand;
  tokens: number;
  share: number;
};

export type SessionContextStory = {
  bands: ContextBandShare[];
  steps: ContextStep[];
  files: ContextFileRow[];
};

export type SessionUsagePartKind = "cached" | "uncached" | "reasoning" | "answer";

export type SessionUsagePart = {
  kind: SessionUsagePartKind;
  tokens: number;
};

type UsageCounts = {
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
};

const FILE_LIMIT = 40;
const PATH_KEYS = ["path", "file_path", "filePath", "target_file", "target_notebook", "notebook_path"];

export function contextShareLabel(share: number, tokens: number) {
  if (tokens > 0 && share < 1) return "<1%";
  return `${share}%`;
}

export function emptyContextBands(): Record<ContextBand, number> {
  return { context: 0, user: 0, assistant: 0, reasoning: 0, tool: 0, skill: 0 };
}

export function sessionUsageParts(usage: UsageCounts): SessionUsagePart[] {
  const uncached = Math.max(0, usage.inputTokens - usage.cachedInputTokens);
  const answer = Math.max(0, usage.outputTokens - usage.reasoningOutputTokens);
  const parts: SessionUsagePart[] = [
    { kind: "cached", tokens: Math.max(0, usage.cachedInputTokens) },
    { kind: "uncached", tokens: uncached },
    { kind: "reasoning", tokens: Math.max(0, usage.reasoningOutputTokens) },
    { kind: "answer", tokens: answer },
  ];
  return parts.filter((part) => part.tokens > 0);
}

export function orderSessionFamily<T extends { id: string; parentSessionId?: string }>(
  sessions: readonly T[],
): { session: T; depth: number }[] {
  const ids = new Set(sessions.map((session) => session.id));
  const byParent = new Map<string, T[]>();
  for (const session of sessions) {
    const parent = session.parentSessionId && ids.has(session.parentSessionId) ? session.parentSessionId : "";
    const list = byParent.get(parent) ?? [];
    list.push(session);
    byParent.set(parent, list);
  }
  const rows: { session: T; depth: number }[] = [];
  const seen = new Set<string>();
  const visit = (parentId: string, depth: number) => {
    for (const session of byParent.get(parentId) ?? []) {
      if (seen.has(session.id)) continue;
      seen.add(session.id);
      rows.push({ session, depth });
      visit(session.id, depth + 1);
    }
  };
  visit("", 0);
  return rows;
}

export function buildSessionContext(
  items: readonly ContextSourceItem[],
  counts: ReadonlyMap<string, number>,
): SessionContextStory {
  const windowBands = emptyContextBands();
  const steps: ContextStep[] = [];
  const files = new Map<string, ContextFileRow>();
  let turn = 0;
  let you = "";
  let youKey = "";
  let incoming = "";
  let incomingKey = "";

  const pushStep = (key: string, compacted: boolean, reply: string) => {
    steps.push({
      key,
      turn,
      total: compacted ? 0 : bandTotal(windowBands),
      compacted,
      you,
      youKey,
      incoming: compacted ? "" : incoming,
      incomingKey: compacted ? "" : incomingKey,
      reply,
      bands: compacted ? emptyContextBands() : { ...windowBands },
    });
    if (!compacted) {
      incoming = "";
      incomingKey = "";
    }
  };

  items.forEach((item, index) => {
    if (item.type === "toolGroup") {
      item.tools?.forEach((tool, childIndex) => {
        absorbItem(tool, childKey(index, childIndex, tool.type));
      });
      return;
    }
    absorbItem(item, contextItemKey(item.type, index));
  });

  return {
    bands: bandShares(windowBands),
    steps,
    files: [...files.values()].sort((left, right) => fileOps(right) - fileOps(left) || left.path.localeCompare(right.path)).slice(0, FILE_LIMIT),
  };

  function absorbItem(item: ContextSourceItem, key: string) {
    const type = item.type;
    if (type === "compaction") {
      incoming = "";
      incomingKey = "";
      pushStep(key, true, "Context compacted");
      for (const band of CONTEXT_BANDS) windowBands[band] = 0;
      return;
    }
    if (type === "model_config" || type === "toolGroup") return;
    if (type === "user") {
      turn += 1;
      you = firstLine(item.body);
      youKey = key;
      addBand(windowBands, "user", countedTokens(item.body, counts));
      return;
    }
    if (type === "assistant" || type === "agent") {
      addBand(windowBands, "assistant", countedTokens(item.body, counts));
      pushStep(key, false, firstLine(item.body));
      return;
    }
    if (type === "thinking" || type === "reasoning") {
      addBand(windowBands, "reasoning", countedTokens(item.body, counts));
      return;
    }
    if (type === "context") {
      addBand(windowBands, skillTag(item.tag) ? "skill" : "context", countedTokens(item.body, counts));
      return;
    }
    if (type === "notification") {
      addBand(windowBands, "context", countedTokens(item.body, counts));
      return;
    }
    if (type !== "tool") return;

    const command = item.command?.trim() || item.body?.trim() || "";
    const kind = fileKind(item.tag ?? "", command);
    const skill = skillTag(item.tag) || commandMentionsSkill(command);
    const tokens = countedTokens(item.command, counts) + countedTokens(item.result, counts) + (item.command?.trim() ? 0 : countedTokens(item.body, counts));
    addBand(windowBands, skill ? "skill" : "tool", tokens);
    if (!skill && (kind === "read" || kind === "write" || kind === "search")) {
      for (const path of pathsFromCommand(command)) rememberFile(files, path, kind, key);
    }
    const shell = kind === "command" ? shellLine(command) : "";
    const entered = firstLine(pathsFromCommand(command)[0] || shell || item.tag);
    if (entered) {
      incoming = entered;
      incomingKey = key;
    }
  }
}

function contextItemKey(prefix: string, index: string | number) {
  return `${prefix}-${index}`;
}

function childKey(groupIndex: number, childIndex: number, type: string) {
  return contextItemKey(type, `${groupIndex}-${childIndex}`);
}

function addBand(bands: Record<ContextBand, number>, band: ContextBand, tokens: number) {
  if (tokens <= 0) return;
  bands[band] += tokens;
}

function bandTotal(bands: Record<ContextBand, number>) {
  return CONTEXT_BANDS.reduce((total, band) => total + bands[band], 0);
}

function bandShares(bands: Record<ContextBand, number>): ContextBandShare[] {
  const present = CONTEXT_BANDS.filter((band) => bands[band] > 0).map((band) => ({ band, tokens: bands[band] }));
  const total = present.reduce((sum, part) => sum + part.tokens, 0);
  if (total <= 0) return [];
  const exact = present.map((part) => (part.tokens / total) * 100);
  const shares = exact.map((value) => Math.floor(value));
  let leftover = 100 - shares.reduce((sum, value) => sum + value, 0);
  const order = exact.map((value, index) => ({ index, fraction: value - shares[index] }))
    .sort((left, right) => right.fraction - left.fraction || left.index - right.index);
  for (let index = 0; index < leftover && order[index]; index += 1) shares[order[index].index] += 1;
  return present.map((part, index) => ({ ...part, share: shares[index] }));
}

function countedTokens(value: string | undefined, counts: ReadonlyMap<string, number>) {
  const text = value?.trim() ?? "";
  if (!text) return 0;
  return counts.get(text) ?? 0;
}

export function contextTokenTexts(items: readonly ContextSourceItem[]): string[] {
  const texts = new Set<string>();
  const add = (value: string | undefined) => {
    const text = value?.trim() ?? "";
    if (text) texts.add(text);
  };
  const visit = (item: ContextSourceItem) => {
    if (item.type === "toolGroup") {
      item.tools?.forEach(visit);
      return;
    }
    if (item.type === "tool") {
      add(item.command);
      add(item.result);
      if (!item.command?.trim()) add(item.body);
      return;
    }
    if (item.type === "user" || item.type === "assistant" || item.type === "agent" || item.type === "thinking" || item.type === "reasoning" || item.type === "context" || item.type === "notification") {
      add(item.body);
    }
  };
  items.forEach(visit);
  return [...texts];
}

function firstLine(value: string | undefined, limit = 80) {
  const line = value?.split(/\r?\n/).find((entry) => entry.trim())?.trim() ?? "";
  if (line.length <= limit) return line;
  return `${line.slice(0, limit - 3)}...`;
}

function skillTag(tag: string | undefined) {
  return Boolean(tag && /skill/i.test(tag));
}

function commandMentionsSkill(command: string) {
  return /SKILL\.md\b/i.test(command);
}

function fileKind(tag: string, command: string): ContextFileKind | "command" | "" {
  if (tag.trim().toLowerCase() === "exec") return "command";
  const name = (tag || command.trim().split(/\s+/)[0] || "").replace(/[^a-z_]/gi, "").toLowerCase();
  if (/^(read|cat|head|tail|open|view)$/.test(name)) return "read";
  if (/^(write|edit|strreplace|searchreplace|applypatch|delete|notebookedit|multiedit)$/.test(name)) return "write";
  if (/^(grep|rg|glob|find|search|codebasesearch|semanticsearch)$/.test(name)) return "search";
  if (/^(exec|shell|bash|runterminalcmd|terminal|powershell)$/.test(name)) return "command";
  return "";
}

function pathsFromCommand(command: string): string[] {
  const trimmed = command.trim();
  if (!trimmed) return [];
  if (trimmed.startsWith("{") || trimmed.startsWith("[")) {
    try {
      return collectPaths(JSON.parse(trimmed));
    } catch {
      return [];
    }
  }
  return [];
}

function collectPaths(value: unknown, depth = 0): string[] {
  if (depth > 3 || !value || typeof value !== "object") return [];
  if (Array.isArray(value)) return value.flatMap((item) => collectPaths(item, depth + 1));
  const record = value as Record<string, unknown>;
  const found: string[] = [];
  for (const key of PATH_KEYS) {
    const path = record[key];
    if (typeof path === "string" && path.trim()) found.push(path.trim());
  }
  return found;
}

function rememberFile(files: Map<string, ContextFileRow>, path: string, kind: ContextFileKind, key: string) {
  const current = files.get(path) ?? { path, read: 0, write: 0, search: 0, key };
  current[kind] += 1;
  current.key = key;
  files.set(path, current);
}

function fileOps(row: ContextFileRow) {
  return row.read + row.write + row.search;
}

function shellLine(command: string) {
  const line = firstLine(command, 160);
  const program = programName(line);
  if (!line || !/^[\w.+@:-]+$/.test(program)) return "";
  return line;
}

function programName(command: string) {
  const token = command.trim().split(/\s+/)[0] ?? "";
  const base = token.split(/[/\\]/).pop() ?? token;
  return base || "command";
}
