import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ArrowLeft, ArrowRight, ArrowUpRight, RefreshCw } from "lucide-react";
import { ContextMenu } from "radix-ui";

import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { Button } from "../components/shared/Button.tsx";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { ChartFrame } from "../components/shared/chart/ChartFrame.tsx";
import { ChartLegend } from "../components/shared/chart/ChartLegend.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadingDots } from "../components/shared/LoadingDots.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { SessionTitleText, TranscriptLinkText } from "../components/shared/TranscriptLinkText.tsx";
import { SKILL_BADGE_TONES } from "../features/skills/skill-badge-tones.ts";
import { sessionProject, type SessionRecord } from "../lib/sessions.ts";
import { summarizeSessionUsage } from "../lib/overview.ts";
import { summarizeSessionPreviewRecord } from "../lib/session-preview.ts";
import { formatRelativeTime } from "../lib/strings.ts";
import { dialogCopy } from "../lib/dialog-copy.ts";
import {
  type AnalyticsGranularity,
  type OverviewAnalytics,
  selectAnalyticsGranularity,
} from "../lib/analytics.ts";
import { invokeAnalyticsOverview } from "../lib/runtime-gateway.ts";
import { logger } from "../lib/logger.ts";
import { OpenInEditorMenuItem } from "../components/shared/DataTableMenus.tsx";
import { OverviewTrendChart, OverviewUsageMetric } from "./OverviewTrendChart.tsx";
import { desktopStore, selectAnalyticsDisplayValue, selectAnalyticsValue, useDesktopStore } from "../store/desktop-store.ts";
import { ALL_AGENT_FILTER, DOMAIN_NAV_ITEMS, EMPTY_DISPLAY_VALUE, RuntimeDomainKey } from "../lib/index.ts";
import type { DomainKey } from "../lib/index.ts";
import { SessionListStatus } from "../store/desktop-store.ts";
import "./OverviewView.css";

export type OverviewViewProps = {
  counts: Record<DomainKey, number>;
  hookReviewCount?: number;
  sessions: SessionRecord[];
  onRetryAnalyticsRevision?: () => void | Promise<void>;
  agentFilter: string;
  overviewCountsLoaded: ReadonlySet<DomainKey>;
  overviewCountErrors: ReadonlySet<DomainKey>;
  onRetryCounts?: () => void;
  sessionListStatus: SessionListStatus;
  sessionListError: string;
  skillUpdateCount: number;
  onNavigate: (id: DomainKey) => void;
  onOpenSession: (session: SessionRecord) => void;
};

// Match the initial viewport prefetch window so startup uses one full query.
const ANALYTICS_DEFAULT_RANGE_DAYS = 61;
const MAX_ANALYTICS_DAYS = 365;
const ANALYTICS_REVISION_SETTLE_MS = 400;
const ANALYTICS_LOAD_STEPS = [ANALYTICS_DEFAULT_RANGE_DAYS, 90, 182, MAX_ANALYTICS_DAYS] as const;
let retainedAnalyticsRange = ANALYTICS_DEFAULT_RANGE_DAYS;
let retainedUsageMetric: OverviewUsageMetric = OverviewUsageMetric.Tokens;
const USAGE_METRICS = [
  OverviewUsageMetric.Sessions,
  OverviewUsageMetric.Turns,
  OverviewUsageMetric.Tokens,
  OverviewUsageMetric.Projects,
  OverviewUsageMetric.Cost,
  OverviewUsageMetric.Cache,
  OverviewUsageMetric.Time,
  OverviewUsageMetric.Tools,
  OverviewUsageMetric.Skills,
] as const satisfies ReadonlyArray<OverviewUsageMetric>;
const USAGE_METRIC_LABELS: Record<OverviewUsageMetric, string> = {
  [OverviewUsageMetric.Sessions]: "Sessions",
  [OverviewUsageMetric.Turns]: "Turns",
  [OverviewUsageMetric.Tokens]: "Tokens",
  [OverviewUsageMetric.Projects]: "Projects",
  [OverviewUsageMetric.Cost]: "Cost",
  [OverviewUsageMetric.Cache]: "Cache",
  [OverviewUsageMetric.Time]: "Time",
  [OverviewUsageMetric.Tools]: "Tools",
  [OverviewUsageMetric.Skills]: "Skills",
};
const OVERVIEW_INVENTORY = DOMAIN_NAV_ITEMS.map(({ domain, label }) => ({ id: domain, label }));
const overviewAnalyticsQueries = new Map<string, Promise<OverviewAnalytics | null>>();

