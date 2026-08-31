import { extractTitle, isJsonObject, stringAt } from "../transcript.ts";
import type { JsonObject, JsonlTranscriptParseResult, TranscriptItem } from "../transcript.ts";
import { compareTimestamps } from "../time.ts";
import { agentDefinitions } from "./index.ts";
import type { AgentDefinition, TranscriptParser } from "./types.ts";

type ParsedTokenUsage = NonNullable<JsonlTranscriptParseResult["tokenUsage"]>;

/** Parse using one provider's owner. Callers that already know the source
 * must use this entry point so another provider cannot claim the record first.
 */
export function parseJsonlTranscriptForProvider(
  text: string,
  providerId: string,
  definitions: readonly AgentDefinition[] = agentDefinitions,
): JsonlTranscriptParseResult {
  const definition = definitions.find((candidate) => (
    candidate.id === providerId || candidate.aliases.includes(providerId)
  ));
  if (!definition?.transcriptParser) {
    return {
      items: [],
      warnings: [`Unsupported transcript provider: ${providerId}`],
      lineCount: text.split(/\r?\n/).filter((line) => line.trim()).length,
      parsedCount: 0,
    };
  }
  return parseJsonlTranscriptWithParsers(text, [definition.transcriptParser]);
}

function parseJsonlTranscriptWithParsers(
  text: string,
  parsers: readonly TranscriptParser[],
): JsonlTranscriptParseResult {
  const items: TranscriptItem[] = [];
  const warnings: string[] = [];
  const meta: Partial<JsonlTranscriptParseResult> = {};
  const messageUsage = new Map<string, ParsedTokenUsage>();
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
    warnings.push(`Line ${index + 1}: unsupported transcript record was ignored`);
  });

  if (!meta.tokenUsage && messageUsage.size > 0) {
    meta.tokenUsage = sumTokenUsage(messageUsage.values());
  }

  return { items, warnings, lineCount, parsedCount, ...meta };
}

function collectJsonlMeta(value: JsonObject, meta: Partial<JsonlTranscriptParseResult>) {
  const timestamp = stringAt(value, ["timestamp"]) || stringAt(value, ["payload", "timestamp"]);
  if (timestamp) updateJsonlTimeBounds(meta, timestamp);
  meta.project ||= stringAt(value, ["cwd"]) || stringAt(value, ["payload", "cwd"]);
  meta.title ??= extractTitle(value);
}

function updateJsonlTimeBounds(meta: Partial<JsonlTranscriptParseResult>, timestamp: string) {
  if (
    !meta.startedAt
    || compareTimestamps(timestamp, meta.startedAt) < 0
  ) {
    meta.startedAt = timestamp;
  }
  if (
    !meta.updatedAt
    || compareTimestamps(timestamp, meta.updatedAt) > 0
  ) {
    meta.updatedAt = timestamp;
  }
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
