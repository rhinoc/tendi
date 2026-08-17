import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { ComponentType, ReactNode } from "react";
import { ContextMenu, Dialog, DropdownMenu, ToggleGroup } from "radix-ui";
import {
  ChevronRight,
  Copy,
  Eye,
  FolderOpen,
  Hammer,
  List,
  PackagePlus,
  Plus,
  RefreshCw,
  Sparkles,
  Trash2,
  Waypoints,
  X,
} from "lucide-react";

import { AgentChips } from "../components/shared/AgentChips.tsx";
import { AgentOptionLabel } from "../components/shared/AgentOptionLabel.tsx";
import { Button } from "../components/shared/Button.tsx";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogAdvanceButton } from "../components/shared/DialogAdvanceButton.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { SelectionCheckbox } from "../components/shared/SelectionCheckbox.tsx";
import { SegmentedControl, SegmentedControlItem } from "../components/shared/SegmentedControl.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { SkillRelationshipMap } from "../features/skills/SkillRelationshipMap.tsx";
import { StatefulButton } from "../components/shared/StatefulButton.tsx";
import { Visibility } from "../features/skills/Visibility.tsx";
import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import { TauriCommand, SkillVisibility, agentIdentityKey, allSkillVisibilities, compactDateTime, copyText, editableSkillVisibilities, invokeCommand, isSkillRowSelectable, isSkillSelectable, isSkillVisibilityEditable, primarySkillPath, safeInvoke, skillSourceAction, skillSourceDetails, skillTargets, sourceRemoteDetails, suppressNextClick } from "../lib/index.ts";

export type SkillsTableSort = SortState;

type SkillsViewMode = "list" | "network";

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
  ctime?: string;
  mtime?: string;
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

type MarketplaceSource = {
  id: string;
  name: string;
  description?: string;
  source: string;
  url: string;
  version?: string;
  metric?: number;
  metricLabel?: string;
  trustLabel?: string;
  kind: string;
};

type MarketplaceSearchResponse = {
  items?: MarketplaceSource[];
  warnings?: string[];
};

const recommendedSkillSources: MarketplaceSource[] = [
  {
    id: "mattpocock-skills",
    name: "Matt Pocock Skills",
    source: "mattpocock/skills",
    url: "https://github.com/mattpocock/skills",
    trustLabel: "Community",
    kind: "Collection",
  },
  {
    id: "claude-code-plugin-dev",
    name: "Claude Code Plugin Dev",
    source: "anthropics/claude-code/plugins/plugin-dev",
    url: "https://github.com/anthropics/claude-code/tree/main/plugins/plugin-dev",
    trustLabel: "Anthropic official",
    kind: "Collection",
  },
  {
    id: "emil-skills",
    name: "Emil's Design Skills",
    source: "emilkowalski/skills",
    url: "https://github.com/emilkowalski/skills",
    trustLabel: "Community",
    kind: "Collection",
  },
  {
    id: "vercel-agent-skills",
    name: "Vercel Agent Skills",
    source: "vercel-labs/agent-skills",
    url: "https://github.com/vercel-labs/agent-skills",
    trustLabel: "Vercel official",
    kind: "Collection",
  },
];

type SkillMarkdownPreview = {
  name: string;
  relativePath: string;
  content: string;
};

