import { Tooltip } from "../components/shared/Tooltip.tsx";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ComponentType, type ReactNode } from "react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import { Code2, Copy, Crosshair, Delete as DeleteKeyIcon, FolderOpen, Power, PowerOff, SearchX, Trash2, Webhook } from "lucide-react";

import { DataTable } from "../components/DataTable.tsx";
import { ColumnDataType, type ColumnDef, type SortState } from "../components/DataTable.types";
import { SortDirection } from "../lib/sort.ts";
import { useTabState } from "../lib/tab-state.ts";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { Button } from "../components/shared/Button.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { DataTableSelectionActions, renderDataTableSelectionMenu, type DataTableSelectionActionDefinition } from "../components/shared/DataTableSelectionActions.tsx";
import { MenuShortcut, OpenInEditorMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DetailPanel } from "../components/shared/DetailPanel.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { DialogActionButton } from "../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { DialogStatefulButton } from "../components/shared/DialogStatefulButton.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { Switch } from "../components/shared/Switch.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { AsyncStatus } from "../lib/async-status.ts";
import { useHookOperations } from "../features/hooks/use-hook-operations.ts";
import { useHookSource } from "../features/hooks/use-hook-source.ts";
import "./HooksView.css";

import {
  actionLabels,
  copiedValueLabel,
  copyValueLabel,
  EMPTY_DISPLAY_VALUE,
  HOOK_FREEZE_COLUMN,
  selectionDeleteLabel,
  TableSelectionActionId,
  TauriCommand,
  compactCommand,
  copyText,
  formatUserPath,
  filterHooks,
  friendlyAgent,
  hookDisplayName,
  hookHandlerText,
  hookItemsFromRows,
  hookSelectionActionIds,
  hookSourcePath,
  hookTypeLabel,
  isWebSource,
  safeInvoke,
  scopeColumn,
  selectionDeleteLoadingLabel,
  suppressNextClick,
  type ProjectSummary,
} from "../lib/index.ts";
import { hookEnableDisabledReason, hookReviewDisabledReason, hookSelectionTargets } from "../lib/hooks.ts";
import type { HookItem, HookRecord } from "../lib/hooks.ts";
import type { CatalogMutationResponse } from "../lib/runtime-gateway.ts";

const HookSourcePreview = lazy(() => import("../features/hooks/HookSourcePreview.tsx").then(({ HookSourcePreview: component }) => ({ default: component })));

function hookSelectionIdentity(hook: HookRecord | null | undefined) {
  if (!hook) return "";
  return [hook.agent, hook.path, hook.event, hook.matcher, hook.hook_type]
    .map((value) => `${value ?? ""}`).join("|");
}

type HookMenuComponents = {
  Item: ComponentType<{
    className?: string;
    disabled?: boolean;
    key?: string;
    onSelect?: () => void;
    children?: ReactNode;
  }>;
  Separator: ComponentType<{ className?: string }>;
};

type HookParameterRow =
  | { label: string; value: string; mono?: boolean; copyable?: boolean }
  | { label: string; render: ReactNode };

type HookDetailRowProps = {
  label: string;
  value?: string | null;
  mono?: boolean;
  copyable?: boolean;
};

type HookEnabledSwitchProps = {
  checked: boolean;
  disabledReason?: string;
  updating?: boolean;
  onToggle?: (enabled: boolean) => void;
};

type HooksViewProps = {
  rows: HookRecord[];
  loadingRows?: boolean;
  loadError?: string;
  hasRows?: boolean;
  onRetry?: () => void;
  onDeleteHook?: (hook: HookRecord) => Promise<CatalogMutationResponse>;
  onDeleteHooks?: (hooks: HookRecord[]) => Promise<CatalogMutationResponse>;
  onSetHookEnabled?: (hook: HookRecord, enabled: boolean) => Promise<CatalogMutationResponse>;
  onSetHooksEnabled?: (hooks: HookRecord[], enabled: boolean) => Promise<CatalogMutationResponse>;
  onReviewHook?: (hook: HookRecord) => Promise<CatalogMutationResponse>;
  locateHookId?: string;
  onLocateHookComplete?: (id: string) => void;
  projects?: ProjectSummary[];
};

const defaultSort: SortState = { key: "event", direction: SortDirection.Asc };

