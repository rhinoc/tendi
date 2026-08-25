import { basename, compactDateTime } from "./strings.ts";
import { friendlyAgent, normalizedAgentKey } from "./agents.ts";
import { agentDefinition } from "./agent/index.ts";
import { sessionProjectOptionForPaths, type MissingSessionProjectPolicy, type ProjectSummary, type SessionProjectSummary } from "./projects.ts";

export type SessionRecord = {
  id: string;
  title: string;
  project?: string;
  projectPath?: string;
  repository?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
  logicalProjectId?: string;
  logicalProjectName?: string;
  path?: string;
  agent?: string;
  startedAt?: string;
  updatedAt?: string;
  time?: string;
  startedLabel?: string;
  updatedLabel?: string;
  updatedDetailLabel?: string;
  messages?: number;
  firstUserMessage?: string;
  lastUserMessage?: string;
  lastAssistantMessage?: string;
  turnCount?: number;
  model?: string;
  mode?: string;
  approvalMode?: string;
  isRunEverything?: boolean;
  parentSessionId?: string;
  searchScore?: number;
  searchSnippet?: string;
  tokenUsage?: SessionTokenUsage;
  [key: string]: unknown;
};

export type SessionResumeTarget = "auto" | "terminal" | "app";

export type SessionResumeOptions = {
  forceActiveWriter?: boolean;
};

export type SessionResumeOutcome =
  | { status: "activeWriter"; lockPath: string; pids: number[] }
  | { status: "launched"; target: SessionResumeTarget; terminal?: string };

export function normalizeSessionResumeTarget(value: unknown): SessionResumeTarget {
  if (value === "app" || value === "codex") return "app";
  if (value === "terminal") return "terminal";
  return "auto";
}

export function sessionResumeTargetForAgent(target: SessionResumeTarget, agent: unknown): SessionResumeTarget {
  if (target === "auto") return "auto";
  const definition = agentDefinition(normalizedAgentKey(agent));
  return target === "app" && definition?.sessionAppDeepLink ? "app" : "terminal";
}

export function sessionAppDeepLink(session: Pick<SessionRecord, "id" | "agent" | "project" | "projectPath">): string | undefined {
  return agentDefinition(normalizedAgentKey(session.agent))?.sessionAppDeepLink?.(session);
}

export type SessionTokenUsage = {
  inputTokens: number;
  cachedInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
};

export type SortState = { key: string; direction: "asc" | "desc" };

export type SessionKind = "main" | "child";

function textValue(value: unknown): string {
  return value === null || value === undefined ? "" : `${value}`.trim();
}

function optionalCount(value: unknown): number | undefined {
  if (value === null || value === undefined || value === "") return undefined;
  const count = Number(value);
  return Number.isFinite(count) && count >= 0 ? count : undefined;
}

function normalizeTokenUsage(value: unknown): SessionTokenUsage | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const usage = value as Record<string, unknown>;
  const inputTokens = Number(usage.input_tokens ?? usage.inputTokens);
  const cachedInputTokens = Number(usage.cached_input_tokens ?? usage.cachedInputTokens);
  const outputTokens = Number(usage.output_tokens ?? usage.outputTokens);
  const reasoningOutputTokens = Number(usage.reasoning_output_tokens ?? usage.reasoningOutputTokens ?? 0);
  const totalTokens = Number(usage.total_tokens ?? usage.totalTokens);
  const values = [inputTokens, cachedInputTokens, outputTokens, reasoningOutputTokens, totalTokens];
  if (values.some((tokenCount) => !Number.isFinite(tokenCount) || tokenCount < 0) || totalTokens <= 0) return undefined;
  return { inputTokens, cachedInputTokens, outputTokens, reasoningOutputTokens, totalTokens };
}

