import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type WheelEvent } from "react";

import { ChartFrame } from "../components/shared/chart/ChartFrame.tsx";
import { ChartLegend, type ChartLegendItem } from "../components/shared/chart/ChartLegend.tsx";
import { ChartTooltipContent, type ChartTooltipDetail } from "../components/shared/chart/ChartTooltipContent.tsx";
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { useVirtualViewport } from "../components/shared/useVirtualViewport.ts";
import { agentDefinition, agentIdentityKey, agentDefinitions, EMPTY_DISPLAY_VALUE, formatDayGroupLabel, friendlyAgent, normalizedAgentKey } from "../lib/index.ts";
import { AnalyticsGranularity, groupAnalyticsDays, stepAnalyticsGranularity, type AnalyticsPeriod, type OverviewAnalytics } from "../lib/analytics.ts";
import { formatTokenCount } from "../lib/token-format.ts";
import { fixedVirtualRange } from "../lib/virtualization.ts";
import { trackpadZoomDirection, useTrackpadZoom } from "../lib/zoom-gesture.ts";
import { logger } from "../lib/logger.ts";

const MAX_RUNG_COUNT = 28;
const MAX_CATEGORY_COUNT = 4;
const TREND_COLUMN_WIDTH = 24;
const TREND_EDGE_PADDING = 32;
const TREND_WINDOW_OVERSCAN = 8;
const TREND_VIRTUALIZATION_LIMIT = 80;
const TREND_INITIAL_WINDOW_COLUMNS = 64;
const TREND_LABEL_TARGET_GAP = 84;
const TREND_DEFAULT_VIEWPORT_WIDTH = 640;
const TREND_PREFETCH_SCREEN_RATIO = 1.75;
const TREND_PLOT_HEIGHT = 184;
const CACHE_RATE_MAX = 100;
const CACHE_RATE_LOG_SCALE = Math.log1p(CACHE_RATE_MAX);

export enum OverviewUsageMetric {
  Sessions = "sessions",
  Turns = "turns",
  Tokens = "tokens",
  Cost = "cost",
  Cache = "cache",
  Time = "time",
  Tools = "tools",
  Skills = "skills",
}

export enum OverviewUsageGroup {
  Agent = "agent",
  Status = "status",
  Model = "model",
  Project = "project",
  Tool = "tool",
  Server = "server",
  Skill = "skill",
  Overall = "overall",
}

export const OVERVIEW_USAGE_GROUPS: Record<OverviewUsageMetric, readonly OverviewUsageGroup[]> = {
  [OverviewUsageMetric.Sessions]: [OverviewUsageGroup.Agent, OverviewUsageGroup.Project],
  [OverviewUsageMetric.Turns]: [
    OverviewUsageGroup.Agent,
    OverviewUsageGroup.Model,
    OverviewUsageGroup.Project,
    OverviewUsageGroup.Status,
  ],
  [OverviewUsageMetric.Tokens]: [OverviewUsageGroup.Agent, OverviewUsageGroup.Model, OverviewUsageGroup.Project],
  [OverviewUsageMetric.Cost]: [OverviewUsageGroup.Agent, OverviewUsageGroup.Model, OverviewUsageGroup.Project],
  [OverviewUsageMetric.Cache]: [
    OverviewUsageGroup.Overall,
    OverviewUsageGroup.Agent,
    OverviewUsageGroup.Model,
    OverviewUsageGroup.Project,
  ],
  [OverviewUsageMetric.Time]: [OverviewUsageGroup.Agent, OverviewUsageGroup.Model, OverviewUsageGroup.Project],
  [OverviewUsageMetric.Tools]: [
    OverviewUsageGroup.Agent,
    OverviewUsageGroup.Project,
    OverviewUsageGroup.Tool,
    OverviewUsageGroup.Server,
  ],
  [OverviewUsageMetric.Skills]: [OverviewUsageGroup.Agent, OverviewUsageGroup.Project, OverviewUsageGroup.Skill],
};

export const OVERVIEW_USAGE_GROUP_LABELS: Record<OverviewUsageGroup, string> = {
  [OverviewUsageGroup.Agent]: "by agent",
  [OverviewUsageGroup.Status]: "by status",
  [OverviewUsageGroup.Model]: "by model",
  [OverviewUsageGroup.Project]: "by project",
  [OverviewUsageGroup.Tool]: "by tool",
  [OverviewUsageGroup.Server]: "by server",
  [OverviewUsageGroup.Skill]: "by skill",
  [OverviewUsageGroup.Overall]: "Overall",
};

export const DEFAULT_OVERVIEW_USAGE_GROUPS: Record<OverviewUsageMetric, OverviewUsageGroup> = {
  [OverviewUsageMetric.Sessions]: OverviewUsageGroup.Agent,
  [OverviewUsageMetric.Turns]: OverviewUsageGroup.Status,
  [OverviewUsageMetric.Tokens]: OverviewUsageGroup.Model,
  [OverviewUsageMetric.Cost]: OverviewUsageGroup.Model,
  [OverviewUsageMetric.Cache]: OverviewUsageGroup.Overall,
  [OverviewUsageMetric.Time]: OverviewUsageGroup.Model,
  [OverviewUsageMetric.Tools]: OverviewUsageGroup.Tool,
  [OverviewUsageMetric.Skills]: OverviewUsageGroup.Skill,
};

type TrendSegment = {
  key: string;
  label: string;
  value: number;
  className: string;
};

type BreakdownItem = {
  key: string;
  label: string;
  value: number;
};

type TrendRung = {
  key: string;
  className: string;
  path: string;
};

type TrendPeriodModel = {
  period: AnalyticsPeriod;
  index: number;
  total: number;
  totalRungs: number;
  segments: TrendSegment[];
  rungs: TrendRung[];
  tooltipSegments: Array<BreakdownItem & { className: string }>;
};

type ScrollSnapshot = {
  firstKey: string;
  left: number;
  viewKey: string;
  width: number;
};

type TrendWindow = {
  start: number;
  end: number;
};

type TrendLinePoint = {
  x: number;
  y: number;
};

function periodStartTimestamp(key: string, granularity: AnalyticsGranularity): number {
  const normalizedKey = granularity === AnalyticsGranularity.Month ? `${key}-01` : key;
  return new Date(`${normalizedKey}T00:00:00`).getTime();
}

function periodIndexAtTimestamp(
  periods: AnalyticsPeriod[],
  timestamp: number,
  granularity: AnalyticsGranularity,
): number {
  if (!periods.length || !Number.isFinite(timestamp)) return -1;
  let candidate = 0;
  for (let index = 0; index < periods.length; index += 1) {
    const start = periodStartTimestamp(periods[index].key, granularity);
    if (timestamp < start) break;
    candidate = index;
    const nextStart = periods[index + 1]
      ? periodStartTimestamp(periods[index + 1].key, granularity)
      : Number.POSITIVE_INFINITY;
    if (timestamp < nextStart) return index;
  }
  return candidate;
}

function apportionRungs(values: number[], totalRungs: number): number[] {
  const total = values.reduce((sum, value) => sum + value, 0);
  if (total <= 0 || totalRungs <= 0) return values.map(() => 0);

  const exact = values.map((value) => value / total * totalRungs);
  const counts = exact.map(Math.floor);
  const order = exact
    .map((value, index) => ({ index, remainder: value - counts[index] }))
    .sort((left, right) => right.remainder - left.remainder || left.index - right.index);
  let remaining = totalRungs - counts.reduce((sum, value) => sum + value, 0);
  for (let index = 0; remaining > 0; index += 1, remaining -= 1) {
    counts[order[index].index] += 1;
  }
  return counts;
}

function rungWidth(periodIndex: number, rungIndex: number, segmentIndex: number): number {
  const seed = Math.abs(((periodIndex + 1) * 73856093) ^ ((rungIndex + 3) * 19349663) ^ ((segmentIndex + 5) * 83492791));
  return 74 + (seed % 27);
}

function cacheRate(period: AnalyticsPeriod): number | null {
  if (period.inputTokens <= 0) return null;
  return period.cachedInputTokens / period.inputTokens * 100;
}

