import { useEffect, useMemo, useState } from "react";
import { Dialog } from "radix-ui";

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
import { skillDisplayName } from "../../lib/index.ts";
import type { NormalizedSkill, WrapperArgs } from "../../lib/index.ts";
import type { SkillChangeResponse } from "../../lib/runtime-gateway.ts";

export type SkillWrapperScopeDialogProps = {
  open: boolean;
  wrapper?: NormalizedSkill | null;
  skills: NormalizedSkill[];
  onOpenChange: (open: boolean) => void;
  onApplyWrapper: (args: WrapperArgs) => Promise<SkillChangeResponse>;
};

function normalizedSkillName(name: string) {
  return name.trim().toLocaleLowerCase();
}

function sameValues(left: string[], right: string[]) {
  if (left.length !== right.length) return false;
  const rightSet = new Set(right);
  return left.every((value) => rightSet.has(value));
}

export function SkillWrapperScopeDialog({
  open,
  wrapper,
  skills,
  onOpenChange,
  onApplyWrapper,
}: SkillWrapperScopeDialogProps) {
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const availableSkills = useMemo(
    () => skills
      .filter((skill) => skill.id !== wrapper?.id && skill.name !== wrapper?.name)
      .sort((left, right) => skillDisplayName(left).localeCompare(skillDisplayName(right))),
    [skills, wrapper?.id, wrapper?.name],
  );
  const initialSelectedIds = useMemo(() => {
    if (!wrapper) return [];
    const dependencyIds = new Set(wrapper.dependencyIds);
    const dependencyNames = new Set(wrapper.dependencies.map(normalizedSkillName));
    return availableSkills
      .filter((skill) => dependencyIds.has(skill.id) || dependencyNames.has(normalizedSkillName(skill.name)))
      .map((skill) => skill.id);
  }, [availableSkills, wrapper]);
  const initialSelectedIdsKey = initialSelectedIds.join("\u0000");
  const orderedAvailableSkills = useMemo(() => {
    const selected = new Set(selectedIds);
    return [...availableSkills].sort((left, right) => Number(selected.has(right.id)) - Number(selected.has(left.id)));
  }, [availableSkills, selectedIds]);
  const hasScopeChanges = !sameValues(selectedIds, initialSelectedIds);
  const canApply = Boolean(wrapper)
    && selectedIds.length > 0
    && hasScopeChanges
    && !busy;
  const applyDisabledReason = busy
    ? "Applying wrapper scope changes"
    : selectedIds.length === 0
      ? "Select at least one child skill."
      : !hasScopeChanges
        ? "Select a different child skill set."
        : "";

  useEffect(() => {
    if (!open) return;
    setSelectedIds(initialSelectedIds);
    setError("");
    setBusy(false);
  }, [initialSelectedIdsKey, open]);

  const apply = async () => {
    if (!canApply || !wrapper) return;
    setBusy(true);
    setError("");
    try {
      await onApplyWrapper({
        name: wrapper.name,
        skillIds: selectedIds,
        manualChildren: false,
        refresh: true,
      });
      onOpenChange(false);
    } catch (applyError) {
      setError(String(applyError));
    } finally {
      setBusy(false);
    }
  };

  if (!wrapper) return null;

  return (
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => !busy && onOpenChange(nextOpen)}
      className="confirmDialogPanel skillWrapperScopeDialog"
      descriptionId="skill-wrapper-scope-dialog-description"
    >
      <Dialog.Title className="confirmDialogTitle">Manage wrapper scope</Dialog.Title>
      <Dialog.Description id="skill-wrapper-scope-dialog-description" className="dialogVisuallyHidden">
        Choose the child skills routed through this wrapper.
      </Dialog.Description>
      <div className="skillLocationBody">
        <BadgeList items={[skillDisplayName(wrapper)]} ariaLabel="Wrapper skill" active={open} className="skillLocationSkillList" />
        <div className="skillLocationField">
          <span>Child skills</span>
          <MultiSelect
            value={selectedIds}
            disabled={busy}
            onValueChange={setSelectedIds}
          >
            <MultiSelectTrigger className="skillLocationTrigger">
              <MultiSelectValue placeholder="Select child skills" />
            </MultiSelectTrigger>
            <MultiSelectContent>
              <MultiSelectList ariaLabel="Child skills" className="selectViewport">
                {orderedAvailableSkills.map((skill, index) => (
                  <MultiSelectItem
                    key={skill.id}
                    value={skill.id}
                    textValue={skillDisplayName(skill)}
                    keywords={skill.description ? [skill.description] : []}
                    disabled={busy}
                    order={index}
                    className="menuItem skillLocationMenuItem"
                  >
                    {skillDisplayName(skill)}
                  </MultiSelectItem>
                ))}
                <MultiSelectEmpty>No child skills found.</MultiSelectEmpty>
              </MultiSelectList>
            </MultiSelectContent>
          </MultiSelect>
        </div>
        {error ? <Toast message={error} tone="error" /> : null}
      </div>
      <DialogActionBar onCancel={() => onOpenChange(false)}>
        <Tooltip content={applyDisabledReason}>
          <span className="skillLocationApplyTooltipTarget">
            <DialogApplyButton
              label="Apply"
              busyLabel="Applying wrapper scope changes"
              ariaLabel={applyDisabledReason || "Apply wrapper scope changes"}
              busy={busy}
              disabled={!canApply}
              onClick={() => { void apply(); }}
            />
          </span>
        </Tooltip>
      </DialogActionBar>
    </DialogShell>
  );
}
