import { sessionCacheRate, sessionTimeMs, type SessionRecord } from "./sessions.ts";

export type TokenMix = {
  cachedInput: number;
  uncachedInput: number;
  output: number;
};

export type SessionUsageSummary = {
  sessionsWithUsage: number;
  totalTokens: number;
  averageCacheRate: number | undefined;
  mix: TokenMix;
  sparklineRates: number[];
  recentSessions: SessionRecord[];
};

const RECENT_SESSION_LIMIT = 8;
const SPARKLINE_LIMIT = 12;

function hasTokenUsage(session: SessionRecord): boolean {
  return Boolean(session.tokenUsage && session.tokenUsage.totalTokens > 0);
}

export function summarizeSessionUsage(sessions: SessionRecord[]): SessionUsageSummary {
  const withUsage = sessions.filter(hasTokenUsage);
  const sortedByUpdated = [...sessions].sort(
    (a, b) => sessionTimeMs(b.updatedAt ?? b.time) - sessionTimeMs(a.updatedAt ?? a.time),
  );
  const recentSessions = sortedByUpdated.slice(0, RECENT_SESSION_LIMIT);

  let totalTokens = 0;
  let cachedInput = 0;
  let uncachedInput = 0;
  let output = 0;
  let cacheRateSum = 0;
  let cacheRateCount = 0;

  for (const session of withUsage) {
    const usage = session.tokenUsage!;
    totalTokens += usage.totalTokens;
    cachedInput += usage.cachedInputTokens;
    uncachedInput += Math.max(0, usage.inputTokens - usage.cachedInputTokens);
    output += usage.outputTokens;
    const rate = sessionCacheRate(session);
    if (rate !== undefined) {
      cacheRateSum += rate;
      cacheRateCount += 1;
    }
  }

  const sparklineSource = [...withUsage]
    .sort((a, b) => sessionTimeMs(a.updatedAt ?? a.time) - sessionTimeMs(b.updatedAt ?? b.time))
    .slice(-SPARKLINE_LIMIT);
  const sparklineRates = sparklineSource
    .map((session) => sessionCacheRate(session))
    .filter((rate): rate is number => rate !== undefined);

  return {
    sessionsWithUsage: withUsage.length,
    totalTokens,
    averageCacheRate: cacheRateCount > 0 ? cacheRateSum / cacheRateCount : undefined,
    mix: { cachedInput, uncachedInput, output },
    sparklineRates,
    recentSessions,
  };
}