export function normalizeSession(session: Record<string, unknown>, index: number): SessionRecord {
  const updatedAt = `${session.updated_at ?? session.updatedAt ?? session.time ?? ""}`;
  const startedAt = `${session.started_at ?? session.startedAt ?? ""}`;
  const projectPath = textValue(session.project ?? session.projectPath);
  const repositoryPath = textValue(session.repository ?? session.repositoryPath);
  const repositoryUrl = textValue(session.repository_url ?? session.repositoryUrl);
  const logicalProjectId = textValue(session.logical_project_id ?? session.logicalProjectId);
  const logicalProjectName = textValue(session.logical_project_name ?? session.logicalProjectName);
  const isRunEverything = session.is_run_everything ?? session.isRunEverything;
  return {
    id: `${session.id ?? `session-${index}`}`,
    title: `${session.title || session.id || `Session ${index + 1}`}`,
    project: projectPath,
    projectPath,
    repository: repositoryPath || undefined,
    repositoryPath: repositoryPath || undefined,
    repositoryUrl: repositoryUrl || undefined,
    logicalProjectId: logicalProjectId || undefined,
    logicalProjectName: logicalProjectName || undefined,
    path: session.path as string | undefined,
    agent: friendlyAgent(session.agent),
    startedAt,
    updatedAt,
    time: updatedAt,
    startedLabel: compactDateTime(startedAt),
    updatedLabel: compactDateTime(updatedAt),
    updatedDetailLabel: compactDateTime(updatedAt, { year: true }),
    messages: Number(session.message_count ?? session.messages ?? 0),
    firstUserMessage: textValue(session.first_user_message ?? session.firstUserMessage) || undefined,
    lastUserMessage: textValue(session.last_user_message ?? session.lastUserMessage) || undefined,
    lastAssistantMessage: textValue(session.last_assistant_message ?? session.lastAssistantMessage) || undefined,
    turnCount: optionalCount(session.turn_count ?? session.turnCount),
    model: `${session.model ?? ""}`,
    mode: textValue(session.mode) || undefined,
    approvalMode: textValue(session.approval_mode ?? session.approvalMode) || undefined,
    isRunEverything: typeof isRunEverything === "boolean" ? isRunEverything : undefined,
    parentSessionId: textValue(session.parent_session_id ?? session.parentSessionId) || undefined,
    searchScore: Number(session.search_score ?? session.searchScore) || undefined,
    searchSnippet: textValue(session.search_snippet ?? session.searchSnippet) || undefined,
    tokenUsage: normalizeTokenUsage(session.token_usage ?? session.tokenUsage),
  };
}

