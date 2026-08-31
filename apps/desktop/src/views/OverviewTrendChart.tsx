import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent, type WheelEvent } from "react";

import { ChartFrame } from "../components/shared/chart/ChartFrame.tsx";
import { ChartLegend, type ChartLegendItem } from "../components/shared/chart/ChartLegend.tsx";
import { ChartTooltipContent, type ChartTooltipDetail } from "../components/shared/chart/ChartTooltipContent.tsx";
import { Tooltip } from "../components/shared/Tooltip.tsx";
import { useElementSize } from "../components/shared/useElementSize.ts";
import { agentDefinition, agentIdentityKey, agentDefinitions, formatDayGroupLabel, friendlyAgent, normalizedAgentKey } from "../lib/index.ts";
import { AnalyticsGranularity, groupAnalyticsDays, stepAnalyticsGranularity, type AnalyticsPeriod, type OverviewAnalytics } from "../lib/analytics.ts";
import { formatTokenCount } from "../lib/token-format.ts";
import { fixedVirtualRange } from "../lib/virtualization.ts";
import { trackpadZoomDirection, useTrackpadZoom } from "../lib/zoom-gesture.ts";

const MAX_RUNG_COUNT = 28;
const MAX_CATEGORY_COUNT = 4;
const TREND_COLUMN_WIDTH = 24;
const TREND_EDGE_PADDING = 32;
const TREND_WINDOW_OVERSCAN = 8;
const TREND_VIRTUALIZATION_LIMIT = 80;
const TREND_INITIAL_WINDOW_COLUMNS = 64;
const TREND_ANIMATION_STAGGER_MS = 12;
const TREND_RUNG_ANIMATION_STAGGER_MS = 8;
const TREND_ANIMATION_MAX_DELAY_MS = 480;
const TREND_LABEL_TARGET_GAP = 84;
const TREND_DEFAULT_VIEWPORT_WIDTH = 640;
const TREND_PLOT_HEIGHT = 184;
const CACHE_RATE_MAX = 100;
const CACHE_RATE_LOG_SCALE = Math.log1p(CACHE_RATE_MAX);

export enum OverviewUsageMetric {
  Sessions = "sessions",
  Turns = "turns",
  Tokens = "tokens",
  Cache = "cache",
  Tools = "tools",
  Skills = "skills",
}
export enum OverviewOlderLoadReason {
  Auto = "auto",
  Scroll = "scroll",
}

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
  isSegmentStart: boolean;
  width: number;
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

function tokensPerResponse(period: AnalyticsPeriod): number | null {
  if (period.responses <= 0) return null;
  return period.totalTokens / period.responses;
}

function tokensPerResponsePlotPosition(value: number, max: number): number {
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
  if (metric === OverviewUsageMetric.Tools) return callTotal(period.tools);
  if (metric === OverviewUsageMetric.Skills) return callTotal(period.skills);
  return period.totalTokens;
}

function formatMetricValue(value: number, metric: OverviewUsageMetric): string {
  if (metric === OverviewUsageMetric.Tokens) return formatTokenCount(value);
  if (metric === OverviewUsageMetric.Cache) return `${value.toFixed(1)}%`;
  return value.toLocaleString();
}

function metricLabel(metric: OverviewUsageMetric, granularity?: AnalyticsGranularity): string {
  if (metric === OverviewUsageMetric.Sessions) return granularity && granularity !== AnalyticsGranularity.Day ? "peak daily sessions" : "active sessions";
  if (metric === OverviewUsageMetric.Cache) return "cache rate";
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

function breakdownItems(period: AnalyticsPeriod, metric: OverviewUsageMetric): BreakdownItem[] {
  if (metric === OverviewUsageMetric.Tokens) {
    return period.models.map((model) => ({
      key: model.model,
      label: model.model,
      value: model.totalTokens,
    }));
  }
  if (metric === OverviewUsageMetric.Tools || metric === OverviewUsageMetric.Skills) {
    return period[metric].map((call) => ({
      key: `${call.server}\0${call.name}`,
      label: call.server ? `${call.server} · ${call.name}` : call.name,
      value: call.calls,
    }));
  }
  return [];
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
  const average = metric === OverviewUsageMetric.Tokens ? tokensPerResponse(period) : null;
  const averageLabel = average === null ? "" : ` Average ${formatTokensPerResponse(average)} tokens per response.`;
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
  topCategories: BreakdownItem[],
  segments: TrendSegment[],
): Array<BreakdownItem & { className: string }> {
  if (metric === OverviewUsageMetric.Cache) return [];
  if (metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Tools || metric === OverviewUsageMetric.Skills) {
    return breakdownItems(period, metric)
      .filter((item) => item.value > 0)
      .map((item) => {
        const categoryIndex = topCategories.findIndex((category) => category.key === item.key);
        return {
          ...item,
          className: categoryIndex >= 0 ? `category${categoryIndex}` : "categoryOther",
        };
      })
      .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label));
  }
  return [...segments]
    .filter((segment) => segment.value > 0)
    .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label));
}

