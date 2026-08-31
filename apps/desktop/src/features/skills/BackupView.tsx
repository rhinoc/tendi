import { useCallback, useEffect, useState } from "react";
import { ChevronRight, FolderOpen, Settings2 } from "lucide-react";
import { Dialog } from "radix-ui";

import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { LoadingInline } from "../../components/shared/LoadingInline.tsx";
import { SelectionCheckbox } from "../../components/shared/SelectionCheckbox.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton, type StatefulButtonState } from "../../components/shared/StatefulButton.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { BackupDialogLeadingAction, backupDialogLeadingAction } from "../../lib/backup-dialog.ts";
import { BackupConfigurationState, backupConfigurationState } from "../../lib/backup-state.ts";
import { compactCommand, formatUserPath, revealPathLabel, safeInvoke, TauriCommand, type RawSkillRecord } from "../../lib/index.ts";
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

function backupDate(createdAt: number) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(createdAt * 1000));
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
  const [contentCategory, setContentCategory] = useState<BackupCategory | null>(null);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restoreVersion, setRestoreVersion] = useState<BackupVersion | null>(null);
  const [restoreTarget, setRestoreTarget] = useState("shared");
  const [restoreScope, setRestoreScope] = useState<SkillScope>(SkillScope.Global);
  const [restorePlan, setRestorePlan] = useState<BackupRestorePlan | null>(null);
  const [restoreSelectedIds, setRestoreSelectedIds] = useState<string[]>([]);
  const [restoreResolutions, setRestoreResolutions] = useState<Record<string, RestoreResolution>>({});
  const [restoreBusy, setRestoreBusy] = useState(false);
  const restoreTargetOptions = targets.map((target) => ({ value: target.id, label: target.displayName }));
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
        targetOptions.map((target) => ({ value: target.id, label: target.displayName })),
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
      setContentCategory(null);
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
  const activeCategory = backupCategoryDefinitions.find((category) => category.key === contentCategory) ?? null;
  const leadingAction = backupDialogLeadingAction(Boolean(activeCategory), configured);
  const openDetails = () => {
    if (loading) return;
    setContentCategory(null);
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
        <div className={`settingsBackupSummaryActions ${loading ? "isLoading" : configured ? "isConfigured" : "isNotConfigured"}`} aria-busy={loading}>
          <div className="settingsBackupActionStack">
            <div className="settingsBackupPrimaryActions">
              {loading ? <LoadingInline label="Loading sync" size={14} /> : configured ? <StatefulButton size="sm" variant="primary" state={stateFor(BackupAction.Backup)} aria-label="Sync now" width={112} onClick={() => { void backupNow(); }}>Sync now</StatefulButton> : <span className="settingsBackupNotConfigured">Not configured</span>}
              {!loading ? <IconButton aria-label="Sync settings" onClick={openDetails}><Settings2 size={16} aria-hidden="true" /></IconButton> : null}
            </div>
            {configured ? <span className="settingsBackupLastSync">Last sync: {lastBackup ? backupDate(lastBackup.createdAt) : "Never"}</span> : null}
          </div>
        </div>
      </div>
      <DialogShell open={detailsOpen} onOpenChange={setDetailsOpen} className="confirmDialogPanel backupDetailsDialog" descriptionId="backup-details-description">
        <Dialog.Title className="confirmDialogTitle">{activeCategory ? activeCategory.label : configured ? "Sync details" : "Set up sync"}</Dialog.Title>
        <Dialog.Description id="backup-details-description" className="dialogVisuallyHidden">Sync settings</Dialog.Description>
        <div className={`backupDetailsBody ${activeCategory ? "backupDetailsCategoryBody" : "backupDetailsHomeBody"}`}>
          {activeCategory ? (
            <>
              <div className="backupDetailsSection">
                {!catalog ? <p className="settingsBackupEmpty">Sync contents are unavailable. Refresh and try again.</p> : catalog[activeCategory.key].length ? (
                  <div className="backupContentItems">
                    {catalog[activeCategory.key].map((item) => {
                      const selected = contents[activeCategory.key].enabled && !contents[activeCategory.key].excluded.includes(item.id);
                      const subtitle = backupCatalogSubtitle(activeCategory.key, item);
                      return (
                        <label className="backupContentItemRow" key={item.id}>
                          <SelectionCheckbox
                            checked={selected}
                            disabled={!contents[activeCategory.key].enabled}
                            label={`Include ${item.label}`}
                            onChange={(checked) => toggleCategoryItem(activeCategory.key, item.id, checked)}
                          />
                          <div>
                            <strong className="dataCellTitle">{item.label}</strong>
                            {subtitle ? <span className="dataCellSubLine"><span className="dataCellSub">{subtitle}</span></span> : null}
                          </div>
                        </label>
                      );
                    })}
                  </div>
                ) : <p className="settingsBackupEmpty">No items found.</p>}
              </div>
            </>
          ) : (
            <>
              <div className="backupDetailsSection">
                <h3>Repository</h3>
                <div className="settingsBackupFormGrid">
                  <label className="settingsBackupField">
                    <span>Repository address</span>
                    <div className="settingsBackupRepositoryInput">
                      <input className="settingsTextInput" value={repository} onChange={(event) => setRepository(event.target.value)} placeholder="https://github.com/org/repo or ~/path/to/repo" />
                      {configured && backupConfig ? <IconButton
                        aria-label={revealPathLabel("sync repository")}
                        onClick={() => void safeInvoke(TauriCommand.RevealInFinder, { path: backupConfig.checkoutPath })}
                      >
                        <FolderOpen size={14} aria-hidden="true" />
                      </IconButton> : null}
                    </div>
                  </label>
                </div>
              </div>
              <div className="backupDetailsSection">
                <h3>Sync contents</h3>
                <div className="backupCategoryList">
                  {backupCategoryDefinitions.map((category) => {
                    const selection = contents[category.key];
                    const itemCount = catalog?.[category.key].length ?? 0;
                    const selectedCount = selectedCategoryCount(category.key);
                    return (
                      <div className="backupCategoryRow" key={category.key}>
                        <SelectionCheckbox checked={selection.enabled} label={`Include ${category.label}`} onChange={(checked) => toggleCategory(category.key, checked)} />
                        <button type="button" className="backupCategoryDetail" onClick={() => setContentCategory(category.key)}>
                          <span>
                            <strong>{category.label}</strong>
                            <span>{!catalog ? "Unavailable" : selection.enabled ? (itemCount ? `${selectedCount} of ${itemCount} included` : "No items") : "Not included"}</span>
                          </span>
                          <ChevronRight size={15} aria-hidden="true" />
                        </button>
                      </div>
                    );
                  })}
                </div>
              </div>
              {configured ? <div className="backupDetailsSection backupHistorySection">
                <h3>History</h3>
                {data?.versions.length ? <ul className="settingsBackupVersionList">{data.versions.map((version) => <li key={version.id}><div className="settingsBackupVersionCopy"><strong>{version.summary}</strong><span>{backupDate(version.createdAt)} · <code>{version.id.slice(0, 12)}</code></span></div><DialogActionButton variant="secondary" onClick={() => openRestore(version)}>Restore</DialogActionButton></li>)}</ul> : <p className="settingsBackupEmpty">No sync versions yet.</p>}
              </div> : null}
            </>
          )}
        </div>
        <DialogActionBar
          onCancel={() => { setContentCategory(null); setDetailsOpen(false); }}
          cancelDisabled={action !== null}
          leading={leadingAction === BackupDialogLeadingAction.Back ? <DialogActionButton variant="secondary" disabled={action !== null} onClick={() => setContentCategory(null)}>Back</DialogActionButton> : leadingAction === BackupDialogLeadingAction.Disconnect ? <StatefulButton size="sm" variant="danger" state={stateFor(BackupAction.Disconnect)} aria-label="Disconnect this device from sync" width={104} onClick={() => { void disconnect(); }}>Disconnect</StatefulButton> : undefined}
        >
          <StatefulButton variant="primary" state={stateFor(BackupAction.Configure)} aria-label={configured ? "Save sync settings" : "Set up sync"} width={configured ? 112 : 96} disabled={!repository.trim()} onClick={() => { void configure(); }}>{configured ? "Save" : "Set up"}</StatefulButton>
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