export function isAbsolutePath(value: unknown): boolean {
  return typeof value === "string" && (/^\//.test(value) || /^[A-Za-z]:[\\/]/.test(value));
}

export function sessionWorkspacePath(session: Pick<SessionRecord, "project" | "projectPath">): string {
  const projectPath = textValue(session.projectPath);
  if (isAbsolutePath(projectPath)) return projectPath;
  const project = textValue(session.project);
  return isAbsolutePath(project) ? project : "";
}

export function sessionRepositoryPath(session: Pick<SessionRecord, "repository" | "repositoryPath">): string {
  const repositoryPath = textValue(session.repositoryPath);
  if (isAbsolutePath(repositoryPath)) return repositoryPath;
  const repository = textValue(session.repository);
  return isAbsolutePath(repository) ? repository : "";
}

export function sessionLaunchPayload(session: Pick<SessionRecord, "id" | "agent" | "title" | "project" | "projectPath" | "path">) {
  const projectPath = sessionWorkspacePath(session);
  return {
    id: `${session.id ?? ""}`,
    agent: `${session.agent ?? ""}`,
    title: session.title ?? null,
    project: projectPath || null,
    path: `${session.path ?? ""}`,
  };
}

export function sessionIdentity(session: Pick<SessionRecord, "agent" | "id" | "path">): string {
  return `${session.agent ?? ""}\u0000${session.id ?? ""}\u0000${session.path ?? ""}`;
}

export function sessionLogicalIdentity(session: Pick<SessionRecord, "agent" | "id">): string {
  return `${session.agent ?? ""}\u0000${session.id ?? ""}`;
}

export function sessionWorkspace(session: Pick<SessionRecord, "project" | "projectPath">): string {
  return sessionWorkspacePath(session) || textValue(session.projectPath) || textValue(session.project) || "Unknown";
}

export function sessionProject(session: Pick<SessionRecord, "project" | "projectPath" | "repository" | "repositoryPath" | "logicalProjectName">): string {
  return textValue(session.logicalProjectName) || basename(sessionRepositoryPath(session) || sessionWorkspace(session));
}

export function sessionProjectGroupKey(session: Pick<SessionRecord, "project" | "projectPath" | "repository" | "repositoryPath" | "logicalProjectId" | "logicalProjectName">): string {
  const projectId = textValue(session.logicalProjectId);
  return projectId
    ? JSON.stringify(["logical-project", projectId, sessionProject(session)])
    : sessionRepositoryPath(session) || sessionWorkspace(session);
}

export function sessionProjectOption(
  session: SessionRecord,
  policy: MissingSessionProjectPolicy,
  sessionProjects: readonly SessionProjectSummary[],
  projects: readonly ProjectSummary[],
): { key: string; label: string; title: string } | null {
  return sessionProjectOptionForPaths({
    key: sessionProjectGroupKey(session),
    label: sessionProject(session),
    title: session.repositoryUrl || sessionWorkspace(session),
    logicalProjectId: textValue(session.logicalProjectId),
    workspacePath: sessionWorkspacePath(session),
    repositoryPath: sessionRepositoryPath(session),
  }, policy, sessionProjects, projects);
}

export function sessionProjectGroupLabel(key: string): string {
  if (key.startsWith('["logical-project",')) {
    try {
      const value = JSON.parse(key);
      if (Array.isArray(value) && typeof value[2] === "string") return value[2];
    } catch {
      // Fall through to a path label for malformed external data.
    }
  }
  return basename(key);
}

export function sessionKind(session: Pick<SessionRecord, "parentSessionId">): SessionKind {
  return session.parentSessionId ? "child" : "main";
}

export function sessionCacheRate(session: Pick<SessionRecord, "tokenUsage">): number | undefined {
  const usage = session.tokenUsage;
  if (!usage || usage.inputTokens <= 0) return undefined;
  return (usage.cachedInputTokens / usage.inputTokens) * 100;
}

export function sessionSnapshot(session: SessionRecord): string {
  return JSON.stringify(session);
}

export function mergeSessionRows(currentRows: SessionRecord[], incomingRows: Array<Record<string, unknown>>): SessionRecord[] {
  const currentByKey = new Map(currentRows.map((row) => [sessionIdentity(row), row]));
  const nextRows = incomingRows.map((row, index) => {
    const nextRow = normalizeSession(row, index);
    const currentRow = currentByKey.get(sessionIdentity(nextRow));
    if (currentRow?.projectPath && !nextRow.projectPath) nextRow.projectPath = currentRow.projectPath;
    if (currentRow?.repositoryPath && !nextRow.repositoryPath) {
      nextRow.repository = currentRow.repository;
      nextRow.repositoryPath = currentRow.repositoryPath;
    }
    return currentRow && sessionSnapshot(currentRow) === sessionSnapshot(nextRow) ? currentRow : nextRow;
  });
  if (nextRows.length !== currentRows.length) return nextRows;
  for (let index = 0; index < nextRows.length; index += 1) {
    if (nextRows[index] !== currentRows[index]) return nextRows;
  }
  return currentRows;
}

export type SessionIdentityRecord = {
  id?: unknown;
  agent?: unknown;
  path?: unknown;
};

export function applySessionDelta(
  currentRows: SessionRecord[],
  incomingRows: Array<Record<string, unknown>>,
  deletedRows: SessionIdentityRecord[] = [],
): SessionRecord[] {
  const deletedKeys = new Set(deletedRows.map((row) => sessionIdentity({
    id: `${row.id ?? ""}`,
    agent: `${row.agent ?? ""}`,
    path: `${row.path ?? ""}`,
  })));
  const incoming = incomingRows.map((row, index) => normalizeSession(row, index));
  const incomingByKey = new Map(incoming.map((row) => [sessionLogicalIdentity(row), row]));
  const emittedKeys = new Set<string>();
  const nextRows = currentRows.flatMap((row) => {
    const identity = sessionIdentity(row);
    const key = sessionLogicalIdentity(row);
    if (deletedKeys.has(identity) || emittedKeys.has(key)) return [];
    emittedKeys.add(key);
    const nextRow = incomingByKey.get(key);
    if (!nextRow) return [row];
    incomingByKey.delete(key);
    if (row.projectPath && !nextRow.projectPath) nextRow.projectPath = row.projectPath;
    if (row.repositoryPath && !nextRow.repositoryPath) {
      nextRow.repository = row.repository;
      nextRow.repositoryPath = row.repositoryPath;
    }
    return [sessionSnapshot(row) === sessionSnapshot(nextRow) ? row : nextRow];
  });
  nextRows.push(...incomingByKey.values());
  if (nextRows.length !== currentRows.length) return nextRows;
  return nextRows.some((row, index) => row !== currentRows[index]) ? nextRows : currentRows;
}

export function sortValue(session: SessionRecord, key: string): string | number {
  if (key === "title") return `${session.title ?? ""}`.toLowerCase();
  if (key === "agent") return `${session.agent ?? ""}`.toLowerCase();
  if (key === "project") return sessionProject(session).toLowerCase();
  if (key === "startedAt") return `${session.startedAt ?? ""}`;
  if (key === "updatedAt") return `${session.updatedAt ?? ""}`;
  if (key === "messages") return Number(session.messages) || 0;
  if (key === "turns") return Number(session.turnCount) || 0;
  if (key === "cacheRate") return sessionCacheRate(session) ?? -1;
  return "";
}

export function compareSessions(a: SessionRecord, b: SessionRecord, sort: SortState): number {
  const left = sortValue(a, sort.key);
  const right = sortValue(b, sort.key);
  const direction = sort.direction === "asc" ? 1 : -1;
  if (typeof left === "number" || typeof right === "number") {
    return ((Number(left) || 0) - (Number(right) || 0)) * direction;
  }
  return `${left}`.localeCompare(`${right}`) * direction;
}

export function sessionTimeMs(value: unknown): number {
    const time = Date.parse(`${value ?? ""}`);
    return Number.isFinite(time) ? time : 0;
}
