import { countTokens as countO200kTokens } from "gpt-tokenizer/encoding/o200k_base";
import { parse as parseYaml } from "yaml";

export const TOKENIZER_LABEL = "OpenAI o200k_base";
export const TOKENIZER_PACKAGE = "gpt-tokenizer";
export const TOKENIZER_URL = "https://github.com/niieani/gpt-tokenizer";
const ESTIMATED_CONTEXT_LIMIT = 200_000;

export { cacheRateTone, tokenTone, tokenToneClass } from "./token-style.ts";
export type { TokenTone } from "./token-style.ts";

export type MarkdownTokenStats = {
  file: number;
  selection: number;
  description?: number;
  content?: number;
  isSkillMarkdown: boolean;
};

export type TranscriptTokenStats = {
  input: number;
  output: number;
  total: number;
  inputDetails?: TokenBreakdownDetail[];
  outputDetails?: TokenBreakdownDetail[];
};

export type TranscriptTokenItem = {
  type?: string;
  kind?: string;
  body?: string;
  tag?: string;
  command?: string;
  result?: string;
  tools?: TranscriptTokenItem[];
};

export type TranscriptSkillLink = {
  skill_name?: string;
  skillName?: string;
  evidence_kind?: string;
  evidenceKind?: string;
};

export type TokenBreakdownDetail = {
  label: string;
  value: number;
};

export type TokenBreakdownSegment = {
  label: string;
  value: number;
  details?: TokenBreakdownDetail[];
  notes?: string[];
};

export function countTextTokens(value: unknown): number {
  const text = `${value ?? ""}`;
  if (!text) return 0;
  return countO200kTokens(text);
}

