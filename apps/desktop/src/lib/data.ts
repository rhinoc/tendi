import { normalizePrompt } from "./prompt-model.ts";
import type { PromptRecord } from "./prompt-model.ts";
import { normalizeHook } from "./hooks.ts";
import type { HookRecord } from "./hooks.ts";
import { normalizeMcp } from "./mcp.ts";
import type { McpRecord } from "./mcp.ts";
import { normalizeRule } from "./rules.ts";
import type { RuleRecord } from "./rules.ts";
import { normalizeSession, type SessionRecord } from "./sessions.ts";
import { normalizeSkill, type NormalizedSkill } from "./skills.ts";
import { friendlyAgent } from "./agents.ts";
import type { RuntimeDomainKey } from "./domain.ts";

export type RuntimeData = {
  agents: Record<string, unknown>[];
  skills: NormalizedSkill[];
  prompts: PromptRecord[];
  sessions: SessionRecord[];
  rules: RuleRecord[];
  hooks: HookRecord[];
  mcp: McpRecord[];
  sources: Array<{ label: string; count: number }>;
  [key: string]: unknown;
};

function domainList(value: unknown): unknown[] | undefined {
  return Array.isArray(value) ? value : undefined;
}

function sourcesFor(agents: Record<string, unknown>[], skills: NormalizedSkill[], sessions: SessionRecord[]) {
  const agentCounts = new Map<string, number>();
  for (const agent of agents) {
    if (agent.installed !== true) continue;
    const label = friendlyAgent(agent.name);
    if (label) agentCounts.set(label, agentCounts.get(label) ?? 0);
  }
  for (const skill of skills) {
    for (const agent of skill.agents) agentCounts.set(agent, (agentCounts.get(agent) ?? 0) + 1);
  }
  for (const session of sessions) {
    const agent = session.agent;
    if (agent) agentCounts.set(agent, Math.max(agentCounts.get(agent) ?? 0, 1));
  }
  return [...agentCounts]
    .map(([label, count]) => ({ label, count }))
    .sort((left, right) => left.label.localeCompare(right.label));
}

export function recomputeSources(data: RuntimeData): RuntimeData {
  return { ...data, sources: sourcesFor(data.agents, data.skills, data.sessions) };
}

export function normalizeDomainRows(current: RuntimeData, domain: RuntimeDomainKey, value: unknown): RuntimeData {
  const rows = domainList(value);
  if (!rows) return current;
  let next: RuntimeData;
  if (domain === "agents") {
    next = { ...current, agents: rows as Record<string, unknown>[] };
  } else if (domain === "skills") {
    next = { ...current, skills: rows.flatMap((row) => {
      const skill = normalizeSkill(row as Record<string, unknown>);
      return skill ? [skill] : [];
    }) };
  } else if (domain === "prompts") {
    next = { ...current, prompts: rows.flatMap((row) => {
      const prompt = normalizePrompt(row as Record<string, unknown>);
      return prompt ? [prompt] : [];
    }) };
  } else if (domain === "sessions") {
    next = { ...current, sessions: rows.flatMap((row) => {
      const session = normalizeSession(row as Record<string, unknown>);
      return session ? [session] : [];
    }) };
  } else if (domain === "rules") {
    next = { ...current, rules: rows.flatMap((row) => {
      const rule = normalizeRule(row as Record<string, unknown>);
      return rule ? [rule] : [];
    }) };
  } else if (domain === "hooks") {
    next = { ...current, hooks: rows.flatMap((row) => {
      const hook = normalizeHook(row as Record<string, unknown>);
      return hook ? [hook] : [];
    }) };
  } else {
    next = { ...current, mcp: rows.flatMap((row) => {
      const server = normalizeMcp(row as Record<string, unknown>);
      return server ? [server] : [];
    }) };
  }
  if (domain !== "agents" && domain !== "skills" && domain !== "sessions") return next;
  return { ...next, sources: sourcesFor(next.agents, next.skills, next.sessions) };
}

export function normalizeReport(
  report: Record<string, unknown> | null | undefined,
): RuntimeData {
  const reportRows = (key: "agents" | "skills" | "prompts" | "sessions" | "rules" | "hooks" | "servers"): unknown[] => {
    const value = report?.[key === "servers" ? "mcp" : key];
    if (!value || typeof value !== "object" || Array.isArray(value)) return [];
    const rows = (value as Record<string, unknown>)[key];
    return Array.isArray(rows) ? rows : [];
  };
  const skillsList = reportRows("skills");
  const promptsList = reportRows("prompts");
  const sessionsList = reportRows("sessions");
  const agentsList = reportRows("agents");

  const skills = skillsList.flatMap((skill) => {
    const normalized = normalizeSkill(skill as Record<string, unknown>);
    return normalized ? [normalized] : [];
  });
  const prompts = promptsList.flatMap((prompt) => {
    const normalized = normalizePrompt(prompt as Record<string, unknown>);
    return normalized ? [normalized] : [];
  });
  const sessions = sessionsList.flatMap((session) => {
    const normalized = normalizeSession(session as Record<string, unknown>);
    return normalized ? [normalized] : [];
  });
  const rulesList = reportRows("rules");
  const hooksList = reportRows("hooks");
  const mcpList = reportRows("servers");

  const normalizedAgents = agentsList as Record<string, unknown>[];
  const normalizedSessions = sessions;
  const normalizedRules = rulesList.flatMap((rule) => {
    const normalized = normalizeRule(rule as Record<string, unknown>);
    return normalized ? [normalized] : [];
  });
  const normalizedHooks = hooksList.flatMap((hook) => {
    const normalized = normalizeHook(hook as Record<string, unknown>);
    return normalized ? [normalized] : [];
  });
  const normalizedMcp = mcpList.flatMap((server) => {
    const normalized = normalizeMcp(server as Record<string, unknown>);
    return normalized ? [normalized] : [];
  });
  const normalizedSkills = skills;
  return {
    agents: normalizedAgents,
    skills: normalizedSkills,
    prompts,
    sessions: normalizedSessions,
    rules: normalizedRules,
    hooks: normalizedHooks,
    mcp: normalizedMcp,
    sources: sourcesFor(normalizedAgents, normalizedSkills, normalizedSessions),
  };
}

export function emptyRuntimeData(): RuntimeData {
  return {
    agents: [],
    skills: [],
    prompts: [],
    sessions: [],
    rules: [],
    hooks: [],
    mcp: [],
    sources: [],
  };
}

export function initialData(): RuntimeData {
  return emptyRuntimeData();
}
