import { useCallback, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { Code2, Copy, FolderOpen, Power, PowerOff, Server } from "lucide-react";

import { DataTable } from "../components/DataTable.tsx";
import { ColumnDataType, type ColumnDef } from "../components/DataTable.types";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { CopyPathMenuItem, OpenInEditorMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DataTableSelectionActions, renderDataTableSelectionMenu, type DataTableSelectionActionDefinition } from "../components/shared/DataTableSelectionActions.tsx";
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
import { actionLabels, isMcpMutationDelta, MCP_FREEZE_COLUMN, mcpCopy, TableSelectionActionId, TauriCommand, mcpRowKey, mcpSelectionActionIds, mcpSourcePath, safeInvoke, scopeColumn, type McpRecord, type ProjectSummary } from "../lib/index.ts";

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
  if (result && typeof result === "object" && "error" in result) {
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

function McpActionsCell({ row, actions }: { row: McpRow; actions: ReactNode }) {
  return (
    <RowActionsMenu ariaLabel={`MCP actions for ${row.name}`}>
      {actions}
    </RowActionsMenu>
  );
}

function mcpSelectionActions(
  selectedRows: McpRow[],
  Menu: McpMenuComponents,
  setSelectedMcpEnabled: (rows: McpRow[], enabled: boolean) => Promise<void>,
  updating: boolean,
): DataTableSelectionActionDefinition[] {
  const enableTargets = selectedRows.filter((row) => !mcpEnableDisabledReason(row) && !mcpEnabled(row));
  const disableTargets = selectedRows.filter((row) => !mcpEnableDisabledReason(row) && mcpEnabled(row));
  const updateSelected = async (enabled: boolean) => {
    const targets = enabled ? enableTargets : disableTargets;
    await setSelectedMcpEnabled(targets, enabled);
  };
  const actions: Record<string, DataTableSelectionActionDefinition> = {
    [TableSelectionActionId.Enable]: {
      id: TableSelectionActionId.Enable,
      direct: <Button size="sm" variant="ghost" aria-label="Enable selected MCP servers" disabled={enableTargets.length === 0 || updating} onClick={() => { void updateSelected(true); }}><Power size={15} /><span>{actionLabels.enable}</span></Button>,
      menu: <Menu.Item className="skillMenuItem" disabled={enableTargets.length === 0 || updating} onSelect={() => { void updateSelected(true); }}><Power size={14} />{actionLabels.enable}</Menu.Item>,
      measure: <><Power size={15} /><span>{actionLabels.enable}</span></>,
    },
    [TableSelectionActionId.Disable]: {
      id: TableSelectionActionId.Disable,
      direct: <Button size="sm" variant="ghost" aria-label="Disable selected MCP servers" disabled={disableTargets.length === 0 || updating} onClick={() => { void updateSelected(false); }}><PowerOff size={15} /><span>{actionLabels.disable}</span></Button>,
      menu: <Menu.Item className="skillMenuItem" disabled={disableTargets.length === 0 || updating} onSelect={() => { void updateSelected(false); }}><PowerOff size={14} />{actionLabels.disable}</Menu.Item>,
      measure: <><PowerOff size={15} /><span>{actionLabels.disable}</span></>,
    },
    [TableSelectionActionId.OpenEditor]: {
      id: TableSelectionActionId.OpenEditor,
      direct: <button aria-label={actionLabels.openInEditor} disabled={selectedRows.length !== 1 || !mcpSourcePath(selectedRows[0])} onClick={() => { const path = mcpSourcePath(selectedRows[0]); if (path) void safeInvoke(TauriCommand.OpenInEditor, { path }); }}><Code2 size={15} /><span>{actionLabels.openInEditor}</span></button>,
      menu: <OpenInEditorMenuItem Menu={Menu} path={mcpSourcePath(selectedRows[0])} />,
      measure: <><Code2 size={15} /><span>{actionLabels.openInEditor}</span></>,
    },
    [TableSelectionActionId.Reveal]: {
      id: TableSelectionActionId.Reveal,
      direct: <button aria-label={actionLabels.revealInFinder} disabled={selectedRows.length !== 1 || !mcpSourcePath(selectedRows[0])} onClick={() => { const path = mcpSourcePath(selectedRows[0]); if (path) void safeInvoke(TauriCommand.RevealInFinder, { path }); }}><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></button>,
      menu: <RevealInFinderMenuItem Menu={Menu} path={mcpSourcePath(selectedRows[0])} />,
      measure: <><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></>,
    },
    [TableSelectionActionId.CopyPath]: {
      id: TableSelectionActionId.CopyPath,
      direct: <CopyButton value={mcpSourcePath(selectedRows[0])} disabled={!mcpSourcePath(selectedRows[0])} copyLabel={actionLabels.copyPath} copiedLabel={actionLabels.pathCopied} iconSize={15}>{actionLabels.copyPath}</CopyButton>,
      menu: <CopyPathMenuItem Menu={Menu} path={mcpSourcePath(selectedRows[0])} />,
      measure: <><Copy size={15} /><span>{actionLabels.copyPath}</span></>,
    },
  };
  return mcpSelectionActionIds(selectedRows.length).map((id) => actions[id]);
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
  onSetMcpEnabledMany?: (rows: McpRow[], enabled: boolean) => Promise<unknown>;
  projects?: ProjectSummary[];
};

export function DataListView({ title, rows, columns = defaultMcpColumns, loading = false, loadError = "", hasRows = false, onRetry, onSetMcpEnabled, onSetMcpEnabledMany, projects = [] }: DataListViewProps) {
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
      else if (!isMcpMutationDelta(result)) setOperationError("Could not update MCP server.");
      else setSelected((current) => current.filter((selectedId) => selectedId !== key));
    } catch (error) {
      setOperationError(`${error}`);
    } finally {
      setUpdatingKeys(new Set());
    }
  }, [onSetMcpEnabled, updatingKeys.size]);
  const setSelectedMcpEnabled = useCallback(async (targets: McpRow[], enabled: boolean) => {
    if (updatingKeys.size > 0 || targets.length === 0) return;
    setUpdatingKeys(new Set(targets.map((row) => mcpRowKey(row))));
    setOperationError("");
    try {
      if (targets.length > 1 && onSetMcpEnabledMany) {
        const result = await onSetMcpEnabledMany(targets, enabled);
        const error = mcpOperationError(result);
        if (error) setOperationError(error);
        else if (!isMcpMutationDelta(result)) setOperationError("Could not update MCP servers.");
        else setSelected([]);
        return;
      }
      for (const row of targets) {
        const result = await onSetMcpEnabled?.(row, enabled);
        const error = mcpOperationError(result);
        if (error) {
          setOperationError(error);
          return;
        }
        if (!isMcpMutationDelta(result)) {
          setOperationError("Could not update MCP servers.");
          return;
        }
      }
      setSelected([]);
    } catch (error) {
      setOperationError(`${error}`);
    } finally {
      setUpdatingKeys(new Set());
    }
  }, [onSetMcpEnabled, onSetMcpEnabledMany, updatingKeys.size]);
  const tableColumns = useMemo((): ColumnDef<McpRow>[] => {
    const nextColumns = columns.filter((column) => column.key !== "scope");
    const statusIndex = nextColumns.findIndex((column) => column.key === "status");
    nextColumns.splice(statusIndex >= 0 ? statusIndex : nextColumns.length, 0, {
      key: "enabled",
      header: "Enabled",
      label: "Enabled",
      type: ColumnDataType.Enum,
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
        render: (row) => (
          <McpActionsCell
            row={row}
            actions={renderDataTableSelectionMenu(mcpSelectionActions(
              [row],
              DropdownMenu,
              setSelectedMcpEnabled,
              updatingKeys.size > 0,
            ))}
          />
        ),
      },
    ];
  }, [columns, projects, setSelectedMcpEnabled, updatingKeys]);
  const rowContextMenu = useCallback((row: McpRow, { selectedRows, selected: isSelected }: { selectedRows: McpRow[]; selected: boolean }) => {
    const actionRows = isSelected ? selectedRows : [row];
    const actions = mcpSelectionActions(actionRows, ContextMenu, setSelectedMcpEnabled, updatingKeys.size > 0);
    return actions.length > 0 ? renderDataTableSelectionMenu(actions) : null;
  }, [setSelectedMcpEnabled, updatingKeys.size]);
  const bottomBar = useCallback((selectedRows: McpRow[]) => (
    <DataTableSelectionActions
      actions={mcpSelectionActions(selectedRows, DropdownMenu, setSelectedMcpEnabled, updatingKeys.size > 0)}
      ariaLabel="More selected MCP actions"
    />
  ), [setSelectedMcpEnabled, updatingKeys.size]);

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
        bottomBarActionsClassName="selectionActions"
        bottomBarCheckboxLabel={mcpCopy.selectVisibleLabel}
        selectionLabel={mcpCopy.selectionLabel}
        loading={loading && !hasRows}
        loadingLabel={mcpCopy.loadingLabel}
        emptyState={loadError && !hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : (
          <EmptyState
            icon={<Server size={27} strokeWidth={1.55} />}
            title={mcpCopy.emptyTitle}
            description={mcpCopy.emptyDescription}
          />
        )}
      />
    </section>
  );
}
