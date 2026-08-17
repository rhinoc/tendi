import { sessionKind, sessionTimeMs, type SessionRecord } from "./sessions.ts";

export type TokenMix = {
  cachedInput: number;
  uncachedInput: number;
  output: number;
};

export type SessionUsageSummary = {
  sessionsWithUsage: number;
  totalTokens: number;
  mix: TokenMix;
  recentSessions: SessionRecord[];
};

const RECENT_SESSION_LIMIT = 10;

function hasTokenUsage(session: SessionRecord): boolean {
  return Boolean(session.tokenUsage && session.tokenUsage.totalTokens > 0);
}

export function summarizeSessionUsage(sessions: SessionRecord[]): SessionUsageSummary {
  const withUsage = sessions.filter(hasTokenUsage);
  const sortedByUpdated = sessions
    .filter((session) => sessionKind(session) === "main")
    .sort(
      (a, b) => sessionTimeMs(b.updatedAt ?? b.time) - sessionTimeMs(a.updatedAt ?? a.time),
    );
  const recentSessions = sortedByUpdated.slice(0, RECENT_SESSION_LIMIT);

  let totalTokens = 0;
  let cachedInput = 0;
  let uncachedInput = 0;
  let output = 0;

  for (const session of withUsage) {
    const usage = session.tokenUsage!;
    totalTokens += usage.totalTokens;
    cachedInput += usage.cachedInputTokens;
    uncachedInput += Math.max(0, usage.inputTokens - usage.cachedInputTokens);
    output += usage.outputTokens;
  }

  return {
    sessionsWithUsage: withUsage.length,
    totalTokens,
    mix: { cachedInput, uncachedInput, output },
    recentSessions,
  };
}
