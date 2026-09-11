import { useCallback, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { Code2, Copy, FolderOpen, Power, PowerOff, RefreshCw, Server } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";

import { DataTable } from "../components/DataTable.tsx";
import { ColumnDataType, type ColumnDef } from "../components/DataTable.types";
import { useTabState } from "../lib/tab-state.ts";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { CopyPathMenuItem, OpenInEditorMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DataTableSelectionActions, renderDataTableSelectionMenu, type DataTableSelectionActionDefinition } from "../components/shared/DataTableSelectionActions.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { Button } from "../components/shared/Button.tsx";
import { CollapsibleAccordion, type CollapsibleAccordionItem } from "../components/shared/CollapsibleAccordion.tsx";
import { DetailPanel } from "../components/shared/DetailPanel.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { McpServerIcon } from "../components/shared/McpServerIcon.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { Switch } from "../components/shared/Switch.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { mcpColumns as defaultMcpColumns } from "../lib/tableColumns.tsx";
import { actionLabels, EMPTY_DISPLAY_VALUE, formatUserPath, isMcpMutationDelta, MCP_FREEZE_COLUMN, mcpCopy, mcpDisplayName, mcpNeedsLogin, scopeNameForValue, TableSelectionActionId, TauriCommand, mcpRowKey, mcpSelectionActionIds, mcpSourcePath, safeInvoke, type McpRecord } from "../lib/index.ts";
import type { JsonValue } from "../lib/generated/runtime-types.ts";
import type { McpTool } from "../lib/mcp.ts";

import "./McpView.css";

export { mcpColumns } from "../lib/tableColumns.tsx";

type McpRow = McpRecord;

function McpDetailRow({
  label,
  value,
  mono = false,
}: {
  label: string;
  value: ReactNode;
  mono?: boolean;
}) {
  return (
    <div className="mcpDetailRow">
      <span className="mcpDetailLabel">{label}</span>
      <div className={`mcpDetailValue ${mono ? "mono" : ""}`}>
        <span className="mcpDetailText">{value}</span>
      </div>
    </div>
  );
}

function jsonObject(value: JsonValue | undefined): Record<string, JsonValue> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? value as Record<string, JsonValue>
    : undefined;
}

function mcpSchemaType(value: JsonValue): string {
  const schema = jsonObject(value);
  const type = schema?.type;
  if (typeof type === "string") return type;
  if (Array.isArray(type)) {
    const types = type.filter((item): item is string => typeof item === "string");
    if (types.length > 0) return types.join(" | ");
  }
  if (typeof schema?.$ref === "string") return schema.$ref.split("/").pop() || schema.$ref;
  if (Array.isArray(schema?.enum)) return "enum";
  if (jsonObject(schema?.properties)) return "object";
  return "any";
}

type McpToolParameter = {
  name: string;
  type: string;
  required: boolean;
  description?: string;
  defaultValue?: JsonValue;
  enumValues?: JsonValue[];
};

function mcpToolParameters(tool: McpTool): McpToolParameter[] {
  const schema = jsonObject(tool.input_schema);
  const properties = jsonObject(schema?.properties);
  if (!properties) return [];
  const required = new Set(
    Array.isArray(schema?.required)
      ? schema.required.filter((item): item is string => typeof item === "string")
      : [],
  );
  return Object.entries(properties).map(([name, value]) => {
    const property = jsonObject(value);
    return {
      name,
      type: mcpSchemaType(value),
      required: required.has(name),
      description: typeof property?.description === "string" ? property.description : undefined,
      defaultValue: property && "default" in property ? property.default : undefined,
      enumValues: property && Array.isArray(property.enum) ? property.enum : undefined,
    };
  });
}

function mcpJsonText(value: JsonValue): string {
  return JSON.stringify(value);
}

