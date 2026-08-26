import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { ArrowLeft, ArrowRight, ArrowUpRight, RefreshCw } from "lucide-react";

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
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { SessionTitleText, TranscriptLinkText } from "../components/shared/TranscriptLinkText.tsx";
import { SKILL_BADGE_TONES } from "../features/skills/skill-badge-tones.ts";
import { tokenToneClass } from "../lib/token-style.ts";
import { formatTokenCount } from "../lib/token-format.ts";
import { sessionProject, type SessionRecord } from "../lib/sessions.ts";
import { summarizeSessionUsage } from "../lib/overview.ts";
import { formatSessionTitle, summarizeSessionPreviewRecord } from "../lib/session-preview.ts";
import {
  type AnalyticsGranularity,
  type AnalyticsRefreshProgress,
  type OverviewAnalytics,
  selectAnalyticsGranularity,
} from "../lib/analytics.ts";
import { TauriCommand, safeInvoke } from "../lib/tauri.ts";
import { OverviewTrendChart, type OverviewOlderLoadReason, type OverviewUsageMetric } from "./OverviewTrendChart.tsx";
import type { SkillRecord } from "./SkillsView.tsx";
import "./OverviewView.css";

export type OverviewNavId = "skills" | "prompts" | "sessions" | "rules" | "hooks" | "mcp";
export type SessionListStatus = "loading" | "loaded" | "error";

export type OverviewViewProps = {
  counts: Record<OverviewNavId, number>;
  hookReviewCount?: number;
  skills: SkillRecord[];
  sessions: SessionRecord[];
  analyticsRevision: number;
  analyticsRevisionReady: boolean;
  analyticsRevisionError?: string;
  onRetryAnalyticsRevision?: () => void | Promise<void>;
  agentFilter: string;
  overviewCountsLoaded: ReadonlySet<OverviewNavId>;
  overviewCountErrors: ReadonlySet<OverviewNavId>;
  sessionListStatus: SessionListStatus;
  sessionListError: string;
  skillUpdateCount: number;
  onNavigate: (id: OverviewNavId) => void;
  onOpenSession: (session: SessionRecord) => void;
};

const ANALYTICS_LOAD_STEPS = [30, 90, 182, 365] as const;
const MAX_ANALYTICS_DAYS = 365;
let retainedAnalyticsRange = 30;
let retainedUsageMetric: OverviewUsageMetric = "tokens";
const USAGE_METRICS = ["sessions", "turns", "tokens", "cache", "tools", "skills"] as const satisfies ReadonlyArray<OverviewUsageMetric>;
const USAGE_METRIC_LABELS: Record<OverviewUsageMetric, string> = {
  sessions: "Sessions",
  turns: "Turns",
  tokens: "Tokens",
  cache: "Cache",
  tools: "Tools",
  skills: "Skills",
};
const overviewAnalyticsCache = new Map<string, OverviewAnalytics>();
const overviewAnalyticsQueries = new Map<string, Promise<OverviewAnalytics | null>>();

function inclusiveDaysSince(date: string): number {
  const [year, month, day] = date.split("-").map(Number);
  if (!year || !month || !day) return ANALYTICS_LOAD_STEPS[0];
  const now = new Date();
  const todayUtc = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate());
  const firstUtc = Date.UTC(year, month - 1, day);
  return Math.max(1, Math.floor((todayUtc - firstUtc) / 86_400_000) + 1);
}

function overviewAnalyticsCacheKey(agent: string, days: number, analyticsRevision: number) {
  return `${agent}:${days}:${analyticsRevision}`;
}

function AnalyticsLoadingState({
  progress,
}: {
  progress: AnalyticsRefreshProgress | null;
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
    <div className="overviewSessionsLoading" role="status" aria-label="Loading recent sessions">
      <LoadingDots className="overviewSessionsLoadingDots" />
    </div>
  );
}

