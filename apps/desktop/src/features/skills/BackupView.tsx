import { useCallback, useEffect, useState } from "react";
import { ChevronLeft, ChevronRight, Settings2 } from "lucide-react";
import { Dialog } from "radix-ui";

import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { IconButton } from "../../components/shared/IconButton.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { SelectionCheckbox } from "../../components/shared/SelectionCheckbox.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton, type StatefulButtonState } from "../../components/shared/StatefulButton.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { TauriCommand, invokeCommand } from "../../lib/index.ts";
import { resolveSelectValue } from "../../lib/select-options.ts";
import "./BackupView.css";

type BackupConfig = {
  remoteUrl: string;
  checkoutPath: string;
  deviceLabel: string;
  contents: BackupContents;
};

type BackupCategorySelection = { enabled: boolean; excluded: string[] };
type BackupContents = {
  skills: BackupCategorySelection;
  mcp: BackupCategorySelection;
  rules: BackupCategorySelection;
  hooks: BackupCategorySelection;
};

type BackupCategory = keyof BackupContents;
type BackupCatalogItem = { id: string; label: string; detail: string };
type BackupCatalog = Record<BackupCategory, BackupCatalogItem[]>;

type BackupStatus = {
  skillPath: string;
  state: string;
  reason?: string | null;
};

type BackupVersion = {
  id: string;
  createdAt: number;
  summary: string;
};

type BackupStatusResponse = {
  config: BackupConfig | null;
  statuses: BackupStatus[];
  versions: BackupVersion[];
  catalog: BackupCatalog;
};

type BackupTarget = { id: string; displayName: string; supportsGlobal: boolean };

type BackupRestorePlan = {
  revision: string;
  targetRoot: string;
  operations: Array<{ id: string; name: string; category: string; target: string; status: string; message?: string | null }>;
};

type RestoreResolution = "skip" | "replace" | "keep-both";

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : `${error}`;
}

function backupDate(createdAt: number) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(createdAt * 1000));
}

function defaultBackupContents(): BackupContents {
  return {
    skills: { enabled: true, excluded: [] },
    mcp: { enabled: true, excluded: [] },
    rules: { enabled: true, excluded: [] },
    hooks: { enabled: true, excluded: [] },
  };
}

const backupCategoryDefinitions: Array<{ key: BackupCategory; label: string; detail: string }> = [
  { key: "skills", label: "Skills", detail: "Global managed skills" },
  { key: "mcp", label: "MCP", detail: "Global MCP configuration" },
  { key: "rules", label: "Rules", detail: "Global rules" },
  { key: "hooks", label: "Hooks", detail: "Global hooks" },
];