export function HookDetailRow({ label, value, mono = false, copyable = false }: HookDetailRowProps) {
  const text = `${value ?? ""}`;
  return (
    <div className="hookDetailRow">
      <span className="hookDetailLabel">{label}</span>
      <div className={`hookDetailValue ${mono ? "mono" : ""}`}>
        {isWebSource(text.trim()) ? (
          <span>{text || EMPTY_DISPLAY_VALUE}</span>
        ) : (
          <Tooltip content={text || undefined} onlyWhenTruncated>
            <span>{text || EMPTY_DISPLAY_VALUE}</span>
          </Tooltip>
        )}
        {copyable && text ? (
          <CopyButton className="hookCopyButton" value={text} copyLabel={copyValueLabel(label)} copiedLabel={copiedValueLabel(label)} />
        ) : null}
      </div>
    </div>
  );
}

function hookEnableDisabledReasonForView(hook: HookRecord | null, updating: boolean): string {
  return hookEnableDisabledReason(hook) || (updating ? "Updating hooks" : "");
}

function HookEnabledSwitch({ checked, disabledReason = "", updating = false, onToggle }: HookEnabledSwitchProps) {
  // Use aria-disabled (not native disabled) so clicks still hit this control and do not fall through to row onClick.
  const disabled = Boolean(disabledReason) || updating;
  return (
    <Tooltip content={disabledReason || undefined}><Switch
      className={`hookEnabledSwitch ${checked ? "on" : "off"} ${updating ? "updating" : ""}`}
      checked={checked}
      label={checked ? "Disable hook" : "Enable hook"}
      disabled={disabled}
      onCheckedChange={(nextChecked) => onToggle?.(nextChecked)}
      data-no-drag
      data-no-row-click
      onClick={(event) => {
        event.stopPropagation();
      }}
      onKeyDown={(event) => {
        if (!disabled) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          event.stopPropagation();
        }
      }}
    /></Tooltip>
  );
}

function HookActionsCell({
  item,
  actions,
}: {
  item: HookItem;
  actions: ReactNode;
}) {
  const hook = item.hook;
  return (
    <RowActionsMenu
      ariaLabel={`Hook actions for ${hookDisplayName(hook)}`}
      onOpenChange={(open) => { if (!open) suppressNextClick(); }}
    >
      {actions}
    </RowActionsMenu>
  );
}

