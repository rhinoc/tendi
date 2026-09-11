import { useCallback, useEffect, useState } from "react";
import { FolderOpen, GitCommitHorizontal, Settings2 } from "lucide-react";
import { Dialog } from "radix-ui";

import { CollapsibleAccordion, type CollapsibleAccordionItem } from "../../components/shared/CollapsibleAccordion.tsx";
import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { SelectionCheckbox } from "../../components/shared/SelectionCheckbox.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton, type StatefulButtonState } from "../../components/shared/StatefulButton.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { BackupConfigurationState, backupConfigurationState } from "../../lib/backup-state.ts";
import { compactCommand, formatRelativeTime, formatUserPath, isVisibleAgent, revealPathLabel, safeInvoke, TauriCommand, type RawSkillRecord } from "../../lib/index.ts";
import { AsyncStatus } from "../../lib/async-status.ts";
import {
  configureSkillBackup,
  disconnectSkillBackup,
  readSkillBackup,
  readSkillTargets,
  restoreSkillBackup,
  runSkillBackup,
  SkillScope,
} from "../../lib/runtime-gateway.ts";
import type {
  BackupContents,
  BackupRestoreOperation,
  BackupStatusResponse,
  SkillTargetResponse,
} from "../../lib/runtime-gateway.ts";
import { resolveSelectValue } from "../../lib/select-options.ts";
import "./BackupView.css";

type BackupCategorySelection = BackupContents[BackupCategory];

type BackupCategory = keyof BackupContents;
type BackupCatalogItem = { id: string; label: string; detail: string };

type BackupVersion = {
  id: string;
  createdAt: number;
  summary: string;
};

const BACKUP_CATEGORY_ACCORDION_RADIUS = 12;

type BackupTarget = SkillTargetResponse;

type BackupRestorePlan = {
  revision: string;
  targetRoot: string;
  operations: BackupRestoreOperation[];
};

enum RestoreResolution {
  Skip = "skip",
  Replace = "replace",
  KeepBoth = "keep-both",
}
enum BackupAction {
  Configure = "configure",
  Backup = "backup",
  Disconnect = "disconnect",
}
enum BackupRestoreOperationStatus {
  Conflict = "conflict",
  Planned = "planned",
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : `${error}`;
}

function BackupHistoryList({ versions, now, onRestore }: { versions: BackupVersion[]; now: number; onRestore: (version: BackupVersion) => void }) {
  return versions.length ? <ul className="settingsBackupVersionList">
    {versions.map((version) => (
      <li className="backupHistoryItem" key={version.id}>
        <div className="backupHistoryItemCard">
          <div className="backupHistoryVersionRow">
            <div className="settingsBackupVersionCopy">
              <div className="settingsBackupVersionTitle">
                <GitCommitHorizontal size={14} aria-hidden="true" />
                <strong>{version.summary}</strong>
              </div>
              <span>{formatRelativeTime(version.createdAt * 1000, now) || "Unknown"} · <code>{version.id.slice(0, 12)}</code></span>
            </div>
            <DialogActionButton variant="secondary" onClick={() => onRestore(version)}>Restore</DialogActionButton>
          </div>
        </div>
      </li>
    ))}
  </ul> : <p className="settingsBackupEmpty">No sync versions yet.</p>;
}

function backupCatalogSubtitle(category: BackupCategory, item: BackupCatalogItem) {
  if (category === "mcp") return "";
  if (!item.detail.trim()) return "";
  if (category === "rules") return formatUserPath(item.detail);
  if (category === "hooks") return compactCommand(item.detail);
  return item.detail;
}

function defaultBackupContents(): BackupContents {
  return {
    skills: { enabled: true, excluded: [] },
    mcp: { enabled: true, excluded: [] },
    rules: { enabled: true, excluded: [] },
    hooks: { enabled: true, excluded: [] },
  };
}

const backupCategoryDefinitions: Array<{ key: BackupCategory; label: string }> = [
  { key: "skills", label: "Skills" },
  { key: "mcp", label: "MCP" },
  { key: "rules", label: "Rules" },
  { key: "hooks", label: "Hooks" },
];

