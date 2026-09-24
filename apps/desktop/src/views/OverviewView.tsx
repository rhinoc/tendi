import {
  memo,
  useEffect,
  useMemo,
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
  selectAnalyticsGranularity,
} from "../lib/analytics.ts";
import { useOverviewAnalytics } from "../features/overview/use-overview-analytics.ts";
import { OpenInEditorMenuItem } from "../components/shared/DataTableMenus.tsx";
import { OverviewTrendChart, OverviewUsageMetric } from "./OverviewTrendChart.tsx";
import { DOMAIN_NAV_ITEMS, EMPTY_DISPLAY_VALUE, RuntimeDomainKey } from "../lib/index.ts";
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
  const {
    analytics,
    analyticsError,
    analyticsLoading,
    analyticsProgress,
    analyticsRevisionError,
    analyticsRange,
    loadingOlder: loadingOlderAnalytics,
    hasOlder: hasOlderAnalytics,
    loadOlderAnalytics,
    loadAnalytics,
  } = useOverviewAnalytics({
    agentFilter,
  });
  const automaticGranularity = selectAnalyticsGranularity(analytics?.days.length ?? analyticsRange);
  const [granularityOverride, setGranularityOverride] = useState<AnalyticsGranularity | null>(null);
  const granularity = granularityOverride ?? automaticGranularity;
  const [usageMetric, setUsageMetric] = useState<OverviewUsageMetric>(retainedUsageMetric);
  const showAnalyticsLoading = !analyticsRevisionError && analyticsLoading && !analytics;
  const analyticsRefreshing = analyticsLoading
    || analyticsProgress?.running === true;
  useEffect(() => {
    setGranularityOverride(null);
  }, [automaticGranularity]);

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
