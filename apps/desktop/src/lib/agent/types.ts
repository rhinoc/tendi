import type { JsonObject, JsonlTranscriptParseResult, TranscriptItem } from "../transcript.ts";

export type AgentTranscriptFormat = "codex" | "claude" | "generic";

export type TranscriptParseContext = {
  items: TranscriptItem[];
  meta: Partial<JsonlTranscriptParseResult>;
  messageUsage: Map<string, NonNullable<JsonlTranscriptParseResult["tokenUsage"]>>;
};

export type TranscriptParser = (value: JsonObject, context: TranscriptParseContext) => boolean;

export type AgentSessionAppResumeInput = {
  id: string;
  project?: string;
  projectPath?: string;
};

export type AgentDefinition = {
  id: string;
  aliases: readonly string[];
  displayName: string;
  trendClass?: string;
  icon?: string;
  transcriptFormat: AgentTranscriptFormat;
  transcriptParser?: TranscriptParser;
  sessionAppDeepLink?: (session: AgentSessionAppResumeInput) => string;
};
