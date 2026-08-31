import { ColumnDataType, type ColumnDef } from "../components/DataTable.types";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { AgentChips } from "../components/shared/AgentChips.tsx";
import { basename, friendlyAgent, titleValue } from "./index.ts";
import type { McpRecord, RuleRecord } from "./index.ts";

type AgentRow = { agent?: string | null };

export const agentColumn: ColumnDef<AgentRow> = {
  key: "agent",
  header: "Agent",
  label: "Agent",
  type: ColumnDataType.Enum,
  width: "78px",
  groupBy: (row) => friendlyAgent(row.agent),
  sortValue: (row) => friendlyAgent(row.agent).toLowerCase(),
  render: (row) => <AgentBadge agent={friendlyAgent(row.agent)} />,
};

type RuleRow = RuleRecord;

export const ruleColumns: ColumnDef<RuleRow>[] = [
  {
    key: "agents",
    header: "Agents",
    label: "Agents",
  type: ColumnDataType.Enum,
    width: "96px",
    groupBy: (row) => row.agents.join(", "),
    sortValue: (row) => row.agents.join(",").toLowerCase(),
    render: (row) => <AgentChips agents={row.agents} />,
  },
  { key: "kind", header: "Kind", label: "Kind", type: ColumnDataType.Enum, width: "96px" },
  { key: "scope", header: "Scope", label: "Scope", type: ColumnDataType.Enum, width: "96px" },
  {
    key: "order",
    header: "Order",
    label: "Order",
    type: ColumnDataType.Text,
    width: "72px",
    sortValue: (row) => row.order,
  },
  {
    key: "source",
    header: "Source",
    label: "Source",
    type: ColumnDataType.Text,
    width: "minmax(120px, 1fr)",
    value: (row) => basename(row.path),
  },
];

type McpRow = McpRecord;

export const mcpColumns: ColumnDef<McpRow>[] = [
  { ...agentColumn, sticky: true, width: "var(--data-freeze-column-width, 96px)" },
  {
    key: "name",
    header: "Name",
    label: "Name",
    type: ColumnDataType.Text,
    width: "150px",
    sortValue: (row) => row.name.toLowerCase(),
  },
  {
    key: "scope",
    header: "Scope",
    label: "Scope",
    type: ColumnDataType.Enum,
    width: "180px",
    sortValue: (row) => row.scope.toLowerCase(),
  },
  {
    key: "transport",
    header: "Transport",
    label: "Transport",
    type: ColumnDataType.Enum,
    width: "120px",
    sortValue: (row) => row.transport.toLowerCase(),
  },
  {
    key: "status",
    header: "Status",
    label: "Status",
    type: ColumnDataType.Enum,
    width: "100px",
    sortValue: (row) => titleValue(row.status).toLowerCase(),
    value: (row) => titleValue(row.status),
  },
  {
    key: "source",
    header: "Source",
    label: "Source",
    type: ColumnDataType.Text,
    width: "minmax(0, 1fr)",
    sortValue: (row) => basename(row.path).toLowerCase(),
    value: (row) => basename(row.path),
  },
];
