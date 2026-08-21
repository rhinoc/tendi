import { agentClassName, agentIcon } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentChipsProps = {
  agents?: string[];
  onAgentClick?: (agent: string) => void;
};

export function AgentChips({ agents = [], onAgentClick }: AgentChipsProps) {
  return (
    <div className="chips">
      {agents.map((agent) => {
        const className = "agentIconSurface chip " + agentClassName(agent);
        return onAgentClick ? (
          <button
            type="button"
            className={className + " agentChipButton"}
            key={agent}
            aria-label={agent}
            onClick={(event) => {
              event.stopPropagation();
              onAgentClick(agent);
            }}
          >
            {agentIcon(agent)}
          </button>
        ) : (
          <span className={className} key={agent} aria-label={agent}>
            {agentIcon(agent)}
          </span>
        );
      })}
    </div>
  );
}
