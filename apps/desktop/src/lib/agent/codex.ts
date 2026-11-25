import codexIcon from "@lobehub/icons-static-svg/icons/codex-color.svg";

import { codexTranscriptParser } from "./codex-transcript.ts";
import { AgentTranscriptFormat, type AgentDefinition } from "./types.ts";

export const codexAgent: AgentDefinition = {
  id: "codex",
  aliases: ["codex"],
  displayName: "Codex",
  trendClass: "agentCodex",
  icon: codexIcon,
  supportsAssistant: true,
  transcriptFormat: AgentTranscriptFormat.Codex,
  transcriptParser: codexTranscriptParser,
  sessionAppDeepLink: ({ id }) => `codex://threads/${encodeURIComponent(id)}`,
};
