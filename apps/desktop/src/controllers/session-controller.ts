import { agentIdentityKey, friendlyAgent } from "../lib/agents.ts";
import { compactDateTime, dayGroupKey } from "../lib/strings.ts";
import { formatTranscriptPreview } from "../lib/session-preview.ts";
import { transcriptItemType, type JsonlTranscriptParseResult } from "../lib/transcript.ts";
import {
  compareSessions,
  normalizeSession,
  sessionIdentity,
  sessionIdentityRecordKey,
  SessionKind,
  sessionKind,
  sessionProject,
  sessionProjectGroupKey,
  sessionProjectOption,
  SessionSortKey,
  type SessionSkillLinkRecord,
  type SessionIdentityRecord,
  type SessionRecord,
  type SortState,
} from "../lib/sessions.ts";
import type { RawDomainRow } from "./controller-types.ts";
import type { MissingSessionProjectPolicy, ProjectSummary, SessionProjectSummary } from "../lib/projects.ts";

export type SessionProjectOption = {
  key: string;
  label: string;
  title: string;
  count: number;
};

export type GroupedSessionPage = {
  rows: SessionRecord[];
  start: number;
  end: number;
  groupCount: number;
};

export type SessionListControllerInput = {
  sessions: readonly SessionRecord[];
  importedSessions?: readonly SessionRecord[];
  searchRows?: readonly SessionRecord[];
  query: string;
  remoteSearch: boolean;
  searchSort?: SortState | null;
  sort: SortState;
  pageSize: number;
  groupBy: string | null;
  showChildSessions: boolean;
  selectedProjectKeys: readonly string[];
  projectFilterQuery: string;
  missingSessionProjectPolicy: MissingSessionProjectPolicy;
  projects: readonly ProjectSummary[];
  sessionProjects: readonly SessionProjectSummary[];
  currentPage?: number;
  pageSelectionContextKey?: string;
  searchRowsKey?: string;
};

export type SessionListView = {
  allSessionItems: SessionRecord[];
  projectOptions: SessionProjectOption[];
  visibleProjectOptions: SessionProjectOption[];
  searchCandidates: SessionRecord[];
  searchRequestKey: string;
  childSessionCount: number;
  activeSort: SortState;
  pageContextKey: string;
  currentPage: number;
  sortedSessions: SessionRecord[];
  groupedPages: GroupedSessionPage[];
  pageCount: number;
  boundedCurrentPage: number;
  pageStart: number;
  pageEnd: number;
  tableSessions: SessionRecord[];
};

export const IMPORTED_SESSION_AGENT = "Imported";

export function coalesceSessionEventBuffer(
  recent: readonly RawDomainRow[],
  watch: readonly RawDomainRow[],
  deleted: readonly SessionIdentityRecord[],
): { upserts: RawDomainRow[]; deleted: readonly SessionIdentityRecord[] } {
  const upserts = new Map<string, RawDomainRow>();
  for (const session of [...recent, ...watch]) {
    const key = sessionIdentityRecordKey(session);
    if (key) upserts.set(key, session);
  }
  return { upserts: [...upserts.values()], deleted };
}

export function normalizeSessionRows(rows: readonly RawDomainRow[]): SessionRecord[] {
  return rows.flatMap((row) => normalizeSession(row) ?? []);
}

export function sessionSearchCandidateRows(candidates: readonly SessionRecord[]): Pick<SessionRecord, "id" | "agent" | "path">[] {
  return candidates
    .filter((session) => agentIdentityKey(session.agent) !== "imported")
    .map((session) => ({
      id: session.id,
      agent: agentIdentityKey(session.agent),
      path: session.path,
    }));
}

