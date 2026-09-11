import { useEffect, useMemo, useState } from "react";
import { Dialog } from "radix-ui";

import { AgentOptionLabel } from "../../components/shared/AgentOptionLabel.tsx";
import { BadgeList } from "../../components/shared/BadgeList.tsx";
import { DialogActionBar } from "../../components/shared/DialogActionBar.tsx";
import { DialogApplyButton } from "../../components/shared/DialogApplyButton.tsx";
import { DialogShell } from "../../components/shared/DialogShell.tsx";
import {
  MultiSelect,
  MultiSelectContent,
  MultiSelectEmpty,
  MultiSelectItem,
  MultiSelectList,
  MultiSelectTrigger,
  MultiSelectValue,
} from "../../components/shared/MultiSelect.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { Tooltip } from "../../components/shared/Tooltip.tsx";
import { agentIdentityKey, formatUserPath, isVisibleAgent } from "../../lib/index.ts";
import type { NormalizedSkill, RawSkillRecord } from "../../lib/index.ts";
import { distributeSkills, removeSkillLocations, SkillDistributionMode, SkillScope } from "../../lib/runtime-gateway.ts";
import { mergeSkillRows } from "../../controllers/skill-controller.ts";

type SkillTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
  globalPath?: string;
};

enum SkillLocationBusyAction {
  Idle = "",
  Apply = "apply",
}

export type SkillLocationDialogProps = {
  open: boolean;
  skill?: NormalizedSkill | null;
  skills?: NormalizedSkill[];
  initialAgent?: string;
  installedAgentKeys: string[];
  targetOptions?: SkillTargetOption[];
  onOpenChange: (open: boolean) => void;
  onApplied: (skills?: RawSkillRecord[], options?: { patch?: boolean; deleted?: string[] }) => void | Promise<void>;
};

function sourcePathForSkill(skill: NormalizedSkill, initialAgent: string | undefined, singleSkill: boolean): string {
  const sourceOptions = skill.paths.filter((path) => path.path);
  const preferred = initialAgent && singleSkill
    ? sourceOptions.find((path) => path.install_target.split(":")[0].toLowerCase() === initialAgent.toLowerCase())
    : undefined;
  return preferred?.path ?? sourceOptions[0]?.path ?? "";
}

function targetOptionsForDialog(options: SkillTargetOption[]): SkillTargetOption[] {
  return options.filter((option) => option.id !== "universal" && (option.id === "shared" || isVisibleAgent(option.id)));
}

