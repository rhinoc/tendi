import { ColumnDataType, type ColumnDef } from "../components/DataTable.types";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { AgentChips } from "../components/shared/AgentChips.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { McpServerIcon } from "../components/shared/McpServerIcon.tsx";
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { basename, EMPTY_DISPLAY_VALUE, friendlyAgent, mcpDisplayName, mcpNeedsLogin, scopeNameForValue } from "./index.ts";
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
  {
    key: "name",
    header: "MCP server",
    label: "MCP server",
    type: ColumnDataType.Text,
    width: "var(--data-freeze-column-width, 220px)",
    sticky: true,
    sortValue: (row) => row.name.toLowerCase(),
    title: (row) => row.server_description?.trim() || undefined,
    render: (row) => {
      const title = mcpDisplayName(row);
      return (
        <Tooltip content={row.server_description?.trim() || undefined} onlyWhenTruncated>
          <span className="mcpNameCell">
            <McpServerIcon icons={row.icons} />
            <span className="mcpNameCopy">
              <span className="dataCellTitleLine">
                <span className="dataCellTitle">{title}</span>
                {mcpNeedsLogin(row.status) ? <Badge tone="warning">Need login</Badge> : null}
              </span>
              <span className="dataCellSubLine">
                <span className="dataCellSub">{row.server_description?.trim() || EMPTY_DISPLAY_VALUE}</span>
              </span>
            </span>
          </span>
        </Tooltip>
      );
    },
  },
  { ...agentColumn },
  {
    key: "tools",
    header: "Tools",
    label: "Tools",
    type: ColumnDataType.Text,
    width: "72px",
    sortValue: (row) => row.tools.length,
    value: (row) => row.probe_state === "ready" || row.probe_state === "ready-empty" ? row.tools.length : EMPTY_DISPLAY_VALUE,
    title: (row) => row.probe_error || undefined,
  },
  {
    key: "scope",
    header: "Scope",
    label: "Scope",
    type: ColumnDataType.Enum,
    width: "180px",
    sortValue: (row) => scopeNameForValue(row.scope).toLowerCase(),
    groupBy: (row) => scopeNameForValue(row.scope),
    value: (row) => scopeNameForValue(row.scope),
  },
  {
    key: "transport",
    header: "Transport",
    label: "Transport",
    type: ColumnDataType.Enum,
    width: "120px",
    sortValue: (row) => row.transport.toLowerCase(),
  },
];