export function createImportedSessionRecord(input: {
  id: string;
  fileName: string;
  lastModified: number;
  parsed: JsonlTranscriptParseResult;
}): SessionRecord {
  const { id, fileName, lastModified, parsed } = input;
  const startedAt = parsed.startedAt ?? new Date(lastModified || Date.now()).toISOString();
  const updatedAt = parsed.updatedAt ?? startedAt;
  const userMessages = parsed.items
    .filter((item) => transcriptItemType(item) === "user")
    .map((item) => formatTranscriptPreview(item.body))
    .filter(Boolean);
  const assistantMessages = parsed.items
    .filter((item) => transcriptItemType(item) === "assistant")
    .map((item) => formatTranscriptPreview(item.body))
    .filter(Boolean);
  return {
    id,
    title: formatTranscriptPreview(parsed.title),
    project: parsed.project ?? "",
    projectPath: parsed.project ?? "",
    agent: IMPORTED_SESSION_AGENT,
    path: fileName,
    startedAt,
    updatedAt,
    time: updatedAt,
    startedLabel: compactDateTime(startedAt),
    updatedLabel: compactDateTime(updatedAt),
    updatedDetailLabel: compactDateTime(updatedAt, { year: true }),
    messages: parsed.items.length,
    firstUserMessage: userMessages[0],
    lastUserMessage: userMessages.at(-1),
    lastAssistantMessage: assistantMessages.at(-1),
    tokenUsage: parsed.tokenUsage,
  };
}

export type LinkedSessionRow = SessionRecord & { linkedSessionLink: SessionSkillLinkRecord };

export function linkedSessionToSession(link: SessionSkillLinkRecord): LinkedSessionRow | undefined {
  const id = link.session_id.trim();
  const sessionPath = link.session_path.trim();
  const agent = link.agent.trim();
  if (!id || !sessionPath || !agent) return undefined;
  const project = typeof link.session_project === "string" ? link.session_project.trim() : "";
  const normalized = normalizeSession({
    id,
    agent,
    title: link.session_title,
    project,
    path: sessionPath,
    started_at: link.session_started_at,
    updated_at: link.session_updated_at,
    message_count: link.session_message_count,
  });
  if (!normalized) return undefined;
  return {
    ...normalized,
    projectPath: project,
    path: sessionPath,
    linkedSessionLink: link,
  };
}

export type SessionRelationshipView = {
  childSessions: SessionRecord[];
  tree: SessionRecord[];
};

function rowId(session: SessionRecord): string {
  return `${session.agent}\0${session.id}`;
}

export function sessionMatchesQuery(session: SessionRecord, query: string): boolean {
  return [
    session.title,
    sessionProject(session),
    session.projectPath,
    session.agent,
    session.model,
    session.mode,
    session.approvalMode,
    session.isRunEverything,
    session.startedAt,
    session.updatedAt,
  ].some((value) => `${value ?? ""}`.toLowerCase().includes(query));
}

export function selectSessionRelationships(
  sessions: readonly SessionRecord[],
  activeSession: SessionRecord | null | undefined,
): SessionRelationshipView {
  if (!activeSession) return { childSessions: [], tree: [] };
  const agent = friendlyAgent(activeSession.agent).toLowerCase();
  const sameAgentSessions = sessions.filter((session) => friendlyAgent(session.agent).toLowerCase() === agent);
  const byId = new Map(sameAgentSessions.map((session) => [session.id.toLowerCase(), session]));
  const childSessions = sameAgentSessions.filter((session) => session.parentSessionId?.toLowerCase() === activeSession.id.toLowerCase());
  let rootSession = activeSession;
  const visited = new Set<string>();
  while (rootSession.parentSessionId && !visited.has(rootSession.id.toLowerCase())) {
    visited.add(rootSession.id.toLowerCase());
    const parent = byId.get(rootSession.parentSessionId.toLowerCase());
    if (!parent) break;
    rootSession = parent;
  }
  const childrenByParent = new Map<string, SessionRecord[]>();
  for (const session of sameAgentSessions) {
    if (!session.parentSessionId) continue;
    const children = childrenByParent.get(session.parentSessionId.toLowerCase()) ?? [];
    children.push(session);
    childrenByParent.set(session.parentSessionId.toLowerCase(), children);
  }
  const tree: SessionRecord[] = [];
  const pending = [rootSession];
  const treeIds = new Set<string>();
  while (pending.length > 0) {
    const session = pending.shift();
    if (!session || treeIds.has(session.id.toLowerCase())) continue;
    treeIds.add(session.id.toLowerCase());
    tree.push(session);
    pending.push(...(childrenByParent.get(session.id.toLowerCase()) ?? []));
  }
  return { childSessions, tree };
}