function skillHasTarget(skill: NormalizedSkill, target: SkillTargetOption): boolean {
  return skill.paths.some((path) => {
    const scope = `${path.scope ?? ""}`.toLowerCase();
    if (scope && scope !== SkillScope.Global) return false;
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
  const [targetOverrides, setTargetOverrides] = useState<Record<string, boolean>>({});
  const [busy, setBusy] = useState<SkillLocationBusyAction>(SkillLocationBusyAction.Idle);
  const [error, setError] = useState("");

  const visibleTargets = useMemo(() => {
    const installed = new Set(installedAgentKeys);
    return targetOptionsForDialog(targetOptions)
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
  }, [installedAgentKeys, targetOptions]);
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
  const selectedLocationIds = useMemo(
    () => visibleTargets
      .filter((option) => targetState(option.id) !== "unchecked")
      .map((option) => option.id),
    [targetOverrides, visibleTargets, currentTargetCounts, selectedSkills.length],
  );
  const sourcePathsBySkill = useMemo(
    () => selectedSkills.map((item) => sourcePathForSkill(item, initialAgent, selectedSkills.length === 1)),
    [initialAgent, selectedSkills],
  );
  const sourcePaths = sourcePathsBySkill.filter(Boolean);
  const selectedSkillsKey = useMemo(() => selectedSkills.map((item) => item.id).sort().join("\u0000"), [selectedSkills]);
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
    setTargetOverrides({});
    setError("");
  }, [initialAgent, open, selectedSkillsKey]);

  const missingSource = locationChanges.addedTargetIds.length > 0 && sourcePaths.length !== selectedSkills.length;
  const hasLocationChanges = locationChanges.addedTargetIds.length > 0 || locationChanges.removedTargetIds.length > 0;
  const hasSelectedLocations = selectedLocationIds.length > 0;
  const canApply = hasLocationChanges
    && hasSelectedLocations
    && !missingSource
    && !busy;
  const applyDisabledReason = busy
    ? "Applying location changes"
    : !hasSelectedLocations
      ? "Select at least one location."
      : missingSource
        ? "One selected skill has no source to link."
        : !hasLocationChanges
          ? "Select a location to add or remove."
          : "";

  const apply = async () => {
    if (!canApply) return;
    setBusy(SkillLocationBusyAction.Apply);
    setError("");
    try {
      let updatedSkills: RawSkillRecord[] = [];
      let deleted: string[] = [];
      if (locationChanges.addedTargetIds.length > 0) {
        const response = await distributeSkills({
          sourcePaths,
          targets: locationChanges.addedTargetIds,
          scope: SkillScope.Global,
          mode: SkillDistributionMode.Symlink,
          dryRun: false,
        });
        updatedSkills = mergeSkillRows(updatedSkills, response.updated ?? response.skills ?? []);
      }
      if (locationChanges.removedTargetIds.length > 0) {
        const response = await removeSkillLocations({
          ids: selectedSkills.map((item) => item.id),
          targets: locationChanges.removedTargetIds,
          scope: SkillScope.Global,
        });
        updatedSkills = mergeSkillRows(updatedSkills, response.updated ?? response.skills ?? []);
        deleted = response.deleted ?? [];
      }
      onOpenChange(false);
      await onApplied(updatedSkills, { patch: true, deleted: deleted.length > 0 ? deleted : undefined });
    } catch (applyError) {
      setError(String(applyError));
    } finally {
      setBusy(SkillLocationBusyAction.Idle);
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
          <MultiSelect
            value={selectedLocationIds}
            disabled={busy === SkillLocationBusyAction.Apply}
            onValueChange={(nextValues) => {
              const next = new Set(nextValues);
              const previous = new Set(selectedLocationIds);
              setTargetOverrides((current) => {
                const updated = { ...current };
                for (const option of visibleTargets) {
                  if (previous.has(option.id) !== next.has(option.id)) updated[option.id] = next.has(option.id);
                }
                return updated;
              });
            }}
          >
            <MultiSelectTrigger className="skillLocationTrigger">
              <MultiSelectValue placeholder="" hidePlaceholderWhenOpen={false}>
                {(value, label) => <AgentOptionLabel agent={value} label={label} />}
              </MultiSelectValue>
            </MultiSelectTrigger>
            <MultiSelectContent className="skillLocationMenu selectControlContent">
              <MultiSelectList ariaLabel="Locations" className="selectViewport">
                {visibleTargets.map((option) => (
                  <MultiSelectItem
                    key={option.id}
                    value={option.id}
                    textValue={option.displayName}
                    keywords={option.globalPath ? [option.globalPath] : []}
                    disabled={busy === SkillLocationBusyAction.Apply}
                    className="menuItem skillLocationMenuItem"
                  >
                    <span className="skillLocationTargetDetails">
                      <AgentOptionLabel agent={option.id} label={option.displayName} />
                      {option.globalPath ? <span className="skillLocationTargetPath">({formatUserPath(option.globalPath)})</span> : null}
                      {selectedSkills.length > 1 && (currentTargetCounts.get(option.id) ?? 0) > 0 ? (
                        <span className="skillLocationTargetCount">
                          {currentTargetCounts.get(option.id)}/{selectedSkills.length}
                        </span>
                      ) : null}
                    </span>
                  </MultiSelectItem>
                ))}
                <MultiSelectEmpty>No locations found.</MultiSelectEmpty>
              </MultiSelectList>
            </MultiSelectContent>
          </MultiSelect>
          {missingSource ? <small className="skillLocationHint">One selected skill has no source to link.</small> : null}
        </div>
        {error ? <Toast message={error} tone="error" /> : null}
      </div>
      <DialogActionBar onCancel={() => onOpenChange(false)}>
        <Tooltip content={applyDisabledReason}>
          <span className="skillLocationApplyTooltipTarget">
            <DialogApplyButton
              label="Apply"
              busyLabel="Applying location changes"
              ariaLabel={applyDisabledReason || "Apply location changes"}
              busy={busy === SkillLocationBusyAction.Apply}
              disabled={!canApply}
              onClick={() => { void apply(); }}
            />
          </span>
        </Tooltip>
      </DialogActionBar>
    </DialogShell>
    </>
  );
}
