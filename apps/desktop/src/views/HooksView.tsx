import { Tooltip } from "../components/shared/Tooltip.tsx";
import { lazy, Suspense, useCallback, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import { Copy, Crosshair, FolderOpen, Power, PowerOff, SearchX, Trash2, Webhook } from "lucide-react";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { Button } from "../components/shared/Button.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
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
import "./HooksView.css";

import {
  HOOK_FREEZE_COLUMN,
  TauriCommand,
  compactCommand,
  copyText,
  formatUserPath,
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
  invokeCommand,
  isWebSource,
  safeInvoke,
  suppressNextClick,
} from "../lib/index.ts";

const HookSourcePreview = lazy(() => import("../features/hooks/HookSourcePreview.tsx").then(({ HookSourcePreview: component }) => ({ default: component })));

type HookRecord = {
  agent?: string | null;
  event?: string | null;
  matcher?: string | null;
  enabled?: boolean | null;
  needs_review?: boolean | null;
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
  loadError?: string;
  hasRows?: boolean;
  onRetry?: () => void;
  onDeleteHook?: (hook: HookRecord) => Promise<unknown>;
  onDeleteHooks?: (hooks: HookRecord[]) => Promise<unknown>;
  onSetHookEnabled?: (hook: HookRecord, enabled: boolean) => Promise<unknown>;
  onReviewHook?: (hook: HookRecord) => Promise<unknown>;
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
  const content = (
    <div className={`hookDetailValue ${mono ? "mono" : ""}`}>
      <span>{text || "-"}</span>
      {copyable && text ? (
        <CopyButton className="hookCopyButton" value={text} copyLabel={`Copy ${label}`} copiedLabel={`${label} copied`} />
      ) : null}
    </div>
  );
  return (
    <div className="hookDetailRow">
      <span className="hookDetailLabel">{label}</span>
      {isWebSource(text.trim()) ? content : <Tooltip content={text || undefined} onlyWhenTruncated>{content}</Tooltip>}
    </div>
  );
}

function hookEnableDisabledReason(hook: HookRecord | null): string {
  if (!hook) return "Missing hook source path";
  const path = hookSourcePath(hook);
  if (!path) return "Missing hook source path";
  if (path.startsWith("/etc/cursor/") || path.startsWith("/Library/Application Support/Cursor/") || path.startsWith("/etc/claude-code/") || path.startsWith("/Library/Application Support/ClaudeCode/")) {
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

function hookEnableDisabledReasonForView(hook: HookRecord | null, updating: boolean): string {
  return hookEnableDisabledReason(hook) || (updating ? "Updating hooks" : "");
}

function hookReviewDisabledReason(hook: HookRecord | null): string {
  if (!hook?.needs_review) return "Hook does not need review";
  return "";
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
        <FolderOpen size={14} />
        Reveal in Finder
      </Menu.Item>
      <Menu.Item className="skillMenuItem" disabled={!path} onSelect={() => path && copyText(path)}>
        <Copy size={14} />
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
    <RowActionsMenu
      ariaLabel={`Hook actions for ${hook.event ?? "hook"}`}
      onOpenChange={(open) => { if (!open) suppressNextClick(); }}
    >
      <HookActionsMenuItems Menu={DropdownMenu} item={item} onDeleteHooks={onDeleteHooks} />
    </RowActionsMenu>
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
  return rows;
}

export function HooksView({ rows, loadingRows = false, loadError = "", hasRows = false, onRetry, onDeleteHook, onDeleteHooks, onSetHookEnabled, onReviewHook }: HooksViewProps) {
  const hookItems = useMemo(() => hookItemsFromRows(rows), [rows]);
  const [activeKey, setActiveKey] = useState(hookItems[0]?.key ?? "");
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [detailCollapsed, setDetailCollapsed] = useState(false);
  const [deletingKey, setDeletingKey] = useState("");
  const [updatingEnabledKeys, setUpdatingEnabledKeys] = useState<Set<string>>(() => new Set());
  const [reviewingKey, setReviewingKey] = useState("");
  const [pendingReviewItem, setPendingReviewItem] = useState<HookItem | null>(null);
  const [deleteError, setDeleteError] = useState("");
  const [pendingDeleteItems, setPendingDeleteItems] = useState<HookItem[]>([]);
  const [sourceState, setSourceState] = useState<HookSourceState>({ key: "", loading: false, showLoading: false, data: null, error: "" });
  const normalizedQuery = query.trim().toLowerCase();
  const filteredHooks = useMemo(() => {
    if (!normalizedQuery) return hookItems;
    return hookItems.filter((item) => hookSearchText(item.hook).includes(normalizedQuery));
  }, [hookItems, normalizedQuery]);
  const setHookEnabled = useCallback(async (item: HookItem, enabled: boolean) => {
    if (!item?.hook || updatingEnabledKeys.size > 0) return;
    const disabledReason = hookEnableDisabledReason(item.hook);
    if (disabledReason) return;
    setUpdatingEnabledKeys(new Set([item.key]));
    setDeleteError("");
    try {
      const result = await onSetHookEnabled?.(item.hook, enabled);
      const updateError = hookOperationError(result);
      if (updateError) setDeleteError(updateError);
      else if (!result) setDeleteError("Could not update hook.");
    } catch (error) {
      setDeleteError(`${error}`);
    } finally {
      setUpdatingEnabledKeys(new Set());
    }
  }, [onSetHookEnabled, updatingEnabledKeys.size]);
  const setSelectedHooksEnabled = useCallback(async (items: HookItem[], enabled: boolean) => {
    if (updatingEnabledKeys.size > 0) return;
    const targets = items.filter((item) => (
      !hookEnableDisabledReason(item.hook) && Boolean(item.hook.enabled) !== enabled
    ));
    if (targets.length === 0) return;

    setUpdatingEnabledKeys(new Set(targets.map((item) => item.key)));
    setDeleteError("");
    let latestRows = rows;
    let firstError = "";
    try {
      for (const item of targets) {
        const identity = hookDeleteIdentity(item.hook);
        const hook = latestRows.find((row) => hookDeleteIdentity(row) === identity);
        if (!hook) {
          firstError ||= "Could not find selected hook.";
          continue;
        }
        try {
          const result = await onSetHookEnabled?.(hook, enabled);
          const updateError = hookOperationError(result);
          if (updateError) firstError ||= updateError;
          else if (!result) firstError ||= "Could not update selected hooks.";
          else if (Array.isArray(result)) latestRows = result as HookRecord[];
        } catch (error) {
          firstError ||= `${error}`;
        }
      }
    } finally {
      setUpdatingEnabledKeys(new Set());
      if (firstError) setDeleteError(firstError);
      else setSelected([]);
    }
  }, [onSetHookEnabled, rows, updatingEnabledKeys.size]);
  const reviewHook = useCallback(async (item: HookItem) => {
    if (!item?.hook || reviewingKey) return;
    const disabledReason = hookReviewDisabledReason(item.hook);
    if (disabledReason) return;
    setReviewingKey(item.key);
    setDeleteError("");
    const result = await onReviewHook?.(item.hook);
    setReviewingKey("");
    const reviewError = hookOperationError(result);
    if (reviewError) setDeleteError(reviewError);
    else if (!result) setDeleteError("Could not review hook.");
  }, [onReviewHook, reviewingKey]);
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
    const deletable = items.filter((item) => !hookDeleteDisabledReason(item.hook));
    if (deletable.length === 0) return;
    setPendingDeleteItems(deletable);
  }, []);
  const confirmDeleteHooks = useCallback(async () => {
    const targets = pendingDeleteItems;
    if (targets.length === 0 || deletingKey) return;
    setPendingDeleteItems([]);
    setDeleteError("");
    if (targets.length > 1 && onDeleteHooks) {
      setDeletingKey("batch");
      const result = await onDeleteHooks(targets.map((item) => item.hook));
      const deleteResultError = hookOperationError(result);
      if (deleteResultError) setDeleteError(deleteResultError);
      else if (!result) setDeleteError("Could not delete hooks.");
      setDeletingKey("");
      setSelected([]);
      return;
    }
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
        setDeleteError("Could not delete hook.");
        break;
      }
      if (Array.isArray(result)) latestRows = result as HookRecord[];
    }
    setDeletingKey("");
    setSelected([]);
  }, [deletingKey, onDeleteHook, onDeleteHooks, pendingDeleteItems, rows]);
  const pendingDeleteMessage = pendingDeleteItems.length === 1
    ? `Delete this hook from ${formatUserPath(hookSourcePath(pendingDeleteItems[0]?.hook))}?`
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
            <span className="hookEventTitle">
              <span className="dataCellTitle">{hook.event || "Hook"}</span>
              {hook.needs_review ? (
                <Badge
                  as="button"
                  type="button"
                  tone="warning"
                  aria-label={`Review ${hook.event || "hook"}`}
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
              <Tooltip content={handler} onlyWhenTruncated><span className="dataCellSub">{compactCommand(handler) || hookSourceTitle(hook)}</span></Tooltip>
            </span>
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
          disabledReason={hookEnableDisabledReasonForView(item.hook, updatingEnabledKeys.size > 0)}
          updating={updatingEnabledKeys.has(item.key)}
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
  ], [requestDeleteHooks, requestReviewHook, reviewingKey, setHookEnabled, updatingEnabledKeys]);
  const activeItem = useMemo(() => hookItems.find((item) => item.key === activeKey), [activeKey, hookItems]);
  const activeHook = activeItem?.hook ?? null;
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
    const enableTargets = selectedRows.filter((item) => (
      !hookEnableDisabledReason(item.hook) && !item.hook.enabled
    ));
    const disableTargets = selectedRows.filter((item) => (
      !hookEnableDisabledReason(item.hook) && Boolean(item.hook.enabled)
    ));
    const enabledUpdateInProgress = updatingEnabledKeys.size > 0;
    return (
      <>
        <Button
          size="sm"
          variant="ghost"
          className="hookBatchActionButton"
          aria-label="Enable selected hooks"
          disabled={enableTargets.length === 0 || enabledUpdateInProgress || Boolean(deletingKey)}
          onClick={() => void setSelectedHooksEnabled(selectedRows, true)}
        >
          <Power size={15} />
          Enable
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="hookBatchActionButton"
          aria-label="Disable selected hooks"
          disabled={disableTargets.length === 0 || enabledUpdateInProgress || Boolean(deletingKey)}
          onClick={() => void setSelectedHooksEnabled(selectedRows, false)}
        >
          <PowerOff size={15} />
          Disable
        </Button>
        <Button
          size="sm"
          variant="ghost"
          className="hookBatchActionButton danger"
          aria-label="Delete selected hooks"
          aria-busy={Boolean(deletingKey)}
          disabled={deletable.length === 0 || Boolean(deletingKey) || enabledUpdateInProgress}
          onClick={() => requestDeleteHooks(deletable)}
        >
          {deletingKey ? <LoadingIcon size={15} /> : <Trash2 size={15} />}
          {deletingKey ? null : "Delete"}
        </Button>
      </>
    );
  }, [deletingKey, requestDeleteHooks, setSelectedHooksEnabled, updatingEnabledKeys.size]);

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
    invokeCommand<HookSourceData>(TauriCommand.HookSourceRead, {
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
            <SearchField pageSearch placeholder="Search hooks" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
          </PageHeader>
          {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : null}
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
        railLabel={activeHook?.event ?? "Hook"}
        hasSelection={Boolean(activeHook)}
        emptyState={loadingRows ? <LoadingState label="Loading hooks" /> : <EmptyState compact title="Select a hook to view its details." />}
        hostClassName="hookDetailPanelHost"
      >
        {activeHook ? (
          <DetailPanel
            className="ruleEditorPanel hookDetailPanel"
            title={activeHook.event || "Hook"}
            meta={(
              <div className="threadMeta hookSourceMeta">
                <Tooltip content={formatUserPath(hookSourcePath(activeHook))} onlyWhenTruncated><span>{formatUserPath(hookSourcePath(activeHook)) || "-"}</span></Tooltip>
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
                  <LoadingState className="hookSourceLoading" label="Loading source" />
                ) : sourceState.loading ? null : sourceState.error ? (
                  <div className="hookSourcePreviewError">{sourceState.error}</div>
                ) : sourceState.data?.content ? (
                  <div className="hookSourcePreview">
                    <Suspense fallback={<LoadingState className="hookSourceLoading" label="Loading preview" />}>
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
              state={deletingKey ? "loading" : "idle"}
              loadingLabel="Deleting hooks"
              variant="danger"
              aria-label={pendingDeleteItems.length === 1 ? "Delete hook" : "Delete hooks"}
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
            Approve the current configuration for this {pendingReviewItem?.hook.agent ?? "agent"} hook?
          </p>
          <div className="hookReviewDialogDetails">
            <strong>{pendingReviewItem?.hook.event ?? "Hook"}</strong>
            <span>{formatUserPath(hookSourcePath(pendingReviewItem?.hook))}</span>
            <code>{compactCommand(hookHandlerText(pendingReviewItem?.hook)) || "No command"}</code>
          </div>
          <div className="confirmDialogActions">
            <DialogActionButton variant="secondary" onClick={() => setPendingReviewItem(null)}>Cancel</DialogActionButton>
            <DialogStatefulButton
              state={reviewingKey ? "loading" : "idle"}
              loadingLabel="Approving hook"
              variant="primary"
              aria-label="Approve hook"
              onClick={() => void confirmReviewHook()}
            >
              Approve
            </DialogStatefulButton>
          </div>
    </DialogShell>
    </>
  );
}
