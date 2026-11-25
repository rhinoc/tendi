import antigravityIcon from "../../assets/agent-icons/antigravity.svg";
import type { AgentDefinition } from "./types.ts";

export const antigravityAgent: AgentDefinition = {
  id: "antigravity",
  aliases: ["antigravity", "antigravitycli", "agy"],
  displayName: "Antigravity",
  icon: antigravityIcon,
  skillVisibility: false,
};
