import { Fragment, lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ComponentType, ReactNode } from "react";
import { ContextMenu, Dialog, DropdownMenu } from "radix-ui";
import {
  ChevronLeft,
  ChevronRight,
  CloudUpload,
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
import { DialogActionButton } from "../components/shared/DialogActionButton.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogAdvanceButton } from "../components/shared/DialogAdvanceButton.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { MoreActionsButton } from "../components/shared/MoreActionsButton.tsx";
import { MenuContent } from "../components/shared/MenuContent.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { SelectionCheckbox } from "../components/shared/SelectionCheckbox.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { SegmentedControl, SegmentedControlItem } from "../components/shared/SegmentedControl.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { SkillRelationshipMap } from "../features/skills/SkillRelationshipMap.tsx";
import { SkillLocationDialog } from "../features/skills/SkillLocationDialog.tsx";
import { backupSkillsForSelection, backupStatusForSkill, type BackupStatusRecord } from "../features/skills/backup-action.ts";
import { SKILL_BADGE_TONES } from "../features/skills/skill-badge-tones.ts";
import { Visibility } from "../features/skills/Visibility.tsx";
import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import { SKILL_FREEZE_COLUMN, TauriCommand, SkillVisibility, agentIdentityKey, allSkillVisibilities, compactDateTime, copyText, editableSkillVisibilities, invokeCommand, isReadOnlySkillSource, isSkillRowSelectable, isSkillSelectable, isSkillVisibilityEditable, primarySkillPath, safeInvoke, scopeColumn, skillSourceAction, skillSourceDetails, skillTargets, sourceRemoteDetails, suppressNextClick, type NormalizedSkill, type ProjectSummary } from "../lib/index.ts";

export type SkillsTableSort = SortState;

type SkillsViewMode = "list" | "network";

export type SkillRecord = NormalizedSkill;
export type RawSkillRecord = Record<string, unknown>;

type SkillWrapperSelection = {
  id: string;
  name: string;
  description?: string;
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
  return remote?.host.includes("github.") && remote.path ? remote.path : skill.section;
}
enum SkillOperationStatus {
  Planned = "planned",
  Ready = "ready",
  AlreadyExists = "already-exists",
  Replace = "replace",
  AlreadyInstalled = "already-installed",
}

type SkillOperation = {
  name: string;
  status: SkillOperationStatus;
  message?: string;
};

type AvailableSkill = {
  name: string;
  description?: string;
  relative_path: string;
  dependencies: string[];
};

type MarketplaceSource = {
  id: string;
  name: string;
  description?: string;
  source: string;
  url?: string;
  version?: string;
  metric?: number;
  metricLabel?: string;
  trustLabel?: string;
  kind: string;
};

type MarketplaceSearchResponse = {
  items: MarketplaceSource[];
  warnings: string[];
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

type SkillAddPlan = {
  available: AvailableSkill[];
  selected: AvailableSkill[];
  source: string;
  source_kind: string;
  target: string;
  operations: SkillOperation[];
};

export type SkillInstallResult = {
  skills: RawSkillRecord[];
  report: {
    plan: SkillAddPlan;
    results: Array<{ target: string }>;
  };
};

function installedSkillName(result: SkillInstallResult): string | null {
  const installedNames = new Set(result.skills.flatMap((skill) => typeof skill.name === "string" ? [skill.name] : []));
  return result.report.plan.selected
    .map((skill) => skill.name)
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
  onManageLocations: (skill: SkillRecord) => void;
  onAddToBackup: (skills: SkillRecord[]) => void;
  getBackupStatus: (skill: SkillRecord) => BackupStatusRecord;
  backupConfigured: boolean;
  backupBusy: boolean;
};

export function SkillActionsMenuItems({ Menu, skill, onApplyUpdates, onDeleteSkills, onManageLocations, onAddToBackup, getBackupStatus, backupConfigured, backupBusy }: SkillActionsMenuItemsProps) {
  const targets = skillTargets(skill);
  const primaryPath = primarySkillPath(skill);
  const hasMultipleTargets = targets.length > 1;
  const readOnly = !isSkillSelectable(skill);
  const locationReadOnly = isReadOnlySkillSource(skill);
  const backupStatus = getBackupStatus(skill);
  const canAddToBackup = backupConfigured && backupStatus.state === "unmanaged" && Boolean(backupStatus.skillPath) && !backupBusy;
  return (
    <>
      <Menu.Item
        className="skillMenuItem"
        disabled={locationReadOnly || targets.length === 0}
        onSelect={() => {
          suppressNextClick();
          onManageLocations(skill);
        }}
      >
        <ArrowRightLeft size={14} />
        Manage locations
      </Menu.Item>
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
      {backupConfigured ? (
        <Menu.Item
          className="skillMenuItem"
          disabled={!canAddToBackup}
          onSelect={() => {
            suppressNextClick();
            onAddToBackup([skill]);
          }}
        >
          <CloudUpload size={14} />
          Add to backup
        </Menu.Item>
      ) : null}
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
        {skill.updateStatus === "update-available" && (
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
  skill: SkillRecord;
  onApplyUpdates: (names: string[]) => void;
  onDeleteSkills: (names: string[]) => void;
  onManageLocations: (skill: SkillRecord) => void;
  onAddToBackup: (skills: SkillRecord[]) => void;
  getBackupStatus: (skill: SkillRecord) => BackupStatusRecord;
  backupConfigured: boolean;
  backupBusy: boolean;
};

export function SkillActionsCell({ skill, onApplyUpdates, onDeleteSkills, onManageLocations, onAddToBackup, getBackupStatus, backupConfigured, backupBusy }: SkillActionsCellProps) {
  return (
    <RowActionsMenu
      ariaLabel={`Skill actions for ${skill.name}`}
      onOpenChange={(open) => { if (!open) suppressNextClick(); }}
    >
      <SkillActionsMenuItems Menu={DropdownMenu} skill={skill} onApplyUpdates={onApplyUpdates} onDeleteSkills={onDeleteSkills} onManageLocations={onManageLocations} onAddToBackup={onAddToBackup} getBackupStatus={getBackupStatus} backupConfigured={backupConfigured} backupBusy={backupBusy} />
    </RowActionsMenu>
  );
}

type SkillSelectionAction = {
  id: string;
  direct: ReactNode;
  menu: ReactNode;
  measure: ReactNode;
};

function SkillSelectionActions({
  selectedSkills,
  applyUpdates,
  deleteSkills,
  manageLocations,
  createWrapper,
  setVisibility,
  addToBackup,
  getBackupStatus,
  backupConfigured,
  backupBusy,
}: {
  selectedSkills: SkillRecord[];
  applyUpdates: (names: string[]) => void;
  deleteSkills: (names: string[]) => void;
  manageLocations: () => void;
  createWrapper: () => void;
  setVisibility: (names: string[], visibility: SkillVisibility) => void;
  addToBackup: (skills: SkillRecord[]) => void;
  getBackupStatus: (skill: SkillRecord) => BackupStatusRecord;
  backupConfigured: boolean;
  backupBusy: boolean;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const [visibleCount, setVisibleCount] = useState(selectedSkills.length > 0 ? 1 : 0);
  const updateNames = selectedSkills.filter((skill) => skill.updateStatus === "update-available").map((skill) => skill.name);
  const deletableNames = selectedSkills.filter(isSkillSelectable).map((skill) => skill.name);
  const backupSkills = backupSkillsForSelection(selectedSkills, getBackupStatus, backupConfigured);
  const canManageLocations = selectedSkills.some((skill) => !isReadOnlySkillSource(skill) && skillTargets(skill).length > 0);
  const backupLabel = `Backup${backupSkills.length > 1 ? ` (${backupSkills.length})` : ""}`;
  const actions = useMemo<SkillSelectionAction[]>(() => {
    const visibilityMenu = (
      <DropdownMenu.Sub>
        <DropdownMenu.SubTrigger className="skillMenuItem skillMenuSubTrigger">
          <Eye size={14} />
          <span>Visibility</span>
          <ChevronRight className="skillMenuSubIcon" size={14} />
        </DropdownMenu.SubTrigger>
        <DropdownMenu.Portal>
          <DropdownMenu.SubContent className="skillMenuContent" sideOffset={8} alignOffset={-6}>
            <VisibilityMenuItems Menu={DropdownMenu} selectedSkills={selectedSkills} onSetVisibility={setVisibility} />
          </DropdownMenu.SubContent>
        </DropdownMenu.Portal>
      </DropdownMenu.Sub>
    );
    const backupAction: SkillSelectionAction | null = backupConfigured ? {
      id: "backup",
      direct: (
        <button aria-label={backupLabel} aria-busy={backupBusy} disabled={backupSkills.length === 0 || backupBusy} onClick={() => addToBackup(backupSkills)}>
          {backupBusy ? <LoadingInline size={15} gap={6} label={backupLabel} /> : <><CloudUpload size={15} /><span>{backupLabel}</span></>}
        </button>
      ),
      menu: (
        <DropdownMenu.Item className="skillMenuItem" disabled={backupSkills.length === 0 || backupBusy} onSelect={() => addToBackup(backupSkills)}>
          <CloudUpload size={14} />
          {backupLabel}
        </DropdownMenu.Item>
      ),
      measure: <><CloudUpload size={15} /><span>{backupLabel}</span></>,
    } : null;
    const updateMenu = (
      <DropdownMenu.Item className="skillMenuItem" disabled={updateNames.length === 0} onSelect={() => applyUpdates(updateNames)}>
        <RefreshCw size={14} />
        Update{updateNames.length ? ` (${updateNames.length})` : ""}
      </DropdownMenu.Item>
    );
    const manageMenu = (
      <DropdownMenu.Item className="skillMenuItem" disabled={!canManageLocations} onSelect={manageLocations}>
        <ArrowRightLeft size={14} />
        Locations
      </DropdownMenu.Item>
    );
    const wrapperMenu = (
      <DropdownMenu.Item className="skillMenuItem" onSelect={createWrapper}>
        <PackagePlus size={14} />
        Wrapper
      </DropdownMenu.Item>
    );
    const deleteMenu = (
      <DropdownMenu.Item className="skillMenuItem danger" disabled={deletableNames.length === 0} onSelect={() => deleteSkills(deletableNames)}>
        <Trash2 size={14} />
        Delete
      </DropdownMenu.Item>
    );
    return [
      {
        id: "visibility",
        direct: (
          <DropdownMenu.Root>
            <DropdownMenu.Trigger asChild>
              <button aria-label="Visibility"><Eye size={15} />Visibility</button>
            </DropdownMenu.Trigger>
            <DropdownMenu.Portal>
              <MenuContent align="start" sideOffset={6}>
                <VisibilityMenuItems Menu={DropdownMenu} selectedSkills={selectedSkills} onSetVisibility={setVisibility} />
              </MenuContent>
            </DropdownMenu.Portal>
          </DropdownMenu.Root>
        ),
        menu: visibilityMenu,
        measure: <><Eye size={15} /><span>Visibility</span></>,
      },
      ...(backupAction ? [backupAction] : []),
      {
        id: "update",
        direct: (
          <button className="skillApplyUpdatesButton" aria-label={`Update${updateNames.length ? ` (${updateNames.length})` : ""}`} disabled={updateNames.length === 0} onClick={() => applyUpdates(updateNames)}>
            <RefreshCw size={15} aria-hidden="true" />
            <span>Update{updateNames.length ? ` (${updateNames.length})` : ""}</span>
          </button>
        ),
        menu: updateMenu,
        measure: <><RefreshCw size={15} /><span>Update{updateNames.length ? ` (${updateNames.length})` : ""}</span></>,
      },
      {
        id: "locations",
        direct: (
          <button aria-label="Manage selected skill locations" disabled={!canManageLocations} onClick={manageLocations}>
            <ArrowRightLeft size={15} />
            <span>Locations</span>
          </button>
        ),
        menu: manageMenu,
        measure: <><ArrowRightLeft size={15} /><span>Locations</span></>,
      },
      {
        id: "wrapper",
        direct: <button aria-label="Wrapper" onClick={createWrapper}><PackagePlus size={15} /><span>Wrapper</span></button>,
        menu: wrapperMenu,
        measure: <><PackagePlus size={15} /><span>Wrapper</span></>,
      },
      {
        id: "delete",
        direct: <button className="danger" aria-label="Delete selected skills" disabled={deletableNames.length === 0} onClick={() => deleteSkills(deletableNames)}><Trash2 size={15} /><span>Delete</span></button>,
        menu: deleteMenu,
        measure: <><Trash2 size={15} /><span>Delete</span></>,
      },
    ];
  }, [addToBackup, backupBusy, backupConfigured, backupLabel, backupSkills, canManageLocations, createWrapper, deleteSkills, deletableNames, manageLocations, selectedSkills, setVisibility, updateNames, applyUpdates]);

  useLayoutEffect(() => {
    const container = containerRef.current;
    const measure = measureRef.current;
    if (!container || !measure) return undefined;
    const updateVisibleCount = () => {
      const availableWidth = container.clientWidth;
      const gap = Number.parseFloat(getComputedStyle(measure).gap) || 4;
      const widths = actions.map((action) => measure.querySelector<HTMLElement>(`[data-action-measure="${action.id}"]`)?.getBoundingClientRect().width ?? 0);
      const overflowWidth = measure.querySelector<HTMLElement>("[data-overflow-measure]")?.getBoundingClientRect().width ?? 0;
      let nextVisibleCount = actions.length;
      while (nextVisibleCount > 0) {
        const hiddenCount = actions.length - nextVisibleCount;
        const actionsWidth = widths.slice(0, nextVisibleCount).reduce((sum, width) => sum + width, 0) + Math.max(0, nextVisibleCount - 1) * gap;
        const totalWidth = actionsWidth + (hiddenCount > 0 ? gap + overflowWidth : 0);
        if (totalWidth <= availableWidth + 1) break;
        nextVisibleCount -= 1;
      }
      setVisibleCount((current) => current === nextVisibleCount ? current : nextVisibleCount);
    };
    if (typeof ResizeObserver === "undefined") {
      updateVisibleCount();
      return undefined;
    }
    const observer = new ResizeObserver(updateVisibleCount);
    observer.observe(container);
    observer.observe(measure);
    updateVisibleCount();
    return () => observer.disconnect();
  }, [actions]);

  const visibleActions = actions.slice(0, visibleCount);
  const overflowActions = actions.slice(visibleCount);
  return (
    <div ref={containerRef} className="skillsSelectionActionsContent">
      <div className="skillsSelectionActionsVisible">
        {visibleActions.map((action) => <span className="skillsSelectionAction" key={action.id}>{action.direct}</span>)}
        {overflowActions.length > 0 ? <RowActionsMenu ariaLabel="More selected skill actions">{overflowActions.map((action) => <Fragment key={action.id}>{action.menu}</Fragment>)}</RowActionsMenu> : null}
      </div>
      <div ref={measureRef} className="skillsSelectionActionsMeasure" aria-hidden="true">
        {actions.map((action) => <button data-action-measure={action.id} key={action.id}>{action.measure}</button>)}
        <MoreActionsButton data-overflow-measure aria-label="More selected skill actions" />
      </div>
    </div>
  );
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

function normalizeSkillOperationStatus(status: unknown): SkillOperationStatus | undefined {
  if (!status) return undefined;
  if (status === SkillOperationStatus.Planned) return SkillOperationStatus.Planned;
  if (status === SkillOperationStatus.Ready) return SkillOperationStatus.Ready;
  if (status === SkillOperationStatus.AlreadyExists) return SkillOperationStatus.AlreadyExists;
  if (status === SkillOperationStatus.Replace) return SkillOperationStatus.Replace;
  if (status === SkillOperationStatus.AlreadyInstalled) return SkillOperationStatus.AlreadyInstalled;
  return undefined;
}

function normalizeSkillAddPlan(plan: SkillAddPlan | null | undefined): SkillAddPlan | null {
  if (!plan) return null;
  if (
    !Array.isArray(plan.available)
    || !Array.isArray(plan.selected)
    || !Array.isArray(plan.operations)
    || typeof plan.source !== "string"
    || !plan.source.trim()
    || typeof plan.source_kind !== "string"
    || !plan.source_kind.trim()
    || typeof plan.target !== "string"
    || !plan.target.trim()
  ) return null;
  const normalizeAvailable = (skills: AvailableSkill[]) => skills.flatMap((skill) => (
    typeof skill.name === "string"
      && skill.name.trim()
      && typeof skill.relative_path === "string"
      && skill.relative_path.trim()
      && Array.isArray(skill.dependencies)
      ? [{
        ...skill,
        name: skill.name.trim(),
        relative_path: skill.relative_path.trim(),
        dependencies: skill.dependencies.filter((dependency): dependency is string => typeof dependency === "string"),
      }]
      : []
  ));
  const available = normalizeAvailable(plan.available);
  const selected = normalizeAvailable(plan.selected);
  const operations = plan.operations.flatMap((operation) => {
    const status = normalizeSkillOperationStatus(operation.status);
    return typeof operation.name === "string" && operation.name.trim() && status
      ? [{ ...operation, name: operation.name.trim(), status }]
      : [];
  });
  return {
    ...plan,
    source: plan.source.trim(),
    source_kind: plan.source_kind.trim(),
    target: plan.target.trim(),
    available,
    selected,
    operations,
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
  return status === SkillOperationStatus.AlreadyInstalled
    || status === SkillOperationStatus.AlreadyExists
    || status === SkillOperationStatus.Replace;
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
    ...skill.dependencies,
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
  onRequestWrapper: (skills: SkillWrapperSelection[]) => void;
  installedAgentKeys: string[];
  targetOptions: SkillTargetOption[];
};

export type SkillTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
  globalPath?: string;
};

export function AddSkillDialog({ open, onOpenChange, trigger, onClose, onInstalled, onRequestWrapper, installedAgentKeys, targetOptions }: AddSkillDialogProps) {
  const [source, setSource] = useState("");
  const [marketplaceQuery, setMarketplaceQuery] = useState("");
  const [marketplaceResults, setMarketplaceResults] = useState<MarketplaceSource[]>([]);
  const [marketplaceBusy, setMarketplaceBusy] = useState(false);
  const [marketplaceError, setMarketplaceError] = useState("");
  const [marketplaceNotice, setMarketplaceNotice] = useState("");
  const [skillPreview, setSkillPreview] = useState<SkillMarkdownPreview | null>(null);
  const [skillPreviewBusy, setSkillPreviewBusy] = useState("");
  const [skillPreviewError, setSkillPreviewError] = useState("");
  const [target, setTarget] = useState("");
  const [installTargets, setInstallTargets] = useState<SkillTargetOption[]>([]);
  const [copy, setCopy] = useState(false);
  const [visibility, setVisibility] = useState<SkillVisibility>(SkillVisibility.Auto);
  const [plan, setPlan] = useState<SkillAddPlan | null>(null);
  const [previewId, setPreviewId] = useState("");
  const [selectedRoots, setSelectedRoots] = useState<string[]>([]);
  const [createWrapper, setCreateWrapper] = useState(false);
  const [replaceExisting, setReplaceExisting] = useState(false);
  const [skillFilter, setSkillFilter] = useState<"all" | "new" | "existing">("all");
  const [reviewingSkills, setReviewingSkills] = useState(false);
  const [advancedOpen, setAdvancedOpen] = useState(false);
  const [skillSearch, setSkillSearch] = useState("");
  const [busyAction, setBusyAction] = useState("");
  const [error, setError] = useState("");
  const skillItemRefs = useRef(new Map<string, HTMLDivElement>());
  const busy = busyAction !== "";
  const installing = busyAction === "install";
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
      setTarget(visibleInstallTargets[0]?.id ?? "");
    }
  }, [target, visibleInstallTargets]);
  const available = plan?.available ?? [];
  const showSkillSearch = available.length > 10;
  const normalizedSkillSearch = skillSearch.trim().toLowerCase();
  const dependencyByName = useMemo(
    () => new Map(available.map((skill) => [skill.name, skill.dependencies])),
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
  const selectedHasExisting = useMemo(
    () => selected.some((name) => isExistingSkillOperationStatus(rawOperationByName.get(name)?.status)),
    [rawOperationByName, selected],
  );
  useEffect(() => {
    setReplaceExisting(selectedHasExisting);
  }, [selectedHasExisting]);
  const operationByName = useMemo(
    () => new Map((plan?.operations ?? []).map((operation) => [
      operation.name,
      selectedExistingSkillOperation(operation, selectedSet.has(operation.name) && replaceExisting),
    ])),
    [plan, replaceExisting, selectedSet],
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
  const allSelected = selectableSkills.length > 0 && selectableSkills.every((skill) => selectedSet.has(skill.name));
  const mixedSelected = selected.length > 0 && !allSelected;
  const visibleAvailableSkills = useMemo(
    () => skillFilter === "all"
      ? available
      : available.filter((skill) => skillFilter === "new"
        ? isNewSkillOperationStatus(rawOperationByName.get(skill.name)?.status)
        : isExistingSkillOperationStatus(rawOperationByName.get(skill.name)?.status)),
    [available, rawOperationByName, skillFilter],
  );
  const searchMatches = useMemo(() => {
    if (!normalizedSkillSearch) return [];
    return visibleAvailableSkills
      .filter((skill) => availableSkillSearchText(skill).includes(normalizedSkillSearch))
      .map((skill) => skill.name);
  }, [normalizedSkillSearch, visibleAvailableSkills]);
  const searchMatchSet = useMemo(() => new Set(searchMatches), [searchMatches]);
  const firstSearchMatch = searchMatches[0] ?? "";
  const canInstall = Boolean(target && source.trim() && plan && selected.length > 0 && (!selectedHasExisting || replaceExisting) && !busy);
  const advanceLabel = busy ? "Preparing installation" : "Install selected skills";
  const advanceText = busy ? "Installing" : `Install ${selected.length}`;
  const sourceCandidates = source.trim() ? marketplaceResults : recommendedSkillSources;
  const sourceCandidatesLabel = source.trim() ? "Matches" : "Recommended";

  const handleSourceChange = (value: string) => {
    if (marketplaceBusy) return;
    setSource(value);
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
      const response = await invokeCommand<MarketplaceSearchResponse>(TauriCommand.SkillsMarketplaceSearch, {
        query,
      });
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
    if (!normalizedSource || !target || busyAction || marketplaceBusy) return;
    setSource(normalizedSource);
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreview(null);
    setSkillPreviewError("");
    setBusyAction("preview");
    setError("");
    try {
      const response = await invokeCommand(TauriCommand.SkillsAdd, {
        source: normalizedSource,
        target,
        scope: "global",
        skills: [],
        copy,
        overwrite: false,
        visibility: visibility.toLowerCase(),
        dryRun: true,
      }) as { plan?: SkillAddPlan; previewId?: string } | null;
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
      setSkillFilter("all");
      setSkillSearch("");
      setSelectedRoots(nextPlan.selected.map((skill) => skill.name));
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

  const clearTransientErrors = useCallback(() => {
    setError("");
    setMarketplaceError("");
    setMarketplaceNotice("");
    setSkillPreviewError("");
  }, []);

  const closeDialog = () => {
    setReviewingSkills(false);
    setAdvancedOpen(false);
    clearTransientErrors();
    onClose();
  };

  const goBack = () => {
    if (busy) return;
    if (reviewingSkills) {
      setReviewingSkills(false);
      setSkillPreview(null);
      return;
    }
    setPlan(null);
    setPreviewId("");
    setSkillPreview(null);
    setSkillPreviewError("");
    setSource(marketplaceQuery || source);
    setError("");
    setSkillFilter("all");
    setSkillSearch("");
    setReplaceExisting(false);
    setCreateWrapper(false);
    setAdvancedOpen(false);
  };

  const install = async () => {
    if (!canInstall || busy) return;
    setError("");
    try {
      setBusyAction("preview");
      const previewResponse = await invokeCommand<{ plan?: SkillAddPlan; previewId?: string }>(TauriCommand.SkillsAdd, {
        source: source.trim(),
        target,
        scope: "global",
        skills: selected,
        copy,
        overwrite: replaceExisting,
        visibility: visibility.toLowerCase(),
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
      setBusyAction("install");
      const result = await invokeCommand<SkillInstallResult>(TauriCommand.SkillsAdd, {
        source: source.trim(),
        target,
        scope: "global",
        skills: selected,
        copy,
        overwrite: replaceExisting,
        visibility: visibility.toLowerCase(),
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
      setBusyAction("");
    }
  };

  const advance = () => {
    if (busy) return;
    if (plan) install();
  };

  useEffect(() => {
    if (targetOptions.length === 0) return;
    setInstallTargets(targetOptions);
  }, [targetOptions]);

  useEffect(() => {
    if (!firstSearchMatch) return;
    skillItemRefs.current.get(firstSearchMatch)?.scrollIntoView({
      block: "nearest",
      behavior: "smooth",
    });
  }, [firstSearchMatch]);

  useEffect(() => {
    if (!open) {
      setReviewingSkills(false);
      setAdvancedOpen(false);
      clearTransientErrors();
    }
  }, [clearTransientErrors, open]);

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

  return (
    <DialogShell
      open={open}
      onOpenChange={(nextOpen) => {
        if (!nextOpen) {
          setReviewingSkills(false);
          setAdvancedOpen(false);
          clearTransientErrors();
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
          Add Skills
        </Dialog.Title>
      </div>
      <Dialog.Description id="add-skill-dialog-description" className="dialogVisuallyHidden">
        {!plan
          ? "Choose a skill source and review it before installation."
          : reviewingSkills
            ? "Select the skills to install."
            : "Review the selected skills and install them with the default settings."}
      </Dialog.Description>
      {!reviewingSkills && <div className={`addSkillGrid ${plan ? "hasPlan" : "sourceStage"}`}>
        {!plan && <div className="skillSourceField">
          {busyAction !== "preview" && (
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
          {!plan && (marketplaceBusy || busyAction === "preview") && (
            <LoadingState
              variant="progress"
              label={marketplaceBusy ? "Searching marketplaces" : "Scanning repository"}
            />
          )}
          {source.trim() && !marketplaceBusy && busyAction !== "preview" && sourceCandidates.length === 0 && !marketplaceNotice && !marketplaceError && (
            <EmptyState
              className="sourceSearchEmpty"
              compact
              title="Search for a skill"
              description="Click Search to find matching skills."
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
                value={target}
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
                <span>Select all</span>
              </div>
              {existingSkills.length > 0 && (
                <SegmentedControl
                  className="addSkillFilterTabs"
                  value={skillFilter}
                  onValueChange={(value) => {
                    if (value === "all" || value === "new" || value === "existing") setSkillFilter(value);
                  }}
                  aria-label="Filter skills"
                >
                  {([
                    ["all", "All", available.length],
                    ["new", "New", newSkills.length],
                    ["existing", "Existing", existingSkills.length],
                  ] as const).map(([value, label, count]) => (
                    <SegmentedControlItem
                      value={value}
                      key={value}
                    >
                      {label} <span className="addSkillFilterCount">{count}</span>
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
          <div className="addSkillList">
            {visibleAvailableSkills.map((skill) => {
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
                    ? <LoadingInline size={14} gap={3} label="Preview" />
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
                value={copy ? "copy" : "symlink"}
                disabled={installing}
                onValueChange={(value) => {
                  if (!value || busy) return;
                  const nextCopy = value === "copy";
                  setCopy(nextCopy);
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
        leading={plan ? (
          <DialogActionButton variant="secondary" disabled={busy} onClick={goBack}>Back</DialogActionButton>
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
        ) : !busy && !marketplaceBusy ? (
          <DialogActionButton
            variant="primary"
            disabled={!source.trim()}
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
  openSkill: (skill: SkillRecord) => void;
  skills: SkillRecord[];
  loadingSkills: boolean;
  loadError: string;
  hasRows: boolean;
  checkingUpdates: boolean;
  updateError: string;
  onRefresh: () => void | Promise<void>;
  onSkillsUpdated: (skills: RawSkillRecord[]) => void;
  onSetVisibility: (names: string[], visibility: SkillVisibility) => void | Promise<void>;
  onApplyWrapper: (args: WrapperArgs) => Promise<unknown>;
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
  const [viewMode, setViewMode] = useState<SkillsViewMode>("list");
  const [showWrapper, setShowWrapper] = useState(false);
  const [showAddSkill, setShowAddSkill] = useState(false);
  const [installedWrapperSkills, setInstalledWrapperSkills] = useState<SkillWrapperSelection[]>([]);
  const [skillLocatorRequest, setSkillLocatorRequest] = useState("");
  const [locationSkills, setLocationSkills] = useState<SkillRecord[]>([]);
  const [locationAgent, setLocationAgent] = useState<string | undefined>(undefined);
  const [backupStatuses, setBackupStatuses] = useState<Map<string, BackupStatusRecord>>(new Map());
  const [backupConfigured, setBackupConfigured] = useState(false);
  const [backupBusy, setBackupBusy] = useState(false);
  const backupStatusInFlight = useRef<Promise<void> | null>(null);
  const refreshBackupStatuses = useCallback((): Promise<void> => {
    if (backupStatusInFlight.current) return backupStatusInFlight.current;
    const request = (async () => {
      try {
        const result = await invokeCommand<{ config: unknown | null; statuses: BackupStatusRecord[] }>(TauriCommand.SkillsBackupStatus);
        const next = new Map<string, BackupStatusRecord>();
        for (const status of result.statuses) next.set(status.skillPath, status);
        setBackupConfigured(Boolean(result.config));
        setBackupStatuses(next);
      } catch {
        // Backup availability is auxiliary to skill management; retain the last known state.
      }
    })();
    backupStatusInFlight.current = request;
    void request.then(
      () => {
        if (backupStatusInFlight.current === request) backupStatusInFlight.current = null;
      },
      () => {
        if (backupStatusInFlight.current === request) backupStatusInFlight.current = null;
      },
    );
    return request;
  }, []);
  useEffect(() => {
    if (skillItems.length > 0) void refreshBackupStatuses();
  }, [refreshBackupStatuses, skillItems]);
  const normalizedQuery = query.trim().toLowerCase();
  const visibleSkills = useMemo(() => {
    if (!normalizedQuery) return skillItems;
    return skillItems.filter((skill) => [skill.name, skill.description].some((value) => value.toLowerCase().includes(normalizedQuery)));
  }, [normalizedQuery, skillItems]);
  const tableRows = visibleSkills;
  const networkNodes = useMemo(
    () => visibleSkills.map((skill) => ({
      name: skill.name,
      label: skill.name,
      description: skill.description,
      dependencies: skill.dependencies,
      dependents: skill.dependents,
      kind: skill.isWrapper ? "wrapper" : skill.section.toLowerCase(),
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
  const openManageLocations = useCallback((skill: SkillRecord, agent?: string) => {
    setLocationSkills([skill]);
    setLocationAgent(agent);
  }, []);
  const openManageLocationsBatch = useCallback((skills: SkillRecord[]) => {
    const movableSkills = skills.filter((skill) => !isReadOnlySkillSource(skill) && skillTargets(skill).length > 0);
    if (movableSkills.length === 0) return;
    setLocationSkills(movableSkills);
    setLocationAgent(undefined);
  }, []);
  const applyLocationsAndClear = useCallback(async (skills?: RawSkillRecord[]) => {
    setLocationSkills([]);
    setLocationAgent(undefined);
    clearSelection();
    if (skills) onSkillsUpdated(skills);
    else await onRefresh();
  }, [clearSelection, onRefresh, onSkillsUpdated]);
  const getBackupStatus = useCallback((skill: SkillRecord) => backupStatusForSkill(skill, backupStatuses), [backupStatuses]);
  const adoptSkillsForBackup = useCallback(async (skills: SkillRecord[]) => {
    if (!backupConfigured || backupBusy) return;
    const candidates = backupSkillsForSelection(skills, getBackupStatus, backupConfigured);
    if (candidates.length === 0) return;
    setBackupBusy(true);
    try {
      let latestSkills: RawSkillRecord[] | undefined;
      for (const skill of candidates) {
        const status = getBackupStatus(skill);
        const result = await invokeCommand<{ skills: RawSkillRecord[] }>(TauriCommand.SkillsBackupAdopt, { name: skill.name, skillPath: status.skillPath });
        latestSkills = result.skills;
      }
      if (latestSkills) onSkillsUpdated(latestSkills);
      await refreshBackupStatuses();
    } finally {
      setBackupBusy(false);
    }
  }, [backupBusy, backupConfigured, getBackupStatus, onSkillsUpdated, refreshBackupStatuses]);

  const handleInstalled = useCallback((result: SkillInstallResult) => {
    onAddInstalled(result);
    const name = installedSkillName(result);
    if (!name) return;
    setQuery("");
    setViewMode("list");
    setSkillLocatorRequest(name);
  }, [onAddInstalled]);
  const completeSkillLocator = useCallback((rowId: string) => {
    setSkillLocatorRequest((current) => current === rowId ? "" : current);
  }, []);

  const columns = useMemo((): ColumnDef<SkillTableRow>[] => [
    {
      key: "main",
      header: "Skill",
      type: "text",
      sortValue: (skill) => skill.name.toLowerCase(),
      width: "minmax(250px, 1fr)",
      render: (skill) => <SkillMainCell skill={skill} openSkill={openSkill} onApplyUpdates={applyUpdatesAndClear} />,
    },
    {
      key: "agents",
      header: "Agents",
      type: "enum",
      groupBy: (skill) => skill.agents.join(", "),
      sortValue: (skill) => skill.agents.join(",").toLowerCase(),
      width: "90px",
      render: (skill) => <AgentChips agents={skill.agents} onAgentClick={(agent) => openManageLocations(skill, agent)} />,
    },
    {
      key: "origin",
      header: "Origin",
      type: "enum",
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
      type: "enum",
      groupOrder: [...allSkillVisibilities],
      sortValue: (skill) => skill.visibility.toLowerCase(),
      width: "98px",
      render: (skill) => <Visibility value={skill.visibility} skill={skill} onSetVisibility={setVisibilityAndClear} />,
    },
    {
      key: "ctime",
      header: "Created",
      type: "date",
      sortValue: (skill) => skill.ctime ?? "",
      width: "98px",
      value: (skill) => compactDateTime(skill.ctime),
      empty: "",
    },
    {
      key: "mtime",
      header: "Updated",
      type: "date",
      sortValue: (skill) => skill.mtime ?? "",
      width: "98px",
      value: (skill) => compactDateTime(skill.mtime),
      empty: "",
    },
    {
      key: "actions",
      header: "",
      width: "40px",
      render: (skill) => <SkillActionsCell skill={skill} onApplyUpdates={applyUpdatesAndClear} onDeleteSkills={deleteSkillsAndClear} onManageLocations={openManageLocations} onAddToBackup={adoptSkillsForBackup} getBackupStatus={getBackupStatus} backupConfigured={backupConfigured} backupBusy={backupBusy} />,
    },
  ], [adoptSkillsForBackup, applyUpdatesAndClear, backupBusy, backupConfigured, deleteSkillsAndClear, getBackupStatus, openManageLocations, openSkill, projects, setVisibilityAndClear]);

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
      <SkillActionsMenuItems Menu={ContextMenu} skill={skill} onApplyUpdates={applyUpdatesAndClear} onDeleteSkills={deleteSkillsAndClear} onManageLocations={openManageLocations} onAddToBackup={adoptSkillsForBackup} getBackupStatus={getBackupStatus} backupConfigured={backupConfigured} backupBusy={backupBusy} />
    );
  }, [adoptSkillsForBackup, applyUpdatesAndClear, backupBusy, backupConfigured, deleteSkillsAndClear, getBackupStatus, openManageLocations]);

  const bottomBar = useCallback((selectedRows: SkillTableRow[]) => (
    <SkillSelectionActions
      selectedSkills={selectedRows}
      applyUpdates={applyUpdatesAndClear}
      deleteSkills={deleteSkillsAndClear}
      manageLocations={() => openManageLocationsBatch(selectedRows)}
      createWrapper={() => setShowWrapper(true)}
      setVisibility={setVisibilityAndClear}
      addToBackup={adoptSkillsForBackup}
      getBackupStatus={getBackupStatus}
      backupConfigured={backupConfigured}
      backupBusy={backupBusy}
    />
  ), [adoptSkillsForBackup, applyUpdatesAndClear, backupBusy, backupConfigured, deleteSkillsAndClear, getBackupStatus, openManageLocationsBatch, setVisibilityAndClear]);

  return (
    <section className="content skillsPage">
      <ContentTopDragStrip />
      <PageHeader title="Skills">
        <SegmentedControl
          variant="icon"
          value={viewMode}
          onValueChange={(value) => {
            if (value === "list" || value === "network") setViewMode(value);
          }}
          aria-label="Skills view"
        >
          <SegmentedControlItem value="list" aria-label="Show skills as a list">
            <List size={15} aria-hidden="true" />
          </SegmentedControlItem>
          <SegmentedControlItem value="network" aria-label="Show skill relationships">
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
          onOpenChange={setShowAddSkill}
          trigger={(
            <Dialog.Trigger asChild>
              <IconButton className="filled" aria-label="Add skill"><Plus size={16} /></IconButton>
            </Dialog.Trigger>
          )}
          onClose={() => setShowAddSkill(false)}
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
      {viewMode === "network" ? (
        <SkillRelationshipMap
          nodes={networkNodes}
          loading={loadingSkills}
          error={hasRows ? "" : loadError}
          onRetry={() => { void onRefresh(); }}
          onOpenSkill={(name) => {
            const skill = skillItems.find((item) => item.name === name);
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
            defaultSort={{ key: "mtime", direction: "desc" }}
            onRowClick={openSkill}
            rowContextMenu={rowContextMenu}
            bottomBar={bottomBar}
            bottomBarActionsClassName="skillsSelectionActions"
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
  );
}