function projectCacheRate(project: AnalyticsPeriod["projects"][number] | undefined): number | null {
  if (!project || project.usage.inputTokens <= 0) return null;
  return project.usage.cachedInputTokens / project.usage.inputTokens * 100;
}

function modelCacheRate(model: AnalyticsPeriod["models"][number] | undefined): number | null {
  if (!model || model.inputTokens <= 0) return null;
  return model.cachedInputTokens / model.inputTokens * 100;
}

function agentCacheRate(period: AnalyticsPeriod, agent: string): number | null {
  const usage = period.agents.find((item) => item.agent === agent)?.usage;
  if (!usage || usage.inputTokens <= 0) return null;
  return usage.cachedInputTokens / usage.inputTokens * 100;
}

function otherProjectCacheRate(period: AnalyticsPeriod, topProjectIds: ReadonlySet<string>): number | null {
  let inputTokens = period.inputTokens;
  let cachedInputTokens = period.cachedInputTokens;
  for (const project of period.projects) {
    if (!topProjectIds.has(project.id)) continue;
    inputTokens -= project.usage.inputTokens;
    cachedInputTokens -= project.usage.cachedInputTokens;
  }
  if (inputTokens <= 0) return null;
  return Math.max(0, cachedInputTokens) / inputTokens * 100;
}

function otherModelCacheRate(period: AnalyticsPeriod, topModelIds: ReadonlySet<string>): number | null {
  let inputTokens = period.inputTokens;
  let cachedInputTokens = period.cachedInputTokens;
  for (const model of period.models) {
    if (!topModelIds.has(model.model)) continue;
    inputTokens -= model.inputTokens;
    cachedInputTokens -= model.cachedInputTokens;
  }
  if (inputTokens <= 0) return null;
  return Math.max(0, cachedInputTokens) / inputTokens * 100;
}

function otherAgentCacheRate(period: AnalyticsPeriod, topAgentKeys: ReadonlySet<string>): number | null {
  let inputTokens = period.inputTokens;
  let cachedInputTokens = period.cachedInputTokens;
  for (const agent of period.agents) {
    if (!topAgentKeys.has(`agent:${agent.agent}`)) continue;
    inputTokens -= agent.usage.inputTokens;
    cachedInputTokens -= agent.usage.cachedInputTokens;
  }
  if (inputTokens <= 0) return null;
  return Math.max(0, cachedInputTokens) / inputTokens * 100;
}

function tokensPerResponse(period: AnalyticsPeriod): number | null {
  if (period.responses <= 0) return null;
  return period.totalTokens / period.responses;
}

function averageTurnMs(period: AnalyticsPeriod): number | null {
  if (period.timedCompletedRuns <= 0) return null;
  return period.totalRunMs / period.timedCompletedRuns;
}

function formatDuration(milliseconds: number): string {
  const value = Math.max(0, Math.round(milliseconds));
  if (value < 1000) return `${value}ms`;
  const seconds = value / 1000;
  if (seconds < 60) return `${seconds.toFixed(seconds < 10 ? 1 : 0).replace(/\.0$/, "")}s`;
  const minutes = Math.floor(value / 60_000);
  const remainingSeconds = Math.floor(value / 1000) % 60;
  if (minutes < 60) return `${minutes}m${remainingSeconds ? ` ${remainingSeconds}s` : ""}`;
  const hours = Math.floor(minutes / 60);
  const remainingMinutes = minutes % 60;
  return `${hours}h${remainingMinutes ? ` ${remainingMinutes}m` : ""}`;
}

const USD_FORMATTER = new Intl.NumberFormat(undefined, {
  style: "currency",
  currency: "USD",
  minimumFractionDigits: 2,
  maximumFractionDigits: 2,
});

function formatUsd(value: number): string {
  return USD_FORMATTER.format(Math.max(0, value));
}

function linearPlotPosition(value: number, max: number): number {
  if (max <= 0) return 0;
  return Math.max(0, Math.min(1, value / max));
}

function smoothTrendLineSegmentPath(points: TrendLinePoint[]): string {
  if (!points.length) return "";
  if (points.length === 1) return `M${points[0].x} ${points[0].y}`;

  let path = `M${points[0].x} ${points[0].y}`;
  for (let index = 0; index < points.length - 1; index += 1) {
    const previous = points[index - 1] ?? points[index];
    const current = points[index];
    const next = points[index + 1];
    const following = points[index + 2] ?? next;
    const controlOne = {
      x: current.x + (next.x - previous.x) / 6,
      y: current.y + (next.y - previous.y) / 6,
    };
    const controlTwo = {
      x: next.x - (following.x - current.x) / 6,
      y: next.y - (following.y - current.y) / 6,
    };
    path += ` C${controlOne.x} ${Math.max(0, Math.min(TREND_PLOT_HEIGHT, controlOne.y))} ${controlTwo.x} ${Math.max(0, Math.min(TREND_PLOT_HEIGHT, controlTwo.y))} ${next.x} ${next.y}`;
  }
  return path;
}

function trendLinePath(
  models: TrendPeriodModel[],
  valueForPeriod: (period: AnalyticsPeriod) => number | null,
  plotPosition: (value: number) => number,
): string {
  let path = "";
  let points: TrendLinePoint[] = [];
  const flush = () => {
    path += smoothTrendLineSegmentPath(points);
    points = [];
  };

  for (const [localIndex, model] of models.entries()) {
    const value = valueForPeriod(model.period);
    if (value === null) {
      flush();
      continue;
    }
    points.push({
      x: localIndex * TREND_COLUMN_WIDTH + TREND_COLUMN_WIDTH / 2,
      y: TREND_PLOT_HEIGHT - plotPosition(value) * TREND_PLOT_HEIGHT,
    });
  }
  flush();
  return path;
}

function formatTokensPerResponse(value: number): string {
  return formatTokenCount(value);
}

function cacheRatePlotPosition(rate: number): number {
  const clampedRate = Math.max(0, Math.min(CACHE_RATE_MAX, rate));
  return 1 - Math.log1p(CACHE_RATE_MAX - clampedRate) / CACHE_RATE_LOG_SCALE;
}

function cacheRateAxisValue(position: number): number {
  const clampedPosition = Math.max(0, Math.min(1, position));
  return CACHE_RATE_MAX - Math.expm1((1 - clampedPosition) * CACHE_RATE_LOG_SCALE);
}

function isCacheTurningPoint(periods: AnalyticsPeriod[], index: number): boolean {
  if (index <= 0 || index >= periods.length - 1) return false;
  const previous = cacheRate(periods[index - 1]);
  const current = cacheRate(periods[index]);
  const next = cacheRate(periods[index + 1]);
  if (previous === null || current === null || next === null) return false;
  return (current > previous && current > next) || (current < previous && current < next);
}

function callTotal(calls: AnalyticsPeriod["tools"]): number {
  return calls.reduce((sum, call) => sum + call.calls, 0);
}

function metricValue(period: AnalyticsPeriod, metric: OverviewUsageMetric): number {
  if (metric === OverviewUsageMetric.Sessions) return period.sessions;
  if (metric === OverviewUsageMetric.Turns) return period.runs;
  if (metric === OverviewUsageMetric.Cache) return cacheRate(period) ?? 0;
  if (metric === OverviewUsageMetric.Cost) return period.cost.totalUsd;
  if (metric === OverviewUsageMetric.Time) return period.totalRunMs;
  if (metric === OverviewUsageMetric.Tools) return callTotal(period.tools);
  if (metric === OverviewUsageMetric.Skills) return callTotal(period.skills);
  return period.totalTokens;
}

function formatMetricValue(value: number, metric: OverviewUsageMetric): string {
  if (metric === OverviewUsageMetric.Tokens) return formatTokenCount(value);
  if (metric === OverviewUsageMetric.Cost) return formatUsd(value);
  if (metric === OverviewUsageMetric.Cache) return `${value.toFixed(1)}%`;
  if (metric === OverviewUsageMetric.Time) return formatDuration(value);
  return value.toLocaleString();
}

