export type AnalyticsTokenUsage = {
  inputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
};

export type AnalyticsCapabilities = {
  tokenUsage: boolean;
  reasoningTokens: boolean;
  explicitRuns: boolean;
  rateLimitHistory: boolean;
};

export type AnalyticsRefreshProgress = {
  phase: "overview" | "backfill" | "watch" | "session";
  total: number;
  completed: number;
  parsed: number;
  appended: number;
  skipped: number;
  failed: number;
  running: boolean;
  error?: string | null;
};

export function supportsResponseTokenUsage(agent: unknown): boolean {
  const key = `${agent ?? ""}`.toLowerCase().replace(/[^a-z0-9]/g, "");
  return key === "codex" || key === "claude" || key === "claudecode";
}

export type AnalyticsRunSummary = {
  started: number;
  completed: number;
  unclosed: number;
  totalMs: number;
  maxMs: number;
};

export type AnalyticsDay = {
  date: string;
  usage: AnalyticsTokenUsage;
  responses: number;
  sessions: number;
  runs: AnalyticsRunSummary;
  aborted: number;
  compacted: number;
  models: Array<{ model: string; totalTokens: number }>;
  rateLimits: Record<string, number>;
};

export type AnalyticsRankItem = {
  name: string;
  server: string;
  calls: number;
  sessions: number;
  share: number;
};

export type OverviewAnalytics = {
  generatedAt: string;
  daysRequested: number;
  rankDays: number;
  coverage: {
    first?: string;
    last?: string;
    totalSessions: number;
    analyzedSessions: number;
  };
  capabilities: Array<{ agent: string } & AnalyticsCapabilities>;
  summary: {
    usage: AnalyticsTokenUsage;
    responses: number;
    sessions: number;
    runs: AnalyticsRunSummary;
    aborted: number;
    abortedRate: number;
    compacted: number;
    compactedSessions: number;
  };
  days: AnalyticsDay[];
  tools: AnalyticsRankItem[];
  skills: AnalyticsRankItem[];
  warnings: string[];
};

export type SessionAnalyticsDetail = {
  sessionId: string;
  agent: string;
  sessionPath: string;
  capabilities: AnalyticsCapabilities;
  responses: Array<{
    index: number;
    timestamp: string;
    model: string;
    usage: AnalyticsTokenUsage;
    cumulative: AnalyticsTokenUsage;
  }>;
  runs: Array<{ start: string; end: string; completed: boolean }>;
  tools: Array<{ timestamp: string; name: string; server: string }>;
  skills: Array<{ timestamp: string; name: string }>;
  aborts: string[];
  compactions: string[];
  limitSamples: Array<{ timestamp: string; windowMinutes: number; usedPercent: number }>;
  malformedLines: number;
  indexedBytes: number;
};

export type AnalyticsGranularity = "day" | "week" | "month";

export type AnalyticsPeriod = {
  key: string;
  label: string;
  totalTokens: number;
  responses: number;
  sessions: number;
  runs: number;
  aborted: number;
  compacted: number;
  models: Array<{ model: string; totalTokens: number }>;
};

function periodKey(date: string, granularity: AnalyticsGranularity): string {
  if (granularity === "day") return date;
  if (granularity === "month") return date.slice(0, 7);
  const value = new Date(`${date}T00:00:00`);
  const day = (value.getDay() + 6) % 7;
  value.setDate(value.getDate() - day);
  return `${value.getFullYear()}-${`${value.getMonth() + 1}`.padStart(2, "0")}-${`${value.getDate()}`.padStart(2, "0")}`;
}

function periodLabel(key: string, granularity: AnalyticsGranularity): string {
  if (granularity === "month") {
    const [year, month] = key.split("-");
    return `${year}-${month}`;
  }
  const date = new Date(`${key}T00:00:00`);
  return new Intl.DateTimeFormat(undefined, {
    month: "short",
    day: "numeric",
  }).format(date);
}

export function groupAnalyticsDays(
  days: AnalyticsDay[],
  granularity: AnalyticsGranularity,
): AnalyticsPeriod[] {
  const grouped = new Map<string, AnalyticsPeriod & { modelMap: Map<string, number> }>();
  for (const day of days) {
    const key = periodKey(day.date, granularity);
    const period = grouped.get(key) ?? {
      key,
      label: periodLabel(key, granularity),
      totalTokens: 0,
      responses: 0,
      sessions: 0,
      runs: 0,
      aborted: 0,
      compacted: 0,
      models: [],
      modelMap: new Map<string, number>(),
    };
    period.totalTokens += day.usage.totalTokens;
    period.responses += day.responses;
    period.sessions = Math.max(period.sessions, day.sessions);
    period.runs += day.runs.started;
    period.aborted += day.aborted;
    period.compacted += day.compacted;
    for (const model of day.models) {
      period.modelMap.set(model.model, (period.modelMap.get(model.model) ?? 0) + model.totalTokens);
    }
    grouped.set(key, period);
  }
  return [...grouped.values()].map(({ modelMap, ...period }) => ({
    ...period,
    models: [...modelMap]
      .map(([model, totalTokens]) => ({ model, totalTokens }))
      .sort((left, right) => right.totalTokens - left.totalTokens),
  }));
}

export function analyticsHeatLevels(days: AnalyticsDay[]): Map<string, number> {
  const positive = days
    .map((day) => day.usage.totalTokens)
    .filter((value) => value > 0)
    .sort((left, right) => left - right);
  const threshold = (fraction: number) => positive[Math.min(positive.length - 1, Math.floor(positive.length * fraction))] ?? 0;
  const thresholds = [threshold(0.25), threshold(0.5), threshold(0.75)];
  return new Map(days.map((day) => {
    const value = day.usage.totalTokens;
    if (value <= 0) return [day.date, 0];
    return [day.date, 1 + thresholds.filter((entry) => value > entry).length];
  }));
}
