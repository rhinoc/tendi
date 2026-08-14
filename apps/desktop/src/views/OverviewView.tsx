import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ArrowLeft, ArrowRight, ArrowUpRight, Check, ChevronDown, RefreshCw } from "lucide-react";
import { DropdownMenu } from "radix-ui";

import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { TranscriptLinkText } from "../components/shared/TranscriptLinkText.tsx";
import { tokenToneClass } from "../lib/token-style.ts";
import { formatTokenCount } from "../lib/token-format.ts";
import { sessionProject, type SessionRecord } from "../lib/sessions.ts";
import { summarizeSessionUsage } from "../lib/overview.ts";
import { summarizeSessionPreviewRecord } from "../lib/session-preview.ts";
import {
  type AnalyticsGranularity,
  type AnalyticsRefreshProgress,
  type OverviewAnalytics,
} from "../lib/analytics.ts";
import { TauriCommand, safeInvoke } from "../lib/tauri.ts";
import { OverviewTrendChart, type OverviewUsageMetric } from "./OverviewTrendChart.tsx";
import type { SkillRecord } from "./SkillsView.tsx";
import "./OverviewView.css";

export type OverviewNavId = "skills" | "prompts" | "sessions" | "rules" | "hooks" | "mcp";

export type OverviewViewProps = {
  counts: Record<OverviewNavId, number>;
  hookReviewCount?: number;
  skills: SkillRecord[];
  sessions: SessionRecord[];
  analyticsRevision: number;
  analyticsRevisionReady: boolean;
  agentFilter: string;
  onNavigate: (id: OverviewNavId) => void;
  onOpenSession: (session: SessionRecord) => void;
};

const ANALYTICS_LOAD_STEPS = [30, 90, 182, 365] as const;
const MAX_ANALYTICS_DAYS = 365;
let retainedAnalyticsRange = 30;
let retainedAnalyticsGranularity: AnalyticsGranularity = "day";
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
    <section className="overviewTrendBlock" aria-label="Loading usage chart">
      <div className="overviewTrendLegend overviewTrendLegendEmpty" aria-hidden="true" />
      <div className="overviewAnalyticsLoadingDots" aria-hidden="true" />
      <div
        role="progressbar"
        aria-label={total ? `Analyzing ${completed} of ${total} sessions` : "Loading usage analytics"}
        aria-valuemin={0}
        aria-valuemax={total || undefined}
        aria-valuenow={total ? completed : undefined}
        className="overviewVisuallyHidden"
      />
    </section>
  );
}

