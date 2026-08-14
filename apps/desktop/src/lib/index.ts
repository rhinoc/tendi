export {
  navItems,
  fallbackAgents,
  fallbackSkills,
  fallbackSkillFiles,
  fallbackSessions,
  fallbackTranscript,
  SIDEBAR_SIZE,
  COLLAPSED_SIDEBAR_SIZE,
  SESSION_FREEZE_COLUMN,
  RULE_FREEZE_COLUMN,
  HOOK_FREEZE_COLUMN,
  MCP_FREEZE_COLUMN,
  MARQUEE_DRAG_THRESHOLD,
  MARQUEE_AUTO_SCROLL_EDGE,
  MARQUEE_AUTO_SCROLL_MAX_SPEED,
} from "./constants.ts";
export type { NavItem, FreezeColumnConfig } from "./constants.ts";

export {
  agentIcons,
  normalizedAgentKey,
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
  clearSkillUpdateAvailability,
  applyVisibilityState,
} from "./skills.ts";
export type { NormalizedSkill } from "./skills.ts";

export { suppressNextClick } from "./dom.ts";

export {
  normalizeSession,
  isAbsolutePath,
  sessionWorkspacePath,
  sessionRepositoryPath,
  sessionLaunchPayload,
  sessionIdentity,
  sessionLogicalIdentity,
  sessionWorkspace,
  sessionProject,
  sessionProjectGroupKey,
  sessionProjectGroupLabel,
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
export type { SessionIdentityRecord } from "./sessions.ts";
export type { SessionKind, SessionRecord, SessionTokenUsage } from "./sessions.ts";
export { applySessionProjectDelta } from "./session-project-delta.ts";
export type { SessionProjectDelta } from "./session-project-delta.ts";

export { summarizeRecentSession, summarizeSessionUsage } from "./overview.ts";
export { summarizeSessionPreview, summarizeSessionPreviewRecord } from "./session-preview.ts";
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
  SessionAnalyticsDetail,
} from "./analytics.ts";

export {
  normalizePrompt,
  normalizePromptTags,
  promptTagsLabel,
  promptPreview,
} from "./prompt-model.ts";
export type { PromptRecord } from "./prompt-model.ts";

export {
  ruleTitle,
  ruleKey,
  ruleSearchText,
  ruleSortValue,
  compareRules,
} from "./rules.ts";
export type { RuleRecord, RuleRow } from "./rules.ts";

export {
  hookKey,
  hookDeleteIdentity,
  hookItemsFromRows,
  hookHandlerText,
  hookTypeLabel,
  hookSourceTitle,
  hookTrustHash,
  hookSearchText,
  hookSourcePath,
  hookDeleteDisabledReason,
} from "./hooks.ts";
export type { HookRecord } from "./hooks.ts";

export {
  createLatestRequestAuthority,
  mergeTranscriptItems,
  transcriptItemsSize,
  normalizeTranscript,
  normalizeTranscriptPage,
  parseJsonlTranscript,
  transcriptItemType,
  groupTranscriptItems,
} from "./transcript.ts";
export type { JsonlTranscriptParseResult, TranscriptItem, TranscriptGroup, TranscriptPage } from "./transcript.ts";


export {
  fallbackData,
  emptyRuntimeData,
  initialData,
  normalizeDomainRows,
  normalizeReport,
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
  fallbackSkillContent,
  splitMarkdownFrontmatter,
  isMarkdownPath,
  isYamlPath,
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
} from "./diff.ts";
export type { DiffSegment, DiffLine } from "./diff.ts";

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

export { TauriCommand, safeInvoke, copyText } from "./tauri.ts";
export type {
  BundledSkillInstallReport,
  BundledSkillStatus,
  CliInstallState,
  CliInstallStatus,
} from "./tauri.ts";

export { textMatchRank, boostedTextMatchRank } from "./text-search.ts";
