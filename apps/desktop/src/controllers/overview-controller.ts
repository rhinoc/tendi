import { DOMAIN_KEYS, type DomainKey } from "../lib/domain.ts";
import type { RuntimeData } from "../lib/data.ts";
import { sameAgent } from "../lib/agents.ts";
import { SkillUpdateAvailability } from "../lib/skill-status.ts";

export type OverviewCounts = Record<DomainKey, number>;

const overviewCountsCache = new WeakMap<RuntimeData, OverviewCounts>();

export function selectOverviewCounts(data: RuntimeData): OverviewCounts {
  const cached = overviewCountsCache.get(data);
  if (cached) return cached;
  const counts = Object.fromEntries(DOMAIN_KEYS.map((domain) => [domain, data[domain].length])) as OverviewCounts;
  overviewCountsCache.set(data, counts);
  return counts;
}

export function selectOverviewHookReviewCount(data: RuntimeData): number {
  return data.hooks.reduce((count, hook) => count + (hook.needs_review === true ? 1 : 0), 0);
}

export function selectOverviewSkillUpdateCount(data: RuntimeData, agentFilter: string): number {
  return data.skills.reduce((count, skill) => count + (
    skill.updateAvailability === SkillUpdateAvailability.UpdateAvailable
    && (agentFilter === "All" || skill.agents.some((agent) => sameAgent(agent, agentFilter)))
      ? 1
      : 0
  ), 0);
}
