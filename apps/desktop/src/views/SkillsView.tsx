import { Tooltip } from "../components/shared/Tooltip.tsx";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ComponentType, ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { ContextMenu, Dialog, DropdownMenu, Select, ToggleGroup } from "radix-ui";
import {
  Check,
  ChevronRight,
  Hammer,
  PackagePlus,
  Plus,
  RefreshCw,
  Search,
  Trash2,
} from "lucide-react";

import { AgentChips } from "../components/shared/AgentChips.tsx";
import { AgentOptionLabel } from "../components/shared/AgentOptionLabel.tsx";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogAdvanceButton } from "../components/shared/DialogAdvanceButton.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { MoreActionsButton } from "../components/shared/MoreActionsButton.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { SelectionCheckbox } from "../components/shared/SelectionCheckbox.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { SelectTrigger } from "../components/shared/SelectTrigger.tsx";
import { Visibility } from "../components/shared/Visibility.tsx";
import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import { TauriCommand, SkillVisibility, allSkillVisibilities, copyText, editableSkillVisibilities, isSkillRowSelectable, isSkillSelectable, isSkillVisibilityEditable, primarySkillPath, safeInvoke, skillSourceAction, skillSourceDetails, skillTargets, sourceRemoteDetails, suppressNextClick, targetAgentLabel } from "../lib/index.ts";

export type SkillsTableSort = SortState;

type SkillSection = "Local" | "Remote" | "Plugin" | "System";

type SkillPath = {
  path?: string | null;
  agent?: string | null;
  scope?: string | null;
  source?: string | null;
  source_kind?: string | null;
  source_relative_path?: string | null;
  install_target?: string | null;
  root?: string | null;
};

export type SkillRecord = {
  id: string;
  name: string;
  description?: string;
  section?: SkillSection | string;
  agents?: string[];
  visibility?: SkillVisibility | string;
  isWrapper?: boolean;
  statusTone?: string;
  updateStatus?: string;
  paths?: SkillPath[];
  tags?: string[];
  dependencies?: string[];
  dependents?: string[];
  source?: string;
  source_summary?: string;
  is_system?: boolean;
  isSystem?: boolean;
};

type SkillTableRow = SkillRecord;

type SkillTarget = {
  id: string;
  agent: string;
  label: string;
  path: string;
};

function skillOriginLabel(skill: SkillRecord): string {
  const source = skillSourceDetails(skill);
  const remote = sourceRemoteDetails(source.value, source.kind);
  return remote?.host.includes("github.") && remote.path ? remote.path : skill.section ?? "Local";
}
enum SkillOperationStatus {
  Planned = "planned",
  Ready = "ready",
  AlreadyExists = "already-exists",
  Replace = "replace",
  AlreadyInstalled = "already-installed",
  Unknown = "unknown",
}

type SkillOperation = {
  name: string;
  status?: SkillOperationStatus;
  rawStatus?: string;
  message?: string;
};

type AvailableSkill = {
  name: string;
  description?: string;
  relative_path?: string;
  relativePath?: string;
  dependencies?: string[];
};

type SkillAddPlan = {
  available?: AvailableSkill[];
  selected?: AvailableSkill[];
  source?: string;
  source_kind?: string;
  target?: string;
  operations?: SkillOperation[];
};

export type SkillInstallResult = {
  skills?: SkillRecord[];
  report?: {
    plan?: SkillAddPlan;
    results?: Array<{ target?: string }>;
  };
};

export type WrapperArgs = {
  name: string;
  names: string[];
  description?: string;
  manualChildren: boolean;
  refresh: boolean;
};

function commandErrorMessage(error: unknown, fallback: string) {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return fallback;
}

type SkillMenuComponents = {
  Item: ComponentType<{
    className?: string;
    disabled?: boolean;
    key?: string;
    onSelect?: () => void;
    children?: ReactNode;
  }>;
  Separator: ComponentType<{ className?: string }>;
  Sub: ComponentType<{ children?: ReactNode }>;
  SubTrigger: ComponentType<{ className?: string; children?: ReactNode }>;
  Portal: ComponentType<{ children?: ReactNode }>;
  SubContent: ComponentType<{ className?: string; sideOffset?: number; alignOffset?: number; children?: ReactNode }>;
};

export type VisibilityMenuItemsProps = {
  Menu: SkillMenuComponents;
  selectedSkills: SkillRecord[];
  onSetVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
};

export function VisibilityMenuItems({ Menu, selectedSkills, onSetVisibility }: VisibilityMenuItemsProps) {
  const names = selectedSkills.filter(isSkillVisibilityEditable).map((skill) => skill.name);
  const disabled = names.length === 0;
  return (
    <>
      {editableSkillVisibilities.map((visibility) => (
        <Menu.Item
          className="skillMenuItem"
          disabled={disabled}
          key={visibility}
          onSelect={() => onSetVisibility(names, visibility)}
        >
          {visibility}
        </Menu.Item>
      ))}
    </>
  );
}

export type SkillActionsMenuItemsProps = {
  Menu: SkillMenuComponents;
  skill: SkillRecord;
  onApplyUpdates: (names: string[]) => void;
  onDeleteSkills: (names: string[]) => void;
};

