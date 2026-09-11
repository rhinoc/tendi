export { actionLabels, copiedPathLabel, copiedValueLabel, copyPathLabel, copyValueLabel, deleteConfirmationDescription, logExportLabels, promptActionLabels, revealPathLabel, selectionCopiedLabel, selectionCopyLabel, selectionDeleteErrorLabel, selectionDeleteLabel, selectionDeleteLoadingLabel } from "./action-labels.ts";
export { mcpCopy } from "./mcp-copy.ts";

export {
  navItems,
  DOMAIN_NAV_ITEMS,
  AppPage,
  EMPTY_DISPLAY_VALUE,
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
export type { NavItem, DomainNavItem, FreezeColumnConfig } from "./constants.ts";
export { AsyncStatus } from "./async-status.ts";
export { SortDirection } from "./sort.ts";

export { agentDefinition, agentDefinitions } from "./agent/index.ts";

export {
  agentIcons,
  ALL_AGENT_FILTER,
  normalizedAgentKey,
  agentIdentityKey,
  isConcreteAgent,
  isVisibleAgent,
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
  formatRelativeTime,
  dayGroupKey,
  formatDayGroupLabel,
  formatDuration,
} from "./strings.ts";
export { compareTimestamps, timestampMs } from "./time.ts";
export { decideRevision, snapshotRevision } from "./runtime-contract.ts";
export type {
  DomainSnapshot,
  InstallationId,
  Revision,
  RevisionDecision,
  RevisionedEvent,
  SessionKey,
  SourceLocator,
  ScopeKey,
} from "./runtime-contract.ts";

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
  sourceFaviconUrl,
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
  primarySkillScope,
  skillTargets,
  targetLabel,
  targetAgentLabel,
  localSkillSourcePath,
  skillSourceActionLabels,
  skillSourceAction,
  skillDisplayName,
  SkillChangeCommand,
} from "./skills.ts";
export type { AvailableSkill, NormalizedSkill, NormalizedSkillPath, RawSkillRecord, SkillAddPlan, SkillInstallResult, SkillOperation, WrapperArgs } from "./skills.ts";
export { SkillOperationStatus, SkillUpdateAvailability } from "./skills.ts";

export { suppressNextClick } from "./dom.ts";

export { skillChangeActionLabel, skillChangeBusyLabel, skillChangeCanConfirm, skillChangeDescription, skillChangeDisabledReason, skillChangeLoadingCopy, skillChangeTitle } from "./skill-change-copy.ts";

export { dialogCopy } from "./dialog-copy.ts";

export { DOMAIN_KEYS, RUNTIME_DOMAIN_KEYS, RuntimeDomainKey } from "./domain.ts";
export type { DomainKey } from "./domain.ts";

export { normalizeConfigProfiles, normalizeSettings } from "./settings.ts";
export type { SettingsPayload, SettingsState } from "./settings.ts";

export { logger } from "./logger.ts";
export { LogLevel } from "./logger.ts";
export type { LogFields } from "./logger.ts";

export { MissingSessionProjectPolicy, ProjectScopeFilter, matchesProjectScope, normalizeMissingSessionProjectPolicy, normalizeProjectScopeFilter, pathIsProjectScoped, projectForPath, scopeColumn, scopeColumnFromValue, scopeNameForPath, scopeNameForValue } from "./projects.ts";
export type { ProjectSummary, SessionProjectSummary } from "./projects.ts";
export { normalizeSidebarFilters, persistSidebarFilters, readCachedSidebarFilters, SIDEBAR_FILTERS_STORAGE_KEY } from "./sidebar-filters.ts";
export type { SidebarFilterState } from "./sidebar-filters.ts";

export {
  SessionKind,
  SessionSortKey,
  SessionResumeErrorAction,
  SessionResumeErrorCode,
  SessionResumeOutcomeStatus,
  SessionResumeTarget,
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
  SESSION_SEARCH_SORT,
  sortValue,
  compareSessions,
  sessionTimeMs,
} from "./sessions.ts";
export { sessionResumeError, sessionResumeErrorMessage, sessionResumeLabel, sessionResumeTargetForMenu, sessionResumeTargetsForMenu } from "./session-resume.ts";
export type { SessionResumeState } from "./session-resume.ts";
export { sessionExternalKey, sessionSourceExternalKey, resolveInitialSession, resolveInitialSessionId } from "./session-selection.ts";
export type { SessionIdentityRecord, SessionResumeError, SessionResumeOutcome, SessionSkillLinkRecord } from "./sessions.ts";
export type { SessionRecord, SessionTokenUsage } from "./sessions.ts";

