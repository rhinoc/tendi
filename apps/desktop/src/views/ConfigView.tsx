import { Tooltip } from "../components/shared/Tooltip.tsx";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import { ArrowRightLeft, Code2, Copy, Files, FolderOpen, Plus, RefreshCw, Trash2 } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef } from "../components/DataTable.types.ts";
import { AgentOptionLabel } from "../components/shared/AgentOptionLabel.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { Button } from "../components/shared/Button.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { DiscardChangesDialog } from "../components/shared/DiscardChangesDialog.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { DialogStatefulButton } from "../components/shared/DialogStatefulButton.tsx";
import { CopyPathMenuItem, DeleteMenuItem, OpenInEditorMenuItem, RevealInFinderMenuItem, type DataTableMenuComponents } from "../components/shared/DataTableMenus.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { DataTableSelectionActions, renderDataTableSelectionMenu, type DataTableSelectionActionDefinition } from "../components/shared/DataTableSelectionActions.tsx";
import { DeleteConfirmationDialog } from "../components/shared/DeleteConfirmationDialog.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { ResizeSeparator } from "../components/shared/ResizeSeparator.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { StatefulButton } from "../components/shared/StatefulButton.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { actionLabels, DaemonCommandError, selectionDeleteLabel, TauriCommand, compactDateTime, configSelectionActionIds, formatUserPath, friendlyAgent, invokeCommand, logger, mergeThreeWay, safeInvoke, subscribeDaemonEvents } from "../lib/index.ts";
import { resolveSelectValue } from "../lib/select-options.ts";
import type { DaemonEvent } from "../lib/index.ts";
import "./ConfigView.css";

const BASE_PROFILE_VALUE = "__tendi_base_config__";
const MarkdownFilePane = lazy(() => import("../components/shared/MarkdownFilePane.tsx").then(({ MarkdownFilePane: component }) => ({ default: component })));

type AgentConfigFile = Record<string, unknown> & {
  agent: string;
  label: string;
  path: string;
  format: "json" | "toml";
  exists: boolean;
  updatedAt?: string;
  profile?: string;
};

type AgentConfigContent = {
  path: string;
  content: string;
  sha256: string;
  exists: boolean;
  updatedAt?: string;
};

type ConfigConflict = {
  base: string;
  local: string;
  disk: string;
  diskSha256: string;
  diskExists: boolean;
  diskUpdatedAt?: string;
  merged: boolean;
};

type ConfigProfileCommandResult = {
  configProfiles: Record<string, string>;
};

type ConfigViewProps = {
  /** App-owned persisted profile state. ConfigView only presents and mutates this slice. */
  activeProfiles: Record<string, string>;
  onActiveProfilesChange: (profiles: Record<string, string>) => void;
};

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return "Config operation failed";
}

function profileValueForConfig(config: AgentConfigFile | undefined): string {
  return config?.profile ? `profile:${config.profile}` : BASE_PROFILE_VALUE;
}

function isConfigSnapshot(value: unknown): value is AgentConfigContent {
  if (!value || typeof value !== "object") return false;
  const snapshot = value as Partial<AgentConfigContent>;
  return typeof snapshot.path === "string"
    && typeof snapshot.content === "string"
    && typeof snapshot.sha256 === "string"
    && typeof snapshot.exists === "boolean";
}

function isConflictMarkerContent(value: string) {
  return /^(?:<{7} |\|{7} |={7}$|>{7} )/m.test(value);
}