export type ProjectSearchRank = { distance: number; start: number; length: number };

export function projectSearchRank(label: string, query: string): ProjectSearchRank | null {
  const value = [...label.toLowerCase()];
  const needle = [...query.toLowerCase()];
  if (needle.length === 0) return null;
  const required = new Map<string, number>();
  for (const character of needle) required.set(character, (required.get(character) ?? 0) + 1);
  const found = new Map<string, number>();
  let complete = 0;
  let left = 0;
  let best: ProjectSearchRank | null = null;
  for (let right = 0; right < value.length; right += 1) {
    const character = value[right];
    const requiredCount = required.get(character) ?? 0;
    if (requiredCount === 0) continue;
    const nextCount = (found.get(character) ?? 0) + 1;
    found.set(character, nextCount);
    if (nextCount <= requiredCount) complete += 1;
    while (complete === needle.length && left <= right) {
      const candidate = { distance: right - left + 1, start: left, length: value.length };
      if (!best || candidate.distance < best.distance || (candidate.distance === best.distance && candidate.start < best.start)) best = candidate;
      const leftCharacter = value[left];
      const leftRequiredCount = required.get(leftCharacter) ?? 0;
      if (leftRequiredCount > 0) {
        const leftCount = (found.get(leftCharacter) ?? 0) - 1;
        found.set(leftCharacter, leftCount);
        if (leftCount < leftRequiredCount) complete -= 1;
      }
      left += 1;
    }
  }
  return best;
}

function sessionGroupKeyForPaging(session: SessionRecord, groupBy: string): string {
  if (groupBy === SessionSortKey.Agent) return friendlyAgent(session.agent);
  if (groupBy === SessionSortKey.Project) return sessionProjectGroupKey(session);
  if (groupBy === SessionSortKey.StartedAt) return dayGroupKey(session.startedAt);
  if (groupBy === SessionSortKey.UpdatedAt) return dayGroupKey(session.updatedAt);
  return `${session[groupBy as keyof SessionRecord] ?? ""}`;
}

export function buildGroupedSessionPages(sessions: readonly SessionRecord[], groupBy: string | null, pageSize: number): GroupedSessionPage[] {
  if (!groupBy) return [];
  const groups: SessionRecord[][] = [];
  const groupIndex = new Map<string, SessionRecord[]>();
  for (const session of sessions) {
    const key = sessionGroupKeyForPaging(session, groupBy);
    const group = groupIndex.get(key) ?? [];
    if (group.length === 0) {
      groupIndex.set(key, group);
      groups.push(group);
    }
    group.push(session);
  }
  const pages: GroupedSessionPage[] = [];
  let pageRows: SessionRecord[] = [];
  let pageStart = 0;
  let pageGroupCount = 0;
  for (const group of groups) {
    if (pageRows.length > 0 && pageRows.length >= pageSize) {
      pages.push({ rows: pageRows, start: pageStart, end: pageStart + pageRows.length, groupCount: pageGroupCount });
      pageStart += pageRows.length;
      pageRows = [];
      pageGroupCount = 0;
    }
    pageRows.push(...group);
    pageGroupCount += 1;
  }
  if (pageRows.length > 0 || sessions.length === 0) pages.push({ rows: pageRows, start: pageStart, end: pageStart + pageRows.length, groupCount: pageGroupCount });
  return pages;
}

export function sessionPageForRow(
  sessions: readonly SessionRecord[],
  targetRowId: string,
  sort: SortState,
  groupBy: string | null,
  pageSize: number,
  showChildSessions: boolean,
): number {
  const sessionsToShow = sessions.filter((session) => showChildSessions || sessionKind(session) === SessionKind.Main);
  const sortedSessions = [...sessionsToShow].sort((left, right) => compareSessions(left, right, sort));
  const targetIndex = sortedSessions.findIndex((session) => sessionTableRowId(session) === targetRowId);
  if (groupBy) {
    return buildGroupedSessionPages(sortedSessions, groupBy, pageSize)
      .findIndex((page) => page.rows.some((session) => sessionTableRowId(session) === targetRowId));
  }
  return targetIndex < 0 ? -1 : Math.floor(targetIndex / pageSize);
}

