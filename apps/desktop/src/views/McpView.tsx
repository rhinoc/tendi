import { useCallback, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { Power, PowerOff, Server } from "lucide-react";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef } from "../components/DataTable.types";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { CopyPathMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { Button } from "../components/shared/Button.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { Switch } from "../components/shared/Switch.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { mcpColumns as defaultMcpColumns } from "../lib/tableColumns.tsx";
import { MCP_FREEZE_COLUMN, TauriCommand, mcpRowKey, mcpSourcePath, safeInvoke, scopeColumn, type McpRecord, type ProjectSummary } from "../lib/index.ts";

export { mcpColumns } from "../lib/tableColumns.tsx";

type McpRow = McpRecord;

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

function mcpEnabled(row: McpRow): boolean {
  return row.enabled;
}

function mcpEnableDisabledReason(row: McpRow | null | undefined): string {
  if (!row) return "Missing MCP server";
  if (row.read_only_reason) return row.read_only_reason;
  const path = mcpSourcePath(row);
  if (!path) return "Missing MCP source path";
  if (!row.trust_hash) return "MCP source hash is unavailable; reload the list";
  return "";
}

function mcpOperationError(result: unknown): string | undefined {
  if (result && typeof result === "object" && !Array.isArray(result) && "error" in result) {
    const error = (result as { error?: unknown }).error;
    return typeof error === "string" ? error : undefined;
  }
  return undefined;
}

function McpEnabledSwitch({
  row,
  updating,
  onToggle,
}: {
  row: McpRow;
  updating?: boolean;
  onToggle: (enabled: boolean) => void;
}) {
  const disabledReason = mcpEnableDisabledReason(row);
  const disabled = Boolean(disabledReason) || Boolean(updating);
  return (
    <Tooltip content={disabledReason || undefined}>
      <Switch
        className={`mcpEnabledSwitch ${updating ? "updating" : ""}`}
        checked={mcpEnabled(row)}
        label={mcpEnabled(row) ? "Disable MCP server" : "Enable MCP server"}
        disabled={disabled}
        aria-busy={updating || undefined}
        onCheckedChange={onToggle}
        data-no-drag
        data-no-row-click
        onClick={(event) => event.stopPropagation()}
      />
    </Tooltip>
  );
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
    <RowActionsMenu ariaLabel={`MCP actions for ${row.name}`}>
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
  onSetMcpEnabled?: (row: McpRow, enabled: boolean) => Promise<unknown>;
  projects?: ProjectSummary[];
};

export function DataListView({ title, rows, columns = defaultMcpColumns, loading = false, loadError = "", hasRows = false, onRetry, onSetMcpEnabled, projects = [] }: DataListViewProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const [updatingKeys, setUpdatingKeys] = useState<Set<string>>(() => new Set());
  const [operationError, setOperationError] = useState("");
  const getRowId = useCallback(
    mcpRowKey,
    [],
  );
  const rowIds = useMemo(() => rows.map(getRowId), [getRowId, rows]);
  const setMcpEnabled = useCallback(async (row: McpRow, enabled: boolean) => {
    if (updatingKeys.size > 0 || mcpEnableDisabledReason(row)) return;
    const key = mcpRowKey(row);
    setUpdatingKeys(new Set([key]));
    setOperationError("");
    try {
      const result = await onSetMcpEnabled?.(row, enabled);
      const error = mcpOperationError(result);
      if (error) setOperationError(error);
      else if (!Array.isArray(result)) setOperationError("Could not update MCP server.");
      else setSelected((current) => current.filter((selectedId) => selectedId !== key));
    } catch (error) {
      setOperationError(`${error}`);
    } finally {
      setUpdatingKeys(new Set());
    }
  }, [onSetMcpEnabled, updatingKeys.size]);
  const tableColumns = useMemo((): ColumnDef<McpRow>[] => {
    const nextColumns = columns.filter((column) => column.key !== "scope");
    const statusIndex = nextColumns.findIndex((column) => column.key === "status");
    nextColumns.splice(statusIndex >= 0 ? statusIndex : nextColumns.length, 0, {
      key: "enabled",
      header: "Enabled",
      label: "Enabled",
      type: "enum",
      width: "92px",
      groupBy: (row) => (mcpEnabled(row) ? "On" : "Off"),
      sortValue: (row) => (mcpEnabled(row) ? 1 : 0),
      render: (row) => (
        <McpEnabledSwitch
          row={row}
          updating={updatingKeys.has(mcpRowKey(row))}
          onToggle={(enabled) => { void setMcpEnabled(row, enabled); }}
        />
      ),
    });
    if (projects.length > 0) {
      const scopeIndex = columns.findIndex((column) => column.key === "scope");
      nextColumns.splice(scopeIndex >= 0 ? scopeIndex : nextColumns.length, 0, scopeColumn<McpRow>(projects, mcpSourcePath));
    }
    return [
      ...nextColumns,
      {
        key: "actions",
        header: "",
        width: "40px",
        render: (row) => <McpActionsCell row={row} />,
      },
    ];
  }, [columns, projects, setMcpEnabled, updatingKeys]);
  const rowContextMenu = useCallback((row: McpRow, { selectedRows, selected: isSelected }: { selectedRows: McpRow[]; selected: boolean }) => {
    if (isSelected && selectedRows.length > 1) return null;
    return <McpActionsMenuItems Menu={ContextMenu} row={row} />;
  }, []);
  const bottomBar = useCallback((selectedRows: McpRow[]) => {
    const firstPath = mcpSourcePath(selectedRows[0]);
    const enableTargets = selectedRows.filter((row) => !mcpEnableDisabledReason(row) && !mcpEnabled(row));
    const disableTargets = selectedRows.filter((row) => !mcpEnableDisabledReason(row) && mcpEnabled(row));
    const updateSelected = async (enabled: boolean) => {
      const targets = enabled ? enableTargets : disableTargets;
      for (const row of targets) await setMcpEnabled(row, enabled);
    };
    return (
      <>
        <Button
          size="sm"
          variant="ghost"
          aria-label="Enable selected MCP servers"
          disabled={enableTargets.length === 0 || updatingKeys.size > 0}
          onClick={() => { void updateSelected(true); }}
        >
          <Power size={15} />
          Enable
        </Button>
        <Button
          size="sm"
          variant="ghost"
          aria-label="Disable selected MCP servers"
          disabled={disableTargets.length === 0 || updatingKeys.size > 0}
          onClick={() => { void updateSelected(false); }}
        >
          <PowerOff size={15} />
          Disable
        </Button>
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
  }, [setMcpEnabled, updatingKeys.size]);

  useEffect(() => {
    setSelected((current) => current.filter((id) => rowIds.includes(id)));
  }, [rowIds]);

  return (
    <section className="content dataPage">
      <ContentTopDragStrip />
      <PageHeader title={title}>{null}</PageHeader>
      {operationError ? <Toast tone="error" message={operationError} onDismiss={() => setOperationError("")} /> : null}
      {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : null}
      <DataTable
        rows={rows}
        columns={tableColumns}
        getRowId={getRowId}
        getRowLabel={(row) => row.name}
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
