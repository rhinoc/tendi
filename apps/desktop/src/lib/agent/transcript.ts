import {
  collectGenericItem,
  extractTitle,
  isJsonObject,
  stringAt,
} from "../transcript.ts";
import type { JsonObject, JsonlTranscriptParseResult, TranscriptItem } from "../transcript.ts";
import { agentDefinitions } from "./index.ts";
import type { AgentDefinition, TranscriptParser } from "./types.ts";

type ParsedTokenUsage = NonNullable<JsonlTranscriptParseResult["tokenUsage"]>;

export function parseJsonlTranscript(
  text: string,
  definitions: readonly AgentDefinition[] = agentDefinitions,
): JsonlTranscriptParseResult {
  const items: TranscriptItem[] = [];
  const warnings: string[] = [];
  const meta: Partial<JsonlTranscriptParseResult> = {};
  const messageUsage = new Map<string, ParsedTokenUsage>();
  const parsers = definitions.flatMap<TranscriptParser>((definition) => (
    definition.transcriptFormat !== "generic" && definition.transcriptParser
      ? [definition.transcriptParser]
      : []
  ));
  let lineCount = 0;
  let parsedCount = 0;

  text.split(/\r?\n/).forEach((line, index) => {
    if (!line.trim()) return;
    lineCount += 1;
    let value: unknown;
    try {
      value = JSON.parse(line);
      parsedCount += 1;
    } catch (error) {
      warnings.push(`Line ${index + 1}: ${error instanceof Error ? error.message : `${error}`}`);
      return;
    }
    if (!isJsonObject(value)) return;
    collectJsonlMeta(value, meta);
    if (parsers.some((parser) => parser(value, { items, meta, messageUsage }))) return;
    collectGenericItem(value, items);
  });

  if (!meta.tokenUsage && messageUsage.size > 0) {
    meta.tokenUsage = sumTokenUsage(messageUsage.values());
  }

  return { items, warnings, lineCount, parsedCount, ...meta };
}

function collectJsonlMeta(value: JsonObject, meta: Partial<JsonlTranscriptParseResult>) {
  const timestamp = stringAt(value, ["timestamp"]) || stringAt(value, ["payload", "timestamp"]);
  if (timestamp) {
    if (!meta.startedAt || timestamp < meta.startedAt) meta.startedAt = timestamp;
    if (!meta.updatedAt || timestamp > meta.updatedAt) meta.updatedAt = timestamp;
  }
  meta.project ||= stringAt(value, ["cwd"]) || stringAt(value, ["payload", "cwd"]);
  meta.title ??= extractTitle(value);
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