type SkillPreviewOverrides = {
  target?: string;
  copy?: boolean;
  visibility?: SkillVisibility;
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

function installedSkillName(result: SkillInstallResult): string | null {
  const installedNames = new Set((result.skills ?? []).map((skill) => skill.name));
  return result.report?.plan?.selected
    ?.map((skill) => skill.name)
    .find((name) => installedNames.has(name)) ?? null;
}

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
            <RefreshCw size={14} />
            Apply update
          </Menu.Item>
        </>
      )}
      {hasMultipleTargets ? (
        <Menu.Sub>
          <Menu.SubTrigger className="skillMenuItem skillMenuSubTrigger">
            <FolderOpen size={14} />
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
          <FolderOpen size={14} />
          Reveal in Finder
        </Menu.Item>
      )}
      {hasMultipleTargets ? (
        <Menu.Sub>
          <Menu.SubTrigger className="skillMenuItem skillMenuSubTrigger">
            <Copy size={14} />
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
          <Copy size={14} />
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
          <RefreshCw size={14} />
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
        {sourceAction && (
          <button
            className="skillSourceButton"
            aria-label={sourceAction.ariaLabel}
            onClick={(event) => {
              event.stopPropagation();
              sourceAction.onClick();
            }}
          >
            {sourceAction.icon}
          </button>
        )}
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
    <RowActionsMenu
      ariaLabel={`Skill actions for ${skill.name}`}
      onOpenChange={(open) => { if (!open) suppressNextClick(); }}
    >
      <SkillActionsMenuItems Menu={DropdownMenu} skill={skill} onApplyUpdates={onApplyUpdates} onDeleteSkills={onDeleteSkills} />
    </RowActionsMenu>
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
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => !busy && onOpenChange(nextOpen)}
      className="wrapperDialogPanel"
      descriptionId="wrapper-skill-dialog-description"
    >
          <Dialog.Title className="confirmDialogTitle">Create wrapper skill</Dialog.Title>
          <Dialog.Description id="wrapper-skill-dialog-description" className="dialogVisuallyHidden">
            Create a wrapper skill from the selected child skills.
          </Dialog.Description>
          <div className="wrapperDialogBody">
            <DialogTextField label="Name" value={name} onChange={updateName} placeholder="" />
            <label className="dialogField">
              <span>Description</span>
              <textarea
                autoFocus
                className="dialogTextArea"
                value={description}
                onChange={(event) => updateDescription(event.target.value)}
              />
            </label>
            <label className="checkboxLine">
              <SelectionCheckbox
                checked={manualChildren}
                label="Only trigger selected skills through this wrapper"
                onChange={(checked: boolean | "indeterminate") => setManualChildren(Boolean(checked))}
              />
              <span>
                <strong>Make children manual</strong>
                <br />
                <small>Selected skills will stop triggering on their own.</small>
              </span>
            </label>
            {error ? <div className="dialogError">{error}</div> : null}
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
    </DialogShell>
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

function isDirectSkillSource(value: string) {
  const source = value.trim();
  return /^(?:https?:\/\/|ssh:\/\/|git@|github:|gitlab:|huggingface:|\/|\/?\.\.?\/)/i.test(source)
    || /^[^/\s]+\/[^/\s]+(?:#\S+)?$/.test(source);
}

const TiptapMarkdownPreview = lazy(() => import("../components/shared/TiptapMarkdownPreview.tsx").then(({ TiptapMarkdownPreview: component }) => ({ default: component })));

export type AddSkillDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  trigger: ReactNode;
  onClose: () => void;
  onInstalled: (result: SkillInstallResult) => void;
  onRequestWrapper: (skills: SkillRecord[]) => void;
  installedAgentKeys: string[];
};

type InstallTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
};

