import { Tooltip } from "../components/shared/Tooltip.tsx";
import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState, type ComponentType, type ReactNode } from "react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { Code2, Copy, FolderOpen, Info, ScrollText, SearchX, Trash2 } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";

import { DataTable } from "../components/DataTable.tsx";
import { ColumnDataType, type ColumnDef, type SortState } from "../components/DataTable.types";
import { SortDirection } from "../lib/sort.ts";
import { DiffLineKind } from "../lib/diff.ts";
import { AgentChips } from "../components/shared/AgentChips.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { CopyPathMenuItem, DeleteMenuItem, OpenInEditorMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DataTableSelectionActions, renderDataTableSelectionMenu, type DataTableSelectionActionDefinition } from "../components/shared/DataTableSelectionActions.tsx";
import { DetailPanel } from "../components/shared/DetailPanel.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { DeleteConfirmationDialog } from "../components/shared/DeleteConfirmationDialog.tsx";
import { DiscardChangesDialog } from "../components/shared/DiscardChangesDialog.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { EditorStatePlaceholder } from "../components/shared/EditorStatePlaceholder.tsx";
import { InfoDropdownMenu } from "../components/shared/InfoDropdownMenu.tsx";
import { InfoSection } from "../components/shared/InfoSection.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import type { SkillDependencyRecord } from "../features/skills/SkillDependencyGraph.tsx";
import { ruleColumns as sharedRuleColumns } from "../lib/tableColumns.tsx";
import { actionLabels, copiedPathLabel, copyPathLabel, revealPathLabel, RULE_FREEZE_COLUMN, RuleScope, selectionDeleteErrorLabel, selectionDeleteLabel, TableSelectionActionId, TauriCommand, diffPreview, formatUserPath, friendlyAgent, ruleAgents, ruleKey, ruleSelectionActionIds, ruleSortValue, ruleTitle, safeInvoke, scopeColumn, suppressNextClick, type ProjectSummary, type RuleRecord } from "../lib/index.ts";
import { selectRuleListView } from "../controllers/rule-controller.ts";
import { readRule, saveRule, SkillScope, type CatalogMutationResponse } from "../lib/runtime-gateway.ts";

const MarkdownFilePane = lazy(() => import("../components/shared/MarkdownFilePane.tsx").then(({ MarkdownFilePane: component }) => ({ default: component })));

type RuleItem = { key: string; rule: RuleRecord };
type RuleTableRow = RuleItem & { id: string };

type RuleMenuComponents = {
  Item: ComponentType<{
    className?: string;
    disabled?: boolean;
    key?: string;
    onSelect?: () => void;
    children?: ReactNode;
  }>;
  Separator: ComponentType<{ className?: string }>;
};

function ruleSourcePath(rule: RuleRecord): string {
  return rule.path;
}

function operationError(value: CatalogMutationResponse | undefined): string | null {
  if (!value || typeof value !== "object") return null;
  const error = "error" in value ? value.error : undefined;
  return typeof error === "string" ? error : null;
}

