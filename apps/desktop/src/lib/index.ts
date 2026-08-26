export {
  navItems,
  SIDEBAR_SIZE,
  COLLAPSED_SIDEBAR_SIZE,
  SESSION_FREEZE_COLUMN,
  SKILL_FREEZE_COLUMN,
  RULE_FREEZE_COLUMN,
  HOOK_FREEZE_COLUMN,
  MCP_FREEZE_COLUMN,
  MARQUEE_DRAG_THRESHOLD,
  MARQUEE_AUTO_SCROLL_EDGE,
  MARQUEE_AUTO_SCROLL_MAX_SPEED,
} from "./constants.ts";
export type { NavItem, FreezeColumnConfig } from "./constants.ts";

export { agentDefinition, agentDefinitions } from "./agent/index.ts";

export {
  agentIcons,
  normalizedAgentKey,
  agentIdentityKey,
  isConcreteAgent,
  agentClassName,
  agentIcon,
  friendlyAgent,
  GitLabSourceIcon,
  sameAgent,
} from "./agents.ts";

export {
  basename,
  titleValue,
  compactCommand,
  formatUserPath,
  compactDateTime,
  dayGroupKey,
  formatDayGroupLabel,
  formatDuration,
} from "./strings.ts";

export {
  friendlySource,
  skillSourceDetails,
  isWebSource,
  normalizeGitPath,
  parseRemoteSource,
  remoteRepositoryLabel,
  isGitSource,
  normalizeSourceFilePath,
  encodeRemotePath,
  sourceRemoteDetails,
  remoteSkillFileUrl,
  sourceOpenUrl,
  sourceLocalPath,
  sourceIconDetails,
  pathLooksLikePluginCache,
  openSource,
} from "./sources.ts";
export type { SkillLike, SourceDetails, RemoteSource } from "./sources.ts";

export {
  isPluginSkillSource,
  isSystemSkillSource,
  isReadOnlySkillSource,
  statusTone,
  isSkillVisibilityEditable,
  isSkillRowSelectable,
  isSkillSelectable,
  skillSection,
  SkillVisibility,
  editableSkillVisibilities,
  allSkillVisibilities,
  normalizeSkill,
  normalizeSkillVisibility,
  primarySkillPath,
  skillTargets,
  targetLabel,
  targetAgentLabel,
  localSkillSourcePath,
  skillSourceAction,
  SkillChangeCommand,
  skillChangeActionLabel,
  applySkillUpdateReports,
  mergeSkillListPreservingUpdates,
  replaceSkillReportPreservingUpdates,
  clearSkillUpdateAvailability,
  applyVisibilityState,
} from "./skills.ts";
export type { NormalizedSkill, NormalizedSkillPath } from "./skills.ts";

export { suppressNextClick } from "./dom.ts";

export { logger } from "./logger.ts";
export type { LogFields, LogLevel } from "./logger.ts";

export { normalizeMissingSessionProjectPolicy, projectForPath, scopeColumn, scopeNameForPath } from "./projects.ts";
export type { MissingSessionProjectPolicy, ProjectSummary, SessionProjectSummary } from "./projects.ts";

export {
  normalizeSession,
  normalizeSessionSkillLink,
  normalizeSessionResumeTarget,
  sessionResumeTargetForAgent,
  sessionAppDeepLink,
  isAbsolutePath,
  sessionWorkspacePath,
  sessionRepositoryPath,
  sessionLaunchPayload,
  sessionIdentity,
  sessionIdentityRecordKey,
  sessionLogicalIdentity,
  sessionWorkspace,
  sessionProject,
  sessionProjectGroupKey,
  sessionProjectGroupLabel,
  sessionProjectOption,
  sessionKind,
  sessionCacheRate,
  sessionSnapshot,
  mergeSessionRows,
  applySessionDelta,
  sortValue,
  compareSessions,
  sessionTimeMs,
} from "./sessions.ts";
export { sessionExternalKey, resolveInitialSession, resolveInitialSessionId } from "./session-selection.ts";
export type { SessionIdentityRecord, SessionResumeOutcome, SessionResumeTarget, SessionSkillLinkRecord } from "./sessions.ts";
export type { SessionKind, SessionRecord, SessionTokenUsage } from "./sessions.ts";

