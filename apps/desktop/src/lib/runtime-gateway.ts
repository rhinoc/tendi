import { TauriCommand, invokeCommand, subscribeDaemonEvents, type BundledSkillInstallReport, type BundledSkillStatus, type CliInstallStatus, type DaemonEvent, type UpdateCheckResult } from "./tauri.ts";
import { RuntimeDomainKey, type DomainKey } from "./domain.ts";
import { assertRuntimeEvent } from "./generated/runtime-events.ts";
import { RuntimeEventName } from "./generated/runtime-events.ts";
import type {
  AgentConfigFile,
  AgentKind,
  HookDeleteRequest,
  HookReviewRequest,
  HookSetEnabledRequest,
  HookSourceReadRequest,
  McpSetEnabledRequest,
  ResponseFor,
  SessionResumeRequest,
  SessionScanEvent,
  SessionsSearchRequest,
  SkillFileMutationResponse as GeneratedSkillFileMutationResponse,
  SkillsAddRequest as GeneratedSkillsAddRequest,
  SkillsBackupRestoreRequest as GeneratedSkillsBackupRestoreRequest,
  SkillsDistributeRequest as GeneratedSkillsDistributeRequest,
  SkillsDeleteManyRequest as GeneratedSkillsDeleteManyRequest,
  SkillsRemoveLocationsRequest as GeneratedSkillsRemoveLocationsRequest,
  SkillsSetRequest as GeneratedSkillsSetRequest,
  SkillsUpdateManyRequest as GeneratedSkillsUpdateManyRequest,
  SkillsWrapRequest as GeneratedSkillsWrapRequest,
  SkillUpdatePlan,
  SkillsDeleteManyResponse as GeneratedSkillsDeleteManyResponse,
  SkillsSetResponse as GeneratedSkillsSetResponse,
  SkillsUpdateManyResponse as GeneratedSkillsUpdateManyResponse,
  SkillsWrapResponse as GeneratedSkillsWrapResponse,
  SkillsUpdatesEvent,
  AnalyticsRevisionEvent,
  SkillVisibility as GeneratedSkillVisibility,
} from "./generated/runtime-types.ts";
import type { RawDomainRow } from "../controllers/controller-types.ts";
import { toRawDomainRow, toRawDomainRows } from "./raw-domain.ts";
import type { SkillAddPlan, SkillInstallResult } from "./skills.ts";
import { SkillChangeCommand } from "./skills.ts";
import type { SkillUpdateReport } from "./skill-updates.ts";
import { isRuntimeAgentKind, runtimeAgentKind } from "./agents.ts";
import { normalizeSkillFileEntries, type SkillFileEntry } from "./file-tree.ts";
import { readRuleFile, type RuleFileResult } from "./rule-file.ts";
import { SessionResumeOutcomeStatus, SessionResumeTarget, type SessionIdentityRecord, type SessionRecord } from "./sessions.ts";
import { normalizeConfigProfiles, normalizeSettings, type SettingsPayload } from "./settings.ts";
import type { ProjectSummary, SessionProjectSummary } from "./projects.ts";
import type { OverviewAnalytics } from "./analytics.ts";
import {
  normalizeTranscriptLocatorPage,
  normalizeTranscriptPage,
  normalizeTranscriptSearchResult,
  type TranscriptLocatorPage,
  type TranscriptPage,
  type TranscriptSearchResult,
  type TranscriptSearchScopes,
} from "./transcript.ts";

export enum SkillScope {
  Global = "global",
  Project = "project",
}

export enum SkillDistributionMode {
  Move = "move",
  Symlink = "symlink",
  Copy = "copy",
}

export enum SessionScanPhase {
  Recent = "recent",
  Backfill = "backfill",
  Watch = "watch",
  Error = "error",
}

export enum SkillUpdateEventStatus {
  Completed = "completed",
  Failed = "failed",
}

export enum SkillUpdateCheckState {
  Started = "started",
  AlreadyRunning = "already-running",
}

