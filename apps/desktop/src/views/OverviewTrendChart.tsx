import { useEffect, useMemo, useRef, useState, type CSSProperties, type KeyboardEvent } from "react";

import { groupAnalyticsDays, type AnalyticsGranularity, type AnalyticsPeriod, type OverviewAnalytics } from "../lib/analytics.ts";
import { formatTokenCount } from "../lib/tokenizer.ts";

function periodAriaLabel(period: AnalyticsPeriod, dayCount: number, expectedDays: number): string {
  const modelSummary = period.models.length
    ? period.models.map((model) => `${model.model} ${model.totalTokens.toLocaleString()}`).join(", ")
    : "No model attribution";
  const coverage = dayCount < expectedDays ? ` Partial period, ${dayCount} of ${expectedDays} days.` : "";
  return `${period.label}: ${period.totalTokens.toLocaleString()} tokens, ${period.responses.toLocaleString()} responses, ${period.runs.toLocaleString()} runs.${coverage} ${modelSummary}`;
}
export function OverviewTrendChart({ analytics, granularity }: { analytics: OverviewAnalytics; granularity: AnalyticsGranularity }) {
  const periods = useMemo(() => groupAnalyticsDays(analytics.days, granularity), [analytics.days, granularity]);
  const visible = useMemo(
    () => periods.slice(-(granularity === "day" ? 60 : granularity === "week" ? 52 : 24)),
    [granularity, periods],
  );
  const modelTotals = useMemo(() => {
    const totals = new Map<string, number>();
    for (const period of visible) {
      for (const model of period.models) {
        totals.set(model.model, (totals.get(model.model) ?? 0) + model.totalTokens);
      }
    }
    return totals;
  }, [visible]);
  const topModels = useMemo(
    () => [...modelTotals].sort((left, right) => right[1] - left[1]).slice(0, 4).map(([model]) => model),
    [modelTotals],
  );
  const topModelSet = useMemo(() => new Set(topModels), [topModels]);
  const expectedDays = (period: AnalyticsPeriod) => {
    if (granularity === "day") return 1;
    if (granularity === "week") return 7;
    const [year, month] = period.key.split("-").map(Number);
    return new Date(year, month, 0).getDate();
  };
  const periodDayCounts = useMemo(() => new Map(visible.map((period) => {
    if (granularity === "day") return [period.key, 1];
    if (granularity === "month") {
      return [period.key, analytics.days.filter((day) => day.date.startsWith(period.key)).length];
    }
    const start = new Date(`${period.key}T00:00:00Z`);
    const end = new Date(start);
    end.setUTCDate(end.getUTCDate() + 7);
    const endDate = end.toISOString().slice(0, 10);
    return [period.key, analytics.days.filter((day) => day.date >= period.key && day.date < endDate).length];
  })), [analytics.days, granularity, visible]);
  const hasOtherModels = visible.some((period) => (
    period.totalTokens > period.models
      .filter((model) => topModelSet.has(model.model))
      .reduce((sum, model) => sum + model.totalTokens, 0)
  ));
  const max = Math.max(1, ...visible.map((period) => period.totalTokens));
  const [activeKey, setActiveKey] = useState(visible[visible.length - 1]?.key ?? "");
  const barsRef = useRef<HTMLDivElement>(null);
  const viewportRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    setActiveKey((current) => (
      visible.some((period) => period.key === current)
        ? current
        : visible[visible.length - 1]?.key ?? ""
    ));
    const viewport = viewportRef.current;
    if (!viewport) return;
    let pinnedToLatest = true;
    const scrollToLatest = () => {
      if (pinnedToLatest) viewport.scrollLeft = viewport.scrollWidth;
    };
    const trackScrollPosition = () => {
      pinnedToLatest = viewport.scrollWidth - viewport.clientWidth - viewport.scrollLeft <= 2;
    };
    const frame = requestAnimationFrame(scrollToLatest);
    const observer = new ResizeObserver(scrollToLatest);
    observer.observe(viewport);
    viewport.addEventListener("scroll", trackScrollPosition, { passive: true });
    return () => {
      cancelAnimationFrame(frame);
      observer.disconnect();
      viewport.removeEventListener("scroll", trackScrollPosition);
    };
  }, [visible]);

  if (!visible.length) {
    return (
      <section className="overviewTrendBlock" aria-labelledby="overview-trend-title">
        <div className="overviewSectionHeader">
          <div>
            <h3 id="overview-trend-title">Token volume</h3>
            <p>No activity in the selected range.</p>
          </div>
        </div>
      </section>
    );
  }

  const activeIndex = Math.max(0, visible.findIndex((period) => period.key === activeKey));
  const activePeriod = visible[activeIndex] ?? visible[visible.length - 1];
  const firstPeriod = visible[0];
  const lastPeriod = visible[visible.length - 1];
  const activeDayCount = periodDayCounts.get(activePeriod.key) ?? 0;
  const activeExpectedDays = expectedDays(activePeriod);
  const activeIsPartial = activeDayCount < activeExpectedDays;
  const lastDayCount = periodDayCounts.get(lastPeriod.key) ?? 0;
  const lastExpectedDays = expectedDays(lastPeriod);
  const labelStep = Math.max(1, Math.ceil(visible.length / 6));
  const otherModelTotal = Math.max(
    0,
    visible.reduce((sum, period) => sum + period.totalTokens, 0)
      - topModels.reduce((sum, model) => sum + (modelTotals.get(model) ?? 0), 0),
  );
  const activeKnownTokens = activePeriod.models
    .filter((model) => topModelSet.has(model.model))
    .reduce((sum, model) => sum + model.totalTokens, 0);
  const activeBreakdown = topModels
    .map((model, index) => ({
      key: model,
      label: model,
      value: activePeriod.models.find((entry) => entry.model === model)?.totalTokens ?? 0,
      className: `model${index}`,
    }))
    .filter((model) => model.value > 0);
  if (hasOtherModels && activePeriod.totalTokens > activeKnownTokens) {
    activeBreakdown.push({
      key: "other",
      label: "Other",
      value: activePeriod.totalTokens - activeKnownTokens,
      className: "modelOther",
    });
  }

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

  return (
    <section className="overviewTrendBlock" aria-labelledby="overview-trend-title">
      <div className="overviewTrendHeader">
        <div className="overviewSectionHeader">
          <div>
            <h3 id="overview-trend-title">Token volume</h3>
            <p>{firstPeriod.label}–{lastPeriod.label} · grouped {granularity === "day" ? "daily" : granularity === "week" ? "weekly" : "monthly"}</p>
          </div>
        </div>
        <div className="overviewTrendSelection" aria-live="polite">
          <span>{activePeriod.label}{activeIsPartial ? " · partial" : ""}</span>
          <strong>{formatTokenCount(activePeriod.totalTokens)}</strong>
        </div>
      </div>

      <div className="overviewTrendLegend" aria-label="Model totals in chart">
        {topModels.map((model, index) => (
          <span key={model}>
            <span className={`overviewTrendSwatch model${index}`} aria-hidden="true" />
            <span>{model}</span>
            <strong>{formatTokenCount(modelTotals.get(model) ?? 0)}</strong>
          </span>
        ))}
        {hasOtherModels ? (
          <span>
            <span className="overviewTrendSwatch modelOther" aria-hidden="true" />
            <span>Other</span>
            <strong>{formatTokenCount(otherModelTotal)}</strong>
          </span>
        ) : null}
      </div>

      <p id="overview-trend-instructions" className="overviewVisuallyHidden">
        Use Left and Right Arrow keys to inspect periods. Use Home and End to jump to the first or last period.
      </p>
      <div className="overviewTrendPlotLayout">
        <div className="overviewTrendYAxis" aria-hidden="true">
          <span>{formatTokenCount(max)}</span>
          <span>{formatTokenCount(max / 2)}</span>
          <span>0</span>
        </div>
        <div className="overviewTrendViewport" ref={viewportRef}>
          <div
            className="overviewTrendCanvas"
            style={{ "--trend-columns": visible.length } as CSSProperties}
          >
            <div className="overviewTrendGrid" aria-hidden="true"><span /><span /><span /></div>
            <div
              className="overviewTrendBars"
              ref={barsRef}
              role="listbox"
              aria-label={`${granularity} token volume`}
              aria-describedby="overview-trend-instructions"
            >
              {visible.map((period, index) => {
                const knownTokens = period.models
                  .filter((model) => topModelSet.has(model.model))
                  .reduce((sum, model) => sum + model.totalTokens, 0);
                const otherTokens = Math.max(0, period.totalTokens - knownTokens);
                const isActive = period.key === activePeriod.key;
                return (
                  <button
                    key={period.key}
                    type="button"
                    role="option"
                    aria-selected={isActive}
                    aria-label={periodAriaLabel(period, periodDayCounts.get(period.key) ?? 0, expectedDays(period))}
                    tabIndex={isActive ? 0 : -1}
                    className={`overviewTrendBarButton${isActive ? " isActive" : ""}`}
                    onClick={() => setActiveKey(period.key)}
                    onFocus={() => setActiveKey(period.key)}
                    onMouseEnter={() => setActiveKey(period.key)}
                    onKeyDown={(event) => moveSelection(event, index)}
                  >
                    <span
                      className="overviewTrendBar"
                      style={{ height: `${period.totalTokens ? Math.max(2, (period.totalTokens / max) * 100) : 0}%` }}
                      aria-hidden="true"
                    >
                      {period.models.filter((model) => topModelSet.has(model.model)).map((model) => (
                        <span
                          key={model.model}
                          className={`overviewTrendSegment model${topModels.indexOf(model.model)}`}
                          style={{ flexGrow: model.totalTokens }}
                        />
                      ))}
                      {otherTokens > 0 ? (
                        <span className="overviewTrendSegment modelOther" style={{ flexGrow: otherTokens }} />
                      ) : null}
                    </span>
                  </button>
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
      <p className="overviewTrendScrollHint">Scroll horizontally, or use arrow keys on the chart, to inspect every period.</p>
      {lastDayCount < lastExpectedDays ? (
        <p className="overviewTrendPartialNote">Latest period is partial: {lastDayCount} of {lastExpectedDays} days.</p>
      ) : null}

      <div className="overviewTrendDetail">
        <div className="overviewTrendDetailSummary">
          <span>Selected period</span>
          <strong>{activePeriod.label}</strong>
          <p>
            {activePeriod.responses.toLocaleString()} responses · {activePeriod.runs.toLocaleString()} runs
            {activeIsPartial ? ` · ${activeDayCount} of ${activeExpectedDays} days` : ""}
          </p>
        </div>
        <div className="overviewTrendBreakdown" aria-label={`Model breakdown for ${activePeriod.label}`}>
          {activeBreakdown.length ? activeBreakdown.map((model) => (
            <div key={model.key}>
              <span className={`overviewTrendSwatch ${model.className}`} aria-hidden="true" />
              <span>{model.label}</span>
              <strong>{formatTokenCount(model.value)}</strong>
              <span>{activePeriod.totalTokens ? `${((model.value / activePeriod.totalTokens) * 100).toFixed(0)}%` : "0%"}</span>
            </div>
          )) : <p className="overviewQuiet">No model attribution for this period.</p>}
        </div>
      </div>
    </section>
  );
}
