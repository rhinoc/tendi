import { Tooltip } from "../components/shared/Tooltip.tsx";
import { lazy, Suspense, useCallback, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import { Crosshair, MoreHorizontal, RefreshCw, Search, Trash2 } from "lucide-react";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { DetailPanel } from "../components/shared/DetailPanel.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import "../components/shared/confirm-dialog.css";
import "./HooksView.css";

import {
  HOOK_FREEZE_COLUMN,
  TauriCommand,
  compactCommand,
  copyText,
  friendlyAgent,
  hookDeleteDisabledReason,
  hookDeleteIdentity,
  hookHandlerText,
  hookItemsFromRows,
  hookSearchText,
  hookSourcePath,
  hookSourceTitle,
  hookTrustHash,
  hookTypeLabel,
  safeInvoke,
  suppressNextClick,
} from "../lib/index.ts";

const HookSourcePreview = lazy(() => import("../components/shared/HookSourcePreview.tsx").then(({ HookSourcePreview: component }) => ({ default: component })));

type HookRecord = {
  agent?: string | null;
  event?: string | null;
  matcher?: string | null;
  enabled?: boolean | null;
  command?: string | null;
  script?: string | null;
  url?: string | null;
  prompt?: string | null;
  filter?: string | null;
  status_message?: string | null;
  statusMessage?: string | null;
  hook_type?: string | null;
  hookType?: string | null;
  path?: string | null;
  source?: string | null;
  trust_hash?: string | null;
  trustHash?: string | null;
  hash?: string | null;
};

type HookItem = { key: string; hook: HookRecord };

type HookSourceData = {
  content?: string;
  source_line?: number | null;
  path?: string;
} | null;

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

type HookSourceState = {
  key: string;
  loading: boolean;
  showLoading: boolean;
  data: HookSourceData;
  error: string;
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
  onDeleteHook?: (hook: HookRecord) => Promise<unknown>;
  onSetHookEnabled?: (hook: HookRecord, enabled: boolean) => Promise<unknown>;
};

const defaultSort: SortState = { key: "event", direction: "asc" };

function hookOperationError(result: unknown): string | undefined {
  if (result && typeof result === "object" && !Array.isArray(result) && "error" in result) {
    const error = (result as { error?: unknown }).error;
    return typeof error === "string" ? error : undefined;
  }
  return undefined;
}

export function HookDetailRow({ label, value, mono = false, copyable = false }: HookDetailRowProps) {
  const text = `${value ?? ""}`;
  return (
    <div className="hookDetailRow">
      <span className="hookDetailLabel">{label}</span>
      <Tooltip content={text || undefined} onlyWhenTruncated><div className={`hookDetailValue ${mono ? "mono" : ""}`}>
        <span>{text || "-"}</span>
        {copyable && text ? (
          <CopyButton className="hookCopyButton" value={text} copyLabel={`Copy ${label}`} copiedLabel={`${label} copied`} />
        ) : null}
      </div></Tooltip>
    </div>
  );
}

function hookEnableDisabledReason(hook: HookRecord | null): string {
  if (!hook) return "Missing hook source path";
  const path = hookSourcePath(hook);
  if (!path) return "Missing hook source path";
  if (path.startsWith("/etc/cursor/") || path.startsWith("/etc/claude-code/") || path.startsWith("/Library/Application Support/ClaudeCode/")) {
    return "Managed hook source";
  }
  if (path.includes("/.claude/plugins/")) {
    return "Plugin-managed hook source";
  }
  if (!path.endsWith(".json") && !path.endsWith(".toml")) {
    return "Hook source cannot be changed";
  }
  return "";
}

function HookEnabledSwitch({ checked, disabledReason = "", updating = false, onToggle }: HookEnabledSwitchProps) {
  // Use aria-disabled (not native disabled) so clicks still hit this control and do not fall through to row onClick.
  const disabled = Boolean(disabledReason) || updating;
  return (
    <Tooltip content={disabledReason || (checked ? "Disable hook" : "Enable hook")}><button
      type="button"
      className={`hookEnabledSwitch ${checked ? "on" : "off"} ${updating ? "updating" : ""}`}
      role="switch"
      aria-checked={checked}
      aria-label={checked ? "Disable hook" : "Enable hook"}
      aria-disabled={disabled || undefined}
      data-no-drag
      data-no-row-click
      onClick={(event) => {
        event.stopPropagation();
        if (disabled) return;
        onToggle?.(!checked);
      }}
      onKeyDown={(event) => {
        if (!disabled) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          event.stopPropagation();
        }
      }}
    >
      <span className="hookEnabledSwitchThumb" />
    </button></Tooltip>
  );
}

