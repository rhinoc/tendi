import { useEffect, useMemo, useRef, useState } from "react";
import { Dialog } from "radix-ui";

import { AgentOptionLabel } from "../../components/shared/AgentOptionLabel.tsx";
import { BadgeList } from "../../components/shared/BadgeList.tsx";
import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogAdvanceButton } from "../../components/shared/DialogAdvanceButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { LoadingState } from "../../components/shared/LoadingState.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { SegmentedControl, SegmentedControlItem } from "../../components/shared/SegmentedControl.tsx";
import { formatUserPath, invokeCommand, TauriCommand } from "../../lib/index.ts";
import type { SkillRecord } from "../../views/SkillsView.tsx";

type SkillTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
};

type DistributionMode = "move" | "symlink" | "copy";

type DistributionPlan = {
  name?: string;
  source?: string;
  destination?: string;
  mode?: DistributionMode;
  status?: string;
  message?: string | null;
};

type DistributionResponse = {
  plans?: DistributionPlan[];
  previewId?: string;
};

function isApplicablePlan(plan: DistributionPlan): boolean {
  return plan.status === "ready"
    || plan.status === "already-installed"
    || plan.status === "already-at-destination";
}

export type SkillLocationDialogProps = {
  open: boolean;
  skill?: SkillRecord | null;
  skills?: SkillRecord[];
  initialAgent?: string;
  onOpenChange: (open: boolean) => void;
  onApplied: () => void | Promise<void>;
};

const modeLabels: Record<DistributionMode, string> = {
  move: "Move",
  symlink: "Link (symlink)",
  copy: "Copy",
};

function sourcePathForSkill(skill: SkillRecord, initialAgent: string | undefined, singleSkill: boolean): string {
  const sourceOptions = (skill.paths ?? []).filter((path) => path.path);
  const preferred = initialAgent && singleSkill
    ? sourceOptions.find((path) => (path.install_target ?? path.agent ?? "target").split(":")[0].toLowerCase() === initialAgent.toLowerCase())
    : undefined;
  return preferred?.path ?? sourceOptions[0]?.path ?? "";
}

function targetOptionsWithShared(options: SkillTargetOption[]): SkillTargetOption[] {
  const shared: SkillTargetOption = {
    id: "shared",
    displayName: "Shared",
    supportsGlobal: true,
  };
  return [shared, ...options.filter((option) => option.id !== "shared")];
}

