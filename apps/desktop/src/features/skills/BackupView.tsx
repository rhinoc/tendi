import { useCallback, useEffect, useMemo, useState } from "react";
import { Dialog } from "radix-ui";

import { ContentTopDragStrip } from "../../components/shared/ContentTopDragStrip.tsx";
import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { DialogTextField } from "../../components/shared/DialogTextField.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { PageHeader } from "../../components/shared/PageHeader.tsx";
import { SelectionCheckbox } from "../../components/shared/SelectionCheckbox.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton, type StatefulButtonState } from "../../components/shared/StatefulButton.tsx";
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

export function BackupView() {
  const [data, setData] = useState<BackupStatusResponse | null>(null);
  const [remoteUrl, setRemoteUrl] = useState("");
  const [deviceLabel, setDeviceLabel] = useState("My device");
  const [publicRemoteAcknowledged, setPublicRemoteAcknowledged] = useState(false);
  const [error, setError] = useState("");
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
      const next = await invokeCommand<BackupStatusResponse>(TauriCommand.SkillsBackupStatus);
      setData(next);
      setError("");
      if (next.config) {
        setRemoteUrl(next.config.remoteUrl);
        setDeviceLabel(next.config.deviceLabel);
      }
      const targetOptions = await invokeCommand<BackupTarget[]>(TauriCommand.SkillsTargets);
      setTargets(targetOptions);
    } catch (loadError) {
      setError(errorMessage(loadError));
    }
  }, []);

  useEffect(() => { void load(); }, [load]);

  const counts = useMemo(() => {
    const next = new Map<string, number>();
    for (const status of data?.statuses ?? []) next.set(status.state, (next.get(status.state) ?? 0) + 1);
    return next;
  }, [data?.statuses]);

  const configure = async () => {
    if (!remoteUrl.trim() || !deviceLabel.trim() || !publicRemoteAcknowledged) return;
    setAction("configure");
    try {
      await invokeCommand(TauriCommand.SkillsBackupConfigure, { remoteUrl, deviceLabel });
      await load();
    } catch (configureError) {
      setError(errorMessage(configureError));
    } finally {
      setAction("");
    }
  };

  const backupNow = async () => {
    setAction("backup");
    try {
      await invokeCommand(TauriCommand.SkillsBackupNow);
      await load();
    } catch (backupError) {
      setError(errorMessage(backupError));
    } finally {
      setAction("");
    }
  };

  const disconnect = async () => {
    setAction("disconnect");
    try {
      await invokeCommand(TauriCommand.SkillsBackupDisconnect);
      await load();
    } catch (disconnectError) {
      setError(errorMessage(disconnectError));
    } finally {
      setAction("");
    }
  };

  const previewRestore = useCallback(async (version: BackupVersion, target = restoreTarget, scope = restoreScope, resetSelection = false) => {
    setRestoreBusy(true);
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
      setError("");
    } catch (previewError) {
      setRestorePlan(null);
      setError(errorMessage(previewError));
    } finally {
      setRestoreBusy(false);
    }
  }, [restoreScope, restoreTarget]);

  const openRestore = (version: BackupVersion) => {
    setRestoreVersion(version);
    setRestorePlan(null);
    setRestoreSelectedIds([]);
    setRestoreResolutions({});
    setRestoreOpen(true);
    void previewRestore(version, restoreTarget, restoreScope, true);
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
      setError(errorMessage(restoreError));
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
    <section className="content backupPage">
      <ContentTopDragStrip />
      <PageHeader title="Backup" />
      <div className="backupContent">
        {error ? <LoadErrorState message={error} onRetry={() => { void load(); }} /> : null}
        {!configured ? (
          <section className="backupPanel">
            <h2>Connect backup</h2>
            <DialogTextField label="Git remote" value={remoteUrl} onChange={setRemoteUrl} placeholder="git@github.com:you/tendi-skills-backup.git" />
            <DialogTextField label="Device label" value={deviceLabel} onChange={setDeviceLabel} placeholder="My device" />
            <label className="backupAcknowledgement">
              <input type="checkbox" checked={publicRemoteAcknowledged} onChange={(event) => setPublicRemoteAcknowledged(event.target.checked)} />
              <span>I confirm this remote can store my skills.</span>
            </label>
            <StatefulButton
              variant="primary"
              state={stateFor("configure")}
              aria-label="Connect backup remote"
              width={132}
              disabled={!remoteUrl.trim() || !deviceLabel.trim() || !publicRemoteAcknowledged}
              onClick={() => { void configure(); }}
            >
              Connect
            </StatefulButton>
          </section>
        ) : (
          <>
            <section className="backupPanel backupSummary">
              <div>
                <h2>Connected</h2>
                <p className="dataCellSub">{data?.config?.remoteUrl}</p>
                <p className="dataCellSub">{data?.config?.deviceLabel}</p>
              </div>
              <StatefulButton variant="primary" state={stateFor("backup")} aria-label="Back up now" width={132} onClick={() => { void backupNow(); }}>
                Back up now
              </StatefulButton>
            </section>
            <section className="backupPanel">
              <h2>Coverage</h2>
              <dl className="backupCounts">
                <div><dt>Backed up</dt><dd>{counts.get("backed-up") ?? 0}</dd></div>
                <div><dt>Pending</dt><dd>{counts.get("pending") ?? 0}</dd></div>
                <div><dt>Needs attention</dt><dd>{counts.get("needs-attention") ?? 0}</dd></div>
                <div><dt>Excluded</dt><dd>{counts.get("excluded") ?? 0}</dd></div>
              </dl>
            </section>
            {(counts.get("needs-attention") ?? 0) > 0 ? (
              <section className="backupPanel">
                <h2>Needs attention</h2>
                <p className="dataCellSub">Resolve the Git conflict in {data?.config?.checkoutPath}, then back up again.</p>
              </section>
            ) : null}
            <section className="backupPanel">
              <h2>History</h2>
              {data?.versions.length ? (
                <ul className="backupVersionList">
                  {data.versions.map((version) => (
                    <li key={version.id}>
                      <code>{version.id.slice(0, 12)}</code>
                      <span>{version.summary}</span>
                      <DialogActionButton variant="secondary" onClick={() => openRestore(version)}>Restore</DialogActionButton>
                    </li>
                  ))}
                </ul>
              ) : <p className="dataCellSub">No backup versions yet.</p>}
            </section>
            <section className="backupPanel backupDisconnect">
              <div>
                <h2>Disconnect this machine</h2>
                <p className="dataCellSub">Keeps local skills and the remote backup unchanged.</p>
              </div>
              <StatefulButton variant="danger" state={stateFor("disconnect")} aria-label="Disconnect this machine from backup" width={132} onClick={() => { void disconnect(); }}>
                Disconnect
              </StatefulButton>
            </section>
          </>
        )}
      </div>
      <DialogShell open={restoreOpen} onOpenChange={setRestoreOpen} className="confirmDialogPanel backupRestoreDialog" descriptionId="backup-restore-description">
        <div className="confirmDialogHeader">
          <Dialog.Title>Restore backup</Dialog.Title>
          <Dialog.Description id="backup-restore-description">
            Choose where to restore this snapshot. Existing skills are never overwritten automatically.
          </Dialog.Description>
        </div>
        <div className="backupRestoreControls">
          <SelectControl
            label="Agent"
            value={restoreTarget}
            onValueChange={changeRestoreTarget}
            options={[{ value: "shared", label: "Shared" }, ...targets.map((target) => ({ value: target.id, label: target.displayName }))]}
          />
          <SelectControl
            label="Scope"
            value={restoreScope}
            onValueChange={(value) => changeRestoreScope(value as "global" | "project")}
            options={[
              { value: "global", label: "Global" },
              { value: "project", label: "Project" },
            ]}
          />
        </div>
        {restoreBusy && !restorePlan ? <p className="dataCellSub">Preparing restore plan…</p> : null}
        {restorePlan ? (
          <ul className="backupRestorePlan">
            {restorePlan.operations.map((operation) => (
              <li key={operation.id} data-status={operation.status}>
                <SelectionCheckbox
                  checked={restoreSelectedIds.includes(operation.id)}
                  label={`Restore ${operation.name}`}
                  disabled={restoreBusy}
                  onChange={(checked) => toggleRestoreSkill(operation.id, checked)}
                />
                <span>{operation.name}</span>
                {operation.status === "conflict" && restoreSelectedIds.includes(operation.id) ? (
                  <SelectControl
                    label={`${operation.name} conflict`}
                    value={restoreResolutions[operation.id] ?? ""}
                    onValueChange={(value) => setRestoreResolutions((current) => ({ ...current, [operation.id]: value as RestoreResolution }))}
                    options={[
                      { value: "keep-both", label: "Keep both" },
                      { value: "replace", label: "Replace existing" },
                      { value: "skip", label: "Keep existing" },
                    ]}
                    renderValue={(option) => <span className="selectValueText">{option?.label ?? "Choose action"}</span>}
                  />
                ) : <span>{restoreSelectedIds.includes(operation.id) && operation.status === "planned" ? operation.target : operation.message ?? "Not selected"}</span>}
              </li>
            ))}
          </ul>
        ) : null}
        <DialogActionBar onCancel={() => setRestoreOpen(false)} cancelDisabled={restoreBusy}>
          <StatefulButton
            variant="primary"
            state={restoreBusy ? "loading" : "idle"}
            aria-label="Restore backup"
            width={104}
            disabled={!restorePlan || restoreBusy || restoreSelectedIds.length === 0 || hasUnresolvedRestoreConflict}
            onClick={() => { void applyRestore(); }}
          >
            Restore
          </StatefulButton>
        </DialogActionBar>
      </DialogShell>
    </section>
  );
}