export { summarizeSessionUsage } from "./overview.ts";
export { formatSessionTitle, formatTranscriptPreview, sessionTitleValue, summarizeSessionPreviewRecord } from "./session-preview.ts";
export type { RecentSessionPreview } from "./session-preview.ts";
export type { SessionUsageSummary, TokenMix } from "./overview.ts";

export { AnalyticsGranularity, AnalyticsRefreshPhase, groupAnalyticsDays } from "./analytics.ts";
export type {
  AnalyticsCallUsage,
  AnalyticsCapabilities,
  AnalyticsDay,
  AnalyticsPeriod,
  AnalyticsRankItem,
  AnalyticsTokenUsage,
  OverviewAnalytics,
} from "./analytics.ts";

export {
  normalizePrompt,
  normalizePromptTags,
  promptDisplayName,
  promptTagsLabel,
  promptPreview,
  promptTitleFromBody,
} from "./prompt-model.ts";
export type { PromptRecord } from "./prompt-model.ts";

export { configDisplayName } from "./config.ts";

export {
  normalizeRule,
  ruleAgents,
  ruleTitle,
  ruleKey,
  ruleSearchText,
  ruleSortValue,
  compareRules,
  RuleScope,
} from "./rules.ts";
export type { RuleRecord, RuleRow } from "./rules.ts";


export {
  configSelectionActionIds,
  hookSelectionActionIds,
  mcpSelectionActionIds,
  promptSelectionActionIds,
  TableSelectionActionId,
  ruleSelectionActionIds,
  visibleSelectionActionCount,
} from "./table-selection-actions.ts";
export { SkillActionId, skillActionIds } from "./skill-actions.ts";

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
  hookDisplayName,
  isHookMutationDelta,
} from "./hooks.ts";
export type { HookRecord, HookMutationDelta } from "./hooks.ts";

export { mcpDisplayName, mcpNeedsLogin, mcpRowKey, mcpSourcePath, mcpStatusLabel, normalizeMcp, isMcpMutationDelta } from "./mcp.ts";
export type { McpRecord, McpMutationDelta } from "./mcp.ts";

export {
  createLatestRequestAuthority,
  mergeTranscriptItems,
  transcriptItemsSize,
  normalizeTranscript,
  normalizeTranscriptLocatorPage,
  normalizeTranscriptPage,
  normalizeTranscriptSearchResult,
  transcriptContextPreview,
  transcriptEvidenceSearchText,
  transcriptItemType,
  TranscriptGroupType,
  groupTranscriptItems,
} from "./transcript.ts";
export { parseJsonlTranscriptForProvider } from "./agent/transcript.ts";
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


export { emptyRuntimeData } from "./data.ts";
export type { RuntimeData } from "./data.ts";

export { selectOverviewCounts, selectOverviewHookReviewCount, selectOverviewSkillUpdateCount } from "../controllers/overview-controller.ts";
export type { OverviewCounts } from "../controllers/overview-controller.ts";

export {
  applyDomainSnapshot,
  applyRuleCommandResult,
  buildCatalogIndexes,
  reconcileCollection,
  selectCatalogView,
  selectProjectScopeView,
} from "../controllers/catalog-controller.ts";
export type { CatalogIndexes, CatalogSource, DomainRows, RawDomainRow, RawDomainRows } from "../controllers/controller-types.ts";
export {
  applySkillPatch,
  applySkillSnapshot,
  applySkillUpdateReports as applySkillUpdateReportsController,
  applySkillVisibility,
  clearSkillUpdateAvailability as clearSkillUpdateAvailabilityFromController,
  expandSkillDependencies,
  partitionSkillOperations,
} from "../controllers/skill-controller.ts";
export {
  buildGroupedSessionPages,
  projectSearchRank,
  sessionPageForRow,
  selectSessionListView,
  selectSessionRelationships,
  sessionPageContextKey,
  sessionMatchesQuery,
  sessionTableRowId,
} from "../controllers/session-controller.ts";
export type { GroupedSessionPage, ProjectSearchRank, SessionListControllerInput, SessionListView, SessionProjectOption, SessionRelationshipView } from "../controllers/session-controller.ts";
export { selectRuleListView } from "../controllers/rule-controller.ts";
export type { RuleListItem, RuleListView, RuleTableItem } from "../controllers/rule-controller.ts";

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
export { CliInstallState, DesktopUpdateStatus, UpdateCheckStatus } from "./tauri.ts";
export { assertRuntimeEventPayload } from "./generated/runtime-events.ts";
export type {
  BundledSkillInstallReport,
  BundledSkillStatus,
  CliInstallStatus,
  DaemonEvent,
  DesktopUpdateState,
  UpdateCheckResult,
} from "./tauri.ts";

export { textMatchRank, boostedTextMatchRank } from "./text-search.ts";