function ruleSelectionActions(
  selectedRows: RuleTableRow[],
  Menu: RuleMenuComponents,
  requestDeleteRules: (rows: RuleTableRow[]) => void,
  deleting: boolean,
): DataTableSelectionActionDefinition[] {
  const row = selectedRows.length === 1 ? selectedRows[0] : undefined;
  if (selectedRows.length === 0 || ruleSelectionActionIds(selectedRows.length).length === 0) return [];
  const path = row ? ruleSourcePath(row.rule) : "";
  const deletable = selectedRows.filter((item) => Boolean(item.rule.path));
  const deleteLabel = selectionDeleteLabel("rule", selectedRows.length);
  const actions: Record<string, DataTableSelectionActionDefinition> = {
    [TableSelectionActionId.OpenEditor]: {
      id: TableSelectionActionId.OpenEditor,
      direct: <button aria-label={actionLabels.openInEditor} disabled={!path} onClick={() => path && safeInvoke(TauriCommand.OpenInEditor, { path })}><Code2 size={15} /><span>{actionLabels.openInEditor}</span></button>,
      menu: <OpenInEditorMenuItem Menu={Menu} path={path} />,
      measure: <><Code2 size={15} /><span>{actionLabels.openInEditor}</span></>,
    },
    [TableSelectionActionId.Reveal]: {
      id: TableSelectionActionId.Reveal,
      direct: <button aria-label={actionLabels.revealInFinder} disabled={!path} onClick={() => path && safeInvoke(TauriCommand.RevealInFinder, { path })}><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></button>,
      menu: <RevealInFinderMenuItem Menu={Menu} path={path} />,
      measure: <><FolderOpen size={15} /><span>{actionLabels.revealInFinder}</span></>,
      separatorBefore: true,
    },
    [TableSelectionActionId.CopyPath]: {
      id: TableSelectionActionId.CopyPath,
      direct: <CopyButton value={path} disabled={!path} copyLabel={actionLabels.copyPath} copiedLabel={actionLabels.pathCopied} iconSize={15}>{actionLabels.copyPath}</CopyButton>,
      menu: <CopyPathMenuItem Menu={Menu} path={path} />,
      measure: <><Copy size={15} /><span>{actionLabels.copyPath}</span></>,
    },
    [TableSelectionActionId.Delete]: {
      id: TableSelectionActionId.Delete,
      direct: <button type="button" className="danger" aria-label={deleteLabel} disabled={deletable.length === 0 || deleting} onClick={() => requestDeleteRules(deletable)}><Trash2 size={15} /><span>{deleteLabel}</span></button>,
      menu: <DeleteMenuItem Menu={Menu} label={deleteLabel} disabled={deletable.length === 0 || deleting} onSelect={() => requestDeleteRules(deletable)} />,
      measure: <><Trash2 size={15} /><span>{deleteLabel}</span></>,
      separatorBefore: true,
    },
  };
  return ruleSelectionActionIds(selectedRows.length).map((id) => actions[id]);
}

function RuleActionsCell({ rule, requestDeleteRules, deleting }: { rule: RuleRecord; requestDeleteRules: (rows: RuleTableRow[]) => void; deleting: boolean }) {
  return (
    <RowActionsMenu
      ariaLabel={`Rule actions for ${ruleTitle(rule)}`}
      onOpenChange={(open) => { if (!open) suppressNextClick(); }}
    >
      {renderDataTableSelectionMenu(ruleSelectionActions([{ key: ruleKey(rule), id: ruleKey(rule), rule }], DropdownMenu, requestDeleteRules, deleting))}
    </RowActionsMenu>
  );
}

function normalizeSkillName(value: string) {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

function preferReferencedSkill(matches: SkillDependencyRecord[], rule: RuleRecord) {
  if (matches.length <= 1) return matches[0];
  if (rule.scope === RuleScope.Project) {
    const project = matches.find((skill) =>
      (skill.paths ?? []).some((path) => path.scope === SkillScope.Project),
    );
    if (project) return project;
  }
  return matches.find((skill) => (skill.paths ?? []).every((path) => path.scope !== SkillScope.Project))
    ?? matches[0];
}

function ruleSkillRefs(content: string, skills: SkillDependencyRecord[], rule: RuleRecord | null) {
  if (!rule) return [];
  const byName = new Map<string, SkillDependencyRecord[]>();
  for (const skill of skills) {
    const key = normalizeSkillName(skill.name);
    const group = byName.get(key) ?? [];
    group.push(skill);
    byName.set(key, group);
  }
  const refs = new Map<string, SkillDependencyRecord>();
  for (const match of content.matchAll(/(?:^|[^\w])[$/]([a-zA-Z0-9][a-zA-Z0-9_-]*)/g)) {
    const candidates = byName.get(normalizeSkillName(match[1]));
    if (!candidates?.length) continue;
    const preferred = preferReferencedSkill(candidates, rule);
    refs.set(preferred.id ?? preferred.name, preferred);
  }
  return [...refs.values()].sort((left, right) => left.name.localeCompare(right.name));
}

function RuleInfoMenu({
  rule,
  referencedSkills,
  onOpenSkill,
}: {
  rule: RuleRecord;
  referencedSkills: SkillDependencyRecord[];
  onOpenSkill?: (name: string) => void;
}) {
  const title = ruleTitle(rule);
  const path = ruleSourcePath(rule);
  const displayPath = formatUserPath(path);
  const agents = ruleAgents(rule);
  const agentLabels = agents.map((agent) => friendlyAgent(agent)).join(", ");
  const kind = rule.kind && rule.kind !== title ? rule.kind : "";
  const order = typeof rule.order === "number" && rule.order !== 0 ? `${rule.order}` : "";
  return (
    <InfoDropdownMenu
      trigger={(
        <IconButton className="threadPanelToggle" aria-label="Show rule info">
          <Info size={15} />
        </IconButton>
      )}
      label="Rule info"
      title={title}
      contentClassName="ruleInfoContent"
    >
            <InfoSection label="Agents">
                <AgentChips agents={agents} />
                <span className="ruleInfoValue">{agentLabels}</span>
            </InfoSection>
            {rule.scope && (
              <InfoSection label="Scope"><span className="ruleInfoValue">{rule.scope}</span></InfoSection>
            )}
            {kind && (
              <InfoSection label="Kind"><span className="ruleInfoValue">{kind}</span></InfoSection>
            )}
            {order && (
              <InfoSection label="Order"><span className="ruleInfoValue">{order}</span></InfoSection>
            )}
            {path && (
              <InfoSection label="Path" className="ruleInfoPath">
                  <Tooltip content={displayPath} onlyWhenTruncated><code>{displayPath}</code></Tooltip>
                  <button
                    aria-label={revealPathLabel("rule")}
                    className="appButton appButton-icon"
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path })}
                  >
                    <FolderOpen size={13} />
                  </button>
                  <CopyButton className="appButton appButton-icon" value={path} copyLabel={copyPathLabel("rule")} copiedLabel={copiedPathLabel("rule")} />
              </InfoSection>
            )}
            {referencedSkills.length > 0 && (
              <InfoSection label="Referenced skills" valueLine={false}>
                <div className="ruleInfoSkillRefs">
                  {referencedSkills.map((skill) => (
                    <Tooltip key={skill.id ?? skill.name} content={skill.description}><button
                      disabled={!onOpenSkill}
                      onClick={() => onOpenSkill?.(skill.id ?? skill.name)}
                    >
                      {skill.name}
                    </button></Tooltip>
                  ))}
                </div>
              </InfoSection>
            )}
    </InfoDropdownMenu>
  );
}

