import { agentClassName, agentIcon, targetAgentLabel } from "../../lib/index.ts";
import "./agent-surface.css";

export type AgentOptionLabelProps = {
  agent: string;
  label?: string;
};

export function AgentOptionLabel({ agent, label }: AgentOptionLabelProps) {
  return (
    <span className="agentOptionLabel">
      <span className={`agentIconSurface agentOptionIcon ${agentClassName(agent)}`} aria-hidden="true">
        {agentIcon(agent)}
      </span>
      <span>{label ?? targetAgentLabel(agent)}</span>
    </span>
  );
}