function configSelectionActions(
  selectedRows: AgentConfigFile[],
  Menu: DataTableMenuComponents,
  requestDeleteConfigs: (configs: AgentConfigFile[]) => void,
  deleting: boolean,
): DataTableSelectionActionDefinition[] {
  const config = selectedRows.length === 1 ? selectedRows[0] : undefined;
  const path = config?.path ?? "";
  const deletable = selectedRows.filter((item) => item.exists);
  const deleteLabel = selectionDeleteLabel("config file", selectedRows.length);
  const actions: Record<string, DataTableSelectionActionDefinition> = {
    "open-editor": {
      id: "open-editor",
      direct: <button type="button" aria-label={actionLabels.openInEditor} disabled={!path} onClick={() => path && void safeInvoke(TauriCommand.OpenInEditor, { path })}><Code2 size={15} /><span>{actionLabels.openInEditor}</span></button>,
      menu: <OpenInEditorMenuItem Menu={Menu} path={path} />,
      measure: <><Code2 size={15} /><span>{actionLabels.openInEditor}</span></>,
    },
    reveal: {
      id: "reveal",
      direct: <button type="button" aria-label={actionLabels.revealInFinder} disabled={!path} onClick={() => path && void safeInvoke(TauriCommand.RevealInFinder, { path })}><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></button>,
      menu: <RevealInFinderMenuItem Menu={Menu} path={path} />,
      measure: <><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></>,
      separatorBefore: true,
    },
    "copy-path": {
      id: "copy-path",
      direct: <CopyButton value={path} disabled={!path} copyLabel={actionLabels.copyPath} copiedLabel={actionLabels.pathCopied} iconSize={15}>{actionLabels.copyPath}</CopyButton>,
      menu: <CopyPathMenuItem Menu={Menu} path={path} />,
      measure: <><Copy size={15} /><span>{actionLabels.copyPath}</span></>,
    },
    delete: {
      id: "delete",
      direct: <button type="button" className="danger" aria-label={deleteLabel} disabled={deletable.length === 0 || deleting} onClick={() => requestDeleteConfigs(deletable)}><Trash2 size={15} /><span>{deleteLabel}</span></button>,
      menu: <DeleteMenuItem Menu={Menu} label={deleteLabel} disabled={deletable.length === 0 || deleting} onSelect={() => requestDeleteConfigs(deletable)} />,
      measure: <><Trash2 size={15} /><span>{deleteLabel}</span></>,
      separatorBefore: true,
    },
  };
  return configSelectionActionIds(selectedRows.length).map((id) => actions[id]);
}

