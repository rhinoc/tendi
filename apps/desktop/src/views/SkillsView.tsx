import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ComponentType, ReactNode } from "react";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import {
  ChevronLeft,
  ChevronRight,
  Code2,
  Copy,
  ArrowRightLeft,
  Eye,
  FolderOpen,
  Hammer,
  List,
  PackagePlus,
  Plus,
  RefreshCw,
  SearchX,
  Trash2,
  Waypoints,
  X,
} from "lucide-react";

import { AgentChips } from "../components/shared/AgentChips.tsx";
import { AgentOptionLabel } from "../components/shared/AgentOptionLabel.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { BadgeList } from "../components/shared/BadgeList.tsx";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { OpenInEditorMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DialogActionButton } from "../components/shared/DialogActionButton.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogAdvanceButton } from "../components/shared/DialogAdvanceButton.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { DataTableSelectionActions, renderDataTableSelectionMenu, type DataTableSelectionActionDefinition } from "../components/shared/DataTableSelectionActions.tsx";
import { MenuContent } from "../components/shared/MenuContent.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { SelectionCheckbox } from "../components/shared/SelectionCheckbox.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { SegmentedControl, SegmentedControlItem } from "../components/shared/SegmentedControl.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { RelationshipGraphKind, SkillRelationshipMap } from "../features/skills/SkillRelationshipMap.tsx";
import { SkillLocationDialog } from "../features/skills/SkillLocationDialog.tsx";
import { SKILL_BADGE_TONES } from "../features/skills/skill-badge-tones.ts";
import { Visibility } from "../features/skills/Visibility.tsx";
import { DataTable } from "../components/DataTable.tsx";
import { ColumnDataType, type ColumnDef, type SortState } from "../components/DataTable.types";
import { SortDirection } from "../lib/sort.ts";
import { actionLabels, SKILL_FREEZE_COLUMN, selectionDeleteLabel, SkillUpdateAvailability, TauriCommand, SkillOperationStatus, SkillVisibility, agentIdentityKey, allSkillVisibilities, compactDateTime, copyText, editableSkillVisibilities, findSkillBySelector, isReadOnlySkillSource, isSkillRowSelectable, isSkillSelectable, isSkillVisibilityEditable, primarySkillPath, safeInvoke, scopeColumn, skillSourceAction, skillSourceDetails, skillTargets, sourceRemoteDetails, suppressNextClick, type NormalizedSkill, type ProjectSummary, type RawSkillRecord, type SkillAddPlan, type SkillInstallResult, type WrapperArgs } from "../lib/index.ts";
import { captureSkillSourcePage, isSkillSourceActionReady, normalizeSkillAddPlan, resolveSkillInstallTarget, restoreSkillSourcePage, shouldShowSkillQuickSelect, skillSourceErrorMessage, type SkillSourcePageSnapshot } from "../lib/add-skill-dialog.ts";
import { SkillActionId, skillActionIds } from "../lib/skill-actions.ts";
import {
  buildSkillInstallViewModel,
  isExistingSkillOperationStatus,
  isSelectableOperationStatus,
  SkillInstallFilter,
  selectSkillListView,
} from "../controllers/skill-controller.ts";
import {
  installSkillAdd,
  previewSkillAdd,
  readSkillPreview,
  searchSkillMarketplace,
  SkillDistributionMode,
  SkillScope,
  type MarketplaceSource,
  type SkillChangeResponse,
  type SkillPreviewReadResponse,
} from "../lib/runtime-gateway.ts";

export type SkillsTableSort = SortState;

enum SkillsViewMode {
  List = "list",
  Network = "network",
}

enum SkillAddBusyAction {
  Idle = "",
  Preview = "preview",
  Install = "install",
}

type SkillWrapperSelection = {
  id: string;
  name: string;
  description?: string;
};

type SkillTableRow = NormalizedSkill;

type SkillTarget = {
  id: string;
  agent: string;
  label: string;
  path: string;
};

function skillOriginLabel(skill: NormalizedSkill): string {
  const source = skillSourceDetails(skill);
  const remote = sourceRemoteDetails(source.value, source.kind);
  return remote?.host.includes("github.") && remote.path ? remote.path : skill.section;
}
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

function installedSkillName(result: SkillInstallResult): string | null {
  const rows = result.updated ?? result.skills ?? [];
  const installedNames = new Set(rows.flatMap((skill) => typeof skill.name === "string" ? [skill.name] : []));
  return result.report.plan.selected
    .map((skill) => skill.name)
    .find((name) => installedNames.has(name)) ?? null;
}

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
  selectedSkills: NormalizedSkill[];
  onSetVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
};

