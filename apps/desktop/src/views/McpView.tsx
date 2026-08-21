import { useCallback, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { Server } from "lucide-react";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef } from "../components/DataTable.types";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { CopyPathMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { mcpColumns as defaultMcpColumns } from "../lib/tableColumns.tsx";
import { MCP_FREEZE_COLUMN, TauriCommand, safeInvoke } from "../lib/index.ts";

export { mcpColumns } from "../lib/tableColumns.tsx";

type McpRow = {
  id?: string;
  agent?: string | null;
  name?: string | null;
  scope?: string | null;
  transport?: string | null;
  status?: string | null;
  path?: string | null;
  source?: string | null;
};

type McpMenuComponents = {
  Item: ComponentType<{
    className?: string;
    disabled?: boolean;
    key?: string;
    onSelect?: () => void;
    children?: ReactNode;
  }>;
  Separator: ComponentType<{ className?: string }>;
};

function mcpSourcePath(row: McpRow): string {
  return `${row.path ?? row.source ?? ""}`.trim();
}

function McpActionsMenuItems({
  Menu,
  row,
}: {
  Menu: McpMenuComponents;
  row: McpRow;
}) {
  const path = mcpSourcePath(row);
  return (
    <>
      <RevealInFinderMenuItem Menu={Menu} path={path} />
      <CopyPathMenuItem Menu={Menu} path={path} />
    </>
  );
}

function McpActionsCell({ row }: { row: McpRow }) {
  return (
    <RowActionsMenu ariaLabel={`MCP actions for ${row.name ?? "server"}`}>
      <McpActionsMenuItems Menu={DropdownMenu} row={row} />
    </RowActionsMenu>
  );
}

type DataListViewProps = {
  title: string;
  rows: McpRow[];
  columns?: ColumnDef<McpRow>[];
  loading?: boolean;
  loadError?: string;
  hasRows?: boolean;
  onRetry?: () => void;
};

export function DataListView({ title, rows, columns = defaultMcpColumns, loading = false, loadError = "", hasRows = false, onRetry }: DataListViewProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const getRowId = useCallback(
    (row: McpRow) => row.id ?? JSON.stringify([row.agent ?? "", row.name ?? "", mcpSourcePath(row)]),
    [],
  );
  const rowIds = useMemo(() => rows.map(getRowId), [getRowId, rows]);
  const tableColumns = useMemo((): ColumnDef<McpRow>[] => [
    ...columns,
    {
      key: "actions",
      header: "",
      width: "40px",
      render: (row) => <McpActionsCell row={row} />,
    },
  ], [columns]);
  const rowContextMenu = useCallback((row: McpRow, { selectedRows, selected: isSelected }: { selectedRows: McpRow[]; selected: boolean }) => {
    if (isSelected && selectedRows.length > 1) return null;
    return <McpActionsMenuItems Menu={ContextMenu} row={row} />;
  }, []);
  const bottomBar = useCallback((selectedRows: McpRow[]) => {
    const firstPath = mcpSourcePath(selectedRows[0]);
    return (
      <>
        <button
          aria-label="Reveal selected MCP config in Finder"
          disabled={!firstPath}
          onClick={() => firstPath && safeInvoke(TauriCommand.RevealInFinder, { path: firstPath })}
        >
          Reveal in Finder
        </button>
        <CopyButton
          value={firstPath}
          copyLabel="Copy path"
          copiedLabel="Path copied"
          disabled={!firstPath}
          iconSize={15}
        >
          Copy path
        </CopyButton>
      </>
    );
  }, []);

  useEffect(() => {
    setSelected((current) => current.filter((id) => rowIds.includes(id)));
  }, [rowIds]);

  return (
    <section className="content dataPage">
      <ContentTopDragStrip />
      <PageHeader title={title}>{null}</PageHeader>
      {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : null}
      <DataTable
        rows={rows}
        columns={tableColumns}
        getRowId={getRowId}
        getRowLabel={(row) => row.name ?? row.id ?? "MCP server"}
        freezeColumn={MCP_FREEZE_COLUMN}
        selectable
        selectedIds={selected}
        onSelectionChange={setSelected}
        enableMarquee
        rowContextMenu={rowContextMenu}
        bottomBar={bottomBar}
        bottomBarCheckboxLabel={`Select visible ${title.toLowerCase()}`}
        selectionLabel="servers"
        loading={loading && !hasRows}
        loadingLabel={`Loading ${title.toLowerCase()}`}
        emptyState={loadError && !hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : (
          <EmptyState
            icon={<Server size={27} strokeWidth={1.55} />}
            title="No MCP servers found"
            description="Adjust the agent filter to see more."
          />
        )}
      />
    </section>
  );
}