function inclusiveDaysSince(date: string): number {
  const [year, month, day] = date.split("-").map(Number);
  if (!year || !month || !day) return ANALYTICS_LOAD_STEPS[0];
  const now = new Date();
  const todayUtc = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate());
  const firstUtc = Date.UTC(year, month - 1, day);
  return Math.max(1, Math.floor((todayUtc - firstUtc) / 86_400_000) + 1);
}

function previousCalendarDate(date: string): string {
  const value = new Date(`${date}T00:00:00Z`);
  value.setUTCDate(value.getUTCDate() - 1);
  return value.toISOString().slice(0, 10);
}

function overviewAnalyticsCacheKey(agent: string, days: number, analyticsRevision: number) {
  return `${agent}:${days}:${analyticsRevision}`;
}

function mergeAnalyticsDays(
  existing: OverviewAnalytics | null,
  incoming: OverviewAnalytics,
  requestedRange: number,
): OverviewAnalytics {
  if (!existing || existing.days.length === 0) {
    return incoming;
  }
  const daysByDate = new Map(existing.days.map((day) => [day.date, day]));
  for (const day of incoming.days) daysByDate.set(day.date, day);
  const capabilitiesByAgent = new Map(existing.capabilities.map((capability) => [capability.agent, { ...capability }]));
  for (const capability of incoming.capabilities) {
    const current = capabilitiesByAgent.get(capability.agent);
    if (!current) capabilitiesByAgent.set(capability.agent, { ...capability });
    else {
      current.tokenUsage ||= capability.tokenUsage;
      current.reasoningTokens ||= capability.reasoningTokens;
      current.explicitRuns ||= capability.explicitRuns;
      current.duration ||= capability.duration;
      current.rateLimitHistory ||= capability.rateLimitHistory;
    }
  }
  return {
    ...incoming,
    daysRequested: Math.max(requestedRange, incoming.daysRequested, existing.daysRequested),
    days: [...daysByDate.values()].sort((left, right) => left.date.localeCompare(right.date)),
    capabilities: [...capabilitiesByAgent.values()],
    warnings: [...new Set([...existing.warnings, ...incoming.warnings])],
  };
}

function AnalyticsLoadingState({
  progress,
}: {
  progress: { completed: number; total: number } | null;
}) {
  const completed = progress?.completed ?? 0;
  const total = progress?.total ?? 0;
  return (
    <ChartFrame ariaLabel="Loading usage chart" legend={<ChartLegend items={[]} />}>
      <LoadingDots className="overviewAnalyticsLoadingDots" />
      <div
        role="progressbar"
        aria-label={total ? `Analyzing ${completed} of ${total} sessions` : "Loading usage analytics"}
        aria-valuemin={0}
        aria-valuemax={total || undefined}
        aria-valuenow={total ? completed : undefined}
        className="overviewVisuallyHidden"
      />
    </ChartFrame>
  );
}

function SessionsLoadingState() {
  return (
    <div className="overviewSessionsLoading" role="status" aria-label={dialogCopy.linkedSessionsLoadingLabel}>
      <LoadingDots className="overviewSessionsLoadingDots" />
    </div>
  );
}

