import {
  fallbackSkills,
} from "./constants.ts";
import { normalizePrompt } from "./prompt-model.ts";
import { normalizeSession } from "./sessions.ts";
import { normalizeSkill, type NormalizedSkill } from "./skills.ts";
import { friendlyAgent } from "./agents.ts";

export type RuntimeData = {
  agents: Record<string, unknown>[];
  skills: NormalizedSkill[];
  prompts: ReturnType<typeof normalizePrompt>[];
  sessions: ReturnType<typeof normalizeSession>[];
  rules: Record<string, unknown>[];
  hooks: Record<string, unknown>[];
  mcp: Record<string, unknown>[];
  sources: Array<{ label: string; count: number }>;
  [key: string]: unknown;
};

export type RuntimeDomainKey = "agents" | "skills" | "prompts" | "sessions" | "rules" | "hooks" | "mcp";

function domainList(value: unknown, key: "agents" | "skills" | "prompts" | "sessions" | "rules" | "hooks" | "servers"): unknown[] {
  if (Array.isArray(value)) return value;
  if (!value || typeof value !== "object") return [];
  const rows = (value as Record<string, unknown>)[key];
  return Array.isArray(rows) ? rows : [];
}

function sourcesFor(agents: Record<string, unknown>[], skills: NormalizedSkill[], sessions: ReturnType<typeof normalizeSession>[]) {
  const agentCounts = new Map<string, number>();
  for (const agent of agents) {
    if (agent.installed !== true) continue;
    const label = friendlyAgent(agent.name ?? agent.kind);
    if (label) agentCounts.set(label, agentCounts.get(label) ?? 0);
  }
  for (const skill of skills) {
    for (const agent of skill.agents) agentCounts.set(agent, (agentCounts.get(agent) ?? 0) + 1);
  }
  for (const session of sessions) {
    const agent = `${session.agent ?? ""}`;
    if (agent) agentCounts.set(agent, Math.max(agentCounts.get(agent) ?? 0, 1));
  }
  return [...agentCounts]
    .map(([label, count]) => ({ label, count }))
    .sort((left, right) => left.label.localeCompare(right.label));
}

export function normalizeDomainRows(current: RuntimeData, domain: RuntimeDomainKey, value: unknown): RuntimeData {
  const rows = domainList(value, domain === "mcp" ? "servers" : domain);
  let next: RuntimeData;
  if (domain === "agents") {
    next = { ...current, agents: rows as Record<string, unknown>[] };
  } else if (domain === "skills") {
    next = { ...current, skills: rows.map((row, index) => normalizeSkill(row as Record<string, unknown>, index)) };
  } else if (domain === "prompts") {
    next = { ...current, prompts: rows.map((row, index) => normalizePrompt(row as Record<string, unknown>, index)) };
  } else if (domain === "sessions") {
    next = { ...current, sessions: rows.map((row, index) => normalizeSession(row as Record<string, unknown>, index)) };
  } else {
    next = { ...current, [domain]: rows as Record<string, unknown>[] };
  }
  if (domain !== "agents" && domain !== "skills" && domain !== "sessions") return next;
  return { ...next, sources: sourcesFor(next.agents, next.skills, next.sessions) };
}

export function normalizeReport(
  report: Record<string, unknown> | null | undefined,
  { fallback = true }: { fallback?: boolean } = {},
): RuntimeData {
  const skillsRaw = (report?.skills as { skills?: unknown[] } | unknown[]) ?? [];
  const promptsRaw = (report?.prompts as { prompts?: unknown[] } | unknown[]) ?? [];
  const sessionsRaw = (report?.sessions as { sessions?: unknown[] } | unknown[]) ?? [];
  const agentsRaw = (report?.agents as { agents?: unknown[] } | unknown[]) ?? [];
  const skillsList = Array.isArray(skillsRaw)
    ? skillsRaw
    : (skillsRaw as { skills?: unknown[] }).skills ?? [];
  const promptsList = Array.isArray(promptsRaw)
    ? promptsRaw
    : (promptsRaw as { prompts?: unknown[] }).prompts ?? [];
  const sessionsList = Array.isArray(sessionsRaw)
    ? sessionsRaw
    : (sessionsRaw as { sessions?: unknown[] }).sessions ?? [];
  const agentsList = Array.isArray(agentsRaw)
    ? agentsRaw
    : (agentsRaw as { agents?: unknown[] }).agents ?? [];

  const skills = skillsList.map((skill, index) => normalizeSkill(skill as Record<string, unknown>, index));
  const prompts = promptsList.map((prompt, index) => normalizePrompt(prompt as Record<string, unknown>, index));
  const sessions = sessionsList.map((session, index) => normalizeSession(session as Record<string, unknown>, index));
  const rules = (report?.rules as { rules?: unknown[] } | unknown[]) ?? [];
  const rulesList = Array.isArray(rules) ? rules : (rules as { rules?: unknown[] }).rules ?? [];
  const hooks = (report?.hooks as { hooks?: unknown[] } | unknown[]) ?? [];
  const hooksList = Array.isArray(hooks) ? hooks : (hooks as { hooks?: unknown[] }).hooks ?? [];
  const mcp = (report?.mcp as { servers?: unknown[] } | unknown[]) ?? [];
  const mcpList = Array.isArray(mcp) ? mcp : (mcp as { servers?: unknown[] }).servers ?? [];

  const normalizedAgents = agentsList as Record<string, unknown>[];
  const normalizedSessions = sessions;
  const normalizedSkills = skills.length || !fallback ? skills : fallbackSkills.map((skill, index) => normalizeSkill(skill as Record<string, unknown>, index));
  return {
    agents: normalizedAgents,
    skills: normalizedSkills,
    prompts,
    sessions: normalizedSessions,
    rules: rulesList as Record<string, unknown>[],
    hooks: hooksList as Record<string, unknown>[],
    mcp: mcpList as Record<string, unknown>[],
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