export function BackupSettings() {
  const [data, setData] = useState<BackupStatusResponse | null>(null);
  const [remoteUrl, setRemoteUrl] = useState("");
  const [deviceLabel, setDeviceLabel] = useState("My device");
  const [contents, setContents] = useState<BackupContents>(defaultBackupContents);
  const [loadError, setLoadError] = useState("");
  const [actionError, setActionError] = useState("");
  const [action, setAction] = useState<"configure" | "backup" | "disconnect" | "">("");
  const [targets, setTargets] = useState<BackupTarget[]>([]);
  const [detailsOpen, setDetailsOpen] = useState(false);
  const [contentCategory, setContentCategory] = useState<BackupCategory | null>(null);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restoreVersion, setRestoreVersion] = useState<BackupVersion | null>(null);
  const [restoreTarget, setRestoreTarget] = useState("shared");
  const [restoreScope, setRestoreScope] = useState<"global" | "project">("global");
  const [restorePlan, setRestorePlan] = useState<BackupRestorePlan | null>(null);
  const [restoreSelectedIds, setRestoreSelectedIds] = useState<string[]>([]);
  const [restoreResolutions, setRestoreResolutions] = useState<Record<string, RestoreResolution>>({});
  const [restoreBusy, setRestoreBusy] = useState(false);
  const restoreTargetOptions = targets.map((target) => ({ value: target.id, label: target.displayName }));
  const resolvedRestoreTarget = resolveSelectValue(restoreTarget, restoreTargetOptions);

  const load = useCallback(async () => {
    try {
      const [next, targetOptions] = await Promise.all([
        invokeCommand<BackupStatusResponse>(TauriCommand.SkillsBackupStatus),
        invokeCommand<BackupTarget[]>(TauriCommand.SkillsTargets),
      ]);
      setData(next);
      setLoadError("");
      if (next.config) {
        setRemoteUrl(next.config.remoteUrl);
        setDeviceLabel(next.config.deviceLabel);
        setContents(next.config.contents);
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
    if (!remoteUrl.trim() || !deviceLabel.trim()) return;
    setAction("configure");
    setActionError("");
    try {
      await invokeCommand(TauriCommand.SkillsBackupConfigure, { remoteUrl, deviceLabel, contents });
      await load();
      setContentCategory(null);
      setDetailsOpen(false);
    } catch (configureError) {
      setActionError(errorMessage(configureError));
    } finally {
      setAction("");
    }
  };

  const backupNow = async () => {
    setAction("backup");
    setActionError("");
    try {
      await invokeCommand(TauriCommand.SkillsBackupNow);
      await load();
    } catch (backupError) {
      setActionError(errorMessage(backupError));
    } finally {
      setAction("");
    }
  };

  const disconnect = async () => {
    setAction("disconnect");
    setActionError("");
    try {
      await invokeCommand(TauriCommand.SkillsBackupDisconnect);
      await load();
    } catch (disconnectError) {
      setActionError(errorMessage(disconnectError));
    } finally {
      setAction("");
    }
  };

  const previewRestore = useCallback(async (version: BackupVersion, target = resolvedRestoreTarget, scope = restoreScope, resetSelection = false) => {
    setRestoreBusy(true);
    setActionError("");
    try {
      const plan = await invokeCommand<BackupRestorePlan>(TauriCommand.SkillsBackupRestore, {
        revision: version.id,
        skillIds: [],
        target,
        scope,
        dryRun: true,
      });
      setRestorePlan(plan);
      if (resetSelection) setRestoreSelectedIds(plan.operations.map((operation) => operation.id));
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

  const changeRestoreScope = (nextScope: "global" | "project") => {
    setRestoreScope(nextScope);
    setRestoreResolutions({});
    if (restoreVersion) void previewRestore(restoreVersion, resolvedRestoreTarget, nextScope);
  };

  const applyRestore = async () => {
    if (!restoreVersion) return;
    setRestoreBusy(true);
    setActionError("");
    try {
      await invokeCommand(TauriCommand.SkillsBackupRestore, {
        revision: restoreVersion.id,
        skillIds: restoreSelectedIds,
        target: resolvedRestoreTarget,
        scope: restoreScope,
        confirmed: true,
        resolutions: Object.entries(restoreResolutions).map(([id, action]) => ({ id, action })),
      });
      setRestoreOpen(false);
      await load();
    } catch (restoreError) {
      setActionError(errorMessage(restoreError));
    } finally {
      setRestoreBusy(false);
    }
  };

  const stateFor = (next: typeof action): StatefulButtonState => action === next ? "loading" : "idle";
  const configured = Boolean(data?.config);
  const lastBackup = data?.versions[0] ?? null;
  const catalog = data?.catalog ?? null;
  const activeCategory = backupCategoryDefinitions.find((category) => category.key === contentCategory) ?? null;
  const openDetails = () => {
    setContentCategory(null);
    if (data?.config) {
      setRemoteUrl(data.config.remoteUrl);
      setDeviceLabel(data.config.deviceLabel);
      setContents(data.config.contents);
    } else {
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
      && operation.status === "conflict"
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
            {configured ? <StatefulButton size="sm" variant="primary" state={stateFor("backup")} aria-label="Back up now" width={112} onClick={() => { void backupNow(); }}>Back up now</StatefulButton> : <span className="settingsBackupNotConfigured">Not configured</span>}
            {configured ? <span className="settingsBackupLastSync">Last sync: {lastBackup ? backupDate(lastBackup.createdAt) : "Never"}</span> : null}
          </div>
          <IconButton aria-label="Backup settings" onClick={openDetails}><Settings2 size={16} aria-hidden="true" /></IconButton>
        </div>
      </div>
      <DialogShell open={detailsOpen} onOpenChange={setDetailsOpen} className="confirmDialogPanel backupDetailsDialog" descriptionId="backup-details-description">
        <Dialog.Title className="confirmDialogTitle">{activeCategory ? activeCategory.label : configured ? "Backup details" : "Set up backup"}</Dialog.Title>
        <Dialog.Description id="backup-details-description" className="dialogVisuallyHidden">Backup settings</Dialog.Description>
        <div className="backupDetailsBody">
          {activeCategory ? (
            <>
              <button type="button" className="backupDetailsBack" onClick={() => setContentCategory(null)}>
                <ChevronLeft size={15} aria-hidden="true" />
                <span>Backup contents</span>
              </button>
              <div className="backupDetailsSection backupContentDetailSection">
                <h3>{activeCategory.detail}</h3>
                {!catalog ? <p className="settingsBackupEmpty">Backup contents are unavailable. Refresh and try again.</p> : catalog[activeCategory.key].length ? (
                  <div className="backupContentItems">
                    {catalog[activeCategory.key].map((item) => {
                      const selected = contents[activeCategory.key].enabled && !contents[activeCategory.key].excluded.includes(item.id);
                      return (
                        <div className="backupContentItemRow" key={item.id}>
                          <SelectionCheckbox
                            checked={selected}
                            disabled={!contents[activeCategory.key].enabled}
                            label={`Include ${item.label}`}
                            onChange={(checked) => toggleCategoryItem(activeCategory.key, item.id, checked)}
                          />
                          <div>
                            <strong>{item.label}</strong>
                            <span>{item.detail}</span>
                          </div>
                        </div>
                      );
                    })}
                  </div>
                ) : <p className="settingsBackupEmpty">No global items found.</p>}
              </div>
            </>
          ) : (
            <>
              <div className="backupDetailsSection">
                <h3>Repository</h3>
                <div className="settingsBackupFormGrid">
                  <label className="settingsBackupField">
                    <span>Git remote</span>
                    <input className="settingsTextInput" value={remoteUrl} onChange={(event) => setRemoteUrl(event.target.value)} placeholder="owner/repository or git@github.com:you/tendi-skills-backup.git" />
                  </label>
                  <label className="settingsBackupField">
                    <span>Device label</span>
                    <input className="settingsTextInput" value={deviceLabel} onChange={(event) => setDeviceLabel(event.target.value)} placeholder="My device" />
                  </label>
                </div>
              </div>
              <div className="backupDetailsSection">
                <h3>Backup contents</h3>
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
                            <span>{!catalog ? "Unavailable" : selection.enabled ? (itemCount ? `${selectedCount} of ${itemCount} included` : "No global items") : "Not included"}</span>
                          </span>
                          <ChevronRight size={15} aria-hidden="true" />
                        </button>
                      </div>
                    );
                  })}
                </div>
              </div>
              {configured ? <div className="backupDetailsSection">
                <h3>History</h3>
                {data?.versions.length ? <ul className="settingsBackupVersionList">{data.versions.map((version) => <li key={version.id}><div className="settingsBackupVersionCopy"><strong>{version.summary}</strong><span>{backupDate(version.createdAt)} · <code>{version.id.slice(0, 12)}</code></span></div><DialogActionButton variant="secondary" onClick={() => openRestore(version)}>Restore</DialogActionButton></li>)}</ul> : <p className="settingsBackupEmpty">No backup versions yet.</p>}
              </div> : null}
            </>
          )}
        </div>
        <DialogActionBar onCancel={() => { setContentCategory(null); setDetailsOpen(false); }} cancelDisabled={action !== ""} leading={configured ? <StatefulButton size="sm" variant="danger" state={stateFor("disconnect")} aria-label="Disconnect this device from backup" width={104} onClick={() => { void disconnect(); }}>Disconnect</StatefulButton> : undefined}>
          <StatefulButton variant="primary" state={stateFor("configure")} aria-label={configured ? "Save backup settings" : "Set up backup"} width={configured ? 112 : 96} disabled={!remoteUrl.trim() || !deviceLabel.trim()} onClick={() => { void configure(); }}>{configured ? "Save" : "Set up"}</StatefulButton>
        </DialogActionBar>
      </DialogShell>
      <DialogShell open={restoreOpen} onOpenChange={setRestoreOpen} className="confirmDialogPanel backupRestoreDialog" descriptionId="backup-restore-description">
        <Dialog.Title className="confirmDialogTitle">Restore backup</Dialog.Title>
        <Dialog.Description id="backup-restore-description" className="dialogVisuallyHidden">Restore a backup version</Dialog.Description>
        <div className="backupRestoreControls">
          <SelectControl label="Agent" value={resolvedRestoreTarget} onValueChange={changeRestoreTarget} options={restoreTargetOptions} />
          <SelectControl label="Scope" value={restoreScope} onValueChange={(value) => changeRestoreScope(value as "global" | "project")} options={[{ value: "global", label: "Global" }, { value: "project", label: "Project" }]} />
        </div>
        {restoreBusy && !restorePlan ? <p className="backupDialogHint">Preparing restore plan…</p> : null}
        {!restorePlan && !restoreBusy ? <div className="backupRestoreEmpty"><DialogActionButton variant="secondary" onClick={prepareRestore}>Prepare restore plan</DialogActionButton></div> : null}
        {restorePlan ? <ul className="backupRestorePlan">{restorePlan.operations.map((operation) => <li key={operation.id} data-status={operation.status}><SelectionCheckbox checked={restoreSelectedIds.includes(operation.id)} label={`Restore ${operation.name}`} disabled={restoreBusy} onChange={(checked) => toggleRestoreSkill(operation.id, checked)} /><span>{operation.name}</span>{operation.status === "conflict" && restoreSelectedIds.includes(operation.id) ? <SelectControl label={`${operation.name} conflict`} value={restoreResolutions[operation.id] ?? ""} onValueChange={(value) => setRestoreResolutions((current) => ({ ...current, [operation.id]: value as RestoreResolution }))} options={[{ value: "keep-both", label: "Keep both" }, { value: "replace", label: "Replace existing" }, { value: "skip", label: "Keep existing" }]} renderValue={(option) => <span className="selectValueText">{option?.label ?? "Choose action"}</span>} /> : <span>{restoreSelectedIds.includes(operation.id) && operation.status === "planned" ? operation.target : operation.message ?? "Not selected"}</span>}</li>)}</ul> : null}
        <DialogActionBar onCancel={() => setRestoreOpen(false)} cancelDisabled={restoreBusy}><StatefulButton variant="primary" state={restoreBusy ? "loading" : "idle"} aria-label="Restore backup" width={104} disabled={!restorePlan || restoreBusy || restoreSelectedIds.length === 0 || hasUnresolvedRestoreConflict} onClick={() => { void applyRestore(); }}>Restore</StatefulButton></DialogActionBar>
      </DialogShell>
    </div>
  );
}
