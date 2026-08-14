import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";

import { Tooltip } from "../components/shared/Tooltip.tsx";
import { groupAnalyticsDays, type AnalyticsGranularity, type AnalyticsPeriod, type OverviewAnalytics } from "../lib/analytics.ts";
import { formatTokenCount } from "../lib/token-format.ts";

const MAX_RUNG_COUNT = 28;
const MAX_CATEGORY_COUNT = 4;

export type OverviewUsageMetric = "sessions" | "turns" | "tokens" | "cache" | "tools" | "skills";

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

type ScrollSnapshot = {
  firstKey: string;
  left: number;
  viewKey: string;
  width: number;
};

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

function cacheRate(period: AnalyticsPeriod): number {
  if (period.inputTokens <= 0) return 0;
  return period.cachedInputTokens / period.inputTokens * 100;
}

function callTotal(calls: AnalyticsPeriod["tools"]): number {
  return calls.reduce((sum, call) => sum + call.calls, 0);
}

function metricValue(period: AnalyticsPeriod, metric: OverviewUsageMetric): number {
  if (metric === "sessions") return period.sessions;
  if (metric === "turns") return period.runs;
  if (metric === "cache") return cacheRate(period);
  if (metric === "tools") return callTotal(period.tools);
  if (metric === "skills") return callTotal(period.skills);
  return period.totalTokens;
}

function formatMetricValue(value: number, metric: OverviewUsageMetric): string {
  if (metric === "tokens") return formatTokenCount(value);
  if (metric === "cache") return `${value.toFixed(1)}%`;
  return value.toLocaleString();
}

function metricLabel(metric: OverviewUsageMetric, granularity?: AnalyticsGranularity): string {
  if (metric === "sessions") return granularity && granularity !== "day" ? "peak daily sessions" : "active sessions";
  if (metric === "cache") return "cache rate";
  if (metric === "tools") return "tool calls";
  if (metric === "skills") return "skill uses";
  return metric;
}

