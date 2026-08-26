import { useEffect, useMemo, useState } from "react";
import { ArrowRightLeft, ChevronDown, Copy, Link2 } from "lucide-react";
import { Dialog, DropdownMenu } from "radix-ui";

import { AgentOptionLabel } from "../../components/shared/AgentOptionLabel.tsx";
import { BadgeList } from "../../components/shared/BadgeList.tsx";
import { CheckboxIndicator } from "../../components/shared/CheckboxIndicator.tsx";
import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogAdvanceButton } from "../../components/shared/DialogAdvanceButton.tsx";
import { DialogActionButton } from "../../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import { DialogMenuContent } from "../../components/shared/DialogMenuContent.tsx";
import { SegmentedControl, SegmentedControlItem } from "../../components/shared/SegmentedControl.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { agentIdentityKey, formatUserPath, invokeCommand, TauriCommand } from "../../lib/index.ts";
import type { RawSkillRecord, SkillRecord } from "../../views/SkillsView.tsx";

type SkillTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
  globalPath?: string;
};

type DistributionMode = "move" | "symlink" | "copy";

type DistributionResponse = {
  skills: RawSkillRecord[];
};

export type SkillLocationDialogProps = {
  open: boolean;
  skill?: SkillRecord | null;
  skills?: SkillRecord[];
  initialAgent?: string;
  installedAgentKeys: string[];
  targetOptions?: SkillTargetOption[];
  onOpenChange: (open: boolean) => void;
  onApplied: (skills?: RawSkillRecord[]) => void | Promise<void>;
};

const modeOptions: Array<{ value: DistributionMode; label: string; icon: typeof ArrowRightLeft }> = [
  { value: "move", label: "Move", icon: ArrowRightLeft },
  { value: "copy", label: "Copy", icon: Copy },
  { value: "symlink", label: "Link", icon: Link2 },
];

function sourcePathForSkill(skill: SkillRecord, initialAgent: string | undefined, singleSkill: boolean): string {
  const sourceOptions = skill.paths.filter((path) => path.path);
  const preferred = initialAgent && singleSkill
    ? sourceOptions.find((path) => path.install_target.split(":")[0].toLowerCase() === initialAgent.toLowerCase())
    : undefined;
  return preferred?.path ?? sourceOptions[0]?.path ?? "";
}

function targetOptionsForDialog(options: SkillTargetOption[]): SkillTargetOption[] {
  return options.filter((option) => option.id !== "universal");
}

function skillHasTarget(skill: SkillRecord, target: SkillTargetOption): boolean {
  return skill.paths.some((path) => {
    const scope = `${path.scope ?? ""}`.toLowerCase();
    if (scope && scope !== "global") return false;
    const agent = path.install_target.split(":")[0];
    return agentIdentityKey(agent) === agentIdentityKey(target.id);
  });
}

