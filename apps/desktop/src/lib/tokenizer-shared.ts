import { formatTokenCount } from "./token-format.ts";
import { parse as parseYaml } from "yaml";
import type {
  MarkdownTokenStats,
  TokenBreakdownDetail,
  TokenBreakdownSegment,
  TranscriptSkillLink,
  TranscriptTokenItem,
  TranscriptTokenStats,
} from "./tokenizer-types.ts";

export type {
  MarkdownTokenStats,
  TokenBreakdownDetail,
  TokenBreakdownSegment,
  TranscriptSkillLink,
  TranscriptTokenItem,
  TranscriptTokenStats,
} from "./tokenizer-types.ts";

export type TokenCounter = (value: unknown) => number;

const ESTIMATED_CONTEXT_LIMIT = 200_000;

function textValue(value: unknown): string {
  return `${value ?? ""}`;
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

export type MarkdownTokenParts = {
  isSkillMarkdown: boolean;
  texts: string[];
};

export function markdownTokenParts(activePath: string, content: string, selectionText = ""): MarkdownTokenParts {
  const isSkillMarkdown = isSkillMarkdownPath(activePath);
  if (!isSkillMarkdown) return { isSkillMarkdown, texts: [selectionText, content] };
  const parts = splitMarkdownFrontmatter(content);
  return {
    isSkillMarkdown,
    texts: [selectionText, frontmatterScalar(parts.frontmatter, "description"), parts.body],
  };
}

export function markdownTokenStatsFromCounts(parts: MarkdownTokenParts, counts: number[]): MarkdownTokenStats {
  const selection = counts[0] ?? 0;
  if (!parts.isSkillMarkdown) {
    return {
      file: counts[1] ?? 0,
      selection,
      isSkillMarkdown: false,
    };
  }
  const description = counts[1] ?? 0;
  const content = counts[2] ?? 0;
  return {
    file: description + content,
    selection,
    description,
    content,
    isSkillMarkdown: true,
  };
}

export function markdownTokenStats(
  activePath: string,
  content: string,
  selectionText = "",
  count: TokenCounter,
): MarkdownTokenStats {
  const parts = markdownTokenParts(activePath, content, selectionText);
  return markdownTokenStatsFromCounts(parts, parts.texts.map(count));
}

export function markdownTokenSegments(stats: MarkdownTokenStats): TokenBreakdownSegment[] {
  const segments: TokenBreakdownSegment[] = [];
  if (stats.selection > 0) segments.push({ label: "Selection", value: stats.selection });
  if (stats.isSkillMarkdown) {
    segments.push({ label: "Desc", value: stats.description ?? 0 });
    segments.push({ label: "Content", value: stats.content ?? 0 });
  } else {
    segments.push({ label: "File", value: stats.file });
  }
  return segments;
}

function transcriptItemText(item: TranscriptTokenItem): string {
  return textValue(item.body);
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
  const tag = textValue(item.tag).trim();
  if (tag) return tag;
  const command = textValue(item.command).trim();
  const firstWord = command.split(/\s+/)[0];
  return firstWord;
}

function messageLabel(type: string, bucket: "input" | "output"): string {
  if (type === "user") return "User messages";
  if (type === "assistant" || type === "agent") return "Assistant messages";
  if (type === "thinking" || type === "reasoning") return "Reasoning / thinking";
  if (!type) return "";
  return bucket === "output" ? `Other output: ${type}` : `Other input: ${type}`;
}

function isOutputType(type: string): boolean {
  return type === "assistant" || type === "agent" || type === "thinking" || type === "reasoning";
}

export function transcriptTokenTexts(items: TranscriptTokenItem[]): string[] {
  const texts = new Set<string>();
  const add = (value: unknown) => {
    const text = textValue(value);
    if (text) texts.add(text);
  };
  for (const item of items) {
    const type = item.type ?? "";
    if (type === "compaction" || type === "model_config") continue;
    if (type === "toolGroup") {
      for (const tool of item.tools ?? []) {
        add(tool.command);
        add(tool.result);
      }
      continue;
    }
    if (type === "tool") {
      add(item.command);
      add(item.result);
      continue;
    }
    add(item.body);
  }
  return [...texts];
}

function cachedTokenCount(value: unknown, cache: Map<string, number>, count: TokenCounter): number {
  const text = textValue(value);
  if (!text) return 0;
  let tokens = cache.get(text);
  if (tokens == null) {
    tokens = count(text);
    cache.set(text, tokens);
  }
  return tokens;
}

function estimateTranscriptTokensWithCounter(items: TranscriptTokenItem[], count: TokenCounter): TranscriptTokenStats {
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
    const type = item.type ?? "";
    if (type === "compaction" || type === "model_config") continue;
    if (type === "toolGroup") {
      const tools = item.tools ?? [];
      chargeVisibleContext();
      for (const tool of tools) {
        const outputTokens = cachedTokenCount(tool.command, cache, count);
        stats.output += outputTokens;
        visibleContextTokens += outputTokens;
        addDetail(outputDetails, `Tool call: ${toolLabel(tool)}`, outputTokens);
      }
      for (const tool of tools) {
        const inputTokens = cachedTokenCount(tool.result, cache, count);
        visibleContextTokens += inputTokens;
        if (inputTokens > 0) requestPending = true;
      }
      continue;
    }
    if (type === "tool") {
      chargeVisibleContext();
      while (index < items.length && (items[index].type ?? "") === "tool") {
        const tool = items[index];
        const outputTokens = cachedTokenCount(tool.command, cache, count);
        stats.output += outputTokens;
        visibleContextTokens += outputTokens;
        addDetail(outputDetails, `Tool call: ${toolLabel(tool)}`, outputTokens);
        index += 1;
      }
      const groupEnd = index;
      for (let toolIndex = groupEnd - 1; toolIndex >= 0; toolIndex -= 1) {
        const tool = items[toolIndex];
        if ((tool.type ?? "") !== "tool") break;
        const inputTokens = cachedTokenCount(tool.result, cache, count);
        visibleContextTokens += inputTokens;
        if (inputTokens > 0) requestPending = true;
      }
      index = groupEnd - 1;
      continue;
    }

    const tokens = cachedTokenCount(transcriptItemText(item), cache, count);
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

export function estimateTranscriptTokens(
  items: TranscriptTokenItem[],
  count: TokenCounter,
): TranscriptTokenStats {
  return estimateTranscriptTokensWithCounter(items, count);
}

export function estimateTranscriptTokensFromCounts(
  items: TranscriptTokenItem[],
  counts: ReadonlyMap<string, number>,
): TranscriptTokenStats {
  return estimateTranscriptTokensWithCounter(items, (value) => counts.get(textValue(value)) ?? 0);
}

function skillNotes(skillLinks: TranscriptSkillLink[] = []): string[] {
  const counts = new Map<string, number>();
  for (const link of skillLinks) {
    const name = link.skill_name.trim();
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

export function transcriptTokenSegmentsFromStats(
  stats: TranscriptTokenStats,
  skillLinks: TranscriptSkillLink[] = [],
): TokenBreakdownSegment[] {
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

export function transcriptTokenSegments(
  items: TranscriptTokenItem[],
  skillLinks: TranscriptSkillLink[] = [],
  count: TokenCounter,
): TokenBreakdownSegment[] {
  return transcriptTokenSegmentsFromStats(estimateTranscriptTokens(items, count), skillLinks);
}

export function transcriptTokenSegmentsFromCounts(
  items: TranscriptTokenItem[],
  skillLinks: TranscriptSkillLink[] = [],
  counts: ReadonlyMap<string, number>,
): TokenBreakdownSegment[] {
  return transcriptTokenSegmentsFromStats(estimateTranscriptTokensFromCounts(items, counts), skillLinks);
}
