import { Tooltip } from "../components/shared/Tooltip.tsx";
import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useMemo, useState, type ComponentType, type ReactNode } from "react";
import { ContextMenu, DropdownMenu } from "radix-ui";
import { FolderOpen, Info, ScrollText, SearchX } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import { AgentChips } from "../components/shared/AgentChips.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { CopyPathMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DetailPanel } from "../components/shared/DetailPanel.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { DiscardChangesDialog } from "../components/shared/DiscardChangesDialog.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { EditorStatePlaceholder } from "../components/shared/EditorStatePlaceholder.tsx";
import { InfoDropdownMenu } from "../components/shared/InfoDropdownMenu.tsx";
import { InfoSection } from "../components/shared/InfoSection.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RowActionsMenu } from "../components/shared/RowActionsMenu.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import type { SkillDependencyRecord } from "../features/skills/SkillDependencyGraph.tsx";
import { ruleColumns as sharedRuleColumns } from "../lib/tableColumns.tsx";
import { RULE_FREEZE_COLUMN, TauriCommand, diffPreview, formatUserPath, friendlyAgent, ruleAgents, ruleKey, ruleSearchText, ruleSortValue, ruleTitle, safeInvoke, scopeColumn, suppressNextClick, type ProjectSummary, type RuleRecord } from "../lib/index.ts";

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

function RuleActionsMenuItems({ Menu, rule }: { Menu: RuleMenuComponents; rule: RuleRecord }) {
  const path = ruleSourcePath(rule);
  return (
    <>
      <RevealInFinderMenuItem Menu={Menu} path={path} />
      <CopyPathMenuItem Menu={Menu} path={path} />
    </>
  );
}

function RuleActionsCell({ rule }: { rule: RuleRecord }) {
  return (
    <RowActionsMenu
      ariaLabel={`Rule actions for ${ruleTitle(rule)}`}
      onOpenChange={(open) => { if (!open) suppressNextClick(); }}
    >
      <RuleActionsMenuItems Menu={DropdownMenu} rule={rule} />
    </RowActionsMenu>
  );
}