function breakdownItems(period: AnalyticsPeriod, metric: OverviewUsageMetric): BreakdownItem[] {
  if (metric === "tokens") {
    return period.models.map((model) => ({
      key: model.model,
      label: model.model,
      value: model.totalTokens,
    }));
  }
  if (metric === "tools" || metric === "skills") {
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
  return `${period.label}: ${formatMetricValue(metricValue(period, metric), metric)} ${metricLabel(metric, granularity)}.${coverage}${breakdown ? ` ${breakdown}.` : ""}`;
}

function turnSegments(period: AnalyticsPeriod): TrendSegment[] {
  const aborted = Math.min(period.aborted, period.unclosedRuns);
  return [
    { key: "completed", label: "Completed", value: period.completedRuns, className: "statusCompleted" },
    { key: "aborted", label: "Aborted", value: aborted, className: "statusAborted" },
    { key: "unclosed", label: "Other unclosed", value: Math.max(0, period.unclosedRuns - aborted), className: "statusUnclosed" },
  ];
}

// Lieflat F7 · Stacked Rungs · templates/basics-gallery.html · "Where each region's revenue sits"
export function OverviewTrendChart({
  analytics,
  granularity,
  hasOlder,
  loadingOlder,
  metric,
  onLoadOlder,
}: {
  analytics: OverviewAnalytics;
  granularity: AnalyticsGranularity;
  hasOlder: boolean;
  loadingOlder: boolean;
  metric: OverviewUsageMetric;
  onLoadOlder: () => void;
}) {
  const chartDays = useMemo(() => {
    const firstAvailableDate = analytics.coverage.first;
    if (!firstAvailableDate) return analytics.days;
    return analytics.days.filter((day) => day.date >= firstAvailableDate);
  }, [analytics.coverage.first, analytics.days]);
  const periods = useMemo(() => groupAnalyticsDays(chartDays, granularity), [chartDays, granularity]);
  const visible = periods;
  const categoryTotals = useMemo(() => {
    const totals = new Map<string, BreakdownItem>();
    for (const period of visible) {
      for (const item of breakdownItems(period, metric)) {
        const current = totals.get(item.key);
        if (current) current.value += item.value;
        else totals.set(item.key, { ...item });
      }
    }
    return totals;
  }, [metric, visible]);
  const topCategories = useMemo(
    () => [...categoryTotals.values()]
      .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label))
      .slice(0, MAX_CATEGORY_COUNT),
    [categoryTotals],
  );
  const topCategorySet = useMemo(() => new Set(topCategories.map((item) => item.key)), [topCategories]);
  const expectedDays = (period: AnalyticsPeriod) => {
    if (granularity === "day") return 1;
    if (granularity === "week") return 7;
    const [year, month] = period.key.split("-").map(Number);
    return new Date(year, month, 0).getDate();
  };
  const periodDayCounts = useMemo(() => {
    const counts = new Map<string, number>();
    for (const day of chartDays) {
      const key = granularity === "day"
        ? day.date
        : granularity === "month"
          ? day.date.slice(0, 7)
          : groupAnalyticsDays([day], "week")[0]?.key;
      if (key) counts.set(key, (counts.get(key) ?? 0) + 1);
    }
    return counts;
  }, [chartDays, granularity]);
  const hasOtherCategories = visible.some((period) => (
    breakdownItems(period, metric).some((item) => !topCategorySet.has(item.key) && item.value > 0)
  ));
  const hasMetricActivity = visible.some((period) => (
    metric === "cache" ? period.inputTokens > 0 : metricValue(period, metric) > 0
  ));
  const max = metric === "cache" ? 100 : Math.max(1, ...visible.map((period) => metricValue(period, metric)));
  const maxRungs = metric === "tokens" || metric === "cache"
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

  useEffect(() => {
    setActiveKey((current) => (
      visible.some((period) => period.key === current)
        ? current
        : visible[visible.length - 1]?.key ?? ""
    ));
  }, [visible]);

  useLayoutEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const viewKey = `${metric}:${granularity}`;
    const firstKey = visible[0]?.key ?? "";
    const previous = scrollSnapshotRef.current;
    if (!previous || previous.viewKey !== viewKey) {
      viewport.scrollLeft = viewport.scrollWidth;
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

    if (!loadingOlder) loadRequestedRef.current = false;
    if (
      hasOlder
      && !loadingOlder
      && !loadRequestedRef.current
      && viewport.scrollWidth <= viewport.clientWidth + 1
    ) {
      loadRequestedRef.current = true;
      onLoadOlder();
    }
  }, [granularity, hasOlder, loadingOlder, metric, onLoadOlder, visible]);

  const handleViewportScroll = () => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const snapshot = scrollSnapshotRef.current;
    if (snapshot) {
      snapshot.left = viewport.scrollLeft;
      snapshot.width = viewport.scrollWidth;
    }
    if (viewport.scrollLeft > 48 || !hasOlder || loadingOlder || loadRequestedRef.current) return;
    loadRequestedRef.current = true;
    onLoadOlder();
  };

  if (!visible.length || !hasMetricActivity) {
    return (
      <section className="overviewTrendBlock" aria-labelledby="overview-trend-title">
        <div className="overviewTrendLegend overviewTrendLegendEmpty" aria-hidden="true" />
        <div className="overviewTrendPlotLayout">
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
        </div>
      </section>
    );
  }

  const labelStep = Math.max(1, Math.ceil(visible.length / 6));
  const moveSelection = (event: KeyboardEvent<HTMLButtonElement>, index: number) => {
    let nextIndex = index;
    if (event.key === "ArrowLeft") nextIndex = Math.max(0, index - 1);
    else if (event.key === "ArrowRight") nextIndex = Math.min(visible.length - 1, index + 1);
    else if (event.key === "Home") nextIndex = 0;
    else if (event.key === "End") nextIndex = visible.length - 1;
    else return;
    event.preventDefault();
    setActiveKey(visible[nextIndex].key);
    const button = barsRef.current?.querySelectorAll<HTMLButtonElement>(".overviewTrendBarButton")[nextIndex];
    button?.focus();
    button?.scrollIntoView({ block: "nearest", inline: "nearest" });
  };

  const legend = metric === "tokens" || metric === "tools" || metric === "skills"
    ? [
        ...topCategories.map((item, index) => ({ key: item.key, label: item.label, className: `category${index}` })),
        ...(hasOtherCategories ? [{ key: "other", label: "Other", className: "categoryOther" }] : []),
      ]
    : metric === "sessions"
      ? [{ key: "sessions", label: granularity === "day" ? "Active sessions" : "Peak active sessions per day", className: "sessionActive" }]
      : metric === "cache"
        ? [{ key: "cache", label: "Cached input rate", className: "cacheRate" }]
        : [
          { key: "completed", label: "Completed", className: "statusCompleted" },
          { key: "aborted", label: "Aborted", className: "statusAborted" },
          { key: "unclosed", label: "Other unclosed", className: "statusUnclosed" },
        ];

  return (
    <section className="overviewTrendBlock" aria-label={`${metricLabel(metric, granularity)} trend`}>
      <div className="overviewTrendLegend" aria-label={`${metricLabel(metric, granularity)} chart legend`}>
        {legend.map((item) => (
          <span key={item.key}>
            <span className={`overviewTrendSwatch ${item.className}`} aria-hidden="true" />
            <span>{item.label}</span>
          </span>
        ))}
      </div>

      <p id="overview-trend-instructions" className="overviewVisuallyHidden">
        Use Left and Right Arrow keys to inspect periods. Use Home and End to jump to the first or last period.
      </p>
      <div className="overviewTrendPlotLayout">
        <div className="overviewTrendYAxis" aria-hidden="true">
          <span>{formatMetricValue(max, metric)}</span>
          <span>{formatMetricValue(max / 2, metric)}</span>
          <span>{formatMetricValue(0, metric)}</span>
        </div>
        <div
          className="overviewTrendViewport"
          ref={viewportRef}
          onScroll={handleViewportScroll}
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
            >
              {visible.map((period, index) => {
                const periodBreakdown = breakdownItems(period, metric);
                const periodValues = new Map(periodBreakdown.map((item) => [item.key, item.value]));
                const knownValue = topCategories.reduce((sum, item) => sum + (periodValues.get(item.key) ?? 0), 0);
                const otherValue = Math.max(0, metricValue(period, metric) - knownValue);
                const segments: TrendSegment[] = metric === "tokens" || metric === "tools" || metric === "skills"
                  ? [
                      ...topCategories.map((item, categoryIndex) => ({
                        key: item.key,
                        label: item.label,
                        value: periodValues.get(item.key) ?? 0,
                        className: `category${categoryIndex}`,
                      })),
                      ...(hasOtherCategories ? [{ key: "other", label: "Other", value: otherValue, className: "categoryOther" }] : []),
                    ]
                  : metric === "sessions"
                    ? [{ key: "sessions", label: "Active sessions", value: period.sessions, className: "sessionActive" }]
                    : metric === "cache"
                      ? [{ key: "cache", label: "Cached input rate", value: cacheRate(period), className: "cacheRate" }]
                      : turnSegments(period);
                const total = metricValue(period, metric);
                const totalRungs = total ? Math.max(1, Math.round(total / rungUnit)) : 0;
                const segmentRungs = apportionRungs(segments.map((segment) => segment.value), totalRungs);
                const rungs = segmentRungs.flatMap((count, segmentIndex) => (
                  Array.from({ length: count }, (_, rungIndex) => ({
                    key: `${segments[segmentIndex].key}-${rungIndex}`,
                    className: segments[segmentIndex].className,
                    isSegmentStart: segmentIndex > 0 && rungIndex === 0,
                    width: rungWidth(index, rungIndex, segmentIndex),
                  }))
                ));
                const isActive = period.key === activeKey;
                const isHovered = period.key === hoveredKey;
                const isPeak = period.key === peakPeriod.key;
                const tooltipSegments = metric === "cache"
                  ? []
                  : metric === "tokens" || metric === "tools" || metric === "skills"
                    ? periodBreakdown
                      .filter((item) => item.value > 0)
                      .map((item) => {
                        const categoryIndex = topCategories.findIndex((category) => category.key === item.key);
                        return {
                          ...item,
                          className: categoryIndex >= 0 ? `category${categoryIndex}` : "categoryOther",
                        };
                      })
                      .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label))
                    : [...segments]
                      .filter((segment) => segment.value > 0)
                      .sort((left, right) => right.value - left.value || left.label.localeCompare(right.label));
                return (
                  <Tooltip
                    key={period.key}
                    interactive
                    content={(
                      <div className="overviewTrendTooltip">
                        <div className="overviewTrendTooltipHeader">
                          <strong>{period.label}</strong>
                          <span>{formatMetricValue(total, metric)} {metricLabel(metric, granularity)}</span>
                        </div>
                        <div className="overviewTrendTooltipModels">
                          {tooltipSegments.map((segment) => (
                            <div key={segment.key}>
                              <span className={`overviewTrendSwatch ${segment.className}`} aria-hidden="true" />
                              <span>{segment.label}</span>
                              <strong>
                                {formatMetricValue(segment.value, metric)}
                                {tooltipSegments.length > 1 && total > 0 ? ` · ${Math.round(segment.value / total * 100)}%` : ""}
                              </strong>
                            </div>
                          ))}
                        </div>
                        {metric === "cache" ? (
                          <p className="overviewTrendTooltipMeta">
                            {period.cachedInputTokens.toLocaleString()} cached of {period.inputTokens.toLocaleString()} input tokens
                          </p>
                        ) : null}
                        {metric === "tokens" ? (
                          <p className="overviewTrendTooltipMeta">
                            {period.responses.toLocaleString()} responses · {period.compacted.toLocaleString()} compactions
                          </p>
                        ) : null}
                        {metric === "turns" ? (
                          <p className="overviewTrendTooltipMeta">
                            Longest {period.maxRunMs ? `${Math.round(period.maxRunMs / 1000)}s` : "—"}
                          </p>
                        ) : null}
                      </div>
                    )}
                  >
                    <button
                      type="button"
                      role="option"
                      aria-selected={isActive}
                      aria-label={periodAriaLabel(period, metric, segments, periodDayCounts.get(period.key) ?? 0, expectedDays(period), granularity)}
                      tabIndex={isActive ? 0 : -1}
                      className={`overviewTrendBarButton${isHovered ? " isHovered" : ""}${isPeak ? " isPeak" : ""}`}
                      onClick={() => setActiveKey(period.key)}
                      onFocus={() => setActiveKey(period.key)}
                      onMouseEnter={() => setHoveredKey(period.key)}
                      onMouseLeave={() => setHoveredKey(null)}
                      onKeyDown={(event) => moveSelection(event, index)}
                    >
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
                                animationDelay: `${index * 12 + rungIndex * 8}ms`,
                              }}
                            />
                          ))}
                        </span>
                        {isPeak ? <span className="overviewTrendPeakValue">{formatMetricValue(total, metric)}</span> : null}
                      </span>
                    </button>
                  </Tooltip>
                );
              })}
            </div>
          </div>
          <div
            className="overviewTrendXAxis"
            style={{ "--trend-columns": visible.length } as CSSProperties}
            aria-hidden="true"
          >
            {visible.map((period, index) => {
              const showLabel = index === 0 || index === visible.length - 1 || index % labelStep === 0;
              return <span key={period.key}>{showLabel ? period.label : ""}</span>;
            })}
          </div>
        </div>
      </div>
    </section>
  );
}