function dedupeSessions(rows: readonly SessionRecord[]): SessionRecord[] {
  const result = new Map<string, SessionRecord>();
  for (const row of rows) result.set(rowId(row), row);
  return [...result.values()];
}

export function sessionPageContextKey(sort: SortState, groupBy: string | null, query: string, pageSize: number, projectFilterKey: string, showChildSessions: boolean): string {
  return JSON.stringify([sort.key, sort.direction, groupBy ?? "", query, pageSize, projectFilterKey, showChildSessions]);
}

type SessionListViewCacheEntry = {
  input: SessionListControllerInput;
  view: SessionListView;
};

const sessionListViewCache: SessionListViewCacheEntry[] = [];

function sameSessionListInput(left: SessionListControllerInput, right: SessionListControllerInput): boolean {
  return left.sessions === right.sessions
    && left.importedSessions === right.importedSessions
    && left.searchRows === right.searchRows
    && left.query === right.query
    && left.remoteSearch === right.remoteSearch
    && left.searchSort === right.searchSort
    && left.sort === right.sort
    && left.pageSize === right.pageSize
    && left.groupBy === right.groupBy
    && left.showChildSessions === right.showChildSessions
    && left.selectedProjectKeys === right.selectedProjectKeys
    && left.projectFilterQuery === right.projectFilterQuery
    && left.missingSessionProjectPolicy === right.missingSessionProjectPolicy
    && left.projects === right.projects
    && left.sessionProjects === right.sessionProjects
    && left.currentPage === right.currentPage
    && left.pageSelectionContextKey === right.pageSelectionContextKey
    && left.searchRowsKey === right.searchRowsKey;
}