export function SkillActionsMenuItems({ Menu, skill, onApplyUpdates, onDeleteSkills }: SkillActionsMenuItemsProps) {
  const targets = skillTargets(skill);
  const primaryPath = primarySkillPath(skill);
  const hasMultipleTargets = targets.length > 1;
  const readOnly = !isSkillSelectable(skill);
  return (
    <>
      {skill.updateStatus === "update-available" && (
        <>
          <Menu.Item className="skillMenuItem" onSelect={() => {
            suppressNextClick();
            onApplyUpdates([skill.name]);
          }}>
            Apply update
          </Menu.Item>
          <Menu.Separator className="skillMenuSeparator" />
        </>
      )}
      {hasMultipleTargets ? (
        <Menu.Sub>
          <Menu.SubTrigger className="skillMenuItem skillMenuSubTrigger">
            <span>Reveal in Finder</span>
            <ChevronRight className="skillMenuSubIcon" size={14} />
          </Menu.SubTrigger>
          <Menu.Portal>
            <Menu.SubContent className="skillMenuContent" sideOffset={8} alignOffset={-6}>
              {(targets as SkillTarget[]).map((target) => (
                <Menu.Item
                  className="skillMenuItem"
                  key={`reveal-${target.id}`}
                  onSelect={() => safeInvoke(TauriCommand.RevealInFinder, { path: target.path })}
                >
                  <AgentOptionLabel agent={target.agent} label={target.label} />
                </Menu.Item>
              ))}
            </Menu.SubContent>
          </Menu.Portal>
        </Menu.Sub>
      ) : (
        <Menu.Item
          className="skillMenuItem"
          disabled={!primaryPath}
          onSelect={() => {
            if (primaryPath) safeInvoke(TauriCommand.RevealInFinder, { path: primaryPath });
          }}
        >
          Reveal in Finder
        </Menu.Item>
      )}
      <Menu.Separator className="skillMenuSeparator" />
      {hasMultipleTargets ? (
        <Menu.Sub>
          <Menu.SubTrigger className="skillMenuItem skillMenuSubTrigger">
            <span>Copy path</span>
            <ChevronRight className="skillMenuSubIcon" size={14} />
          </Menu.SubTrigger>
          <Menu.Portal>
            <Menu.SubContent className="skillMenuContent" sideOffset={8} alignOffset={-6}>
              {(targets as SkillTarget[]).map((target) => (
                <Menu.Item className="skillMenuItem" key={`copy-${target.id}`} onSelect={() => copyText(target.path)}>
                  <AgentOptionLabel agent={target.agent} label={target.label} />
                </Menu.Item>
              ))}
            </Menu.SubContent>
          </Menu.Portal>
        </Menu.Sub>
      ) : (
        <Menu.Item className="skillMenuItem" disabled={!primaryPath} onSelect={() => copyText(primaryPath ?? "")}>
          Copy path
        </Menu.Item>
      )}
      {!readOnly && (
        <>
          <Menu.Separator className="skillMenuSeparator" />
          <Menu.Item
            className="skillMenuItem danger"
            onSelect={() => {
              suppressNextClick();
              onDeleteSkills([skill.name]);
            }}
          >
            <Trash2 size={14} />
            Delete skill
          </Menu.Item>
        </>
      )}
    </>
  );
}

export type BulkSkillActionsMenuItemsProps = {
  Menu: SkillMenuComponents;
  selectedSkills: SkillRecord[];
  onApplyUpdates: (names: string[]) => void;
  onDeleteSkills: (names: string[]) => void;
};

export function BulkSkillActionsMenuItems({ Menu, selectedSkills, onApplyUpdates, onDeleteSkills }: BulkSkillActionsMenuItemsProps) {
  const updateNames = selectedSkills
    .filter((skill) => skill.updateStatus === "update-available")
    .map((skill) => skill.name);
  const deletableNames = selectedSkills
    .filter((skill) => skill.statusTone !== "muted")
    .map((skill) => skill.name);
  return (
    <>
      {updateNames.length > 0 && (
        <Menu.Item className="skillMenuItem" onSelect={() => {
          suppressNextClick();
          onApplyUpdates(updateNames);
        }}>
          Update
        </Menu.Item>
      )}
      {updateNames.length > 0 && <Menu.Separator className="skillMenuSeparator" />}
      <Menu.Item
        className="skillMenuItem danger"
        disabled={deletableNames.length === 0}
        onSelect={() => {
          suppressNextClick();
          onDeleteSkills(deletableNames);
        }}
      >
        <Trash2 size={14} />
        Delete selected
      </Menu.Item>
    </>
  );
}

export type SkillMainCellProps = {
  skill: SkillRecord;
  openSkill: (skill: SkillRecord) => void;
  onApplyUpdates: (names: string[]) => void;
};

export function SkillMainCell({ skill, openSkill, onApplyUpdates }: SkillMainCellProps) {
  const sourceDetails = skillSourceDetails(skill);
  const sourceAction = skillSourceAction(skill, sourceDetails);
  return (
    <div className="skillMain">
      <div className="skillTitleRow">
        <button
          className="skillOpen skillTitleOpen"
          onClick={(event) => {
            event.stopPropagation();
            openSkill(skill);
          }}
        >
          <span className="skillNameText">{skill.name}</span>
          {skill.isWrapper && <em>wrapper</em>}
        </button>
        {skill.updateStatus === "update-available" && (
          <button
            type="button"
            className="skillUpdateBadge"
            aria-label={`View update for ${skill.name}`}
            onClick={(event) => {
              event.stopPropagation();
              onApplyUpdates([skill.name]);
            }}
          >
            Update
          </button>
        )}
        {sourceAction && (
          <Tooltip content={sourceAction.title}><button
            className="skillSourceButton"
            aria-label={sourceAction.ariaLabel}
            onClick={(event) => {
              event.stopPropagation();
              sourceAction.onClick();
            }}
          >
            {sourceAction.icon}
          </button></Tooltip>
        )}
      </div>
      <button
        className="skillOpen skillDescriptionOpen"
        onClick={(event) => {
          event.stopPropagation();
          openSkill(skill);
        }}
      >
        <div className="skillDescription">{skill.description}</div>
      </button>
    </div>
  );
}

export type SkillActionsCellProps = {
  skill: SkillRecord;
  onApplyUpdates: (names: string[]) => void;
  onDeleteSkills: (names: string[]) => void;
};

