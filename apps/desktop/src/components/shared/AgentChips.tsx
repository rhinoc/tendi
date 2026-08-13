import { Tooltip } from "./Tooltip.tsx";
import { agentClassName, agentIcon } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentChipsProps = {
  agents?: string[];
};

export function AgentChips({ agents = [] }: AgentChipsProps) {
  return (
    <div className="chips">
      {agents.map((agent) => (
        <Tooltip key={agent} content={agent}><span className={`agentIconSurface chip ${agentClassName(agent)}`} key={agent} aria-label={agent}>
          {agentIcon(agent)}
        </span></Tooltip>
      ))}
    </div>
  );
}