export function OverviewView({
  counts,
  hookReviewCount = 0,
  skills,
  sessions,
  analyticsRevision,
  analyticsRevisionReady,
  agentFilter,
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
  const [granularity, setGranularity] = useState<AnalyticsGranularity>(retainedAnalyticsGranularity);
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
  const loadOlderAnalytics = useCallback(() => {
    setAnalyticsRange((current) => {
      if (current >= analyticsTargetRange) return current;
      const step = ANALYTICS_LOAD_STEPS.find((days) => days > current);
      const next = Math.min(step ?? analyticsTargetRange, analyticsTargetRange);
      retainedAnalyticsRange = next;
      return next;
    });
  }, [analyticsTargetRange]);
  const loadAnalytics = useCallback(async (refreshTranscripts = false, showLoading = refreshTranscripts) => {
    const request = ++analyticsRequestRef.current;
    const cacheKey = overviewAnalyticsCacheKey(agentFilter, analyticsRange, analyticsRevision);
    const cached = overviewAnalyticsCache.get(cacheKey);
    if (!refreshTranscripts && cached) {
      analyticsRef.current = cached;
      setAnalytics(cached);
      setAnalyticsLoading(false);
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
  }, [agentFilter, analyticsRange, analyticsRevision]);
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

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | undefined;
    void listen<AnalyticsRefreshProgress>("analytics://progress", ({ payload }) => {
      if (disposed) return;
      setAnalyticsProgress(payload);
      if (payload.error) setAnalyticsError(payload.error);
    }).then((dispose) => {
      if (disposed) dispose();
      else unlisten = dispose;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  const updateSkills = useMemo(
    () => skills.filter((skill) => skill.updateStatus === "update-available"),
    [skills],
  );

  const inventory: Array<{ id: OverviewNavId; label: string; primary?: boolean }> = [
    { id: "skills", label: "Skills", primary: true },
    { id: "sessions", label: "Sessions", primary: true },
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
              className={`overviewInventoryItem${item.primary ? " isPrimary" : ""}`}
              onClick={() => onNavigate(item.id)}
              aria-label={`${item.label}: ${counts[item.id].toLocaleString()}${item.id === "hooks" && hookReviewCount > 0 ? `, ${hookReviewCount} need review` : ""}. Open ${item.label}`}
            >
              <span>
                <span className="overviewInventoryLabelRow">
                  <span className="overviewInventoryLabel">{item.label}</span>
                  {item.id === "skills" && updateSkills.length > 0 ? (
                    <span className="overviewSkillUpdateBadge">
                      {updateSkills.length} {updateSkills.length === 1 ? "update" : "updates"}
                    </span>
                  ) : null}
                  {item.id === "hooks" && hookReviewCount > 0 ? (
                    <span className="overviewSkillUpdateBadge">
                      {hookReviewCount} review{hookReviewCount === 1 ? "" : "s"}
                    </span>
                  ) : null}
                </span>
                <span className="overviewInventoryValue">{counts[item.id]}</span>
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
              <DropdownMenu.Root>
                <DropdownMenu.Trigger asChild>
                  <button type="button" className="overviewMetricSelect" aria-label={`Usage metric: ${USAGE_METRIC_LABELS[usageMetric]}`}>
                    <span className="overviewMetricSelectInner">
                      <span>{USAGE_METRIC_LABELS[usageMetric]}</span>
                      <ChevronDown size={13} aria-hidden="true" />
                    </span>
                  </button>
                </DropdownMenu.Trigger>
                <DropdownMenu.Portal>
                  <DropdownMenu.Content className="skillMenuContent overviewMetricMenu" align="end" sideOffset={6}>
                    <DropdownMenu.RadioGroup
                      value={usageMetric}
                      onValueChange={(value) => {
                        const metric = value as OverviewUsageMetric;
                        retainedUsageMetric = metric;
                        setUsageMetric(metric);
                      }}
                    >
                      {USAGE_METRICS.map((value) => (
                        <DropdownMenu.RadioItem className="skillMenuItem overviewMetricMenuItem" key={value} value={value}>
                          <span className="overviewMetricMenuIndicator" aria-hidden="true">
                            <DropdownMenu.ItemIndicator><Check size={12} strokeWidth={2.5} /></DropdownMenu.ItemIndicator>
                          </span>
                          <span>{USAGE_METRIC_LABELS[value]}</span>
                        </DropdownMenu.RadioItem>
                      ))}
                    </DropdownMenu.RadioGroup>
                  </DropdownMenu.Content>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
              <div className="overviewGranularityControl" role="group" aria-label="Group chart by">
                {(["day", "week", "month"] as const).map((value) => (
                  <button
                    key={value}
                    type="button"
                    aria-pressed={granularity === value}
                    className={granularity === value ? "isActive" : undefined}
                    onClick={() => {
                      retainedAnalyticsGranularity = value;
                      setGranularity(value);
                    }}
                  >
                    {value === "day" ? "Day" : value === "week" ? "Week" : "Month"}
                  </button>
                ))}
              </div>
              <button type="button" className="iconButton" onClick={() => void loadAnalytics(true)} disabled={analyticsLoading} aria-label="Refresh analytics">
                {analyticsLoading ? <LoadingIcon size={15} /> : <RefreshCw size={15} />}
              </button>
            </div>
          </div>

          {analyticsLoading && !analytics ? (
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
              />
              {analytics.warnings.length ? <p className="overviewAnalyticsWarning">{analytics.warnings.length} transcript files could not be fully analyzed.</p> : null}
              {analytics.coverage.indexingSessions > 0 ? (
                <p className="overviewAnalyticsWarning">
                  Indexing {analytics.coverage.indexingSessions.toLocaleString()} older sessions in the background.
                </p>
              ) : null}
            </>
          ) : analyticsLoading ? null : (
            <div className="overviewAnalyticsEmpty">
              <strong>Analytics unavailable</strong>
              <p>{analyticsError || "No transcript analytics are available yet."}</p>
              <button type="button" className="overviewRetryButton" onClick={() => void loadAnalytics(true)}>
                Retry analytics
              </button>
            </div>
          )}
          {analyticsError && analytics ? (
            <p className="overviewAnalyticsWarning" role="alert">Refresh failed. Existing analytics are still shown. {analyticsError}</p>
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

            {usage.recentSessions.length > 0 ? (
              <div className="overviewSessionGrid" aria-label="Recent sessions">
                {usage.recentSessions.map((session) => {
                  const previewKey = `${session.agent ?? ""}:${session.id ?? ""}`;
                  const preview = summarizeSessionPreviewRecord(session) ?? {
                    title: session.title || session.id || "Untitled session",
                    userLast: "—",
                    assistantLast: "—",
                  };
                  return (
                    <button
                      key={previewKey}
                      type="button"
                      className="overviewSessionRow"
                      onClick={() => onOpenSession(session)}
                    >
                      <span className="overviewSessionRowHeader">
                        <span className="overviewSessionTitleLine">
                          <AgentBadge agent={session.agent || "Unknown"} small />
                          <Tooltip content={preview.title} onlyWhenTruncated>
                            <span className="overviewSessionRowTitle"><TranscriptLinkText interactive={false} value={preview.title} /></span>
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
            ) : (
              <p className="overviewQuiet">No sessions found for this agent.</p>
            )}
          </section>
        </div>
      </div>
    </section>
  );
}
