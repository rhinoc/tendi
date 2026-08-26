import { useCallback, useEffect, useMemo, useState } from "react";
import { Dialog } from "radix-ui";

import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { SelectionCheckbox } from "../../components/shared/SelectionCheckbox.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton, type StatefulButtonState } from "../../components/shared/StatefulButton.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { SettingsSection } from "../settings/SettingsSection.tsx";
import { TauriCommand, invokeCommand } from "../../lib/index.ts";
import "./BackupView.css";

type BackupConfig = {
  remoteUrl: string;
  checkoutPath: string;
  deviceLabel: string;
};

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
};

type BackupTarget = { id: string; displayName: string; supportsGlobal: boolean };

type BackupRestorePlan = {
  revision: string;
  targetRoot: string;
  operations: Array<{ id: string; name: string; target: string; status: string; message?: string | null }>;
};

type RestoreResolution = "skip" | "replace" | "keep-both";

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : `${error}`;
}

function backupDate(createdAt: number) {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(new Date(createdAt * 1000));
}

export function BackupSettings() {
  const [data, setData] = useState<BackupStatusResponse | null>(null);
  const [remoteUrl, setRemoteUrl] = useState("");
  const [deviceLabel, setDeviceLabel] = useState("My device");
  const [loadError, setLoadError] = useState("");
  const [actionError, setActionError] = useState("");
  const [action, setAction] = useState<"configure" | "backup" | "disconnect" | "">("");
  const [targets, setTargets] = useState<BackupTarget[]>([]);
  const [restoreOpen, setRestoreOpen] = useState(false);
  const [restoreVersion, setRestoreVersion] = useState<BackupVersion | null>(null);
  const [restoreTarget, setRestoreTarget] = useState("shared");
  const [restoreScope, setRestoreScope] = useState<"global" | "project">("global");
  const [restorePlan, setRestorePlan] = useState<BackupRestorePlan | null>(null);
  const [restoreSelectedIds, setRestoreSelectedIds] = useState<string[]>([]);
  const [restoreResolutions, setRestoreResolutions] = useState<Record<string, RestoreResolution>>({});
  const [restoreBusy, setRestoreBusy] = useState(false);

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
      }
      setTargets(targetOptions);
    } catch (loadError) {
      setLoadError(errorMessage(loadError));
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const counts = useMemo(() => {
    const next = new Map<string, number>();
    for (const status of data?.statuses ?? []) next.set(status.state, (next.get(status.state) ?? 0) + 1);
    return next;
  }, [data?.statuses]);

  const configure = async () => {
    if (!remoteUrl.trim() || !deviceLabel.trim()) return;
    setAction("configure");
    setActionError("");
    try {
      await invokeCommand(TauriCommand.SkillsBackupConfigure, { remoteUrl, deviceLabel });
      await load();
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

  const previewRestore = useCallback(async (version: BackupVersion, target = restoreTarget, scope = restoreScope, resetSelection = false) => {
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
  }, [restoreScope, restoreTarget]);

  const openRestore = (version: BackupVersion) => {
    setRestoreVersion(version);
    setRestorePlan(null);
    setRestoreSelectedIds([]);
    setRestoreResolutions({});
    setActionError("");
    setRestoreOpen(true);
  };

  const prepareRestore = () => {
    if (restoreVersion && !restoreBusy) void previewRestore(restoreVersion, restoreTarget, restoreScope, true);
  };

  const changeRestoreTarget = (nextTarget: string) => {
    setRestoreTarget(nextTarget);
    setRestoreResolutions({});
    if (restoreVersion) void previewRestore(restoreVersion, nextTarget, restoreScope);
  };

  const changeRestoreScope = (nextScope: "global" | "project") => {
    setRestoreScope(nextScope);
    setRestoreResolutions({});
    if (restoreVersion) void previewRestore(restoreVersion, restoreTarget, nextScope);
  };

  const applyRestore = async () => {
    if (!restoreVersion) return;
    setRestoreBusy(true);
    setActionError("");
    try {
      await invokeCommand(TauriCommand.SkillsBackupRestore, {
        revision: restoreVersion.id,
        skillIds: restoreSelectedIds,
        target: restoreTarget,
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
      {!configured ? (
        <>
          <SettingsSection title="Repository">
              <div className="settingsBackupFormGrid">
                <label className="settingsBackupField">
                  <span>Git remote</span>
                  <input className="settingsTextInput" value={remoteUrl} onChange={(event) => setRemoteUrl(event.target.value)} placeholder="git@github.com:you/tendi-skills-backup.git" />
                </label>
                <label className="settingsBackupField">
                  <span>Device label</span>
                  <input className="settingsTextInput" value={deviceLabel} onChange={(event) => setDeviceLabel(event.target.value)} placeholder="My device" />
                </label>
              </div>
              <div className="settingsBackupActions">
                <StatefulButton size="sm" variant="primary" state={stateFor("configure")} aria-label="Connect backup remote" width={96} disabled={!remoteUrl.trim() || !deviceLabel.trim()} onClick={() => { void configure(); }}>Connect</StatefulButton>
              </div>
          </SettingsSection>
        </>
      ) : (
        <>
          <SettingsSection title="Repository">
              <div className="settingsBackupRepository">
                <div className="settingsBackupRepositoryInfo">
                  <strong>{data?.config?.remoteUrl}</strong>
                  <span>Device: {data?.config?.deviceLabel}</span>
                </div>
                <StatefulButton size="sm" variant="primary" state={stateFor("backup")} aria-label="Back up now" width={112} onClick={() => { void backupNow(); }}>Back up now</StatefulButton>
              </div>
              <div className="settingsBackupCheckout"><span>Checkout</span><code>{data?.config?.checkoutPath}</code></div>
          </SettingsSection>
          <SettingsSection title="Coverage">
              <div className="settingsBackupStats" aria-label="Backup coverage">
                <div><strong>{counts.get("backed-up") ?? 0}</strong><span>Backed up</span></div>
                <div><strong>{counts.get("pending") ?? 0}</strong><span>Pending</span></div>
                <div><strong>{counts.get("unmanaged") ?? 0}</strong><span>Not added</span></div>
                <div><strong>{counts.get("needs-attention") ?? 0}</strong><span>Attention</span></div>
              </div>
          </SettingsSection>
          <SettingsSection title="History">
              {data?.versions.length ? <ul className="settingsBackupVersionList">{data.versions.map((version) => <li key={version.id}><div className="settingsBackupVersionCopy"><strong>{version.summary}</strong><span>{backupDate(version.createdAt)} · <code>{version.id.slice(0, 12)}</code></span></div><DialogActionButton variant="secondary" onClick={() => openRestore(version)}>Restore</DialogActionButton></li>)}</ul> : <p className="settingsBackupEmpty">No backup versions yet.</p>}
              <div className="settingsBackupDisconnect">
                <span>Disconnect this device</span>
                <StatefulButton size="sm" variant="danger" state={stateFor("disconnect")} aria-label="Disconnect this device from backup" width={104} onClick={() => { void disconnect(); }}>Disconnect</StatefulButton>
              </div>
          </SettingsSection>
        </>
      )}
      <DialogShell open={restoreOpen} onOpenChange={setRestoreOpen} className="confirmDialogPanel backupRestoreDialog" descriptionId="backup-restore-description">
        <div className="confirmDialogHeader"><Dialog.Title>Restore backup</Dialog.Title><Dialog.Description id="backup-restore-description">Choose where to restore this snapshot. Existing skills are never overwritten automatically.</Dialog.Description></div>
        <div className="backupRestoreControls">
          <SelectControl label="Agent" value={restoreTarget} onValueChange={changeRestoreTarget} options={targets.map((target) => ({ value: target.id, label: target.displayName }))} />
          <SelectControl label="Scope" value={restoreScope} onValueChange={(value) => changeRestoreScope(value as "global" | "project")} options={[{ value: "global", label: "Global" }, { value: "project", label: "Project" }]} />
        </div>
        {restoreBusy && !restorePlan ? <p className="backupDialogHint">Preparing restore plan…</p> : null}
        {!restorePlan && !restoreBusy ? <div className="backupRestoreEmpty"><p className="backupDialogHint">Choose when to inspect this backup snapshot.</p><DialogActionButton variant="secondary" onClick={prepareRestore}>Prepare restore plan</DialogActionButton></div> : null}
        {restorePlan ? <ul className="backupRestorePlan">{restorePlan.operations.map((operation) => <li key={operation.id} data-status={operation.status}><SelectionCheckbox checked={restoreSelectedIds.includes(operation.id)} label={`Restore ${operation.name}`} disabled={restoreBusy} onChange={(checked) => toggleRestoreSkill(operation.id, checked)} /><span>{operation.name}</span>{operation.status === "conflict" && restoreSelectedIds.includes(operation.id) ? <SelectControl label={`${operation.name} conflict`} value={restoreResolutions[operation.id] ?? ""} onValueChange={(value) => setRestoreResolutions((current) => ({ ...current, [operation.id]: value as RestoreResolution }))} options={[{ value: "keep-both", label: "Keep both" }, { value: "replace", label: "Replace existing" }, { value: "skip", label: "Keep existing" }]} renderValue={(option) => <span className="selectValueText">{option?.label ?? "Choose action"}</span>} /> : <span>{restoreSelectedIds.includes(operation.id) && operation.status === "planned" ? operation.target : operation.message ?? "Not selected"}</span>}</li>)}</ul> : null}
        <DialogActionBar onCancel={() => setRestoreOpen(false)} cancelDisabled={restoreBusy}><StatefulButton variant="primary" state={restoreBusy ? "loading" : "idle"} aria-label="Restore backup" width={104} disabled={!restorePlan || restoreBusy || restoreSelectedIds.length === 0 || hasUnresolvedRestoreConflict} onClick={() => { void applyRestore(); }}>Restore</StatefulButton></DialogActionBar>
      </DialogShell>
    </div>
  );
}
