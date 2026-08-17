import codexIcon from "@lobehub/icons-static-svg/icons/codex-color.svg";

import type { AgentDefinition } from "./types.ts";

export const codexAgent: AgentDefinition = {
  id: "codex",
  aliases: ["codex"],
  displayName: "Codex",
  icon: codexIcon,
  transcriptFormat: "codex",
};
