import claudeIcon from "@lobehub/icons-static-svg/icons/claude-color.svg";

import { claudeTranscriptParser } from "./claude-transcript.ts";
import { AgentTranscriptFormat, type AgentDefinition } from "./types.ts";

export const claudeAgent: AgentDefinition = {
  id: "claude",
  aliases: ["claude", "claudecode"],
  displayName: "Claude",
  trendClass: "agentClaude",
  icon: claudeIcon,
  transcriptFormat: AgentTranscriptFormat.Claude,
  transcriptParser: claudeTranscriptParser,
  sessionAppDeepLink: ({ id, project, projectPath }) => {
    const params = new URLSearchParams({ session: id });
    const cwd = projectPath || project;
    if (cwd) params.set("cwd", cwd);
    return `claude://resume?${params.toString()}`;
  },
};