export { summarizeSessionUsage } from "./overview.ts";
export { formatSessionTitle, formatTranscriptPreview, summarizeSessionPreviewRecord } from "./session-preview.ts";
export type { RecentSessionPreview } from "./session-preview.ts";
export type { SessionUsageSummary, TokenMix } from "./overview.ts";

export { groupAnalyticsDays } from "./analytics.ts";
export type {
  AnalyticsCallUsage,
  AnalyticsCapabilities,
  AnalyticsDay,
  AnalyticsGranularity,
  AnalyticsPeriod,
  AnalyticsRankItem,
  AnalyticsTokenUsage,
  OverviewAnalytics,
} from "./analytics.ts";

export {
  normalizePrompt,
  normalizePromptTags,
  promptTagsLabel,
  promptPreview,
  promptTitleFromBody,
} from "./prompt-model.ts";
export type { PromptRecord } from "./prompt-model.ts";

export {
  normalizeRule,
  ruleAgents,
  ruleTitle,
  ruleKey,
  ruleSearchText,
  ruleSortValue,
  compareRules,
} from "./rules.ts";
export type { RuleRecord, RuleRow } from "./rules.ts";

export {
  normalizeHook,
  hookKey,
  hookDeleteIdentity,
  hookItemsFromRows,
  hookHandlerText,
  hookTypeLabel,
  hookTrustHash,
  hookSearchText,
  hookSourcePath,
  hookDeleteDisabledReason,
} from "./hooks.ts";
export type { HookRecord } from "./hooks.ts";

export { mcpRowKey, mcpSourcePath, normalizeMcp } from "./mcp.ts";
export type { McpRecord } from "./mcp.ts";

export {
  createLatestRequestAuthority,
  mergeTranscriptItems,
  transcriptItemsSize,
  normalizeTranscript,
  normalizeTranscriptLocatorPage,
  normalizeTranscriptPage,
  normalizeTranscriptSearchResult,
  transcriptItemType,
  groupTranscriptItems,
} from "./transcript.ts";
export { parseJsonlTranscript } from "./agent/transcript.ts";
export type {
  JsonlTranscriptParseResult,
  TranscriptItem,
  TranscriptGroup,
  TranscriptLocatorItem,
  TranscriptLocatorPage,
  TranscriptPage,
  TranscriptSearchHit,
  TranscriptSearchResult,
  TranscriptSearchScopes,
} from "./transcript.ts";


export {
  emptyRuntimeData,
  initialData,
  normalizeDomainRows,
  normalizeReport,
  recomputeSources,
} from "./data.ts";
export type { RuntimeData, RuntimeDomainKey } from "./data.ts";

export {
  buildFileTreeRows,
  displayFileName,
  normalizeSkillFileEntries,
  preferredSkillFileName,
  parentPath,
  joinRelativePath,
  uniqueChildPath,
  splitMarkdownFrontmatter,
  isMarkdownPath,
  isYamlPath,
  isJsonPath,
  normalizeLinkHref,
} from "./file-tree.ts";
export type { SkillFileEntry, FileTreeRow } from "./file-tree.ts";

export {
  diffPreview,
  inlineDiffSegments,
  inlineCommonCount,
  shouldPairInlineDiff,
  addInlineDiffSegments,
  currentLineDiffMap,
  mergeThreeWay,
} from "./diff.ts";
export type { DiffSegment, DiffLine, ThreeWayMergeResult } from "./diff.ts";

export {
  clamp,
  edgeAutoScrollDelta,
  marqueeAutoScrollDelta,
  clientRectFromPoints,
  rectFromPoints,
  contentPointFromClient,
  clientPointFromContent,
  elementContentRect,
  rectsIntersect,
} from "./geometry.ts";
export type { Point, Rect } from "./geometry.ts";

export {
  WINDOW_DRAG_REGION_SELECTOR,
  WINDOW_DRAG_BLOCK_SELECTOR,
  shouldStartWindowDrag,
  startWindowDrag,
} from "./window-drag.ts";

export { DaemonCommandError, TauriCommand, UPDATE_AVAILABLE_EVENT, invokeCommand, isTauriRuntime, safeInvoke, subscribeDaemonEvents, copyText } from "./tauri.ts";
export type {
  BundledSkillInstallReport,
  BundledSkillStatus,
  CliInstallState,
  CliInstallStatus,
  DaemonEvent,
  DesktopUpdateState,
  UpdateCheckResult,
} from "./tauri.ts";

export { textMatchRank, boostedTextMatchRank } from "./text-search.ts";