export function OverviewView({
  counts,
  hookReviewCount = 0,
  skills,
  sessions,
  analyticsRevision,
  analyticsRevisionReady,
  analyticsRevisionError = "",
  onRetryAnalyticsRevision,
  agentFilter,
  overviewCountsLoaded,
  overviewCountErrors,
  sessionListStatus,
  sessionListError,
  skillUpdateCount,
  onNavigate,
  onOpenSession,
}: OverviewViewProps) {
  const usage = useMemo(() => summarizeSessionUsage(sessions), [sessions]);
  const initialAnalytics = overviewAnalyticsCache.get(
    overviewAnalyticsCacheKey(agentFilter, retainedAnalyticsRange, analyticsRevision),
  ) ?? null;
  const [analytics, setAnalytics] = useState<OverviewAnalytics | null>(initialAnalytics);
  const [analyticsLoading, setAnalyticsLoading] = useState(!initialAnalytics);
  const [analyticsProgress, setAnalyticsProgress] = useState<AnalyticsRefreshProgress | null>(null);
  const [analyticsError, setAnalyticsError] = useState("");
  const [analyticsRange, setAnalyticsRange] = useState<number>(retainedAnalyticsRange);
  const [preparingAnalyticsRange, setPreparingAnalyticsRange] = useState(false);
  const automaticGranularity = selectAnalyticsGranularity(analytics?.days.length ?? analyticsRange);
  const [granularityOverride, setGranularityOverride] = useState<AnalyticsGranularity | null>(null);
  const granularity = granularityOverride ?? automaticGranularity;
  const [usageMetric, setUsageMetric] = useState<OverviewUsageMetric>(retainedUsageMetric);
  const analyticsRequestRef = useRef(0);
  const analyticsRef = useRef(analytics);
  analyticsRef.current = analytics;
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
  const loadOlderAnalytics = useCallback((reason: OverviewOlderLoadReason) => {
    if (analyticsRange >= analyticsTargetRange) return;
    if (reason === "auto") setPreparingAnalyticsRange(true);
    setAnalyticsRange((current) => {
      if (current >= analyticsTargetRange) return current;
      const step = ANALYTICS_LOAD_STEPS.find((days) => days > current);
      const next = Math.min(step ?? analyticsTargetRange, analyticsTargetRange);
      retainedAnalyticsRange = next;
      return next;
    });
  }, [analyticsRange, analyticsTargetRange]);
  const loadAnalytics = useCallback(async (refreshTranscripts = false, showLoading = refreshTranscripts) => {
    const request = ++analyticsRequestRef.current;
    const cacheKey = overviewAnalyticsCacheKey(agentFilter, analyticsRange, analyticsRevision);
    const cached = overviewAnalyticsCache.get(cacheKey);
    if (!refreshTranscripts && cached) {
      analyticsRef.current = cached;
      setAnalytics(cached);
      setAnalyticsLoading(false);
      setPreparingAnalyticsRange(false);
      return;
    }
    if (refreshTranscripts) {
      overviewAnalyticsCache.clear();
      overviewAnalyticsQueries.clear();
    }
    if (showLoading) {
      setAnalyticsLoading(true);
      setAnalyticsProgress(null);
    }
    setAnalyticsError("");
    const args = {
      agent: agentFilter === "All" ? null : agentFilter,
      days: analyticsRange,
      rankDays: Math.min(30, analyticsRange),
      refreshTranscripts,
    };
    let query = refreshTranscripts ? undefined : overviewAnalyticsQueries.get(cacheKey);
    if (!query) {
      query = safeInvoke<OverviewAnalytics>(TauriCommand.AnalyticsOverview, args);
      if (!refreshTranscripts) {
        const pendingQuery = query;
        overviewAnalyticsQueries.set(cacheKey, pendingQuery);
        void pendingQuery.then(() => {
          if (overviewAnalyticsQueries.get(cacheKey) === pendingQuery) {
            overviewAnalyticsQueries.delete(cacheKey);
          }
        });
      }
    }
    const result = await query;
    if (request !== analyticsRequestRef.current) return;
    if (result) {
      overviewAnalyticsCache.set(
        overviewAnalyticsCacheKey(agentFilter, analyticsRange, result.revision),
        result,
      );
      analyticsRef.current = result;
      setAnalytics(result);
      setPreparingAnalyticsRange(false);
    }
    else {
      setAnalyticsError((current) => current || "Analytics could not be loaded");
      setPreparingAnalyticsRange(false);
      const loadedDays = analyticsRef.current?.daysRequested;
      if (loadedDays && analyticsRange > loadedDays) {
        retainedAnalyticsRange = loadedDays;
        setAnalyticsRange(loadedDays);
      }
    }
    setAnalyticsLoading(false);
  }, [agentFilter, analyticsRange, analyticsRevision]);
  const showAnalyticsLoading = !analyticsRevisionError && (
    preparingAnalyticsRange || (analyticsLoading && !analytics)
  );
  useEffect(() => {
    setGranularityOverride(null);
  }, [automaticGranularity]);

  useEffect(() => {
    if (!analyticsRevisionReady) return;
    const cacheKey = overviewAnalyticsCacheKey(agentFilter, analyticsRange, analyticsRevision);
    const cached = overviewAnalyticsCache.get(cacheKey);
    if (cached) {
      analyticsRef.current = cached;
      setAnalytics(cached);
      setAnalyticsLoading(false);
    }
    void loadAnalytics(false, !cached && analyticsRef.current === null);
  }, [agentFilter, analyticsRange, analyticsRevision, analyticsRevisionReady, loadAnalytics]);

  const updateSkillCount = skillUpdateCount;

  const inventory: Array<{ id: OverviewNavId; label: string }> = [
    { id: "skills", label: "Skills" },
    { id: "sessions", label: "Sessions" },
    { id: "prompts", label: "Prompts" },
    { id: "rules", label: "Rules" },
    { id: "hooks", label: "Hooks" },
    { id: "mcp", label: "MCP" },
  ];

  return (
    <section className="content overviewPage">
      <ContentTopDragStrip />
      <PageHeader title="Overview" />

      <div className="overviewBody">
        <nav className="overviewInventory" aria-label="Workspace inventory">
          {inventory.map((item) => (
            <button
              key={item.id}
              type="button"
              className="overviewInventoryItem"
              onClick={() => onNavigate(item.id)}
              aria-label={`${item.label}: ${counts[item.id].toLocaleString()}${item.id === "hooks" && hookReviewCount > 0 ? `, ${hookReviewCount} need review` : ""}. Open ${item.label}`}
            >
              <span>
                <span className="overviewInventoryLabelRow">
                  <span className="overviewInventoryLabel">{item.label}</span>
                  {item.id === "skills" && updateSkillCount > 0 ? (
                    <Badge tone={SKILL_BADGE_TONES.update}>
                      {updateSkillCount} {updateSkillCount === 1 ? "update" : "updates"}
                    </Badge>
                  ) : null}
                  {item.id === "hooks" && hookReviewCount > 0 ? (
                    <Badge tone="warning">
                      {hookReviewCount} review{hookReviewCount === 1 ? "" : "s"}
                    </Badge>
                  ) : null}
                </span>
                <span className="overviewInventoryValue">
                  {overviewCountsLoaded.has(item.id) ? counts[item.id] : "—"}
                </span>
              </span>
              <ArrowUpRight size={14} aria-hidden="true" />
            </button>
          ))}
        </nav>

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
              <IconButton type="button" onClick={() => void loadAnalytics(true)} disabled={analyticsLoading || preparingAnalyticsRange} aria-label="Refresh analytics" aria-busy={analyticsLoading || preparingAnalyticsRange}>
                {analyticsLoading || preparingAnalyticsRange ? <LoadingIcon size={15} /> : <RefreshCw size={15} />}
              </IconButton>
            </div>
          </div>

          {analyticsRevisionError && !analytics ? (
            <>
              <div className="overviewAnalyticsEmpty">
                <strong>Analytics unavailable</strong>
                <p>No transcript analytics are available yet.</p>
                {onRetryAnalyticsRevision ? (
                  <Button type="button" variant="ghost" size="sm" className="overviewRetryButton" onClick={() => void onRetryAnalyticsRevision()}>
                    Retry analytics
                  </Button>
                ) : null}
              </div>
              <Toast tone="error" message={`Analytics refresh failed. ${analyticsRevisionError}`} />
            </>
          ) : showAnalyticsLoading ? (
            <AnalyticsLoadingState
              progress={analyticsProgress}
            />
          ) : null}
          {analytics && !preparingAnalyticsRange ? (
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
              {analytics.coverage.indexingSessions > 0 ? (
                <p className="overviewAnalyticsWarning">
                  Indexing {analytics.coverage.indexingSessions.toLocaleString()} older sessions in the background.
                </p>
              ) : null}
            </>
          ) : analyticsRevisionError ? null : showAnalyticsLoading ? null : (
            <>
              <div className="overviewAnalyticsEmpty">
                <strong>Analytics unavailable</strong>
                <p>{analyticsError ? "Analytics could not be loaded." : "No transcript analytics are available yet."}</p>
                <Button type="button" variant="ghost" size="sm" className="overviewRetryButton" onClick={() => void loadAnalytics(true)}>
                  Retry analytics
                </Button>
              </div>
              {analyticsError ? <Toast tone="error" message={`Analytics refresh failed. ${analyticsError}`} /> : null}
            </>
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
              <h2 id="overview-sessions-title" className="overviewPaneTitle">Recent sessions</h2>
              <button type="button" className="overviewPaneAction" onClick={() => onNavigate("sessions")}>
                Open sessions
              </button>
            </div>

            {sessionListStatus === "loading" && usage.recentSessions.length === 0 ? (
              <SessionsLoadingState />
            ) : usage.recentSessions.length > 0 ? (
              <div className="overviewSessionGrid" aria-label="Recent sessions">
                {usage.recentSessions.map((session) => {
                  const previewKey = `${session.agent}:${session.id}`;
                  const preview = summarizeSessionPreviewRecord(session);
                  const displayTitle = session.title;
                  return (
                    <button
                      key={previewKey}
                      type="button"
                      className="overviewSessionRow"
                      onClick={() => onOpenSession(session)}
                    >
                      <span className="overviewSessionRowHeader">
                        <span className="overviewSessionTitleLine">
                          <AgentBadge agent={session.agent} small />
                          <Tooltip content={formatSessionTitle(displayTitle)} onlyWhenTruncated>
                            <span className="overviewSessionRowTitle"><SessionTitleText interactive={false} value={displayTitle} /></span>
                          </Tooltip>
                        </span>
                        <span className="overviewSessionProject">{sessionProject(session)}</span>
                      </span>
                      <span className="overviewSessionMessage">
                        <span className="overviewSessionMessageLabel" role="img" aria-label="User message"><ArrowRight size={13} aria-hidden="true" /></span>
                        <span className="overviewSessionMessageText"><TranscriptLinkText interactive={false} value={preview.userLast} /></span>
                      </span>
                      <span className="overviewSessionMessage">
                        <span className="overviewSessionMessageLabel" role="img" aria-label="Agent reply"><ArrowLeft size={13} aria-hidden="true" /></span>
                        <span className="overviewSessionMessageText"><TranscriptLinkText interactive={false} value={preview.assistantLast} /></span>
                      </span>
                    </button>
                  );
                })}
              </div>
            ) : sessionListStatus === "error" ? (
              <Toast tone="error" message={sessionListError || "Could not load sessions. Try again."} />
            ) : (
              <p className="overviewQuiet">No sessions found for this agent.</p>
            )}
          </section>
        </div>
      </div>
    </section>
  );
}