export function ConfigView({ activeProfiles: activeProfilesProp, onActiveProfilesChange }: ConfigViewProps) {
  const [configs, setConfigs] = useState<AgentConfigFile[]>([]);
  const [activePath, setActivePath] = useState("");
  const [selectedPath, setSelectedPath] = useState("");
  const [selected, setSelected] = useState<string[]>([]);
  const [content, setContent] = useState("");
  const [originalContent, setOriginalContent] = useState("");
  const [sha256, setSha256] = useState("");
  const [loadingConfig, setLoadingConfig] = useState(true);
  const [loadingConfigs, setLoadingConfigs] = useState(true);
  const [loadError, setLoadError] = useState("");
  const [contentError, setContentError] = useState("");
  const [saving, setSaving] = useState(false);
  const [pendingPath, setPendingPath] = useState("");
  const [pendingReload, setPendingReload] = useState(false);
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [pendingDeleteConfigs, setPendingDeleteConfigs] = useState<AgentConfigFile[]>([]);
  const [pendingDeleteConfirmConfigs, setPendingDeleteConfirmConfigs] = useState<AgentConfigFile[]>([]);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const [selectedProfileValue, setSelectedProfileValue] = useState(BASE_PROFILE_VALUE);
  const [profileDialogOpen, setProfileDialogOpen] = useState(false);
  const [profileName, setProfileName] = useState("");
  const [profileSaving, setProfileSaving] = useState(false);
  const [profileSwitching, setProfileSwitching] = useState(false);
  const [profileError, setProfileError] = useState("");
  const [conflict, setConflict] = useState<ConfigConflict | null>(null);
  const [saveError, setSaveError] = useState("");
  const readRequestRef = useRef(0);
  const contentRef = useRef(content);
  const originalContentRef = useRef(originalContent);
  const sha256Ref = useRef(sha256);
  const activePathRef = useRef(activePath);
  contentRef.current = content;
  originalContentRef.current = originalContent;
  sha256Ref.current = sha256;
  activePathRef.current = activePath;
  const activeConfig = configs.find((config) => config.path === activePath) ?? null;
  const dirty = content !== originalContent;
  const editorSaveError = conflict
    ? conflict.merged
      ? "Resolve highlighted conflicts before saving."
      : conflict.diskExists
        ? "Source file changed on disk. Review the conflict before saving."
        : "Source file was deleted."
    : saveError;
  const profileAgent = activeConfig?.agent ?? null;
  const profileConfigs = useMemo(
    () => profileAgent ? configs.filter((config) => config.agent === profileAgent && config.profile) : [],
    [configs, profileAgent],
  );
  const activeProfiles = activeProfilesProp;
  const activeProfile = profileAgent ? activeProfiles[profileAgent] ?? "" : "";
  const profileOptions = useMemo(() => [
    { value: BASE_PROFILE_VALUE, label: "Base config" },
    ...profileConfigs.map((config) => ({
      value: `profile:${config.profile}`,
      label: config.profile ?? "",
    })),
  ], [profileConfigs]);
  const activeProfileValue = activeProfile && profileConfigs.some((config) => config.profile === activeProfile)
    ? `profile:${activeProfile}`
    : BASE_PROFILE_VALUE;
  const resolvedSelectedProfileValue = resolveSelectValue(selectedProfileValue, profileOptions);
  const profileSelectionChanged = resolvedSelectedProfileValue !== activeProfileValue;
  const updateActiveProfiles = useCallback((next: Record<string, string>) => {
    onActiveProfilesChange(next);
  }, [onActiveProfilesChange]);
  const columns = useMemo<ColumnDef<AgentConfigFile>[]>(() => [
    {
      key: "agent",
      header: "Agent",
      width: "minmax(180px, 1fr)",
      render: (config) => (
        <div className="configAgentCell">
          <AgentOptionLabel agent={config.agent} variant="filter" collapsed />
          <span className="configAgentText">
            <strong className="dataCellTitle">{config.label}</strong>
            <span className="dataCellSubLine">
              <Tooltip content={formatUserPath(config.path)} onlyWhenTruncated><span className="dataCellSub">{formatUserPath(config.path)}</span></Tooltip>
            </span>
          </span>
        </div>
      ),
    },
    {
      key: "updatedAt",
      header: "Updated",
      type: "date",
      sortValue: (config) => config.updatedAt ?? "",
      width: "116px",
      value: (config) => compactDateTime(config.updatedAt),
      empty: "",
    },
  ], []);
  const applyExternalSnapshot = useCallback((next: AgentConfigContent) => {
    setConfigs((current) => current.map((config) => (
      config.path === next.path ? { ...config, exists: next.exists, updatedAt: next.updatedAt } : config
    )));
    if (next.path !== activePathRef.current || next.sha256 === sha256Ref.current) return;
    const local = contentRef.current;
    const base = originalContentRef.current;
    if (local === base) {
      setContent(next.content);
      setOriginalContent(next.content);
      setSha256(next.sha256);
      setSaveError("");
    } else {
      setConflict({
        base,
        local,
        disk: next.content,
        diskSha256: next.sha256,
        diskExists: next.exists,
        diskUpdatedAt: next.updatedAt,
        merged: false,
      });
      setSaveError("");
    }
  }, []);
  const readExternalSnapshot = useCallback(async (path: string) => {
    try {
      const next = await invokeCommand<AgentConfigContent>(TauriCommand.AgentConfigRead, { path });
      if (next.path === activePathRef.current) applyExternalSnapshot(next);
    } catch (error) {
      setSaveError(errorMessage(error));
    }
  }, [applyExternalSnapshot]);
  const readConfig = useCallback(async (path: string, config?: AgentConfigFile) => {
    const requestId = ++readRequestRef.current;
    setSelectedPath(path);
    setLoadingConfig(true);
    setContentError("");
    try {
      const next = await invokeCommand<AgentConfigContent>(TauriCommand.AgentConfigRead, { path });
      if (requestId !== readRequestRef.current) return;
      setSelectedPath(next.path);
      setActivePath(next.path);
      if (config) setSelectedProfileValue(profileValueForConfig(config));
      setContent(next.content);
      setOriginalContent(next.content);
      setSha256(next.sha256);
      setConflict(null);
      setSaveError("");
      setConfigs((current) => current.map((config) => (
        config.path === next.path ? { ...config, exists: next.exists, updatedAt: next.updatedAt } : config
      )));
    } catch (error) {
      if (requestId !== readRequestRef.current) return;
      setContentError(errorMessage(error));
    } finally {
      if (requestId === readRequestRef.current) setLoadingConfig(false);
    }
  }, []);

  const loadConfigs = useCallback(async () => {
    setLoadingConfigs(true);
    setLoadingConfig(true);
    setLoadError("");
    try {
      const next = await invokeCommand<AgentConfigFile[]>(TauriCommand.AgentConfigsList);
      setConfigs(next);
      const selected = next.find((config) => config.path === activePath) ?? next[0];
      if (selected) {
        setSelectedProfileValue(profileValueForConfig(selected));
        await readConfig(selected.path, selected);
      } else {
        readRequestRef.current += 1;
        setActivePath("");
        setSelectedPath("");
        setContent("");
        setOriginalContent("");
        setSha256("");
        setConflict(null);
        setSaveError("");
        setContentError("");
        setLoadingConfig(false);
      }
    } catch (error) {
      setLoadError(errorMessage(error));
      setLoadingConfig(false);
    } finally {
      setLoadingConfigs(false);
    }
  }, [activePath, readConfig]);

  useEffect(() => {
    void loadConfigs();
    // The initial catalog load must not repeat after activePath is populated.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => () => {
    readRequestRef.current += 1;
  }, []);

  useEffect(() => {
    if (!activePath) return undefined;
    let disposed = false;
    let unsubscribe: (() => void) | undefined;
    void invokeCommand(TauriCommand.AgentConfigWatch, { path: activePath }).catch((error) => {
      if (!disposed) logger.warn("failed to watch config", { path: activePath, error: errorMessage(error) });
    });
    void subscribeDaemonEvents((event: DaemonEvent) => {
      if (disposed || event.event !== "config://changed" || !isConfigSnapshot(event.payload)) return;
      applyExternalSnapshot(event.payload);
    }).then((dispose) => {
      if (disposed) dispose();
      else unsubscribe = dispose;
    }).catch((error) => {
      if (!disposed) logger.warn("failed to subscribe to config changes", { error: errorMessage(error) });
    });
    return () => {
      disposed = true;
      unsubscribe?.();
    };
  }, [activePath, applyExternalSnapshot]);

  useEffect(() => {
    setSelected((current) => current.filter((path) => configs.some((config) => config.path === path)));
  }, [configs]);

  const chooseConfig = (path: string) => {
    if (path === activePath) {
      if (selectedPath !== path) {
        readRequestRef.current += 1;
        setSelectedPath(path);
        setSelectedProfileValue(profileValueForConfig(configs.find((config) => config.path === path)));
        setContentError("");
        setLoadingConfig(false);
      }
      return;
    }
    if (path === selectedPath) return;
    if (dirty) {
      setPendingPath(path);
      setPendingReload(false);
      setShowDiscardDialog(true);
      return;
    }
    void readConfig(path, configs.find((config) => config.path === path));
  };

  const reload = () => {
    if (!activePath) {
      void loadConfigs();
      return;
    }
    if (dirty) {
      setPendingPath("");
      setPendingReload(true);
      setShowDiscardDialog(true);
      return;
    }
    void loadConfigs();
  };

  const discardPendingChanges = () => {
    const deleteTargets = pendingDeleteConfigs;
    const nextPath = pendingPath;
    const shouldReload = pendingReload;
    setPendingDeleteConfigs([]);
    setPendingPath("");
    setPendingReload(false);
    if (deleteTargets.length > 0) {
      setContent(originalContent);
      setSha256(sha256);
      setConflict(null);
      setSaveError("");
      setPendingDeleteConfirmConfigs(deleteTargets);
      return;
    }
    if (nextPath) {
      setSelectedProfileValue(profileValueForConfig(configs.find((config) => config.path === nextPath)));
      void readConfig(nextPath, configs.find((config) => config.path === nextPath));
    }
    else if (shouldReload) void loadConfigs();
  };

  const requestDeleteConfigs = useCallback((items: AgentConfigFile[]) => {
    const deletable = items.filter((config) => config.exists);
    if (deletable.length === 0) return;
    if (dirty && deletable.some((config) => config.path === activePath)) {
      setPendingDeleteConfigs(deletable);
      setPendingDeleteConfirmConfigs([]);
      setShowDiscardDialog(true);
      return;
    }
    setPendingDeleteConfirmConfigs(deletable);
  }, [activePath, dirty]);

  const confirmDeleteConfigs = useCallback(async () => {
    const targets = pendingDeleteConfirmConfigs;
    if (targets.length === 0 || deleting) return;
    setDeleting(true);
    setDeleteError("");
    try {
      await invokeCommand<AgentConfigFile[]>(TauriCommand.AgentConfigsDeleteMany, {
        paths: targets.map((config) => config.path),
      });
      setPendingDeleteConfirmConfigs([]);
      setSelected([]);
      const nextProfiles = { ...activeProfiles };
      for (const config of targets) {
        if (config.profile && nextProfiles[config.agent] === config.profile) delete nextProfiles[config.agent];
      }
      updateActiveProfiles(nextProfiles);
      await loadConfigs();
    } catch (error) {
      setDeleteError(errorMessage(error));
    } finally {
      setDeleting(false);
    }
  }, [activeProfiles, deleting, loadConfigs, pendingDeleteConfirmConfigs, updateActiveProfiles]);

  const rowContextMenu = useCallback((config: AgentConfigFile, { selectedRows, selected: isSelected }: { selectedRows: AgentConfigFile[]; selected: boolean }) => {
    const actionRows = isSelected ? selectedRows : [config];
    const actions = configSelectionActions(actionRows, ContextMenu, requestDeleteConfigs, deleting);
    return actions.length > 0 ? renderDataTableSelectionMenu(actions) : null;
  }, [deleting, requestDeleteConfigs]);

  const bottomBar = useCallback((selectedRows: AgentConfigFile[]) => (
    <DataTableSelectionActions actions={configSelectionActions(selectedRows, DropdownMenu, requestDeleteConfigs, deleting)} ariaLabel="More selected config actions" />
  ), [deleting, requestDeleteConfigs]);

  const activateProfile = useCallback(async (agent: string, value: string) => {
    if (profileSwitching) return;
    const profile = value === BASE_PROFILE_VALUE ? null : value.replace(/^profile:/, "");
    const target = configs.find((config) => (
      config.agent === agent && (profile ? config.profile === profile : !config.profile)
    ));
    if (!target) {
      logger.error("config profile not found", { action: "activate_profile" });
      return;
    }
    if (dirty) {
      return;
    }
    setProfileSwitching(true);
    try {
      const next = await invokeCommand<ConfigProfileCommandResult>(TauriCommand.ConfigProfileSet, { agent, profile });
      updateActiveProfiles(next.configProfiles);
      if (target.path !== activePath) await readConfig(target.path, target);
    } catch (error) {
      logger.error("failed to activate config profile", { error: errorMessage(error), agent });
    } finally {
      setProfileSwitching(false);
    }
  }, [activePath, configs, dirty, profileSwitching, readConfig, updateActiveProfiles]);

  const openProfileDialog = () => {
    if (!profileAgent) return;
    setProfileName("");
    setProfileError("");
    setProfileDialogOpen(true);
  };

  const createProfile = useCallback(async () => {
    if (!profileAgent) return;
    const name = profileName.trim();
    if (!name) {
      setProfileError("Enter a profile name");
      return;
    }
    setProfileSaving(true);
    setProfileError("");
    try {
      await invokeCommand<AgentConfigContent>(TauriCommand.ConfigProfileCreate, {
        agent: profileAgent,
        name,
        content,
      });
      const next = await invokeCommand<AgentConfigFile[]>(TauriCommand.AgentConfigsList);
      setConfigs(next);
      setSelectedProfileValue(`profile:${name}`);
      setProfileDialogOpen(false);
      setProfileName("");
    } catch (error) {
      setProfileError(errorMessage(error));
    } finally {
      setProfileSaving(false);
    }
  }, [content, profileAgent, profileName]);

  const saveContent = useCallback(async (path: string, nextContent: string, expectedSha256: string) => {
    if (saving) return false;
    setSaving(true);
    setSaveError("");
    try {
      const saved = await invokeCommand<AgentConfigContent>(TauriCommand.AgentConfigSave, {
        path,
        expectedSha256,
        content: nextContent,
      });
      setContent(saved.content);
      setOriginalContent(saved.content);
      setSha256(saved.sha256);
      setConflict(null);
      setConfigs((current) => current.map((config) => (
        config.path === saved.path ? { ...config, exists: true, updatedAt: saved.updatedAt } : config
      )));
      return true;
    } catch (error) {
      logger.error("failed to save config", { error: errorMessage(error), path });
      if (error instanceof DaemonCommandError && error.code === "CONFLICT") {
        if (isConfigSnapshot(error.data)) applyExternalSnapshot(error.data);
        else await readExternalSnapshot(path);
        setSaveError("Source file changed on disk. Review the conflict before saving.");
      } else {
        setSaveError(errorMessage(error));
      }
      return false;
    } finally {
      setSaving(false);
    }
  }, [applyExternalSnapshot, readExternalSnapshot, saving]);

  const save = useCallback(async () => {
    if (!activeConfig || !dirty || saving || conflict || isConflictMarkerContent(content)) return;
    await saveContent(activeConfig.path, content, sha256);
  }, [activeConfig, conflict, content, dirty, saveContent, saving, sha256]);

  const useDiskVersion = useCallback(() => {
    if (!conflict) return;
    setContent(conflict.disk);
    setOriginalContent(conflict.disk);
    setSha256(conflict.diskSha256);
    setConflict(null);
    setSaveError("");
  }, [conflict]);

  const mergeConflict = useCallback(() => {
    if (!conflict) return;
    const merged = mergeThreeWay(conflict.base, content, conflict.disk);
    setContent(merged.content);
    setOriginalContent(conflict.disk);
    setSha256(conflict.diskSha256);
    setSaveError("");
    setConflict(merged.hasConflicts ? { ...conflict, local: merged.content, merged: true } : null);
  }, [conflict, content]);

  const overwriteDisk = useCallback(() => {
    if (!activeConfig || !conflict || saving) return;
    void saveContent(activeConfig.path, content, conflict.diskSha256);
  }, [activeConfig, conflict, content, saveContent, saving]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "s") return;
      event.preventDefault();
      void save();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [save]);

  return (
    <PanelGroup className="sessionsLayout configLayout" orientation="horizontal">
      <DiscardChangesDialog
        open={showDiscardDialog}
        onOpenChange={(open) => {
          setShowDiscardDialog(open);
          if (!open) {
            setPendingDeleteConfigs([]);
            setPendingPath("");
            setPendingReload(false);
          }
        }}
        onDiscard={discardPendingChanges}
      />
      <DeleteConfirmationDialog
        open={pendingDeleteConfirmConfigs.length > 0}
        items={pendingDeleteConfirmConfigs.map((config) => formatUserPath(config.path))}
        itemLabel="config file"
        busy={deleting}
        onOpenChange={(open) => { if (!open) setPendingDeleteConfirmConfigs([]); }}
        onConfirm={() => { void confirmDeleteConfigs(); }}
      />
      <DialogShell
        open={profileDialogOpen}
        onOpenChange={(open) => {
          if (!profileSaving) setProfileDialogOpen(open);
        }}
        className="confirmDialogPanel configProfileDialog"
        descriptionId="config-profile-description"
      >
        <Dialog.Title className="confirmDialogTitle">Create config profile</Dialog.Title>
        <p id="config-profile-description" className="confirmDialogDescription">
          Save the current {profileAgent ? friendlyAgent(profileAgent) : "agent"} {activeConfig?.format.toUpperCase() ?? "config"} as a named profile.
        </p>
        <div className="configProfileDialogBody">
          <DialogTextField
            label="Profile name"
            value={profileName}
            onChange={setProfileName}
            placeholder="deep-review"
          />
          <p className="configProfileHint">Use letters, numbers, hyphens, or underscores.</p>
          {profileError ? <Toast tone="error" message={profileError} /> : null}
        </div>
        <DialogActionBar cancelDisabled={profileSaving} onCancel={() => setProfileDialogOpen(false)}>
          <DialogStatefulButton
            state={profileSaving ? "loading" : "idle"}
            loadingLabel="Creating config profile"
            loadingContent={<LoadingInline size={16} gap={6} label="Create" />}
            variant="primary"
            className="dialogAdvanceButton"
            aria-label="Create config profile"
            disabled={!profileName.trim()}
            onClick={() => { void createProfile(); }}
          >
            Create
          </DialogStatefulButton>
        </DialogActionBar>
      </DialogShell>
      <Panel className="sessionListPanel configListPanel" defaultSize="36%" minSize="280px">
        <div className="sessionListPane configListPane">
          <PageHeader title="Config" compact>
            <IconButton
              aria-label="Reload config"
              aria-busy={loadingConfigs}
              disabled={loadingConfigs}
              onClick={reload}
            >
              {loadingConfigs ? <LoadingIcon size={14} /> : <RefreshCw size={14} />}
            </IconButton>
          </PageHeader>
          {deleteError ? <Toast tone="error" message={deleteError} onDismiss={() => setDeleteError("")} /> : null}
          <div className="sessionListBody">
            <DataTable
              rows={configs}
              columns={columns}
              getRowId={(config) => config.path}
              getRowLabel={(config) => config.label}
              selectable
              selectedIds={selected}
              onSelectionChange={setSelected}
              enableMarquee
              onRowClick={(config) => chooseConfig(config.path)}
              rowProps={(config) => ({
                className: config.path === selectedPath ? "configRowActive" : "",
                "aria-current": config.path === selectedPath ? "true" : undefined,
              })}
              rowContextMenu={rowContextMenu}
              bottomBar={bottomBar}
              bottomBarActionsClassName="selectionActions"
              bottomBarCheckboxLabel="Select visible configs from toolbar"
              selectionLabel="configs"
              loading={loadingConfigs && configs.length === 0}
              loadingLabel="Loading configs"
              emptyState={loadError ? <LoadErrorState message={loadError} onRetry={() => { void loadConfigs(); }} /> : (
                <EmptyState
                  icon={<Files size={27} strokeWidth={1.55} />}
                  title="No supported agent configs found"
                  description="Open a supported agent, then refresh."
                />
              )}
            />
          </div>
        </div>
      </Panel>
      <ResizeSeparator />
      <Panel className="configEditorPanelHost" defaultSize="64%" minSize="420px">
        <aside className="ruleEditorPanel configEditorPanel">
          <header className="threadHeader configEditorHeader">
            <div className="threadTitleLine">
              <div className="configTitle">
                <h2>{activeConfig?.label ?? "Config file"}</h2>
                {!loadingConfig && activeConfig && dirty ? <Badge tone="warning">modified</Badge> : null}
              </div>
            </div>
            {conflict ? (
              <div className="configConflictActions" role="group" aria-label="Resolve config conflict">
                <Button variant="secondary" onClick={useDiskVersion} disabled={saving}>Use disk</Button>
                <Button variant="danger" onClick={overwriteDisk} disabled={saving}>Overwrite</Button>
                {!conflict.merged ? <Button variant="primary" onClick={mergeConflict} disabled={saving}>Merge</Button> : null}
              </div>
            ) : null}
            {profileAgent ? (
              <div className="configProfileFloating">
                <SelectControl
                  contentClassName="configProfileSelectContent"
                  label={`Active ${friendlyAgent(profileAgent)} profile`}
                  value={resolvedSelectedProfileValue}
                  onValueChange={setSelectedProfileValue}
                  options={profileOptions}
                  indicatorPosition="left"
                  menuAction={{
                    label: "Create profile",
                    icon: <Plus size={14} aria-hidden="true" />,
                    onSelect: openProfileDialog,
                  }}
                />
                <Tooltip content={dirty ? "Save or discard current changes before switching a profile" : undefined}><StatefulButton
                  state={profileSwitching ? "loading" : profileSelectionChanged ? "idle" : "success"}
                  variant="primary"
                  size="sm"
                  width={80}
                  minWidth={80}
                  successLabel="Active"
                  loadingContent={<LoadingInline size={14} gap={6} label="Switch" />}
                  aria-label={dirty ? "Save or discard changes before switching a config profile" : profileSwitching ? "Switching config profile" : profileSelectionChanged ? "Switch selected config profile" : "Active config profile"}
                  disabled={!profileSelectionChanged || dirty || profileSwitching || profileSaving}
                  onClick={() => { void activateProfile(profileAgent, resolvedSelectedProfileValue); }}
                >
                  <><ArrowRightLeft size={14} /><span>Switch</span></>
                </StatefulButton></Tooltip>
              </div>
            ) : null}
          </header>
          {contentError ? (
            <div className="configEditorMessage"><LoadErrorState message={contentError} onRetry={() => { if (selectedPath) void readConfig(selectedPath, configs.find((config) => config.path === selectedPath)); }} /></div>
          ) : activeConfig ? (
            <div className="configEditorContent">
              <Suspense fallback={<div className="configEditorMessage"><LoadingState label="Loading editor" /></div>}>
                <MarkdownFilePane
                  activePath={activeConfig.path}
                  dirty={dirty}
                  content={content}
                  originalContent={originalContent}
                  language={activeConfig.format}
                  onChange={(value) => {
                    setContent(value);
                    setSaveError("");
                    if (conflict?.merged) {
                      if (isConflictMarkerContent(value)) setConflict((current) => current ? { ...current, local: value } : current);
                      else setConflict(null);
                    }
                  }}
                  onSave={() => { void save(); }}
                  showConflictMarkers={Boolean(conflict?.merged) || isConflictMarkerContent(content)}
                  onConflictResolve={(value) => {
                    setContent(value);
                    setSaveError("");
                    if (isConflictMarkerContent(value)) setConflict((current) => current ? { ...current, local: value } : current);
                    else setConflict(null);
                  }}
                  saveDisabled={Boolean(conflict) || isConflictMarkerContent(content)}
                  copyablePath
                  showDirtyIndicator={false}
                  showTokenStatusBar={false}
                  saveState={editorSaveError ? "error" : "idle"}
                  saveError={editorSaveError}
                />
              </Suspense>
              {loadingConfig ? <div className="configEditorLoading"><LoadingState label="Loading config" /></div> : null}
            </div>
          ) : loadingConfig ? (
            <div className="configEditorMessage"><LoadingState label="Loading config" /></div>
          ) : (
            <div className="configEditorMessage">Select a config file</div>
          )}
        </aside>
      </Panel>
    </PanelGroup>
  );
}