function metricLabel(metric: OverviewUsageMetric, granularity?: AnalyticsGranularity): string {
  if (metric === OverviewUsageMetric.Sessions) return granularity && granularity !== AnalyticsGranularity.Day ? "peak daily sessions" : "active sessions";
  if (metric === OverviewUsageMetric.Cache) return "cache rate";
  if (metric === OverviewUsageMetric.Cost) return "estimated cost";
  if (metric === OverviewUsageMetric.Time) return "total time";
  if (metric === OverviewUsageMetric.Tools) return "tool calls";
  if (metric === OverviewUsageMetric.Skills) return "skill uses";
  return metric;
}

const SESSION_AGENT_ORDER = [...agentDefinitions.map((definition) => definition.id), "unknown"];

function sessionAgentSort(left: string, right: string): number {
  const leftKey = agentIdentityKey(left);
  const rightKey = agentIdentityKey(right);
  const leftIndex = SESSION_AGENT_ORDER.indexOf(leftKey);
  const rightIndex = SESSION_AGENT_ORDER.indexOf(rightKey);
  return (leftIndex < 0 ? SESSION_AGENT_ORDER.length : leftIndex)
    - (rightIndex < 0 ? SESSION_AGENT_ORDER.length : rightIndex)
    || leftKey.localeCompare(rightKey);
}

function sessionAgentLabel(agent: string): string {
  return normalizedAgentKey(agent) === "shared" ? "Shared" : friendlyAgent(agent);
}

function sessionAgentClass(agent: string): string {
  return agentDefinition(normalizedAgentKey(agent))?.trendClass ?? "";
}

function sessionSegments(period: AnalyticsPeriod): TrendSegment[] {
  return Object.entries(period.sessionsByAgent)
    .filter(([, value]) => value > 0)
    .sort(([left], [right]) => sessionAgentSort(left, right))
    .map(([agent, value]) => ({
      key: `session-agent:${agent}`,
      label: sessionAgentLabel(agent),
      value,
      className: sessionAgentClass(agent),
    }));
}

function breakdownItems(
  period: AnalyticsPeriod,
  metric: OverviewUsageMetric,
  group: OverviewUsageGroup,
): BreakdownItem[] {
  if (group === OverviewUsageGroup.Model) {
    return period.models
      .filter((model) => metric !== OverviewUsageMetric.Turns || model.runs > 0)
      .map((model) => ({
        key: model.model,
        label: model.model,
        value: metric === OverviewUsageMetric.Turns
          ? model.runs
          : metric === OverviewUsageMetric.Cache
            ? model.inputTokens
            : metric === OverviewUsageMetric.Time
              ? model.totalMs
              : metric === OverviewUsageMetric.Cost
                ? model.cost.totalUsd
                : model.totalTokens,
      }));
  }
  if (group === OverviewUsageGroup.Project) {
    return period.projects
      .filter((project) => metric === OverviewUsageMetric.Turns
        ? project.runs > 0
        : metric === OverviewUsageMetric.Sessions
          ? (period.sessionsByProject[project.id] ?? 0) > 0
          : metric === OverviewUsageMetric.Cache
            ? project.usage.inputTokens > 0
            : metric === OverviewUsageMetric.Tools
              ? (period.toolsByProject[project.id] ?? 0) > 0
              : metric === OverviewUsageMetric.Skills
                ? (period.skillsByProject[project.id] ?? 0) > 0
                : true)
      .map((project) => ({
        key: project.id,
        label: project.name,
        value: metric === OverviewUsageMetric.Turns
          ? project.runs
          : metric === OverviewUsageMetric.Sessions
            ? period.sessionsByProject[project.id] ?? 0
            : metric === OverviewUsageMetric.Cache
              ? project.usage.inputTokens
              : metric === OverviewUsageMetric.Time
                ? project.totalMs
                : metric === OverviewUsageMetric.Cost
                  ? project.cost.totalUsd
                  : metric === OverviewUsageMetric.Tools
                    ? period.toolsByProject[project.id] ?? 0
                    : metric === OverviewUsageMetric.Skills
                      ? period.skillsByProject[project.id] ?? 0
                      : project.usage.totalTokens,
      }));
  }
  if (
    group === OverviewUsageGroup.Agent
    && metric !== OverviewUsageMetric.Sessions
  ) {
    if (metric === OverviewUsageMetric.Turns || metric === OverviewUsageMetric.Time) {
      const values = metric === OverviewUsageMetric.Turns ? period.runsByAgent : period.runMsByAgent;
      return Object.entries(values)
        .map(([agent, value]) => ({
          key: `agent:${agent}`,
          label: sessionAgentLabel(agent),
          value,
        }))
        .sort((left, right) => sessionAgentSort(left.key.slice("agent:".length), right.key.slice("agent:".length)));
    }
    if (metric === OverviewUsageMetric.Tools || metric === OverviewUsageMetric.Skills) {
      const values = metric === OverviewUsageMetric.Tools ? period.toolsByAgent : period.skillsByAgent;
      return Object.entries(values).map(([agent, value]) => ({
        key: `agent:${agent}`,
        label: sessionAgentLabel(agent),
        value,
      }));
    }
    return period.agents.map((agent) => ({
      key: `agent:${agent.agent}`,
      label: sessionAgentLabel(agent.agent),
      value: metric === OverviewUsageMetric.Cache
        ? agent.usage.inputTokens
        : metric === OverviewUsageMetric.Cost
          ? agent.cost.totalUsd
          : agent.usage.totalTokens,
    }));
  }
  if (group === OverviewUsageGroup.Tool && metric === OverviewUsageMetric.Tools) {
    return period.tools.map((call) => ({
      key: `${call.server}\0${call.name}`,
      label: call.server ? `${call.server} · ${call.name}` : call.name,
      value: call.calls,
    }));
  }
  if (group === OverviewUsageGroup.Server && metric === OverviewUsageMetric.Tools) {
    const servers = new Map<string, BreakdownItem>();
    for (const call of period.tools) {
      const key = call.server.trim();
      const current = servers.get(key);
      if (current) current.value += call.calls;
      else servers.set(key, { key: `server:${key}`, label: key || "Unknown server", value: call.calls });
    }
    return [...servers.values()];
  }
  if (group === OverviewUsageGroup.Skill && metric === OverviewUsageMetric.Skills) {
    return period.skills.map((call) => ({
      key: call.name,
      label: call.name,
      value: call.calls,
    }));
  }
  return [];
}

function hasCategoricalGroup(group: OverviewUsageGroup, metric: OverviewUsageMetric): boolean {
  return group === OverviewUsageGroup.Model
    || group === OverviewUsageGroup.Project
    || group === OverviewUsageGroup.Tool
    || group === OverviewUsageGroup.Server
    || group === OverviewUsageGroup.Skill
    || (group === OverviewUsageGroup.Agent && metric !== OverviewUsageMetric.Sessions);
}

function hasCacheCategoryGroup(group: OverviewUsageGroup): boolean {
  return group === OverviewUsageGroup.Agent
    || group === OverviewUsageGroup.Model
    || group === OverviewUsageGroup.Project;
}

function periodAriaLabel(
  period: AnalyticsPeriod,
  metric: OverviewUsageMetric,
  segments: TrendSegment[],
  dayCount: number,
  expectedDays: number,
  granularity: AnalyticsGranularity,
): string {
  const coverage = dayCount < expectedDays ? ` Partial period, ${dayCount} of ${expectedDays} days.` : "";
  const breakdown = segments
    .filter((segment) => segment.value > 0)
    .map((segment) => `${segment.label} ${formatMetricValue(segment.value, metric)}`)
    .join(", ");
  const peakDate = metric === OverviewUsageMetric.Sessions && granularity !== AnalyticsGranularity.Day && period.sessionPeakDate
    ? ` Peak day ${formatDayGroupLabel(period.sessionPeakDate)}.`
    : "";
  const value = metric === OverviewUsageMetric.Cache ? cacheRate(period) : metricValue(period, metric);
  const valueLabel = value === null ? "No data" : `${formatMetricValue(value, metric)} ${metricLabel(metric, granularity)}`;
  const averageTokens = metric === OverviewUsageMetric.Tokens ? tokensPerResponse(period) : null;
  const averageTurn = metric === OverviewUsageMetric.Time ? averageTurnMs(period) : null;
  const averageLabel = averageTokens === null
    ? averageTurn === null ? "" : ` Average ${formatDuration(averageTurn)} per turn.`
    : ` Average ${formatTokensPerResponse(averageTokens)} tokens per response.`;
  return `${period.label}: ${valueLabel}.${averageLabel}${coverage}${peakDate}${breakdown ? ` ${breakdown}.` : ""}`;
}

