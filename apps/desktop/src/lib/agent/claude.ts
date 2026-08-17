import claudeIcon from "@lobehub/icons-static-svg/icons/claude-color.svg";

import type { AgentDefinition } from "./types.ts";

export const claudeAgent: AgentDefinition = {
  id: "claude",
  aliases: ["claude", "claudecode"],
  displayName: "Claude",
  icon: claudeIcon,
  transcriptFormat: "claude",
};
