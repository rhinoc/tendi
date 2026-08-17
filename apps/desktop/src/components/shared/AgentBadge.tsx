import { agentClassName, agentIcon } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentBadgeProps = {
  agent: string;
  small?: boolean;
};

export function AgentBadge({ agent, small = false }: AgentBadgeProps) {
  return (
    <span className={`agentIconSurface agentPill ${agentClassName(agent)} ${small ? "small" : ""}`} aria-label={agent}>
      {agentIcon(agent)}
    </span>
  );
}
