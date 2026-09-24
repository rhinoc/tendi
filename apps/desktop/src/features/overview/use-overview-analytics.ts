import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import {
  ANALYTICS_DEFAULT_RANGE_DAYS,
  ANALYTICS_REVISION_SETTLE_MS,
  mergeAnalyticsDays,
  nextAnalyticsRange,
  previousCalendarDate,
  queryOverviewAnalytics,
  selectAnalyticsOlderRange,
} from "./overview-analytics-query.ts";
import { ALL_AGENT_FILTER } from "../../lib/agents.ts";
import type { OverviewAnalytics } from "../../lib/analytics.ts";
import { logger } from "../../lib/logger.ts";
import { desktopStore, selectAnalyticsDisplayValue, selectAnalyticsValue, useDesktopStore } from "../../store/desktop-store.ts";

let retainedAnalyticsRange = ANALYTICS_DEFAULT_RANGE_DAYS;

export function useOverviewAnalytics({ agentFilter }: { agentFilter: string }) {
  const analyticsRevision = useDesktopStore((state) => state.analytics.revision);
  const analyticsRevisionReady = useDesktopStore((state) => state.analytics.ready);
  const analyticsRevisionError = useDesktopStore((state) => state.analytics.error);
  const [analyticsRange, setAnalyticsRange] = useState(retainedAnalyticsRange);
  const queryKey = useMemo(() => ({ agent: agentFilter, range: analyticsRange, revision: analyticsRevision }), [agentFilter, analyticsRange, analyticsRevision]);
  const analytics = useDesktopStore((state) => selectAnalyticsDisplayValue(state, queryKey));
  const analyticsProgress = useDesktopStore((state) => state.analytics.progress);
  const [analyticsLoading, setAnalyticsLoading] = useState(!analytics);
  const [analyticsError, setAnalyticsError] = useState("");
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

  const updateAnalyticsRange = useCallback((nextRange: number) => {
    retainedAnalyticsRange = nextRange;
    setAnalyticsRange(nextRange);
  }, []);

  const { targetRange, hasOlder } = selectAnalyticsOlderRange(
    analyticsRange,
    analytics?.coverage.first,
    analytics?.days[0]?.date,
  );
  const loadingOlder = Boolean(analytics && analytics.daysRequested < analyticsRange);
  const loadOlderAnalytics = useCallback((minimumRange?: number) => {
    setAnalyticsRange((current) => {
      const nextRange = nextAnalyticsRange(current, targetRange, minimumRange);
      retainedAnalyticsRange = nextRange;
      return nextRange;
    });
  }, [targetRange]);

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
    const currentQueryKey = { agent: agentFilter, range: analyticsRange, revision: analyticsRevision };
    const cached = selectAnalyticsValue(desktopStore.getSnapshot(), currentQueryKey);
    if (!refreshTranscripts && cached) {
      analyticsRef.current = cached;
      setAnalyticsLoading(false);
      return;
    }
    if (showLoading) {
      setAnalyticsLoading(true);
      desktopStore.actions.setAnalyticsProgress(null);
    }
    setAnalyticsError("");
    const loadedDays = analyticsRef.current?.daysRequested ?? 0;
    const isOlderRangeQuery = !refreshTranscripts && analyticsRange > loadedDays && loadedDays > 0;
    const queryDays = isOlderRangeQuery ? analyticsRange - loadedDays : analyticsRange;
    const existingFirstDate = isOlderRangeQuery ? analyticsRef.current?.days[0]?.date : undefined;
    const queryEndDate = existingFirstDate ? previousCalendarDate(existingFirstDate) : undefined;
    const args = {
      agent: agentFilter === ALL_AGENT_FILTER ? null : agentFilter,
      days: queryDays,
      rankDays: Math.min(30, queryDays),
      refreshTranscripts,
      ...(queryEndDate ? { endDate: queryEndDate } : {}),
    };
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
      const result = await queryOverviewAnalytics(agentFilter, analyticsRange, analyticsRevision, args, refreshTranscripts);
      resultRevision = result?.revision ?? analyticsRevision;
      if (request !== analyticsRequestRef.current) return;
      if (result) {
        const merged = mergeAnalyticsDays(analyticsRef.current, result, analyticsRange);
        // The request revision identifies the snapshot the query requested. The
        // response revision can lag while the analytics worker is publishing;
        // using it as the cache key makes every settled refresh look stale.
        desktopStore.actions.setAnalyticsValue(merged, currentQueryKey);
        analyticsRef.current = merged;
      }
      else {
        setAnalyticsError((current) => current || "Analytics could not be loaded");
        const loadedDays = analyticsRef.current?.daysRequested;
        if (loadedDays && analyticsRange > loadedDays) updateAnalyticsRange(loadedDays);
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
  }, [agentFilter, analyticsRange, analyticsRevision, scheduleAnalyticsRevisionRefresh, updateAnalyticsRange]);

  analyticsLoadRef.current = loadAnalytics;

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

  return {
    analytics,
    analyticsError,
    analyticsLoading,
    analyticsProgress,
    analyticsRevisionError,
    analyticsRange,
    loadingOlder,
    hasOlder,
    loadOlderAnalytics,
    loadAnalytics,
  };
}