function mcpToolContent(tool: McpTool): ReactNode {
  const description = tool.description?.trim();
  const parameters = mcpToolParameters(tool);
  return (
    <div className="mcpToolContent">
      <div className={description ? "mcpToolDescription" : "mcpToolDescription mcpToolDescriptionEmpty"}>
        {description || "No description available"}
      </div>
      {parameters.length > 0 ? (
        <div className="mcpToolParameters">
          <div className="mcpToolSectionLabel">
            <span>Parameters</span>
            <span className="mcpToolParameterCount">{parameters.length}</span>
          </div>
          <div className="mcpToolParameterList">
            {parameters.map((parameter) => (
              <div className="mcpToolParameter" key={parameter.name}>
                <div className="mcpToolParameterName">
                  <code>{parameter.name}</code>
                  {parameter.required ? <span className="mcpToolRequired">required</span> : null}
                </div>
                <div className="mcpToolParameterDetails">
                  <span className="mcpToolParameterType">{parameter.type}</span>
                  {parameter.description ? <span>{parameter.description}</span> : null}
                  {parameter.enumValues ? <span>Allowed: {parameter.enumValues.map(mcpJsonText).join(", ")}</span> : null}
                  {parameter.defaultValue !== undefined ? <span>Default: {mcpJsonText(parameter.defaultValue)}</span> : null}
                </div>
              </div>
            ))}
          </div>
        </div>
      ) : null}
    </div>
  );
}

function mcpToolItems(row: McpRow): CollapsibleAccordionItem[] {
  return row.tools.map((tool) => {
    const title = tool.title?.trim() || tool.name;
    const description = tool.description?.trim();
    return {
      id: tool.name,
      title: (
        <div className="mcpToolCopy">
          <strong>{title}</strong>
          {tool.title?.trim() ? <code>{tool.name}</code> : null}
        </div>
      ),
      content: mcpToolContent(tool),
    };
  });
}

function mcpToolCount(row: McpRow): string {
  if (row.probe_state !== "ready" && row.probe_state !== "ready-empty") return EMPTY_DISPLAY_VALUE;
  return String(row.tools.length);
}

function mcpToolPlaceholder(row: McpRow): string {
  if (row.probe_state === "needs-auth") return "Authentication required";
  if (row.probe_state === "failed") return row.probe_error || "Tool metadata probe failed";
  return "No tool metadata available";
}

function McpWebsiteLink({ url }: { url: string }) {
  return (
    <a
      className="mcpWebsiteLink"
      href={url}
      onClick={(event) => {
        event.preventDefault();
        event.stopPropagation();
        void safeInvoke(TauriCommand.OpenUrl, { url });
      }}
    >
      {url}
    </a>
  );
}

function McpDetail({
  row,
  onToggle,
  onProbe,
  probing,
  onCollapse,
}: {
  row: McpRow;
  onToggle: (enabled: boolean) => void;
  onProbe: () => void;
  probing: boolean;
  onCollapse: () => void;
}) {
  const title = mcpDisplayName(row);
  const path = formatUserPath(row.path);
  const description = row.server_description?.trim() || EMPTY_DISPLAY_VALUE;
  const probeSupported = ["stdio", "http", "sse", "cursor-plugin"].includes(row.transport);

  return (
    <DetailPanel
      title={(
        <span className="mcpDetailTitle">
          <McpServerIcon icons={row.icons} size={22} />
          <span className="mcpDetailTitleText">{title}</span>
          {mcpNeedsLogin(row.status) ? <Badge tone="warning">Need login</Badge> : null}
        </span>
      )}
      meta={(
        <div className="threadMeta mcpDetailMeta">
          <Tooltip content={path} onlyWhenTruncated>
            <span>{path || EMPTY_DISPLAY_VALUE}</span>
          </Tooltip>
        </div>
      )}
      headerActions={probeSupported ? (
        <Button
          className="mcpProbeButton"
          variant="ghost"
          size="sm"
          aria-label={probing ? "Checking MCP connection" : "Check MCP connection"}
          aria-busy={probing || undefined}
          disabled={probing}
          onClick={onProbe}
          data-no-drag
          data-no-row-click
        >
          {probing ? <LoadingIcon size={15} /> : <RefreshCw size={15} />}
          {probing ? null : <span>Check connection</span>}
        </Button>
      ) : undefined}
      collapseLabel="Collapse MCP detail"
      onCollapse={onCollapse}
    >
      <div className="mcpDetailBody">
        <section className="mcpDetailSection">
          <h3>Server</h3>
          <p className="mcpDescription">{description}</p>
          <div className="mcpDetailTable">
            {row.server_name && row.server_name !== title ? <McpDetailRow label="Protocol name" value={row.server_name} /> : null}
            {row.server_version ? <McpDetailRow label="Version" value={row.server_version} /> : null}
            <McpDetailRow label="Enabled" value={<McpEnabledSwitch row={row} updating={probing} onToggle={onToggle} />} />
            <McpDetailRow label="Transport" value={row.transport} />
            <McpDetailRow label="Scope" value={scopeNameForValue(row.scope)} />
            {row.probe_error ? <McpDetailRow label="Probe error" value={row.probe_error} /> : null}
            {row.server_website_url ? (
              <McpDetailRow label="Website" value={<McpWebsiteLink url={row.server_website_url} />} />
            ) : null}
          </div>
        </section>
        <section className="mcpDetailSection">
          <div className="mcpSectionHeaderRow">
            <h3>Tools</h3>
            <span className="mcpToolCount">{mcpToolCount(row)}</span>
          </div>
          {row.tools.length > 0 ? (
            <CollapsibleAccordion className="mcpToolList" items={mcpToolItems(row)} />
          ) : (
            <div className="mcpToolPlaceholder">
              {mcpToolPlaceholder(row)}
            </div>
          )}
        </section>
      </div>
    </DetailPanel>
  );
}

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
    <RowActionsMenu ariaLabel={`MCP actions for ${mcpDisplayName(row)}`}>
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
      menu: <Menu.Item className="menuItem" disabled={enableTargets.length === 0 || updating} onSelect={() => { void updateSelected(true); }}><Power size={14} />{actionLabels.enable}</Menu.Item>,
      measure: <><Power size={15} /><span>{actionLabels.enable}</span></>,
    },
    [TableSelectionActionId.Disable]: {
      id: TableSelectionActionId.Disable,
      direct: <Button size="sm" variant="ghost" aria-label="Disable selected MCP servers" disabled={disableTargets.length === 0 || updating} onClick={() => { void updateSelected(false); }}><PowerOff size={15} /><span>{actionLabels.disable}</span></Button>,
      menu: <Menu.Item className="menuItem" disabled={disableTargets.length === 0 || updating} onSelect={() => { void updateSelected(false); }}><PowerOff size={14} />{actionLabels.disable}</Menu.Item>,
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
  onProbeMcp?: (row: McpRow) => Promise<unknown>;
  locateMcpId?: string;
  onLocateMcpComplete?: (id: string) => void;
};