function BackupCategoryAccordion({
  catalog,
  contents,
  onToggleCategory,
  onToggleCategoryItem,
  selectedCategoryCount,
}: {
  catalog: BackupStatusResponse["catalog"] | null;
  contents: BackupContents;
  onToggleCategory: (category: BackupCategory, enabled: boolean) => void;
  onToggleCategoryItem: (category: BackupCategory, id: string, selected: boolean) => void;
  selectedCategoryCount: (category: BackupCategory) => number;
}) {
  const items: CollapsibleAccordionItem[] = backupCategoryDefinitions.map((category) => {
    const selection = contents[category.key];
    const itemCount = catalog?.[category.key].length ?? 0;
    const selectedCount = selectedCategoryCount(category.key);
    const mixed = selection.enabled && itemCount > 0 && selectedCount > 0 && selectedCount < itemCount;
    const checked = selection.enabled && !mixed;

    return {
      id: category.key,
      title: (
        <span className="backupCategoryAccordionTitle">
          <strong>{category.label}</strong>
          <span>{!catalog ? "Unavailable" : selection.enabled ? (itemCount ? `${selectedCount}/${itemCount}` : "No items") : "Not included"}</span>
        </span>
      ),
      leading: <SelectionCheckbox checked={checked} mixed={mixed} label={`Include ${category.label}`} onChange={(nextChecked) => onToggleCategory(category.key, nextChecked)} />,
      content: !catalog ? <p className="settingsBackupEmpty">Sync contents are unavailable. Refresh and try again.</p> : catalog[category.key].length ? (
        <div className="backupContentItems">
          {catalog[category.key].map((item) => {
            const selected = selection.enabled && !selection.excluded.includes(item.id);
            const subtitle = backupCatalogSubtitle(category.key, item);
            return (
              <label className="backupContentItemRow" key={item.id}>
                <SelectionCheckbox
                  checked={selected}
                  disabled={!selection.enabled}
                  label={`Include ${item.label}`}
                  onChange={(checked) => onToggleCategoryItem(category.key, item.id, checked)}
                />
                <div>
                  <strong className="dataCellTitle">{item.label}</strong>
                  {subtitle ? <span className="dataCellSubLine"><span className="dataCellSub">{subtitle}</span></span> : null}
                </div>
              </label>
            );
          })}
        </div>
      ) : <p className="settingsBackupEmpty">No items found.</p>,
    };
  });

  return <CollapsibleAccordion
    className="backupCategoryAccordion"
    cornerRadius={BACKUP_CATEGORY_ACCORDION_RADIUS}
    separateExpandedItems={false}
    items={items}
  />;
}