export type MarketplaceSource = {
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

export type MarketplaceSearchResponse = {
  items: MarketplaceSource[];
  warnings: string[];
};

export type SkillAddRequest = {
  source: string;
  target: string;
  scope: SkillScope;
  skills: string[];
  copy: boolean;
  overwrite: boolean;
  visibility: string;
  dryRun: boolean;
  previewId?: string;
};

export type SkillAddResponse = {
  plan?: SkillAddPlan;
  previewId?: string;
};

export type SkillPreviewReadResponse = {
  name: string;
  relativePath: string;
  content: string;
};

export type SkillRefreshResponse = {
  skills: RawDomainRow[];
  updateCheck?: string;
  updates?: SkillUpdateReport[];
};

export type SkillUpdateCheckResponse = { updateCheck: string };

export type SkillChangeResponse = {
  summary?: string;
  applied?: boolean;
  plan?: SkillUpdatePlan;
  previewId?: string;
  canApply?: boolean;
  refreshRequired?: boolean;
  skills?: RawDomainRow[];
  updated?: RawDomainRow[];
};

export type SkillChangeArgs =
  | (Omit<GeneratedSkillsSetRequest, "visibility"> & { visibility: string })
  | GeneratedSkillsUpdateManyRequest
  | GeneratedSkillsDeleteManyRequest
  | GeneratedSkillsWrapRequest;

export type { SkillChangeCommand } from "./skills.ts";

export type CatalogMutationError = { error: string };
export type HookMutationResponse = ResponseFor<"hook_delete"> | CatalogMutationError;
export type McpMutationResponse = ResponseFor<"mcp_set_enabled"> | CatalogMutationError;
export type RuleDeleteResponse = ResponseFor<"rule_file_delete_many"> | CatalogMutationError;
export type CatalogMutationResponse = HookMutationResponse | McpMutationResponse | RuleDeleteResponse;

export type RuleFileResponse = RuleFileResult;

export type HookSourceResponse = {
  content?: string;
  source_line?: number | null;
  path?: string;
};

export enum AgentConfigFormat {
  Json = "json",
  Toml = "toml",
}

export type AgentConfigFileResponse = {
  agent: string;
  label: string;
  path: string;
  format: AgentConfigFormat;
  exists: boolean;
  updatedAt?: string;
  profile?: string;
};

export type AgentConfigContentResponse = {
  path: string;
  content: string;
  sha256: string;
  exists: boolean;
  updatedAt?: string;
};

export type AgentConfigWriteResponse = {
  path: string;
  sha256: string;
  exists: boolean;
  updatedAt?: string;
  content?: string;
};

export type AgentConfigDeleteResponse = {
  configs?: AgentConfigFileResponse[];
  deleted?: string[];
  configProfiles?: Record<string, string>;
};

export type ConfigProfileResponse = { configProfiles: Record<string, string> };

export type SkillFileReadResponse = {
  content: string;
  sha256: string;
};

export type SkillFileMutationResponse = {
  sha256?: string;
  files?: SkillFileEntry[];
  skills?: RawDomainRow[];
};

export type SkillDistributionResponse = {
  updated?: RawDomainRow[];
  skills?: RawDomainRow[];
  deleted?: string[];
};

export type TerminalAppResponse = {
  id: string;
  label: string;
  available?: boolean;
};

export type ProjectScanScopeResponse = {
  path: string;
  excluded?: boolean;
  enabled: boolean;
  lastScannedAt?: string | null;
  projectCount: number;
};

export type BackupCategorySelection = { enabled: boolean; excluded: string[] };
export type BackupContents = {
  skills: BackupCategorySelection;
  mcp: BackupCategorySelection;
  rules: BackupCategorySelection;
  hooks: BackupCategorySelection;
};
export type BackupConfigResponse = {
  remoteUrl: string;
  checkoutPath: string;
  contents: BackupContents;
};
export type BackupCatalogResponse = {
  skills: Array<{ id: string; label: string; detail: string }>;
  mcp: Array<{ id: string; label: string; detail: string }>;
  rules: Array<{ id: string; label: string; detail: string }>;
  hooks: Array<{ id: string; label: string; detail: string }>;
};
export type BackupStatusResponse = {
  config: BackupConfigResponse | null;
  statuses: Array<{ skillPath: string; state: string; reason?: string | null }>;
  versions: Array<{ id: string; createdAt: number; summary: string }>;
  catalog: BackupCatalogResponse;
};
export type BackupTargetResponse = { id: string; displayName: string; supportsGlobal: boolean };
export type BackupRestoreOperation = { id: string; name: string; category: string; target: string; status: string; message?: string | null };
export type BackupRestoreResponse = {
  revision?: string;
  targetRoot?: string;
  operations?: BackupRestoreOperation[];
  updated?: RawDomainRow[];
  skills?: RawDomainRow[];
};

export type PromptSaveRequest = {
  id: string | null;
  title: string;
  tags: string[];
  body: string;
};

export type SessionSnapshotDto = Omit<ResponseFor<"sessions_snapshot">, "payload"> & { payload: RawDomainRow[] };

export type SessionScanStartDto = ResponseFor<"sessions_scan_start">;

export type RuntimeSessionScanEvent = Omit<SessionScanEvent, "upserts" | "deleted"> & {
  upserts: RawDomainRow[];
  deleted: SessionIdentityRecord[];
};

export type RuntimeSkillUpdateEvent = Omit<SkillsUpdatesEvent, "skills" | "updates" | "status"> & {
  status: SkillUpdateEventStatus;
  skills: RawDomainRow[] | null;
  updates: SkillUpdateReport[];
};

export type RuntimeAnalyticsRevisionEvent = AnalyticsRevisionEvent;

export type RuntimeEvent =
  | (DaemonEvent<RuntimeSessionScanEvent> & { event: typeof RuntimeEventName.SessionsScan })
  | (DaemonEvent<RuntimeSkillUpdateEvent> & { event: typeof RuntimeEventName.SkillsUpdates })
  | (DaemonEvent<RuntimeAnalyticsRevisionEvent> & { event: typeof RuntimeEventName.AnalyticsRevision });

type ListableDomain = Exclude<DomainKey, RuntimeDomainKey.Sessions>;

const domainCommands: Record<ListableDomain, TauriCommand> = {
  skills: TauriCommand.SkillsList,
  prompts: TauriCommand.PromptsList,
  rules: TauriCommand.RulesList,
  hooks: TauriCommand.HooksList,
  mcp: TauriCommand.McpList,
};

function recordRows(value: unknown, label: string): RawDomainRow[] {
  return toRawDomainRows(value, label);
}

function normalizeAgentConfigFile(value: AgentConfigFile): AgentConfigFileResponse {
  return {
    ...value,
    format: value.format as AgentConfigFormat,
    updatedAt: value.updatedAt ?? undefined,
    profile: value.profile ?? undefined,
  };
}

function runtimeScope(value: SkillScope): "global" | "project" {
  return value === SkillScope.Global ? "global" : "project";
}

function runtimeDistributionMode(value: SkillDistributionMode): "move" | "symlink" | "copy" {
  return value;
}

function runtimeVisibility(value: string): GeneratedSkillVisibility {
  if (value === "auto" || value === "manual" || value === "off" || value === "mixed") return value;
  throw new Error("Invalid skill visibility");
}

function runtimeSkillAddRequest(request: SkillAddRequest): GeneratedSkillsAddRequest {
  return {
    source: request.source,
    target: request.target,
    scope: runtimeScope(request.scope),
    skills: [...request.skills],
    copy: request.copy,
    overwrite: request.overwrite,
    visibility: runtimeVisibility(request.visibility),
    dryRun: request.dryRun,
    previewId: request.previewId,
  };
}

type HookRequestInput<T extends { agent: AgentKind }> = Omit<T, "agent"> & { agent: string };

function runtimeHookRequest<T extends { agent: AgentKind }>(args: HookRequestInput<T>): T {
  return { ...args, agent: runtimeAgentKind(args.agent) } as T;
}

type McpRequestInput = Omit<McpSetEnabledRequest, "agent"> & { agent: string };

function runtimeMcpRequest(args: McpRequestInput): McpSetEnabledRequest {
  return { ...args, agent: runtimeAgentKind(args.agent), serverPath: [...args.serverPath] };
}

function isNonNegativeSafeInteger(value: unknown): value is number {
  return typeof value === "number" && Number.isSafeInteger(value) && value >= 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function parseSessionIdentity(value: unknown): SessionIdentityRecord | null {
  if (!isRecord(value)) return null;
  const id = typeof value.id === "string" ? value.id.trim() : "";
  const agent = typeof value.agent === "string" ? value.agent.trim() : "";
  const path = typeof value.path === "string" ? value.path.trim() : "";
  return id && agent && path ? { id, agent, path } : null;
}

function parseSessionScanEvent(value: SessionScanEvent): RuntimeSessionScanEvent | null {
  const upserts = recordRows(value.upserts, "session scan upserts");
  const deleted = value.deleted.flatMap((item) => {
    const identity = parseSessionIdentity(item);
    return identity ? [identity] : [];
  });
  if (deleted.length !== value.deleted.length) return null;
  return {
    generation: value.generation,
    phase: value.phase,
    upserts,
    deleted,
    scanned: value.scanned,
    complete: value.complete,
    error: value.error,
  };
}

function parseSkillUpdateEvent(value: SkillsUpdatesEvent): RuntimeSkillUpdateEvent | null {
  if (value.status !== SkillUpdateEventStatus.Completed && value.status !== SkillUpdateEventStatus.Failed) return null;
  return {
    status: value.status as SkillUpdateEventStatus,
    skills: value.skills === null ? null : recordRows(value.skills, "skills update event"),
    updates: value.updates.map((item) => ({
      id: item.id,
      name: item.name,
      status: item.status,
    })),
    error: value.error,
  };
}

function parseAnalyticsRevisionEvent(value: AnalyticsRevisionEvent): RuntimeAnalyticsRevisionEvent {
  return value;
}

export async function subscribeRuntimeEvents(handler: (event: RuntimeEvent) => void): Promise<() => void> {
  return subscribeDaemonEvents((event) => {
    try {
      assertRuntimeEvent(event);
    } catch {
      return;
    }
    if (event.event === RuntimeEventName.SessionsScan) {
      const payload = parseSessionScanEvent(event.payload);
      if (payload) handler({ ...event, event: RuntimeEventName.SessionsScan, payload });
    } else if (event.event === RuntimeEventName.SkillsUpdates) {
      const payload = parseSkillUpdateEvent(event.payload);
      if (payload) handler({ ...event, event: RuntimeEventName.SkillsUpdates, payload });
    } else if (event.event === RuntimeEventName.AnalyticsRevision) {
      const payload = parseAnalyticsRevisionEvent(event.payload);
      if (payload) handler({ ...event, event: RuntimeEventName.AnalyticsRevision, payload });
    }
  });
}

export async function invokeDomainList(domain: ListableDomain): Promise<RawDomainRow[]> {
  return recordRows(await invokeCommand(domainCommands[domain]), domain);
}

export async function invokeAgentsList(): Promise<RawDomainRow[]> {
  return recordRows(await invokeCommand(TauriCommand.AgentsList), RuntimeDomainKey.Agents);
}

export async function invokeSkillsList(): Promise<RawDomainRow[]> {
  return invokeDomainList(RuntimeDomainKey.Skills);
}

export async function refreshSkills(): Promise<SkillRefreshResponse> {
  const response = await invokeCommand(TauriCommand.SkillsRefresh);
  return { ...response, skills: recordRows(response.skills, "skills refresh") };
}

export async function requestSkillUpdates(): Promise<SkillUpdateCheckResponse> {
  const response = await invokeCommand(TauriCommand.SkillsUpdates, { check: true }) as SkillUpdateCheckResponse;
  if (response.updateCheck !== SkillUpdateCheckState.Started && response.updateCheck !== SkillUpdateCheckState.AlreadyRunning) {
    throw new Error("Invalid skill update check response");
  }
  return response;
}

export async function invokeSessionSnapshot(): Promise<SessionSnapshotDto> {
  const snapshot = await invokeCommand(TauriCommand.SessionsSnapshot);
  const payload = recordRows(snapshot.payload, "sessions snapshot payload");
  return {
    scopeKey: snapshot.scopeKey,
    domain: RuntimeDomainKey.Sessions,
    revision: snapshot.revision,
    schemaVersion: snapshot.schemaVersion,
    snapshotId: snapshot.snapshotId,
    payload,
  };
}

export async function invokeSessionScanStart(): Promise<SessionScanStartDto> {
  return invokeCommand(TauriCommand.SessionsScanStart);
}

type RuntimeSessionCandidate = NonNullable<SessionsSearchRequest["candidates"]>[number];

function runtimeSessionCandidate(value: Pick<SessionRecord, "id" | "agent" | "path">): RuntimeSessionCandidate {
  const id = typeof value.id === "string" ? value.id.trim() : "";
  const agent = typeof value.agent === "string" ? value.agent.trim() : "";
  const path = typeof value.path === "string" ? value.path.trim() : "";
  if (!id || !path || !isRuntimeAgentKind(agent)) {
    throw new Error("Invalid session search candidate");
  }
  return { id, agent: agent as AgentKind, path };
}

export async function invokeSessionSearch(
  query: string,
  candidates: readonly Pick<SessionRecord, "id" | "agent" | "path">[],
): Promise<RawDomainRow[]> {
  const request = { query, candidates: candidates.map(runtimeSessionCandidate) };
  return recordRows(await invokeCommand(TauriCommand.SessionsSearch, request), "session search");
}

export async function invokeSessionSkillLinks(sessionId: string, agent: string): Promise<RawDomainRow[]> {
  return recordRows(await invokeCommand(TauriCommand.SessionSkillLinks, { sessionId, agent: runtimeAgentKind(agent) }), "session skill links");
}

export async function invokeSkillSessionLinks(skillName: string): Promise<RawDomainRow[]> {
  return recordRows(await invokeCommand(TauriCommand.SkillSessionLinks, { skillName }), "skill session links");
}

export async function invokeProjectList(): Promise<ProjectSummary[] | null> {
  try {
    return await invokeCommand(TauriCommand.ProjectsList);
  } catch {
    return null;
  }
}

export async function invokeSessionProjectList(): Promise<SessionProjectSummary[] | null> {
  try {
    return await invokeCommand(TauriCommand.SessionProjectsList);
  } catch {
    return null;
  }
}

export async function invokeAnalyticsRevision(): Promise<number> {
  const revision = await invokeCommand(TauriCommand.AnalyticsRevision);
  if (!Number.isSafeInteger(revision) || revision < 0) throw new Error("Invalid analytics revision response");
  return revision;
}

export async function invokeAnalyticsOverview(args: {
  agent: string | null;
  days: number;
  rankDays: number;
  refreshTranscripts: boolean;
}): Promise<OverviewAnalytics | null> {
  try {
    const response = await invokeCommand(TauriCommand.AnalyticsOverview, args);
    return {
      ...response,
      coverage: {
        ...response.coverage,
        first: response.coverage.first ?? undefined,
        last: response.coverage.last ?? undefined,
      },
    };
  } catch {
    return null;
  }
}

export type SkillIndexStatusResponse = {
  total?: number;
  indexed?: number;
  pending?: number;
  failed?: number;
  running?: boolean;
  last_indexed_at?: string | null;
};

export async function runSkillIndex(force: boolean): Promise<SkillIndexStatusResponse> {
  return invokeCommand(TauriCommand.SessionSkillIndexRun, { force });
}

export async function readSkillIndexStatus(): Promise<SkillIndexStatusResponse | null> {
  try {
    return await invokeCommand(TauriCommand.SessionSkillIndexStatus);
  } catch {
    return null;
  }
}

export type SkillTargetResponse = { id: string; displayName: string; supportsGlobal: boolean; globalPath?: string };

export async function readSkillTargets(): Promise<SkillTargetResponse[] | null> {
  try {
    const targets = await invokeCommand(TauriCommand.SkillsTargets);
    return targets.map((target) => ({ ...target, globalPath: target.globalPath ?? undefined }));
  } catch {
    return null;
  }
}

export async function syncSkillBackup(): Promise<void> {
  try {
    await invokeCommand(TauriCommand.SkillsBackupSync);
  } catch {
    // Background backup synchronization is best effort.
  }
}

export async function readBundledSkillStatus(): Promise<BundledSkillStatus | null> {
  try {
    return await invokeCommand(TauriCommand.BundledSkillStatus) as BundledSkillStatus;
  } catch {
    return null;
  }
}

export async function dismissBundledSkillPrompt(): Promise<void> {
  await invokeCommand(TauriCommand.BundledSkillPromptDismiss);
}

export async function installCli(): Promise<CliInstallStatus> {
  return invokeCommand(TauriCommand.CliInstall) as Promise<CliInstallStatus>;
}

export async function installBundledSkill(): Promise<BundledSkillInstallReport> {
  const response = await invokeCommand(TauriCommand.BundledSkillInstall);
  return {
    ...response,
    updated: response.updated === undefined ? undefined : recordRows(response.updated, "bundled skill install"),
  } as BundledSkillInstallReport;
}

export async function checkForUpdates(): Promise<UpdateCheckResult> {
  const response = await invokeCommand(TauriCommand.CheckForUpdates);
  return { ...response, status: response.status as UpdateCheckResult["status"] };
}

export async function installUpdate(): Promise<UpdateCheckResult> {
  const response = await invokeCommand(TauriCommand.InstallUpdate);
  return { ...response, status: response.status as UpdateCheckResult["status"] };
}

export async function readSettings(): Promise<SettingsPayload> {
  const response = await invokeCommand(TauriCommand.SettingsGet);
  return {
    ...normalizeSettings(response as Partial<SettingsPayload>),
    configProfiles: normalizeConfigProfiles(response.configProfiles),
  };
}

export async function saveSettings(settings: SettingsPayload): Promise<SettingsPayload | null> {
  try {
    const response = await invokeCommand(TauriCommand.SettingsSave, settings);
    return {
      ...normalizeSettings(response as Partial<SettingsPayload>),
      configProfiles: normalizeConfigProfiles(response.configProfiles),
    };
  } catch {
    return null;
  }
}

export async function readTerminalApps(): Promise<TerminalAppResponse[] | null> {
  try {
    return await invokeCommand(TauriCommand.TerminalAppsList);
  } catch {
    return null;
  }
}

export async function readCliStatus(): Promise<CliInstallStatus | null> {
  try {
    return await invokeCommand(TauriCommand.CliStatus) as CliInstallStatus;
  } catch {
    return null;
  }
}

export async function removeCli(): Promise<CliInstallStatus> {
  return invokeCommand(TauriCommand.CliRemove) as Promise<CliInstallStatus>;
}

export async function readProjectScanScopes(): Promise<ProjectScanScopeResponse[] | null> {
  try {
    return await invokeCommand(TauriCommand.ProjectScanScopesList);
  } catch {
    return null;
  }
}

export async function saveProjectScanScopes(paths: readonly string[]): Promise<ProjectScanScopeResponse[] | null> {
  try {
    return await invokeCommand(TauriCommand.ProjectScanScopesSave, { paths: [...paths] });
  } catch {
    return null;
  }
}

export async function scanProjects(): Promise<{ projects?: ProjectSummary[] } | null> {
  try {
    return await invokeCommand(TauriCommand.ProjectsScan) as { projects?: ProjectSummary[] };
  } catch {
    return null;
  }
}

export async function testTerminalApp(terminal: string): Promise<boolean> {
  try {
    return Boolean(await invokeCommand(TauriCommand.TerminalAppTest, { terminal }));
  } catch {
    return false;
  }
}

export async function testEditorApp(editor: string): Promise<boolean> {
  try {
    return Boolean(await invokeCommand(TauriCommand.EditorAppTest, { editor }));
  } catch {
    return false;
  }
}

export async function exportLogs(): Promise<string> {
  return invokeCommand(TauriCommand.LogsExport) as Promise<string>;
}

export async function watchAgentConfig(path: string): Promise<void> {
  await invokeCommand(TauriCommand.AgentConfigWatch, { path });
}

export async function readAgentConfigs(): Promise<AgentConfigFileResponse[]> {
  return (await invokeCommand(TauriCommand.AgentConfigsList)).map(normalizeAgentConfigFile);
}

export async function readAgentConfig(path: string): Promise<AgentConfigContentResponse> {
  const response = await invokeCommand(TauriCommand.AgentConfigRead, { path });
  return { ...response, updatedAt: response.updatedAt ?? undefined };
}

export async function saveAgentConfig(args: {
  path: string;
  expectedSha256: string;
  content: string;
}): Promise<AgentConfigWriteResponse> {
  const response = await invokeCommand(TauriCommand.AgentConfigSave, args);
  return { ...response, updatedAt: response.updatedAt ?? undefined };
}

export async function deleteAgentConfigs(paths: readonly string[]): Promise<AgentConfigDeleteResponse> {
  const response = await invokeCommand(TauriCommand.AgentConfigsDeleteMany, { paths: [...paths] });
  return {
    ...response,
    configs: response.configs?.map(normalizeAgentConfigFile),
  };
}

export async function createConfigProfile(args: { agent: string; name: string; content: string }): Promise<AgentConfigFileResponse> {
  return normalizeAgentConfigFile(await invokeCommand(TauriCommand.ConfigProfileCreate, {
    ...args,
    agent: runtimeAgentKind(args.agent),
  }));
}

export async function setConfigProfile(agent: string, profile: string | null): Promise<ConfigProfileResponse> {
  const response = await invokeCommand(TauriCommand.ConfigProfileSet, { agent: runtimeAgentKind(agent), profile });
  return { configProfiles: response.configProfiles };
}

export async function readSkillFiles(args: { skillId: string; skillPath?: string }): Promise<SkillFileEntry[]> {
  return normalizeSkillFileEntries(await invokeCommand(TauriCommand.SkillFiles, args));
}

export async function readSkillFile(args: {
  skillId: string;
  relativePath: string;
  skillPath?: string;
}): Promise<SkillFileReadResponse> {
  const response = await invokeCommand(TauriCommand.SkillFileRead, args);
  if (typeof response.content !== "string" || typeof response.sha256 !== "string") {
    throw new Error("Invalid skill file response");
  }
  return response;
}

function parseSkillFileMutation(response: GeneratedSkillFileMutationResponse): SkillFileMutationResponse {
  if (response.sha256 !== undefined && typeof response.sha256 !== "string") throw new Error("Invalid skill file mutation response");
  const files = response.files === undefined ? undefined : normalizeSkillFileEntries(response.files);
  const skills = response.skills === undefined ? undefined : recordRows(response.skills, "skill file mutation");
  return { sha256: response.sha256, files, skills };
}

function parseSkillFileSave(response: ResponseFor<"skill_file_save">): SkillFileMutationResponse {
  const skills = response.skills === undefined ? undefined : recordRows(response.skills, "skill file save");
  return { sha256: response.sha256, skills };
}

export async function saveSkillFile(args: {
  skillId: string;
  relativePath: string;
  expectedSha256: string;
  content: string;
  skillPath?: string;
}): Promise<SkillFileMutationResponse> {
  return parseSkillFileSave(await invokeCommand(TauriCommand.SkillFileSave, args));
}

export async function createSkillFile(args: { skillId: string; relativePath: string; skillPath?: string }): Promise<SkillFileMutationResponse> {
  return parseSkillFileMutation(await invokeCommand(TauriCommand.SkillFileCreate, args));
}

export async function createSkillFolder(args: { skillId: string; relativePath: string; skillPath?: string }): Promise<SkillFileMutationResponse> {
  return parseSkillFileMutation(await invokeCommand(TauriCommand.SkillFolderCreate, args));
}

export async function renameSkillPath(args: {
  skillId: string;
  fromRelativePath: string;
  toRelativePath: string;
  skillPath?: string;
}): Promise<SkillFileMutationResponse> {
  return parseSkillFileMutation(await invokeCommand(TauriCommand.SkillPathRename, args));
}

export async function deleteSkillPath(args: { skillId: string; relativePath: string; skillPath?: string }): Promise<SkillFileMutationResponse> {
  return parseSkillFileMutation(await invokeCommand(TauriCommand.SkillPathDelete, args));
}

export async function distributeSkills(args: {
  sourcePaths: readonly string[];
  targets: readonly string[];
  scope: SkillScope;
  mode: SkillDistributionMode;
  dryRun: boolean;
}): Promise<SkillDistributionResponse> {
  const request: GeneratedSkillsDistributeRequest = {
    sourcePaths: [...args.sourcePaths],
    targets: [...args.targets],
    scope: runtimeScope(args.scope),
    mode: runtimeDistributionMode(args.mode),
    dryRun: args.dryRun,
  };
  const response = await invokeCommand(TauriCommand.SkillsDistribute, request);
  return {
    ...response,
    updated: response.updated === undefined ? undefined : recordRows(response.updated, "skills distribute"),
  } as SkillDistributionResponse;
}

export async function removeSkillLocations(args: {
  names: readonly string[];
  targets: readonly string[];
  scope: SkillScope;
}): Promise<SkillDistributionResponse> {
  const request: GeneratedSkillsRemoveLocationsRequest = {
    skillIds: [...args.names],
    targets: [...args.targets],
    scope: runtimeScope(args.scope),
  };
  const response = await invokeCommand(TauriCommand.SkillsRemoveLocations, request);
  return {
    ...response,
    updated: recordRows(response.updated, "skills remove locations"),
  } as SkillDistributionResponse;
}

export async function readSkillBackup(): Promise<BackupStatusResponse> {
  return invokeCommand(TauriCommand.SkillsBackupStatus) as Promise<BackupStatusResponse>;
}

export async function configureSkillBackup(args: {
  repository: string;
  checkoutPath: string;
  contents: BackupContents;
}): Promise<BackupConfigResponse> {
  return invokeCommand(TauriCommand.SkillsBackupConfigure, args) as Promise<BackupConfigResponse>;
}

export async function runSkillBackup(): Promise<void> {
  await invokeCommand(TauriCommand.SkillsBackupNow);
}

export async function disconnectSkillBackup(): Promise<void> {
  await invokeCommand(TauriCommand.SkillsBackupDisconnect);
}

export async function restoreSkillBackup(args: {
  revision: string;
  skillIds: readonly string[];
  target: string;
  scope: SkillScope;
  dryRun?: boolean;
  confirmed?: boolean;
  resolutions?: Array<{ id: string; action: string }>;
}): Promise<BackupRestoreResponse> {
  const request: GeneratedSkillsBackupRestoreRequest = {
    revision: args.revision,
    skillIds: [...args.skillIds],
    target: args.target,
    scope: runtimeScope(args.scope),
    dryRun: args.dryRun,
    confirmed: args.confirmed,
    resolutions: args.resolutions ? [...args.resolutions] : undefined,
  };
  const response = await invokeCommand(TauriCommand.SkillsBackupRestore, request);
  return {
    ...response,
    updated: response.updated === undefined ? undefined : recordRows(response.updated, "skills backup restore"),
  } as BackupRestoreResponse;
}

export async function loadSessionTranscript(
  session: Pick<SessionRecord, "path" | "agent">,
  cursor?: string,
  knownSourceVersion?: string,
): Promise<TranscriptPage> {
  const value = await invokeCommand(TauriCommand.SessionTranscript, {
    path: session.path,
    agent: runtimeAgentKind(session.agent),
    cursor,
    limit: 160,
    knownSourceVersion,
  });
  return normalizeTranscriptPage(value);
}

export async function loadSessionTranscriptLocator(session: Pick<SessionRecord, "path" | "agent">): Promise<TranscriptLocatorPage> {
  const value = await invokeCommand(TauriCommand.SessionTranscriptLocator, {
    path: session.path,
    agent: runtimeAgentKind(session.agent),
  });
  return normalizeTranscriptLocatorPage(value);
}

export async function searchSessionTranscript(
  session: Pick<SessionRecord, "path" | "agent">,
  query: string,
  scopes: TranscriptSearchScopes,
): Promise<TranscriptSearchResult> {
  const value = await invokeCommand(TauriCommand.SessionTranscriptSearch, {
    path: session.path,
    agent: runtimeAgentKind(session.agent),
    query,
    scopes,
  });
  return normalizeTranscriptSearchResult(value);
}

export async function inferSessionResumeTarget(session: SessionResumeRequest): Promise<Exclude<SessionResumeTarget, SessionResumeTarget.Auto>> {
  const target = await invokeCommand(TauriCommand.SessionResumeTarget, { session });
  if (target === "app") return SessionResumeTarget.App;
  if (target === "terminal") return SessionResumeTarget.Terminal;
  throw new Error("Invalid session resume target response");
}

export async function openUrl(url: string): Promise<void> {
  await invokeCommand(TauriCommand.OpenUrl, { url });
}

export type SessionResumeInTerminalResponse =
  | { status: typeof SessionResumeOutcomeStatus.ActiveWriter; lockPath: string }
  | { status: typeof SessionResumeOutcomeStatus.Launched; agent: string; terminal: string; commandLine: string };

export async function resumeSessionInTerminal(session: SessionResumeRequest): Promise<SessionResumeInTerminalResponse> {
  const response = await invokeCommand(TauriCommand.SessionResumeInTerminal, { session });
  if (response.status === "activeWriter") {
    if (!response.lockPath) throw new Error("Invalid active writer response");
    return { status: SessionResumeOutcomeStatus.ActiveWriter, lockPath: response.lockPath };
  }
  if (!response.agent || !response.terminal || !response.commandLine) throw new Error("Invalid terminal launch response");
  return {
    status: SessionResumeOutcomeStatus.Launched,
    agent: response.agent,
    terminal: response.terminal,
    commandLine: response.commandLine,
  };
}

export async function deleteRules(paths: readonly string[]): Promise<ResponseFor<"rule_file_delete_many">> {
  return invokeCommand(TauriCommand.RuleFileDeleteMany, { paths: [...paths] });
}

export async function readRule(path: string): Promise<RuleFileResponse> {
  return readRuleFile(() => invokeCommand(TauriCommand.RuleFileRead, { path }) as Promise<RuleFileResponse>);
}

export async function saveRule(args: {
  path: string;
  expectedSha256: string;
  content: string;
}): Promise<RuleFileResponse | null> {
  try {
    const response = await invokeCommand(TauriCommand.RuleFileSave, args);
    if (typeof response.sha256 !== "string") return null;
    return { content: args.content, sha256: response.sha256 };
  } catch {
    return null;
  }
}

export async function savePrompt(request: PromptSaveRequest): Promise<RawDomainRow | null> {
  try {
    return toRawDomainRow(await invokeCommand(TauriCommand.PromptSave, request), "prompt save");
  } catch {
    return null;
  }
}

export async function deletePrompts(ids: readonly string[]): Promise<boolean> {
  try {
    const result = await invokeCommand(TauriCommand.PromptsDeleteMany, { ids: [...ids] });
    return result !== null && result !== undefined;
  } catch {
    return false;
  }
}

export async function readHookSource(args: HookRequestInput<HookSourceReadRequest>): Promise<HookSourceResponse> {
  return invokeCommand(TauriCommand.HookSourceRead, runtimeHookRequest(args));
}

export async function deleteHook(args: HookRequestInput<HookDeleteRequest>): Promise<ResponseFor<"hook_delete">> {
  return invokeCommand(TauriCommand.HookDelete, runtimeHookRequest(args));
}

export async function deleteHooks(requests: readonly HookRequestInput<HookDeleteRequest>[]): Promise<ResponseFor<"hook_delete_many">> {
  return invokeCommand(TauriCommand.HookDeleteMany, { requests: requests.map(runtimeHookRequest) });
}

export async function setHookEnabled(args: HookRequestInput<HookSetEnabledRequest>): Promise<ResponseFor<"hook_set_enabled">> {
  return invokeCommand(TauriCommand.HookSetEnabled, runtimeHookRequest(args));
}

export async function setHooksEnabled(requests: readonly HookRequestInput<HookSetEnabledRequest>[]): Promise<ResponseFor<"hook_set_enabled_many">> {
  return invokeCommand(TauriCommand.HookSetEnabledMany, { requests: requests.map(runtimeHookRequest) });
}

export async function reviewHook(args: HookRequestInput<HookReviewRequest>): Promise<ResponseFor<"hook_review">> {
  return invokeCommand(TauriCommand.HookReview, runtimeHookRequest(args));
}

export async function setMcpEnabled(args: McpRequestInput): Promise<ResponseFor<"mcp_set_enabled">> {
  return invokeCommand(TauriCommand.McpSetEnabled, runtimeMcpRequest(args));
}

export async function setMcpEnabledMany(requests: readonly McpRequestInput[]): Promise<ResponseFor<"mcp_set_enabled_many">> {
  return invokeCommand(TauriCommand.McpSetEnabledMany, { requests: requests.map(runtimeMcpRequest) });
}

export async function searchSkillMarketplace(query: string): Promise<MarketplaceSearchResponse> {
  return invokeCommand(TauriCommand.SkillsMarketplaceSearch, { query }) as Promise<MarketplaceSearchResponse>;
}

export async function previewSkillAdd(request: SkillAddRequest): Promise<SkillAddResponse | null> {
  return invokeCommand(TauriCommand.SkillsAdd, runtimeSkillAddRequest(request)) as Promise<SkillAddResponse | null>;
}

export async function installSkillAdd(request: SkillAddRequest): Promise<SkillInstallResult> {
  const response = await invokeCommand(TauriCommand.SkillsAdd, runtimeSkillAddRequest(request));
  return {
    ...response,
    updated: response.updated === undefined ? undefined : recordRows(response.updated, "skills add"),
  } as SkillInstallResult;
}

export async function readSkillPreview(previewId: string, skillName: string): Promise<SkillPreviewReadResponse> {
  return invokeCommand(TauriCommand.SkillsAddPreviewRead, { previewId, skillName }) as Promise<SkillPreviewReadResponse>;
}

function skillRows(value: unknown, label: string): RawDomainRow[] | undefined {
  return value === null || value === undefined ? undefined : recordRows(value, label);
}

function normalizeSkillSetResponse(response: GeneratedSkillsSetResponse): SkillChangeResponse {
  return {
    summary: response.summary,
    applied: response.applied,
    updated: skillRows(response.updated, "skills set"),
  };
}

function normalizeSkillWrapResponse(response: GeneratedSkillsWrapResponse): SkillChangeResponse {
  return {
    summary: response.summary,
    applied: response.applied,
    updated: skillRows(response.updated, "skills wrap"),
  };
}

function normalizeSkillUpdateManyResponse(response: GeneratedSkillsUpdateManyResponse): SkillChangeResponse {
  if ("updated" in response) {
    return {
      summary: response.summary,
      applied: response.applied,
      canApply: response.canApply,
      plan: response.plan,
      previewId: response.previewId ?? undefined,
      updated: recordRows(response.updated, "skills update many"),
    };
  }
  return {
    summary: response.summary,
    applied: response.applied,
    canApply: response.canApply,
    plan: response.plan,
    previewId: response.previewId ?? undefined,
  };
}

function normalizeSkillDeleteManyResponse(response: GeneratedSkillsDeleteManyResponse): SkillChangeResponse {
  return {
    summary: response.summary,
    applied: response.applied,
    previewId: response.previewId ?? undefined,
    refreshRequired: response.refreshRequired,
  };
}

async function invokeSkillChange(command: SkillChangeCommand, args: SkillChangeArgs, dryRun: boolean): Promise<SkillChangeResponse> {
  switch (command) {
    case SkillChangeCommand.Set: {
      const source = args as Omit<GeneratedSkillsSetRequest, "visibility"> & { visibility: string };
      const request: GeneratedSkillsSetRequest = { ...source, visibility: runtimeVisibility(source.visibility), dryRun };
      return normalizeSkillSetResponse(await invokeCommand(TauriCommand.SkillsSet, request));
    }
    case SkillChangeCommand.Wrap: {
      const request: GeneratedSkillsWrapRequest = { ...(args as GeneratedSkillsWrapRequest), dryRun };
      return normalizeSkillWrapResponse(await invokeCommand(TauriCommand.SkillsWrap, request));
    }
    case SkillChangeCommand.UpdateMany: {
      const request: GeneratedSkillsUpdateManyRequest = { ...(args as GeneratedSkillsUpdateManyRequest), dryRun };
      return normalizeSkillUpdateManyResponse(await invokeCommand(TauriCommand.SkillsUpdateMany, request));
    }
    case SkillChangeCommand.DeleteMany:
      return normalizeSkillDeleteManyResponse(await invokeCommand(
        TauriCommand.SkillsDeleteMany,
        args as GeneratedSkillsDeleteManyRequest,
      ));
  }
}

export async function previewSkillChange(command: SkillChangeCommand, args: SkillChangeArgs): Promise<SkillChangeResponse | null> {
  return invokeSkillChange(command, args, true);
}

export async function previewSkillChangeIfAvailable(command: SkillChangeCommand, args: SkillChangeArgs): Promise<SkillChangeResponse | null> {
  try {
    return await previewSkillChange(command, args);
  } catch {
    return null;
  }
}

export async function applySkillChange(command: SkillChangeCommand, args: SkillChangeArgs): Promise<SkillChangeResponse> {
  return invokeSkillChange(command, args, false);
}

export async function applySkillChangeIfAvailable(command: SkillChangeCommand, args: SkillChangeArgs): Promise<SkillChangeResponse | null> {
  try {
    return await applySkillChange(command, args);
  } catch {
    return null;
  }
}
