import { ALL_AGENT_FILTER, agentClassName, agentIcon, targetAgentLabel } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentOptionLabelProps = {
  agent: string;
  label?: string;
  variant?: "target" | "filter";
  collapsed?: boolean;
};

export function AgentOptionLabel({ agent, label, variant = "target", collapsed = false }: AgentOptionLabelProps) {
  const isFilter = variant === "filter";
  const isAll = isFilter && agent === ALL_AGENT_FILTER;
  return (
    <span className="agentOptionLabel">
      <span className={`agentIconSurface agentOptionIcon ${isAll ? "all" : agentClassName(agent)}`} aria-hidden="true">
        {isAll ? <span className="agentOptionIconText">All</span> : agentIcon(agent)}
      </span>
      {!collapsed && <span>{label ?? (isFilter ? agent : targetAgentLabel(agent))}</span>}
    </span>
  );
}