function turnSegments(period: AnalyticsPeriod): TrendSegment[] {
  const aborted = Math.min(period.aborted, period.unclosedRuns);
  return [
    { key: "completed", label: "Completed", value: period.completedRuns, className: "statusCompleted" },
    { key: "aborted", label: "Aborted", value: aborted, className: "statusAborted" },
    { key: "unclosed", label: "Other unclosed", value: Math.max(0, period.unclosedRuns - aborted), className: "statusUnclosed" },
  ];
}

function trendWindowForScroll(
  count: number,
  scrollLeft: number,
  viewportWidth: number,
): TrendWindow {
  if (count <= TREND_VIRTUALIZATION_LIMIT) return { start: 0, end: count };
  return fixedVirtualRange(
    count,
    Math.max(0, scrollLeft - TREND_EDGE_PADDING),
    Math.max(TREND_COLUMN_WIDTH, viewportWidth - TREND_EDGE_PADDING * 2),
    TREND_COLUMN_WIDTH,
    TREND_WINDOW_OVERSCAN,
  );
}

function initialTrendWindow(count: number): TrendWindow {
  if (count <= TREND_VIRTUALIZATION_LIMIT) return { start: 0, end: count };
  return {
    start: Math.max(0, count - TREND_INITIAL_WINDOW_COLUMNS - TREND_WINDOW_OVERSCAN),
    end: count,
  };
}

export function buildTrendTooltipSegments(
  period: AnalyticsPeriod,
  metric: OverviewUsageMetric,
  group: OverviewUsageGroup,
  topCategories: BreakdownItem[],
  segments: TrendSegment[],
): Array<BreakdownItem & { className: string }> {
  if (metric === OverviewUsageMetric.Cache) {
    if (!hasCacheCategoryGroup(group)) return [];
    const categoryIds = new Set(topCategories.map((category) => category.key));
    const groupedItems = topCategories.flatMap((category) => {
      const rate = group === OverviewUsageGroup.Project
        ? projectCacheRate(period.projects.find((project) => project.id === category.key))
        : group === OverviewUsageGroup.Model
          ? modelCacheRate(period.models.find((model) => model.model === category.key))
          : agentCacheRate(period, category.key.slice("agent:".length));
      return rate === null ? [] : [{ ...category, value: rate }];
    });
    const otherRate = group === OverviewUsageGroup.Project
      ? otherProjectCacheRate(period, categoryIds)
      : group === OverviewUsageGroup.Model
        ? otherModelCacheRate(period, categoryIds)
        : otherAgentCacheRate(period, categoryIds);
    if (otherRate !== null) groupedItems.push({ key: "ungrouped", label: "Other", value: otherRate });
    return groupedItems.map((item) => {
      const categoryIndex = topCategories.findIndex((category) => category.key === item.key);
      return { ...item, className: categoryIndex >= 0 ? `category${categoryIndex}` : "categoryOther" };
    });
  }
  if (hasCategoricalGroup(group, metric)) {
    const items = breakdownItems(period, metric, group).filter((item) => item.value > 0);
    if (items.length || metric !== OverviewUsageMetric.Time) {
      const groupedTotal = items.reduce((sum, item) => sum + item.value, 0);
      const ungroupedValue = Math.max(0, metricValue(period, metric) - groupedTotal);
      if (ungroupedValue > Math.max(1e-9, Math.abs(metricValue(period, metric)) * 1e-9)) {
        items.push({ key: "ungrouped", label: "Other", value: ungroupedValue });
      }
      return items
        .map((item) => {
          const categoryIndex = topCategories.findIndex((category) => category.key === item.key);
          return {
            ...item,
            className: categoryIndex >= 0 ? `category${categoryIndex}` : "categoryOther",
          };
        })
        .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label));
    }
  }
  return [...segments]
    .filter((segment) => segment.value > 0)
    .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label));
}

export function buildTrendPeriodModel(
  period: AnalyticsPeriod,
  index: number,
  metric: OverviewUsageMetric,
  group: OverviewUsageGroup,
  topCategories: BreakdownItem[],
  hasOtherCategories: boolean,
  rungUnit: number,
  options: { includeTooltip?: boolean } = {},
): TrendPeriodModel {
  // Rung geometry is only needed for the virtualized window. Tooltip rows are
  // optional so callers can skip them for off-screen / non-hovered periods.
  const includeTooltip = options.includeTooltip === true;
  const periodBreakdown = hasCategoricalGroup(group, metric)
    ? breakdownItems(period, metric, group)
    : [];
  const periodValues = new Map(periodBreakdown.map((item) => [item.key, item.value]));
  const knownValue = topCategories.reduce((sum, item) => sum + (periodValues.get(item.key) ?? 0), 0);
  const otherValue = Math.max(0, metricValue(period, metric) - knownValue);
  const segments: TrendSegment[] = metric === OverviewUsageMetric.Cache && group === OverviewUsageGroup.Project
    ? buildTrendTooltipSegments(period, metric, group, topCategories, [])
    : hasCategoricalGroup(group, metric)
    && (metric !== OverviewUsageMetric.Time || periodBreakdown.length > 0)
    ? [
        ...topCategories.map((item, categoryIndex) => ({
          key: item.key,
          label: item.label,
          value: periodValues.get(item.key) ?? 0,
          className: `category${categoryIndex}`,
        })),
        ...(hasOtherCategories ? [{ key: "other", label: "Other", value: otherValue, className: "categoryOther" }] : []),
      ]
    : metric === OverviewUsageMetric.Sessions
      ? sessionSegments(period)
        : metric === OverviewUsageMetric.Cache
          ? [{ key: "cache", label: "Cached input rate", value: cacheRate(period) ?? 0, className: "cacheRate" }]
          : metric === OverviewUsageMetric.Time
            ? [{ key: "total-time", label: "Total time", value: period.totalRunMs, className: "duration" }]
            : turnSegments(period);
  const total = metricValue(period, metric);
  const totalRungs = metric === OverviewUsageMetric.Cache ? 0 : total ? Math.max(1, Math.round(total / rungUnit)) : 0;
  const segmentRungs = metric === OverviewUsageMetric.Cache ? [] : apportionRungs(segments.map((segment) => segment.value), totalRungs);
  let rungOffset = 0;
  const rungs = segmentRungs.flatMap((count, segmentIndex) => {
    const commands = Array.from({ length: count }, (_, rungIndex) => {
      const width = rungWidth(index, rungIndex, segmentIndex);
      const y = totalRungs - rungOffset - rungIndex - 0.5;
      const start = (100 - width) / 2;
      return `M ${start} ${y} H ${start + width}`;
    });
    rungOffset += count;
    return commands.length > 0 ? [{
      key: segments[segmentIndex].key,
      className: segments[segmentIndex].className,
      path: commands.join(" "),
    }] : [];
  });
  const tooltipSegments = includeTooltip
    ? buildTrendTooltipSegments(period, metric, group, topCategories, segments)
    : [];
  return { period, index, total, totalRungs, segments, rungs, tooltipSegments };
}