export function DataListView({ title, rows, columns = defaultMcpColumns, loading = false, loadError = "", hasRows = false, onRetry, onSetMcpEnabled, onSetMcpEnabledMany, onProbeMcp, locateMcpId, onLocateMcpComplete }: DataListViewProps) {
  const [activeKey, setActiveKey] = useTabState("mcp.activeKey", rows[0] ? mcpRowKey(rows[0]) : "");
  const [selected, setSelected] = useState<string[]>([]);
  const [updatingKeys, setUpdatingKeys] = useState<Set<string>>(() => new Set());
  const [probingKey, setProbingKey] = useState("");
  const [operationError, setOperationError] = useState("");
  const [mcpLocatorRequest, setMcpLocatorRequest] = useState("");
  const [detailCollapsed, setDetailCollapsed] = useTabState("mcp.detailCollapsed", false);
  const getRowId = useCallback(
    mcpRowKey,
    [],
  );
  const rowIds = useMemo(() => rows.map(getRowId), [getRowId, rows]);
  const activeRow = useMemo(
    () => rows.find((row) => mcpRowKey(row) === activeKey) ?? null,
    [activeKey, rows],
  );
  const operationBusy = updatingKeys.size > 0 || Boolean(probingKey);
  useEffect(() => {
    if (!locateMcpId) return;
    setMcpLocatorRequest(locateMcpId);
    const locatedRow = rows.find((row) => mcpRowKey(row) === locateMcpId);
    if (locatedRow) {
      setActiveKey(mcpRowKey(locatedRow));
      setDetailCollapsed(false);
    }
  }, [locateMcpId, rows]);
  const completeMcpLocator = useCallback((id: string) => {
    setMcpLocatorRequest((current) => current === id ? "" : current);
    onLocateMcpComplete?.(id);
  }, [onLocateMcpComplete]);
  const setMcpEnabled = useCallback(async (row: McpRow, enabled: boolean) => {
    if (operationBusy || mcpEnableDisabledReason(row)) return;
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
  }, [onSetMcpEnabled, operationBusy]);
  const setSelectedMcpEnabled = useCallback(async (targets: McpRow[], enabled: boolean) => {
    if (operationBusy || targets.length === 0) return;
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
  }, [onSetMcpEnabled, onSetMcpEnabledMany, operationBusy]);
  const probeMcp = useCallback(async (row: McpRow) => {
    if (operationBusy || !onProbeMcp || !["stdio", "http", "sse", "cursor-plugin"].includes(row.transport)) return;
    const key = mcpRowKey(row);
    setProbingKey(key);
    setOperationError("");
    try {
      const result = await onProbeMcp(row);
      const error = mcpOperationError(result);
      if (error) setOperationError(error);
      else if (!isMcpMutationDelta(result)) setOperationError("Could not check MCP connection.");
    } catch (error) {
      setOperationError(`${error}`);
    } finally {
      setProbingKey("");
    }
  }, [onProbeMcp, operationBusy]);
  const tableColumns = useMemo((): ColumnDef<McpRow>[] => {
    const nextColumns = columns.filter((column) => column.key !== "scope" && column.key !== "status");
    nextColumns.push({
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
          updating={updatingKeys.has(mcpRowKey(row)) || probingKey === mcpRowKey(row)}
          onToggle={(enabled) => { void setMcpEnabled(row, enabled); }}
        />
      ),
    });
    const scopeColumn = columns.find((column) => column.key === "scope");
    if (scopeColumn) {
      const scopeIndex = columns.findIndex((column) => column.key === "scope");
      nextColumns.splice(scopeIndex >= 0 ? scopeIndex : nextColumns.length, 0, scopeColumn);
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
              operationBusy,
            ))}
          />
        ),
      },
    ];
  }, [columns, operationBusy, probingKey, setMcpEnabled, setSelectedMcpEnabled, updatingKeys]);
  const rowContextMenu = useCallback((row: McpRow, { selectedRows, selected: isSelected }: { selectedRows: McpRow[]; selected: boolean }) => {
    const actionRows = isSelected ? selectedRows : [row];
    const actions = mcpSelectionActions(actionRows, ContextMenu, setSelectedMcpEnabled, operationBusy);
    return actions.length > 0 ? renderDataTableSelectionMenu(actions) : null;
  }, [operationBusy, setSelectedMcpEnabled]);
  const bottomBar = useCallback((selectedRows: McpRow[]) => (
    <DataTableSelectionActions
      actions={mcpSelectionActions(selectedRows, DropdownMenu, setSelectedMcpEnabled, operationBusy)}
      ariaLabel="More selected MCP actions"
    />
  ), [operationBusy, setSelectedMcpEnabled]);

  useEffect(() => {
    setSelected((current) => current.filter((id) => rowIds.includes(id)));
  }, [rowIds]);

  useEffect(() => {
    if (activeKey && rows.some((row) => mcpRowKey(row) === activeKey)) return;
    setActiveKey(rows[0] ? mcpRowKey(rows[0]) : "");
  }, [activeKey, rows]);

  return (
    <>
      <ContentTopDragStrip />
      <PanelGroup className="sessionsLayout mcpLayout" orientation="horizontal">
        <Panel className="sessionListPanel mcpListPanel" defaultSize="54%" minSize="360px">
          <div className="sessionListPane mcpListPane">
            <PageHeader title={title} compact>{null}</PageHeader>
            {operationError ? <Toast tone="error" message={operationError} onDismiss={() => setOperationError("")} /> : null}
            {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : null}
            <div className="sessionListBody">
              <DataTable
                rows={rows}
                columns={tableColumns}
                getRowId={getRowId}
                getRowLabel={mcpDisplayName}
                freezeColumn={MCP_FREEZE_COLUMN}
                selectable
                selectedIds={selected}
                onSelectionChange={setSelected}
                enableMarquee
                scrollRestorationKey="mcp.list"
                onRowClick={(row) => {
                  setActiveKey(mcpRowKey(row));
                  setDetailCollapsed(false);
                }}
                scrollToRowId={mcpLocatorRequest}
                onScrollToRowComplete={completeMcpLocator}
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
            </div>
          </div>
        </Panel>
        <DetailPanelHost
          collapsed={detailCollapsed}
          onExpand={() => setDetailCollapsed(false)}
          expandLabel="Expand MCP detail"
          railLabel={activeRow ? mcpDisplayName(activeRow) : "MCP detail"}
          hasSelection={Boolean(activeRow)}
          emptyState={loading ? <LoadingState label="Loading MCP servers" /> : <EmptyState compact title="Select an MCP server to view its details." />}
        >
          {activeRow ? (
            <McpDetail
              row={activeRow}
              onToggle={(enabled) => { void setMcpEnabled(activeRow, enabled); }}
              onProbe={() => { void probeMcp(activeRow); }}
              probing={probingKey === mcpRowKey(activeRow)}
              onCollapse={() => setDetailCollapsed(true)}
            />
          ) : null}
        </DetailPanelHost>
      </PanelGroup>
    </>
  );
}