export function VisibilityMenuItems({ Menu, selectedSkills, onSetVisibility }: VisibilityMenuItemsProps) {
  const names = selectedSkills.filter(isSkillVisibilityEditable).map((skill) => skill.id || skill.name);
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

type SkillActionDefinition = DataTableSelectionActionDefinition & { id: SkillActionId };

type SkillActionDefinitionOptions = {
  Menu: SkillMenuComponents;
  selectedSkills: NormalizedSkill[];
  applyUpdates: (names: string[]) => void;
  deleteSkills: (names: string[]) => void;
  manageLocations: (skills: NormalizedSkill[]) => void;
  createWrapper: (skills: NormalizedSkill[]) => void;
  setVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
};

type SkillPathActionId = SkillActionId.Reveal | SkillActionId.CopyPath;

function skillPathActionLabel(action: SkillPathActionId) {
  return action === SkillActionId.Reveal ? actionLabels.revealInFinder : actionLabels.copyPath;
}

function skillPathActionIcon(action: SkillPathActionId) {
  return action === SkillActionId.Reveal ? FolderOpen : Copy;
}

function runSkillPathAction(action: SkillPathActionId, path: string) {
  if (action === SkillActionId.Reveal) safeInvoke(TauriCommand.RevealInFinder, { path });
  else copyText(path);
}

function renderSkillPathMenuItems(Menu: SkillMenuComponents, action: SkillPathActionId, targets: SkillTarget[], primaryPath: string | null) {
  const label = skillPathActionLabel(action);
  const Icon = skillPathActionIcon(action);
  if (targets.length > 1) {
    return (
      <Menu.Sub>
        <Menu.SubTrigger className="skillMenuItem skillMenuSubTrigger">
          <Icon size={14} />
          <span>{label}</span>
          <ChevronRight className="skillMenuSubIcon" size={14} />
        </Menu.SubTrigger>
        <Menu.Portal>
          <Menu.SubContent className="skillMenuContent" sideOffset={8} alignOffset={-6}>
            {targets.map((target) => (
              <Menu.Item className="skillMenuItem" key={`${action}-${target.id}`} onSelect={() => runSkillPathAction(action, target.path)}>
                <AgentOptionLabel agent={target.agent} label={target.label} />
              </Menu.Item>
            ))}
          </Menu.SubContent>
        </Menu.Portal>
      </Menu.Sub>
    );
  }
  return (
    <Menu.Item className="skillMenuItem" disabled={!primaryPath} onSelect={() => primaryPath && runSkillPathAction(action, primaryPath)}>
      <Icon size={14} />
      {label}
    </Menu.Item>
  );
}

function renderSkillPathDirectAction(action: SkillPathActionId, targets: SkillTarget[], primaryPath: string | null) {
  const label = skillPathActionLabel(action);
  const Icon = skillPathActionIcon(action);
  if (targets.length > 1) {
    return (
      <DropdownMenu.Root>
        <DropdownMenu.Trigger asChild>
          <button aria-label={label}>
            <Icon size={15} />
            <span>{label}</span>
          </button>
        </DropdownMenu.Trigger>
        <DropdownMenu.Portal>
          <MenuContent align="start" sideOffset={6}>
            {renderSkillPathMenuItems(DropdownMenu, action, targets, primaryPath)}
          </MenuContent>
        </DropdownMenu.Portal>
      </DropdownMenu.Root>
    );
  }
  return (
    <button aria-label={label} disabled={!primaryPath} onClick={() => primaryPath && runSkillPathAction(action, primaryPath)}>
      <Icon size={15} />
      <span>{label}</span>
    </button>
  );
}

function skillActionDefinitions({ Menu, selectedSkills, applyUpdates, deleteSkills, manageLocations, createWrapper, setVisibility }: SkillActionDefinitionOptions): SkillActionDefinition[] {
  const singleSkill = selectedSkills.length === 1 ? selectedSkills[0] : undefined;
  const primaryPath = singleSkill ? primarySkillPath(singleSkill) : null;
  const targets: SkillTarget[] = singleSkill ? skillTargets(singleSkill) : [];
  const updateNames = selectedSkills.filter((skill) => skill.updateAvailability === SkillUpdateAvailability.UpdateAvailable).map((skill) => skill.id || skill.name);
  const deletableNames = selectedSkills.filter(isSkillSelectable).map((skill) => skill.id || skill.name);
  const movableSkills = selectedSkills.filter((skill) => !isReadOnlySkillSource(skill) && skillTargets(skill).length > 0);
  const updateLabel = selectedSkills.length === 1 ? "Apply update" : "Update";
  const locationLabel = selectedSkills.length === 1 ? "Manage locations" : "Locations";
  const deleteLabel = selectionDeleteLabel("skill", selectedSkills.length);
  const visibilityMenu = (
    <Menu.Sub>
      <Menu.SubTrigger className="skillMenuItem skillMenuSubTrigger">
        <Eye size={14} />
        <span>Visibility</span>
        <ChevronRight className="skillMenuSubIcon" size={14} />
      </Menu.SubTrigger>
      <Menu.Portal>
        <Menu.SubContent className="skillMenuContent" sideOffset={8} alignOffset={-6}>
          <VisibilityMenuItems Menu={Menu} selectedSkills={selectedSkills} onSetVisibility={setVisibility} />
        </Menu.SubContent>
      </Menu.Portal>
    </Menu.Sub>
  );
  const actions: Record<SkillActionId, SkillActionDefinition> = {
    [SkillActionId.OpenEditor]: {
      id: SkillActionId.OpenEditor,
      direct: <button aria-label={actionLabels.openInEditor} disabled={!primaryPath} onClick={() => primaryPath && safeInvoke(TauriCommand.OpenInEditor, { path: primaryPath })}><Code2 size={15} /><span>{actionLabels.openInEditor}</span></button>,
      menu: <OpenInEditorMenuItem Menu={Menu} path={primaryPath} />,
      measure: <><Code2 size={15} /><span>{actionLabels.openInEditor}</span></>,
    },
    [SkillActionId.Locations]: {
      id: SkillActionId.Locations,
      direct: <button aria-label={locationLabel} disabled={movableSkills.length === 0} onClick={() => movableSkills.length > 0 && manageLocations(movableSkills)}><ArrowRightLeft size={15} /><span>{locationLabel}</span></button>,
      menu: <Menu.Item className="skillMenuItem" disabled={movableSkills.length === 0} onSelect={() => { if (movableSkills.length === 0) return; suppressNextClick(); manageLocations(movableSkills); }}><ArrowRightLeft size={14} />{locationLabel}</Menu.Item>,
      measure: <><ArrowRightLeft size={15} /><span>{locationLabel}</span></>,
    },
    [SkillActionId.Update]: {
      id: SkillActionId.Update,
      direct: <button className="skillApplyUpdatesButton" aria-label={updateLabel} disabled={updateNames.length === 0} onClick={() => updateNames.length > 0 && applyUpdates(updateNames)}><RefreshCw size={15} aria-hidden="true" /><span>{updateLabel}{selectedSkills.length > 1 && updateNames.length ? ` (${updateNames.length})` : ""}</span></button>,
      menu: <Menu.Item className="skillMenuItem" disabled={updateNames.length === 0} onSelect={() => { if (updateNames.length === 0) return; suppressNextClick(); applyUpdates(updateNames); }}><RefreshCw size={14} />{updateLabel}{selectedSkills.length > 1 && updateNames.length ? ` (${updateNames.length})` : ""}</Menu.Item>,
      measure: <><RefreshCw size={15} /><span>{updateLabel}{selectedSkills.length > 1 && updateNames.length ? ` (${updateNames.length})` : ""}</span></>,
    },
    [SkillActionId.Reveal]: {
      id: SkillActionId.Reveal,
      direct: renderSkillPathDirectAction(SkillActionId.Reveal, targets, primaryPath),
      menu: renderSkillPathMenuItems(Menu, SkillActionId.Reveal, targets, primaryPath),
      measure: <><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></>,
    },
    [SkillActionId.CopyPath]: {
      id: SkillActionId.CopyPath,
      direct: renderSkillPathDirectAction(SkillActionId.CopyPath, targets, primaryPath),
      menu: renderSkillPathMenuItems(Menu, SkillActionId.CopyPath, targets, primaryPath),
      measure: <><Copy size={15} /><span>{actionLabels.copyPath}</span></>,
    },
    [SkillActionId.Visibility]: {
      id: SkillActionId.Visibility,
      direct: <DropdownMenu.Root><DropdownMenu.Trigger asChild><button aria-label="Visibility"><Eye size={15} /><span>Visibility</span></button></DropdownMenu.Trigger><DropdownMenu.Portal><MenuContent align="start" sideOffset={6}><VisibilityMenuItems Menu={DropdownMenu} selectedSkills={selectedSkills} onSetVisibility={setVisibility} /></MenuContent></DropdownMenu.Portal></DropdownMenu.Root>,
      menu: visibilityMenu,
      measure: <><Eye size={15} /><span>Visibility</span></>,
    },
    [SkillActionId.Wrapper]: {
      id: SkillActionId.Wrapper,
      direct: <button aria-label="Create wrapper" onClick={() => createWrapper(selectedSkills)}><PackagePlus size={15} /><span>Create wrapper</span></button>,
      menu: <Menu.Item className="skillMenuItem" onSelect={() => { suppressNextClick(); createWrapper(selectedSkills); }}><PackagePlus size={14} />Create wrapper</Menu.Item>,
      measure: <><PackagePlus size={15} /><span>Create wrapper</span></>,
    },
    [SkillActionId.Delete]: {
      id: SkillActionId.Delete,
      direct: <button className="danger" aria-label={deleteLabel} disabled={deletableNames.length === 0} onClick={() => deletableNames.length > 0 && deleteSkills(deletableNames)}><Trash2 size={15} /><span>{deleteLabel}</span></button>,
      menu: <Menu.Item className="skillMenuItem danger" disabled={deletableNames.length === 0} onSelect={() => { if (deletableNames.length === 0) return; suppressNextClick(); deleteSkills(deletableNames); }}><Trash2 size={14} />{deleteLabel}</Menu.Item>,
      measure: <><Trash2 size={15} /><span>{deleteLabel}</span></>,
      separatorBefore: true,
    },
  };
  return skillActionIds({ selectionCount: selectedSkills.length }).map((id) => actions[id]);
}

export type SkillActionsMenuItemsProps = {
  Menu: SkillMenuComponents;
  skill: NormalizedSkill;
  onApplyUpdates: (names: string[]) => void;
  onDeleteSkills: (names: string[]) => void;
  onManageLocations: (skill: NormalizedSkill) => void;
  onCreateWrapper: (skill: NormalizedSkill) => void;
  onSetVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
};

export function SkillActionsMenuItems({ Menu, skill, onApplyUpdates, onDeleteSkills, onManageLocations, onCreateWrapper, onSetVisibility }: SkillActionsMenuItemsProps) {
  const actions = skillActionDefinitions({
    Menu,
    selectedSkills: [skill],
    applyUpdates: onApplyUpdates,
    deleteSkills: onDeleteSkills,
    manageLocations: (skills) => { if (skills[0]) onManageLocations(skills[0]); },
    createWrapper: (skills) => { if (skills[0]) onCreateWrapper(skills[0]); },
    setVisibility: onSetVisibility,
  });
  return renderDataTableSelectionMenu(actions);
}

export type BulkSkillActionsMenuItemsProps = {
  Menu: SkillMenuComponents;
  selectedSkills: NormalizedSkill[];
  onApplyUpdates: (names: string[]) => void;
  onDeleteSkills: (names: string[]) => void;
  onManageLocations: (skills: NormalizedSkill[]) => void;
  createWrapper: () => void;
  setVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
};

export function BulkSkillActionsMenuItems({ Menu, selectedSkills, onApplyUpdates, onDeleteSkills, onManageLocations, createWrapper, setVisibility }: BulkSkillActionsMenuItemsProps) {
  const actions = skillActionDefinitions({
    Menu,
    selectedSkills,
    applyUpdates: onApplyUpdates,
    deleteSkills: onDeleteSkills,
    manageLocations: onManageLocations,
    createWrapper: () => createWrapper(),
    setVisibility,
  });
  return renderDataTableSelectionMenu(actions);
}

export type SkillMainCellProps = {
  skill: NormalizedSkill;
  openSkill: (skill: NormalizedSkill) => void;
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
          {skill.isWrapper && <Badge tone={SKILL_BADGE_TONES.wrapper}>wrapper</Badge>}
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
        {skill.updateAvailability === SkillUpdateAvailability.UpdateAvailable && (
          <Badge
            as="button"
            tone={SKILL_BADGE_TONES.update}
            type="button"
            aria-label={`View update for ${skill.name}`}
            onClick={(event) => {
              event.stopPropagation();
              onApplyUpdates([skill.name]);
            }}
          >
            Update
          </Badge>
        )}
      </div>
      <button
        className="skillOpen skillDescriptionOpen dataCellSub"
        onClick={(event) => {
          event.stopPropagation();
          openSkill(skill);
        }}
      >
        {skill.description}
      </button>
    </div>
  );
}

