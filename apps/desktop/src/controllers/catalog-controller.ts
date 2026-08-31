import { friendlyAgent, normalizedAgentKey, sameAgent } from "../lib/agents.ts";
import { hookDeleteIdentity, isHookMutationDelta, normalizeHook, type HookRecord } from "../lib/hooks.ts";
import { isMcpMutationDelta, mcpRowKey, normalizeMcp, type McpRecord } from "../lib/mcp.ts";
import { normalizePrompt, type PromptRecord } from "../lib/prompt-model.ts";
import { normalizeRule, ruleAgents, type RuleRecord } from "../lib/rules.ts";
import { normalizeSession, sessionIdentity, type SessionRecord } from "../lib/sessions.ts";
import { normalizeSkill, type NormalizedSkill } from "../lib/skills.ts";
import { DOMAIN_KEYS, RuntimeDomainKey, type DomainKey } from "../lib/domain.ts";
import type { RuntimeData } from "../lib/data.ts";
import type { CatalogIndexes, CatalogSource, RawDomainRow } from "./controller-types.ts";

export type CatalogView = RuntimeData & { sourceIndex: readonly CatalogSource[] };

type CatalogViewCacheEntry = {
  data: RuntimeData;
  agentFilter: string;
  sourceIndex: readonly CatalogSource[];
  view: CatalogView;
};

const catalogViewCache = new WeakMap<RuntimeData, CatalogViewCacheEntry[]>();
const latestCatalogViews: CatalogViewCacheEntry[] = [];
const EMPTY_SOURCE_INDEX: readonly CatalogSource[] = [];

type NormalizedDomainRows = {
  agents: Record<string, unknown>[];
  skills: NormalizedSkill[];
  prompts: PromptRecord[];
  sessions: SessionRecord[];
  rules: RuleRecord[];
  hooks: HookRecord[];
  mcp: McpRecord[];
};

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value && typeof value === "object" && !Array.isArray(value));
}

function normalizeRows<K extends RuntimeDomainKey>(domain: K, rows: readonly RawDomainRow[]): NormalizedDomainRows[K] {
  if (domain === RuntimeDomainKey.Agents) return rows as unknown as NormalizedDomainRows[K];
  if (domain === RuntimeDomainKey.Skills) return rows.flatMap((row) => normalizeSkill(row) ?? []) as NormalizedDomainRows[K];
  if (domain === RuntimeDomainKey.Prompts) return rows.flatMap((row) => normalizePrompt(row) ?? []) as NormalizedDomainRows[K];
  if (domain === RuntimeDomainKey.Sessions) return rows.flatMap((row) => normalizeSession(row) ?? []) as NormalizedDomainRows[K];
  if (domain === RuntimeDomainKey.Rules) return rows.flatMap((row) => normalizeRule(row) ?? []) as NormalizedDomainRows[K];
  if (domain === RuntimeDomainKey.Hooks) return rows.flatMap((row) => normalizeHook(row) ?? []) as NormalizedDomainRows[K];
  return rows.flatMap((row) => normalizeMcp(row) ?? []) as NormalizedDomainRows[K];
}

function applyNormalizedRows<K extends RuntimeDomainKey>(
  current: RuntimeData,
  domain: K,
  rows: NormalizedDomainRows[K],
): RuntimeData {
  const oldRows = current[domain] as unknown as Record<string, unknown>[];
  const nextRows = reconcileCollection(
    oldRows,
    rows as unknown as Record<string, unknown>[],
    (row) => recordKey(domain, row),
  );
  if (nextRows === oldRows) return current;
  return { ...current, [domain]: nextRows } as RuntimeData;
}

function recordKey(domain: RuntimeDomainKey, row: Record<string, unknown>): string {
  if (domain === RuntimeDomainKey.Sessions) return sessionIdentity(row as Pick<SessionRecord, "agent" | "id" | "path">);
  if (domain === RuntimeDomainKey.Skills) return `${row.id ?? row.name ?? ""}`;
  if (domain === RuntimeDomainKey.Agents) return `${row.id ?? row.name ?? row.path ?? ""}`;
  if (domain === RuntimeDomainKey.Prompts) return `${row.id ?? ""}`;
  if (domain === RuntimeDomainKey.Rules) return `${row.path ?? ""}`;
  if (domain === RuntimeDomainKey.Hooks) return `${row.agent ?? ""}\0${row.path ?? ""}\0${row.event ?? ""}`;
  return `${row.agent ?? ""}\0${row.name ?? ""}\0${row.path ?? ""}`;
}

