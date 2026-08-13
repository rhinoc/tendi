import { Tooltip } from "../components/shared/Tooltip.tsx";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
} from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ArrowUpRight, RefreshCw } from "lucide-react";

import { ContentTopDragStrip } from "../components/shared/ContentTopDragStrip.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { cacheRateTone, tokenToneClass } from "../lib/token-style.ts";
import { formatTokenCount } from "../lib/tokenizer.ts";
import { sessionCacheRate, type SessionRecord } from "../lib/sessions.ts";
import { summarizeSessionUsage, type TokenMix } from "../lib/overview.ts";
import {
  analyticsHeatLevels,
  type AnalyticsGranularity,
  type AnalyticsRefreshProgress,
  type AnalyticsRankItem,
  type OverviewAnalytics,
} from "../lib/analytics.ts";
import { TauriCommand, safeInvoke } from "../lib/tauri.ts";
import { OverviewTrendChart } from "./OverviewTrendChart.tsx";
import type { SkillRecord } from "./SkillsView.tsx";
import "./OverviewView.css";

export type OverviewNavId = "skills" | "prompts" | "sessions" | "rules" | "hooks" | "mcp";

export type OverviewViewProps = {
  counts: Record<OverviewNavId, number>;
  skills: SkillRecord[];
  sessions: SessionRecord[];
  sessionsLoading: boolean;
  agentFilter: string;
  checkingUpdates?: boolean;
  applyingUpdates?: boolean;
  updateError?: string;
  onNavigate: (id: OverviewNavId) => void;
  onOpenSkill: (skill: SkillRecord) => void;
  onOpenSession: (session: SessionRecord) => void;
  onCheckUpdates: () => void | Promise<void>;
  onApplyUpdates: (names: string[]) => void;
};

const ANALYTICS_RANGES = [30, 90, 182, 365] as const;
let retainedAnalyticsRange = 182;
let retainedAnalyticsGranularity: AnalyticsGranularity = "week";
const overviewAnalyticsCache = new Map<string, OverviewAnalytics>();
const overviewAnalyticsQueries = new Map<string, Promise<OverviewAnalytics | null>>();
let refreshedAnalyticsSessionRevision = "";

function overviewAnalyticsCacheKey(agent: string, days: number, sessionRevision: string) {
  return `${agent}:${days}:${sessionRevision}`;
}

function AnalyticsProgressStatus({ progress }: { progress: AnalyticsRefreshProgress | null }) {
  const completed = progress?.completed ?? 0;
  const total = progress?.total ?? 0;
  const percent = total ? Math.min(100, Math.round((completed / total) * 100)) : 0;
  return (
    <div className="overviewAnalyticsProgress" aria-live="polite">
      <div className="overviewAnalyticsProgressLabel">
        <span>{total ? `Analyzing ${completed.toLocaleString()} / ${total.toLocaleString()} sessions` : "Loading analytics…"}</span>
        {total ? <strong>{percent}%</strong> : null}
      </div>
      <div
        className="overviewAnalyticsProgressTrack"
        role="progressbar"
        aria-label="Transcript analysis progress"
        aria-valuemin={0}
        aria-valuemax={total || undefined}
        aria-valuenow={total ? completed : undefined}
      >
        <span style={{ width: `${percent}%` }} />
      </div>
      {progress ? (
        <span className="overviewAnalyticsProgressDetail">
          {progress.parsed.toLocaleString()} updated · {progress.skipped.toLocaleString()} cached
          {progress.failed ? ` · ${progress.failed.toLocaleString()} failed` : ""}
        </span>
      ) : null}
    </div>
  );
}