export type SkillActionsCellProps = {
  skill: NormalizedSkill;
  onApplyUpdates: (names: string[]) => void;
  onDeleteSkills: (names: string[]) => void;
  onManageLocations: (skill: NormalizedSkill) => void;
  onCreateWrapper: (skill: NormalizedSkill) => void;
  onSetVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
};

export function SkillActionsCell({ skill, onApplyUpdates, onDeleteSkills, onManageLocations, onCreateWrapper, onSetVisibility }: SkillActionsCellProps) {
  return (
    <RowActionsMenu
      ariaLabel={`Skill actions for ${skill.name}`}
      onOpenChange={(open) => { if (!open) suppressNextClick(); }}
    >
      <SkillActionsMenuItems Menu={DropdownMenu} skill={skill} onApplyUpdates={onApplyUpdates} onDeleteSkills={onDeleteSkills} onManageLocations={onManageLocations} onCreateWrapper={onCreateWrapper} onSetVisibility={onSetVisibility} />
    </RowActionsMenu>
  );
}

function SkillSelectionActions({
  selectedSkills,
  applyUpdates,
  deleteSkills,
  manageLocations,
  createWrapper,
  setVisibility,
}: {
  selectedSkills: NormalizedSkill[];
  applyUpdates: (names: string[]) => void;
  deleteSkills: (names: string[]) => void;
  manageLocations: () => void;
  createWrapper: () => void;
  setVisibility: (names: string[], visibility: SkillVisibility) => void;
}) {
  const actions = useMemo(
    () => skillActionDefinitions({
      Menu: DropdownMenu,
      selectedSkills,
      applyUpdates,
      deleteSkills,
      manageLocations: () => manageLocations(),
      createWrapper,
      setVisibility,
    }),
    [applyUpdates, createWrapper, deleteSkills, manageLocations, selectedSkills, setVisibility],
  );
  return <DataTableSelectionActions actions={actions} ariaLabel="More selected skill actions" />;
}

export function suggestedWrapperName(selectedSkills: SkillWrapperSelection[]) {
  const names = selectedSkills.map((skill) => skill.name).filter(Boolean);
  if (names.length === 0) return "wrapper";
  const prefixes = names
    .map((name) => name.split(/[-_]/)[0])
    .filter((prefix) => prefix.length > 1);
  return prefixes.length > 0 && prefixes.every((prefix) => prefix === prefixes[0]) ? prefixes[0] : "wrapper";
}

function childSkillSummary(selectedSkills: SkillWrapperSelection[]) {
  const names = selectedSkills.map((skill) => skill.name).filter(Boolean);
  if (names.length === 0) return "the selected child skills";
  const visibleNames = names.slice(0, 4);
  const suffix = names.length > visibleNames.length ? `, and ${names.length - visibleNames.length} more` : "";
  return `${visibleNames.join(", ")}${suffix}`;
}

export function suggestedWrapperDescription(name: string, selectedSkills: SkillWrapperSelection[] = []) {
  const domain = name.trim().replace(/[-_]+/g, " ");
  return domain ? `Use when the request is about ${domain} and matches one of these child skills: ${childSkillSummary(selectedSkills)}.` : "";
}

export type WrapperDialogProps = {
  open: boolean;
  selectedSkills: SkillWrapperSelection[];
  onOpenChange: (open: boolean) => void;
  onApplyWrapper: (args: WrapperArgs) => Promise<SkillChangeResponse>;
};

export function WrapperDialog({ open, selectedSkills, onOpenChange, onApplyWrapper }: WrapperDialogProps) {
  const [name, setName] = useState("lark");
  const [description, setDescription] = useState("");
  const [descriptionEdited, setDescriptionEdited] = useState(false);
  const [manualChildren, setManualChildren] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const selectedNames = useMemo(() => selectedSkills.map((skill) => skill.name), [selectedSkills]);
  const selectedSkillIds = useMemo(() => selectedSkills.map((skill) => skill.id), [selectedSkills]);
  const selectedSkillIdentity = useMemo(
    () => selectedSkills.map((skill) => skill.id).sort().join("\u0000"),
    [selectedSkills],
  );
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
  }, [open, selectedSkillIdentity]);

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
    skillIds: selectedSkillIds,
    description: description.trim(),
    manualChildren,
    refresh: false,
  }), [description, manualChildren, name, selectedNames, selectedSkillIds]);

  const apply = async () => {
    if (!canCreate || busy) return;
    setBusy(true);
    setError("");
    try {
      await onApplyWrapper(args);
    } catch (error) {
      setBusy(false);
      setError(commandErrorMessage(error, "Could not create the wrapper skill."));
      return;
    }
    setBusy(false);
    onOpenChange(false);
  };

  return (
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => !busy && onOpenChange(nextOpen)}
      className="wrapperDialogPanel"
      descriptionId="wrapper-skill-dialog-description"
    >
          <div className="addSkillHeader">
            <Dialog.Title className="confirmDialogTitle">Create wrapper skill</Dialog.Title>
          </div>
          <Dialog.Description id="wrapper-skill-dialog-description" className="dialogVisuallyHidden">
            Create a wrapper skill from the selected child skills.
          </Dialog.Description>
          <div className="addSkillGrid wrapperDialogBody">
            <DialogTextField label="Name" value={name} onChange={updateName} placeholder="" />
            <label className="dialogField">
              <span>Description</span>
              <textarea
                className="dialogTextArea"
                value={description}
                onChange={(event) => updateDescription(event.target.value)}
              />
            </label>
            <label className="addSkillAdvancedCheckbox">
              <SelectionCheckbox
                checked={manualChildren}
                label="Only trigger selected skills through this wrapper"
                onChange={(checked: boolean | "indeterminate") => setManualChildren(Boolean(checked))}
              />
              <span>Make children manual</span>
            </label>
            {error ? <Toast tone="error" message={error} /> : null}
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