// Lieflat F7 · Stacked Rungs · templates/basics-gallery.html · "Where each region's revenue sits"
export const OverviewTrendChart = memo(function OverviewTrendChart({
  analytics,
  granularity,
  hasOlder,
  loadingOlder,
  metric,
  groupBy,
  onLoadOlder,
  onGranularityChange,
}: {
  analytics: OverviewAnalytics;
  granularity: AnalyticsGranularity;
  hasOlder: boolean;
  loadingOlder: boolean;
  metric: OverviewUsageMetric;
  groupBy: OverviewUsageGroup;
  onLoadOlder: (minimumRange?: number) => void;
  onGranularityChange?: (granularity: AnalyticsGranularity) => void;
}) {
  const chartDays = useMemo(() => {
    const firstAvailableDate = analytics.coverage.first;
    if (!firstAvailableDate) return analytics.days;
    return analytics.days.filter((day) => day.date >= firstAvailableDate);
  }, [analytics.coverage.first, analytics.days]);
  const periods = useMemo(() => groupAnalyticsDays(chartDays, granularity), [chartDays, granularity]);
  const visible = periods;
  const periodBreakdowns = useMemo(
    () => visible.map((period) => breakdownItems(period, metric, groupBy)),
    [groupBy, metric, visible],
  );
  const categoryTotals = useMemo(() => {
    const totals = new Map<string, BreakdownItem>();
    for (const items of periodBreakdowns) {
      for (const item of items) {
        const current = totals.get(item.key);
        if (current) current.value += item.value;
        else totals.set(item.key, { ...item });
      }
    }
    return totals;
  }, [periodBreakdowns]);
  const topCategories = useMemo(
    () => [...categoryTotals.values()]
      .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label))
      .slice(0, MAX_CATEGORY_COUNT),
    [categoryTotals],
  );
  const topCategorySet = useMemo(() => new Set(topCategories.map((item) => item.key)), [topCategories]);
  const sessionAgentKeys = useMemo(() => {
    const keys = new Set<string>();
    for (const period of visible) {
      for (const agent of Object.keys(period.sessionsByAgent)) keys.add(agent);
    }
    return [...keys].sort(sessionAgentSort);
  }, [visible]);
  const expectedDays = (period: AnalyticsPeriod) => {
    if (granularity === AnalyticsGranularity.Day) return 1;
    if (granularity === AnalyticsGranularity.Week) return 7;
    const [year, month] = period.key.split("-").map(Number);
    return new Date(year, month, 0).getDate();
  };
  const periodDayCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const day of chartDays) {
      const key = granularity === AnalyticsGranularity.Day
        ? day.date
        : granularity === AnalyticsGranularity.Month
          ? day.date.slice(0, 7)
          : groupAnalyticsDays([day], AnalyticsGranularity.Week)[0]?.key;
      if (key) counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }, [chartDays, granularity]);
  const hasOtherCategories = useMemo(
    () => hasCategoricalGroup(groupBy, metric) && visible.some((period, index) => {
      if (metric === OverviewUsageMetric.Cache && hasCacheCategoryGroup(groupBy)) {
        const knownInputTokens = topCategories.reduce((sum, item) => sum + (periodBreakdowns[index].find((entry) => entry.key === item.key)?.value ?? 0), 0);
        return period.inputTokens - knownInputTokens > Math.max(1e-9, period.inputTokens * 1e-9);
      }
      const values = new Map(periodBreakdowns[index].map((item) => [item.key, item.value]));
      const knownTotal = topCategories.reduce((sum, item) => sum + (values.get(item.key) ?? 0), 0);
      const total = metricValue(period, metric);
      return total - knownTotal > Math.max(1e-9, Math.abs(total) * 1e-9);
    }),
    [groupBy, metric, periodBreakdowns, topCategories, visible],
  );
  const hasMetricActivity = visible.some((period) => (
    metric === OverviewUsageMetric.Cache ? period.inputTokens > 0 : metricValue(period, metric) > 0
  ));
  const hasTimingSupport = analytics.capabilities.some((capability) => capability.duration);
  const max = metric === OverviewUsageMetric.Cache ? CACHE_RATE_MAX : Math.max(1, ...visible.map((period) => metricValue(period, metric)));
  const tokensPerResponseMax = Math.max(1, ...visible.map((period) => tokensPerResponse(period) ?? 0));
  const maxRungs = metric === OverviewUsageMetric.Tokens
    ? MAX_RUNG_COUNT
    : Math.min(MAX_RUNG_COUNT, Math.max(1, max));
  const rungUnit = max / maxRungs;
  const peakPeriod = visible.reduce((peak, period) => (
    metricValue(period, metric) > metricValue(peak, metric) ? period : peak
  ), visible[0]);
  const [activeKey, setActiveKey] = useState(visible[visible.length - 1]?.key ?? "");
  const [hoveredKey, setHoveredKey] = useState<string | null>(null);
  const [openTooltipKey, setOpenTooltipKey] = useState<string | null>(null);
  const barsRef = useRef<HTMLDivElement>(null);
  const loadRequestedRef = useRef(false);
  const automaticOlderLoadAttemptedRef = useRef(false);
  const scrollSnapshotRef = useRef<ScrollSnapshot | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const {
    size: trendViewportSize,
    scrollOffset: trendScrollLeft,
    scheduleScrollSync,
  } = useVirtualViewport<HTMLDivElement>(
    { width: 0, height: 0 },
    {
      ref: viewportRef,
      axis: "horizontal",
      refreshKey: `${metric}:${groupBy}:${granularity}:${visible[0]?.key ?? ""}:${visible.length}`,
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ width }) => width > 0,
      isEqual: (current, next) => current.width === next.width,
    },
  );
  const [trendWindow, setTrendWindow] = useState<TrendWindow>(() => initialTrendWindow(visible.length));
  const pendingFocusIndexRef = useRef<number | null>(null);
  const zoomScaleRef = useRef(1);
  const zoomFocusTimestampRef = useRef<number | null>(null);

  const windowStart = Math.min(trendWindow.start, Math.max(0, visible.length));
  const windowEnd = Math.max(windowStart, Math.min(trendWindow.end, visible.length));
  const windowed = visible.length > TREND_VIRTUALIZATION_LIMIT;
  const periodModels = useMemo(
    () => visible.slice(windowStart, windowEnd).map((period, localIndex) => buildTrendPeriodModel(
      period,
      windowStart + localIndex,
      metric,
      groupBy,
      topCategories,
      hasOtherCategories,
      rungUnit,
    )),
    [groupBy, hasOtherCategories, metric, rungUnit, topCategories, visible, windowStart, windowEnd],
  );

  const syncTrendWindow = useCallback((viewport: HTMLDivElement, scrollLeft = viewport.scrollLeft) => {
    const next = trendWindowForScroll(visible.length, scrollLeft, viewport.clientWidth);
    setTrendWindow((current) => current.start === next.start && current.end === next.end ? current : next);
  }, [visible.length]);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    syncTrendWindow(viewport, trendScrollLeft);
  }, [syncTrendWindow, trendScrollLeft, trendViewportSize.width]);

  useEffect(() => {
    const focusTimestamp = zoomFocusTimestampRef.current;
    if (focusTimestamp !== null) {
      const focusIndex = periodIndexAtTimestamp(visible, focusTimestamp, granularity);
      zoomFocusTimestampRef.current = null;
      if (focusIndex >= 0) {
        pendingFocusIndexRef.current = focusIndex;
        setActiveKey(visible[focusIndex].key);
        return;
      }
    }
    setActiveKey((current) => (
      visible.some((period) => period.key === current)
        ? current
        : visible[visible.length - 1]?.key ?? ""
    ));
  }, [granularity, visible]);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const viewKey = `${metric}:${groupBy}:${granularity}`;
    const firstKey = visible[0]?.key ?? "";
    const previous = scrollSnapshotRef.current;
    if (!previous || previous.viewKey !== viewKey) {
      const focusTimestamp = zoomFocusTimestampRef.current;
      const focusIndex = focusTimestamp === null
        ? -1
        : periodIndexAtTimestamp(visible, focusTimestamp, granularity);
      if (focusIndex >= 0) {
        pendingFocusIndexRef.current = focusIndex;
        const button = barsRef.current?.querySelector<HTMLButtonElement>(`[data-trend-index="${focusIndex}"]`);
        if (!button && windowed) {
          const maxScrollLeft = Math.max(0, viewport.scrollWidth - viewport.clientWidth);
          const targetScrollLeft = TREND_EDGE_PADDING + focusIndex * TREND_COLUMN_WIDTH
            - Math.max(0, (viewport.clientWidth - TREND_COLUMN_WIDTH) / 2);
          viewport.scrollLeft = Math.max(0, Math.min(maxScrollLeft, targetScrollLeft));
        }
      } else {
        viewport.scrollLeft = viewport.scrollWidth;
      }
    } else if (previous.firstKey !== firstKey) {
      viewport.scrollLeft = previous.left + Math.max(0, viewport.scrollWidth - previous.width);
    } else {
      viewport.scrollLeft = Math.min(previous.left, viewport.scrollWidth - viewport.clientWidth);
    }
    scrollSnapshotRef.current = {
      firstKey,
      left: viewport.scrollLeft,
      viewKey,
      width: viewport.scrollWidth,
    };
    syncTrendWindow(viewport);

    if (!loadingOlder) loadRequestedRef.current = false;
    const availableWidth = Math.max(1, viewport.clientWidth - TREND_EDGE_PADDING * 2);
    const periodsPerScreen = availableWidth / TREND_COLUMN_WIDTH;
    const targetPeriods = Math.ceil(periodsPerScreen * TREND_PREFETCH_SCREEN_RATIO);
    if (
      !automaticOlderLoadAttemptedRef.current
      && hasOlder
      && !loadingOlder
      && visible.length < targetPeriods
    ) {
      const daysPerPeriod = analytics.daysRequested / Math.max(1, visible.length);
      const targetRange = Math.ceil(targetPeriods * daysPerPeriod);
      automaticOlderLoadAttemptedRef.current = true;
      loadRequestedRef.current = true;
      logger.info("overview analytics viewport prefetch requested", {
        granularity,
        loadedPeriods: visible.length,
        targetPeriods,
        viewportWidth: viewport.clientWidth,
        requestedRange: targetRange,
      });
      onLoadOlder(targetRange);
    }
  }, [analytics.daysRequested, granularity, groupBy, hasOlder, loadingOlder, metric, onLoadOlder, trendViewportSize.width, visible, windowed]);

  useEffect(() => {
    const pendingIndex = pendingFocusIndexRef.current;
    if (pendingIndex === null) return;
    const button = barsRef.current?.querySelector<HTMLButtonElement>(`[data-trend-index="${pendingIndex}"]`);
    if (!button) return;
    pendingFocusIndexRef.current = null;
    button.focus();
  }, [granularity, trendWindow, visible]);

  const handleTrackpadZoom = useTrackpadZoom<HTMLDivElement>(({ factor }) => {
    if (!onGranularityChange || !visible.length) return;
    zoomScaleRef.current *= factor;
    const direction = trackpadZoomDirection(zoomScaleRef.current);
    if (direction === 0) return;
    zoomScaleRef.current = 1;
    const nextGranularity = stepAnalyticsGranularity(granularity, direction);
    if (nextGranularity === granularity) return;

    const focusedKey = hoveredKey ?? activeKey;
    const focusedPeriod = visible.find((period) => period.key === focusedKey);
    zoomFocusTimestampRef.current = focusedPeriod
      ? periodStartTimestamp(focusedPeriod.key, granularity)
      : null;
    if (direction === 1 && hasOlder && !loadingOlder && !loadRequestedRef.current) {
      loadRequestedRef.current = true;
      onLoadOlder();
    }
    onGranularityChange(nextGranularity);
  });

  const trendViewportWidth = trendViewportSize.width || TREND_DEFAULT_VIEWPORT_WIDTH;

  const handleViewportWheel = (event: WheelEvent<HTMLDivElement>) => {
    handleTrackpadZoom(event);
  };

  const handlePeriodClick = (period: AnalyticsPeriod) => {
    setActiveKey(period.key);
    if (!onGranularityChange || granularity === AnalyticsGranularity.Day) return;
    const nextGranularity = stepAnalyticsGranularity(granularity, -1);
    if (nextGranularity === granularity) return;
    zoomScaleRef.current = 1;
    zoomFocusTimestampRef.current = periodStartTimestamp(period.key, granularity);
    onGranularityChange(nextGranularity);
  };

  const handleViewportScroll = () => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    scheduleScrollSync();
    const snapshot = scrollSnapshotRef.current;
    if (snapshot) {
      snapshot.left = viewport.scrollLeft;
      snapshot.width = viewport.scrollWidth;
    }
    syncTrendWindow(viewport);
    if (viewport.scrollLeft > 48 || !hasOlder || loadingOlder || loadRequestedRef.current) return;
    loadRequestedRef.current = true;
    onLoadOlder();
  };

  if (!visible.length || !hasMetricActivity) {
    return (
      <ChartFrame
        ariaLabelledBy="overview-trend-title"
        legend={<ChartLegend items={[]} />}
        emptyState={(
          <div className={`overviewTrendPlotLayout${metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Time ? " hasSecondaryMetric" : ""}`}>
            <div className="overviewTrendYAxis" aria-hidden="true">
              <span>{EMPTY_DISPLAY_VALUE}</span>
              <span>{EMPTY_DISPLAY_VALUE}</span>
              <span>{EMPTY_DISPLAY_VALUE}</span>
            </div>
            <div className="overviewTrendViewport">
              <div
                className="overviewTrendCanvas"
                style={{ "--trend-columns": 1 } as CSSProperties}
              >
                <div className="overviewTrendGrid" aria-hidden="true"><span /><span /><span /></div>
                <div className="overviewTrendEmptyMessage">
                  <h3 id="overview-trend-title">
                    {metric === OverviewUsageMetric.Time && !hasTimingSupport ? "No timing data" : `No ${metricLabel(metric, granularity)} activity`}
                  </h3>
                    <p>
                      {metric === OverviewUsageMetric.Time && !hasTimingSupport
                        ? "This provider does not record assistant completion timestamps."
                        : "No activity in the selected range."}
                    </p>
                  </div>
              </div>
              <div className="overviewTrendXAxis" style={{ "--trend-columns": 1 } as CSSProperties} aria-hidden="true">
                <span />
              </div>
            </div>
            {metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Time ? (
              <div className="overviewTrendSecondaryYAxis" aria-hidden="true">
                <span>{EMPTY_DISPLAY_VALUE}</span>
                <span>{EMPTY_DISPLAY_VALUE}</span>
                <span>{EMPTY_DISPLAY_VALUE}</span>
              </div>
            ) : null}
          </div>
        )}
      >
        {null}
      </ChartFrame>
    );
  }

  const viewportColumns = Math.max(1, Math.floor(trendViewportWidth / TREND_COLUMN_WIDTH));
  const targetLabelCount = Math.max(4, Math.floor(trendViewportWidth / TREND_LABEL_TARGET_GAP));
  const labelStep = Math.max(1, Math.ceil(Math.min(visible.length, viewportColumns) / targetLabelCount));
  const moveSelection = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex = index;
    if (event.key === "ArrowLeft") nextIndex = Math.max(0, index - 1);
    else if (event.key === "ArrowRight") nextIndex = Math.min(visible.length - 1, index + 1);
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = visible.length - 1;
    else return;
    event.preventDefault();
    setActiveKey(visible[nextIndex].key);
    const button = barsRef.current?.querySelector<HTMLButtonElement>(`[data-trend-index="${nextIndex}"]`);
    if (button) {
      button.focus({ preventScroll: true });
      const viewport = viewportRef.current;
      if (viewport) {
        const viewportBounds = viewport.getBoundingClientRect();
        const buttonBounds = button.getBoundingClientRect();
        const nextLeft = buttonBounds.left < viewportBounds.left
          ? viewport.scrollLeft + buttonBounds.left - viewportBounds.left
          : buttonBounds.right > viewportBounds.right
            ? viewport.scrollLeft + buttonBounds.right - viewportBounds.right
            : viewport.scrollLeft;
        viewport.scrollTo({ left: Math.max(0, nextLeft), behavior: "auto" });
      }
    } else {
      pendingFocusIndexRef.current = nextIndex;
      viewportRef.current?.scrollTo({ left: TREND_EDGE_PADDING + nextIndex * TREND_COLUMN_WIDTH, behavior: "auto" });
    }
  };

  const legend: ChartLegendItem[] = metric === OverviewUsageMetric.Cache
    ? hasCacheCategoryGroup(groupBy)
      ? [
          ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
          ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
        ]
      : []
    : metric === OverviewUsageMetric.Tokens
    ? [
        ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
        ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
        { key: "tokensPerResponse", label: "Avg tokens / response", swatchClassName: "tokensPerResponse" },
      ]
    : metric === OverviewUsageMetric.Turns && hasCategoricalGroup(groupBy, metric)
      ? [
          ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
          ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
        ]
    : metric === OverviewUsageMetric.Time
      ? [
          ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
          ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
          { key: "averageTurnTime", label: "Avg turn time", swatchClassName: "averageTurnTime" },
        ]
    : metric === OverviewUsageMetric.Tools || metric === OverviewUsageMetric.Skills || metric === OverviewUsageMetric.Cost
    ? [
        ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
        ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
      ]
      : metric === OverviewUsageMetric.Sessions
        ? groupBy === OverviewUsageGroup.Project
          ? [
              ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
              ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
            ]
          : sessionAgentKeys.map((agent) => ({
              key: `session-agent:${agent}`,
              label: sessionAgentLabel(agent),
              swatchClassName: sessionAgentClass(agent),
            }))
      : [
          { key: "completed", label: "Completed", swatchClassName: "statusCompleted" },
          { key: "aborted", label: "Aborted", swatchClassName: "statusAborted" },
          { key: "unclosed", label: "Other unclosed", swatchClassName: "statusUnclosed" },
        ];
  const renderedModels = periodModels;
  const renderedPeriods = visible.slice(windowStart, windowEnd);
  const cacheLineSeries = metric !== OverviewUsageMetric.Cache
    ? []
    : hasCacheCategoryGroup(groupBy)
      ? [
          ...topCategories.map((category, categoryIndex) => ({
            key: category.key,
            className: `category${categoryIndex}`,
            path: trendLinePath(
              renderedModels,
              (period) => groupBy === OverviewUsageGroup.Project
                ? projectCacheRate(period.projects.find((project) => project.id === category.key))
                : groupBy === OverviewUsageGroup.Model
                  ? modelCacheRate(period.models.find((model) => model.model === category.key))
                  : agentCacheRate(period, category.key.slice("agent:".length)),
              cacheRatePlotPosition,
            ),
          })),
          ...(hasOtherCategories ? [{
            key: "other",
            className: "categoryOther",
            path: trendLinePath(renderedModels, (period) => groupBy === OverviewUsageGroup.Project
              ? otherProjectCacheRate(period, topCategorySet)
              : groupBy === OverviewUsageGroup.Model
                ? otherModelCacheRate(period, topCategorySet)
                : otherAgentCacheRate(period, topCategorySet), cacheRatePlotPosition),
          }] : []),
        ]
      : [{ key: "cache", className: "", path: trendLinePath(renderedModels, cacheRate, cacheRatePlotPosition) }];
  const tokensPerResponsePath = metric === OverviewUsageMetric.Tokens
    ? trendLinePath(
      renderedModels,
      tokensPerResponse,
      (value) => linearPlotPosition(value, tokensPerResponseMax),
    )
    : "";
  const averageTurnTimeMax = Math.max(1, ...visible.map((period) => averageTurnMs(period) ?? 0));
  const averageTurnTimePath = metric === OverviewUsageMetric.Time
    ? trendLinePath(
      renderedModels,
      averageTurnMs,
      (value) => linearPlotPosition(value, averageTurnTimeMax),
    )
    : "";
  const yAxisValues = metric === OverviewUsageMetric.Cache
    ? [CACHE_RATE_MAX, cacheRateAxisValue(0.5), 0]
    : [max, max / 2, 0];
  const secondaryYAxisValues = metric === OverviewUsageMetric.Tokens
    ? [tokensPerResponseMax, tokensPerResponseMax / 2, 0]
    : metric === OverviewUsageMetric.Time
      ? [averageTurnTimeMax, averageTurnTimeMax / 2, 0]
      : [];
  const windowStyle = windowed
    ? {
        minWidth: 0,
        gridTemplateColumns: `repeat(${Math.max(1, windowEnd - windowStart)}, minmax(${TREND_COLUMN_WIDTH}px, 1fr))`,
        width: `${Math.max(1, windowEnd - windowStart) * TREND_COLUMN_WIDTH}px`,
        marginInlineStart: `${windowStart * TREND_COLUMN_WIDTH}px`,
      }
    : undefined;

  return (
    <ChartFrame
      ariaLabel={`${metricLabel(metric, granularity)} trend grouped ${OVERVIEW_USAGE_GROUP_LABELS[groupBy].toLowerCase()}`}
      legend={metric === OverviewUsageMetric.Cache && !hasCacheCategoryGroup(groupBy) ? <ChartLegend items={[]} /> : (
        <ChartLegend
          items={legend.map((item) => ({
            ...item,
            swatchClassName: item.swatchClassName,
          }))}
          ariaLabel={`${metricLabel(metric, granularity)} chart legend`}
        />
      )}
    >
      <p id="overview-trend-instructions" className="overviewVisuallyHidden">
        {granularity === AnalyticsGranularity.Day ? null : "Click a period to zoom in. "}
        {groupBy === OverviewUsageGroup.Overall ? null : `${OVERVIEW_USAGE_GROUP_LABELS[groupBy]}. `}
        {metric === OverviewUsageMetric.Tokens ? "The line shows average tokens per response. " : null}
        {metric === OverviewUsageMetric.Time ? "The line shows average turn time. " : null}
        {metric === OverviewUsageMetric.Cache ? "Cache rate uses a logarithmic scale based on uncached input. " : null}
        Use Left and Right Arrow keys to inspect periods. Use Home and End to jump to the first or last period.
      </p>
      <div className={`overviewTrendPlotLayout${metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Time ? " hasSecondaryMetric" : ""}`}>
        <div className="overviewTrendYAxis" aria-hidden="true">
          {yAxisValues.map((value) => <span key={value}>{formatMetricValue(value, metric)}</span>)}
        </div>
        <div
          className="overviewTrendViewport"
          ref={viewportRef}
          onScroll={handleViewportScroll}
          onWheel={handleViewportWheel}
          aria-busy={loadingOlder}
        >
          <div
            className="overviewTrendCanvas"
            style={{ "--trend-columns": visible.length } as CSSProperties}
          >
            <div className="overviewTrendGrid" aria-hidden="true"><span /><span /><span /></div>
            <div
              className="overviewTrendBars"
              ref={barsRef}
              role="listbox"
              aria-label={`${granularity} ${metricLabel(metric, granularity)} ${OVERVIEW_USAGE_GROUP_LABELS[groupBy].toLowerCase()}`}
              aria-describedby="overview-trend-instructions"
              style={windowStyle}
            >
              {metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Time ? (
                <svg
                  className="overviewTrendLine"
                  viewBox={`0 0 ${Math.max(1, renderedModels.length) * TREND_COLUMN_WIDTH} ${TREND_PLOT_HEIGHT}`}
                  preserveAspectRatio="none"
                  aria-hidden="true"
                >
                  <path className="overviewTrendLinePath" d={metric === OverviewUsageMetric.Tokens ? tokensPerResponsePath : averageTurnTimePath} />
                </svg>
              ) : metric === OverviewUsageMetric.Cache ? (
                <svg
                  className="overviewTrendLine"
                  viewBox={`0 0 ${Math.max(1, renderedModels.length) * TREND_COLUMN_WIDTH} ${TREND_PLOT_HEIGHT}`}
                  preserveAspectRatio="none"
                  aria-hidden="true"
                >
                  {cacheLineSeries.map((series) => (
                    <path className={`overviewTrendLinePath ${series.className}`.trim()} key={series.key} d={series.path} />
                  ))}
                </svg>
              ) : null}
              {renderedModels.map(({ period, index, total, totalRungs, segments, rungs }) => {
                const isActive = period.key === activeKey;
                const isHovered = period.key === hoveredKey;
                const isPeak = period.key === peakPeriod.key;
                const showValueLabel = metric === OverviewUsageMetric.Cache
                  ? isCacheTurningPoint(visible, index)
                  : isPeak;
                const cacheValue = metric === OverviewUsageMetric.Cache ? cacheRate(period) : null;
                const tokensPerResponseValue = metric === OverviewUsageMetric.Tokens ? tokensPerResponse(period) : null;
                const averageTurnTimeValue = metric === OverviewUsageMetric.Time ? averageTurnMs(period) : null;
                const tooltipSegments = (isActive || isHovered || openTooltipKey === period.key)
                  ? buildTrendTooltipSegments(period, metric, groupBy, topCategories, segments)
                  : [];
                return (
                  <Tooltip
                    key={period.key}
                    interactive
                    delayDuration={200}
                    onOpenChange={(open) => {
                      setOpenTooltipKey((current) => {
                        if (open) return period.key;
                        return current === period.key ? null : current;
                      });
                      setHoveredKey((current) => {
                        if (open) return period.key;
                        return current === period.key ? null : current;
                      });
                    }}
                    content={(
                      <ChartTooltipContent
                        title={period.label}
                        value={metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Time ? (
                          <span className="overviewTrendTooltipValues">
                            <span>{formatMetricValue(total, metric)} {metric === OverviewUsageMetric.Tokens ? "total tokens" : "total time"}</span>
                            <strong>
                              {metric === OverviewUsageMetric.Tokens
                                ? tokensPerResponseValue === null
                                  ? `${EMPTY_DISPLAY_VALUE} avg / response`
                                  : `${formatTokensPerResponse(tokensPerResponseValue)} avg / response`
                                : averageTurnTimeValue === null
                                  ? `${EMPTY_DISPLAY_VALUE} avg / turn`
                                  : `${formatDuration(averageTurnTimeValue)} avg / turn`}
                            </strong>
                          </span>
                        ) : metric === OverviewUsageMetric.Cache && cacheValue === null
                          ? "No cache rate"
                          : `${formatMetricValue(total, metric)} ${metricLabel(metric, granularity)}`}
                        details={tooltipSegments.map((segment): ChartTooltipDetail => ({
                          key: segment.key,
                          label: segment.label,
                          swatchClassName: segment.className,
                          value: (
                            <>
                              {formatMetricValue(segment.value, metric)}
                              {metric !== OverviewUsageMetric.Cache && tooltipSegments.length > 1 && total > 0 ? ` · ${Math.round(segment.value / total * 100)}%` : ""}
                            </>
                          ),
                        }))}
                        footer={metric === OverviewUsageMetric.Sessions && granularity !== AnalyticsGranularity.Day ? (
                          <p className="chartTooltipMeta">
                            Peak day {formatDayGroupLabel(period.sessionPeakDate)}
                          </p>
                        ) : metric === OverviewUsageMetric.Cache ? (
                            <p className="chartTooltipMeta">
                            {cacheValue === null
                              ? "No input tokens"
                              : `${formatTokenCount(period.cachedInputTokens)} cached of ${formatTokenCount(period.inputTokens)} input tokens`}
                          </p>
                        ) : metric === OverviewUsageMetric.Tokens ? (
                            <p className="chartTooltipMeta">
                            {period.responses.toLocaleString()} responses
                            {` · ${period.compacted.toLocaleString()} compactions`}
                          </p>
                        ) : metric === OverviewUsageMetric.Time ? (
                            <p className="chartTooltipMeta">
                            {period.timedCompletedRuns.toLocaleString()} timed turns
                            {` · Longest ${period.maxRunMs ? formatDuration(period.maxRunMs) : EMPTY_DISPLAY_VALUE}`}
                          </p>
                        ) : metric === OverviewUsageMetric.Cost ? (
                            <p className="chartTooltipMeta">
                            Estimated from model pricing
                            {` · ${period.responses.toLocaleString()} responses`}
                          </p>
                        ) : metric === OverviewUsageMetric.Turns ? (
                            <p className="chartTooltipMeta">
                            Longest {period.maxRunMs ? `${Math.round(period.maxRunMs / 1000)}s` : EMPTY_DISPLAY_VALUE}
                          </p>
                        ) : null}
                      />
                    )}
                  >
                    <button
                      type="button"
                      role="option"
                      data-trend-index={index}
                      aria-selected={isActive}
                      aria-label={periodAriaLabel(period, metric, segments, periodDayCounts.get(period.key) ?? 0, expectedDays(period), granularity)}
                      tabIndex={isActive ? 0 : -1}
                      className={`overviewTrendBarButton${isHovered ? " isHovered" : ""}${isPeak ? " isPeak" : ""}`}
                      onClick={() => handlePeriodClick(period)}
                      onFocus={() => setActiveKey(period.key)}
                      onMouseEnter={() => setHoveredKey(period.key)}
                      onMouseLeave={() => {
                        if (openTooltipKey !== period.key) setHoveredKey(null);
                      }}
                      onKeyDown={(event) => moveSelection(event, index)}
                    >
                      {tokensPerResponseValue === null || !isHovered ? null : (
                        <span
                          className="overviewTrendLineMarker isHovered"
                          style={{ bottom: `${linearPlotPosition(tokensPerResponseValue, tokensPerResponseMax) * 100}%` }}
                          aria-hidden="true"
                        />
                      )}
                      {averageTurnTimeValue === null || !isHovered ? null : (
                        <span
                          className="overviewTrendLineMarker isHovered"
                          style={{ bottom: `${linearPlotPosition(averageTurnTimeValue, averageTurnTimeMax) * 100}%` }}
                          aria-hidden="true"
                        />
                      )}
                      {metric === OverviewUsageMetric.Cache ? cacheValue === null || !isHovered || groupBy !== OverviewUsageGroup.Overall ? null : (
                        <span
                          className="overviewTrendLineMarker isHovered"
                          style={{ bottom: `${cacheRatePlotPosition(cacheValue) * 100}%` }}
                          aria-hidden="true"
                        >
                          {showValueLabel ? <span className="overviewTrendValueLabel">{formatMetricValue(total, metric)}</span> : null}
                        </span>
                      ) : (
                        <span
                          className="overviewTrendBar"
                          style={{ height: `${totalRungs / maxRungs * 100}%` }}
                          aria-hidden="true"
                        >
                          <span className="overviewTrendRungs">
                            <svg viewBox={`0 0 100 ${Math.max(1, totalRungs)}`} preserveAspectRatio="none">
                              {rungs.map((rung) => (
                                <path
                                  key={rung.key}
                                  className={`overviewTrendRungPath ${rung.className}`}
                                  d={rung.path}
                                  vectorEffect="non-scaling-stroke"
                                />
                              ))}
                            </svg>
                          </span>
                          {showValueLabel ? <span className="overviewTrendValueLabel">{formatMetricValue(total, metric)}</span> : null}
                        </span>
                      )}
                    </button>
                  </Tooltip>
                );
              })}
            </div>
          </div>
          <div
            className="overviewTrendXAxis"
            style={{ "--trend-columns": visible.length, ...windowStyle } as CSSProperties}
            aria-hidden="true"
          >
            {renderedPeriods.map((period, localIndex) => {
              const index = windowStart + localIndex;
              const showLabel = index === 0 || index === visible.length - 1 || index % labelStep === 0;
              return <span key={period.key}>{showLabel ? period.label : ""}</span>;
              })}
            </div>
          </div>
          {metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Time ? (
            <div className="overviewTrendSecondaryYAxis" aria-hidden="true">
              {secondaryYAxisValues.map((value) => (
                <span key={value}>
                  {metric === OverviewUsageMetric.Tokens ? formatTokensPerResponse(value) : formatDuration(value)}
                </span>
              ))}
            </div>
          ) : null}
        </div>
    </ChartFrame>
  );
});
