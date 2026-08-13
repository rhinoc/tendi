import { agentClassName, agentIcon } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentFilterOptionLabelProps = {
  agent: string;
  label?: string;
  collapsed?: boolean;
};

export function AgentFilterOptionLabel({ agent, label, collapsed = false }: AgentFilterOptionLabelProps) {
  const isAll = agent === "All";
  return (
    <span className="agentOptionLabel">
      <span className={`agentIconSurface agentOptionIcon ${isAll ? "all" : agentClassName(agent)}`} aria-hidden="true">
        {isAll ? "All" : agentIcon(agent)}
      </span>
      {!collapsed && <span>{label ?? agent}</span>}
    </span>
  );
}