export function buildTrendPeriodModel(
  period: AnalyticsPeriod,
  index: number,
  metric: OverviewUsageMetric,
  topCategories: BreakdownItem[],
  hasOtherCategories: boolean,
  rungUnit: number,
  options: { includeTooltip?: boolean } = {},
): TrendPeriodModel {
  // Rung geometry is only needed for the virtualized window. Tooltip rows are
  // optional so callers can skip them for off-screen / non-hovered periods.
  const includeTooltip = options.includeTooltip === true;
  const periodBreakdown = (metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Tools || metric === OverviewUsageMetric.Skills)
    ? breakdownItems(period, metric)
    : [];
  const periodValues = new Map(periodBreakdown.map((item) => [item.key, item.value]));
  const knownValue = topCategories.reduce((sum, item) => sum + (periodValues.get(item.key) ?? 0), 0);
  const otherValue = Math.max(0, metricValue(period, metric) - knownValue);
  const segments: TrendSegment[] = metric === OverviewUsageMetric.Tokens || metric === OverviewUsageMetric.Tools || metric === OverviewUsageMetric.Skills
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
        : turnSegments(period);
  const total = metricValue(period, metric);
  const totalRungs = metric === OverviewUsageMetric.Cache ? 0 : total ? Math.max(1, Math.round(total / rungUnit)) : 0;
  const segmentRungs = metric === OverviewUsageMetric.Cache ? [] : apportionRungs(segments.map((segment) => segment.value), totalRungs);
  const rungs = segmentRungs.flatMap((count, segmentIndex) => (
    Array.from({ length: count }, (_, rungIndex) => ({
      key: `${segments[segmentIndex].key}-${rungIndex}`,
      className: segments[segmentIndex].className,
      isSegmentStart: segmentIndex > 0 && rungIndex === 0,
      width: rungWidth(index, rungIndex, segmentIndex),
    }))
  ));
  const tooltipSegments = includeTooltip
    ? buildTrendTooltipSegments(period, metric, topCategories, segments)
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
  onLoadOlder,
  onGranularityChange,
}: {
  analytics: OverviewAnalytics;
  granularity: AnalyticsGranularity;
  hasOlder: boolean;
  loadingOlder: boolean;
  metric: OverviewUsageMetric;
  onLoadOlder: (reason: OverviewOlderLoadReason) => void;
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
    () => visible.map((period) => breakdownItems(period, metric)),
    [metric, visible],
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
    () => periodBreakdowns.some((items) => items.some((item) => !topCategorySet.has(item.key) && item.value > 0)),
    [periodBreakdowns, topCategorySet],
  );
  const hasMetricActivity = visible.some((period) => (
    metric === OverviewUsageMetric.Cache ? period.inputTokens > 0 : metricValue(period, metric) > 0
  ));
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
  const barsRef = useRef<HTMLDivElement>(null);
  const loadRequestedRef = useRef(false);
  const scrollSnapshotRef = useRef<ScrollSnapshot | null>(null);
  const viewportRef = useRef<HTMLDivElement>(null);
  const { size: trendViewportSize } = useElementSize<HTMLDivElement>(
    { width: 0, height: 0 },
    {
      ref: viewportRef,
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
      topCategories,
      hasOtherCategories,
      rungUnit,
    )),
    [hasOtherCategories, metric, rungUnit, topCategories, visible, windowStart, windowEnd],
  );

  const syncTrendWindow = (viewport: HTMLDivElement) => {
    const next = trendWindowForScroll(visible.length, viewport.scrollLeft, viewport.clientWidth);
    setTrendWindow((current) => current.start === next.start && current.end === next.end ? current : next);
  };

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
    const viewKey = `${metric}:${granularity}`;
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
    if (
      hasOlder
      && !loadingOlder
      && !loadRequestedRef.current
      && viewport.scrollWidth <= viewport.clientWidth + 1
    ) {
      loadRequestedRef.current = true;
      onLoadOlder(OverviewOlderLoadReason.Auto);
    }
  }, [granularity, hasOlder, loadingOlder, metric, onLoadOlder, trendViewportSize.width, visible, windowed]);

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
    onGranularityChange(nextGranularity);
  });

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
    const snapshot = scrollSnapshotRef.current;
    if (snapshot) {
      snapshot.left = viewport.scrollLeft;
      snapshot.width = viewport.scrollWidth;
    }
    syncTrendWindow(viewport);
    if (viewport.scrollLeft > 48 || !hasOlder || loadingOlder || loadRequestedRef.current) return;
    loadRequestedRef.current = true;
    onLoadOlder(OverviewOlderLoadReason.Scroll);
  };

  if (!visible.length || !hasMetricActivity) {
    return (
      <ChartFrame
        ariaLabelledBy="overview-trend-title"
        legend={<ChartLegend items={[]} />}
        emptyState={(
          <div className={`overviewTrendPlotLayout${metric === OverviewUsageMetric.Tokens ? " hasTokensPerResponse" : ""}`}>
            <div className="overviewTrendYAxis" aria-hidden="true">
              <span>—</span>
              <span>—</span>
              <span>—</span>
            </div>
            <div className="overviewTrendViewport">
              <div
                className="overviewTrendCanvas"
                style={{ "--trend-columns": 1 } as CSSProperties}
              >
                <div className="overviewTrendGrid" aria-hidden="true"><span /><span /><span /></div>
                <div className="overviewTrendEmptyMessage">
                  <h3 id="overview-trend-title">No {metricLabel(metric, granularity)} activity</h3>
                  <p>No activity in the selected range.</p>
                </div>
              </div>
              <div className="overviewTrendXAxis" style={{ "--trend-columns": 1 } as CSSProperties} aria-hidden="true">
                <span />
              </div>
            </div>
            {metric === OverviewUsageMetric.Tokens ? (
              <div className="overviewTrendSecondaryYAxis" aria-hidden="true">
                <span>—</span>
                <span>—</span>
                <span>—</span>
              </div>
            ) : null}
          </div>
        )}
      >
        {null}
      </ChartFrame>
    );
  }

  const trendViewportWidth = trendViewportSize.width || TREND_DEFAULT_VIEWPORT_WIDTH;
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

  const legend: ChartLegendItem[] = metric === OverviewUsageMetric.Tokens
    ? [
        ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
        ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
        { key: "tokensPerResponse", label: "Avg tokens / response", swatchClassName: "tokensPerResponse" },
      ]
    : metric === OverviewUsageMetric.Tools || metric === OverviewUsageMetric.Skills
    ? [
        ...topCategories.map((item, index) => ({ key: item.key, label: item.label, swatchClassName: `category${index}` })),
        ...(hasOtherCategories ? [{ key: "other", label: "Other", swatchClassName: "categoryOther" }] : []),
      ]
      : metric === OverviewUsageMetric.Sessions
        ? sessionAgentKeys.map((agent) => ({
          key: `session-agent:${agent}`,
          label: sessionAgentLabel(agent),
          swatchClassName: sessionAgentClass(agent),
        }))
      : metric === OverviewUsageMetric.Cache
        ? []
        : [
          { key: "completed", label: "Completed", swatchClassName: "statusCompleted" },
          { key: "aborted", label: "Aborted", swatchClassName: "statusAborted" },
          { key: "unclosed", label: "Other unclosed", swatchClassName: "statusUnclosed" },
        ];
  const renderedModels = periodModels;
  const renderedPeriods = visible.slice(windowStart, windowEnd);
  const cacheLinePath = metric === OverviewUsageMetric.Cache
    ? trendLinePath(renderedModels, cacheRate, cacheRatePlotPosition)
    : "";
  const tokensPerResponsePath = metric === OverviewUsageMetric.Tokens
    ? trendLinePath(
      renderedModels,
      tokensPerResponse,
      (value) => tokensPerResponsePlotPosition(value, tokensPerResponseMax),
    )
    : "";
  const yAxisValues = metric === OverviewUsageMetric.Cache
    ? [CACHE_RATE_MAX, cacheRateAxisValue(0.5), 0]
    : [max, max / 2, 0];
  const secondaryYAxisValues = metric === OverviewUsageMetric.Tokens
    ? [tokensPerResponseMax, tokensPerResponseMax / 2, 0]
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
      ariaLabel={`${metricLabel(metric, granularity)} trend`}
      legend={metric === OverviewUsageMetric.Cache ? <ChartLegend items={[]} /> : (
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
        {metric === OverviewUsageMetric.Tokens ? "The line shows average tokens per response. " : null}
        {metric === OverviewUsageMetric.Cache ? "Cache rate uses a logarithmic scale based on uncached input. " : null}
        Use Left and Right Arrow keys to inspect periods. Use Home and End to jump to the first or last period.
      </p>
      <div className={`overviewTrendPlotLayout${metric === OverviewUsageMetric.Tokens ? " hasTokensPerResponse" : ""}`}>
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
              aria-label={`${granularity} ${metricLabel(metric, granularity)}`}
              aria-describedby="overview-trend-instructions"
              style={windowStyle}
            >
              {metric === OverviewUsageMetric.Tokens ? (
                <svg
                  className="overviewTrendLine"
                  viewBox={`0 0 ${Math.max(1, renderedModels.length) * TREND_COLUMN_WIDTH} ${TREND_PLOT_HEIGHT}`}
                  preserveAspectRatio="none"
                  aria-hidden="true"
                >
                  <path className="overviewTrendLinePath" d={tokensPerResponsePath} />
                </svg>
              ) : metric === OverviewUsageMetric.Cache ? (
                <svg
                  className="overviewTrendLine"
                  viewBox={`0 0 ${Math.max(1, renderedModels.length) * TREND_COLUMN_WIDTH} ${TREND_PLOT_HEIGHT}`}
                  preserveAspectRatio="none"
                  aria-hidden="true"
                >
                  <path className="overviewTrendLinePath" d={cacheLinePath} />
                </svg>
              ) : null}
              {renderedModels.map(({ period, index, total, totalRungs, segments, rungs }, localIndex) => {
                const isActive = period.key === activeKey;
                const isHovered = period.key === hoveredKey;
                const isPeak = period.key === peakPeriod.key;
                const showValueLabel = metric === OverviewUsageMetric.Cache
                  ? isCacheTurningPoint(visible, index)
                  : isPeak;
                const cacheValue = metric === OverviewUsageMetric.Cache ? cacheRate(period) : null;
                const tokensPerResponseValue = metric === OverviewUsageMetric.Tokens ? tokensPerResponse(period) : null;
                const tooltipSegments = (isActive || isHovered)
                  ? buildTrendTooltipSegments(period, metric, topCategories, segments)
                  : [];
                return (
                  <Tooltip
                    key={period.key}
                    interactive
                    content={(
                      <ChartTooltipContent
                        title={period.label}
                        value={metric === OverviewUsageMetric.Tokens ? (
                          <span className="overviewTrendTooltipValues">
                            <span>{formatMetricValue(total, metric)} total tokens</span>
                            <strong>
                              {tokensPerResponseValue === null
                                ? "— avg / response"
                                : `${formatTokensPerResponse(tokensPerResponseValue)} avg / response`}
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
                              {tooltipSegments.length > 1 && total > 0 ? ` · ${Math.round(segment.value / total * 100)}%` : ""}
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
                        ) : metric === OverviewUsageMetric.Turns ? (
                            <p className="chartTooltipMeta">
                            Longest {period.maxRunMs ? `${Math.round(period.maxRunMs / 1000)}s` : "—"}
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
                      onMouseLeave={() => setHoveredKey(null)}
                      onKeyDown={(event) => moveSelection(event, index)}
                    >
                      {tokensPerResponseValue === null || !isHovered ? null : (
                        <span
                          className="overviewTrendLineMarker isHovered"
                          style={{ bottom: `${tokensPerResponsePlotPosition(tokensPerResponseValue, tokensPerResponseMax) * 100}%` }}
                          aria-hidden="true"
                        />
                      )}
                      {metric === OverviewUsageMetric.Cache ? cacheValue === null || !isHovered ? null : (
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
                            {rungs.map((rung, rungIndex) => (
                              <span
                                key={rung.key}
                                className={`overviewTrendRung ${rung.className}${rung.isSegmentStart ? " isSegmentStart" : ""}`}
                                style={{
                                  width: `${rung.width}%`,
                                  animationDelay: `${Math.min(
                                    localIndex * TREND_ANIMATION_STAGGER_MS
                                      + rungIndex * TREND_RUNG_ANIMATION_STAGGER_MS,
                                    TREND_ANIMATION_MAX_DELAY_MS,
                                  )}ms`,
                                }}
                              />
                            ))}
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
          {metric === OverviewUsageMetric.Tokens ? (
            <div className="overviewTrendSecondaryYAxis" aria-hidden="true">
              {secondaryYAxisValues.map((value) => <span key={value}>{formatTokensPerResponse(value)}</span>)}
            </div>
          ) : null}
        </div>
    </ChartFrame>
  );
});