function AnalyticsHeatmap({ analytics }: { analytics: OverviewAnalytics }) {
  const days = useMemo(
    () => analytics.days.slice(-Math.min(182, analytics.daysRequested)),
    [analytics.days, analytics.daysRequested],
  );
  const levels = useMemo(() => analyticsHeatLevels(days), [days]);
  const first = days[0]?.date;
  const leading = first ? (new Date(`${first}T00:00:00Z`).getUTCDay() + 6) % 7 : 0;
  const weekCount = Math.ceil((leading + days.length) / 7);
  const monthLabels = useMemo(() => {
    const formatter = new Intl.DateTimeFormat(undefined, { month: "short", timeZone: "UTC" });
    const labels: Array<{ key: string; label: string; column: number }> = [];
    let previousMonth = "";
    days.forEach((day, index) => {
      const month = day.date.slice(0, 7);
      if (month === previousMonth) return;
      previousMonth = month;
      const marker = {
        key: month,
        label: formatter.format(new Date(`${day.date}T00:00:00Z`)),
        column: Math.floor((leading + index) / 7),
      };
      const previous = labels[labels.length - 1];
      if (previous && marker.column - previous.column < 2) {
        if (labels.length === 1) labels[0] = marker;
        return;
      }
      labels.push(marker);
    });
    return labels;
  }, [days, leading]);
  const activeDays = days.filter((day) => day.usage.totalTokens > 0).length;
  const totalTokens = days.reduce((sum, day) => sum + day.usage.totalTokens, 0);

  return (
    <section className="overviewHeatmapBlock" aria-labelledby="overview-activity-title">
      <div className="overviewSectionHeader">
        <div>
          <h3 id="overview-activity-title">Daily activity</h3>
          <p>{days.length ? `${days.length} days` : "Selected range"}</p>
        </div>
        {days.length ? (
          <strong className="overviewActivitySummary">
            {activeDays}/{days.length} active
          </strong>
        ) : null}
      </div>
      {days.length ? (
        <>
          <div className="overviewHeatmapViewport">
            <div
              className="overviewHeatmapChart"
              style={{ "--overview-heatmap-weeks": weekCount } as CSSProperties}
            >
              <div className="overviewHeatmapMonths" aria-hidden="true">
                {monthLabels.map((month) => (
                  <span key={month.key} style={{ gridColumnStart: month.column + 1 }}>{month.label}</span>
                ))}
              </div>
              <div className="overviewHeatmapWeekdays" aria-hidden="true">
                <span>Mon</span><span>Wed</span><span>Fri</span>
              </div>
              <div
                className="overviewHeatmap"
                role="img"
                aria-label={`${activeDays} active days out of ${days.length}, ${totalTokens.toLocaleString()} total tokens`}
              >
                {Array.from({ length: leading }, (_, index) => (
                  <span key={`empty-${index}`} className="overviewHeatCell isEmpty" />
                ))}
                {days.map((day) => (
                  <Tooltip key={day.date} content={`${day.date}: ${day.usage.totalTokens.toLocaleString()} tokens`}><span
                    key={day.date}
                    className={`overviewHeatCell level${levels.get(day.date) ?? 0}`}
                  /></Tooltip>
                ))}
              </div>
            </div>
          </div>
          <p className="overviewHeatmapScrollHint">Scroll horizontally to view all days.</p>
          <div className="overviewHeatLegend" aria-hidden="true">
            <span>Less</span>
            {[0, 1, 2, 3, 4].map((level) => (
              <span key={level} className={`overviewHeatCell level${level}`} />
            ))}
            <span>More</span>
          </div>
        </>
      ) : (
        <p className="overviewQuiet">No token activity in this range.</p>
      )}
    </section>
  );
}