function normalizeSkillName(value: string) {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

function ruleSkillRefs(content: string, skills: SkillDependencyRecord[]) {
  const byName = new Map(skills.map((skill) => [normalizeSkillName(skill.name), skill.name]));
  const refs = new Set<string>();
  for (const match of content.matchAll(/(?:^|[^\w])[$/]([a-zA-Z0-9][a-zA-Z0-9_-]*)/g)) {
    const name = byName.get(normalizeSkillName(match[1]));
    if (name) refs.add(name);
  }
  return [...refs].sort((left, right) => left.localeCompare(right));
}

function RuleInfoMenu({
  rule,
  referencedSkillNames,
  skillsByName,
  onOpenSkill,
}: {
  rule: RuleRecord;
  referencedSkillNames: string[];
  skillsByName: Map<string, SkillDependencyRecord>;
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
                    aria-label="Reveal rule in Finder"
                    className="appButton appButton-icon"
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path })}
                  >
                    <FolderOpen size={13} />
                  </button>
                  <CopyButton className="appButton appButton-icon" value={path} copyLabel="Copy rule path" copiedLabel="Rule path copied" />
              </InfoSection>
            )}
            {referencedSkillNames.length > 0 && (
              <InfoSection label="Referenced skills" valueLine={false}>
                <div className="ruleInfoSkillRefs">
                  {referencedSkillNames.map((name) => {
                    const skill = skillsByName.get(name);
                    return (
                      <Tooltip key={name} content={skill?.description}><button
                        key={name}
                        disabled={!onOpenSkill}
                        onClick={() => onOpenSkill?.(name)}
                      >
                        {name}
                      </button></Tooltip>
                    );
                  })}
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
  projects,
}: {
  rows: RuleRecord[];
  skills?: SkillDependencyRecord[];
  loadingRows?: boolean;
  loadError?: string;
  hasRows?: boolean;
  onRetry?: () => void;
  onOpenSkill?: (name: string) => void;
  projects?: ProjectSummary[];
}) {
  const projectList = projects ?? [];
  const ruleItems = useMemo(() => rows.map((rule) => ({ key: ruleKey(rule), rule })), [rows]);
  const [activeKey, setActiveKey] = useState(ruleItems[0]?.key ?? "");
  const [selected, setSelected] = useState<string[]>([]);
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState>({ key: "order", direction: "asc" });
  const [draft, setDraft] = useState({ content: "", originalContent: "", sha256: "" });
  const [loading, setLoading] = useState(false);
  const [detailCollapsed, setDetailCollapsed] = useState(false);
  const [showDiscardDialog, setShowDiscardDialog] = useState(false);
  const [pendingKey, setPendingKey] = useState("");
  const normalizedQuery = query.trim().toLowerCase();
  const activeItem = useMemo(() => ruleItems.find((item) => item.key === activeKey), [activeKey, ruleItems]);
  const activeRule = activeItem?.rule ?? null;
  const skillsByName = useMemo(() => new Map(skills.map((skill) => [skill.name, skill])), [skills]);
  const content = draft.content;
  const deferredContent = useDeferredValue(content);
  const referencedSkillNames = useMemo(() => ruleSkillRefs(content, skills), [content, skills]);
  const dirty = content !== draft.originalContent;
  const diffLines = useMemo(
    () => loading || !dirty ? [] : diffPreview(draft.originalContent, deferredContent),
    [deferredContent, dirty, draft.originalContent, loading],
  );
  const diffStats = useMemo(
    () => diffLines.reduce(
      (counts: { added: number; removed: number }, line: { kind?: string }) => ({
        added: counts.added + (line.kind === "added" ? 1 : 0),
        removed: counts.removed + (line.kind === "removed" ? 1 : 0),
      }),
      { added: 0, removed: 0 },
    ),
    [diffLines],
  );
  const filteredRules = useMemo(() => {
    if (!normalizedQuery) return ruleItems;
    return ruleItems.filter((item) => ruleSearchText(item.rule).includes(normalizedQuery));
  }, [normalizedQuery, ruleItems]);
  const tableRows = useMemo(
    () => filteredRules.map((item) => ({ ...item, id: item.key })),
    [filteredRules],
  );
  const columns = useMemo((): ColumnDef<RuleTableRow>[] => [
    {
      key: "source",
      header: "Rule",
      type: "text",
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
          type: column.type ?? (["agents", "kind", "scope"].includes(column.key) ? "enum" : "text"),
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
      render: (row) => <RuleActionsCell rule={row.rule} />,
    },
  ], [projectList]);
  const rowContextMenu = useCallback((row: RuleTableRow, { selectedRows, selected: isSelected }: { selectedRows: RuleTableRow[]; selected: boolean }) => {
    if (isSelected && selectedRows.length > 1) return null;
    return <RuleActionsMenuItems Menu={ContextMenu} rule={row.rule} />;
  }, []);
  const bottomBar = useCallback((selectedRows: RuleTableRow[]) => {
    const firstPath = ruleSourcePath(selectedRows[0].rule);
    return (
      <>
        <button
          aria-label="Reveal selected rule in Finder"
          disabled={!firstPath}
          onClick={() => firstPath && safeInvoke(TauriCommand.RevealInFinder, { path: firstPath })}
        >
          Reveal in Finder
        </button>
        <CopyButton
          value={firstPath}
          copyLabel="Copy path"
          copiedLabel="Path copied"
          disabled={!firstPath}
          iconSize={15}
        >
          Copy path
        </CopyButton>
      </>
    );
  }, []);

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
    setDraft({ content: "", originalContent: "", sha256: "" });
    if (!activeRule?.path) {
      setLoading(false);
      return () => { cancelled = true; };
    }
    const rulePath = activeRule.path;
    setLoading(true);
    async function loadRule() {
      const result = await safeInvoke(TauriCommand.RuleFileRead, { path: rulePath }) as { content?: string; sha256?: string } | null;
      if (cancelled) return;
      if (typeof result?.content === "string" && typeof result.sha256 === "string") {
        setDraft({ content: result.content, originalContent: result.content, sha256: result.sha256 });
      } else {
        setDraft({ content: "", originalContent: "", sha256: "" });
      }
      setLoading(false);
    }
    loadRule();
    return () => { cancelled = true; };
  }, [activeRule?.path, activeRule?.sha256]);

  const save = useCallback(async () => {
    if (!dirty || !draft.sha256 || !activeRule?.path) return;
    const result = await safeInvoke(TauriCommand.RuleFileSave, {
      path: activeRule.path,
      expectedSha256: draft.sha256,
      content,
    }) as { sha256?: string; content?: string } | null;
    if (typeof result?.content === "string" && typeof result.sha256 === "string") {
      setDraft({ content: result.content, originalContent: result.content, sha256: result.sha256 });
    }
  }, [activeRule?.path, content, dirty, draft.sha256]);

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
    if (pendingKey) {
      setActiveKey(pendingKey);
      setDetailCollapsed(false);
      setPendingKey("");
    }
  };

  return (
    <PanelGroup className="sessionsLayout rulesLayout" orientation="horizontal">
      <DiscardChangesDialog open={showDiscardDialog} onOpenChange={setShowDiscardDialog} onDiscard={discardAndOpenPending} />
      <Panel className="sessionListPanel ruleListPanel" defaultSize="54%" minSize="360px">
        <div className="sessionListPane ruleListPane">
          <PageHeader title="Rules" compact>
            <SearchField pageSearch placeholder="Search rules" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
          </PageHeader>
          {loadError && hasRows ? <LoadErrorState message={loadError} onRetry={onRetry} /> : null}
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
              defaultSort={{ key: "order", direction: "asc" }}
              sort={sort}
              onSortChange={setSort}
              freezeColumn={RULE_FREEZE_COLUMN}
              onRowClick={(row) => openRule(row.key)}
              rowContextMenu={rowContextMenu}
              bottomBar={bottomBar}
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
                referencedSkillNames={referencedSkillNames}
                skillsByName={skillsByName}
                onOpenSkill={onOpenSkill}
              />
            )}
          >
            {loading ? (
              <EditorStatePlaceholder className="ruleEditorLoading" label="Loading rule" />
            ) : (
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
            )}
          </DetailPanel>
        ) : null}
      </DetailPanelHost>
    </PanelGroup>
  );
}