function hookSelectionActions(
  selectedRows: HookItem[],
  Menu: HookMenuComponents,
  setSelectedHooksEnabled: (items: HookItem[], enabled: boolean) => void | Promise<void>,
  requestDeleteHooks: (items: HookItem[]) => void,
  busy: boolean,
  deleting: boolean,
): DataTableSelectionActionDefinition[] {
  const singleItem = selectedRows.length === 1 ? selectedRows[0] : undefined;
  const path = singleItem ? hookSourcePath(singleItem.hook) : "";
  const { deletable, enable: enableTargets, disable: disableTargets } = hookSelectionTargets(selectedRows);
  const deleteLabel = selectionDeleteLabel("hook", selectedRows.length);
  const actions: Record<string, DataTableSelectionActionDefinition> = {
    [TableSelectionActionId.OpenEditor]: {
      id: TableSelectionActionId.OpenEditor,
      direct: <button aria-label={actionLabels.openInEditor} disabled={!path} onClick={() => path && void safeInvoke(TauriCommand.OpenInEditor, { path })}><Code2 size={15} /><span>{actionLabels.openInEditor}</span></button>,
      menu: <OpenInEditorMenuItem Menu={Menu} path={path} />,
      measure: <><Code2 size={15} /><span>{actionLabels.openInEditor}</span></>,
    },
    [TableSelectionActionId.Reveal]: {
      id: TableSelectionActionId.Reveal,
      direct: <button aria-label={actionLabels.revealInFinder} disabled={!path} onClick={() => path && void safeInvoke(TauriCommand.RevealInFinder, { path })}><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></button>,
      menu: <Menu.Item className="menuItem" disabled={!path} onSelect={() => path && void safeInvoke(TauriCommand.RevealInFinder, { path })}><FolderOpen size={14} />{actionLabels.revealInFinder}</Menu.Item>,
      measure: <><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></>,
    },
    [TableSelectionActionId.CopyPath]: {
      id: TableSelectionActionId.CopyPath,
      direct: <CopyButton value={path} disabled={!path} copyLabel={actionLabels.copyPath} copiedLabel={actionLabels.pathCopied} iconSize={15}>{actionLabels.copyPath}</CopyButton>,
      menu: <Menu.Item className="menuItem" disabled={!path} onSelect={() => path && copyText(path)}><Copy size={14} />{actionLabels.copyPath}</Menu.Item>,
      measure: <><Copy size={15} /><span>{actionLabels.copyPath}</span></>,
    },
    [TableSelectionActionId.Enable]: {
      id: TableSelectionActionId.Enable,
      direct: <Button size="sm" variant="ghost" aria-label="Enable selected hooks" disabled={enableTargets.length === 0 || busy} onClick={() => { void setSelectedHooksEnabled(enableTargets, true); }}><Power size={15} /><span>{actionLabels.enable}</span></Button>,
      menu: <Menu.Item className="menuItem" disabled={enableTargets.length === 0 || busy} onSelect={() => { void setSelectedHooksEnabled(enableTargets, true); }}><Power size={14} />{actionLabels.enable}</Menu.Item>,
      measure: <><Power size={15} /><span>{actionLabels.enable}</span></>,
    },
    [TableSelectionActionId.Disable]: {
      id: TableSelectionActionId.Disable,
      direct: <Button size="sm" variant="ghost" aria-label="Disable selected hooks" disabled={disableTargets.length === 0 || busy} onClick={() => { void setSelectedHooksEnabled(disableTargets, false); }}><PowerOff size={15} /><span>{actionLabels.disable}</span></Button>,
      menu: <Menu.Item className="menuItem" disabled={disableTargets.length === 0 || busy} onSelect={() => { void setSelectedHooksEnabled(disableTargets, false); }}><PowerOff size={14} />{actionLabels.disable}</Menu.Item>,
      measure: <><PowerOff size={15} /><span>{actionLabels.disable}</span></>,
    },
    [TableSelectionActionId.Delete]: {
      id: TableSelectionActionId.Delete,
      direct: <Button size="sm" variant="ghost" className="danger" aria-label={deleteLabel} aria-busy={deleting || undefined} disabled={deletable.length === 0 || busy} onClick={() => requestDeleteHooks(deletable)}>{deleting ? <LoadingIcon size={15} /> : <Trash2 size={15} />}<span>{deleteLabel}</span></Button>,
      menu: <Menu.Item className="menuItem danger" disabled={deletable.length === 0 || busy} onSelect={() => requestDeleteHooks(deletable)}><Trash2 size={14} />{deleteLabel}<MenuShortcut><DeleteKeyIcon size={14} strokeWidth={1.8} /></MenuShortcut></Menu.Item>,
      measure: <><Trash2 size={15} /><span>{deleteLabel}</span></>,
      separatorBefore: true,
    },
  };
  return hookSelectionActionIds(selectedRows.length).map((id) => actions[id]);
}

function hookParameterRows(hook: HookRecord | null, enabledControl: ReactNode): HookParameterRow[] {
  if (!hook) return [];
  const command = hook.command;
  const rows: HookParameterRow[] = [
    { label: "Type", value: hookTypeLabel(hook) },
    { label: "Matcher", value: hook.matcher ?? "" },
    { label: "Enabled", render: enabledControl },
  ];
  if (command) rows.push({ label: "Command", value: command, mono: true, copyable: true });
  if (hook.url) rows.push({ label: "URL", value: hook.url, mono: true, copyable: true });
  if (hook.prompt) rows.push({ label: "Prompt", value: hook.prompt, mono: true, copyable: true });
  if (hook.filter) rows.push({ label: "If", value: hook.filter, mono: true });
  return rows;
}