export function SkillLocationDialog({
  open,
  skill,
  skills,
  initialAgent,
  onOpenChange,
  onApplied,
}: SkillLocationDialogProps) {
  const selectedSkills = useMemo(
    () => skills?.length ? skills : skill ? [skill] : [],
    [skill, skills],
  );
  const [target, setTarget] = useState("shared");
  const [mode, setMode] = useState<DistributionMode>("symlink");
  const [targets, setTargets] = useState<SkillTargetOption[]>([]);
  const [plans, setPlans] = useState<DistributionPlan[]>([]);
  const [previewId, setPreviewId] = useState("");
  const [busy, setBusy] = useState<"targets" | "preview" | "apply" | "">("");
  const [error, setError] = useState("");
  const requestRef = useRef(0);

  const visibleTargets = useMemo(
    () => targets.filter((option) => option.supportsGlobal),
    [targets],
  );
  const sourcePaths = useMemo(
    () => selectedSkills
      .map((item) => sourcePathForSkill(item, initialAgent, selectedSkills.length === 1))
      .filter(Boolean),
    [initialAgent, selectedSkills],
  );
  const selectedSkillNames = useMemo(() => selectedSkills.map((item) => item.name), [selectedSkills]);
  const sourcePathsKey = sourcePaths.join("\u0000");

  useEffect(() => {
    if (!open || selectedSkills.length === 0) return;
    setTarget("shared");
    setMode("symlink");
    setPlans([]);
    setPreviewId("");
    setError("");
  }, [initialAgent, open, selectedSkills]);

  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setBusy("targets");
    void invokeCommand<SkillTargetOption[]>(TauriCommand.SkillsTargets)
      .then((result) => {
        if (!cancelled) setTargets(targetOptionsWithShared(result ?? []));
      })
      .catch((loadError) => {
        if (!cancelled) setError(String(loadError));
      })
      .finally(() => {
        if (!cancelled) setBusy("");
      });
    return () => {
      cancelled = true;
    };
  }, [open]);

  useEffect(() => {
    if (!visibleTargets.some((option) => option.id === target)) {
      setTarget(visibleTargets[0]?.id ?? "shared");
    }
  }, [target, visibleTargets]);

  useEffect(() => {
    if (!open || sourcePaths.length === 0 || !target) return;
    const requestId = ++requestRef.current;
    let cancelled = false;
    setBusy("preview");
    setPlans([]);
    setPreviewId("");
    setError("");
    void invokeCommand<DistributionResponse>(TauriCommand.SkillsDistribute, {
      sourcePaths,
      target,
      scope: "global",
      mode,
      dryRun: true,
    })
      .then((response) => {
        if (cancelled || requestId !== requestRef.current) return;
        setPlans(response?.plans ?? []);
        setPreviewId(response?.previewId ?? "");
      })
      .catch((previewError) => {
        if (!cancelled && requestId === requestRef.current) setError(String(previewError));
      })
      .finally(() => {
        if (!cancelled && requestId === requestRef.current) setBusy("");
      });
    return () => {
      cancelled = true;
    };
  }, [mode, open, sourcePaths, sourcePathsKey, target]);

  const apply = async () => {
    if (!sourcePaths.length || !previewId || plans.length !== selectedSkills.length || plans.some((plan) => !isApplicablePlan(plan)) || busy) return;
    setBusy("apply");
    setError("");
    try {
      await invokeCommand(TauriCommand.SkillsDistribute, {
        sourcePaths,
        target,
        scope: "global",
        mode,
        previewId,
        dryRun: false,
      });
      onOpenChange(false);
      await onApplied();
    } catch (applyError) {
      setError(String(applyError));
    } finally {
      setBusy("");
    }
  };

  const canApply = plans.length === selectedSkills.length
    && plans.length > 0
    && plans.every(isApplicablePlan)
    && Boolean(previewId)
    && !busy;
  const actionLabel = modeLabels[mode];

  return (
    <DialogShell
      open={open}
      onOpenChange={onOpenChange}
      className="confirmDialogPanel skillLocationDialog"
      descriptionId="skill-location-dialog-description"
    >
      <Dialog.Title className="confirmDialogTitle">Manage locations</Dialog.Title>
      <Dialog.Description id="skill-location-dialog-description" className="dialogVisuallyHidden">
        Choose a destination for the selected skills.
      </Dialog.Description>
      <div className="skillLocationBody">
        <BadgeList items={selectedSkillNames} ariaLabel="Selected skills" active={open} className="skillLocationSkillList" />
        <div className="skillLocationField">
          <span>Destination</span>
          <SelectControl
            value={target}
            onValueChange={setTarget}
            label="Skill destination target"
            contentClassName="dialogSelectContent"
            options={visibleTargets.map((option) => ({ value: option.id, label: option.displayName }))}
            renderOption={(option) => <AgentOptionLabel agent={option.value} label={option.label} />}
            renderValue={(option) => option ? <AgentOptionLabel agent={option.value} label={option.label} /> : null}
          />
        </div>
        <div className="skillLocationField">
          <span>Mode</span>
          <SegmentedControl
            fullWidth
            value={mode}
            onValueChange={(value) => {
              if (value === "move" || value === "symlink" || value === "copy") setMode(value);
            }}
            aria-label="Skill distribution mode"
          >
            <SegmentedControlItem value="move">Move</SegmentedControlItem>
            <SegmentedControlItem value="symlink">Link</SegmentedControlItem>
            <SegmentedControlItem value="copy">Copy</SegmentedControlItem>
          </SegmentedControl>
        </div>
        {busy === "targets" || busy === "preview" ? <LoadingState className="loadingStateCompact" label={busy === "targets" ? "Loading targets" : "Preparing preview"} /> : null}
        {plans.length > 0 ? (
          <div className="skillLocationPlans" data-selectable-text>
            {plans.map((plan) => (
              <div className={"skillLocationPlan" + (plan.status === "conflict" ? " isConflict" : "")} key={plan.name}>
                <strong>{plan.name}</strong>
                <span aria-hidden="true">→</span>
                <code>{formatUserPath(plan.destination)}</code>
                {plan.message && <small>{plan.message}</small>}
              </div>
            ))}
          </div>
        ) : null}
        {error && <div className="dialogError" role="alert" data-selectable-text>{error}</div>}
      </div>
      <DialogActionBar onCancel={() => onOpenChange(false)}>
        <DialogAdvanceButton
          label={actionLabel}
          busyLabel={actionLabel + " skill"}
          busy={busy === "apply"}
          disabled={!canApply}
          onClick={() => { void apply(); }}
        />
      </DialogActionBar>
    </DialogShell>
  );
}
