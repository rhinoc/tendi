import { Tooltip } from "../components/shared/Tooltip.tsx";
import type { ColumnDef } from "../components/DataTable.types";
import { agentClassName, agentIcon, basename, friendlyAgent, titleValue } from "./index.ts";

type AgentRow = { agent?: string | null };

function AgentBadge({ agent, small = false }: { agent: string; small?: boolean }) {
  return (
    <Tooltip content={agent}><span className={`agentIconSurface agentPill ${agentClassName(agent)} ${small ? "small" : ""}`}>
      {agentIcon(agent)}
    </span></Tooltip>
  );
}

export const agentColumn: ColumnDef<AgentRow> = {
  key: "agent",
  header: "Agent",
  label: "Agent",
  type: "enum",
  width: "78px",
  groupBy: (row) => friendlyAgent(row.agent),
  sortValue: (row) => friendlyAgent(row.agent).toLowerCase(),
  render: (row) => <AgentBadge agent={friendlyAgent(row.agent)} />,
};

type RuleRow = {
  agent?: string | null;
  kind?: string | null;
  scope?: string | null;
  order?: number | null;
  path?: string | null;
  source?: string | null;
};

export const ruleColumns: ColumnDef<RuleRow>[] = [
  { ...agentColumn, width: "78px" },
  { key: "kind", header: "Kind", label: "Kind", type: "enum", width: "96px" },
  { key: "scope", header: "Scope", label: "Scope", type: "enum", width: "96px" },
  {
    key: "order",
    header: "Order",
    label: "Order",
    type: "text",
    width: "72px",
    sortValue: (row) => Number(row.order) || 0,
  },
  {
    key: "source",
    header: "Source",
    label: "Source",
    type: "text",
    width: "minmax(120px, 1fr)",
    value: (row) => basename(row.path ?? row.source),
  },
];

type McpRow = {
  agent?: string | null;
  name?: string | null;
  scope?: string | null;
  transport?: string | null;
  status?: string | null;
  path?: string | null;
  source?: string | null;
};

export const mcpColumns: ColumnDef<McpRow>[] = [
  { ...agentColumn, sticky: true, width: "var(--data-freeze-column-width, 96px)" },
  {
    key: "name",
    header: "Name",
    label: "Name",
    type: "text",
    width: "150px",
    sortValue: (row) => `${row.name ?? ""}`.toLowerCase(),
  },
  {
    key: "scope",
    header: "Scope",
    label: "Scope",
    type: "text",
    width: "180px",
    sortValue: (row) => `${row.scope ?? ""}`.toLowerCase(),
  },
  {
    key: "transport",
    header: "Transport",
    label: "Transport",
    type: "enum",
    width: "120px",
    sortValue: (row) => `${row.transport ?? ""}`.toLowerCase(),
  },
  {
    key: "status",
    header: "Status",
    label: "Status",
    type: "enum",
    width: "100px",
    sortValue: (row) => titleValue(row.status).toLowerCase(),
    value: (row) => titleValue(row.status),
  },
  {
    key: "source",
    header: "Source",
    label: "Source",
    type: "text",
    width: "minmax(0, 1fr)",
    sortValue: (row) => basename(row.path ?? row.source).toLowerCase(),
    value: (row) => basename(row.path ?? row.source),
  },
];