export function selectSessionListView(input: SessionListControllerInput): SessionListView {
  const cached = sessionListViewCache.find((entry) => sameSessionListInput(entry.input, input));
  if (cached) return cached.view;
  const query = input.query.trim().toLowerCase();
  const listSessionItems = dedupeSessions([...(input.importedSessions ?? []), ...input.sessions]);
  const projectSource = input.showChildSessions ? listSessionItems : listSessionItems.filter((session) => sessionKind(session) === SessionKind.Main);
  const projectMap = new Map<string, SessionProjectOption>();
  const projectKeyByRow = new Map<string, string>();
  const projectOptionByRow = new Map<string, { key: string; label: string; title: string } | null>();
  for (const session of listSessionItems) {
    const option = sessionProjectOption(session, input.missingSessionProjectPolicy, input.sessionProjects, input.projects);
    projectOptionByRow.set(rowId(session), option);
    projectKeyByRow.set(rowId(session), option?.key ?? sessionProjectGroupKey(session));
  }
  for (const session of projectSource) {
    const option = projectOptionByRow.get(rowId(session));
    if (!option) continue;
    const current = projectMap.get(option.key);
    projectMap.set(option.key, current ? { ...current, count: current.count + 1 } : { ...option, count: 1 });
  }
  const projectOptions = [...projectMap.values()].sort((left, right) => left.label.localeCompare(right.label) || left.title.localeCompare(right.title));
  const selected = new Set(input.selectedProjectKeys);
  const projectFilter = input.projectFilterQuery.trim().toLowerCase();
  const visibleProjectOptions = projectOptions
    .filter((option) => selected.has(option.key) || !projectFilter || projectSearchRank(option.label, projectFilter) !== null)
    .sort((left, right) => {
      const leftSelected = selected.has(left.key);
      const rightSelected = selected.has(right.key);
      if (leftSelected !== rightSelected) return leftSelected ? -1 : 1;
      return right.count - left.count || left.label.localeCompare(right.label) || left.title.localeCompare(right.title);
    });
  const projectKey = (session: SessionRecord) => projectKeyByRow.get(rowId(session)) ?? sessionProjectGroupKey(session);
  const inProject = (session: SessionRecord) => selected.size === 0 || selected.has(projectKey(session));
  const searchCandidates = listSessionItems
    .filter((session) => (input.showChildSessions || sessionKind(session) === SessionKind.Main) && inProject(session))
    .sort((left, right) => compareSessions(left, right, input.searchSort ?? input.sort));
  const searchRequestKey = query ? `${query}\0${searchCandidates.map(sessionIdentity).join("\0")}` : "";
  const searchResultRows = query && input.remoteSearch && input.searchRowsKey === searchRequestKey
    ? dedupeSessions(input.searchRows ?? [])
    : [];
  const allSessionItems = dedupeSessions([...listSessionItems, ...searchResultRows]);
  const matchedSessions = query && input.remoteSearch
    ? searchResultRows.filter(inProject)
    : query
      ? listSessionItems.filter((session) => sessionMatchesQuery(session, query) && inProject(session))
      : listSessionItems.filter(inProject);
  const childSessionCount = matchedSessions.filter((session) => sessionKind(session) === SessionKind.Child).length;
  const filteredSessions = input.showChildSessions ? matchedSessions : matchedSessions.filter((session) => sessionKind(session) === SessionKind.Main);
  const activeSort = input.remoteSearch && query ? input.searchSort ?? input.sort : input.sort;
  const sortedSessions = [...filteredSessions].sort((left, right) => compareSessions(left, right, activeSort));
  const groupedPages = buildGroupedSessionPages(sortedSessions, input.groupBy, input.pageSize);
  const pageCount = input.groupBy ? Math.max(1, groupedPages.length) : Math.max(1, Math.ceil(sortedSessions.length / input.pageSize));
  const calculatedPageContextKey = sessionPageContextKey(activeSort, input.groupBy, query, input.pageSize, input.selectedProjectKeys.join("\0"), input.showChildSessions);
  const currentPage = input.pageSelectionContextKey === calculatedPageContextKey
    ? Math.max(0, input.currentPage ?? 0)
    : 0;
  const boundedCurrentPage = Math.min(currentPage, pageCount - 1);
  const groupedPage = groupedPages[boundedCurrentPage];
  const pageStart = input.groupBy ? groupedPage?.start ?? 0 : sortedSessions.length === 0 ? 0 : boundedCurrentPage * input.pageSize;
  const pageEnd = input.groupBy ? groupedPage?.end ?? 0 : Math.min(pageStart + input.pageSize, sortedSessions.length);
  const pagedSessions = sortedSessions.slice(pageStart, pageEnd);
  const tableSessions = input.groupBy ? groupedPage?.rows ?? [] : pagedSessions;
  const view = {
    allSessionItems,
    projectOptions,
    visibleProjectOptions,
    searchCandidates,
    searchRequestKey,
    childSessionCount,
    activeSort,
    pageContextKey: calculatedPageContextKey,
    currentPage,
    sortedSessions,
    groupedPages,
    pageCount,
    boundedCurrentPage,
    pageStart,
    pageEnd,
    tableSessions,
  };
  sessionListViewCache.unshift({ input, view });
  if (sessionListViewCache.length > 24) sessionListViewCache.pop();
  return view;
}

export function sessionTableRowId(session: Pick<SessionRecord, "agent" | "id">): string {
  return JSON.stringify([session.agent, session.id]);
}

export function applySessionDelta(
  current: readonly SessionRecord[],
  upserts: readonly RawDomainRow[],
  deleted: readonly SessionIdentityRecord[] = [],
): SessionRecord[] {
  const byIdentity = new Map(current.map((session) => [sessionIdentity(session), session]));
  for (const row of deleted) {
    const agent = typeof row.agent === "string" ? row.agent.trim() : "";
    const id = typeof row.id === "string" ? row.id.trim() : "";
    const path = typeof row.path === "string" ? row.path.trim() : "";
    if (agent && id && path) byIdentity.delete(`${agent}\0${id}\0${path}`);
  }
  for (const row of upserts) {
    const session = normalizeSession(row);
    if (session) byIdentity.set(sessionIdentity(session), session);
  }
  const next = [...byIdentity.values()];
  if (next.length === current.length && next.every((session, index) => session === current[index])) return current as SessionRecord[];
  return next;
}