function skillOperationStatusLabel(status: SkillOperationStatus | undefined) {
  if (status === SkillOperationStatus.AlreadyInstalled) return "Installed";
  if (status === SkillOperationStatus.AlreadyExists) return "Exists";
  if (status === SkillOperationStatus.Replace) return "Replace";
  return "";
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
  trigger?: ReactNode;
  onClose: () => void;
  onPreviewError: (message: string) => void;
  onInstalled: (result: SkillInstallResult) => void;
  onRequestWrapper: (skills: SkillWrapperSelection[]) => void;
  installedAgentKeys: string[];
  targetOptions: SkillTargetOption[];
  initialSource?: string;
  sourceLocked?: boolean;
  title?: string;
};

export type SkillTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
  globalPath?: string;
};

export function AddSkillDialog({ open, onOpenChange, trigger, onClose, onPreviewError, onInstalled, onRequestWrapper, installedAgentKeys, targetOptions, initialSource = "", sourceLocked = false, title = "Add skills" }: AddSkillDialogProps) {
  const lockedSource = sourceLocked && Boolean(initialSource.trim());
  const [source, setSource] = useState(initialSource);
  const [sourcePageBeforePreview, setSourcePageBeforePreview] = useState<SkillSourcePageSnapshot<MarketplaceSource> | null>(null);
  const [marketplaceQuery, setMarketplaceQuery] = useState("");
  const [marketplaceResults, setMarketplaceResults] = useState<MarketplaceSource[]>([]);
  const [marketplaceBusy, setMarketplaceBusy] = useState(false);
  const [marketplaceError, setMarketplaceError] = useState("");
  const [marketplaceNotice, setMarketplaceNotice] = useState("");
  const [skillPreview, setSkillPreview] = useState<SkillPreviewReadResponse | null>(null);
  const [skillPreviewBusy, setSkillPreviewBusy] = useState("");
  const [skillPreviewError, setSkillPreviewError] = useState("");
  const [target, setTarget] = useState("");
  const [copy, setCopy] = useState(false);
  const [visibility, setVisibility] = useState<SkillVisibility>(SkillVisibility.Auto);
  const [plan, setPlan] = useState<SkillAddPlan | null>(null);
  const [previewId, setPreviewId] = useState("");
  const [selectedRoots, setSelectedRoots] = useState<string[]>([]);
  const [createWrapper, setCreateWrapper] = useState(false);
  const [replaceExisting, setReplaceExisting] = useState(false);
  const [skillFilter, setSkillFilter] = useState<SkillInstallFilter>(SkillInstallFilter.All);
  const [reviewingSkills, setReviewingSkills] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [skillSearch, setSkillSearch] = useState("");
  const [busyAction, setBusyAction] = useState<SkillAddBusyAction>(SkillAddBusyAction.Idle);
  const [error, setError] = useState("");
  const skillItemRefs = useRef(new Map<string, HTMLDivElement>());
  const skillListRef = useRef<HTMLDivElement>(null);
  const initialPreviewRequestRef = useRef("");
  const busy = busyAction !== SkillAddBusyAction.Idle;
  const dialogBusy = busy || marketplaceBusy || Boolean(skillPreviewBusy);
  const installing = busyAction === SkillAddBusyAction.Install;
  const visibleInstallTargets = useMemo(
    () => {
      const installed = new Set(installedAgentKeys);
      return targetOptions
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
    [targetOptions, installedAgentKeys],
  );
  const resolvedTarget = resolveSkillInstallTarget(target, visibleInstallTargets);
  useEffect(() => {
    if (!visibleInstallTargets.some((option) => option.id === target)) {
      setTarget(visibleInstallTargets[0]?.id ?? "");
    }
  }, [target, visibleInstallTargets]);
  const available = plan?.available ?? [];
  const showSkillSearch = available.length > 10;
  const installView = useMemo(
    () => buildSkillInstallViewModel(
      available,
      plan?.operations ?? [],
      selectedRoots,
      skillFilter,
      skillSearch,
      replaceExisting,
    ),
    [available, plan?.operations, replaceExisting, selectedRoots, skillFilter, skillSearch],
  );
  const {
    selectableSkills,
    selectableNameSet,
    selected,
    selectedSet,
    selectedRootSet,
    selectedHasExisting,
    operationByName,
    dependencyReasonsByName,
    newSkills,
    existingSkills,
    visibleAvailableSkills,
    searchMatches,
    searchMatchSet,
  } = installView;
  const normalizedSkillSearch = skillSearch.trim().toLowerCase();
  useEffect(() => {
    setReplaceExisting(selectedHasExisting);
  }, [selectedHasExisting]);
  const allSelected = selectableSkills.length > 0 && selectableSkills.every((skill) => selectedSet.has(skill.name));
  const mixedSelected = selected.length > 0 && !allSelected;
  const firstSearchMatch = searchMatches[0] ?? "";
  const canInstall = Boolean(target && source.trim() && plan && selected.length > 0 && (!selectedHasExisting || replaceExisting) && !busy);
  const canGoBack = Boolean(plan) || sourcePageBeforePreview !== null;
  const advanceLabel = busy ? "Preparing installation" : "Install selected skills";
  const advanceText = busy ? "Installing" : `Install ${selected.length}`;
  const sourceCandidates = source.trim() ? marketplaceResults : recommendedSkillSources;
  const sourceCandidatesLabel = source.trim() ? "Matches" : "Recommended";

  const handleSourceChange = (value: string) => {
    if (marketplaceBusy || lockedSource) return;
    setSource(value);
    setSourcePageBeforePreview(null);
    setMarketplaceQuery(value);
    setMarketplaceResults([]);
    setMarketplaceError("");
    setMarketplaceNotice("");
    setPlan(null);
    setPreviewId("");
    setSelectedRoots([]);
    setSkillPreview(null);
    setSkillPreviewError("");
    setCopy(false);
    setReviewingSkills(false);
    setAdvancedOpen(false);
    setError("");
  };

  const searchMarketplace = async (rawQuery = source) => {
    const query = rawQuery.trim();
    if (query.length < 2 || marketplaceBusy) return;
    setMarketplaceQuery(query);
    setMarketplaceBusy(true);
    setMarketplaceError("");
    setMarketplaceNotice("");
    setPlan(null);
    setPreviewId("");
    setSkillPreview(null);
    setSkillPreviewError("");
    try {
      const response = await searchSkillMarketplace(query);
      setMarketplaceResults(response.items);
      setMarketplaceNotice(response.warnings.length
        ? "Some marketplaces are unavailable."
        : response.items.length ? "" : "No matching skills.");
    } catch (searchError) {
      setMarketplaceResults([]);
      setMarketplaceError(String(searchError));
    } finally {
      setMarketplaceBusy(false);
    }
  };

  const previewSource = async (nextSource = source) => {
    const normalizedSource = nextSource.trim();
    if (!normalizedSource || !resolvedTarget || busyAction || marketplaceBusy) return;
    const previousPage = captureSkillSourcePage(source, marketplaceResults);
    setSourcePageBeforePreview(previousPage);
    setSource(normalizedSource);
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreview(null);
    setSkillPreviewError("");
    setBusyAction(SkillAddBusyAction.Preview);
    setError("");
    try {
      const response = await previewSkillAdd({
        source: normalizedSource,
        target: resolvedTarget,
        scope: SkillScope.Global,
        skills: [],
        copy,
        overwrite: false,
        visibility,
        dryRun: true,
      });
      if (!response?.plan || !response.previewId) {
        throw new Error("Skill preview returned no data. Restart the development service and try again.");
      }
      const nextPlan = normalizeSkillAddPlan(response.plan);
      if (!nextPlan) throw new Error("Skill preview returned an invalid plan. Restart the development service and try again.");
      setPlan(nextPlan);
      setPreviewId(response.previewId);
      setCreateWrapper(false);
      setReplaceExisting(false);
      setReviewingSkills(false);
      setSkillFilter(SkillInstallFilter.All);
      setSkillSearch("");
      setSelectedRoots(nextPlan.selected.map((skill) => skill.name));
    } catch (previewError) {
      const message = skillSourceErrorMessage(previewError);
      if (lockedSource) {
        setSource(initialSource);
        setError(message);
      } else {
        restoreSourcePage(previousPage);
      }
      onPreviewError(message);
    } finally {
      setBusyAction(SkillAddBusyAction.Idle);
    }
  };

  const resolveSourceInput = () => {
    const value = source.trim();
    const directSource = isDirectSkillSource(value);
    if (!isSkillSourceActionReady(value, directSource, resolvedTarget) || busy || marketplaceBusy) return;
    if (directSource) {
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
      const preview = await readSkillPreview(previewId, name);
      setSkillPreview(preview);
    } catch (previewError) {
      setSkillPreview(null);
      setSkillPreviewError(`${previewError}`);
    } finally {
      setSkillPreviewBusy("");
    }
  };

  const clearTransientErrors = useCallback(() => {
    setError("");
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreviewError("");
  }, []);

  const resetDialogState = useCallback(() => {
    setSource(initialSource);
    setSourcePageBeforePreview(null);
    setMarketplaceQuery("");
    setMarketplaceResults([]);
    setMarketplaceBusy(false);
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreview(null);
    setSkillPreviewBusy("");
    setSkillPreviewError("");
    setTarget("");
    setCopy(false);
    setVisibility(SkillVisibility.Auto);
    setPlan(null);
    setPreviewId("");
    setSelectedRoots([]);
    setCreateWrapper(false);
    setReplaceExisting(false);
    setSkillFilter(SkillInstallFilter.All);
    setReviewingSkills(false);
    setAdvancedOpen(false);
    setSkillSearch("");
    setBusyAction(SkillAddBusyAction.Idle);
    setError("");
    skillItemRefs.current.clear();
    initialPreviewRequestRef.current = "";
  }, [initialSource]);

  const closeDialog = () => {
    if (dialogBusy) return;
    resetDialogState();
    onClose();
  };

  const restoreSourcePage = (previousPage: SkillSourcePageSnapshot<MarketplaceSource>) => {
    const restoredPage = restoreSkillSourcePage(previousPage);
    setPlan(null);
    setPreviewId("");
    setSelectedRoots([]);
    setSkillPreview(null);
    setSkillPreviewError("");
    setSource(restoredPage.source);
    setMarketplaceQuery(restoredPage.marketplaceQuery);
    setMarketplaceResults(restoredPage.marketplaceResults);
    setSourcePageBeforePreview(null);
    clearTransientErrors();
    setSkillFilter(SkillInstallFilter.All);
    setSkillSearch("");
    setReplaceExisting(false);
    setCreateWrapper(false);
    setReviewingSkills(false);
    setAdvancedOpen(false);
  };

  useEffect(() => {
    if (!open || !lockedSource || !initialSource.trim() || plan || busyAction || marketplaceBusy) return;
    if (initialPreviewRequestRef.current === initialSource) return;
    initialPreviewRequestRef.current = initialSource;
    void previewSource(initialSource);
  }, [initialSource, lockedSource, marketplaceBusy, open, plan, resolvedTarget]);

  const goBack = () => {
    if (dialogBusy) return;
    if (!canGoBack) return;
    if (reviewingSkills) {
      setReviewingSkills(false);
      setSkillPreview(null);
      return;
    }
    restoreSourcePage(sourcePageBeforePreview ?? captureSkillSourcePage(marketplaceQuery || source, marketplaceResults));
  };

  const install = async () => {
    if (!canInstall || busy) return;
    setError("");
    try {
      setBusyAction(SkillAddBusyAction.Preview);
      const previewResponse = await previewSkillAdd({
        source: source.trim(),
        target,
        scope: SkillScope.Global,
        skills: selected,
        copy,
        overwrite: replaceExisting,
        visibility,
        dryRun: true,
      });
      if (!previewResponse?.plan || !previewResponse.previewId) {
        throw new Error("Skill preview returned no data. Restart the development service and try again.");
      }
      const finalPlan = normalizeSkillAddPlan(previewResponse.plan);
      if (!finalPlan) {
        throw new Error("Skill preview returned an invalid plan. Restart the development service and try again.");
      }
      setPlan(finalPlan);
      setPreviewId(previewResponse.previewId);
      const finalOperationByName = new Map(finalPlan.operations.map((operation) => [operation.name, operation]));
      const finalSelectedHasExisting = selected.some((name) => isExistingSkillOperationStatus(finalOperationByName.get(name)?.status));
      if (finalSelectedHasExisting && !replaceExisting) {
        throw new Error("Some selected skills already exist at this destination. Choose Replace existing skills and try again.");
      }
      setBusyAction(SkillAddBusyAction.Install);
      const result = await installSkillAdd({
        source: source.trim(),
        target,
        scope: SkillScope.Global,
        skills: selected,
        copy,
        overwrite: replaceExisting,
        visibility,
        previewId: previewResponse.previewId,
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
      closeDialog();
    } catch (installError) {
      setError(`${installError}`);
    } finally {
      setBusyAction(SkillAddBusyAction.Idle);
    }
  };

  const advance = () => {
    if (busy) return;
    if (plan) install();
  };

  useLayoutEffect(() => {
    const root = skillListRef.current;
    if (!root) return;
    if (!firstSearchMatch) {
      root.scrollTo({ top: root.scrollTop, behavior: "auto" });
      return;
    }
    const target = skillItemRefs.current.get(firstSearchMatch);
    if (!target) return;
    const rootBounds = root.getBoundingClientRect();
    const targetBounds = target.getBoundingClientRect();
    const targetTop = root.scrollTop + targetBounds.top - rootBounds.top;
    const targetBottom = targetTop + targetBounds.height;
    const visibleTop = root.scrollTop;
    const visibleBottom = visibleTop + root.clientHeight;
    const nextScrollTop = targetTop < visibleTop
      ? targetTop
      : targetBottom > visibleBottom
        ? targetBottom - root.clientHeight
        : root.scrollTop;
    root.scrollTo({ top: Math.max(0, nextScrollTop), behavior: "auto" });
  }, [firstSearchMatch]);

  useEffect(() => {
    if (!open && !dialogBusy) resetDialogState();
  }, [dialogBusy, open, resetDialogState]);

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

  const selectSkillPreset = (preset: SkillInstallFilter) => {
    const skills = preset === SkillInstallFilter.All ? selectableSkills : preset === SkillInstallFilter.New ? newSkills : existingSkills;
    setSkillFilter(preset);
    setSelectedRoots(
      skills
        .filter((skill) => selectableNameSet.has(skill.name))
        .map((skill) => skill.name),
    );
  };

  return (
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) {
          if (dialogBusy) return;
          resetDialogState();
        }
        onOpenChange(nextOpen);
      }}
      trigger={trigger}
        className={`addSkillPanel ${plan ? "hasPlan" : "sourceStage"} ${reviewingSkills ? "isReviewing" : ""} ${advancedOpen ? "hasAdvanced" : ""} ${skillPreview ? "hasSkillPreview" : ""}`}
      descriptionId="add-skill-dialog-description"
    >
      <div className="addSkillBody">
        <div className="addSkillMain">
        <div className="addSkillHeader">
        <Dialog.Title className="confirmDialogTitle">
          {title}
        </Dialog.Title>
      </div>
      <Dialog.Description id="add-skill-dialog-description" className="dialogVisuallyHidden">
        {!plan
          ? lockedSource
            ? "Review the bundled Tendi skill before installation."
            : "Choose a skill source and review it before installation."
          : reviewingSkills
            ? "Select the skills to install."
            : "Review the selected skills and install them with the default settings."}
      </Dialog.Description>
      {!reviewingSkills && <div className={`addSkillGrid ${plan ? "hasPlan" : "sourceStage"}`}>
        {!plan && !lockedSource && <div className="skillSourceField">
          {busyAction !== SkillAddBusyAction.Preview && (
            <form
              className="skillSourceForm"
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
          )}
          {!plan && (marketplaceBusy || busyAction === SkillAddBusyAction.Preview) && (
            <LoadingState
              variant="progress"
              label={marketplaceBusy ? "Searching marketplaces" : "Scanning repository"}
            />
          )}
          {source.trim() && !marketplaceBusy && busyAction !== SkillAddBusyAction.Preview && sourceCandidates.length === 0 && !marketplaceNotice && !marketplaceError && (
            <EmptyState
              className="sourceSearchEmpty"
              compact
              title={isDirectSkillSource(source) ? "Scan a repository" : "Search for a skill"}
              description={isDirectSkillSource(source) ? "Click Scan to inspect available skills." : "Click Search to find matching skills."}
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
                    <span className="sourceCandidateCopy">
                      <strong className="dataCellTitle">
                        {skill.name}
                      </strong>
                      <small className="dataCellSub">
                        {skill.source}{skill.description ? ` · ${skill.description}` : ""}
                      </small>
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
          {marketplaceNotice && <div className="dialogError" data-selectable-text>{marketplaceNotice}</div>}
          {marketplaceError ? <Toast tone="error" message={marketplaceError} /> : null}
        </div>}
        {plan && !reviewingSkills && (
          <div className="addSkillQuickSetup">
            <div className="addSkillQuickField">
              <span className="addSkillQuickLabel">Skills</span>
              <button
                type="button"
                className="addSkillSelectionCard"
                disabled={installing}
                onClick={() => {
                  if (busy) return;
                  setAdvancedOpen(false);
                  setReviewingSkills(true);
                }}
              >
                <BadgeList items={selected} ariaLabel="Selected skills" active={open && Boolean(plan) && !reviewingSkills} />
                <ChevronRight size={16} aria-hidden="true" />
              </button>
            </div>
            <div className="addSkillQuickField">
              <span className="addSkillQuickLabel">Install to</span>
              <SelectControl
                value={resolvedTarget}
                onValueChange={(value) => {
                  if (busy) return;
                  setTarget(value);
                }}
                label="Skill install target"
                disabled={installing}
                contentClassName="dialogSelectContent"
                options={visibleInstallTargets.map((option) => ({ value: option.id, label: option.displayName }))}
                side="bottom"
                align="start"
                renderValue={(option) => option ? <AgentOptionLabel agent={option.value} label={option.label} /> : null}
                renderOption={(option) => <AgentOptionLabel agent={option.value} label={option.label} />}
              />
            </div>
            <button
              type="button"
              className="addSkillAdvancedTrigger"
              disabled={installing}
              onClick={() => {
                if (busy) return;
                setAdvancedOpen((current) => !current);
              }}
            >
              <span>Advanced settings</span>
              {advancedOpen
                ? <ChevronLeft size={16} aria-hidden="true" />
                : <ChevronRight size={16} aria-hidden="true" />}
            </button>
          </div>
        )}
      </div>}
      {error ? <Toast tone="error" message={error} /> : null}
      {plan && reviewingSkills && (
        <div className="addSkillReview">
          <div className="addSkillResults">
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
                <span>{allSelected ? "Select all" : `Select ${selected.length}/${selectableSkills.length}`}</span>
              </div>
              {shouldShowSkillQuickSelect(available.length, existingSkills.length) && (
                <SegmentedControl
                  className="addSkillQuickSelect"
                  value={skillFilter}
                  onValueChange={(value) => {
                    if (value === SkillInstallFilter.All || value === SkillInstallFilter.New || value === SkillInstallFilter.Existing) selectSkillPreset(value);
                  }}
                  aria-label="Quick select and filter skills"
                >
                  {([
                    [SkillInstallFilter.All, "All", available.length],
                    [SkillInstallFilter.New, "New", newSkills.length],
                    [SkillInstallFilter.Existing, "Existing", existingSkills.length],
                  ] as const).map(([value, label, count]) => (
                    <SegmentedControlItem
                      value={value}
                      key={value}
                    >
                      {label} <span className="addSkillQuickSelectCount">{count}</span>
                    </SegmentedControlItem>
                  ))}
                </SegmentedControl>
              )}
            </div>
          {showSkillSearch && (
            <SearchField
              className="addSkillReviewSearch"
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
          <div className="addSkillList" ref={skillListRef}>
            {normalizedSkillSearch && visibleAvailableSkills.length === 0 ? (
              <EmptyState
                compact
                title="No matching skills"
                description="Try a different filter."
              />
            ) : visibleAvailableSkills.map((skill) => {
              const operation = operationByName.get(skill.name);
              const blocked = !isSelectableOperationStatus(operation?.status);
              const requiredBy = dependencyReasonsByName.get(skill.name) ?? [];
              const lockedDependency = requiredBy.length > 0 && !selectedRootSet.has(skill.name);
              const statusLabel = skillOperationStatusLabel(operation?.status);
              const searchMatch = normalizedSkillSearch && searchMatchSet.has(skill.name);
              const firstMatch = skill.name === firstSearchMatch;
              return (
              <div
                className={`addSkillItem ${selectedSet.has(skill.name) ? "selected" : ""} ${blocked || lockedDependency ? "blocked" : ""} ${searchMatch ? "searchMatch" : ""} ${firstMatch ? "firstSearchMatch" : ""} ${skillPreview?.name === skill.name ? "previewing" : ""}`}
                key={`${skill.name}-${skill.relative_path}`}
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
                  <span className="addSkillItemTitle">
                    {skill.name}
                    {statusLabel && <Badge tone="meta">{statusLabel}</Badge>}
                    {requiredBy.length > 0 && <Badge tone="info">Required</Badge>}
                  </span>
                </span>
                <button
                  type="button"
                  className="addSkillPreviewButton"
                  aria-label={`Preview ${skill.name} SKILL.md`}
                  aria-busy={skillPreviewBusy === skill.name}
                  disabled={Boolean(skillPreviewBusy)}
                  onClick={(event) => {
                    event.stopPropagation();
                    void previewSkill(skill.name);
                  }}
                >
                  {skillPreviewBusy === skill.name
                    ? <LoadingIcon size={14} />
                    : <><span>Preview</span><ChevronRight size={14} aria-hidden="true" /></>}
                </button>
              </div>
            );})}
          </div>
          {skillPreviewError ? <Toast tone="error" message={skillPreviewError} /> : null}
          </div>
        </div>
      )}
        </div>
      {plan && reviewingSkills && (
        <section
          className={`skillMarkdownPreview ${skillPreview ? "isOpen" : ""}`}
          aria-label="Skill preview"
          aria-hidden={!skillPreview}
          data-no-drag
        >
          {skillPreview && (
            <>
              <IconButton
                className="skillPreviewCloseButton"
                aria-label="Close skill preview"
                onClick={() => setSkillPreview(null)}
              >
                <X size={14} />
              </IconButton>
              <div className="skillMarkdownPreviewBody">
                <Suspense fallback={<LoadingState label="Loading preview" />}>
                  <TiptapMarkdownPreview content={skillPreview.content} />
                </Suspense>
              </div>
            </>
          )}
        </section>
      )}
      {plan && (
        <aside
          className={`addSkillAdvancedPanel ${advancedOpen ? "isOpen" : ""}`}
          aria-label="Advanced settings"
          aria-hidden={!advancedOpen}
        >
          <header className="addSkillAdvancedHeader">
            <strong>Advanced settings</strong>
            <IconButton aria-label="Close advanced settings" onClick={() => setAdvancedOpen(false)}>
              <X size={14} />
            </IconButton>
          </header>
          <div className="addSkillAdvancedBody">
            <div className="dialogField">
              <span>Visibility</span>
              <SelectControl
                value={visibility}
                onValueChange={(value) => {
                  if (busy) return;
                  setVisibility(value as SkillVisibility);
                }}
                label="Skill visibility"
                disabled={installing}
                contentClassName="dialogSelectContent"
                options={editableSkillVisibilities.map((option) => ({ value: option, label: option }))}
                side="bottom"
                align="start"
              />
            </div>
            <div className="dialogField">
              <span>Install mode</span>
              <SegmentedControl
                fullWidth
                value={copy ? SkillDistributionMode.Copy : SkillDistributionMode.Symlink}
                disabled={installing}
                onValueChange={(value) => {
                  if (!value || busy) return;
                  const nextCopy = value === SkillDistributionMode.Copy;
                  setCopy(nextCopy);
                }}
                aria-label="Skill install mode"
              >
                <SegmentedControlItem value={SkillDistributionMode.Symlink} aria-label="Install using symlink">
                  Symlink
                </SegmentedControlItem>
                <SegmentedControlItem value={SkillDistributionMode.Copy} aria-label="Install by copying files">
                  Copy
                </SegmentedControlItem>
              </SegmentedControl>
            </div>
            <label className="addSkillAdvancedCheckbox">
              <SelectionCheckbox
                  checked={replaceExisting}
                  label="Replace existing skills"
                  disabled={!selectedHasExisting || installing}
                  onChange={(checked) => {
                    if (busy) return;
                    setReplaceExisting(checked);
                  }}
              />
              <span>Replace existing skills</span>
            </label>
            {selectedRoots.length > 1 && (
              <label className="addSkillAdvancedCheckbox">
                <SelectionCheckbox
                  checked={createWrapper}
                  label="Create a wrapper skill after installation"
                  disabled={installing}
                  onChange={(checked) => {
                    if (busy) return;
                    setCreateWrapper(checked);
                  }}
                />
                <span>Create wrapper after install</span>
              </label>
            )}
          </div>
        </aside>
      )}
      </div>
      <DialogActionBar
        onCancel={closeDialog}
        cancelDisabled={dialogBusy}
        leading={canGoBack ? (
          <DialogActionButton variant="secondary" disabled={dialogBusy} onClick={goBack}>Back</DialogActionButton>
        ) : null}
      >
        {plan && reviewingSkills ? (
          <DialogActionButton variant="primary" onClick={() => setReviewingSkills(false)}>Done</DialogActionButton>
        ) : plan ? (
          <DialogAdvanceButton
            label={advanceText}
            ariaLabel={advanceLabel}
            busy={busy}
            disabled={!canInstall}
            onClick={advance}
          />
        ) : !busy && !marketplaceBusy && lockedSource ? (
          <DialogActionButton
            variant="primary"
            disabled={!initialSource.trim()}
            aria-label="Retry skill preview"
            onClick={() => { void previewSource(initialSource); }}
          >
            Retry
          </DialogActionButton>
        ) : !busy && !marketplaceBusy ? (
          <DialogActionButton
            variant="primary"
            disabled={!isSkillSourceActionReady(source, isDirectSkillSource(source), resolvedTarget)}
            aria-label={isDirectSkillSource(source) ? "Scan repository" : "Search marketplaces"}
            onClick={resolveSourceInput}
          >
            {isDirectSkillSource(source) ? "Scan" : "Search"}
          </DialogActionButton>
        ) : null}
      </DialogActionBar>
    </DialogShell>
  );
}

export type SkillsViewProps = {
  openSkill: (skill: NormalizedSkill) => void;
  skills: NormalizedSkill[];
  loadingSkills: boolean;
  loadError: string;
  hasRows: boolean;
  checkingUpdates: boolean;
  updateError: string;
  onRefresh: () => void | Promise<void>;
  onSkillsUpdated: (skills: RawSkillRecord[], options?: { patch?: boolean; deleted?: string[] }) => void;
  onSetVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
  onApplyWrapper: (args: WrapperArgs) => Promise<SkillChangeResponse>;
  onApplyUpdates: (names: string[], onApplied?: () => void) => void;
  onDeleteSkills: (names: string[], onApplied?: () => void) => void;
  onAddInstalled: (result: SkillInstallResult) => void;
  installedAgentKeys: string[];
  targetOptions: SkillTargetOption[];
  projects?: ProjectSummary[];
};

export function SkillsView({
  openSkill,
  skills: skillItems,
  loadingSkills,
  loadError,
  hasRows,
  checkingUpdates,
  updateError,
  onRefresh,
  onSkillsUpdated,
  onSetVisibility,
  onApplyWrapper,
  onApplyUpdates,
  onDeleteSkills,
  onAddInstalled,
  installedAgentKeys,
  targetOptions,
  projects = [],
}: SkillsViewProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [viewMode, setViewMode] = useState<SkillsViewMode>(SkillsViewMode.List);
  const [showWrapper, setShowWrapper] = useState(false);
  const [showAddSkill, setShowAddSkill] = useState(false);
  const [skillAddError, setSkillAddError] = useState("");
  const [installedWrapperSkills, setInstalledWrapperSkills] = useState<SkillWrapperSelection[]>([]);
  const [skillLocatorRequest, setSkillLocatorRequest] = useState("");
  const [locationSkills, setLocationSkills] = useState<NormalizedSkill[]>([]);
  const [locationAgent, setLocationAgent] = useState<string | undefined>(undefined);
  const normalizedQuery = query.trim().toLowerCase();
  const skillListView = useMemo(() => selectSkillListView(skillItems, query, selected), [query, selected, skillItems]);
  const { visibleSkills, selectedSkills } = skillListView;
  const tableRows = visibleSkills;
  const networkNodes = useMemo(
    () => visibleSkills.map((skill) => ({
      id: skill.id,
      name: skill.name,
      label: skill.name,
      description: skill.description,
      dependencies: skill.dependencies,
      dependents: skill.dependents,
      dependencyIds: skill.dependencyIds,
      dependentIds: skill.dependentIds,
      kind: skill.isWrapper ? RelationshipGraphKind.Wrapper : skill.section.toLowerCase(),
    })),
    [visibleSkills],
  );
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
  const openWrapper = useCallback((skill: NormalizedSkill) => {
    setSelected([skill.id]);
    setShowWrapper(true);
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
  const openManageLocations = useCallback((skill: NormalizedSkill, agent?: string) => {
    setLocationSkills([skill]);
    setLocationAgent(agent);
  }, []);
  const openManageLocationsBatch = useCallback((skills: NormalizedSkill[]) => {
    const movableSkills = skills.filter((skill) => !isReadOnlySkillSource(skill) && skillTargets(skill).length > 0);
    if (movableSkills.length === 0) return;
    setLocationSkills(movableSkills);
    setLocationAgent(undefined);
  }, []);
  const applyLocationsAndClear = useCallback(async (
    skills?: RawSkillRecord[],
    options?: { patch?: boolean; deleted?: string[] },
  ) => {
    setLocationSkills([]);
    setLocationAgent(undefined);
    clearSelection();
    if (skills || options?.deleted?.length) onSkillsUpdated(skills ?? [], { patch: true, deleted: options?.deleted });
    else await onRefresh();
  }, [clearSelection, onRefresh, onSkillsUpdated]);
  const handleInstalled = useCallback((result: SkillInstallResult) => {
    onAddInstalled(result);
    const name = installedSkillName(result);
    if (!name) return;
    setQuery("");
    setViewMode(SkillsViewMode.List);
    setSkillLocatorRequest(name);
  }, [onAddInstalled]);
  const handleAddSkillOpenChange = useCallback((open: boolean) => {
    setShowAddSkill(open);
    if (open) setSkillAddError("");
  }, []);
  const completeSkillLocator = useCallback((rowId: string) => {
    setSkillLocatorRequest((current) => current === rowId ? "" : current);
  }, []);

  const columns = useMemo((): ColumnDef<SkillTableRow>[] => [
    {
      key: "main",
      header: "Skill",
      type: ColumnDataType.Text,
      sortValue: (skill) => skill.name.toLowerCase(),
      width: "minmax(250px, 1fr)",
      render: (skill) => <SkillMainCell skill={skill} openSkill={openSkill} onApplyUpdates={applyUpdatesAndClear} />,
    },
    {
      key: "agents",
      header: "Agents",
      type: ColumnDataType.Enum,
      groupBy: (skill) => skill.agents.join(", "),
      sortValue: (skill) => skill.agents.join(",").toLowerCase(),
      width: "90px",
      render: (skill) => <AgentChips agents={skill.agents} onAgentClick={(agent) => openManageLocations(skill, agent)} />,
    },
    {
      key: "origin",
      header: "Origin",
      type: ColumnDataType.Enum,
      groupBy: (skill) => skill.section,
      groupOrder: ["Local", "Remote", "Plugin", "System"],
      sortValue: (skill) => skillOriginLabel(skill).toLowerCase(),
      width: "128px",
      value: skillOriginLabel,
    },
    ...(projects.length > 0 ? [scopeColumn<SkillTableRow>(projects, (skill) => primarySkillPath(skill))] : []),
    {
      key: "visibility",
      header: "Visibility",
      type: ColumnDataType.Enum,
      groupOrder: [...allSkillVisibilities],
      sortValue: (skill) => skill.visibility.toLowerCase(),
      width: "98px",
      render: (skill) => <Visibility value={skill.visibility} skill={skill} onSetVisibility={setVisibilityAndClear} />,
    },
    {
      key: "ctime",
      header: "Created",
      type: ColumnDataType.Date,
      sortValue: (skill) => skill.ctime ?? "",
      width: "98px",
      value: (skill) => compactDateTime(skill.ctime),
      empty: "",
    },
    {
      key: "mtime",
      header: "Updated",
      type: ColumnDataType.Date,
      sortValue: (skill) => skill.mtime ?? "",
      width: "98px",
      value: (skill) => compactDateTime(skill.mtime),
      empty: "",
    },
    {
      key: "actions",
      header: "",
      width: "40px",
      render: (skill) => <SkillActionsCell skill={skill} onApplyUpdates={applyUpdatesAndClear} onDeleteSkills={deleteSkillsAndClear} onManageLocations={openManageLocations} onCreateWrapper={openWrapper} onSetVisibility={setVisibilityAndClear} />,
    },
  ], [applyUpdatesAndClear, deleteSkillsAndClear, openManageLocations, openSkill, openWrapper, projects, setVisibilityAndClear]);

  const rowContextMenu = useCallback((skill: SkillTableRow, { selectedRows, selected: isSelected }: { selectedRows: SkillTableRow[]; selected: boolean }) => {
    const showBulk = isSelected && selectedRows.length > 1;
    return showBulk ? (
      <BulkSkillActionsMenuItems
        Menu={ContextMenu}
        selectedSkills={selectedRows}
        onApplyUpdates={applyUpdatesAndClear}
        onDeleteSkills={deleteSkillsAndClear}
        onManageLocations={openManageLocationsBatch}
        createWrapper={() => setShowWrapper(true)}
        setVisibility={setVisibilityAndClear}
      />
    ) : (
      <SkillActionsMenuItems Menu={ContextMenu} skill={skill} onApplyUpdates={applyUpdatesAndClear} onDeleteSkills={deleteSkillsAndClear} onManageLocations={openManageLocations} onCreateWrapper={openWrapper} onSetVisibility={setVisibilityAndClear} />
    );
  }, [applyUpdatesAndClear, deleteSkillsAndClear, openManageLocations, openManageLocationsBatch, openWrapper, setVisibilityAndClear]);

  const bottomBar = useCallback((selectedRows: SkillTableRow[]) => (
    <SkillSelectionActions
      selectedSkills={selectedRows}
      applyUpdates={applyUpdatesAndClear}
      deleteSkills={deleteSkillsAndClear}
      manageLocations={() => openManageLocationsBatch(selectedRows)}
      createWrapper={() => setShowWrapper(true)}
      setVisibility={setVisibilityAndClear}
    />
  ), [applyUpdatesAndClear, deleteSkillsAndClear, openManageLocationsBatch, setVisibilityAndClear]);

  return (
    <>
      {skillAddError ? <Toast tone="error" message={skillAddError} onDismiss={() => setSkillAddError("")} /> : null}
      <section className="content skillsPage">
      <ContentTopDragStrip />
      <PageHeader title="Skills">
        <SegmentedControl
          variant="icon"
          value={viewMode}
          onValueChange={(value) => {
            if (value === SkillsViewMode.List || value === SkillsViewMode.Network) setViewMode(value);
          }}
          aria-label="Skills view"
        >
          <SegmentedControlItem value={SkillsViewMode.List} aria-label="Show skills as a list">
            <List size={15} aria-hidden="true" />
          </SegmentedControlItem>
          <SegmentedControlItem value={SkillsViewMode.Network} aria-label="Show skill relationships">
            <Waypoints size={15} aria-hidden="true" />
          </SegmentedControlItem>
        </SegmentedControl>
        <SearchField pageSearch placeholder="Search skills" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
        <IconButton
          disabled={loadingSkills || checkingUpdates}
          onClick={onRefresh}
          aria-label="Refresh skills and check updates"
          aria-busy={loadingSkills || checkingUpdates}
        >
          {loadingSkills || checkingUpdates ? <LoadingIcon size={16} /> : <RefreshCw size={16} />}
        </IconButton>
        {updateError ? <Toast tone="error" message={updateError} /> : null}
        <AddSkillDialog
          open={showAddSkill}
          onOpenChange={handleAddSkillOpenChange}
          trigger={(
            <Dialog.Trigger asChild>
              <IconButton className="filled" aria-label="Add skill"><Plus size={16} /></IconButton>
            </Dialog.Trigger>
          )}
          onClose={() => setShowAddSkill(false)}
          onPreviewError={setSkillAddError}
          onInstalled={handleInstalled}
          installedAgentKeys={installedAgentKeys}
          targetOptions={targetOptions}
          onRequestWrapper={(skills) => {
            setInstalledWrapperSkills(skills);
            setShowWrapper(true);
          }}
        />
      </PageHeader>
      {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={() => { void onRefresh(); }} /> : null}
      {viewMode === SkillsViewMode.Network ? (
        <SkillRelationshipMap
          nodes={networkNodes}
          loading={loadingSkills}
          error={hasRows ? "" : loadError}
          onRetry={() => { void onRefresh(); }}
          onOpenSkill={(selector) => {
            const skill = findSkillBySelector(skillItems, selector);
            if (skill) openSkill(skill);
          }}
        />
      ) : (
        <div className="skillsListBody">
          <DataTable
            rows={tableRows}
            columns={columns}
            getRowId={(skill) => skill.id}
            getRowLabel={(skill) => skill.name}
            freezeColumn={SKILL_FREEZE_COLUMN}
            selectable={(skill) => isSkillRowSelectable(skill)}
            selectedIds={selected}
            onSelectionChange={setSelected}
            enableMarquee
            scrollToRowId={skillLocatorRequest}
            onScrollToRowComplete={completeSkillLocator}
            defaultGroupBy="origin"
            defaultSort={{ key: "mtime", direction: SortDirection.Desc }}
            onRowClick={openSkill}
            rowContextMenu={rowContextMenu}
            bottomBar={bottomBar}
            bottomBarActionsClassName="selectionActions"
            bottomBarCheckboxLabel="Select visible skills from toolbar"
            selectionLabel="skills"
            loading={loadingSkills && !hasRows}
            loadingLabel="Loading skills"
            emptyState={loadError && !hasRows ? <LoadErrorState message={loadError} onRetry={() => { void onRefresh(); }} /> : (
              <EmptyState
                icon={normalizedQuery ? <SearchX size={21} strokeWidth={1.8} /> : <Hammer size={27} strokeWidth={1.55} />}
                iconTone={normalizedQuery ? "muted" : "accent"}
                title={normalizedQuery ? "No skills match this search" : "No skills yet"}
                description={normalizedQuery ? "Try another search or clear filters." : "Add a skill to install and manage it here."}
              />
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
      <SkillLocationDialog
        open={locationSkills.length > 0}
        skills={locationSkills}
        initialAgent={locationAgent}
        installedAgentKeys={installedAgentKeys}
        targetOptions={targetOptions}
        onOpenChange={(open) => {
          if (!open) {
            setLocationSkills([]);
            setLocationAgent(undefined);
          }
        }}
        onApplied={applyLocationsAndClear}
      />
      </section>
    </>
  );
}