export function SkillActionsCell({ skill, onApplyUpdates, onDeleteSkills }: SkillActionsCellProps) {
  return (
    <DropdownMenu.Root onOpenChange={(open) => { if (!open) suppressNextClick(); }}>
      <DropdownMenu.Trigger asChild>
        <MoreActionsButton aria-label={`Skill actions for ${skill.name}`} />
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="skillMenuContent" align="end" sideOffset={6}>
          <SkillActionsMenuItems Menu={DropdownMenu} skill={skill} onApplyUpdates={onApplyUpdates} onDeleteSkills={onDeleteSkills} />
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}

export function suggestedWrapperName(selectedSkills: SkillRecord[]) {
  const names = selectedSkills.map((skill) => skill.name).filter(Boolean);
  if (names.length === 0) return "wrapper";
  const prefixes = names
    .map((name) => name.split(/[-_]/)[0])
    .filter((prefix) => prefix.length > 1);
  return prefixes.length > 0 && prefixes.every((prefix) => prefix === prefixes[0]) ? prefixes[0] : "wrapper";
}

function childSkillSummary(selectedSkills: SkillRecord[]) {
  const names = selectedSkills.map((skill) => skill.name).filter(Boolean);
  if (names.length === 0) return "the selected child skills";
  const visibleNames = names.slice(0, 4);
  const suffix = names.length > visibleNames.length ? `, and ${names.length - visibleNames.length} more` : "";
  return `${visibleNames.join(", ")}${suffix}`;
}

export function suggestedWrapperDescription(name: string, selectedSkills: SkillRecord[] = []) {
  const domain = name.trim().replace(/[-_]+/g, " ") || "selected";
  return `Use when the request is about ${domain} and matches one of these child skills: ${childSkillSummary(selectedSkills)}.`;
}

export type WrapperDialogProps = {
  open: boolean;
  selectedSkills: SkillRecord[];
  onOpenChange: (open: boolean) => void;
  onApplyWrapper: (args: WrapperArgs) => Promise<unknown>;
};

export function WrapperDialog({ open, selectedSkills, onOpenChange, onApplyWrapper }: WrapperDialogProps) {
  const [name, setName] = useState("lark");
  const [description, setDescription] = useState("");
  const [descriptionEdited, setDescriptionEdited] = useState(false);
  const [manualChildren, setManualChildren] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const selectedNames = useMemo(() => selectedSkills.map((skill) => skill.name), [selectedSkills]);
  const canCreate = name.trim() && description.trim() && selectedNames.length > 0;

  useEffect(() => {
    if (!open) return;
    const nextName = suggestedWrapperName(selectedSkills);
    setName(nextName);
    setDescription(suggestedWrapperDescription(nextName, selectedSkills));
    setDescriptionEdited(false);
    setManualChildren(true);
    setError("");
    setBusy(false);
  }, [open, selectedSkills]);

  useEffect(() => {
    setError("");
  }, [description, manualChildren, name, selectedNames]);

  const updateName = (value: string) => {
    setName(value);
    if (!descriptionEdited) setDescription(suggestedWrapperDescription(value, selectedSkills));
  };

  const updateDescription = (value: string) => {
    setDescription(value);
    setDescriptionEdited(true);
  };

  const args = useMemo((): WrapperArgs => ({
    name: name.trim(),
    names: selectedNames,
    description: description.trim(),
    manualChildren,
    refresh: false,
  }), [description, manualChildren, name, selectedNames]);

  const apply = async () => {
    if (!canCreate || busy) return;
    setBusy(true);
    setError("");
    let result: unknown;
    try {
      result = await onApplyWrapper(args);
    } catch (error) {
      setBusy(false);
      setError(commandErrorMessage(error, "Could not create the wrapper skill."));
      return;
    }
    setBusy(false);
    if (result) onOpenChange(false);
    else setError("Could not create the wrapper skill.");
  };

  return (
    <Dialog.Root open={open} onOpenChange={(nextOpen) => !busy && onOpenChange(nextOpen)}>
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content className="wrapperDialogPanel" data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <Dialog.Title className="confirmDialogTitle">Create wrapper skill</Dialog.Title>
          <div className="wrapperDialogBody">
            <DialogTextField label="Name" value={name} onChange={updateName} placeholder="" />
            <label className="dialogField">
              <span>Description</span>
              <textarea
                autoFocus
                className="dialogTextArea wrapperDescriptionInput"
                value={description}
                onChange={(event) => updateDescription(event.target.value)}
              />
            </label>
            <label className={`addSkillReplaceField ${manualChildren ? "isChecked" : ""}`}>
              <SelectionCheckbox
                checked={manualChildren}
                label="Only trigger selected skills through this wrapper"
                onChange={(checked: boolean | "indeterminate") => setManualChildren(Boolean(checked))}
              />
              <span>
                <strong>Make children manual</strong>
                <small>Selected skills will stop triggering on their own.</small>
              </span>
            </label>
            {error ? <div className="wrapperDialogError">{error}</div> : null}
          </div>
          <DialogActionBar cancelDisabled={busy} onCancel={() => onOpenChange(false)}>
            <DialogAdvanceButton
              label="Create"
              ariaLabel="Create wrapper skill"
              busy={busy}
              disabled={!canCreate}
              onClick={apply}
            />
          </DialogActionBar>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
  );
}

function normalizeSkillOperationStatus(status: unknown): SkillOperationStatus | undefined {
  if (!status) return undefined;
  if (status === SkillOperationStatus.Planned) return SkillOperationStatus.Planned;
  if (status === SkillOperationStatus.Ready) return SkillOperationStatus.Ready;
  if (status === SkillOperationStatus.AlreadyExists) return SkillOperationStatus.AlreadyExists;
  if (status === SkillOperationStatus.Replace) return SkillOperationStatus.Replace;
  if (status === SkillOperationStatus.AlreadyInstalled) return SkillOperationStatus.AlreadyInstalled;
  return SkillOperationStatus.Unknown;
}

function normalizeSkillAddPlan(plan: SkillAddPlan | null | undefined): SkillAddPlan | null {
  if (!plan) return null;
  return {
    ...plan,
    operations: (plan.operations ?? []).map((operation) => ({
      ...operation,
      rawStatus: operation.rawStatus ?? `${operation.status ?? ""}`,
      status: normalizeSkillOperationStatus(operation.status),
    })),
  };
}

function skillOperationStatusLabel(status: SkillOperationStatus | undefined) {
  if (status === SkillOperationStatus.AlreadyInstalled) return "Installed";
  if (status === SkillOperationStatus.AlreadyExists) return "Exists";
  if (status === SkillOperationStatus.Replace) return "Replace";
  return "";
}

export function selectedExistingSkillOperation(
  operation: SkillOperation | undefined,
  selected: boolean,
): SkillOperation | undefined {
  if (!operation || !selected || operation.status !== SkillOperationStatus.AlreadyExists) return operation;
  return {
    ...operation,
    status: SkillOperationStatus.Replace,
    message: operation.message?.startsWith("target already exists:")
      ? operation.message.replace("target already exists:", "will replace existing target:")
      : operation.message,
  };
}

export function isNewSkillOperationStatus(status: SkillOperationStatus | undefined) {
  return !status || status === SkillOperationStatus.Planned || status === SkillOperationStatus.Ready;
}

export function isExistingSkillOperationStatus(status: SkillOperationStatus | undefined) {
  return status === SkillOperationStatus.AlreadyExists || status === SkillOperationStatus.Replace;
}

export function isSelectableOperationStatus(status: SkillOperationStatus | undefined) {
  return isNewSkillOperationStatus(status) || isExistingSkillOperationStatus(status);
}

function expandSelectedSkillNames(
  selectedNames: string[],
  dependencyByName: Map<string, string[]>,
  selectableNames: Set<string>,
) {
  const expanded = new Set<string>();
  const stack = [...selectedNames].filter((name) => selectableNames.has(name));
  while (stack.length > 0) {
    const name = stack.pop();
    if (!name || expanded.has(name) || !selectableNames.has(name)) continue;
    expanded.add(name);
    for (const dependency of dependencyByName.get(name) ?? []) {
      if (!expanded.has(dependency)) stack.push(dependency);
    }
  }
  return [...expanded].sort((left, right) => left.localeCompare(right));
}

function dependencyReasonsFor(
  name: string,
  selectedRoots: Set<string>,
  dependencyByName: Map<string, string[]>,
  selectableNames: Set<string>,
) {
  const reasons = new Set<string>();
  for (const root of selectedRoots) {
    if (root === name) continue;
    const expanded = expandSelectedSkillNames([root], dependencyByName, selectableNames);
    if (expanded.includes(name)) reasons.add(root);
  }
  return [...reasons].sort((left, right) => left.localeCompare(right));
}

function availableSkillSearchText(skill: AvailableSkill) {
  return [
    skill.name,
    skill.description,
    skill.relative_path,
    skill.relativePath,
    ...(skill.dependencies ?? []),
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
}

export type AddSkillDialogProps = {
  onClose: () => void;
  onInstalled: (result: SkillInstallResult) => void;
  onRequestWrapper: (skills: SkillRecord[]) => void;
};

type InstallTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
};

export function AddSkillDialog({ onClose, onInstalled, onRequestWrapper }: AddSkillDialogProps) {
  const [source, setSource] = useState("");
  const [target, setTarget] = useState("shared");
  const [scope, setScope] = useState("global");
  const [installTargets, setInstallTargets] = useState<InstallTargetOption[]>([
    { id: "shared", displayName: "Shared", supportsGlobal: true },
  ]);
  const [copy, setCopy] = useState(false);
  const [visibility, setVisibility] = useState<SkillVisibility>(SkillVisibility.Auto);
  const [plan, setPlan] = useState<SkillAddPlan | null>(null);
  const [previewId, setPreviewId] = useState("");
  const [selectedRoots, setSelectedRoots] = useState<string[]>([]);
  const [createWrapper, setCreateWrapper] = useState(false);
  const [skillSearch, setSkillSearch] = useState("");
  const [busyAction, setBusyAction] = useState("");
  const [error, setError] = useState("");
  const skillItemRefs = useRef(new Map<string, HTMLDivElement>());
  const busy = busyAction !== "";
  const visibleInstallTargets = useMemo(
    () => installTargets.filter((option) => scope === "project" || option.supportsGlobal),
    [installTargets, scope],
  );
  const targetDisplayName = installTargets.find((option) => option.id === target)?.displayName
    ?? targetAgentLabel(target);
  const available = plan?.available ?? [];
  const showSkillSearch = available.length > 10;
  const normalizedSkillSearch = skillSearch.trim().toLowerCase();
  const dependencyByName = useMemo(
    () => new Map(available.map((skill) => [skill.name, skill.dependencies ?? []])),
    [available],
  );
  const rawOperationByName = useMemo(
    () => new Map((plan?.operations ?? []).map((operation) => [operation.name, operation])),
    [plan],
  );
  const selectableSkills = useMemo(
    () => available.filter((skill) => {
      const status = rawOperationByName.get(skill.name)?.status;
      return isSelectableOperationStatus(status);
    }),
    [available, rawOperationByName],
  );
  const selectableNameSet = useMemo(
    () => new Set(selectableSkills.map((skill) => skill.name)),
    [selectableSkills],
  );
  const selected = useMemo(
    () => expandSelectedSkillNames(selectedRoots, dependencyByName, selectableNameSet),
    [dependencyByName, selectableNameSet, selectedRoots],
  );
  const selectedSet = useMemo(() => new Set(selected), [selected]);
  const operationByName = useMemo(
    () => new Map((plan?.operations ?? []).map((operation) => [
      operation.name,
      selectedExistingSkillOperation(operation, selectedSet.has(operation.name)),
    ])),
    [plan, selectedSet],
  );
  const selectedRootSet = useMemo(() => new Set(selectedRoots), [selectedRoots]);
  const dependencyReasonsByName = useMemo(
    () => new Map(available.map((skill) => [
      skill.name,
      dependencyReasonsFor(skill.name, selectedRootSet, dependencyByName, selectableNameSet),
    ])),
    [available, dependencyByName, selectableNameSet, selectedRootSet],
  );
  const newSkills = useMemo(
    () => available.filter((skill) => isNewSkillOperationStatus(rawOperationByName.get(skill.name)?.status)),
    [available, rawOperationByName],
  );
  const existingSkills = useMemo(
    () => available.filter((skill) => isExistingSkillOperationStatus(rawOperationByName.get(skill.name)?.status)),
    [available, rawOperationByName],
  );
  const selectedHasExisting = useMemo(
    () => selected.some((name) => isExistingSkillOperationStatus(rawOperationByName.get(name)?.status)),
    [rawOperationByName, selected],
  );
  const allSelected = selectableSkills.length > 0 && selectableSkills.every((skill) => selectedSet.has(skill.name));
  const mixedSelected = selected.length > 0 && !allSelected;
  const newSelected = newSkills.length > 0 && newSkills.every((skill) => selectedSet.has(skill.name));
  const existingSelected = existingSkills.length > 0 && existingSkills.every((skill) => selectedSet.has(skill.name));
  const dependencySelectedCount = selected.filter((name) => !selectedRootSet.has(name)).length;
  const searchMatches = useMemo(() => {
    if (!normalizedSkillSearch) return [];
    return available
      .filter((skill) => availableSkillSearchText(skill).includes(normalizedSkillSearch))
      .map((skill) => skill.name);
  }, [available, normalizedSkillSearch]);
  const searchMatchSet = useMemo(() => new Set(searchMatches), [searchMatches]);
  const firstSearchMatch = searchMatches[0] ?? "";
  const canPreview = source.trim().length > 0 && !busy;
  const canInstall = canPreview && selected.length > 0 && plan;
  const canAdvance = plan ? canInstall : canPreview;
  const advanceLabel = busyAction === "preview"
    ? "Previewing skills"
    : busyAction === "install"
      ? "Installing selected skills"
      : plan ? "Install selected skills" : "Preview skills";
  const advanceText = busyAction === "preview" ? "Loading" : busyAction === "install" ? "Installing" : plan ? "Install" : "Next";

  const preview = async () => {
    if (!source.trim()) return;
    setBusyAction("preview");
    setError("");
    try {
      const response = await invoke(TauriCommand.SkillsAdd, {
        source: source.trim(),
        target,
        scope,
        skills: [],
        copy,
        overwrite: false,
        visibility: visibility.toLowerCase(),
        dryRun: true,
      }) as { plan?: SkillAddPlan; previewId?: string } | null;
      const nextPlan = normalizeSkillAddPlan(response?.plan);
      const nextOperations = new Map((nextPlan?.operations ?? []).map((operation) => [operation.name, operation]));
      setPlan(nextPlan);
      setPreviewId(response?.previewId ?? "");
      setCreateWrapper(false);
      setSkillSearch("");
      setSelectedRoots((nextPlan?.available ?? [])
        .filter((skill) => {
          const status = nextOperations.get(skill.name)?.status;
          return isNewSkillOperationStatus(status);
        })
        .map((skill) => skill.name));
    } catch (previewError) {
      setPlan(null);
      setPreviewId("");
      setSelectedRoots([]);
      setError(`${previewError}`);
    } finally {
      setBusyAction("");
    }
  };

  const install = async () => {
    if (!canInstall) return;
    setBusyAction("install");
    setError("");
    try {
      const result = await invoke<SkillInstallResult>(TauriCommand.SkillsAdd, {
        source: source.trim(),
        target,
        scope,
        skills: selected,
        copy,
        overwrite: selectedHasExisting,
        visibility: visibility.toLowerCase(),
        previewId,
        dryRun: false,
      });
      onInstalled(result);
      if (createWrapper && selectedRoots.length > 1) {
        const selectedRootSet = new Set(selectedRoots);
        onRequestWrapper(available
          .filter((skill) => selectedRootSet.has(skill.name))
          .map((skill) => ({
            id: skill.name,
            name: skill.name,
            description: skill.description,
          })));
      }
      onClose();
    } catch (installError) {
      setError(`${installError}`);
    } finally {
      setBusyAction("");
    }
  };

  const advance = () => {
    if (plan) install();
    else preview();
  };

  useEffect(() => {
    void safeInvoke<InstallTargetOption[]>(TauriCommand.SkillsTargets).then((targets) => {
      if (!targets?.length) return;
      setInstallTargets([
        { id: "shared", displayName: "Shared", supportsGlobal: true },
        ...targets.filter((option) => option.id !== "shared"),
      ]);
    });
  }, []);

  useEffect(() => {
    if (!firstSearchMatch) return;
    skillItemRefs.current.get(firstSearchMatch)?.scrollIntoView({
      block: "nearest",
      behavior: "smooth",
    });
  }, [firstSearchMatch]);

  const toggleSkill = (name: string) => {
    const status = operationByName.get(name)?.status;
    if (!isSelectableOperationStatus(status)) return;
    const requiredBy = dependencyReasonsByName.get(name) ?? [];
    if (selectedSet.has(name) && !selectedRootSet.has(name) && requiredBy.length > 0) return;
    setSelectedRoots((current) => current.includes(name) ? current.filter((item) => item !== name) : [...current, name]);
  };

  const toggleAll = () => {
    setSelectedRoots(allSelected ? [] : selectableSkills.map((skill) => skill.name));
  };

  const toggleSkillGroup = (skills: AvailableSkill[]) => {
    if (skills.length === 0) return;
    setSelectedRoots((current) => {
      const currentSet = new Set(current);
      const groupNames = new Set(skills.map((skill) => skill.name));
      const groupSelected = skills.every((skill) => selectedSet.has(skill.name));
      for (const name of groupNames) {
        if (groupSelected) currentSet.delete(name);
        else currentSet.add(name);
      }
      return selectableSkills
        .filter((skill) => currentSet.has(skill.name))
        .map((skill) => skill.name);
    });
  };

  return (
    <Dialog.Portal>
      <Dialog.Overlay className="dialogOverlay" />
      <Dialog.Content className="addSkillPanel" aria-describedby={undefined} data-no-drag onMouseDown={(event) => event.stopPropagation()}>
      <Dialog.Title className="addSkillTitle">Add skill</Dialog.Title>
      <div className="addSkillGrid">
        <div className="sourceField">
          <DialogTextField
            label="Source"
            value={source}
            onChange={(nextSource: string) => {
              setSource(nextSource);
              setPlan(null);
              setSelectedRoots([]);
              setCreateWrapper(false);
              setSkillSearch("");
              setError("");
            }}
            placeholder="owner/repo, Git URL, or local path"
          />
        </div>
        <div className="dialogField">
          <span>Target</span>
          <Select.Root
            value={target}
            onValueChange={(value) => {
              setTarget(value);
              setPlan(null);
              setSelectedRoots([]);
              setCreateWrapper(false);
              setSkillSearch("");
            }}
          >
            <SelectTrigger className="addSkillSelectTrigger" label="Skill install target">
              <Select.Value>
                <AgentOptionLabel agent={target} label={targetDisplayName} />
              </Select.Value>
            </SelectTrigger>
            <Select.Portal>
              <Select.Content
                className="skillMenuContent addSkillSelectContent"
                position="popper"
                side="bottom"
                align="start"
                sideOffset={6}
              >
                <Select.Viewport>
                  {visibleInstallTargets.map((option) => (
                    <Select.Item className="skillMenuItem" value={option.id} key={option.id}>
                      <Select.ItemText>
                        <AgentOptionLabel agent={option.id} label={option.displayName} />
                      </Select.ItemText>
                      <Select.ItemIndicator className="selectItemIndicator">
                        <Check size={13} strokeWidth={2.6} />
                      </Select.ItemIndicator>
                    </Select.Item>
                  ))}
                </Select.Viewport>
              </Select.Content>
            </Select.Portal>
          </Select.Root>
        </div>
        <div className="dialogField">
          <span>Scope</span>
          <Select.Root
            value={scope}
            onValueChange={(value) => {
              setScope(value);
              const selected = installTargets.find((option) => option.id === target);
              if (value === "global" && selected && !selected.supportsGlobal) setTarget("shared");
              setPlan(null);
              setSelectedRoots([]);
              setCreateWrapper(false);
              setSkillSearch("");
            }}
          >
            <SelectTrigger className="addSkillSelectTrigger" label="Skill install scope">
              <Select.Value>{scope}</Select.Value>
            </SelectTrigger>
            <Select.Portal>
              <Select.Content className="skillMenuContent addSkillSelectContent" position="popper" side="bottom" align="start" sideOffset={6}>
                <Select.Viewport>
                  {["global", "project"].map((value) => (
                    <Select.Item className="skillMenuItem" value={value} key={value}>
                      <Select.ItemText>{value}</Select.ItemText>
                      <Select.ItemIndicator className="selectItemIndicator"><Check size={13} strokeWidth={2.6} /></Select.ItemIndicator>
                    </Select.Item>
                  ))}
                </Select.Viewport>
              </Select.Content>
            </Select.Portal>
          </Select.Root>
        </div>
        <div className="dialogField">
          <span>Visibility</span>
          <Select.Root
            value={visibility}
            onValueChange={(value) => {
              setVisibility(value as SkillVisibility);
              setPlan(null);
              setSelectedRoots([]);
              setCreateWrapper(false);
              setSkillSearch("");
            }}
          >
            <SelectTrigger className="addSkillSelectTrigger" label="Skill visibility">
              <Select.Value>{visibility}</Select.Value>
            </SelectTrigger>
            <Select.Portal>
              <Select.Content
                className="skillMenuContent addSkillSelectContent"
                position="popper"
                side="bottom"
                align="start"
                sideOffset={6}
              >
                <Select.Viewport>
                  {editableSkillVisibilities.map((option) => (
                    <Select.Item className="skillMenuItem" value={option} key={option}>
                      <Select.ItemText>{option}</Select.ItemText>
                      <Select.ItemIndicator className="selectItemIndicator">
                        <Check size={13} strokeWidth={2.6} />
                      </Select.ItemIndicator>
                    </Select.Item>
                  ))}
                </Select.Viewport>
              </Select.Content>
            </Select.Portal>
          </Select.Root>
        </div>
      <div className="addSkillModeField">
        <span>Mode</span>
        <ToggleGroup.Root
          className="addSkillModeControl"
          type="single"
          value={copy ? "copy" : "symlink"}
          onValueChange={(value) => {
            if (!value) return;
            setCopy(value === "copy");
            setPlan(null);
            setSelectedRoots([]);
            setCreateWrapper(false);
            setSkillSearch("");
          }}
          aria-label="Skill install mode"
        >
          <ToggleGroup.Item className="addSkillModeItem" value="symlink" aria-label="Install using symlink">
            Symlink
          </ToggleGroup.Item>
          <ToggleGroup.Item className="addSkillModeItem" value="copy" aria-label="Install by copying files">
            Copy
          </ToggleGroup.Item>
        </ToggleGroup.Root>
      </div>
      </div>
      {error && <div className="addSkillError" data-no-drag data-selectable-text>{error}</div>}
      {plan && (
        <div className="addSkillResults">
          <div className="addSkillSummary">
            <span>{available.length === 1 ? "1 skill found" : `${available.length} skills found`}</span>
            {dependencySelectedCount > 0 && (
              <span>{dependencySelectedCount === 1 ? "1 dependency selected" : `${dependencySelectedCount} dependencies selected`}</span>
            )}
          </div>
          <div className="addSkillSelectionBar">
            <div
              className="addSkillSelectAll"
              role="button"
              tabIndex={0}
              onClick={(event) => {
                if ((event.target as Element).closest(".selectionCheckbox")) return;
                toggleAll();
              }}
              onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") toggleAll(); }}
            >
              <SelectionCheckbox
                checked={allSelected}
                mixed={mixedSelected}
                label="Select all installable skills"
                disabled={selectableSkills.length === 0}
                onChange={toggleAll}
              />
              <span>Select all</span>
            </div>
            <div className="addSkillQuickSelect" aria-label="Skill selection toggles">
              <button
                type="button"
                className={newSelected ? "isActive" : ""}
                aria-pressed={newSelected}
                onClick={() => toggleSkillGroup(newSkills)}
                disabled={newSkills.length === 0}
              >
                New
              </button>
              <button
                type="button"
                className={existingSelected ? "isActive" : ""}
                aria-pressed={existingSelected}
                onClick={() => toggleSkillGroup(existingSkills)}
                disabled={existingSkills.length === 0}
              >
                Existing
              </button>
            </div>
          </div>
          {showSkillSearch && (
            <div className="addSkillSearch">
              <Search size={14} />
              <input
                value={skillSearch}
                onChange={(event) => setSkillSearch(event.target.value)}
                placeholder="Search skills"
                aria-label="Search available skills"
              />
              {normalizedSkillSearch && (
                <span>{searchMatches.length === 1 ? "1 match" : `${searchMatches.length} matches`}</span>
              )}
            </div>
          )}
          <div className="addSkillList">
            {available.map((skill) => {
              const operation = operationByName.get(skill.name);
              const blocked = !isSelectableOperationStatus(operation?.status);
              const dependencyNames = skill.dependencies ?? [];
              const requiredBy = dependencyReasonsByName.get(skill.name) ?? [];
              const lockedDependency = requiredBy.length > 0 && !selectedRootSet.has(skill.name);
              const statusLabel = skillOperationStatusLabel(operation?.status);
              const searchMatch = normalizedSkillSearch && searchMatchSet.has(skill.name);
              const firstMatch = skill.name === firstSearchMatch;
              return (
              <div
                className={`addSkillItem ${selectedSet.has(skill.name) ? "selected" : ""} ${blocked || lockedDependency ? "blocked" : ""} ${searchMatch ? "searchMatch" : ""} ${firstMatch ? "firstSearchMatch" : ""}`}
                key={`${skill.name}-${skill.relative_path ?? skill.relativePath ?? ""}`}
                ref={(node) => {
                  if (node) skillItemRefs.current.set(skill.name, node);
                  else skillItemRefs.current.delete(skill.name);
                }}
                role="button"
                tabIndex={blocked || lockedDependency ? -1 : 0}
                onClick={(event) => {
                  if ((event.target as Element).closest(".selectionCheckbox")) return;
                  toggleSkill(skill.name);
                }}
                onKeyDown={(event) => { if (event.key === "Enter" || event.key === " ") toggleSkill(skill.name); }}
              >
                <SelectionCheckbox
                  checked={selectedSet.has(skill.name)}
                  label={`Select ${skill.name}`}
                  disabled={blocked || lockedDependency}
                  onChange={() => toggleSkill(skill.name)}
                />
                <span>
                  <strong>
                    {skill.name}
                    {statusLabel && <em>{statusLabel}</em>}
                    {requiredBy.length > 0 && <em>Required</em>}
                  </strong>
                  <small>{skill.description || skill.relative_path || skill.relativePath || "No description"}</small>
                  {(dependencyNames.length > 0 || requiredBy.length > 0) && (
                    <small className="addSkillDependencies">
                      {dependencyNames.length > 0 ? `Depends on ${dependencyNames.join(", ")}` : `Required by ${requiredBy.join(", ")}`}
                    </small>
                  )}
                </span>
              </div>
            );})}
          </div>
          {selectedRoots.length > 1 && (
            <label className={`addSkillReplaceField ${createWrapper ? "isChecked" : ""}`}>
              <SelectionCheckbox
                checked={createWrapper}
                label="Create a wrapper skill after installation"
                onChange={(checked: boolean | "indeterminate") => setCreateWrapper(Boolean(checked))}
              />
              <span>
                <strong>Create wrapper skill after install</strong>
                <small>Continue to name a wrapper for the selected skills.</small>
              </span>
            </label>
          )}
        </div>
      )}
      <DialogActionBar onCancel={onClose}>
        <DialogAdvanceButton
          label={advanceText}
          ariaLabel={advanceLabel}
          busy={busy}
          disabled={!canAdvance}
          onClick={advance}
        />
      </DialogActionBar>
      </Dialog.Content>
    </Dialog.Portal>
  );
}

export type SkillsViewProps = {
  openSkill: (skill: SkillRecord) => void;
  skills: SkillRecord[];
  loadingSkills: boolean;
  checkingUpdates: boolean;
  applyingUpdates: boolean;
  updateError: string;
  onRefresh: () => void | Promise<void>;
  onSetVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
  onApplyWrapper: (args: WrapperArgs) => Promise<unknown>;
  onApplyUpdates: (names: string[], onApplied?: () => void) => void;
  onDeleteSkills: (names: string[], onApplied?: () => void) => void;
  onAddInstalled: (result: SkillInstallResult) => void;
};

export function SkillsView({
  openSkill,
  skills: skillItems,
  loadingSkills,
  checkingUpdates,
  applyingUpdates,
  updateError,
  onRefresh,
  onSetVisibility,
  onApplyWrapper,
  onApplyUpdates,
  onDeleteSkills,
  onAddInstalled,
}: SkillsViewProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [showWrapper, setShowWrapper] = useState(false);
  const [showAddSkill, setShowAddSkill] = useState(false);
  const [installedWrapperSkills, setInstalledWrapperSkills] = useState<SkillRecord[]>([]);
  const normalizedQuery = query.trim().toLowerCase();
  const visibleSkills = useMemo(() => {
    if (!normalizedQuery) return skillItems;
    return skillItems.filter((skill) => [skill.name, skill.description].some((value) => `${value ?? ""}`.toLowerCase().includes(normalizedQuery)));
  }, [normalizedQuery, skillItems]);
  const tableRows = loadingSkills ? [] : visibleSkills;
  const selectedSkills = useMemo(() => skillItems.filter((skill) => selected.includes(skill.id)), [selected, skillItems]);

  useEffect(() => {
    setSelected((current) => current.filter((id) => {
      const skill = skillItems.find((item) => item.id === id);
      return skill && isSkillRowSelectable(skill);
    }));
  }, [skillItems]);

  const clearSelection = useCallback(() => {
    setSelected([]);
    setShowWrapper(false);
    setInstalledWrapperSkills([]);
  }, []);
  const setVisibilityAndClear = useCallback(async (names: string[], visibility: SkillVisibility) => {
    await onSetVisibility(names, visibility);
    clearSelection();
  }, [clearSelection, onSetVisibility]);
  const applyWrapperAndClear = useCallback(async (args: WrapperArgs) => {
    const result = await onApplyWrapper(args);
    if (result) clearSelection();
    return result;
  }, [clearSelection, onApplyWrapper]);
  const applyUpdatesAndClear = useCallback((names: string[]) => {
    onApplyUpdates(names, clearSelection);
  }, [clearSelection, onApplyUpdates]);
  const deleteSkillsAndClear = useCallback((names: string[]) => {
    onDeleteSkills(names, clearSelection);
  }, [clearSelection, onDeleteSkills]);

  const columns = useMemo((): ColumnDef<SkillTableRow>[] => [
    {
      key: "main",
      header: "Skill",
      type: "text",
      sortValue: (skill) => skill.name?.toLowerCase() ?? "",
      width: "minmax(250px, 1fr)",
      render: (skill) => <SkillMainCell skill={skill} openSkill={openSkill} onApplyUpdates={applyUpdatesAndClear} />,
    },
    {
      key: "origin",
      header: "Origin",
      type: "enum",
      groupBy: (skill) => skill.section ?? "",
      groupOrder: ["Local", "Remote", "Plugin", "System"],
      sortValue: (skill) => skillOriginLabel(skill).toLowerCase(),
      width: "180px",
      value: skillOriginLabel,
    },
    {
      key: "agents",
      header: "Agents",
      type: "enum",
      groupBy: (skill) => (skill.agents ?? []).join(", ") || "None",
      sortValue: (skill) => (skill.agents ?? []).join(",").toLowerCase(),
      width: "120px",
      render: (skill) => <AgentChips agents={skill.agents} />,
    },
    {
      key: "visibility",
      header: "Visibility",
      type: "enum",
      groupOrder: [...allSkillVisibilities],
      sortValue: (skill) => skill.visibility?.toLowerCase() ?? "",
      width: "176px",
      render: (skill) => <Visibility value={skill.visibility ?? SkillVisibility.Auto} skill={skill} onSetVisibility={setVisibilityAndClear} />,
    },
    {
      key: "actions",
      header: "",
      width: "40px",
      render: (skill) => <SkillActionsCell skill={skill} onApplyUpdates={applyUpdatesAndClear} onDeleteSkills={deleteSkillsAndClear} />,
    },
  ], [applyUpdatesAndClear, deleteSkillsAndClear, openSkill, setVisibilityAndClear]);

  const rowContextMenu = useCallback((skill: SkillTableRow, { selectedRows, selected: isSelected }: { selectedRows: SkillTableRow[]; selected: boolean }) => {
    const showBulk = isSelected && selectedRows.length > 1;
    return showBulk ? (
      <BulkSkillActionsMenuItems
        Menu={ContextMenu}
        selectedSkills={selectedRows}
        onApplyUpdates={applyUpdatesAndClear}
        onDeleteSkills={deleteSkillsAndClear}
      />
    ) : (
      <SkillActionsMenuItems Menu={ContextMenu} skill={skill} onApplyUpdates={applyUpdatesAndClear} onDeleteSkills={deleteSkillsAndClear} />
    );
  }, [applyUpdatesAndClear, deleteSkillsAndClear]);

  const bottomBar = useCallback((selectedRows: SkillTableRow[]) => {
    const deletable = selectedRows.filter(isSkillSelectable);
    const updateNames = selectedRows
      .filter((skill) => skill.updateStatus === "update-available")
      .map((skill) => skill.name);
    return (
      <>
        <DropdownMenu.Root>
          <DropdownMenu.Trigger asChild>
            <button aria-label="Set visibility">Set visibility</button>
          </DropdownMenu.Trigger>
          <DropdownMenu.Portal>
            <DropdownMenu.Content className="skillMenuContent" align="start" sideOffset={6}>
              <VisibilityMenuItems Menu={DropdownMenu} selectedSkills={selectedRows} onSetVisibility={setVisibilityAndClear} />
            </DropdownMenu.Content>
          </DropdownMenu.Portal>
        </DropdownMenu.Root>
        <button
          className="skillApplyUpdatesButton"
          disabled={applyingUpdates || updateNames.length === 0}
          onClick={() => applyUpdatesAndClear(updateNames)}
        >
          {applyingUpdates ? "Preparing preview" : `Update${updateNames.length ? ` (${updateNames.length})` : ""}`}
        </button>
        <button aria-label="Create wrapper skill" onClick={() => setShowWrapper(true)}><PackagePlus size={15} />Create wrapper skill</button>
        <button
          className="danger"
          aria-label="Delete selected skills"
          disabled={deletable.length === 0}
          onClick={() => deleteSkillsAndClear(deletable.map((skill) => skill.name))}
        >
          <Trash2 size={15} />
          Delete
        </button>
      </>
    );
  }, [applyUpdatesAndClear, deleteSkillsAndClear, setVisibilityAndClear]);

  return (
    <section className="content skillsPage">
      <ContentTopDragStrip />
      <PageHeader title="Skills">
        <SearchField placeholder="Search skills" value={query} onChange={(event) => setQuery(event.target.value)} />
        <button
          className="iconButton"
          disabled={loadingSkills}
          onClick={onRefresh}
          aria-label="Refresh skills and check updates"
        >
          {checkingUpdates ? <LoadingIcon size={16} /> : <RefreshCw size={16} />}
        </button>
        {updateError && <span className="skillUpdateError" role="alert">{updateError}</span>}
        <Dialog.Root open={showAddSkill} onOpenChange={setShowAddSkill}>
          <Dialog.Trigger asChild>
            <button className="iconButton filled" aria-label="Add skill"><Plus size={16} /></button>
          </Dialog.Trigger>
          <AddSkillDialog
            onClose={() => setShowAddSkill(false)}
            onInstalled={onAddInstalled}
            onRequestWrapper={(skills) => {
              setInstalledWrapperSkills(skills);
              setShowWrapper(true);
            }}
          />
        </Dialog.Root>
      </PageHeader>
      <DataTable
        rows={tableRows}
        columns={columns}
        getRowId={(skill) => skill.id}
        getRowLabel={(skill) => skill.name}
        selectable={(skill) => isSkillRowSelectable(skill)}
        selectedIds={selected}
        onSelectionChange={setSelected}
        enableMarquee
        defaultGroupBy="origin"
        onRowClick={openSkill}
        rowContextMenu={rowContextMenu}
        bottomBar={bottomBar}
        bottomBarCheckboxLabel="Select visible skills from toolbar"
        selectionLabel="skills"
        loading={loadingSkills}
        loadingLabel={<LoadingInline label="Loading skills" />}
        emptyState={normalizedQuery ? (
          <><Hammer size={20} /><span>No skills match this search</span><span>Try another search or clear filters.</span></>
        ) : (
          <><Hammer size={20} /><span>No skills yet</span><span>Add a skill to install and manage it here.</span></>
        )}
      />
      <WrapperDialog
        open={showWrapper}
        selectedSkills={installedWrapperSkills.length > 0 ? installedWrapperSkills : selectedSkills}
        onOpenChange={(open) => {
          setShowWrapper(open);
          if (!open) setInstalledWrapperSkills([]);
        }}
        onApplyWrapper={applyWrapperAndClear}
      />
    </section>
  );
}