function sameRecord(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

/** Reconcile by stable identity and preserve object references for unchanged rows. */
export function reconcileCollection<T extends Record<string, unknown>>(
  current: readonly T[],
  incoming: readonly T[],
  key: (row: T) => string,
  equal: (left: T, right: T) => boolean = sameRecord,
): T[] {
  const previous = new Map(current.map((row) => [key(row), row]));
  const next = incoming.map((row) => {
    const old = previous.get(key(row));
    return old && equal(old, row) ? old : row;
  });
  if (next.length === current.length && next.every((row, index) => row === current[index])) return current as T[];
  return next;
}

export function applyDomainSnapshot(current: RuntimeData, domain: RuntimeDomainKey, rows: readonly RawDomainRow[]): RuntimeData {
  return applyNormalizedRows(current, domain, normalizeRows(domain, rows));
}

export function applyPromptRecord(
  current: RuntimeData,
  record: RawDomainRow,
  bodyFallback?: string,
): RuntimeData {
  const id = typeof record.id === "string" ? record.id.trim() : "";
  if (!id) return current;
  const withBody = typeof record.body === "string" || bodyFallback === undefined
    ? record
    : { ...record, body: bodyFallback };
  const normalized = normalizePrompt(withBody);
  if (!normalized) return current;
  const existing = current.prompts.find((prompt) => prompt.id === id);
  const nextRecord = {
    ...normalized,
    body: normalized.body || existing?.body || bodyFallback || "",
  };
  const prompts = [nextRecord, ...current.prompts.filter((prompt) => prompt.id !== id)];
  if (prompts.length === current.prompts.length && prompts.every((prompt, index) => prompt === current.prompts[index])) return current;
  return { ...current, prompts };
}

export function applyHookCommandResult(current: RuntimeData, result: unknown): RuntimeData | null {
  if (isHookMutationDelta(result)) {
    const deletedIdentities = new Set(
      (result.deleted ?? []).flatMap((row) => {
        if (!isRecord(row)) return [];
        const hook = normalizeHook(row);
        const identity = hook ? hookDeleteIdentity(hook) : undefined;
        return identity ? [identity] : [];
      }),
    );
    const hooks = current.hooks.filter((hook) => {
      const identity = hookDeleteIdentity(hook);
      return !identity || !deletedIdentities.has(identity);
    });
    for (const row of result.updated ?? []) {
      if (!isRecord(row)) continue;
      const hook = normalizeHook(row);
      if (!hook) continue;
      const identity = hookDeleteIdentity(hook);
      if (!identity) continue;
      const index = hooks.findIndex((item) => hookDeleteIdentity(item) === identity);
      if (index >= 0) hooks[index] = hook;
      else hooks.push(hook);
    }
    return applyNormalizedRows(current, RuntimeDomainKey.Hooks, hooks);
  }
  return null;
}

export function applyMcpCommandResult(current: RuntimeData, result: unknown): RuntimeData | null {
  if (isMcpMutationDelta(result)) {
    const mcp = [...current.mcp];
    for (const row of result.updated ?? []) {
      if (!isRecord(row)) continue;
      const server = normalizeMcp(row);
      if (!server) continue;
      const key = mcpRowKey(server);
      const index = mcp.findIndex((item) => mcpRowKey(item) === key);
      if (index >= 0) mcp[index] = server;
      else mcp.push(server);
    }
    return applyNormalizedRows(current, RuntimeDomainKey.Mcp, mcp);
  }
  return null;
}

export function applyRuleCommandResult(current: RuntimeData, result: unknown): RuntimeData | null {
  if (!isRecord(result) || !Array.isArray(result.deleted) || result.deleted.some((path) => typeof path !== "string")) return null;
  const deleted = new Set(result.deleted);
  const rules = current.rules.filter((rule) => !deleted.has(rule.path));
  return rules.length === current.rules.length ? current : { ...current, rules };
}

export function buildCatalogIndexes(data: RuntimeData, previous?: CatalogIndexes): CatalogIndexes {
  const counts = Object.fromEntries(DOMAIN_KEYS.map((domain) => [domain, data[domain].length])) as Record<DomainKey, number>;
  const sourceCounts = new Map<string, CatalogSource>();
  for (const agent of data.agents) {
    if (agent.installed !== true) continue;
    const label = friendlyAgent(agent.name);
    if (label) sourceCounts.set(normalizedAgentKey(label), { label, count: 0 });
  }
  for (const skill of data.skills) {
    for (const agent of skill.agents) {
      const key = normalizedAgentKey(agent);
      const current = sourceCounts.get(key);
      sourceCounts.set(key, { label: current?.label ?? agent, count: (current?.count ?? 0) + 1 });
    }
  }
  for (const session of data.sessions) {
    const key = normalizedAgentKey(session.agent);
    const current = sourceCounts.get(key);
    sourceCounts.set(key, { label: current?.label ?? session.agent, count: Math.max(current?.count ?? 0, 1) });
  }
  const sources = [...sourceCounts.values()].sort((left, right) => left.label.localeCompare(right.label));
  const installedAgentKeys = [...new Set([
    ...data.agents.filter((agent) => agent.installed === true).map((agent) => normalizedAgentKey(agent.name)),
    ...sources.filter((source) => source.count > 0).map((source) => normalizedAgentKey(source.label)),
  ])];
  const next = {
    counts,
    sources,
    installedAgentKeys,
    loadedDomains: previous?.loadedDomains ?? new Set<DomainKey>(),
    errorDomains: previous?.errorDomains ?? new Set<DomainKey>(),
  };
  if (previous && sameCatalogIndexes(previous, next)) return previous;
  return next;
}

function sameCatalogIndexes(left: CatalogIndexes, right: CatalogIndexes) {
  return DOMAIN_KEYS.every((domain) => left.counts[domain] === right.counts[domain])
    && left.sources.length === right.sources.length
    && left.sources.every((source, index) => source.label === right.sources[index]?.label && source.count === right.sources[index]?.count)
    && left.installedAgentKeys.length === right.installedAgentKeys.length
    && left.installedAgentKeys.every((key, index) => key === right.installedAgentKeys[index])
    && sameSet(left.loadedDomains, right.loadedDomains)
    && sameSet(left.errorDomains, right.errorDomains);
}

function sameSet<T>(left: ReadonlySet<T>, right: ReadonlySet<T>) {
  return left.size === right.size && [...left].every((value) => right.has(value));
}

export function selectCatalogView(data: RuntimeData, agentFilter: string, sourceIndex: readonly CatalogSource[] = EMPTY_SOURCE_INDEX): CatalogView {
  const cached = catalogViewCache.get(data)?.find((entry) => entry.agentFilter === agentFilter && entry.sourceIndex === sourceIndex);
  if (cached) return cached.view;
  const previous = latestCatalogViews.find((entry) => entry.agentFilter === agentFilter && entry.sourceIndex === sourceIndex);
  const view = agentFilter === "All" ? {
    agents: data.agents,
    skills: data.skills,
    prompts: data.prompts,
    sessions: data.sessions,
    rules: data.rules,
    hooks: data.hooks,
    mcp: data.mcp,
    sourceIndex,
  } : {
    agents: data.agents,
    skills: data.skills === previous?.data.skills
      ? previous.view.skills
      : data.skills.filter((skill) => skill.agents.some((agent) => sameAgent(agent, agentFilter))),
    prompts: data.prompts,
    sessions: data.sessions === previous?.data.sessions
      ? previous.view.sessions
      : data.sessions.filter((session) => sameAgent(session.agent, agentFilter)),
    rules: data.rules === previous?.data.rules
      ? previous.view.rules
      : data.rules.filter((rule) => ruleAgents(rule).some((agent) => sameAgent(agent, agentFilter))),
    hooks: data.hooks === previous?.data.hooks
      ? previous.view.hooks
      : data.hooks.filter((hook) => sameAgent(hook.agent, agentFilter)),
    mcp: data.mcp === previous?.data.mcp
      ? previous.view.mcp
      : data.mcp.filter((server) => sameAgent(server.agent, agentFilter)),
    sourceIndex,
  };
  if (previous && previous.sourceIndex === sourceIndex
    && previous.data.agents === data.agents && previous.data.skills === data.skills
    && previous.data.prompts === data.prompts && previous.data.sessions === data.sessions
    && previous.data.rules === data.rules && previous.data.hooks === data.hooks
    && previous.data.mcp === data.mcp) return previous.view;
  const entries = catalogViewCache.get(data) ?? [];
  const entry = { data, agentFilter, sourceIndex, view };
  entries.push(entry);
  catalogViewCache.set(data, entries);
  const latestIndex = latestCatalogViews.findIndex((item) => item.agentFilter === agentFilter && item.sourceIndex === sourceIndex);
  if (latestIndex >= 0) latestCatalogViews[latestIndex] = entry;
  else latestCatalogViews.push(entry);
  return view;
}
