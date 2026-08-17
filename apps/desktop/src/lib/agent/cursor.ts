import cursorIcon from "@lobehub/icons-static-svg/icons/cursor.svg";

import type { AgentDefinition } from "./types.ts";

export const cursorAgent: AgentDefinition = {
  id: "cursor",
  aliases: ["cursor"],
  displayName: "Cursor",
  icon: cursorIcon,
  transcriptFormat: "generic",
};
