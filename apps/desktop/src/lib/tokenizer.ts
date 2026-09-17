import { countTokens as countO200kTokens } from "gpt-tokenizer/encoding/o200k_base";

import {
  estimateTranscriptTokens as estimateTranscriptTokensWithCounter,
  markdownTokenStats as markdownTokenStatsWithCounter,
  transcriptTokenSegments as transcriptTokenSegmentsWithCounter,
} from "./tokenizer-shared.ts";

export * from "./tokenizer-shared.ts";
export { cacheRateTone, tokenTone, tokenToneClass } from "./token-style.ts";
export { TokenTone } from "./token-style.ts";

export function countTextTokens(value: unknown): number {
  const text = `${value ?? ""}`;
  if (!text) return 0;
  return countO200kTokens(text);
}

export function markdownTokenStats(activePath: string, content: string, selectionText = "") {
  return markdownTokenStatsWithCounter(activePath, content, selectionText, countTextTokens);
}

export function estimateTranscriptTokens(items: import("./tokenizer-types.ts").TranscriptTokenItem[]) {
  return estimateTranscriptTokensWithCounter(items, countTextTokens);
}

export function transcriptTokenSegments(
  items: import("./tokenizer-types.ts").TranscriptTokenItem[],
  skillLinks: import("./tokenizer-types.ts").TranscriptSkillLink[] = [],
) {
  return transcriptTokenSegmentsWithCounter(items, skillLinks, countTextTokens);
}