function HookActionsMenuItems({
  Menu,
  item,
  onDeleteHooks,
}: {
  Menu: HookMenuComponents;
  item: HookItem;
  onDeleteHooks: (items: HookItem[]) => void;
}) {
  const hook = item.hook;
  const path = hookSourcePath(hook);
  const deleteDisabledReason = hookDeleteDisabledReason(hook);
  return (
    <>
      <Menu.Item
        className="skillMenuItem"
        disabled={!path}
        onSelect={() => path && safeInvoke(TauriCommand.RevealInFinder, { path })}
      >
        Reveal in Finder
      </Menu.Item>
      <Menu.Item className="skillMenuItem" disabled={!path} onSelect={() => path && copyText(path)}>
        Copy path
      </Menu.Item>
      <Menu.Separator className="skillMenuSeparator" />
      <Menu.Item
        className="skillMenuItem danger"
        disabled={Boolean(deleteDisabledReason)}
        onSelect={() => onDeleteHooks([item])}
      >
        <Trash2 size={14} />
        Delete hook
      </Menu.Item>
    </>
  );
}

function HookActionsCell({
  item,
  onDeleteHooks,
}: {
  item: HookItem;
  onDeleteHooks: (items: HookItem[]) => void;
}) {
  const hook = item.hook;
  return (
    <DropdownMenu.Root onOpenChange={(open) => { if (!open) suppressNextClick(); }}>
      <DropdownMenu.Trigger asChild>
        <button className="iconButton" aria-label={`Hook actions for ${hook.event ?? "hook"}`}>
          <MoreHorizontal size={16} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="skillMenuContent" align="end" sideOffset={6}>
          <HookActionsMenuItems Menu={DropdownMenu} item={item} onDeleteHooks={onDeleteHooks} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

function hookParameterRows(hook: HookRecord | null, enabledControl: ReactNode): HookParameterRow[] {
  if (!hook) return [];
  const command = hook.command ?? hook.script;
  const rows: HookParameterRow[] = [
    { label: "Type", value: hookTypeLabel(hook) },
    { label: "Matcher", value: hook.matcher || "*" },
    { label: "Enabled", render: enabledControl },
  ];
  if (command) rows.push({ label: "Command", value: command, mono: true, copyable: true });
  if (hook.url) rows.push({ label: "URL", value: hook.url, mono: true, copyable: true });
  if (hook.prompt) rows.push({ label: "Prompt", value: hook.prompt, mono: true, copyable: true });
  if (hook.filter) rows.push({ label: "If", value: hook.filter, mono: true });
  const status = hook.status_message ?? hook.statusMessage;
  if (status) rows.push({ label: "Status", value: status });
  return rows;
}

export function HooksView({ rows, loadingRows = false, onDeleteHook, onSetHookEnabled }: HooksViewProps) {
  const hookItems = useMemo(() => hookItemsFromRows(rows), [rows]);
  const [activeKey, setActiveKey] = useState(hookItems[0]?.key ?? "");
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [detailCollapsed, setDetailCollapsed] = useState(false);
  const [deletingKey, setDeletingKey] = useState("");
  const [updatingEnabledKey, setUpdatingEnabledKey] = useState("");
  const [deleteError, setDeleteError] = useState("");
  const [pendingDeleteItems, setPendingDeleteItems] = useState<HookItem[]>([]);
  const [sourceState, setSourceState] = useState<HookSourceState>({ key: "", loading: false, showLoading: false, data: null, error: "" });
  const normalizedQuery = query.trim().toLowerCase();
  const filteredHooks = useMemo(() => {
    if (!normalizedQuery) return hookItems;
    return hookItems.filter((item) => hookSearchText(item.hook).includes(normalizedQuery));
  }, [hookItems, normalizedQuery]);
  const setHookEnabled = useCallback(async (item: HookItem, enabled: boolean) => {
    if (!item?.hook || updatingEnabledKey) return;
    const disabledReason = hookEnableDisabledReason(item.hook);
    if (disabledReason) return;
    setUpdatingEnabledKey(item.key);
    setDeleteError("");
    const result = await onSetHookEnabled?.(item.hook, enabled);
    setUpdatingEnabledKey("");
    const updateError = hookOperationError(result);
    if (updateError) setDeleteError(updateError);
    else if (!result) setDeleteError("Could not update hook. Refresh hooks and try again.");
  }, [onSetHookEnabled, updatingEnabledKey]);
  const requestDeleteHooks = useCallback((items: HookItem[]) => {
    const deletable = items.filter((item) => !hookDeleteDisabledReason(item.hook));
    if (deletable.length === 0) return;
    setPendingDeleteItems(deletable);
  }, []);
  const confirmDeleteHooks = useCallback(async () => {
    const targets = pendingDeleteItems;
    if (targets.length === 0 || deletingKey) return;
    setPendingDeleteItems([]);
    setDeleteError("");
    let latestRows = rows;
    for (const item of targets) {
      const identity = hookDeleteIdentity(item.hook);
      const hook = latestRows.find((row) => hookDeleteIdentity(row) === identity);
      if (!hook) continue;
      setDeletingKey(identity);
      const result = await onDeleteHook?.(hook);
      const deleteResultError = hookOperationError(result);
      if (deleteResultError) {
        setDeleteError(deleteResultError);
        break;
      }
      if (!result) {
        setDeleteError("Could not delete hook. Refresh hooks and try again.");
        break;
      }
      if (Array.isArray(result)) latestRows = result as HookRecord[];
    }
    setDeletingKey("");
    setSelected([]);
  }, [deletingKey, onDeleteHook, pendingDeleteItems, rows]);
  const pendingDeleteMessage = pendingDeleteItems.length === 1
    ? `Delete this hook from ${hookSourcePath(pendingDeleteItems[0]?.hook)}?`
    : `Delete ${pendingDeleteItems.length} selected hooks?`;
  const hookColumns = useMemo((): ColumnDef<HookItem>[] => [
    {
      key: "event",
      header: "Event",
      type: "enum",
      sticky: true,
      groupBy: (item) => item.hook.event || "Unknown",
      sortable: true,
      sortValue: (item) => item.hook.event || "",
      width: "var(--data-freeze-column-width, 330px)",
      render: (item) => {
        const hook = item.hook;
        const handler = hookHandlerText(hook);
        return (
          <>
            <span className="dataCellTitle">{hook.event || "Hook"}</span>
            <Tooltip content={handler} onlyWhenTruncated><span className="dataCellSub">{compactCommand(handler) || hookSourceTitle(hook)}</span></Tooltip>
          </>
        );
      },
    },
    {
      key: "agent",
      header: "Agent",
      type: "enum",
      groupBy: (item) => friendlyAgent(item.hook.agent),
      sortable: true,
      sortValue: (item) => friendlyAgent(item.hook.agent),
      width: "78px",
      render: (item) => <span className="ruleAgentCell"><AgentBadge agent={friendlyAgent(item.hook.agent)} /></span>,
    },
    {
      key: "type",
      header: "Type",
      type: "enum",
      sortable: true,
      sortValue: (item) => hookTypeLabel(item.hook),
      width: "112px",
      value: (item) => hookTypeLabel(item.hook),
    },
    {
      key: "matcher",
      header: "Matcher",
      type: "enum",
      sortable: true,
      sortValue: (item) => item.hook.matcher || "",
      width: "190px",
      value: (item) => item.hook.matcher || "*",
      title: (item) => item.hook.matcher || "",
    },
    {
      key: "enabled",
      header: "Enabled",
      type: "enum",
      groupBy: (item) => (item.hook.enabled ? "On" : "Off"),
      sortable: true,
      sortValue: (item) => (item.hook.enabled ? 1 : 0),
      width: "92px",
      render: (item) => (
        <HookEnabledSwitch
          checked={Boolean(item.hook.enabled)}
          disabledReason={hookEnableDisabledReason(item.hook)}
          updating={updatingEnabledKey === item.key}
          onToggle={(enabled) => setHookEnabled(item, enabled)}
        />
      ),
    },
    {
      key: "actions",
      header: "",
      width: "40px",
      render: (item) => <HookActionsCell item={item} onDeleteHooks={requestDeleteHooks} />,
    },
  ], [requestDeleteHooks, setHookEnabled, updatingEnabledKey]);
  const activeItem = useMemo(() => hookItems.find((item) => item.key === activeKey), [activeKey, hookItems]);
  const activeHook = activeItem?.hook ?? null;
  const activeEnableControl = useMemo(() => activeItem ? (
    <HookEnabledSwitch
      checked={Boolean(activeHook?.enabled)}
      disabledReason={hookEnableDisabledReason(activeHook)}
      updating={updatingEnabledKey === activeItem.key}
      onToggle={(enabled) => setHookEnabled(activeItem, enabled)}
    />
  ) : null, [activeHook, activeItem, setHookEnabled, updatingEnabledKey]);
  const parameterRows = useMemo(() => hookParameterRows(activeHook, activeEnableControl), [activeHook, activeEnableControl]);
  const openHookSourceInEditor = useCallback(() => {
    const path = activeHook ? hookSourcePath(activeHook) : "";
    if (!path) return;
    const line = sourceState.data?.source_line ?? undefined;
    void safeInvoke(TauriCommand.OpenInEditor, { path, line });
  }, [activeHook, sourceState.data?.source_line]);
  const rowContextMenu = useCallback((item: HookItem, { selectedRows, selected: isSelected }: { selectedRows: HookItem[]; selected: boolean }) => {
    const showBulk = isSelected && selectedRows.length > 1;
    return showBulk ? (
      <>
        <ContextMenu.Item
          className="skillMenuItem danger"
          disabled={selectedRows.every((row) => hookDeleteDisabledReason(row.hook))}
          onSelect={() => requestDeleteHooks(selectedRows)}
        >
          <Trash2 size={14} />
          Delete selected
        </ContextMenu.Item>
      </>
    ) : (
      <HookActionsMenuItems Menu={ContextMenu} item={item} onDeleteHooks={requestDeleteHooks} />
    );
  }, [requestDeleteHooks]);
  const bottomBar = useCallback((selectedRows: HookItem[]) => {
    const deletable = selectedRows.filter((item) => !hookDeleteDisabledReason(item.hook));
    return (
      <button
        className="danger"
        aria-label="Delete selected hooks"
        disabled={deletable.length === 0 || Boolean(deletingKey)}
        onClick={() => requestDeleteHooks(deletable)}
      >
        {deletingKey ? <RefreshCw className="loadingSpinner" size={15} /> : <Trash2 size={15} />}
        Delete
      </button>
    );
  }, [deletingKey, requestDeleteHooks]);

  useEffect(() => {
    setDeleteError("");
    if (!activeHook) {
      setSourceState({ key: "", loading: false, showLoading: false, data: null, error: "" });
      return undefined;
    }
    const path = hookSourcePath(activeHook);
    if (!path) {
      setSourceState({ key: activeKey, loading: false, showLoading: false, data: null, error: "Missing hook source path" });
      return undefined;
    }
    let cancelled = false;
    const loadingTimer = window.setTimeout(() => {
      if (!cancelled) {
        setSourceState((current) => (
          current.key === activeKey && current.loading
            ? { ...current, showLoading: true }
            : current
        ));
      }
    }, 180);
    setSourceState({ key: activeKey, loading: true, showLoading: false, data: null, error: "" });
    invoke<HookSourceData>("hook_source_read", {
      path,
      expectedTrustHash: hookTrustHash(activeHook),
      event: activeHook.event ?? null,
      matcher: activeHook.matcher ?? null,
      hookType: activeHook.hook_type ?? activeHook.hookType ?? null,
      command: activeHook.command ?? activeHook.script ?? null,
      url: activeHook.url ?? null,
      prompt: activeHook.prompt ?? null,
      filter: activeHook.filter ?? null,
      statusMessage: activeHook.status_message ?? activeHook.statusMessage ?? null,
      enabled: activeHook.enabled ?? null,
    })
      .then((data) => {
        window.clearTimeout(loadingTimer);
        if (!cancelled) setSourceState({ key: activeKey, loading: false, showLoading: false, data, error: "" });
      })
      .catch((error) => {
        window.clearTimeout(loadingTimer);
        if (!cancelled) setSourceState({ key: activeKey, loading: false, showLoading: false, data: null, error: `${error}` });
      });
    return () => {
      cancelled = true;
      window.clearTimeout(loadingTimer);
    };
  }, [activeHook, activeKey]);

  useEffect(() => {
    if (!activeKey && hookItems[0]) setActiveKey(hookItems[0].key);
    if (activeKey && !hookItems.some((item) => item.key === activeKey)) {
      setActiveKey(hookItems[0]?.key ?? "");
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
            <div className="searchBox"><Search size={15} /><input placeholder="Search hooks" value={query} onChange={(event) => setQuery(event.target.value)} /></div>
          </PageHeader>
          <div className="sessionListBody">
            <DataTable
              rows={filteredHooks}
              columns={hookColumns}
              getRowId={(item) => item.key}
              getRowLabel={(item) => item.hook.event || "Hook"}
              freezeColumn={HOOK_FREEZE_COLUMN}
              defaultSort={defaultSort}
              selectable
              selectedIds={selected}
              onSelectionChange={setSelected}
              enableMarquee
              rowContextMenu={rowContextMenu}
              bottomBar={bottomBar}
              bottomBarCheckboxLabel="Select visible hooks from toolbar"
              selectionLabel="hooks"
              onRowClick={(item) => {
                setActiveKey(item.key);
                setDetailCollapsed(false);
              }}
              loading={loadingRows}
              loadingLabel={<LoadingInline label="Loading hooks" />}
              emptyState={normalizedQuery
                ? "No hooks match this search. Try another search."
                : "No hooks configured. They appear here when an agent defines them."}
            />
          </div>
        </div>
      </Panel>
      <DetailPanelHost
        collapsed={detailCollapsed}
        onExpand={() => setDetailCollapsed(false)}
        expandLabel="Expand hook detail"
        railLabel={activeHook?.event ?? "Hook"}
        hasSelection={Boolean(activeHook)}
        emptyState={(
          <div className="emptyState">
            {loadingRows ? <LoadingInline label="Loading hooks" /> : "Select a hook to view its details."}
          </div>
        )}
        hostClassName="hookDetailPanelHost"
      >
        {activeHook ? (
          <DetailPanel
            className="ruleEditorPanel hookDetailPanel"
            title={activeHook.event || "Hook"}
            meta={(
              <div className="threadMeta hookSourceMeta">
                <Tooltip content={hookSourcePath(activeHook)} onlyWhenTruncated><span>{hookSourcePath(activeHook) || "-"}</span></Tooltip>
              </div>
            )}
            collapseLabel="Collapse hook detail"
            onCollapse={() => setDetailCollapsed(true)}
          >
            <div className="hookDetailBody">
              {deleteError ? <div className="hookDeleteError">{deleteError}</div> : null}
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
                    {sourceState.data?.content ? (
                      <CopyButton
                        className="hookCopyButton hookSourceCopyButton"
                        value={sourceState.data?.content ?? ""}
                        copyLabel="Copy config preview"
                        copiedLabel="Config preview copied"
                      />
                    ) : null}
                  </div>
                </div>
                {sourceState.loading && sourceState.showLoading ? (
                  <div className="hookSourcePlaceholder"><LoadingInline label="Loading source" /></div>
                ) : sourceState.loading ? null : sourceState.error ? (
                  <div className="hookSourcePreviewError">{sourceState.error}</div>
                ) : sourceState.data?.content ? (
                  <div className="hookSourcePreview">
                    <Suspense fallback={<div className="hookSourcePlaceholder"><LoadingInline label="Loading preview" /></div>}>
                      <HookSourcePreview content={sourceState.data.content} />
                    </Suspense>
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
    <Dialog.Root open={pendingDeleteItems.length > 0} onOpenChange={(open) => !open && setPendingDeleteItems([])}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content className="confirmDialogPanel" aria-describedby="hook-delete-description" data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <Dialog.Title className="confirmDialogTitle">Delete hook{pendingDeleteItems.length === 1 ? "" : "s"}?</Dialog.Title>
          <p id="hook-delete-description" className="confirmDialogDescription">{pendingDeleteMessage}</p>
          <div className="confirmDialogActions">
            <button className="secondary" onClick={() => setPendingDeleteItems([])}>Cancel</button>
            <button className="danger" disabled={Boolean(deletingKey)} onClick={() => void confirmDeleteHooks()}>
              {deletingKey
                ? "Deleting…"
                : pendingDeleteItems.length === 1
                  ? "Delete hook"
                  : "Delete hooks"}
            </button>
          </div>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
    </>
  );
}