export function SkillLocationDialog({
  open,
  skill,
  skills,
  initialAgent,
  installedAgentKeys,
  targetOptions = [],
  onOpenChange,
  onApplied,
}: SkillLocationDialogProps) {
  const selectedSkills = useMemo(
    () => skills?.length ? skills : skill ? [skill] : [],
    [skill, skills],
  );
  const [mode, setMode] = useState<DistributionMode>("move");
  const [targets, setTargets] = useState<SkillTargetOption[]>(() => targetOptionsForDialog(targetOptions));
  const [targetOverrides, setTargetOverrides] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState<"apply" | "">("");
  const [confirmRemoval, setConfirmRemoval] = useState(false);
  const [error, setError] = useState("");

  const visibleTargets = useMemo(() => {
    const installed = new Set(installedAgentKeys);
    return targets
      .filter((option) => option.supportsGlobal && option.id !== "universal")
      .map((option, index) => ({
        option,
        index,
        installed: installed.has(agentIdentityKey(option.id)),
      }))
      .sort((left, right) => {
        if (left.option.id === "shared") return -1;
        if (right.option.id === "shared") return 1;
        if (left.installed !== right.installed) return left.installed ? -1 : 1;
        return left.index - right.index;
      })
      .map(({ option }) => option);
  }, [installedAgentKeys, targets]);
  const currentTargetCounts = useMemo(
    () => new Map(visibleTargets.map((option) => [
      option.id,
      selectedSkills.filter((item) => skillHasTarget(item, option)).length,
    ])),
    [selectedSkills, visibleTargets],
  );
  const targetState = (targetId: string): "checked" | "mixed" | "unchecked" => {
    const override = targetOverrides[targetId];
    if (override !== undefined) return override ? "checked" : "unchecked";
    const count = currentTargetCounts.get(targetId) ?? 0;
    if (count === selectedSkills.length && count > 0) return "checked";
    if (count > 0) return "mixed";
    return "unchecked";
  };
  const selectedLocationNames = useMemo(
    () => visibleTargets
      .filter((option) => targetState(option.id) !== "unchecked")
      .map((option) => option.displayName),
    [targetOverrides, visibleTargets, currentTargetCounts, selectedSkills.length],
  );
  const locationTriggerLabel = selectedLocationNames.length === 0
    ? "Select locations"
    : selectedLocationNames.length === 1
      ? selectedLocationNames[0]
      : `${selectedLocationNames.length} locations`;
  const sourcePathsBySkill = useMemo(
    () => selectedSkills.map((item) => sourcePathForSkill(item, initialAgent, selectedSkills.length === 1)),
    [initialAgent, selectedSkills],
  );
  const sourcePaths = sourcePathsBySkill.filter(Boolean);
  const selectedSkillsKey = useMemo(() => selectedSkills.map((item) => item.id).join("\u0000"), [selectedSkills]);
  const selectedSkillNames = useMemo(() => selectedSkills.map((item) => item.name), [selectedSkills]);
  const locationChanges = useMemo(() => {
    const addedTargetIds = new Set<string>();
    const removedTargetIds = new Set<string>();
    for (const option of visibleTargets) {
      const override = targetOverrides[option.id];
      if (override === undefined) continue;
      for (const item of selectedSkills) {
        const installed = skillHasTarget(item, option);
        if (override && !installed) addedTargetIds.add(option.id);
        if (!override && installed) removedTargetIds.add(option.id);
      }
    }
    return {
      addedTargetIds: [...addedTargetIds],
      removedTargetIds: [...removedTargetIds],
    };
  }, [selectedSkills, targetOverrides, visibleTargets]);

  useEffect(() => {
    if (!open || selectedSkills.length === 0) return;
    setMode("move");
    setTargetOverrides({});
    setConfirmRemoval(false);
    setError("");
  }, [initialAgent, open, selectedSkillsKey]);

  useEffect(() => {
    if (targetOptions.length > 0) setTargets(targetOptionsForDialog(targetOptions));
  }, [targetOptions]);

  const moveHasMultipleAdds = mode === "move" && locationChanges.addedTargetIds.length > 1;
  const missingSource = locationChanges.addedTargetIds.length > 0 && sourcePaths.length !== selectedSkills.length;
  const canApply = (locationChanges.addedTargetIds.length > 0 || locationChanges.removedTargetIds.length > 0)
    && !moveHasMultipleAdds
    && !missingSource
    && !busy;
  const applyDisabledReason = busy
    ? "Applying location changes"
    : moveHasMultipleAdds
      ? "Move can add one location at a time."
      : missingSource
        ? "One selected skill has no movable source."
        : locationChanges.addedTargetIds.length === 0 && locationChanges.removedTargetIds.length === 0
          ? "Select a location to add or remove."
          : "";

  const apply = async (confirmed = false) => {
    if (!canApply) return;
    if (!confirmed && locationChanges.removedTargetIds.length > 0) {
      setConfirmRemoval(true);
      return;
    }
    setBusy("apply");
    setError("");
    try {
      let updatedSkills: RawSkillRecord[] | undefined;
      for (const targetId of locationChanges.addedTargetIds) {
        const response = await invokeCommand<DistributionResponse>(TauriCommand.SkillsDistribute, {
          sourcePaths,
          target: targetId,
          scope: "global",
          mode,
          dryRun: false,
        });
        updatedSkills = response.skills;
      }
      if (locationChanges.removedTargetIds.length > 0) {
        const response = await invokeCommand<DistributionResponse>(TauriCommand.SkillsRemoveLocations, {
          names: selectedSkills.map((item) => item.name),
          targets: locationChanges.removedTargetIds,
          scope: "global",
        });
        updatedSkills = response.skills;
      }
      setConfirmRemoval(false);
      onOpenChange(false);
      await onApplied(updatedSkills);
    } catch (applyError) {
      setError(String(applyError));
    } finally {
      setBusy("");
    }
  };

  return (
    <>
    <DialogShell
      open={open}
      onOpenChange={onOpenChange}
      className="confirmDialogPanel skillLocationDialog"
      descriptionId="skill-location-dialog-description"
    >
      <Dialog.Title className="confirmDialogTitle">Manage locations</Dialog.Title>
      <Dialog.Description id="skill-location-dialog-description" className="dialogVisuallyHidden">
        Choose the locations for the selected skills.
      </Dialog.Description>
      <div className="skillLocationBody">
        <BadgeList items={selectedSkillNames} ariaLabel="Selected skills" active={open} className="skillLocationSkillList" />
        <div className="skillLocationField">
          <span>Locations</span>
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button
                type="button"
                className="selectControlTrigger skillLocationTrigger"
                aria-label={`Locations: ${locationTriggerLabel}`}
                disabled={busy === "apply"}
              >
                <span className="skillLocationTriggerText">{locationTriggerLabel}</span>
                <ChevronDown size={14} aria-hidden="true" />
              </button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <DialogMenuContent
                className="skillLocationMenu"
                align="start"
                sideOffset={6}
                collisionPadding={8}
                data-no-drag
              >
                {visibleTargets.map((option) => {
                  const state = targetState(option.id);
                  const checked = state === "checked";
                  const mixed = state === "mixed";
                  return (
                    <DropdownMenu.CheckboxItem
                      key={option.id}
                      className="skillMenuItem skillLocationMenuItem"
                      checked={checked}
                      aria-checked={mixed ? "mixed" : checked}
                      disabled={busy === "apply"}
                      onCheckedChange={(nextChecked) => {
                        setTargetOverrides((current) => ({ ...current, [option.id]: nextChecked }));
                      }}
                      onSelect={(event) => event.preventDefault()}
                    >
                      <CheckboxIndicator checked={checked} mixed={mixed} />
                      <span className="skillLocationTargetDetails">
                        <AgentOptionLabel agent={option.id} label={option.displayName} />
                        {option.globalPath ? <span className="skillLocationTargetPath">({formatUserPath(option.globalPath)})</span> : null}
                        {selectedSkills.length > 1 && (currentTargetCounts.get(option.id) ?? 0) > 0 ? (
                          <span className="skillLocationTargetCount">
                            {currentTargetCounts.get(option.id)}/{selectedSkills.length}
                          </span>
                        ) : null}
                      </span>
                    </DropdownMenu.CheckboxItem>
                  );
                })}
              </DialogMenuContent>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        </div>
        <div className="skillLocationField skillLocationMode">
          <span>Mode</span>
          <SegmentedControl
            fullWidth
            value={mode}
            onValueChange={(value) => {
              if (value === "move" || value === "copy" || value === "symlink") setMode(value);
            }}
            aria-label="Skill distribution mode"
          >
            {modeOptions.map(({ value, label, icon: Icon }) => (
              <SegmentedControlItem value={value} key={value}>
                <Icon size={14} aria-hidden="true" />
                <span>{label}</span>
              </SegmentedControlItem>
            ))}
          </SegmentedControl>
          {moveHasMultipleAdds ? <small className="skillLocationHint">Move can add one location at a time.</small> : null}
          {missingSource ? <small className="skillLocationHint">One selected skill has no movable source.</small> : null}
        </div>
        {error ? <Toast message={error} tone="error" /> : null}
      </div>
      <DialogActionBar onCancel={() => onOpenChange(false)}>
        <Tooltip content={applyDisabledReason}>
          <span className="skillLocationApplyTooltipTarget">
            <DialogAdvanceButton
              label="Apply"
              busyLabel="Applying location changes"
              ariaLabel={applyDisabledReason || "Apply location changes"}
              busy={busy === "apply"}
              disabled={!canApply}
              onClick={() => { void apply(); }}
            />
          </span>
        </Tooltip>
      </DialogActionBar>
    </DialogShell>
    <DialogShell
      open={confirmRemoval}
      onOpenChange={setConfirmRemoval}
      className="confirmDialogPanel skillLocationConfirmDialog"
      descriptionId="skill-location-remove-description"
    >
      <Dialog.Title className="confirmDialogTitle">Remove locations?</Dialog.Title>
      <Dialog.Description id="skill-location-remove-description" className="confirmDialogDescription">
        The selected skill files will be removed from the unchecked locations. The skills themselves will not be deleted.
      </Dialog.Description>
      <div className="confirmDialogActions">
        <DialogActionButton variant="secondary" onClick={() => setConfirmRemoval(false)}>Cancel</DialogActionButton>
        <DialogActionButton variant="danger" onClick={() => { void apply(true); }}>Remove locations</DialogActionButton>
      </div>
    </DialogShell>
    </>
  );
}
