import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ComponentType, ReactNode } from "react";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import {
  ChevronLeft,
  ChevronRight,
  Check,
  Code2,
  Copy,
  Delete as DeleteKeyIcon,
  ArrowRightLeft,
  CircleFadingArrowUp,
  Eye,
  FolderOpen,
  Hammer,
  List,
  Package,
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
import { MenuShortcut, OpenInEditorMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DialogActionButton } from "../components/shared/DialogActionButton.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogApplyButton } from "../components/shared/DialogApplyButton.tsx";
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
import { SkillWrapperScopeDialog } from "../features/skills/SkillWrapperScopeDialog.tsx";
import { useWrapperMutation } from "../features/skills/use-wrapper-mutation.ts";
import { Visibility } from "../features/skills/Visibility.tsx";
import { DataTable } from "../components/DataTable.tsx";
import { ColumnDataType, type ColumnDef, type SortState } from "../components/DataTable.types";
import { SortDirection } from "../lib/sort.ts";
import { useTabState } from "../lib/tab-state.ts";
import { actionLabels, isVisibleAgent, SKILL_FREEZE_COLUMN, scopeColumnFromValue, primarySkillPath, primarySkillScope, selectionDeleteLabel, SkillUpdateAvailability, TauriCommand, SkillOperationStatus, SkillVisibility, agentIdentityKey, allSkillVisibilities, compactDateTime, copyText, editableSkillVisibilities, isSkillRowSelectable, isSkillVisibilityEditable, safeInvoke, skillDisplayName, skillSourceAction, skillSourceDetails, skillTargets, sourceRemoteDetails, suppressNextClick, type NormalizedSkill, type ProjectSummary, type RawSkillRecord, type SkillAddPlan, type SkillInstallResult, type WrapperArgs } from "../lib/index.ts";
import { resolveSkillInstallTarget, shouldShowSkillQuickSelect } from "../lib/add-skill-dialog.ts";
import { SkillActionId, skillActionIds } from "../lib/skill-actions.ts";
import {
  buildSkillInstallViewModel,
  canInstallSkillSelection,
  canToggleSkillInstallRoot,
  installedSkillName,
  isSkillInstallDependencyLocked,
  isSelectableOperationStatus,
  selectMovableSkills,
  selectSkillInstallTargetOptions,
  SkillInstallFilter,
  skillSelectionTargets,
  selectSkillListView,
  suggestedWrapperDescription,
  suggestedWrapperName,
  skillInstallRootsForPreset,
} from "../controllers/skill-controller.ts";
import {
  SkillDistributionMode,
  type SkillChangeResponse,
} from "../lib/runtime-gateway.ts";
import { SkillAddBusyAction, useAddSkillFlow } from "../features/skills/use-add-skill-flow.ts";

enum SkillsViewMode {
  List = "list",
  Network = "network",
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
  const editableSkills = selectedSkills.filter(isSkillVisibilityEditable);
  const names = editableSkills.map((skill) => skill.id);
  const activeVisibility = editableSkills.every((skill) => skill.visibility === editableSkills[0]?.visibility)
    ? editableSkills[0]?.visibility
    : undefined;
  const disabled = names.length === 0;
  return (
    <>
      {editableSkillVisibilities.map((visibility) => (
        <Menu.Item
          className="menuItem selectItemIndicatorRight"
          disabled={disabled}
          key={visibility}
          onSelect={() => onSetVisibility(names, visibility)}
        >
          <span className="selectItemText">{visibility}</span>
          <span className="selectItemLeadingIcon" aria-hidden="true">
            {visibility === activeVisibility ? <Check className="selectItemIndicator" size={14} /> : null}
          </span>
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
        <Menu.SubTrigger className="menuItem menuSubTrigger">
          <Icon size={14} />
          <span>{label}</span>
          <ChevronRight className="menuSubIcon" size={14} />
        </Menu.SubTrigger>
        <Menu.Portal>
          <Menu.SubContent className="menuContent" sideOffset={8} alignOffset={-6}>
            {targets.map((target) => (
              <Menu.Item className="menuItem" key={`${action}-${target.id}`} onSelect={() => runSkillPathAction(action, target.path)}>
                <AgentOptionLabel agent={target.agent} label={target.label} />
              </Menu.Item>
            ))}
          </Menu.SubContent>
        </Menu.Portal>
      </Menu.Sub>
    );
  }
  return (
    <Menu.Item className="menuItem" disabled={!primaryPath} onSelect={() => primaryPath && runSkillPathAction(action, primaryPath)}>
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
  const { updateNames, deletableNames } = skillSelectionTargets(selectedSkills);
  const movableSkills = selectMovableSkills(selectedSkills);
  const updateLabel = "Update";
  const locationLabel = selectedSkills.length === 1 ? "Manage locations" : "Locations";
  const deleteLabel = selectionDeleteLabel("skill", selectedSkills.length);
  const visibilityMenu = (
    <Menu.Sub>
      <Menu.SubTrigger className="menuItem menuSubTrigger">
        <Eye size={14} />
        <span>Visibility</span>
        <ChevronRight className="menuSubIcon" size={14} />
      </Menu.SubTrigger>
      <Menu.Portal>
        <Menu.SubContent className="menuContent" sideOffset={8} alignOffset={-6}>
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
      separatorBefore: true,
    },
    [SkillActionId.Update]: {
      id: SkillActionId.Update,
      direct: <button className="skillApplyUpdatesButton" aria-label={updateLabel} disabled={updateNames.length === 0} onClick={() => updateNames.length > 0 && applyUpdates(updateNames)}><RefreshCw size={15} aria-hidden="true" /><span>{updateLabel}{selectedSkills.length > 1 && updateNames.length ? ` (${updateNames.length})` : ""}</span></button>,
      menu: <Menu.Item className="menuItem" disabled={updateNames.length === 0} onSelect={() => { if (updateNames.length === 0) return; suppressNextClick(); applyUpdates(updateNames); }}><RefreshCw size={14} />{updateLabel}{selectedSkills.length > 1 && updateNames.length ? ` (${updateNames.length})` : ""}</Menu.Item>,
      measure: <><RefreshCw size={15} /><span>{updateLabel}{selectedSkills.length > 1 && updateNames.length ? ` (${updateNames.length})` : ""}</span></>,
      separatorBefore: true,
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
      menu: <Menu.Item className="menuItem" onSelect={() => { suppressNextClick(); createWrapper(selectedSkills); }}><PackagePlus size={14} />Create wrapper</Menu.Item>,
      measure: <><PackagePlus size={15} /><span>Create wrapper</span></>,
      separatorBefore: true,
    },
    [SkillActionId.Locations]: {
      id: SkillActionId.Locations,
      direct: <button aria-label={locationLabel} disabled={movableSkills.length === 0} onClick={() => movableSkills.length > 0 && manageLocations(movableSkills)}><ArrowRightLeft size={15} /><span>{locationLabel}</span></button>,
      menu: <Menu.Item className="menuItem" disabled={movableSkills.length === 0} onSelect={() => { if (movableSkills.length === 0) return; suppressNextClick(); manageLocations(movableSkills); }}><ArrowRightLeft size={14} />{locationLabel}</Menu.Item>,
      measure: <><ArrowRightLeft size={15} /><span>{locationLabel}</span></>,
    },
    [SkillActionId.Delete]: {
      id: SkillActionId.Delete,
      direct: <button className="danger" aria-label={deleteLabel} disabled={deletableNames.length === 0} onClick={() => deletableNames.length > 0 && deleteSkills(deletableNames)}><Trash2 size={15} /><span>{deleteLabel}</span></button>,
      menu: <Menu.Item className="menuItem danger" disabled={deletableNames.length === 0} onSelect={() => { if (deletableNames.length === 0) return; suppressNextClick(); deleteSkills(deletableNames); }}><Trash2 size={14} />{deleteLabel}<MenuShortcut><DeleteKeyIcon size={14} strokeWidth={1.8} /></MenuShortcut></Menu.Item>,
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
  onManageWrapperScope: (skill: NormalizedSkill) => void;
  onApplyUpdates: (names: string[]) => void;
};

export function SkillMainCell({ skill, openSkill, onManageWrapperScope, onApplyUpdates }: SkillMainCellProps) {
  const sourceDetails = skillSourceDetails(skill);
  const sourceAction = skill.section === "Local" ? null : skillSourceAction(skill, sourceDetails);
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
          <span className="skillNameText">{skillDisplayName(skill)}</span>
        </button>
        {skill.isWrapper && (
          <button
            className="skillSourceButton skillWrapperButton"
            type="button"
            aria-label={`Manage wrapper scope for ${skillDisplayName(skill)}`}
            onClick={(event) => {
              event.stopPropagation();
              onManageWrapperScope(skill);
            }}
          >
            <Package size={16} aria-hidden="true" />
          </button>
        )}
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
          <button
            className="skillSourceButton skillUpdateButton"
            type="button"
            aria-label={`Update ${skillDisplayName(skill)}`}
            onClick={(event) => {
              event.stopPropagation();
              onApplyUpdates([skill.id]);
            }}
          >
            <CircleFadingArrowUp size={16} aria-hidden="true" />
          </button>
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
      ariaLabel={`Skill actions for ${skillDisplayName(skill)}`}
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
  const { apply: applyWrapper, busy, clearError, error } = useWrapperMutation({ onApplyWrapper });
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
    clearError();
  }, [clearError, open, selectedSkillIdentity]);

  useEffect(() => {
    clearError();
  }, [clearError, description, manualChildren, name, selectedNames]);

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
    const outcome = await applyWrapper(args);
    if (outcome.status === "succeeded") onOpenChange(false);
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

const TiptapMarkdownPreview = lazy(() => import("../components/shared/TiptapMarkdownPreview.tsx").then(({ TiptapMarkdownPreview: component }) => ({ default: component })));

export type AddSkillDialogProps = {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  trigger?: ReactNode;
  onClose: () => void;
  onPreviewError: (message: string) => void;
  onInstalled: (result: SkillInstallResult) => void;
  onBeginMutation: () => () => void;
  onRequestWrapper: (skills: SkillWrapperSelection[]) => void;
  installedAgentKeys: string[];
  targetOptions: SkillTargetOption[];
  initialSource?: string;
  sourceLocked?: boolean;
  title?: string;
};

const DIALOG_CLOSE_ANIMATION_MS = 220;
const LOCATION_DIALOG_CLOSE_ANIMATION_MS = 260;

export type SkillTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
  globalPath?: string;
};

export function AddSkillDialog({ open, onOpenChange, trigger, onClose, onPreviewError, onInstalled, onBeginMutation, onRequestWrapper, installedAgentKeys, targetOptions, initialSource = "", sourceLocked = false, title = "Add skills" }: AddSkillDialogProps) {
  const lockedSource = sourceLocked && Boolean(initialSource.trim());
  const [source, setSource] = useState(initialSource);
  const [target, setTarget] = useState("");
  const [copy, setCopy] = useState(false);
  const [visibility, setVisibility] = useState<SkillVisibility>(SkillVisibility.Auto);
  const [selectedRoots, setSelectedRoots] = useState<string[]>([]);
  const [createWrapper, setCreateWrapper] = useState(false);
  const [replaceExisting, setReplaceExisting] = useState(false);
  const [skillFilter, setSkillFilter] = useState<SkillInstallFilter>(SkillInstallFilter.All);
  const [reviewingSkills, setReviewingSkills] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [skillSearch, setSkillSearch] = useState("");
  const skillItemRefs = useRef(new Map<string, HTMLDivElement>());
  const skillListRef = useRef<HTMLDivElement>(null);
  const visibleInstallTargets = useMemo(
    () => selectSkillInstallTargetOptions(targetOptions, installedAgentKeys, agentIdentityKey, isVisibleAgent),
    [targetOptions, installedAgentKeys],
  );
  const resolvedTarget = resolveSkillInstallTarget(target, visibleInstallTargets);
  useEffect(() => {
    if (!visibleInstallTargets.some((option) => option.id === target)) {
      setTarget(visibleInstallTargets[0]?.id ?? "");
    }
  }, [target, visibleInstallTargets]);
  const handlePlanReady = useCallback((nextPlan: SkillAddPlan) => {
    setCreateWrapper(false);
    setReplaceExisting(false);
    setReviewingSkills(false);
    setSkillFilter(SkillInstallFilter.All);
    setSkillSearch("");
    setSelectedRoots(nextPlan.selected.map((skill) => skill.name));
  }, []);
  const handlePageRestored = useCallback(() => {
    setSelectedRoots([]);
    setSkillFilter(SkillInstallFilter.All);
    setSkillSearch("");
    setReplaceExisting(false);
    setCreateWrapper(false);
    setReviewingSkills(false);
    setAdvancedOpen(false);
  }, []);
  const handleSourceChanged = useCallback(() => {
    setSelectedRoots([]);
    setCopy(false);
    setReviewingSkills(false);
    setAdvancedOpen(false);
  }, []);
  const flow = useAddSkillFlow({
    open,
    initialSource,
    lockedSource,
    source,
    onSourceChange: setSource,
    target: resolveSkillInstallTarget(target, visibleInstallTargets),
    copy,
    visibility,
    replaceExisting,
    onPlanReady: handlePlanReady,
    onPageRestored: handlePageRestored,
    onSourceChanged: handleSourceChanged,
    onPreviewError,
    onInstalled,
    onBeginMutation,
    onRequestWrapper,
    onClose,
  });
  const {
    marketplaceBusy,
    marketplaceError,
    marketplaceNotice,
    skillPreview,
    skillPreviewBusy,
    skillPreviewError,
    plan,
    busyAction,
    error,
    busy,
    installing,
    dialogBusy,
    sourceActionReady,
    sourceActionText,
    sourceActionLabel,
    sourceCandidates,
    sourceCandidatesLabel,
    canGoBack,
  } = flow;
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
  const canInstall = canInstallSkillSelection({
    resolvedTarget,
    source,
    hasPlan: Boolean(plan),
    selected,
    selectedHasExisting,
    replaceExisting,
    busy,
  });
  const advanceLabel = busy ? "Preparing installation" : "Install selected skills";
  const advanceText = busy ? "Installing" : `Install ${selected.length}`;

  const resetDialogState = useCallback(() => {
    flow.reset();
    setTarget("");
    setCopy(false);
    setVisibility(SkillVisibility.Auto);
    setSelectedRoots([]);
    setCreateWrapper(false);
    setReplaceExisting(false);
    setSkillFilter(SkillInstallFilter.All);
    setReviewingSkills(false);
    setAdvancedOpen(false);
    setSkillSearch("");
    skillItemRefs.current.clear();
  }, [flow.reset]);

  const closeDialog = () => {
    if (dialogBusy) return;
    onClose();
  };

  const goBack = () => {
    if (dialogBusy) return;
    if (!canGoBack) return;
    if (reviewingSkills) {
      setReviewingSkills(false);
      flow.clearSkillPreview();
      return;
    }
    flow.restorePreviousPage();
  };
  const advance = () => {
    if (busy) return;
    if (plan && canInstall) {
      void flow.install({ selected, selectedRoots, available, createWrapper });
    }
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
    if (open || dialogBusy) return;
    const timeoutId = window.setTimeout(resetDialogState, DIALOG_CLOSE_ANIMATION_MS);
    return () => window.clearTimeout(timeoutId);
  }, [dialogBusy, open, resetDialogState]);

  const toggleSkill = (name: string) => {
    if (!canToggleSkillInstallRoot(name, selectedSet, selectedRootSet, operationByName, dependencyReasonsByName)) return;
    setSelectedRoots((current) => current.includes(name) ? current.filter((item) => item !== name) : [...current, name]);
  };

  const toggleAll = () => {
    setSelectedRoots(allSelected ? [] : selectableSkills.map((skill) => skill.name));
  };

  const selectSkillPreset = (preset: SkillInstallFilter) => {
    setSkillFilter(preset);
    setSelectedRoots(skillInstallRootsForPreset(
      preset,
      selectableSkills,
      newSkills,
      existingSkills,
      selectableNameSet,
    ));
  };

  return (
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen && dialogBusy) return;
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
                flow.resolveSourceInput();
              }}
            >
              <SearchField
                value={source}
                onChange={(event) => flow.handleSourceChange(event.target.value)}
                onClear={() => flow.handleSourceChange("")}
                onKeyDown={(event) => {
                  if (event.key !== "Enter") return;
                  event.preventDefault();
                  flow.resolveSourceInput();
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
              title={flow.sourceEmptyTitle}
              description={flow.sourceEmptyDescription}
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
                    onClick={() => flow.selectMarketplaceSource(skill)}
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
              const lockedDependency = isSkillInstallDependencyLocked(skill.name, selectedRootSet, dependencyReasonsByName);
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
                    void flow.previewSkill(skill.name);
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
                onClick={flow.clearSkillPreview}
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
          </header>
          <div className="addSkillAdvancedBody">
            <div className="dialogField">
              <span>Visibility</span>
              <SegmentedControl
                fullWidth
                value={visibility}
                onValueChange={(value) => {
                  if (!value || busy) return;
                  setVisibility(value as SkillVisibility);
                }}
                disabled={installing}
                aria-label="Skill visibility"
              >
                {editableSkillVisibilities.map((option) => (
                  <SegmentedControlItem key={option} value={option}>
                    {option}
                  </SegmentedControlItem>
                ))}
              </SegmentedControl>
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
          <DialogApplyButton
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
            onClick={() => { void flow.previewSource(initialSource); }}
          >
            Retry
          </DialogActionButton>
        ) : !busy && !marketplaceBusy ? (
          <DialogActionButton
            variant="primary"
            disabled={!sourceActionReady}
            aria-label={sourceActionLabel}
            onClick={flow.resolveSourceInput}
          >
            {sourceActionText}
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
  onBeginMutation: () => () => void;
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
  onBeginMutation,
  installedAgentKeys,
  targetOptions,
  projects = [],
}: SkillsViewProps) {
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useTabState("skills.query", "");
  const [viewMode, setViewMode] = useTabState("skills.viewMode", SkillsViewMode.List);
  const [sort, setSort] = useTabState<SortState | null>("skills.sort", { key: "ctime", direction: SortDirection.Desc });
  const [groupBy, setGroupBy] = useTabState<string | null>("skills.groupBy", "origin");
  const [showWrapper, setShowWrapper] = useState(false);
  const [wrapperScopeDialogOpen, setWrapperScopeDialogOpen] = useState(false);
  const [wrapperScopeSkillId, setWrapperScopeSkillId] = useState("");
  const [showAddSkill, setShowAddSkill] = useState(false);
  const [skillAddError, setSkillAddError] = useState("");
  const [installedWrapperSkills, setInstalledWrapperSkills] = useState<SkillWrapperSelection[]>([]);
  const [skillLocatorRequest, setSkillLocatorRequest] = useState("");
  const [locationSkillIds, setLocationSkillIds] = useState<string[]>([]);
  const [locationAgent, setLocationAgent] = useState<string | undefined>(undefined);
  const [locationDialogOpen, setLocationDialogOpen] = useState(false);
  const locationDialogCloseTimer = useRef<number | null>(null);
  const locationSkills = useMemo(() => {
    const skillsById = new Map(skillItems.map((skill) => [skill.id, skill]));
    return locationSkillIds.flatMap((id) => {
      const skill = skillsById.get(id);
      return skill ? [skill] : [];
    });
  }, [locationSkillIds, skillItems]);
  const wrapperScopeSkill = useMemo(
    () => skillItems.find((skill) => skill.id === wrapperScopeSkillId) ?? null,
    [skillItems, wrapperScopeSkillId],
  );
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
    setWrapperScopeDialogOpen(false);
    setWrapperScopeSkillId("");
  }, []);
  const openWrapper = useCallback((skill: NormalizedSkill) => {
    setSelected([skill.id]);
    setShowWrapper(true);
  }, []);
  const openWrapperScope = useCallback((skill: NormalizedSkill) => {
    setWrapperScopeSkillId(skill.id);
    setWrapperScopeDialogOpen(true);
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
  const deleteSelectedSkills = useCallback((skills: NormalizedSkill[]) => {
    const { deletableNames } = skillSelectionTargets(skills);
    if (deletableNames.length > 0) deleteSkillsAndClear(deletableNames);
  }, [deleteSkillsAndClear]);
  const clearLocationDialogCloseTimer = useCallback(() => {
    if (locationDialogCloseTimer.current === null) return;
    window.clearTimeout(locationDialogCloseTimer.current);
    locationDialogCloseTimer.current = null;
  }, []);
  const clearLocationDialogState = useCallback(() => {
    setLocationSkillIds([]);
    setLocationAgent(undefined);
  }, []);
  const scheduleLocationDialogCleanup = useCallback(() => {
    clearLocationDialogCloseTimer();
    locationDialogCloseTimer.current = window.setTimeout(() => {
      locationDialogCloseTimer.current = null;
      clearLocationDialogState();
    }, LOCATION_DIALOG_CLOSE_ANIMATION_MS);
  }, [clearLocationDialogCloseTimer, clearLocationDialogState]);
  useEffect(() => () => clearLocationDialogCloseTimer(), [clearLocationDialogCloseTimer]);
  useEffect(() => {
    if (!locationDialogOpen || locationSkillIds.length === 0 || locationSkills.length === locationSkillIds.length) return;
    setLocationDialogOpen(false);
    scheduleLocationDialogCleanup();
    clearSelection();
  }, [clearSelection, locationDialogOpen, locationSkillIds.length, locationSkills.length, scheduleLocationDialogCleanup]);
  const openManageLocations = useCallback((skill: NormalizedSkill, agent?: string) => {
    clearLocationDialogCloseTimer();
    setLocationSkillIds([skill.id]);
    setLocationAgent(agent);
    setLocationDialogOpen(true);
  }, [clearLocationDialogCloseTimer]);
  const openManageLocationsBatch = useCallback((skills: NormalizedSkill[]) => {
    const movableSkills = selectMovableSkills(skills);
    if (movableSkills.length === 0) return;
    clearLocationDialogCloseTimer();
    setLocationSkillIds(movableSkills.map((skill) => skill.id));
    setLocationAgent(undefined);
    setLocationDialogOpen(true);
  }, [clearLocationDialogCloseTimer]);
  const applyLocationsAndClear = useCallback(async (
    skills?: RawSkillRecord[],
    options?: { patch?: boolean; deleted?: string[] },
  ) => {
    setLocationDialogOpen(false);
    scheduleLocationDialogCleanup();
    clearSelection();
    if (skills || options?.deleted?.length) onSkillsUpdated(skills ?? [], { patch: true, deleted: options?.deleted });
    else await onRefresh();
  }, [clearSelection, onRefresh, onSkillsUpdated, scheduleLocationDialogCleanup]);
  const handleInstalled = useCallback((result: SkillInstallResult) => {
    onAddInstalled(result);
    const name = installedSkillName(result);
    if (!name) return;
    setQuery("");
    setViewMode(SkillsViewMode.List);
    setSkillLocatorRequest(name);
  }, [onAddInstalled, setQuery]);
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
      render: (skill) => <SkillMainCell skill={skill} openSkill={openSkill} onManageWrapperScope={openWrapperScope} onApplyUpdates={applyUpdatesAndClear} />,
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
    ...(projects.length > 0 ? [scopeColumnFromValue<SkillTableRow>((skill) => primarySkillScope(skill))] : []),
    {
      key: "visibility",
      header: "Visibility",
      type: ColumnDataType.Enum,
      groupOrder: [...allSkillVisibilities],
      sortValue: (skill) => skill.visibility.toLowerCase(),
      width: "150px",
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
  ], [applyUpdatesAndClear, deleteSkillsAndClear, openManageLocations, openSkill, openWrapper, openWrapperScope, projects, setVisibilityAndClear]);

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
          onBeginMutation={onBeginMutation}
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
            const skill = skillItems.find((item) => item.id === selector);
            if (skill) openSkill(skill);
          }}
        />
      ) : (
        <div className="skillsListBody">
          <DataTable
            rows={tableRows}
            columns={columns}
            getRowId={(skill) => skill.id}
            getRowLabel={skillDisplayName}
            freezeColumn={SKILL_FREEZE_COLUMN}
            selectable={(skill) => isSkillRowSelectable(skill)}
            selectedIds={selected}
            onSelectionChange={setSelected}
            onDeleteSelected={deleteSelectedSkills}
            enableMarquee
            scrollRestorationKey="skills.list"
            scrollToRowId={skillLocatorRequest}
            onScrollToRowComplete={completeSkillLocator}
            groupBy={groupBy}
            onGroupByChange={setGroupBy}
            sort={sort}
            onSortChange={setSort}
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
      <SkillWrapperScopeDialog
        open={wrapperScopeDialogOpen}
        wrapper={wrapperScopeSkill}
        skills={skillItems}
        onOpenChange={(open) => {
          setWrapperScopeDialogOpen(open);
          if (!open) setWrapperScopeSkillId("");
        }}
        onApplyWrapper={applyWrapperAndClear}
      />
      <SkillLocationDialog
        open={locationDialogOpen}
        skills={locationSkills}
        initialAgent={locationAgent}
        installedAgentKeys={installedAgentKeys}
        targetOptions={targetOptions}
        onOpenChange={(open) => {
          if (open) {
            clearLocationDialogCloseTimer();
            setLocationDialogOpen(true);
            return;
          }
          setLocationDialogOpen(false);
          scheduleLocationDialogCleanup();
        }}
        onApplied={applyLocationsAndClear}
        onBeginMutation={onBeginMutation}
      />
      </section>
    </>
  );
}
