import type { OverviewAnalytics } from "../../lib/analytics.ts";
import { invokeAnalyticsOverview } from "../../lib/runtime-gateway.ts";

export const ANALYTICS_DEFAULT_RANGE_DAYS = 61;
export const MAX_ANALYTICS_DAYS = 365;
export const ANALYTICS_REVISION_SETTLE_MS = 400;
export const ANALYTICS_LOAD_STEPS = [ANALYTICS_DEFAULT_RANGE_DAYS, 90, 182, MAX_ANALYTICS_DAYS] as const;

const overviewAnalyticsQueries = new Map<string, Promise<OverviewAnalytics | null>>();

export function inclusiveDaysSince(date: string): number {
  const [year, month, day] = date.split("-").map(Number);
  if (!year || !month || !day) return ANALYTICS_LOAD_STEPS[0];
  const now = new Date();
  const todayUtc = Date.UTC(now.getFullYear(), now.getMonth(), now.getDate());
  const firstUtc = Date.UTC(year, month - 1, day);
  return Math.max(1, Math.floor((todayUtc - firstUtc) / 86_400_000) + 1);
}

export function selectAnalyticsOlderRange(
  range: number,
  firstAvailableDate?: string,
  firstLoadedDate?: string,
): { targetRange: number; hasOlder: boolean } {
  const targetRange = firstAvailableDate
    ? Math.min(MAX_ANALYTICS_DAYS, inclusiveDaysSince(firstAvailableDate))
    : ANALYTICS_LOAD_STEPS[0];
  return {
    targetRange,
    hasOlder: Boolean(
      range < targetRange
      && firstAvailableDate
      && firstLoadedDate
      && firstAvailableDate < firstLoadedDate
    ),
  };
}

export function nextAnalyticsRange(current: number, target: number, minimumRange?: number): number {
  if (current >= target) return current;
  const step = ANALYTICS_LOAD_STEPS.find((days) => days > current);
  return minimumRange === undefined
    ? Math.min(step ?? target, target)
    : Math.min(target, Math.max(current + 1, Math.ceil(minimumRange)));
}

export function previousCalendarDate(date: string): string {
  const value = new Date(`${date}T00:00:00Z`);
  value.setUTCDate(value.getUTCDate() - 1);
  return value.toISOString().slice(0, 10);
}

export function overviewAnalyticsCacheKey(agent: string, days: number, analyticsRevision: number): string {
  return `${agent}:${days}:${analyticsRevision}`;
}

export function mergeAnalyticsDays(
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

export function queryOverviewAnalytics(
  agent: string,
  range: number,
  revision: number,
  args: {
    agent: string | null;
    days: number;
    rankDays: number;
    refreshTranscripts: boolean;
    endDate?: string;
  },
  refreshTranscripts: boolean,
): Promise<OverviewAnalytics | null> {
  if (refreshTranscripts) {
    overviewAnalyticsQueries.clear();
    return invokeAnalyticsOverview(args);
  }

  const queryCacheKey = `${overviewAnalyticsCacheKey(agent, range, revision)}:${args.days}:${args.endDate ?? "today"}`;
  const cachedQuery = overviewAnalyticsQueries.get(queryCacheKey);
  if (cachedQuery) return cachedQuery;

  const query = invokeAnalyticsOverview(args);
  overviewAnalyticsQueries.set(queryCacheKey, query);
  void query.then(() => {
    if (overviewAnalyticsQueries.get(queryCacheKey) === query) {
      overviewAnalyticsQueries.delete(queryCacheKey);
    }
  });
  return query;
}