function RankList({ title, rows }: { title: string; rows: AnalyticsRankItem[] }) {
  return (
    <section className="overviewRankPane">
      <h3>{title}</h3>
      {rows.length ? (
        <table className="overviewRankTable">
          <thead>
            <tr><th>Name</th><th>Calls</th><th>Sessions</th><th>Share</th></tr>
          </thead>
          <tbody>
            {rows.slice(0, 8).map((row) => (
              <tr key={`${row.server}:${row.name}`}>
                <Tooltip content={row.server ? `${row.server} · ${row.name}` : row.name} onlyWhenTruncated><td className="overviewRankName">
                  {row.server ? `${row.server} · ` : ""}{row.name}
                </td></Tooltip>
                <td>{row.calls.toLocaleString()}</td>
                <td>{row.sessions.toLocaleString()}</td>
                <td>{(row.share * 100).toFixed(1)}%</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : <p className="overviewQuiet">No observed calls in this range.</p>}
    </section>
  );
}

function rateWindowLabel(minutes: number): string {
  if (minutes % 1440 === 0) return `${minutes / 1440}d limit peak`;
  if (minutes % 60 === 0) return `${minutes / 60}h limit peak`;
  return `${minutes}m limit peak`;
}

function TokenMixDonut({ mix, totalTokens }: { mix: TokenMix; totalTokens: number }) {
  const mixTotal = mix.cachedInput + mix.uncachedInput + mix.output;
  if (mixTotal <= 0) return null;
  let offset = 0;
  const segments = [
    { key: "cached", label: "Cached", value: mix.cachedInput, className: "cached" },
    { key: "uncached", label: "Uncached", value: mix.uncachedInput, className: "uncached" },
    { key: "output", label: "Output", value: mix.output, className: "output" },
  ]
    .filter((segment) => segment.value > 0)
    .map((segment) => {
      const percentage = (segment.value / mixTotal) * 100;
      const result = { ...segment, percentage, offset };
      offset += percentage;
      return result;
    });

  return (
    <figure className="overviewSessionChart">
      <figcaption className="overviewSessionChartHeader">
        <span>Token mix</span>
        <strong className={tokenToneClass(totalTokens)}>{formatTokenCount(totalTokens)}</strong>
      </figcaption>
      <div className="overviewTokenDonutBody">
        <svg
          className="overviewTokenDonut"
          viewBox="0 0 76 76"
          role="img"
          aria-label={segments
            .map((segment) => `${segment.label} ${segment.percentage.toFixed(1)} percent`)
            .join(", ")}
        >
          <circle className="overviewTokenDonutTrack" cx="38" cy="38" r="30" pathLength="100" />
          {segments.map((segment) => (
            <circle
              key={segment.key}
              className={`overviewTokenDonutSegment ${segment.className}`}
              cx="38"
              cy="38"
              r="30"
              pathLength="100"
              strokeDasharray={`${segment.percentage} ${100 - segment.percentage}`}
              strokeDashoffset={-segment.offset}
            />
          ))}
        </svg>
        <div className="overviewTokenLegend">
          {segments.map((segment) => (
            <div key={segment.key} className="overviewTokenLegendRow">
              <span className={`overviewTokenLegendSwatch ${segment.className}`} aria-hidden="true" />
              <span>{segment.label}</span>
              <strong>{Math.round(segment.percentage)}%</strong>
            </div>
          ))}
        </div>
      </div>
    </figure>
  );
}

function CacheTrend({ rates }: { rates: number[] }) {
  if (rates.length === 0) return <p className="overviewQuiet">No cache-rate data.</p>;
  const width = 240;
  const height = 72;
  const plotWidth = 204;
  const padY = 7;
  const usable = height - padY * 2;
  const points = rates.map((rate, index) => {
    const x = rates.length === 1 ? plotWidth / 2 : (index / (rates.length - 1)) * plotWidth;
    const y = padY + usable * (1 - Math.min(100, Math.max(0, rate)) / 100);
    return { x, y };
  });
  const line = points.map((point, index) => `${index === 0 ? "M" : "L"}${point.x.toFixed(1)} ${point.y.toFixed(1)}`).join(" ");
  const area = `${line} L${plotWidth} ${height - padY} L0 ${height - padY} Z`;
  const latest = rates[rates.length - 1];

  return (
    <svg
      className="overviewSparkline"
      viewBox={`0 0 ${width} ${height}`}
      role="img"
      aria-label={`Cache rate across ${rates.length} recent sessions, latest ${latest.toFixed(1)} percent`}
    >
      {[100, 50, 0].map((rate) => {
        const y = padY + usable * (1 - rate / 100);
        return (
          <g key={rate} className="overviewCacheGuide">
            <line x1="0" x2={plotWidth} y1={y} y2={y} />
            <text x={width} y={y} dy="0.32em" textAnchor="end">{rate}%</text>
          </g>
        );
      })}
      {points.length > 1 ? <path className="overviewCacheArea" d={area} /> : null}
      {points.length > 1 ? <path className="overviewCacheLine" d={line} /> : null}
      {points.map((point, index) => (
        <circle
          key={`${point.x}:${point.y}`}
          className={index === points.length - 1 ? "overviewCachePoint latest" : "overviewCachePoint"}
          cx={point.x}
          cy={point.y}
          r={index === points.length - 1 ? 3.25 : 2.25}
        />
      ))}
    </svg>
  );
}

function MiniCacheBar({ rate }: { rate: number }) {
  const tone = cacheRateTone(rate);
  return (
    <div className={`overviewMiniCache overviewTone-${tone}`} aria-hidden="true">
      <div className="overviewMiniCacheFill" style={{ width: `${Math.min(100, Math.max(0, rate))}%` }} />
    </div>
  );
}

export function OverviewView({
  counts,
  skills,
  sessions,
  sessionsLoading,
  agentFilter,
  checkingUpdates = false,
  applyingUpdates = false,
  updateError = "",
  onNavigate,
  onOpenSkill,
  onOpenSession,
  onCheckUpdates,
  onApplyUpdates,
}: OverviewViewProps) {
  const usage = useMemo(() => summarizeSessionUsage(sessions), [sessions]);
  const analyticsSessionRevision = `${sessions.length}:${sessions[0]?.updatedAt ?? sessions[0]?.time ?? ""}`;
  const initialAnalytics = overviewAnalyticsCache.get(
    overviewAnalyticsCacheKey(agentFilter, retainedAnalyticsRange, analyticsSessionRevision),
  ) ?? null;
  const [analytics, setAnalytics] = useState<OverviewAnalytics | null>(initialAnalytics);
  const [analyticsLoading, setAnalyticsLoading] = useState(!initialAnalytics);
  const [analyticsProgress, setAnalyticsProgress] = useState<AnalyticsRefreshProgress | null>(null);
  const [analyticsError, setAnalyticsError] = useState("");
  const [analyticsRange, setAnalyticsRange] = useState<number>(retainedAnalyticsRange);
  const [granularity, setGranularity] = useState<AnalyticsGranularity>(retainedAnalyticsGranularity);
  const analyticsRequestRef = useRef(0);
  const analyticsRef = useRef(analytics);
  analyticsRef.current = analytics;
  const rateLimitPeaks = useMemo(() => {
    const peaks = new Map<number, number>();
    for (const day of analytics?.days ?? []) {
      for (const [windowMinutes, usedPercent] of Object.entries(day.rateLimits)) {
        const window = Number(windowMinutes);
        peaks.set(window, Math.max(peaks.get(window) ?? 0, usedPercent));
      }
    }
    return [...peaks].sort((left, right) => left[0] - right[0]);
  }, [analytics]);
  const loadAnalytics = useCallback(async (refreshTranscripts = false, showLoading = refreshTranscripts) => {
    const request = ++analyticsRequestRef.current;
    const cacheKey = overviewAnalyticsCacheKey(agentFilter, analyticsRange, analyticsSessionRevision);
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
      if (refreshTranscripts) refreshedAnalyticsSessionRevision = analyticsSessionRevision;
      overviewAnalyticsCache.set(cacheKey, result);
      analyticsRef.current = result;
      setAnalytics(result);
    }
    else setAnalyticsError((current) => current || "Analytics could not be loaded");
    setAnalyticsLoading(false);
  }, [agentFilter, analyticsRange, analyticsSessionRevision]);
  useEffect(() => {
    if (sessionsLoading) return;
    const cacheKey = overviewAnalyticsCacheKey(agentFilter, analyticsRange, analyticsSessionRevision);
    const cached = overviewAnalyticsCache.get(cacheKey);
    if (cached) {
      analyticsRef.current = cached;
      setAnalytics(cached);
      setAnalyticsLoading(false);
    }
    const refreshTranscripts = refreshedAnalyticsSessionRevision !== analyticsSessionRevision;
    void loadAnalytics(refreshTranscripts, !cached && analyticsRef.current === null);
  }, [agentFilter, analyticsRange, analyticsSessionRevision, loadAnalytics, sessionsLoading]);

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
              aria-label={`${item.label}: ${counts[item.id].toLocaleString()}. Open ${item.label}`}
            >
              <span>
                <span className="overviewInventoryLabel">{item.label}</span>
                <span className="overviewInventoryValue">{counts[item.id]}</span>
              </span>
              <ArrowUpRight size={14} aria-hidden="true" />
            </button>
          ))}
        </nav>

        <section className="overviewAnalytics" aria-labelledby="overview-analytics-title">
          <div className="overviewAnalyticsHeader">
            <div>
              <h2 id="overview-analytics-title">Usage analytics</h2>
              <p>Local transcript data. Tokens are usage counts, not estimated cost.</p>
            </div>
            <div className="overviewAnalyticsControls">
              <label className="overviewRangeControl">
                <span>Range</span>
                <select value={analyticsRange} onChange={(event) => {
                  const days = Number(event.target.value);
                  retainedAnalyticsRange = days;
                  setAnalyticsRange(days);
                }} disabled={analyticsLoading}>
                  {ANALYTICS_RANGES.map((days) => <option key={days} value={days}>{days} days</option>)}
                </select>
              </label>
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
                <RefreshCw size={15} className={analyticsLoading ? "skillRefreshSpinning" : undefined} />
              </button>
            </div>
          </div>

          {analyticsLoading ? <AnalyticsProgressStatus progress={analyticsProgress} /> : null}
          {analytics ? (
            <>
              <dl className="overviewAnalyticsMetrics">
                <div><dt>Tokens</dt><dd>{formatTokenCount(analytics.summary.usage.totalTokens)}</dd></div>
                <div><dt>Responses</dt><dd>{analytics.summary.responses.toLocaleString()}</dd></div>
                <div><dt>Runs</dt><dd>{analytics.summary.runs.started.toLocaleString()}</dd></div>
                <div><dt>Abort rate</dt><dd>{(analytics.summary.abortedRate * 100).toFixed(1)}%</dd></div>
                <div><dt>Compactions</dt><dd>{analytics.summary.compacted.toLocaleString()}</dd></div>
                <div><dt>Analyzed</dt><dd>{analytics.coverage.analyzedSessions}/{analytics.coverage.totalSessions}</dd></div>
              </dl>
              <OverviewTrendChart analytics={analytics} granularity={granularity} />
              <div className="overviewAnalyticsSecondary">
                <AnalyticsHeatmap analytics={analytics} />
                <section className="overviewHealthBlock" aria-labelledby="overview-health-title">
                  <div className="overviewSectionHeader">
                    <div>
                      <h3 id="overview-health-title">Run health</h3>
                      <p>Within the selected range</p>
                    </div>
                  </div>
                  <dl className="overviewHealth">
                    <div><dt>Completed</dt><dd>{analytics.summary.runs.completed.toLocaleString()}</dd></div>
                    <div><dt>Unclosed</dt><dd>{analytics.summary.runs.unclosed.toLocaleString()}</dd></div>
                    <div><dt>Aborted</dt><dd>{analytics.summary.aborted.toLocaleString()}</dd></div>
                    <div><dt>Compacted sessions</dt><dd>{analytics.summary.compactedSessions.toLocaleString()}</dd></div>
                    <div><dt>Longest run</dt><dd>{analytics.summary.runs.maxMs ? `${Math.round(analytics.summary.runs.maxMs / 1000)}s` : "—"}</dd></div>
                    {rateLimitPeaks.map(([windowMinutes, usedPercent]) => (
                      <div key={windowMinutes}><dt>{rateWindowLabel(windowMinutes)}</dt><dd>{usedPercent.toFixed(1)}%</dd></div>
                    ))}
                  </dl>
                </section>
              </div>
              <div className="overviewRanks">
                <RankList title={`Tools · ${analytics.rankDays} days`} rows={analytics.tools} />
                <RankList title={`Skills · ${analytics.rankDays} days`} rows={analytics.skills} />
              </div>
              {analytics.capabilities.some((capability) => !capability.tokenUsage) ? (
                <p className="overviewCapabilityNote">
                  “—” means the provider transcript does not expose that metric; it is not counted as zero.
                </p>
              ) : null}
              {analytics.warnings.length ? <p className="overviewAnalyticsWarning">{analytics.warnings.length} transcript files could not be fully analyzed.</p> : null}
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

            {usage.sessionsWithUsage > 0 ? (
              <div className="overviewSessionCharts">
                <TokenMixDonut mix={usage.mix} totalTokens={usage.totalTokens} />
                <figure className="overviewSessionChart">
                  <figcaption className="overviewSessionChartHeader">
                    <span>Cache rate</span>
                    <strong className={usage.averageCacheRate === undefined ? "" : `overviewTone-${cacheRateTone(usage.averageCacheRate)}`}>
                      {usage.averageCacheRate === undefined ? "—" : `${usage.averageCacheRate.toFixed(1)}%`}
                    </strong>
                  </figcaption>
                  <CacheTrend rates={usage.sparklineRates} />
                </figure>
              </div>
            ) : null}

            {usage.recentSessions.length > 0 ? (
              <div className="overviewList" aria-label="Recent sessions">
                {usage.recentSessions.map((session) => {
                  const rate = sessionCacheRate(session);
                  const tokens = session.tokenUsage?.totalTokens;
                  return (
                    <button
                      key={`${session.agent ?? ""}:${session.id}`}
                      type="button"
                      className="overviewRow"
                      onClick={() => onOpenSession(session)}
                    >
                      <span className="overviewRowMain">
                        <span className="overviewRowTitle">{session.title}</span>
                        <span className="overviewRowMeta">{session.agent}</span>
                      </span>
                      <span className="overviewRowTrailing">
                        <span className={`overviewRowStat ${tokens ? tokenToneClass(tokens) : ""}`}>
                          {tokens ? formatTokenCount(tokens) : "—"}
                        </span>
                        {rate === undefined ? (
                          <span className="overviewRowStat">—</span>
                        ) : (
                          <>
                            <span className={`overviewRowStat overviewTone-${cacheRateTone(rate)}`}>
                              {rate.toFixed(0)}%
                            </span>
                            <MiniCacheBar rate={rate} />
                          </>
                        )}
                      </span>
                    </button>
                  );
                })}
              </div>
            ) : (
              <p className="overviewQuiet">No sessions found for this agent.</p>
            )}
          </section>

          <section className="overviewPane" aria-labelledby="overview-skills-title">
            <div className="overviewPaneHeader">
              <h2 id="overview-skills-title" className="overviewPaneTitle">Skill updates</h2>
              <div className="overviewPaneActions">
                {updateSkills.length > 0 ? (
                  <button
                    type="button"
                    className="overviewUpdateAllButton"
                    disabled={applyingUpdates}
                    onClick={() => onApplyUpdates(updateSkills.map((skill) => skill.name))}
                  >
                    Update all
                  </button>
                ) : null}
                <button
                  type="button"
                  className="overviewPaneAction overviewCheckUpdatesButton"
                  onClick={() => void onCheckUpdates()}
                  disabled={checkingUpdates}
                >
                  <RefreshCw className={checkingUpdates ? "skillRefreshSpinning" : undefined} size={13} aria-hidden="true" />
                  Check updates
                </button>
                <button type="button" className="overviewPaneAction" onClick={() => onNavigate("skills")}>
                  Open skills
                </button>
              </div>
            </div>

            <div className="overviewMetrics">
              <div className="overviewMetric">
                <span className="overviewMetricLabel">Updates</span>
                <span className="overviewMetricValue">{updateSkills.length}</span>
              </div>
            </div>

            {updateSkills.length > 0 ? (
              <div className="overviewList" aria-label="Skills with updates">
                {updateSkills.map((skill) => (
                  <div key={skill.id} className="overviewSkillRow" role="group">
                    <button type="button" className="overviewSkillMain" onClick={() => onOpenSkill(skill)}>
                      <span className="overviewRowTitle">{skill.name}</span>
                    </button>
                    <button
                      type="button"
                      className="overviewSkillUpdateButton"
                      disabled={applyingUpdates}
                      onClick={() => onApplyUpdates([skill.name])}
                    >
                      Update
                    </button>
                  </div>
                ))}
              </div>
            ) : (
              <p className="overviewQuiet">{skills.length ? "No updates available." : "No skills installed."}</p>
            )}
            {updateError ? <p className="overviewUpdateError" role="alert">Unable to check skill updates. {updateError}</p> : null}
          </section>
        </div>
      </div>
    </section>
  );
}