export function BackupSettings({
  onSkillsRestored,
}: {
  onSkillsRestored?: (skills: RawSkillRecord[], options?: { patch?: boolean }) => void;
} = {}) {
  const [data, setData] = useState<BackupStatusResponse | null>(null);
  const [repository, setRepository] = useState("");
  const [contents, setContents] = useState<BackupContents>(defaultBackupContents);
  const [loadError, setLoadError] = useState("");
  const [actionError, setActionError] = useState("");
  const [action, setAction] = useState<BackupAction | null>(null);
  const [targets, setTargets] = useState<BackupTarget[]>([]);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restoreVersion, setRestoreVersion] = useState<BackupVersion | null>(null);
  const [restoreTarget, setRestoreTarget] = useState("shared");
  const [restoreScope, setRestoreScope] = useState<SkillScope>(SkillScope.Global);
  const [restorePlan, setRestorePlan] = useState<BackupRestorePlan | null>(null);
  const [restoreSelectedIds, setRestoreSelectedIds] = useState<string[]>([]);
  const [restoreResolutions, setRestoreResolutions] = useState<Record<string, RestoreResolution>>({});
  const [restoreBusy, setRestoreBusy] = useState(false);
  const [relativeTimeNow, setRelativeTimeNow] = useState(() => Date.now());

  useEffect(() => {
    const timer = window.setInterval(() => setRelativeTimeNow(Date.now()), 30_000);
    return () => window.clearInterval(timer);
  }, []);
  const visibleTargets = targets.filter((target) => isVisibleAgent(target.id));
  const restoreTargetOptions = visibleTargets.map((target) => ({ value: target.id, label: target.displayName }));
  const resolvedRestoreTarget = resolveSelectValue(restoreTarget, restoreTargetOptions);
  const backupConfig = data?.config;

  const load = useCallback(async () => {
    try {
      const [next, targetOptions] = await Promise.all([
        readSkillBackup(),
        readSkillTargets(),
      ]);
      if (!targetOptions) throw new Error("Unable to read skill targets");
      setData(next);
      setLoadError("");
      if (next.config) {
        setRepository(next.config.remoteUrl || next.config.checkoutPath);
        setContents(next.config.contents);
      } else {
        setRepository("");
        setContents(defaultBackupContents());
      }
      setTargets(targetOptions);
      setRestoreTarget((current) => resolveSelectValue(
        current,
        targetOptions
          .filter((target) => isVisibleAgent(target.id))
          .map((target) => ({ value: target.id, label: target.displayName })),
      ));
    } catch (loadError) {
      setLoadError(errorMessage(loadError));
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const configure = async () => {
    if (!repository.trim()) return;
    setAction(BackupAction.Configure);
    setActionError("");
    try {
      await configureSkillBackup({
        repository: repository.trim(),
        checkoutPath: backupConfig?.remoteUrl ? backupConfig.checkoutPath : "",
        contents,
      });
      await load();
      setDetailsOpen(false);
    } catch (configureError) {
      setActionError(errorMessage(configureError));
    } finally {
      setAction(null);
    }
  };

  const backupNow = async () => {
    setAction(BackupAction.Backup);
    setActionError("");
    try {
      await runSkillBackup();
      await load();
    } catch (backupError) {
      setActionError(errorMessage(backupError));
    } finally {
      setAction(null);
    }
  };

  const disconnect = async () => {
    setAction(BackupAction.Disconnect);
    setActionError("");
    try {
      await disconnectSkillBackup();
      await load();
    } catch (disconnectError) {
      setActionError(errorMessage(disconnectError));
    } finally {
      setAction(null);
    }
  };

  const previewRestore = useCallback(async (version: BackupVersion, target = resolvedRestoreTarget, scope = restoreScope, resetSelection = false) => {
    setRestoreBusy(true);
    setActionError("");
    try {
      const plan = await restoreSkillBackup({
        revision: version.id,
        skillIds: [],
        target,
        scope,
        dryRun: true,
      });
      if (!plan.revision || !plan.targetRoot || !plan.operations) throw new Error("Invalid restore plan response");
      const nextPlan: BackupRestorePlan = { revision: plan.revision, targetRoot: plan.targetRoot, operations: plan.operations };
      setRestorePlan(nextPlan);
      if (resetSelection) setRestoreSelectedIds(nextPlan.operations.map((operation) => operation.id));
    } catch (previewError) {
      setRestorePlan(null);
      setActionError(errorMessage(previewError));
    } finally {
      setRestoreBusy(false);
    }
  }, [resolvedRestoreTarget, restoreScope]);

  const openRestore = (version: BackupVersion) => {
    setRestoreVersion(version);
    setRestorePlan(null);
    setRestoreSelectedIds([]);
    setRestoreResolutions({});
    setActionError("");
    setRestoreOpen(true);
  };

  const prepareRestore = () => {
    if (restoreVersion && !restoreBusy) void previewRestore(restoreVersion, resolvedRestoreTarget, restoreScope, true);
  };

  const changeRestoreTarget = (nextTarget: string) => {
    setRestoreTarget(nextTarget);
    setRestoreResolutions({});
    if (restoreVersion) void previewRestore(restoreVersion, nextTarget, restoreScope);
  };

  const changeRestoreScope = (nextScope: SkillScope) => {
    setRestoreScope(nextScope);
    setRestoreResolutions({});
    if (restoreVersion) void previewRestore(restoreVersion, resolvedRestoreTarget, nextScope);
  };

  const applyRestore = async () => {
    if (!restoreVersion) return;
    setRestoreBusy(true);
    setActionError("");
    try {
      const result = await restoreSkillBackup({
        revision: restoreVersion.id,
        skillIds: restoreSelectedIds,
        target: resolvedRestoreTarget,
        scope: restoreScope,
        confirmed: true,
        resolutions: Object.entries(restoreResolutions).map(([id, action]) => ({ id, action })),
      });
      setRestoreOpen(false);
      const nextSkills = result.updated ?? result.skills;
      if (nextSkills) onSkillsRestored?.(nextSkills, { patch: true });
      await load();
    } catch (restoreError) {
      setActionError(errorMessage(restoreError));
    } finally {
      setRestoreBusy(false);
    }
  };

  const stateFor = (next: BackupAction): StatefulButtonState => action === next ? AsyncStatus.Loading : AsyncStatus.Idle;
  const configurationState = backupConfigurationState(data);
  const loading = configurationState === BackupConfigurationState.Loading;
  const configured = configurationState === BackupConfigurationState.Configured;
  const lastBackup = data?.versions[0] ?? null;
  const catalog = data?.catalog ?? null;
  const openDetails = () => {
    if (data?.config) {
      setRepository(data.config.remoteUrl || data.config.checkoutPath);
      setContents(data.config.contents);
    } else {
      setRepository("");
      setContents(defaultBackupContents());
    }
    setDetailsOpen(true);
  };
  const updateCategory = (category: BackupCategory, update: (current: BackupCategorySelection) => BackupCategorySelection) => {
    setContents((current) => ({ ...current, [category]: update(current[category]) }));
  };
  const toggleCategory = (category: BackupCategory, enabled: boolean) => {
    updateCategory(category, (current) => ({ ...current, enabled }));
  };
  const toggleCategoryItem = (category: BackupCategory, id: string, selected: boolean) => {
    updateCategory(category, (current) => ({
      ...current,
      excluded: selected
        ? current.excluded.filter((item) => item !== id)
        : [...new Set([...current.excluded, id])],
    }));
  };
  const selectedCategoryCount = (category: BackupCategory) => {
    if (!catalog) return 0;
    const selection = contents[category];
    return catalog[category].filter((item) => selection.enabled && !selection.excluded.includes(item.id)).length;
  };
  const syncContentsContent = data === null ? (
    loadError ? <p className="settingsBackupEmpty">Sync contents are unavailable. Refresh and try again.</p> : <LoadingState className="backupDetailsSectionLoading" label="Loading sync contents" />
  ) : (
    <BackupCategoryAccordion
      catalog={catalog}
      contents={contents}
      onToggleCategory={toggleCategory}
      onToggleCategoryItem={toggleCategoryItem}
      selectedCategoryCount={selectedCategoryCount}
    />
  );
  const hasUnresolvedRestoreConflict = Boolean(
    restorePlan?.operations.some((operation) => (
      restoreSelectedIds.includes(operation.id)
      && operation.status === BackupRestoreOperationStatus.Conflict
      && !restoreResolutions[operation.id]
    )),
  );
  const toggleRestoreSkill = (id: string, checked: boolean) => {
    setRestoreSelectedIds((current) => checked ? [...new Set([...current, id])] : current.filter((item) => item !== id));
    if (!checked) {
      setRestoreResolutions((current) => {
        const next = { ...current };
        delete next[id];
        return next;
      });
    }
  };

  return (
    <div className="settingsBackup">
      {loadError ? <LoadErrorState message={loadError} onRetry={() => { void load(); }} /> : null}
      {actionError ? <Toast tone="error" message={actionError} onDismiss={() => setActionError("")} /> : null}
      <div className="settingsBackupSummary">
        <div className={`settingsBackupSummaryActions ${configured ? "isConfigured" : "isNotConfigured"}`}>
          <div className="settingsBackupActionStack">
            <div className="settingsBackupPrimaryActions">
              {configured || loading ? <StatefulButton size="sm" variant="primary" state={stateFor(BackupAction.Backup)} aria-label="Sync now" width={112} onClick={() => { void backupNow(); }}>Sync now</StatefulButton> : <span className="settingsBackupNotConfigured">Not configured</span>}
              <IconButton aria-label="Sync settings" onClick={openDetails}><Settings2 size={16} aria-hidden="true" /></IconButton>
            </div>
            {configured ? <span className="settingsBackupLastSync">Last sync: {lastBackup ? formatRelativeTime(lastBackup.createdAt * 1000, relativeTimeNow) || "Never" : "Never"}</span> : null}
          </div>
        </div>
      </div>
      <DialogShell open={detailsOpen} onOpenChange={setDetailsOpen} className="confirmDialogPanel backupDetailsDialog" descriptionId="backup-details-description">
        <Dialog.Title className="confirmDialogTitle">{loading || configured ? "Sync details" : "Set up sync"}</Dialog.Title>
        <Dialog.Description id="backup-details-description" className="dialogVisuallyHidden">Sync settings</Dialog.Description>
        <div className="backupDetailsBody backupDetailsHomeBody">
          <div className="backupDetailsSection backupDetailsAccordionSection">
            <CollapsibleAccordion
              key={detailsOpen ? "open" : "closed"}
              defaultValue="repository"
              items={[
                {
                  id: "repository",
                  title: "Repository",
                  content: <div className="settingsBackupFormGrid">
                    <label className="settingsBackupField">
                      <div className="settingsBackupRepositoryInput">
                        <input aria-label="Repository address" className="settingsTextInput" value={repository} onChange={(event) => setRepository(event.target.value)} placeholder="https://github.com/org/repo or ~/path/to/repo" />
                        {configured && backupConfig ? <IconButton
                          aria-label={revealPathLabel("sync repository")}
                          onClick={() => void safeInvoke(TauriCommand.RevealInFinder, { path: backupConfig.checkoutPath })}
                        >
                          <FolderOpen size={14} aria-hidden="true" />
                        </IconButton> : null}
                      </div>
                    </label>
                  </div>,
                },
                {
                  id: "contents",
                  title: "Sync contents",
                  content: syncContentsContent,
                },
                ...(configured ? [{
                  id: "history",
                  title: "History",
                  content: <BackupHistoryList versions={data?.versions ?? []} now={relativeTimeNow} onRestore={openRestore} />,
                }, {
                  id: "disconnect",
                  title: "Disconnect",
                  content: <div className="backupDisconnectContent">
                    <StatefulButton size="sm" variant="danger" state={stateFor(BackupAction.Disconnect)} aria-label="Disconnect this device from sync" width={104} onClick={() => { void disconnect(); }}>Disconnect</StatefulButton>
                  </div>,
                }] : []),
              ]}
            />
          </div>
        </div>
        <DialogActionBar
          onCancel={() => setDetailsOpen(false)}
          cancelDisabled={action !== null}
        >
          <StatefulButton variant="primary" state={stateFor(BackupAction.Configure)} aria-label={configured ? "Save sync settings" : "Set up sync"} width={configured ? 112 : 96} minWidth={configured ? 112 : 96} disabled={loading || !repository.trim()} onClick={() => { void configure(); }}>{configured ? "Save" : "Set up"}</StatefulButton>
        </DialogActionBar>
      </DialogShell>
      <DialogShell open={restoreOpen} onOpenChange={setRestoreOpen} className="confirmDialogPanel backupRestoreDialog" descriptionId="backup-restore-description">
        <Dialog.Title className="confirmDialogTitle">Restore sync</Dialog.Title>
        <Dialog.Description id="backup-restore-description" className="dialogVisuallyHidden">Restore a sync version</Dialog.Description>
        <div className="backupRestoreControls">
          <SelectControl label="Agent" value={resolvedRestoreTarget} onValueChange={changeRestoreTarget} options={restoreTargetOptions} />
          <SelectControl label="Scope" value={restoreScope} onValueChange={(value) => changeRestoreScope(value as SkillScope)} options={[{ value: SkillScope.Global, label: "Global" }, { value: SkillScope.Project, label: "Project" }]} />
        </div>
        {restoreBusy && !restorePlan ? <p className="backupDialogHint">Preparing restore plan…</p> : null}
        {!restorePlan && !restoreBusy ? <div className="backupRestoreEmpty"><DialogActionButton variant="secondary" onClick={prepareRestore}>Prepare restore plan</DialogActionButton></div> : null}
        {restorePlan ? <ul className="backupRestorePlan">{restorePlan.operations.map((operation) => <li key={operation.id} data-status={operation.status}><SelectionCheckbox checked={restoreSelectedIds.includes(operation.id)} label={`Restore ${operation.name}`} disabled={restoreBusy} onChange={(checked) => toggleRestoreSkill(operation.id, checked)} /><span>{operation.name}</span>{operation.status === BackupRestoreOperationStatus.Conflict && restoreSelectedIds.includes(operation.id) ? <SelectControl label={`${operation.name} conflict`} value={restoreResolutions[operation.id] ?? ""} onValueChange={(value) => setRestoreResolutions((current) => ({ ...current, [operation.id]: value as RestoreResolution }))} options={[{ value: RestoreResolution.KeepBoth, label: "Keep both" }, { value: RestoreResolution.Replace, label: "Replace existing" }, { value: RestoreResolution.Skip, label: "Keep existing" }]} renderValue={(option) => <span className="selectValueText">{option?.label ?? "Choose action"}</span>} /> : <span>{restoreSelectedIds.includes(operation.id) && operation.status === BackupRestoreOperationStatus.Planned ? operation.target : operation.message ?? "Not selected"}</span>}</li>)}</ul> : null}
        <DialogActionBar onCancel={() => setRestoreOpen(false)} cancelDisabled={restoreBusy}><StatefulButton variant="primary" state={restoreBusy ? AsyncStatus.Loading : AsyncStatus.Idle} aria-label="Restore sync" width={104} disabled={!restorePlan || restoreBusy || restoreSelectedIds.length === 0 || hasUnresolvedRestoreConflict} onClick={() => { void applyRestore(); }}>Restore</StatefulButton></DialogActionBar>
      </DialogShell>
    </div>
  );
}
