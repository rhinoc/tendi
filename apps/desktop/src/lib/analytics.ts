export type AnalyticsTokenUsage = {
  inputTokens: number;
  cachedInputTokens: number;
  cacheWriteInputTokens: number;
  outputTokens: number;
  reasoningOutputTokens: number;
  totalTokens: number;
};

export type AnalyticsCost = {
  inputUsd: number;
  cachedInputUsd: number;
  cacheWriteInputUsd: number;
  outputUsd: number;
  totalUsd: number;
};

export type AnalyticsProjectUsage = {
  id: string;
  name: string;
  usage: AnalyticsTokenUsage;
  responses: number;
  cost: AnalyticsCost;
};

export type AnalyticsModelUsage = {
  model: string;
  totalTokens: number;
  totalMs: number;
  completedRuns: number;
  cost: AnalyticsCost;
};

export type AnalyticsCapabilities = {
  tokenUsage: boolean;
  reasoningTokens: boolean;
  explicitRuns: boolean;
  duration: boolean;
  rateLimitHistory: boolean;
};

export enum AnalyticsRefreshPhase {
  Overview = "overview",
  Recent = "recent",
  Backfill = "backfill",
  Watch = "watch",
}

export type AnalyticsRefreshProgress = {
  phase: string;
  total: number;
  completed: number;
  running: boolean;
  error?: string | null;
};

export type AnalyticsRunSummary = {
  started: number;
  completed: number;
  unclosed: number;
  totalMs: number;
  maxMs: number;
  timedCompleted: number;
};

export type AnalyticsCallUsage = {
  name: string;
  server: string;
  calls: number;
};