export const OverviewView = memo(function OverviewView({
  counts,
  hookReviewCount = 0,
  sessions,
  onRetryAnalyticsRevision,
  agentFilter,
  overviewCountsLoaded,
  overviewCountErrors,
  onRetryCounts,
  sessionListStatus,
  sessionListError,
  skillUpdateCount,
  onNavigate,
  onOpenSession,
}: OverviewViewProps) {
  const usage = useMemo(() => summarizeSessionUsage(sessions), [sessions]);
  const [analyticsRange, setAnalyticsRange] = useState<number>(retainedAnalyticsRange);
  const analyticsRevision = useDesktopStore((state) => state.analytics.revision);
  const analyticsRevisionReady = useDesktopStore((state) => state.analytics.ready);
  const analyticsRevisionError = useDesktopStore((state) => state.analytics.error);
  const analyticsRangeKey = useMemo(() => ({
    agent: agentFilter,
    range: analyticsRange,
    revision: analyticsRevision,
  }), [agentFilter, analyticsRange, analyticsRevision]);
  const analytics = useDesktopStore((state) => selectAnalyticsDisplayValue(state, analyticsRangeKey));
  const [analyticsLoading, setAnalyticsLoading] = useState(!analytics);
  const analyticsProgress = useDesktopStore((state) => state.analytics.progress);
  const [analyticsError, setAnalyticsError] = useState("");
  const automaticGranularity = selectAnalyticsGranularity(analytics?.days.length ?? analyticsRange);
  const [granularityOverride, setGranularityOverride] = useState<AnalyticsGranularity | null>(null);
  const granularity = granularityOverride ?? automaticGranularity;
  const [usageMetric, setUsageMetric] = useState<OverviewUsageMetric>(retainedUsageMetric);
  const analyticsRequestRef = useRef(0);
  const analyticsRef = useRef<OverviewAnalytics | null>(analytics);
  const analyticsLatestRevisionRef = useRef(analyticsRevision);
  const analyticsLoadRef = useRef<((refreshTranscripts?: boolean, showLoading?: boolean) => Promise<void>) | null>(null);
  const analyticsQueryInFlightRef = useRef(false);
  const analyticsQueryRevisionRef = useRef<number | null>(null);
  const analyticsRevisionRefreshPendingRef = useRef<number | null>(null);
  const analyticsCoalescedRevisionCountRef = useRef(0);
  const analyticsRevisionTimerRef = useRef<number | null>(null);
  analyticsRef.current = analytics;
  analyticsLatestRevisionRef.current = analyticsRevision;
  const loadingOlderAnalytics = Boolean(analytics && analytics.daysRequested < analyticsRange);
  const firstAvailableDate = analytics?.coverage.first;
  const firstLoadedDate = analytics?.days[0]?.date;
  const analyticsTargetRange = firstAvailableDate
    ? Math.min(MAX_ANALYTICS_DAYS, inclusiveDaysSince(firstAvailableDate))
    : ANALYTICS_LOAD_STEPS[0];
  const hasOlderAnalytics = Boolean(
    analyticsRange < analyticsTargetRange
    && firstAvailableDate
    && firstLoadedDate
    && firstAvailableDate < firstLoadedDate
  );
  const loadOlderAnalytics = useCallback((minimumRange?: number) => {
    if (analyticsRange >= analyticsTargetRange) return;
    setAnalyticsRange((current) => {
      if (current >= analyticsTargetRange) return current;
      const step = ANALYTICS_LOAD_STEPS.find((days) => days > current);
      const next = minimumRange === undefined
        ? Math.min(step ?? analyticsTargetRange, analyticsTargetRange)
        : Math.min(analyticsTargetRange, Math.max(current + 1, Math.ceil(minimumRange)));
      retainedAnalyticsRange = next;
      return next;
    });
  }, [analyticsRange, analyticsTargetRange]);
  const scheduleAnalyticsRevisionRefresh = useCallback(() => {
    if (analyticsRevisionTimerRef.current !== null) {
      window.clearTimeout(analyticsRevisionTimerRef.current);
    }
    analyticsRevisionTimerRef.current = window.setTimeout(() => {
      analyticsRevisionTimerRef.current = null;
      void analyticsLoadRef.current?.(false, false);
    }, ANALYTICS_REVISION_SETTLE_MS);
  }, []);
  const loadAnalytics = useCallback(async (refreshTranscripts = false, showLoading = refreshTranscripts) => {
    if (analyticsQueryInFlightRef.current) {
      const queryRevision = analyticsQueryRevisionRef.current;
      if (!refreshTranscripts && queryRevision !== null && analyticsRevision > queryRevision) {
        analyticsRevisionRefreshPendingRef.current = Math.max(
          analyticsRevisionRefreshPendingRef.current ?? analyticsRevision,
          analyticsRevision,
        );
        analyticsCoalescedRevisionCountRef.current += 1;
        logger.info("overview analytics refresh coalesced", {
          requestedRevision: analyticsRevision,
          inFlightRevision: queryRevision,
          refreshTranscripts,
        });
      }
      return;
    }
    if (refreshTranscripts && analyticsRevisionTimerRef.current !== null) {
      window.clearTimeout(analyticsRevisionTimerRef.current);
      analyticsRevisionTimerRef.current = null;
    }
    const request = ++analyticsRequestRef.current;
    const cacheKey = overviewAnalyticsCacheKey(agentFilter, analyticsRange, analyticsRevision);
    const queryKey = { agent: agentFilter, range: analyticsRange, revision: analyticsRevision };
    const cached = selectAnalyticsValue(desktopStore.getSnapshot(), queryKey);
    if (!refreshTranscripts && cached) {
      analyticsRef.current = cached;
      setAnalyticsLoading(false);
      return;
    }
    if (refreshTranscripts) {
      overviewAnalyticsQueries.clear();
    }
    if (showLoading) {
      setAnalyticsLoading(true);
      desktopStore.actions.setAnalyticsProgress(null);
    }
    setAnalyticsError("");
    const loadedDays = analyticsRef.current?.daysRequested ?? 0;
    const isOlderRangeQuery = !refreshTranscripts && analyticsRange > loadedDays && loadedDays > 0;
    const queryDays = isOlderRangeQuery
      ? analyticsRange - loadedDays
      : analyticsRange;
    const existingFirstDate = isOlderRangeQuery
      ? analyticsRef.current?.days[0]?.date
      : undefined;
    const queryEndDate = existingFirstDate ? previousCalendarDate(existingFirstDate) : undefined;
    const args = {
      agent: agentFilter === ALL_AGENT_FILTER ? null : agentFilter,
      days: queryDays,
      rankDays: Math.min(30, queryDays),
      refreshTranscripts,
      ...(queryEndDate ? { endDate: queryEndDate } : {}),
    };
    const queryCacheKey = `${cacheKey}:${queryDays}:${queryEndDate ?? "today"}`;
    let query = refreshTranscripts ? undefined : overviewAnalyticsQueries.get(queryCacheKey);
    if (!query) {
      query = invokeAnalyticsOverview(args);
      if (!refreshTranscripts) {
        const pendingQuery = query;
        overviewAnalyticsQueries.set(queryCacheKey, pendingQuery);
        void pendingQuery.then(() => {
          if (overviewAnalyticsQueries.get(queryCacheKey) === pendingQuery) {
            overviewAnalyticsQueries.delete(queryCacheKey);
          }
        });
      }
    }
    const startedAt = performance.now();
    const coalescedRevisionCount = analyticsCoalescedRevisionCountRef.current;
    analyticsCoalescedRevisionCountRef.current = 0;
    analyticsQueryInFlightRef.current = true;
    analyticsQueryRevisionRef.current = analyticsRevision;
    logger.info("overview analytics query started", {
      requestedRevision: analyticsRevision,
      agent: agentFilter,
      days: queryDays,
      requestedRange: analyticsRange,
      endDate: queryEndDate,
      refreshTranscripts,
      coalescedRevisionCount,
    });
    let resultRevision = analyticsRevision;
    try {
      const result = await query;
      resultRevision = result?.revision ?? analyticsRevision;
      if (request !== analyticsRequestRef.current) return;
      if (result) {
        const merged = mergeAnalyticsDays(analyticsRef.current, result, analyticsRange);
        // The request revision identifies the snapshot the UI asked for. The
        // response revision can lag while the analytics worker is publishing;
        // using it as the cache key makes every settled refresh look stale.
        desktopStore.actions.setAnalyticsValue(merged, queryKey);
        analyticsRef.current = merged;
      }
      else {
        setAnalyticsError((current) => current || "Analytics could not be loaded");
        const loadedDays = analyticsRef.current?.daysRequested;
        if (loadedDays && analyticsRange > loadedDays) {
          retainedAnalyticsRange = loadedDays;
          setAnalyticsRange(loadedDays);
        }
      }
      setAnalyticsLoading(false);
    } finally {
      analyticsQueryInFlightRef.current = false;
      analyticsQueryRevisionRef.current = null;
      const pendingRevision = analyticsRevisionRefreshPendingRef.current;
      const latestRevision = Math.max(analyticsLatestRevisionRef.current, pendingRevision ?? analyticsRevision);
      const shouldRefreshLatestRevision = request === analyticsRequestRef.current
        && !refreshTranscripts
        && latestRevision > analyticsRevision;
      logger.info("overview analytics query completed", {
        requestedRevision: analyticsRevision,
        resultRevision,
        latestRevision,
        durationMs: Math.round(performance.now() - startedAt),
        coalescedRevisionCount,
        refreshLatestRevision: shouldRefreshLatestRevision,
      });
      analyticsRevisionRefreshPendingRef.current = null;
      if (shouldRefreshLatestRevision) scheduleAnalyticsRevisionRefresh();
    }
  }, [agentFilter, analyticsRange, analyticsRevision, scheduleAnalyticsRevisionRefresh]);
  analyticsLoadRef.current = loadAnalytics;
  const showAnalyticsLoading = !analyticsRevisionError && analyticsLoading && !analytics;
  const analyticsRefreshing = analyticsLoading
    || analyticsProgress?.running === true;
  useEffect(() => {
    setGranularityOverride(null);
  }, [automaticGranularity]);

  useEffect(() => {
    if (!analyticsRevisionReady) return;
    if (analyticsQueryInFlightRef.current) {
      const queryRevision = analyticsQueryRevisionRef.current;
      if (queryRevision !== null && analyticsRevision > queryRevision) {
        analyticsRevisionRefreshPendingRef.current = Math.max(
          analyticsRevisionRefreshPendingRef.current ?? analyticsRevision,
          analyticsRevision,
        );
        analyticsCoalescedRevisionCountRef.current += 1;
        logger.info("overview analytics revision coalesced while query is in flight", {
          revision: analyticsRevision,
          inFlightRevision: queryRevision,
          coalescedRevisionCount: analyticsCoalescedRevisionCountRef.current,
        });
      }
      return;
    }
    if (analyticsRef.current === null) {
      void loadAnalytics(false, true);
      return;
    }
    scheduleAnalyticsRevisionRefresh();
  }, [analyticsRevision, analyticsRevisionReady, loadAnalytics, scheduleAnalyticsRevisionRefresh]);

  useEffect(() => () => {
    if (analyticsRevisionTimerRef.current !== null) {
      window.clearTimeout(analyticsRevisionTimerRef.current);
    }
  }, []);

  const updateSkillCount = skillUpdateCount;

  const overviewCountErrorLabels = OVERVIEW_INVENTORY
    .filter((item) => overviewCountErrors.has(item.id))
    .map((item) => item.label);

  return (
    <section className="content overviewPage">
      <ContentTopDragStrip />
      <PageHeader title="Overview" />

      <div className="overviewBody">
        <nav className="overviewInventory" aria-label="Workspace inventory">
          {OVERVIEW_INVENTORY.map((item) => (
            <button
              key={item.id}
              type="button"
              className="overviewInventoryItem"
              onClick={() => onNavigate(item.id)}
              aria-label={`${item.label}: ${counts[item.id].toLocaleString()}${item.id === RuntimeDomainKey.Hooks && hookReviewCount > 0 ? `, ${hookReviewCount} need review` : ""}. Open ${item.label}`}
            >
              <span>
                <span className="overviewInventoryLabelRow">
                  <span className="overviewInventoryLabel">{item.label}</span>
                  {item.id === RuntimeDomainKey.Skills && updateSkillCount > 0 ? (
                    <Badge tone={SKILL_BADGE_TONES.update}>
                      {updateSkillCount} {updateSkillCount === 1 ? "update" : "updates"}
                    </Badge>
                  ) : null}
                  {item.id === RuntimeDomainKey.Hooks && hookReviewCount > 0 ? (
                    <Badge tone="warning">
                      {hookReviewCount} review{hookReviewCount === 1 ? "" : "s"}
                    </Badge>
                  ) : null}
                </span>
                <span className="overviewInventoryValue">
                  {overviewCountsLoaded.has(item.id) ? counts[item.id] : EMPTY_DISPLAY_VALUE}
                </span>
              </span>
              <ArrowUpRight size={14} aria-hidden="true" />
            </button>
          ))}
        </nav>
        {overviewCountErrorLabels.length > 0 ? (
          <Toast
            tone="error"
            message={`Could not load ${overviewCountErrorLabels.join(", ")} counts.`}
            action={onRetryCounts ? { label: "Retry", onClick: onRetryCounts } : undefined}
          />
        ) : null}

        <section className="overviewAnalytics" aria-labelledby="overview-analytics-title">
          <div className="overviewAnalyticsHeader">
            <div>
              <h2 id="overview-analytics-title">Usage</h2>
            </div>
            <div className="overviewAnalyticsControls">
              <SelectControl
                contentClassName="overviewMetricMenu"
                itemClassName="overviewMetricMenuItem"
                label={`Usage metric: ${USAGE_METRIC_LABELS[usageMetric]}`}
                value={usageMetric}
                onValueChange={(value) => {
                  const metric = value as OverviewUsageMetric;
                  retainedUsageMetric = metric;
                  setUsageMetric(metric);
                }}
                options={USAGE_METRICS.map((value) => ({ value, label: USAGE_METRIC_LABELS[value] }))}
                align="end"
              />
              <IconButton type="button" onClick={() => void loadAnalytics(true)} disabled={analyticsRefreshing} aria-label="Refresh analytics" aria-busy={analyticsRefreshing}>
                {analyticsRefreshing ? <LoadingIcon size={15} /> : <RefreshCw size={15} />}
              </IconButton>
            </div>
          </div>

          {analyticsRevisionError && !analytics ? (
            <Toast
              tone="error"
              message={`Analytics refresh failed. ${analyticsRevisionError}`}
              action={onRetryAnalyticsRevision ? { label: "Retry analytics", onClick: () => { void onRetryAnalyticsRevision(); } } : undefined}
            />
          ) : showAnalyticsLoading ? (
            <AnalyticsLoadingState
              progress={analyticsProgress}
            />
          ) : null}
          {analytics ? (
            <>
              <OverviewTrendChart
                analytics={analytics}
                granularity={granularity}
                hasOlder={hasOlderAnalytics}
                loadingOlder={loadingOlderAnalytics}
                metric={usageMetric}
                onLoadOlder={loadOlderAnalytics}
                onGranularityChange={setGranularityOverride}
              />
              {analytics.warnings.length ? <p className="overviewAnalyticsWarning">{analytics.warnings.length} transcript files could not be fully analyzed.</p> : null}
            </>
          ) : analyticsRevisionError ? null : showAnalyticsLoading ? null : analyticsError ? (
            <Toast
              tone="error"
              message={`Analytics refresh failed. ${analyticsError}`}
              action={{ label: "Retry analytics", onClick: () => { void loadAnalytics(true); } }}
            />
          ) : (
            <div className="overviewAnalyticsEmpty">
              <strong>Analytics unavailable</strong>
              <p>No transcript analytics are available yet.</p>
              <Button type="button" variant="ghost" size="sm" className="overviewRetryButton" onClick={() => void loadAnalytics(true)}>
                Retry analytics
              </Button>
            </div>
          )}
          {analyticsError && analytics ? (
            <Toast tone="error" message={`Refresh failed. Existing analytics are still shown. ${analyticsError}`} />
          ) : null}
          {analyticsRevisionError && analytics ? (
            <Toast tone="error" message={`Analytics revision refresh failed. Existing analytics are still shown. ${analyticsRevisionError}`} />
          ) : null}
        </section>

        <div className="overviewWorkbench">
          <section className="overviewPane" aria-labelledby="overview-sessions-title">
            <div className="overviewPaneHeader">
              <h2 id="overview-sessions-title" className="overviewPaneTitle">{dialogCopy.recentSessionsLabel}</h2>
              <button type="button" className="overviewPaneAction" onClick={() => onNavigate(RuntimeDomainKey.Sessions)}>
                Open sessions
              </button>
            </div>

            {sessionListStatus === SessionListStatus.Loading && usage.recentSessions.length === 0 ? (
              <SessionsLoadingState />
            ) : usage.recentSessions.length > 0 ? (
              <div className="overviewSessionGrid" aria-label={dialogCopy.recentSessionsLabel}>
                {usage.recentSessions.map((session) => {
                  const previewKey = `${session.agent}:${session.id}`;
                  const preview = summarizeSessionPreviewRecord(session);
                  const displayTitle = session.title;
                  return (
                    <ContextMenu.Root key={previewKey}>
                      <ContextMenu.Trigger asChild>
                        <button
                          type="button"
                          className="overviewSessionRow"
                          onClick={() => onOpenSession(session)}
                        >
                          <span className="overviewSessionRowHeader">
                            <span className="overviewSessionTitleLine">
                              <AgentBadge agent={session.agent} small />
                              <span className="overviewSessionRowTitle"><SessionTitleText interactive={false} value={displayTitle} /></span>
                            </span>
                            <span className="overviewSessionUpdated">{formatRelativeTime(session.updatedAt) || EMPTY_DISPLAY_VALUE}</span>
                          </span>
                          <span className="overviewSessionMessage">
                            <span className="overviewSessionMessageLabel" role="img" aria-label="User message"><ArrowRight size={13} aria-hidden="true" /></span>
                            <span className="overviewSessionMessageText"><TranscriptLinkText interactive={false} value={preview?.userLast ?? EMPTY_DISPLAY_VALUE} /></span>
                          </span>
                          <span className="overviewSessionFooter">
                            <span className="overviewSessionMessage">
                              <span className="overviewSessionMessageLabel" role="img" aria-label="Agent reply"><ArrowLeft size={13} aria-hidden="true" /></span>
                              <span className="overviewSessionMessageText"><TranscriptLinkText interactive={false} value={preview?.assistantLast ?? EMPTY_DISPLAY_VALUE} /></span>
                            </span>
                            <span className="overviewSessionProject">{sessionProject(session)}</span>
                          </span>
                        </button>
                      </ContextMenu.Trigger>
                      <ContextMenu.Portal>
                        <ContextMenu.Content className="menuContent" data-no-drag>
                          <OpenInEditorMenuItem Menu={ContextMenu} path={session.path} />
                        </ContextMenu.Content>
                      </ContextMenu.Portal>
                    </ContextMenu.Root>
                  );
                })}
              </div>
            ) : sessionListStatus === SessionListStatus.Error ? (
              <Toast tone="error" message={sessionListError || "Could not load sessions. Try again."} />
            ) : (
              <p className="overviewQuiet">No sessions found for this agent.</p>
            )}
          </section>
        </div>
      </div>
    </section>
  );
});
