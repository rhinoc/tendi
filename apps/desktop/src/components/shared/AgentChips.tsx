import { agentClassName, agentIcon } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentChipsProps = {
  agents?: string[];
};

export function AgentChips({ agents = [] }: AgentChipsProps) {
  return (
    <div className="chips">
      {agents.map((agent) => (
        <span className={`agentIconSurface chip ${agentClassName(agent)}`} key={agent} aria-label={agent}>
          {agentIcon(agent)}
        </span>
      ))}
    </div>
  );
}