export type AnalyticsDay = {
  date: string;
  usage: AnalyticsTokenUsage;
  cost: AnalyticsCost;
  responses: number;
  sessions: number;
  sessionsByAgent: Record<string, number>;
  runs: AnalyticsRunSummary;
  aborted: number;
  compacted: number;
  models: AnalyticsModelUsage[];
  projects: AnalyticsProjectUsage[];
  tools: AnalyticsCallUsage[];
  skills: AnalyticsCallUsage[];
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
  revision: number;
  generatedAt: string;
  daysRequested: number;
  rankDays: number;
  coverage: {
    first?: string;
    last?: string;
    totalSessions: number;
    analyzedSessions: number;
    indexingSessions: number;
  };
  capabilities: Array<{ agent: string } & AnalyticsCapabilities>;
  summary: {
    usage: AnalyticsTokenUsage;
    cost: AnalyticsCost;
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

export enum AnalyticsGranularity {
  Day = "day",
  Week = "week",
  Month = "month",
}

export function selectAnalyticsGranularity(dayCount: number): AnalyticsGranularity {
  const span = Math.max(1, Math.ceil(dayCount));
  if (span <= 60) return AnalyticsGranularity.Day;
  if (span <= 365) return AnalyticsGranularity.Week;
  return AnalyticsGranularity.Month;
}

export function stepAnalyticsGranularity(
  granularity: AnalyticsGranularity,
  direction: -1 | 1,
): AnalyticsGranularity {
  const granularities: AnalyticsGranularity[] = [AnalyticsGranularity.Day, AnalyticsGranularity.Week, AnalyticsGranularity.Month];
  const currentIndex = granularities.indexOf(granularity);
  const nextIndex = Math.max(0, Math.min(granularities.length - 1, currentIndex + direction));
  return granularities[nextIndex];
}

export type AnalyticsPeriod = {
  key: string;
  label: string;
  inputTokens: number;
  cachedInputTokens: number;
  totalTokens: number;
  cost: AnalyticsCost;
  responses: number;
  sessions: number;
  sessionsByAgent: Record<string, number>;
  sessionPeakDate: string;
  runs: number;
  completedRuns: number;
  timedCompletedRuns: number;
  unclosedRuns: number;
  totalRunMs: number;
  maxRunMs: number;
  aborted: number;
  compacted: number;
  models: AnalyticsModelUsage[];
  projects: AnalyticsProjectUsage[];
  tools: AnalyticsCallUsage[];
  skills: AnalyticsCallUsage[];
};

type AnalyticsPeriodAccumulator = AnalyticsPeriod & {
  modelMap: Map<string, AnalyticsModelUsage>;
  projectMap: Map<string, AnalyticsProjectUsage>;
  toolMap: Map<string, AnalyticsCallUsage>;
  skillMap: Map<string, AnalyticsCallUsage>;
};

function addTokenUsage(target: AnalyticsTokenUsage, source: AnalyticsTokenUsage) {
  target.inputTokens += source.inputTokens;
  target.cachedInputTokens += source.cachedInputTokens;
  target.cacheWriteInputTokens += source.cacheWriteInputTokens;
  target.outputTokens += source.outputTokens;
  target.reasoningOutputTokens += source.reasoningOutputTokens;
  target.totalTokens += source.totalTokens;
}

function addCost(target: AnalyticsCost, source: AnalyticsCost) {
  target.inputUsd += source.inputUsd;
  target.cachedInputUsd += source.cachedInputUsd;
  target.cacheWriteInputUsd += source.cacheWriteInputUsd;
  target.outputUsd += source.outputUsd;
  target.totalUsd += source.totalUsd;
}

function addProjectUsage(target: Map<string, AnalyticsProjectUsage>, projects: AnalyticsProjectUsage[]) {
  for (const project of projects) {
    const current = target.get(project.id);
    if (current) {
      addTokenUsage(current.usage, project.usage);
      current.responses += project.responses;
      addCost(current.cost, project.cost);
    } else {
      target.set(project.id, {
        ...project,
        usage: { ...project.usage },
        cost: { ...project.cost },
      });
    }
  }
}

function callUsageKey(call: Pick<AnalyticsCallUsage, "name" | "server">): string {
  return `${call.server}\0${call.name}`;
}

function addCallUsage(target: Map<string, AnalyticsCallUsage>, calls: AnalyticsCallUsage[]) {
  for (const call of calls) {
    const key = callUsageKey(call);
    const current = target.get(key);
    if (current) current.calls += call.calls;
    else target.set(key, { ...call });
  }
}

function sortedCallUsage(calls: Iterable<AnalyticsCallUsage>): AnalyticsCallUsage[] {
  return [...calls].sort((left, right) => (
    right.calls - left.calls
    || left.name.localeCompare(right.name)
    || left.server.localeCompare(right.server)
  ));
}

function periodKey(date: string, granularity: AnalyticsGranularity): string {
  if (granularity === AnalyticsGranularity.Day) return date;
  if (granularity === AnalyticsGranularity.Month) return date.slice(0, 7);
  const value = new Date(`${date}T00:00:00`);
  const day = (value.getDay() + 6) % 7;
  value.setDate(value.getDate() - day);
  return `${value.getFullYear()}-${`${value.getMonth() + 1}`.padStart(2, "0")}-${`${value.getDate()}`.padStart(2, "0")}`;
}

function periodLabel(key: string, granularity: AnalyticsGranularity): string {
  if (granularity === AnalyticsGranularity.Month) {
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
  const grouped = new Map<string, AnalyticsPeriodAccumulator>();
  for (const day of days) {
    const key = periodKey(day.date, granularity);
    const period = grouped.get(key) ?? {
      key,
      label: periodLabel(key, granularity),
      inputTokens: 0,
      cachedInputTokens: 0,
      totalTokens: 0,
      cost: { inputUsd: 0, cachedInputUsd: 0, cacheWriteInputUsd: 0, outputUsd: 0, totalUsd: 0 },
      responses: 0,
      sessions: 0,
      sessionsByAgent: {},
      sessionPeakDate: "",
      runs: 0,
      completedRuns: 0,
      timedCompletedRuns: 0,
      unclosedRuns: 0,
      totalRunMs: 0,
      maxRunMs: 0,
      aborted: 0,
      compacted: 0,
      models: [],
      projects: [],
      tools: [],
      skills: [],
      modelMap: new Map<string, AnalyticsModelUsage>(),
      projectMap: new Map<string, AnalyticsProjectUsage>(),
      toolMap: new Map<string, AnalyticsCallUsage>(),
      skillMap: new Map<string, AnalyticsCallUsage>(),
    };
    period.inputTokens += day.usage.inputTokens;
    period.cachedInputTokens += day.usage.cachedInputTokens;
    period.totalTokens += day.usage.totalTokens;
    addCost(period.cost, day.cost);
    period.responses += day.responses;
    // Daily buckets contain distinct sessions. For wider buckets, keep the
    // the peak day and its agent breakdown instead of double-counting sessions
    // active on many days.
    if (day.sessions > period.sessions) {
      period.sessions = day.sessions;
      period.sessionsByAgent = { ...day.sessionsByAgent };
      period.sessionPeakDate = day.date;
    }
    period.runs += day.runs.started;
    period.completedRuns += day.runs.completed;
    period.timedCompletedRuns += day.runs.timedCompleted;
    period.unclosedRuns += day.runs.unclosed;
    period.totalRunMs += day.runs.totalMs;
    period.maxRunMs = Math.max(period.maxRunMs, day.runs.maxMs);
    period.aborted += day.aborted;
    period.compacted += day.compacted;
    for (const model of day.models) {
      const current = period.modelMap.get(model.model) ?? {
        model: model.model,
        totalTokens: 0,
        totalMs: 0,
        completedRuns: 0,
        cost: { inputUsd: 0, cachedInputUsd: 0, cacheWriteInputUsd: 0, outputUsd: 0, totalUsd: 0 },
      };
      current.totalTokens += model.totalTokens;
      current.totalMs += model.totalMs;
      current.completedRuns += model.completedRuns;
      addCost(current.cost, model.cost);
      period.modelMap.set(model.model, current);
    }
    addProjectUsage(period.projectMap, day.projects);
    addCallUsage(period.toolMap, day.tools);
    addCallUsage(period.skillMap, day.skills);
    grouped.set(key, period);
  }
  return [...grouped.values()].map(({ modelMap, projectMap, toolMap, skillMap, ...period }) => ({
    ...period,
    models: [...modelMap.values()]
      .sort((left, right) => right.totalTokens - left.totalTokens),
    projects: [...projectMap.values()]
      .sort((left, right) => right.usage.totalTokens - left.usage.totalTokens || left.name.localeCompare(right.name)),
    tools: sortedCallUsage(toolMap.values()),
    skills: sortedCallUsage(skillMap.values()),
  }));
}