export function HooksView({ rows, loadingRows = false, loadError = "", hasRows = false, onRetry, onDeleteHook, onDeleteHooks, onSetHookEnabled, onSetHooksEnabled, onReviewHook, locateHookId, onLocateHookComplete, projects = [] }: HooksViewProps) {
  const hookItems = useMemo(() => hookItemsFromRows(rows), [rows]);
  const [activeKey, setActiveKey] = useTabState("hooks.activeKey", hookItems[0]?.key ?? "");
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useTabState("hooks.query", "");
  const [detailCollapsed, setDetailCollapsed] = useTabState("hooks.detailCollapsed", false);
  const [pendingReviewItem, setPendingReviewItem] = useState<HookItem | null>(null);
  const [pendingDeleteItems, setPendingDeleteItems] = useState<HookItem[]>([]);
  const [hookLocatorRequest, setHookLocatorRequest] = useState("");
  const clearSelection = useCallback(() => setSelected([]), []);
  const {
    deletingKey,
    updatingEnabledKeys,
    reviewingKey,
    error: deleteError,
    clearError: clearDeleteError,
    setHookEnabled,
    setSelectedHooksEnabled,
    reviewHook,
    deleteHooks,
  } = useHookOperations({
    rows,
    onDeleteHook,
    onDeleteHooks,
    onSetHookEnabled,
    onSetHooksEnabled,
    onReviewHook,
    clearSelection,
  });
  const activeHookSelectionIdentityRef = useRef("");
  const normalizedQuery = query.trim().toLowerCase();
  useEffect(() => {
    if (!locateHookId) return;
    setQuery("");
    setHookLocatorRequest(locateHookId);
  }, [locateHookId]);
  const completeHookLocator = useCallback((id: string) => {
    setHookLocatorRequest((current) => current === id ? "" : current);
    onLocateHookComplete?.(id);
  }, [onLocateHookComplete]);
  const filteredHooks = useMemo(() => filterHooks(hookItems, query), [hookItems, query]);
  const requestReviewHook = useCallback((item: HookItem) => {
    if (hookReviewDisabledReason(item.hook)) return;
    setPendingReviewItem(item);
  }, []);
  const confirmReviewHook = useCallback(async () => {
    const item = pendingReviewItem;
    if (!item) return;
    setPendingReviewItem(null);
    await reviewHook(item);
  }, [pendingReviewItem, reviewHook]);
  const requestDeleteHooks = useCallback((items: HookItem[]) => {
    const { deletable } = hookSelectionTargets(items);
    if (deletable.length === 0) return;
    setPendingDeleteItems(deletable);
  }, []);
  const confirmDeleteHooks = useCallback(async () => {
    const targets = pendingDeleteItems;
    if (targets.length === 0 || deletingKey) return;
    setPendingDeleteItems([]);
    await deleteHooks(targets);
  }, [deleteHooks, deletingKey, pendingDeleteItems]);
  const pendingDeleteLoadingLabel = selectionDeleteLoadingLabel("hook", pendingDeleteItems.length);
  const pendingDeleteMessage = pendingDeleteItems.length === 1
    ? `Delete this hook from ${formatUserPath(hookSourcePath(pendingDeleteItems[0]?.hook))}?`
    : `Delete ${pendingDeleteItems.length} selected hooks?`;
  const hookColumns = useMemo((): ColumnDef<HookItem>[] => [
    {
      key: "event",
      header: "Event",
      type: ColumnDataType.Enum,
      sticky: true,
      groupBy: (item) => hookDisplayName(item.hook),
      sortable: true,
      sortValue: (item) => hookDisplayName(item.hook),
      width: "var(--data-freeze-column-width, 330px)",
      render: (item) => {
        const hook = item.hook;
        const handler = hookHandlerText(hook);
        return (
          <>
            <span className="dataCellTitleLine">
              <span className="dataCellTitle">{hookDisplayName(hook)}</span>
              {hook.needs_review ? (
                <Badge
                  as="button"
                  type="button"
                  tone="warning"
                  aria-label={`Review ${hookDisplayName(hook)}`}
                  onClick={(event) => {
                    event.stopPropagation();
                    requestReviewHook(item);
                  }}
                >
                  {reviewingKey === item.key ? "Reviewing…" : "Review"}
                </Badge>
              ) : null}
            </span>
            <span className="dataCellSubLine">
              <Tooltip content={handler} onlyWhenTruncated><span className="dataCellSub">{compactCommand(handler)}</span></Tooltip>
            </span>
          </>
        );
      },
    },
    {
      key: "agent",
      header: "Agent",
      type: ColumnDataType.Enum,
      groupBy: (item) => friendlyAgent(item.hook.agent),
      sortable: true,
      sortValue: (item) => friendlyAgent(item.hook.agent),
      width: "78px",
      render: (item) => <span className="ruleAgentCell"><AgentBadge agent={friendlyAgent(item.hook.agent)} /></span>,
    },
    {
      key: "enabled",
      header: "Enabled",
      type: ColumnDataType.Enum,
      groupBy: (item) => (item.hook.enabled ? "On" : "Off"),
      sortable: true,
      sortValue: (item) => (item.hook.enabled ? 1 : 0),
      width: "92px",
      render: (item) => (
        <HookEnabledSwitch
          checked={Boolean(item.hook.enabled)}
          disabledReason={hookEnableDisabledReasonForView(item.hook, updatingEnabledKeys.size > 0)}
          updating={updatingEnabledKeys.has(item.key)}
          onToggle={(enabled) => setHookEnabled(item, enabled)}
        />
      ),
    },
    ...(projects.length > 0 ? [scopeColumn<HookItem>(projects, (item) => hookSourcePath(item.hook))] : []),
    {
      key: "actions",
      header: "",
      width: "40px",
      render: (item) => (
        <HookActionsCell
          item={item}
          actions={renderDataTableSelectionMenu(hookSelectionActions(
            [item],
            DropdownMenu,
            setSelectedHooksEnabled,
            requestDeleteHooks,
            updatingEnabledKeys.size > 0 || Boolean(deletingKey),
            Boolean(deletingKey),
          ))}
        />
      ),
    },
  ], [deletingKey, projects, requestDeleteHooks, requestReviewHook, reviewingKey, setHookEnabled, setSelectedHooksEnabled, updatingEnabledKeys]);
  const activeItem = useMemo(() => {
    const exact = hookItems.find((item) => item.key === activeKey);
    if (exact) return exact;
    const previousIdentity = activeHookSelectionIdentityRef.current;
    return previousIdentity
      ? hookItems.find((item) => hookSelectionIdentity(item.hook) === previousIdentity)
      : undefined;
  }, [activeKey, hookItems]);
  const activeHook = activeItem?.hook ?? null;
  const {
    state: sourceState,
    data: sourceData,
    requestIdentity: sourceRequestIdentity,
    clearError: clearSourceError,
  } = useHookSource(activeHook);
  useEffect(() => clearDeleteError(), [clearDeleteError, sourceRequestIdentity]);
  activeHookSelectionIdentityRef.current = hookSelectionIdentity(activeHook);
  const activeEnableControl = useMemo(() => activeItem ? (
    <HookEnabledSwitch
      checked={Boolean(activeHook?.enabled)}
      disabledReason={hookEnableDisabledReasonForView(activeHook, updatingEnabledKeys.size > 0)}
      updating={updatingEnabledKeys.has(activeItem.key)}
      onToggle={(enabled) => setHookEnabled(activeItem, enabled)}
    />
  ) : null, [activeHook, activeItem, setHookEnabled, updatingEnabledKeys]);
  const parameterRows = useMemo(() => hookParameterRows(activeHook, activeEnableControl), [activeHook, activeEnableControl]);
  const openHookSourceInEditor = useCallback(() => {
    const path = activeHook ? hookSourcePath(activeHook) : "";
    if (!path) return;
    const line = sourceData?.source_line;
    void safeInvoke(TauriCommand.OpenInEditor, { path, line: line ?? undefined });
  }, [activeHook, sourceData?.source_line]);
  const rowContextMenu = useCallback((item: HookItem, { selectedRows, selected: isSelected }: { selectedRows: HookItem[]; selected: boolean }) => {
    const actionRows = isSelected ? selectedRows : [item];
    const actions = hookSelectionActions(
      actionRows,
      ContextMenu,
      setSelectedHooksEnabled,
      requestDeleteHooks,
      updatingEnabledKeys.size > 0 || Boolean(deletingKey),
      Boolean(deletingKey),
    );
    return actions.length > 0 ? renderDataTableSelectionMenu(actions) : null;
  }, [deletingKey, requestDeleteHooks, setSelectedHooksEnabled, updatingEnabledKeys.size]);
  const bottomBar = useCallback((selectedRows: HookItem[]) => (
    <DataTableSelectionActions
      actions={hookSelectionActions(
        selectedRows,
        DropdownMenu,
        setSelectedHooksEnabled,
        requestDeleteHooks,
        updatingEnabledKeys.size > 0 || Boolean(deletingKey),
        Boolean(deletingKey),
      )}
      ariaLabel="More selected hook actions"
    />
  ), [deletingKey, requestDeleteHooks, setSelectedHooksEnabled, updatingEnabledKeys.size]);

  useEffect(() => {
    if (!activeKey && hookItems[0]) setActiveKey(hookItems[0].key);
    if (activeKey && !hookItems.some((item) => item.key === activeKey)) {
      const preserved = hookItems.find((item) => hookSelectionIdentity(item.hook) === activeHookSelectionIdentityRef.current);
      setActiveKey(preserved?.key ?? hookItems[0]?.key ?? "");
    }
  }, [activeKey, hookItems]);

  useEffect(() => {
    setSelected((current) => current.filter((key) => hookItems.some((item) => item.key === key)));
  }, [hookItems]);

  return (
    <>
    <PanelGroup className="sessionsLayout hooksLayout" orientation="horizontal">
      <Panel className="sessionListPanel hookListPanel" defaultSize="54%" minSize="360px">
        <div className="sessionListPane hookListPane">
          <PageHeader title="Hooks" compact>
            <SearchField pageSearch placeholder="Search hooks" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
          </PageHeader>
          {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : null}
          <div className="sessionListBody">
            <DataTable
              rows={filteredHooks}
              columns={hookColumns}
              getRowId={(item) => item.key}
              getRowLabel={(item) => hookDisplayName(item.hook)}
              freezeColumn={HOOK_FREEZE_COLUMN}
              defaultSort={defaultSort}
              selectable
              selectedIds={selected}
              onSelectionChange={setSelected}
              onDeleteSelected={requestDeleteHooks}
              enableMarquee
              scrollRestorationKey="hooks.list"
              rowContextMenu={rowContextMenu}
              bottomBar={bottomBar}
              bottomBarActionsClassName="selectionActions"
              bottomBarCheckboxLabel="Select visible hooks from toolbar"
              selectionLabel="hooks"
              onRowClick={(item) => {
                setActiveKey(item.key);
                setDetailCollapsed(false);
              }}
              scrollToRowId={hookLocatorRequest}
              onScrollToRowComplete={completeHookLocator}
              loading={loadingRows && !hasRows}
              loadingLabel="Loading hooks"
              emptyState={loadError && !hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : (
                <EmptyState
                  icon={normalizedQuery ? <SearchX size={21} strokeWidth={1.8} /> : <Webhook size={27} strokeWidth={1.55} />}
                  iconTone={normalizedQuery ? "muted" : "accent"}
                  title={normalizedQuery ? "No hooks match this search" : "No hooks configured"}
                  description={normalizedQuery ? "Try another search." : "They appear here when an agent defines them."}
                />
              )}
            />
          </div>
        </div>
      </Panel>
      <DetailPanelHost
        collapsed={detailCollapsed}
        onExpand={() => setDetailCollapsed(false)}
        expandLabel="Expand hook detail"
        railLabel={activeHook ? hookDisplayName(activeHook) : ""}
        hasSelection={Boolean(activeHook)}
        emptyState={loadingRows ? <LoadingState label="Loading hooks" /> : <EmptyState compact title="Select a hook to view its details." />}
      >
        {activeHook ? (
          <DetailPanel
            title={hookDisplayName(activeHook)}
            meta={(
              <div className="threadMeta hookSourceMeta">
            <Tooltip content={formatUserPath(hookSourcePath(activeHook))} onlyWhenTruncated><span>{formatUserPath(hookSourcePath(activeHook)) || EMPTY_DISPLAY_VALUE}</span></Tooltip>
              </div>
            )}
            collapseLabel="Collapse hook detail"
            onCollapse={() => setDetailCollapsed(true)}
          >
            <div className="hookDetailBody">
              {deleteError ? <Toast tone="error" message={deleteError} onDismiss={clearDeleteError} /> : null}
              {parameterRows.length ? (
                <section className="hookDetailSection hookParametersSection">
                  <h3>Parameters</h3>
                  <div className="hookParameterTable">
                    {parameterRows.map((row) => (
                      "render" in row ? (
                        <div className="hookDetailRow" key={row.label}>
                          <span className="hookDetailLabel">{row.label}</span>
                          <div className="hookDetailValue">{row.render}</div>
                        </div>
                      ) : (
                        <HookDetailRow key={row.label} label={row.label} value={row.value} mono={row.mono} copyable={row.copyable} />
                      )
                    ))}
                  </div>
                </section>
              ) : null}
              <section className="hookDetailSection">
                <div className="hookSectionHeaderRow">
                  <h3>Config preview</h3>
                  <div className="hookSectionHeaderActions">
                    {hookSourcePath(activeHook) ? (
                      <button
                        className="hookCopyButton hookSourceActionButton"
                        aria-label="Open hook source in editor"
                        onClick={openHookSourceInEditor}
                      >
                        <Crosshair size={13} />
                      </button>
                    ) : null}
                    {sourceData?.content ? (
                      <CopyButton
                        className="hookCopyButton hookSourceCopyButton"
                        value={sourceData.content}
                        copyLabel={copyValueLabel("config preview")}
                        copiedLabel={copiedValueLabel("config preview")}
                      />
                    ) : null}
                  </div>
                </div>
                {sourceState.loading && !sourceData?.content ? (
                  <LoadingState className="hookSourceLoading" label="Loading source" />
                ) : sourceState.error && !sourceData?.content ? (
                  <Toast
                    tone="error"
                    message={sourceState.error}
                    onDismiss={clearSourceError}
                  />
                ) : sourceData?.content ? (
                  <div className="hookSourcePreview">
                    <Suspense fallback={<LoadingState className="hookSourceLoading" label="Loading preview" />}>
                      <HookSourcePreview content={sourceData.content} />
                    </Suspense>
                    {sourceState.loading ? (
                      <div className="hookSourceStatusOverlay" aria-live="polite">
                        <LoadingState label="Refreshing source" />
                      </div>
                    ) : sourceState.error ? (
                      <div className="hookSourceStatusOverlay">
                        <Toast
                          tone="error"
                          message={sourceState.error}
                          onDismiss={clearSourceError}
                        />
                      </div>
                    ) : null}
                  </div>
                ) : (
                  <div className="hookSourcePlaceholder">No source preview available</div>
                )}
              </section>
            </div>
          </DetailPanel>
        ) : null}
      </DetailPanelHost>
    </PanelGroup>
    <DialogShell
      open={pendingDeleteItems.length > 0}
      onOpenChange={(open) => !open && setPendingDeleteItems([])}
      descriptionId="hook-delete-description"
    >
          <Dialog.Title className="confirmDialogTitle">Delete hook{pendingDeleteItems.length === 1 ? "" : "s"}?</Dialog.Title>
          <p id="hook-delete-description" className="confirmDialogDescription">{pendingDeleteMessage}</p>
          <div className="confirmDialogActions">
            <DialogActionButton variant="secondary" onClick={() => setPendingDeleteItems([])}>Cancel</DialogActionButton>
            <DialogStatefulButton
              state={deletingKey ? AsyncStatus.Loading : AsyncStatus.Idle}
              loadingLabel={pendingDeleteLoadingLabel}
              variant="danger"
              aria-label={pendingDeleteItems.length === 1 ? "Delete hook" : "Delete hooks"}
              autoFocus={!deletingKey}
              onClick={() => void confirmDeleteHooks()}
            >
              {pendingDeleteItems.length === 1 ? "Delete hook" : "Delete hooks"}
            </DialogStatefulButton>
          </div>
    </DialogShell>
    <DialogShell
      open={Boolean(pendingReviewItem)}
      onOpenChange={(open) => !open && setPendingReviewItem(null)}
      descriptionId="hook-review-description"
    >
          <Dialog.Title className="confirmDialogTitle">Approve hook?</Dialog.Title>
          <p id="hook-review-description" className="confirmDialogDescription">
            Approve the current configuration for this {pendingReviewItem?.hook.agent ?? ""} hook?
          </p>
          <div className="hookReviewDialogDetails">
            <strong>{hookDisplayName(pendingReviewItem?.hook)}</strong>
            <span>{formatUserPath(hookSourcePath(pendingReviewItem?.hook))}</span>
            <code>{compactCommand(hookHandlerText(pendingReviewItem?.hook))}</code>
          </div>
          <div className="confirmDialogActions">
            <DialogActionButton variant="secondary" onClick={() => setPendingReviewItem(null)}>Cancel</DialogActionButton>
            <DialogStatefulButton
              state={reviewingKey ? AsyncStatus.Loading : AsyncStatus.Idle}
              loadingLabel="Approving hook"
              variant="primary"
              aria-label="Approve hook"
              autoFocus={!reviewingKey}
              onClick={() => void confirmReviewHook()}
            >
              Approve
            </DialogStatefulButton>
          </div>
    </DialogShell>
    </>
  );
}