export function AddSkillDialog({ open, onOpenChange, trigger, onClose, onInstalled, onRequestWrapper, installedAgentKeys }: AddSkillDialogProps) {
  const [source, setSource] = useState("");
  const [marketplaceResults, setMarketplaceResults] = useState<MarketplaceSource[]>([]);
  const [marketplaceBusy, setMarketplaceBusy] = useState(false);
  const [marketplaceError, setMarketplaceError] = useState("");
  const [skillPreview, setSkillPreview] = useState<SkillMarkdownPreview | null>(null);
  const [skillPreviewBusy, setSkillPreviewBusy] = useState("");
  const [skillPreviewError, setSkillPreviewError] = useState("");
  const [target, setTarget] = useState("shared");
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
    () => {
      const installed = new Set(installedAgentKeys);
      return installTargets
        .map((option, index) => ({
          option,
          index,
          installed: installed.has(agentIdentityKey(option.id)),
        }))
        .filter(({ option }) => option.supportsGlobal && option.id !== "universal")
        .sort((left, right) => {
          if (left.option.id === "shared") return -1;
          if (right.option.id === "shared") return 1;
          if (left.installed !== right.installed) return left.installed ? -1 : 1;
          return left.index - right.index;
        })
        .map(({ option }) => option);
    },
    [installTargets, installedAgentKeys],
  );
  useEffect(() => {
    if (!visibleInstallTargets.some((option) => option.id === target)) {
      setTarget(visibleInstallTargets[0]?.id ?? "shared");
    }
  }, [target, visibleInstallTargets]);
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
  const canInstall = Boolean(source.trim() && plan && previewId && selected.length > 0 && !busy);
  const advanceLabel = busyAction === "install" ? "Installing selected skills" : "Install selected skills";
  const advanceText = busyAction === "install" ? "Installing" : "Install";
  const sourceCandidates = source.trim() ? marketplaceResults : recommendedSkillSources;
  const sourceCandidatesLabel = source.trim() ? "Matches" : "Recommended";

  const handleSourceChange = (value: string) => {
    setSource(value);
    setMarketplaceResults([]);
    setMarketplaceError("");
    setPlan(null);
    setPreviewId("");
    setSelectedRoots([]);
    setSkillPreview(null);
    setSkillPreviewError("");
    setError("");
  };

  const searchMarketplace = async (rawQuery = source) => {
    const query = rawQuery.trim();
    if (query.length < 2 || marketplaceBusy) return;
    setMarketplaceBusy(true);
    setMarketplaceError("");
    setPlan(null);
    setPreviewId("");
    setSkillPreview(null);
    setSkillPreviewError("");
    try {
      const response = await invokeCommand<MarketplaceSearchResponse>(TauriCommand.SkillsMarketplaceSearch, {
        query,
      });
      setMarketplaceResults(response?.items ?? []);
      setMarketplaceError(response?.warnings?.length
        ? "Some marketplaces are unavailable."
        : response?.items?.length ? "" : "No matching skills.");
    } catch (searchError) {
      setMarketplaceResults([]);
      setMarketplaceError(String(searchError));
    } finally {
      setMarketplaceBusy(false);
    }
  };

  const previewSource = async (nextSource = source, overrides: SkillPreviewOverrides = {}) => {
    const normalizedSource = nextSource.trim();
    if (!normalizedSource || busyAction || marketplaceBusy) return;
    const nextTarget = overrides.target ?? target;
    const nextCopy = overrides.copy ?? copy;
    const nextVisibility = overrides.visibility ?? visibility;
    setSource(normalizedSource);
    setMarketplaceError("");
    setSkillPreview(null);
    setSkillPreviewError("");
    setBusyAction("preview");
    setError("");
    try {
      const response = await invokeCommand(TauriCommand.SkillsAdd, {
        source: normalizedSource,
        target: nextTarget,
        scope: "global",
        skills: [],
        copy: nextCopy,
        overwrite: false,
        visibility: nextVisibility.toLowerCase(),
        dryRun: true,
      }) as { plan?: SkillAddPlan; previewId?: string } | null;
      if (!response?.plan || !response.previewId) {
        throw new Error("Skill preview returned no data. Restart the development service and try again.");
      }
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

  const resolveSourceInput = () => {
    const value = source.trim();
    if (!value || busy || marketplaceBusy) return;
    if (isDirectSkillSource(value)) {
      void previewSource(value);
    } else {
      void searchMarketplace(value);
    }
  };

  const selectMarketplaceSource = (skill: MarketplaceSource) => {
    void previewSource(skill.source);
  };

  const previewSkill = async (name: string) => {
    if (!previewId || skillPreviewBusy) return;
    setSkillPreviewBusy(name);
    setSkillPreviewError("");
    try {
      const preview = await invokeCommand<SkillMarkdownPreview>(TauriCommand.SkillsAddPreviewRead, {
        previewId,
        skillName: name,
      });
      setSkillPreview(preview);
    } catch (previewError) {
      setSkillPreview(null);
      setSkillPreviewError(`${previewError}`);
    } finally {
      setSkillPreviewBusy("");
    }
  };

  const install = async () => {
    if (!canInstall) return;
    setBusyAction("install");
    setError("");
    try {
      const result = await invokeCommand<SkillInstallResult>(TauriCommand.SkillsAdd, {
        source: source.trim(),
        target,
        scope: "global",
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
    <DialogShell
      open={open}
      onOpenChange={onOpenChange}
      trigger={trigger}
      className="addSkillPanel"
      descriptionId="add-skill-dialog-description"
    >
      <Dialog.Title className="confirmDialogTitle">{plan ? "Review & install" : "Add skill"}</Dialog.Title>
      <Dialog.Description id="add-skill-dialog-description" className="dialogVisuallyHidden">
        {plan ? "Review the selected skills and install them with the chosen settings." : "Choose a skill source and review it before installation."}
      </Dialog.Description>
      <div className={`addSkillGrid ${plan ? "hasPlan" : "sourceStage"}`}>
        <div className="skillSourceField">
          <form
            className="skillSourceForm"
            aria-busy={marketplaceBusy || busyAction === "preview"}
            onSubmit={(event) => {
              event.preventDefault();
              resolveSourceInput();
            }}
          >
            <SearchField
              value={source}
              onChange={(event) => handleSourceChange(event.target.value)}
              onClear={() => handleSourceChange("")}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return;
                event.preventDefault();
                resolveSourceInput();
              }}
              placeholder="Search skills or paste a source"
              aria-label="Search skills or paste a source"
            />
          </form>
          {(marketplaceBusy || busyAction === "preview") && (
            <LoadingState
              className={plan ? "loadingStateCompact" : ""}
              label={marketplaceBusy ? "Searching marketplaces" : "Loading skill"}
            />
          )}
          {sourceCandidates.length > 0 && !plan && !marketplaceBusy && (
            <>
              <div className="sourceCandidatesHeader">
                <span>{sourceCandidatesLabel}</span>
              </div>
              <div className="sourceCandidates" data-no-drag>
                {sourceCandidates.map((skill) => (
                  <button
                    type="button"
                    className="sourceCandidate"
                    key={skill.id + "-" + skill.source}
                    onClick={() => selectMarketplaceSource(skill)}
                  >
                    <span className="sourceCandidateIcon" aria-hidden="true">
                      <Sparkles size={15} />
                    </span>
                    <span className="sourceCandidateCopy">
                      <strong>
                        {skill.name}
                        <em>{skill.kind}</em>
                      </strong>
                      <small>{skill.description || skill.source}</small>
                    </span>
                    <span className="sourceCandidateMeta">
                      {skill.metric != null
                        ? String(skill.metric.toLocaleString()) + (skill.metricLabel ? " " + skill.metricLabel : "")
                        : skill.trustLabel}
                      <ChevronRight size={14} aria-hidden="true" />
                    </span>
                  </button>
                ))}
              </div>
            </>
          )}
          {marketplaceError && <div className="dialogError" data-selectable-text>{marketplaceError}</div>}
        </div>
      </div>
      {error && <div className="dialogError" data-no-drag data-selectable-text>{error}</div>}
      {plan && (
        <div className="addSkillReview">
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
              <Button
                size="sm"
                variant={newSelected ? "primary" : "ghost"}
                aria-pressed={newSelected}
                onClick={() => toggleSkillGroup(newSkills)}
                disabled={newSkills.length === 0}
              >
                New
              </Button>
              <Button
                size="sm"
                variant={existingSelected ? "primary" : "ghost"}
                aria-pressed={existingSelected}
                onClick={() => toggleSkillGroup(existingSkills)}
                disabled={existingSkills.length === 0}
              >
                Existing
              </Button>
            </div>
          </div>
          {showSkillSearch && (
            <SearchField
              value={skillSearch}
              onChange={(event) => setSkillSearch(event.target.value)}
              onClear={() => setSkillSearch("")}
              placeholder="Search skills"
              aria-label="Search available skills"
              endContent={normalizedSkillSearch ? (
                <span>{searchMatches.length === 1 ? "1 match" : `${searchMatches.length} matches`}</span>
              ) : null}
            />
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
                <span className="addSkillItemContent">
                  <strong>
                    {skill.name}
                    {statusLabel && <em>{statusLabel}</em>}
                    {requiredBy.length > 0 && <em>Required</em>}
                  </strong>
                  <small>{skill.description || skill.relative_path || skill.relativePath || "No description"}</small>
                  {(dependencyNames.length > 0 || requiredBy.length > 0) && (
                    <small>
                      {dependencyNames.length > 0 ? `Depends on ${dependencyNames.join(", ")}` : `Required by ${requiredBy.join(", ")}`}
                    </small>
                  )}
                </span>
                <IconButton
                  aria-label={`Preview ${skill.name} SKILL.md`}
                  aria-busy={skillPreviewBusy === skill.name}
                  disabled={Boolean(skillPreviewBusy)}
                  onClick={(event) => {
                    event.stopPropagation();
                    void previewSkill(skill.name);
                  }}
                >
                  {skillPreviewBusy === skill.name ? <LoadingIcon size={13} /> : <Eye size={14} />}
                </IconButton>
              </div>
            );})}
          </div>
          {skillPreviewError && <div className="skillPreviewError" data-selectable-text>{skillPreviewError}</div>}
          {skillPreview && (
            <section className="skillMarkdownPreview" data-no-drag>
              <header className="skillMarkdownPreviewHeader">
                <div>
                  <strong>{skillPreview.name}</strong>
                  <small>{skillPreview.relativePath}</small>
                </div>
                <IconButton
                  aria-label="Close skill preview"
                  onClick={() => setSkillPreview(null)}
                >
                  <X size={14} />
                </IconButton>
              </header>
              <div className="skillMarkdownPreviewBody">
                <Suspense fallback={<LoadingInline label="Loading preview" />}>
                  <TiptapMarkdownPreview content={skillPreview.content} />
                </Suspense>
              </div>
            </section>
          )}
          {selectedRoots.length > 1 && (
            <label className="checkboxLine">
              <SelectionCheckbox
                checked={createWrapper}
                label="Create a wrapper skill after installation"
                onChange={(checked: boolean | "indeterminate") => setCreateWrapper(Boolean(checked))}
              />
              <span>
                <strong>Create wrapper skill after install</strong>
                <br />
                <small>Continue to name a wrapper for the selected skills.</small>
              </span>
            </label>
          )}
          </div>
          <aside className="addSkillSettings" aria-label="Install options">
            <div className="dialogField">
              <strong>Install options</strong>
              <small>{selected.length} selected</small>
            </div>
            <div className="dialogField">
              <span>Target</span>
              <SelectControl
                value={target}
                onValueChange={(value) => {
                  if (busy) return;
                  setTarget(value);
                  void previewSource(source, { target: value });
                }}
                label="Skill install target"
                options={visibleInstallTargets.map((option) => ({ value: option.id, label: option.displayName }))}
                className="dialogSelectTrigger"
                side="bottom"
                align="start"
                renderValue={(option) => option ? <AgentOptionLabel agent={option.value} label={option.label} /> : null}
                renderOption={(option) => <AgentOptionLabel agent={option.value} label={option.label} />}
              />
            </div>
            <div className="dialogField">
              <span>Visibility</span>
              <SelectControl
                value={visibility}
                onValueChange={(value) => {
                  if (busy) return;
                  setVisibility(value as SkillVisibility);
                  void previewSource(source, { visibility: value as SkillVisibility });
                }}
                label="Skill visibility"
                options={editableSkillVisibilities.map((option) => ({ value: option, label: option }))}
                className="dialogSelectTrigger"
                side="bottom"
                align="start"
              />
            </div>
            <div className="dialogField">
              <span>Mode</span>
              <SegmentedControl
                variant="accent"
                fullWidth
                value={copy ? "copy" : "symlink"}
                onValueChange={(value) => {
                  if (!value) return;
                  if (busy) return;
                  const nextCopy = value === "copy";
                  setCopy(nextCopy);
                  void previewSource(source, { copy: nextCopy });
                }}
                aria-label="Skill install mode"
              >
                <SegmentedControlItem value="symlink" aria-label="Install using symlink">
                  Symlink
                </SegmentedControlItem>
                <SegmentedControlItem value="copy" aria-label="Install by copying files">
                  Copy
                </SegmentedControlItem>
              </SegmentedControl>
            </div>
          </aside>
        </div>
      )}
      <DialogActionBar onCancel={onClose}>
        {plan && (
          <DialogAdvanceButton
            label={advanceText}
            ariaLabel={advanceLabel}
            busy={busyAction === "install"}
            disabled={!canInstall}
            onClick={advance}
          />
        )}
      </DialogActionBar>
    </DialogShell>
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
  installedAgentKeys: string[];
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
  installedAgentKeys,
}: SkillsViewProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [viewMode, setViewMode] = useState<SkillsViewMode>("list");
  const [showWrapper, setShowWrapper] = useState(false);
  const [showAddSkill, setShowAddSkill] = useState(false);
  const [installedWrapperSkills, setInstalledWrapperSkills] = useState<SkillRecord[]>([]);
  const [skillLocatorRequest, setSkillLocatorRequest] = useState("");
  const skillListBodyRef = useRef<HTMLDivElement | null>(null);
  const normalizedQuery = query.trim().toLowerCase();
  const visibleSkills = useMemo(() => {
    if (!normalizedQuery) return skillItems;
    return skillItems.filter((skill) => [skill.name, skill.description].some((value) => `${value ?? ""}`.toLowerCase().includes(normalizedQuery)));
  }, [normalizedQuery, skillItems]);
  const tableRows = loadingSkills ? [] : visibleSkills;
  const networkNodes = useMemo(
    () => visibleSkills.map((skill) => ({
      name: skill.name,
      label: skill.name,
      description: skill.description,
      dependencies: skill.dependencies,
      dependents: skill.dependents,
      kind: skill.isWrapper ? "wrapper" : `${skill.section ?? "local"}`.toLowerCase(),
    })),
    [visibleSkills],
  );
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

  const handleInstalled = useCallback((result: SkillInstallResult) => {
    onAddInstalled(result);
    const name = installedSkillName(result);
    if (!name) return;
    setQuery("");
    setViewMode("list");
    setSkillLocatorRequest(name);
  }, [onAddInstalled]);

  useEffect(() => {
    if (!skillLocatorRequest) return undefined;
    let frame = 0;
    let attempts = 0;
    const locate = () => {
      frame = 0;
      const root = skillListBodyRef.current?.querySelector<HTMLElement>(".dataTableBodyScroll");
      if (!root) {
        if (attempts < 20) {
          attempts += 1;
          frame = window.requestAnimationFrame(locate);
        } else {
          setSkillLocatorRequest("");
        }
        return;
      }
      const row = [...root.querySelectorAll<HTMLElement>("[data-row-id]")]
        .find((candidate) => candidate.dataset.rowId === skillLocatorRequest);
      if (row) {
        const rootBounds = root.getBoundingClientRect();
        const rowBounds = row.getBoundingClientRect();
        const desiredTop = root.scrollTop
          + rowBounds.top
          - rootBounds.top
          - (root.clientHeight - rowBounds.height) / 2;
        const maxScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
        root.scrollTo({
          top: Math.min(maxScrollTop, Math.max(0, desiredTop)),
          behavior: "smooth",
        });
        setSkillLocatorRequest("");
        return;
      }
      if (attempts < 20) {
        attempts += 1;
        frame = window.requestAnimationFrame(locate);
      } else {
        setSkillLocatorRequest("");
      }
    };
    frame = window.requestAnimationFrame(locate);
    return () => {
      if (frame !== 0) window.cancelAnimationFrame(frame);
    };
  }, [skillLocatorRequest, tableRows]);

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
      key: "agents",
      header: "Agents",
      type: "enum",
      groupBy: (skill) => (skill.agents ?? []).join(", ") || "None",
      sortValue: (skill) => (skill.agents ?? []).join(",").toLowerCase(),
      width: "120px",
      render: (skill) => <AgentChips agents={skill.agents} />,
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
      key: "visibility",
      header: "Visibility",
      type: "enum",
      groupOrder: [...allSkillVisibilities],
      sortValue: (skill) => skill.visibility?.toLowerCase() ?? "",
      width: "176px",
      render: (skill) => <Visibility value={skill.visibility ?? SkillVisibility.Auto} skill={skill} onSetVisibility={setVisibilityAndClear} />,
    },
    {
      key: "ctime",
      header: "Created",
      type: "date",
      sortValue: (skill) => skill.ctime ?? "",
      width: "104px",
      value: (skill) => compactDateTime(skill.ctime),
      empty: "",
    },
    {
      key: "mtime",
      header: "Modified",
      type: "date",
      sortValue: (skill) => skill.mtime ?? "",
      width: "104px",
      value: (skill) => compactDateTime(skill.mtime),
      empty: "",
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
        <StatefulButton
          className="skillApplyUpdatesButton"
          size="sm"
          width={136}
          minWidth={136}
          state={applyingUpdates ? "loading" : "idle"}
          aria-label={applyingUpdates ? "Preparing update preview" : `Update${updateNames.length ? ` (${updateNames.length})` : ""}`}
          disabled={updateNames.length === 0}
          onClick={() => applyUpdatesAndClear(updateNames)}
          loadingContent={<LoadingIcon size={15} />}
        >
          Update{updateNames.length ? ` (${updateNames.length})` : ""}
        </StatefulButton>
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
        <ToggleGroup.Root
          className="skillsViewToggle"
          type="single"
          value={viewMode}
          onValueChange={(value) => {
            if (value === "list" || value === "network") setViewMode(value);
          }}
          aria-label="Skills view"
        >
          <ToggleGroup.Item className="skillsViewToggleItem" value="list" aria-label="Show skills as a list">
            <List size={15} aria-hidden="true" />
          </ToggleGroup.Item>
          <ToggleGroup.Item className="skillsViewToggleItem" value="network" aria-label="Show skill relationships">
            <Waypoints size={15} aria-hidden="true" />
          </ToggleGroup.Item>
        </ToggleGroup.Root>
        <SearchField placeholder="Search skills" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
        <IconButton
          disabled={loadingSkills}
          onClick={onRefresh}
          aria-label="Refresh skills and check updates"
          aria-busy={checkingUpdates}
        >
          {checkingUpdates ? <LoadingIcon size={16} /> : <RefreshCw size={16} />}
        </IconButton>
        {updateError && <span className="skillUpdateError" role="alert">{updateError}</span>}
        <AddSkillDialog
          open={showAddSkill}
          onOpenChange={setShowAddSkill}
          trigger={(
            <Dialog.Trigger asChild>
              <IconButton className="filled" aria-label="Add skill"><Plus size={16} /></IconButton>
            </Dialog.Trigger>
          )}
          onClose={() => setShowAddSkill(false)}
          onInstalled={handleInstalled}
          installedAgentKeys={installedAgentKeys}
          onRequestWrapper={(skills) => {
            setInstalledWrapperSkills(skills);
            setShowWrapper(true);
          }}
        />
      </PageHeader>
      {viewMode === "network" ? (
        <SkillRelationshipMap
          nodes={loadingSkills ? [] : networkNodes}
          onOpenSkill={(name) => {
            const skill = skillItems.find((item) => item.name === name);
            if (skill) openSkill(skill);
          }}
        />
      ) : (
        <div className="skillsListBody" ref={skillListBodyRef}>
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
            defaultSort={{ key: "mtime", direction: "desc" }}
            onRowClick={openSkill}
            rowContextMenu={rowContextMenu}
            bottomBar={bottomBar}
            bottomBarCheckboxLabel="Select visible skills from toolbar"
            selectionLabel="skills"
            loading={loadingSkills}
            loadingLabel="Loading skills"
            emptyState={normalizedQuery ? (
              <><Hammer size={20} /><span>No skills match this search</span><span>Try another search or clear filters.</span></>
            ) : (
              <><Hammer size={20} /><span>No skills yet</span><span>Add a skill to install and manage it here.</span></>
            )}
          />
        </div>
      )}
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
