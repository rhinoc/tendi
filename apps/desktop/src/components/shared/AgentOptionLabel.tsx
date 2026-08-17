import { agentClassName, agentIcon, targetAgentLabel } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentOptionLabelProps = {
  agent: string;
  label?: string;
  variant?: "target" | "filter";
  collapsed?: boolean;
};

export function AgentOptionLabel({ agent, label, variant = "target", collapsed = false }: AgentOptionLabelProps) {
  const isFilter = variant === "filter";
  const isAll = isFilter && agent === "All";
  return (
    <span className="agentOptionLabel">
      <span className={`agentIconSurface agentOptionIcon ${isAll ? "all" : agentClassName(agent)}`} aria-hidden="true">
        {isAll ? "All" : agentIcon(agent)}
      </span>
      {!collapsed && <span>{label ?? (isFilter ? agent : targetAgentLabel(agent))}</span>}
    </span>
  );
}
