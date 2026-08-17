export type AgentTranscriptFormat = "codex" | "claude" | "generic";

export type AgentDefinition = {
  id: string;
  aliases: readonly string[];
  displayName: string;
  icon?: string;
  transcriptFormat: AgentTranscriptFormat;
};
