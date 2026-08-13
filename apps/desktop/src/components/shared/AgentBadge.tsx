import { Tooltip } from "./Tooltip.tsx";
import { agentClassName, agentIcon } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentBadgeProps = {
  agent: string;
  small?: boolean;
};

export function AgentBadge({ agent, small = false }: AgentBadgeProps) {
  return (
    <Tooltip content={agent}><span className={`agentIconSurface agentPill ${agentClassName(agent)} ${small ? "small" : ""}`}>
      {agentIcon(agent)}
    </span></Tooltip>
  );
}