export function RulesView({
  rows,
  skills = [],
  loadingRows = false,
  loadError = "",
  hasRows = false,
  onRetry,
  onOpenSkill,
  onDeleteRules,
  onRuleSaved,
  projects,
}: {
  rows: RuleRecord[];
  skills?: SkillDependencyRecord[];
  loadingRows?: boolean;
  loadError?: string;
  hasRows?: boolean;
  onRetry?: () => void;
  onOpenSkill?: (name: string) => void;
  onDeleteRules?: (paths: string[]) => Promise<CatalogMutationResponse>;
  onRuleSaved?: (path: string, sha256: string) => void;
  projects?: ProjectSummary[];
}) {
  const projectList = projects ?? [];
  const [query, setQuery] = useState("");
  const ruleView = useMemo(() => selectRuleListView(rows, query), [query, rows]);
  const { items: ruleItems, tableRows } = ruleView;
  const [activeKey, setActiveKey] = useState(ruleItems[0]?.key ?? "");
  const [selected, setSelected] = useState<string[]>([]);
  const [sort, setSort] = useState<SortState>({ key: "order", direction: SortDirection.Asc });
  const [draft, setDraft] = useState({ content: "", originalContent: "", sha256: "" });
  const [loading, setLoading] = useState(false);
  const [detailCollapsed, setDetailCollapsed] = useState(false);
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [pendingKey, setPendingKey] = useState("");
  const [pendingDeleteRows, setPendingDeleteRows] = useState<RuleTableRow[]>([]);
  const [pendingDeleteConfirmRows, setPendingDeleteConfirmRows] = useState<RuleTableRow[]>([]);
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const loadedRulePathRef = useRef("");
  const [ruleLoadError, setRuleLoadError] = useState("");
  const [ruleLoadAttempt, setRuleLoadAttempt] = useState(0);
  const normalizedQuery = query.trim().toLowerCase();
  const activeItem = useMemo(() => ruleItems.find((item) => item.key === activeKey), [activeKey, ruleItems]);
  const activeRule = activeItem?.rule ?? null;
  const content = draft.content;
  const hasLoadedRule = Boolean(
    activeRule
      && loadedRulePathRef.current === activeRule.path,
  );
  const deferredContent = useDeferredValue(content);
  const referencedSkills = useMemo(
    () => ruleSkillRefs(content, skills, activeRule),
    [activeRule, content, skills],
  );
  const dirty = content !== draft.originalContent;
  const dirtyRef = useRef(dirty);
  dirtyRef.current = dirty;
  const diffLines = useMemo(
    () => loading || !dirty ? [] : diffPreview(draft.originalContent, deferredContent),
    [deferredContent, dirty, draft.originalContent, loading],
  );
  const diffStats = useMemo(
    () => diffLines.reduce(
      (counts: { added: number; removed: number }, line: { kind?: string }) => ({
        added: counts.added + (line.kind === DiffLineKind.Added ? 1 : 0),
        removed: counts.removed + (line.kind === DiffLineKind.Removed ? 1 : 0),
      }),
      { added: 0, removed: 0 },
    ),
    [diffLines],
  );
  const requestDeleteRules = useCallback((items: RuleTableRow[]) => {
    const deletable = items.filter((item) => Boolean(item.rule.path));
    if (deletable.length === 0) return;
    if (dirty && deletable.some((item) => item.rule.path === activeRule?.path)) {
      setPendingKey("");
      setPendingDeleteRows(deletable);
      setPendingDeleteConfirmRows([]);
      setShowDiscardDialog(true);
      return;
    }
    setPendingDeleteConfirmRows(deletable);
  }, [activeRule?.path, dirty]);
  const confirmDeleteRules = useCallback(async () => {
    const targets = pendingDeleteConfirmRows;
    if (targets.length === 0 || deleting) return;
    setDeleting(true);
    setDeleteError("");
    try {
      const result = await onDeleteRules?.(targets.map((item) => item.rule.path));
      const error = operationError(result);
      if (error) {
        setDeleteError(error);
        return;
      }
      if (!result) {
        setDeleteError(selectionDeleteErrorLabel("rule", targets.length));
        return;
      }
      setPendingDeleteConfirmRows([]);
      setSelected([]);
    } catch (error) {
      setDeleteError(`${error}`);
    } finally {
      setDeleting(false);
    }
  }, [deleting, onDeleteRules, pendingDeleteConfirmRows]);
  const columns = useMemo((): ColumnDef<RuleTableRow>[] => [
    {
      key: "source",
      header: "Rule",
      type: ColumnDataType.Text,
      sticky: true,
      sortable: true,
      sortValue: (row) => ruleTitle(row.rule).toLowerCase(),
      width: "var(--data-freeze-column-width, 292px)",
      render: (row) => {
        const path = row.rule.path;
        const displayPath = formatUserPath(path);
        return (
          <>
            <span className="dataCellTitle">{ruleTitle(row.rule)}</span>
            <Tooltip content={displayPath} onlyWhenTruncated><span className="dataCellSub">{displayPath || "-"}</span></Tooltip>
          </>
        );
      },
    },
    ...sharedRuleColumns
      .filter((column) => column.key !== "source" && column.key !== "scope")
      .map((column): ColumnDef<RuleTableRow> => {
        const next: ColumnDef<RuleTableRow> = {
          key: column.key,
          width: column.width,
          header: column.label ?? column.header,
          type: column.type ?? (["agents", "kind", "scope"].includes(column.key) ? ColumnDataType.Enum : ColumnDataType.Text),
          sortable: true,
          sortValue: (row) => ruleSortValue({ rule: row.rule }, column.key),
        };
        if (column.key === "agents") {
          next.groupBy = (row) => ruleAgents(row.rule).join(", ");
          next.render = (row) => column.render?.(row.rule);
        } else if (column.key === "kind") {
          next.groupBy = (row) => row.rule.kind;
          next.value = (row) => row.rule.kind;
        } else if (column.key === "scope") {
          next.groupBy = (row) => row.rule.scope;
          next.value = (row) => row.rule.scope;
        } else if (column.render) {
          next.render = (row) => column.render?.({ ...row.rule });
        } else if (column.value) {
          next.value = (row) => column.value?.({ ...row.rule });
        } else {
          next.value = (row) => row.rule[column.key as keyof RuleRecord];
        }
        return next;
      }),
    ...(projectList.length > 0 ? [scopeColumn<RuleTableRow>(projectList, (row) => ruleSourcePath(row.rule))] : []),
    {
      key: "actions",
      header: "",
      width: "40px",
      render: (row) => <RuleActionsCell rule={row.rule} requestDeleteRules={requestDeleteRules} deleting={deleting} />,
    },
  ], [deleting, projectList, requestDeleteRules]);
  const rowContextMenu = useCallback((row: RuleTableRow, { selectedRows, selected: isSelected }: { selectedRows: RuleTableRow[]; selected: boolean }) => {
    const actionRows = isSelected ? selectedRows : [row];
    const actions = ruleSelectionActions(actionRows, ContextMenu, requestDeleteRules, deleting);
    return actions.length > 0 ? renderDataTableSelectionMenu(actions) : null;
  }, [deleting, requestDeleteRules]);
  const bottomBar = useCallback((selectedRows: RuleTableRow[]) => (
    <DataTableSelectionActions actions={ruleSelectionActions(selectedRows, DropdownMenu, requestDeleteRules, deleting)} ariaLabel="More selected rule actions" />
  ), [deleting, requestDeleteRules]);

  useEffect(() => {
    if (!activeKey && ruleItems[0]) setActiveKey(ruleItems[0].key);
    if (activeKey && !ruleItems.some((item) => item.key === activeKey) && !dirty) {
      setActiveKey(ruleItems[0]?.key ?? "");
    }
  }, [activeKey, dirty, ruleItems]);

  useEffect(() => {
    setSelected((current) => current.filter((key) => ruleItems.some((item) => item.key === key)));
  }, [ruleItems]);

  useEffect(() => {
    let cancelled = false;
    const rulePath = activeRule?.path ?? "";
    if (!rulePath) {
      loadedRulePathRef.current = "";
      setDraft({ content: "", originalContent: "", sha256: "" });
      setRuleLoadError("");
      setLoading(false);
      return () => { cancelled = true; };
    }
    if (dirtyRef.current && loadedRulePathRef.current === rulePath) {
      return () => { cancelled = true; };
    }
    const sameRule = loadedRulePathRef.current === rulePath;
    if (!sameRule) {
      loadedRulePathRef.current = "";
      setDraft({ content: "", originalContent: "", sha256: "" });
    }
    setLoading(true);
    setRuleLoadError("");
    async function loadRule() {
      try {
        const result = await readRule(rulePath);
        if (cancelled) return;
        loadedRulePathRef.current = rulePath;
        setDraft({ content: result.content, originalContent: result.content, sha256: result.sha256 });
        setRuleLoadError("");
      } catch {
        if (cancelled) return;
        if (!sameRule) {
          loadedRulePathRef.current = "";
          setDraft({ content: "", originalContent: "", sha256: "" });
        }
        setRuleLoadError("Could not load rule. Try again.");
      } finally {
        if (!cancelled) setLoading(false);
      }
    }
    loadRule();
    return () => { cancelled = true; };
  }, [activeRule?.path, activeRule?.sha256, ruleLoadAttempt]);

  const save = useCallback(async () => {
    if (!dirty || !draft.sha256 || !activeRule?.path) return;
    const result = await saveRule({
      path: activeRule.path,
      expectedSha256: draft.sha256,
      content,
    });
    if (typeof result?.sha256 === "string") {
      const savedContent = typeof result.content === "string" ? result.content : content;
      setDraft({ content: savedContent, originalContent: savedContent, sha256: result.sha256 });
      if (activeRule?.path) onRuleSaved?.(activeRule.path, result.sha256);
    }
  }, [activeRule?.path, content, dirty, draft.sha256, onRuleSaved]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "s") {
        event.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [save]);

  const openRule = (key: string) => {
    if (key === activeKey) return;
    if (dirty) {
      setPendingKey(key);
      setShowDiscardDialog(true);
      return;
    }
    setActiveKey(key);
    setDetailCollapsed(false);
  };

  const discardAndOpenPending = () => {
    const deleteTargets = pendingDeleteRows;
    setPendingDeleteRows([]);
    if (deleteTargets.length > 0) {
      setDraft((current) => ({ ...current, content: current.originalContent }));
      setPendingDeleteConfirmRows(deleteTargets);
      return;
    }
    if (pendingKey) {
      setActiveKey(pendingKey);
      setDetailCollapsed(false);
      setPendingKey("");
    }
  };

  return (
    <PanelGroup className="sessionsLayout rulesLayout" orientation="horizontal">
      <DiscardChangesDialog
        open={showDiscardDialog}
        onOpenChange={(open) => {
          setShowDiscardDialog(open);
          if (!open) {
            setPendingDeleteRows([]);
            setPendingKey("");
          }
        }}
        onDiscard={discardAndOpenPending}
      />
      <DeleteConfirmationDialog
        open={pendingDeleteConfirmRows.length > 0}
        items={pendingDeleteConfirmRows.map((item) => formatUserPath(item.rule.path))}
        itemLabel="rule"
        busy={deleting}
        onOpenChange={(open) => { if (!open) setPendingDeleteConfirmRows([]); }}
        onConfirm={() => { void confirmDeleteRules(); }}
      />
      <Panel className="sessionListPanel ruleListPanel" defaultSize="54%" minSize="360px">
        <div className="sessionListPane ruleListPane">
          <PageHeader title="Rules" compact>
            <SearchField pageSearch placeholder="Search rules" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
          </PageHeader>
          {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : null}
          {deleteError ? <Toast tone="error" message={deleteError} onDismiss={() => setDeleteError("")} /> : null}
          <div className="sessionListBody">
            <DataTable
              rows={tableRows}
              columns={columns}
              getRowId={(row) => row.key}
              getRowLabel={(row) => ruleTitle(row.rule)}
              selectable
              selectedIds={selected}
              onSelectionChange={setSelected}
              enableMarquee
              defaultSort={{ key: "order", direction: SortDirection.Asc }}
              sort={sort}
              onSortChange={setSort}
              freezeColumn={RULE_FREEZE_COLUMN}
              onRowClick={(row) => openRule(row.key)}
              rowContextMenu={rowContextMenu}
              bottomBar={bottomBar}
              bottomBarActionsClassName="selectionActions"
              bottomBarCheckboxLabel="Select visible rules from toolbar"
              selectionLabel="rules"
              loading={loadingRows && !hasRows}
              loadingLabel="Loading rules"
              emptyState={loadError && !hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : (
                <EmptyState
                  icon={normalizedQuery ? <SearchX size={21} strokeWidth={1.8} /> : <ScrollText size={27} strokeWidth={1.55} />}
                  iconTone={normalizedQuery ? "muted" : "accent"}
                  title={normalizedQuery ? "No rules match this search" : "No rules found"}
                  description={normalizedQuery ? "Try another search." : "Try another agent filter."}
                />
              )}
            />
          </div>
        </div>
      </Panel>
      <DetailPanelHost
        collapsed={detailCollapsed}
        onExpand={() => setDetailCollapsed(false)}
        expandLabel="Expand rule detail"
        railLabel={activeRule ? ruleTitle(activeRule) : "Rule detail"}
        hasSelection={Boolean(activeRule)}
        emptyState={<EmptyState compact title="Select a rule to view its contents." />}
        hostClassName="ruleEditorPanelHost"
      >
        {activeRule ? (
          <DetailPanel
            title={ruleTitle(activeRule)}
            collapseLabel="Collapse rule detail"
            onCollapse={() => setDetailCollapsed(true)}
            headerActions={(
              <RuleInfoMenu
                rule={activeRule}
                referencedSkills={referencedSkills}
                onOpenSkill={onOpenSkill}
              />
            )}
          >
            {!hasLoadedRule && ruleLoadError ? (
              <div className="ruleEditorLoading">
                <LoadErrorState message={ruleLoadError} onRetry={() => setRuleLoadAttempt((attempt) => attempt + 1)} />
              </div>
            ) : !hasLoadedRule ? (
              <EditorStatePlaceholder className="ruleEditorLoading" label="Loading rule" />
            ) : (
              <div className="ruleEditorContent">
                <Suspense fallback={<EditorStatePlaceholder className="ruleEditorLoading" label="Loading editor" />}>
                  <MarkdownFilePane
                    activePath={ruleSourcePath(activeRule)}
                    dirty={dirty}
                    diffStats={diffStats}
                    content={content}
                    originalContent={draft.originalContent}
                    copyablePath
                    onSave={save}
                    onChange={(nextContent: string) => setDraft((current) => ({ ...current, content: nextContent }))}
                  />
                </Suspense>
                {loading ? (
                  <div className="ruleEditorStatusOverlay" aria-live="polite">
                    <LoadingState label="Refreshing rule" />
                  </div>
                ) : ruleLoadError ? (
                  <div className="ruleEditorStatusOverlay">
                    <LoadErrorState message={ruleLoadError} onRetry={() => setRuleLoadAttempt((attempt) => attempt + 1)} />
                  </div>
                ) : null}
              </div>
            )}
          </DetailPanel>
        ) : null}
      </DetailPanelHost>
    </PanelGroup>
  );
}
