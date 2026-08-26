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
  path: string;
  agent: string;
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

export type SessionSkillLinkRecord = {
  session_path: string;
  session_project?: string | null;
  session_id: string;
  agent: string;
  session_title?: string | null;
  session_started_at?: string | null;
  session_updated_at?: string | null;
  session_message_count?: number | null;
  skill_name: string;
  skill_path: string;
  evidence_text: string;
  evidence_time?: string | null;
};

export function normalizeSessionSkillLink(value: unknown): SessionSkillLinkRecord | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const link = value as Record<string, unknown>;
  const sessionPath = typeof link.session_path === "string" ? link.session_path.trim() : "";
  const sessionId = typeof link.session_id === "string" ? link.session_id.trim() : "";
  const agent = typeof link.agent === "string" ? link.agent.trim() : "";
  const skillName = typeof link.skill_name === "string" ? link.skill_name.trim() : "";
  const skillPath = typeof link.skill_path === "string" ? link.skill_path.trim() : "";
  if (!sessionPath || !sessionId || !agent || !skillName || !skillPath || typeof link.evidence_text !== "string") return undefined;
  return {
    session_path: sessionPath,
    session_project: typeof link.session_project === "string" ? link.session_project : null,
    session_id: sessionId,
    agent,
    session_title: typeof link.session_title === "string" ? link.session_title : null,
    session_started_at: typeof link.session_started_at === "string" ? link.session_started_at : null,
    session_updated_at: typeof link.session_updated_at === "string" ? link.session_updated_at : null,
    session_message_count: typeof link.session_message_count === "number" && Number.isSafeInteger(link.session_message_count) && link.session_message_count >= 0
      ? link.session_message_count
      : null,
    skill_name: skillName,
    skill_path: skillPath,
    evidence_text: link.evidence_text,
    evidence_time: typeof link.evidence_time === "string" ? link.evidence_time : null,
  };
}

export type SessionResumeTarget = "auto" | "terminal" | "app";

export type SessionResumeOutcome =
  | { status: "activeWriter"; lockPath: string }
  | { status: "launched"; target: SessionResumeTarget; terminal?: string };

export function normalizeSessionResumeTarget(value: unknown): SessionResumeTarget {
  if (value === "app") return "app";
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
  return typeof value === "string" ? value.trim() : "";
}

function optionalCount(value: unknown): number | undefined {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0 ? value : undefined;
}

function normalizeTokenUsage(value: unknown): SessionTokenUsage | undefined {
  if (!value || typeof value !== "object" || Array.isArray(value)) return undefined;
  const usage = value as Record<string, unknown>;
  const inputTokens = usage.input_tokens;
  const cachedInputTokens = usage.cached_input_tokens;
  const outputTokens = usage.output_tokens;
  const reasoningOutputTokens = usage.reasoning_output_tokens;
  const totalTokens = usage.total_tokens;
  if (
    typeof inputTokens !== "number"
    || typeof cachedInputTokens !== "number"
    || typeof outputTokens !== "number"
    || typeof reasoningOutputTokens !== "number"
    || typeof totalTokens !== "number"
  ) return undefined;
  const values = [inputTokens, cachedInputTokens, outputTokens, reasoningOutputTokens, totalTokens];
  if (values.some((tokenCount) => !Number.isFinite(tokenCount) || tokenCount < 0) || totalTokens <= 0) return undefined;
  return { inputTokens, cachedInputTokens, outputTokens, reasoningOutputTokens, totalTokens };
}