export function formatTokenCount(value: number): string {
  if (!Number.isFinite(value)) return "0";
  if (value < 1000) return `${value}`;
  if (value < 10000) return `${(value / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  if (value < 1000000) return `${Math.round(value / 1000)}k`;
  if (value < 10000000) return `${(value / 1000000).toFixed(1).replace(/\.0$/, "")}M`;
  return `${Math.round(value / 1000000)}M`;
}

export function tokenEstimateTitle(): string {
  return `Estimated with ${TOKENIZER_LABEL} using ${TOKENIZER_PACKAGE}; not exact billing.`;
}

function splitMarkdownFrontmatter(content: string): { frontmatter: string; body: string } {
  if (!content.startsWith("---\n")) return { frontmatter: "", body: content };
  const endIndex = content.indexOf("\n---", 4);
  if (endIndex < 0) return { frontmatter: "", body: content };
  const afterFence = endIndex + 4;
  let nextOffset = content[afterFence] === "\n" ? afterFence + 1 : afterFence;
  while (content[nextOffset] === "\n") nextOffset += 1;
  return {
    frontmatter: content.slice(4, endIndex),
    body: content.slice(nextOffset),
  };
}

function frontmatterScalar(frontmatter: string, key: string): string {
  try {
    const parsed = parseYaml(frontmatter);
    const value = parsed && typeof parsed === "object" ? (parsed as Record<string, unknown>)[key] : undefined;
    if (typeof value === "string") return value;
    if (value == null) return "";
    return typeof value === "object" ? JSON.stringify(value) : `${value}`;
  } catch {
    // Fall back to a simple scalar read for partially-written frontmatter.
  }
  const match = frontmatter.match(new RegExp(`^${key}:\\s*(.*)$`, "mi"));
  if (!match) return "";
  return match[1].trim().replace(/^['"]|['"]$/g, "");
}

function isSkillMarkdownPath(path: string): boolean {
  return path.split(/[\\/]/).pop()?.toLowerCase() === "skill.md";
}

export function markdownTokenStats(activePath: string, content: string, selectionText = ""): MarkdownTokenStats {
  const isSkillMarkdown = isSkillMarkdownPath(activePath);
  const selection = selectionText ? countTextTokens(selectionText) : 0;
  if (!isSkillMarkdown) {
    return {
      file: countTextTokens(content),
      selection,
      isSkillMarkdown,
    };
  }

  const parts = splitMarkdownFrontmatter(content);
  const description = countTextTokens(frontmatterScalar(parts.frontmatter, "description"));
  const body = countTextTokens(parts.body);
  return {
    file: description + body,
    selection,
    description,
    content: body,
    isSkillMarkdown,
  };
}

function transcriptItemText(item: TranscriptTokenItem): string {
  return `${item.body ?? ""}`;
}

function detailList(details: Map<string, number>): TokenBreakdownDetail[] {
  return [...details]
    .map(([label, value]) => ({ label, value }))
    .filter((detail) => detail.value > 0)
    .sort((a, b) => b.value - a.value || a.label.localeCompare(b.label));
}

function addDetail(details: Map<string, number>, label: string, value: number): void {
  if (value <= 0) return;
  details.set(label, (details.get(label) ?? 0) + value);
}

function toolLabel(item: TranscriptTokenItem): string {
  const tag = `${item.tag ?? ""}`.trim();
  if (tag) return tag;
  const command = `${item.command || item.body || ""}`.trim();
  const firstWord = command.split(/\s+/)[0];
  return firstWord || "tool";
}

function messageLabel(type: string, bucket: "input" | "output"): string {
  if (type === "user") return "User messages";
  if (type === "assistant" || type === "agent") return "Assistant messages";
  if (type === "thinking" || type === "reasoning") return "Reasoning / thinking";
  return bucket === "output" ? `Other output: ${type || "unknown"}` : `Other input: ${type || "unknown"}`;
}

function cachedTokenCount(value: unknown, cache: Map<string, number>): number {
  const text = `${value ?? ""}`;
  if (!text) return 0;
  let tokens = cache.get(text);
  if (tokens == null) {
    tokens = countTextTokens(text);
    cache.set(text, tokens);
  }
  return tokens;
}

function isOutputType(type: string): boolean {
  return type === "assistant" || type === "agent" || type === "thinking" || type === "reasoning";
}

function skillNotes(skillLinks: TranscriptSkillLink[] = []): string[] {
  const counts = new Map<string, number>();
  for (const link of skillLinks) {
    const name = `${link.skill_name ?? link.skillName ?? ""}`.trim();
    if (!name) continue;
    counts.set(name, (counts.get(name) ?? 0) + 1);
  }
  if (counts.size === 0) return [];
  const skills = [...counts]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, 6)
    .map(([name, count]) => (count > 1 ? `${name} x${count}` : name))
    .join(", ");
  const extra = counts.size > 6 ? `, +${counts.size - 6} more` : "";
  return [`Observed skills: ${skills}${extra}. Skill reads are included in tool call/result buckets when present.`];
}

export function estimateTranscriptTokens(items: TranscriptTokenItem[]): TranscriptTokenStats {
  const stats: TranscriptTokenStats = { input: 0, output: 0, total: 0 };
  const cache = new Map<string, number>();
  const inputDetails = new Map<string, number>();
  const outputDetails = new Map<string, number>();
  let visibleContextTokens = 0;
  let requestPending = false;

  const chargeVisibleContext = () => {
    if (!requestPending) return;
    const tokens = Math.min(visibleContextTokens, ESTIMATED_CONTEXT_LIMIT);
    stats.input += tokens;
    addDetail(inputDetails, "Visible request context", tokens);
    requestPending = false;
  };

  for (let index = 0; index < items.length; index += 1) {
    const item = items[index];
    const type = item.type ?? item.kind ?? "";
    if (type === "compaction" || type === "model_config") continue;
    if (type === "toolGroup") {
      const tools = item.tools ?? [];
      chargeVisibleContext();
      for (const tool of tools) {
        const outputTokens = cachedTokenCount(tool.command || tool.body, cache);
        stats.output += outputTokens;
        visibleContextTokens += outputTokens;
        addDetail(outputDetails, `Tool call: ${toolLabel(tool)}`, outputTokens);
      }
      for (const tool of tools) {
        const inputTokens = cachedTokenCount(tool.result, cache);
        visibleContextTokens += inputTokens;
        if (inputTokens > 0) requestPending = true;
      }
      continue;
    }
    if (type === "tool") {
      chargeVisibleContext();
      while (index < items.length && (items[index].type ?? items[index].kind ?? "") === "tool") {
        const tool = items[index];
        const outputTokens = cachedTokenCount(tool.command || tool.body, cache);
        stats.output += outputTokens;
        visibleContextTokens += outputTokens;
        addDetail(outputDetails, `Tool call: ${toolLabel(tool)}`, outputTokens);
        index += 1;
      }
      const groupEnd = index;
      for (let toolIndex = groupEnd - 1; toolIndex >= 0; toolIndex -= 1) {
        const tool = items[toolIndex];
        if ((tool.type ?? tool.kind ?? "") !== "tool") break;
        const inputTokens = cachedTokenCount(tool.result, cache);
        visibleContextTokens += inputTokens;
        if (inputTokens > 0) requestPending = true;
      }
      index = groupEnd - 1;
      continue;
    }

    const tokens = cachedTokenCount(transcriptItemText(item), cache);
    if (isOutputType(type)) {
      chargeVisibleContext();
      stats.output += tokens;
      visibleContextTokens += tokens;
      addDetail(outputDetails, messageLabel(type, "output"), tokens);
    } else {
      visibleContextTokens += tokens;
      if (tokens > 0) requestPending = true;
    }
  }
  chargeVisibleContext();
  stats.total = stats.input + stats.output;
  stats.inputDetails = detailList(inputDetails);
  stats.outputDetails = detailList(outputDetails);
  return stats;
}

export function transcriptTokenSegments(
  items: TranscriptTokenItem[],
  skillLinks: TranscriptSkillLink[] = [],
): TokenBreakdownSegment[] {
  const stats = estimateTranscriptTokens(items);
  const notes = [
    `Estimated input replays visible context for each model response, capped at ${formatTokenCount(ESTIMATED_CONTEXT_LIMIT)} tokens per request.`,
    "System prompts, tool schemas, cache behavior, and hidden service context are not available; billing usage can differ substantially.",
    ...skillNotes(skillLinks),
  ];
  return [
    { label: "Input", value: stats.input, details: stats.inputDetails, notes },
    { label: "Output", value: stats.output, details: stats.outputDetails, notes },
    {
      label: "Total",
      value: stats.total,
      details: [
        { label: "Input", value: stats.input },
        { label: "Output", value: stats.output },
      ],
      notes,
    },
  ];
}