export function normalizeSession(session: Record<string, unknown>): SessionRecord | undefined {
  const id = typeof session.id === "string" ? session.id.trim() : "";
  const agent = typeof session.agent === "string" ? session.agent.trim() : "";
  const path = typeof session.path === "string" ? session.path.trim() : "";
  if (!id || !agent || !path) return undefined;
  const updatedAt = textValue(session.updated_at);
  const startedAt = textValue(session.started_at);
  const projectPath = textValue(session.project);
  const repositoryPath = textValue(session.repository);
  const repositoryUrl = textValue(session.repository_url);
  const logicalProjectId = textValue(session.logical_project_id);
  const logicalProjectName = textValue(session.logical_project_name);
  const isRunEverything = session.is_run_everything;
  return {
    id,
    title: textValue(session.title),
    project: projectPath,
    projectPath,
    repository: repositoryPath || undefined,
    repositoryPath: repositoryPath || undefined,
    repositoryUrl: repositoryUrl || undefined,
    logicalProjectId: logicalProjectId || undefined,
    logicalProjectName: logicalProjectName || undefined,
    path,
    agent: friendlyAgent(agent),
    startedAt,
    updatedAt,
    time: updatedAt,
    startedLabel: compactDateTime(startedAt),
    updatedLabel: compactDateTime(updatedAt),
    updatedDetailLabel: compactDateTime(updatedAt, { year: true }),
    messages: optionalCount(session.message_count),
    firstUserMessage: textValue(session.first_user_message) || undefined,
    lastUserMessage: textValue(session.last_user_message) || undefined,
    lastAssistantMessage: textValue(session.last_assistant_message) || undefined,
    turnCount: optionalCount(session.turn_count),
    model: textValue(session.model),
    mode: textValue(session.mode) || undefined,
    approvalMode: textValue(session.approval_mode) || undefined,
    isRunEverything: typeof isRunEverything === "boolean" ? isRunEverything : undefined,
    parentSessionId: textValue(session.parent_session_id) || undefined,
    searchScore: typeof session.search_score === "number" && Number.isFinite(session.search_score)
      ? session.search_score
      : undefined,
    searchSnippet: textValue(session.search_snippet) || undefined,
    tokenUsage: normalizeTokenUsage(session.token_usage),
  };
}

export function isAbsolutePath(value: unknown): boolean {
  return typeof value === "string" && (/^\//.test(value) || /^[A-Za-z]:[\\/]/.test(value));
}

export function sessionWorkspacePath(session: Pick<SessionRecord, "project" | "projectPath">): string {
  const projectPath = textValue(session.projectPath);
  return isAbsolutePath(projectPath) ? projectPath : "";
}

export function sessionRepositoryPath(session: Pick<SessionRecord, "repository" | "repositoryPath">): string {
  const repositoryPath = textValue(session.repositoryPath);
  return isAbsolutePath(repositoryPath) ? repositoryPath : "";
}

export function sessionLaunchPayload(session: Pick<SessionRecord, "id" | "agent" | "title" | "project" | "projectPath" | "path">) {
  const projectPath = sessionWorkspacePath(session);
  return {
    id: session.id,
    agent: session.agent,
    title: session.title ?? null,
    project: projectPath || null,
    path: session.path,
  };
}

export function sessionIdentity(session: Pick<SessionRecord, "agent" | "id" | "path">): string {
  return `${session.agent}\u0000${session.id}\u0000${session.path}`;
}

export function sessionLogicalIdentity(session: Pick<SessionRecord, "agent" | "id">): string {
  return `${session.agent}\u0000${session.id}`;
}

export function sessionWorkspace(session: Pick<SessionRecord, "project" | "projectPath">): string {
  return sessionWorkspacePath(session);
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
  if (!key || key === "__empty__") return "";
  if (key.startsWith('["logical-project",')) {
    try {
      const value = JSON.parse(key);
      if (Array.isArray(value) && typeof value[2] === "string") return value[2];
    } catch {
    }
    return "";
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
  const nextRows = incomingRows.flatMap((row) => {
    const nextRow = normalizeSession(row);
    if (!nextRow) return [];
    const currentRow = currentByKey.get(sessionIdentity(nextRow));
    return [currentRow && sessionSnapshot(currentRow) === sessionSnapshot(nextRow) ? currentRow : nextRow];
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

export function sessionIdentityRecordKey(record: SessionIdentityRecord): string | undefined {
  const id = textValue(record.id);
  const agent = textValue(record.agent);
  const path = textValue(record.path);
  if (!id || !agent || !path) return undefined;
  return sessionIdentity({ id, agent, path });
}

export function applySessionDelta(
  currentRows: SessionRecord[],
  incomingRows: Array<Record<string, unknown>>,
  deletedRows: SessionIdentityRecord[] = [],
): SessionRecord[] {
  const deletedKeys = new Set(deletedRows.flatMap((row) => {
    const key = sessionIdentityRecordKey(row);
    return key ? [key] : [];
  }));
  const incoming = incomingRows.flatMap((row) => {
    const normalized = normalizeSession(row);
    return normalized ? [normalized] : [];
  });
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
    return [sessionSnapshot(row) === sessionSnapshot(nextRow) ? row : nextRow];
  });
  nextRows.push(...incomingByKey.values());
  if (nextRows.length !== currentRows.length) return nextRows;
  return nextRows.some((row, index) => row !== currentRows[index]) ? nextRows : currentRows;
}

export function sortValue(session: SessionRecord, key: string): string | number {
  if (key === "title") return session.title.toLowerCase();
  if (key === "agent") return session.agent.toLowerCase();
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
