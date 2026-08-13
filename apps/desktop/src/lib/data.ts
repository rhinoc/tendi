import {
  fallbackAgents,
  fallbackSessions,
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

  const agentCounts = new Map<string, number>();
  for (const agent of agentsList) {
    const record = agent as Record<string, unknown>;
    if (record.installed !== true) continue;
    const label = friendlyAgent(record.name ?? record.kind);
    if (label) agentCounts.set(label, agentCounts.get(label) ?? 0);
  }
  for (const skill of skills) {
    for (const agent of skill.agents) agentCounts.set(agent, (agentCounts.get(agent) ?? 0) + 1);
  }
  for (const session of sessions) {
    const agent = `${session.agent ?? ""}`;
    if (agent) agentCounts.set(agent, Math.max(agentCounts.get(agent) ?? 0, 1));
  }
  return {
    agents: agentsList as Record<string, unknown>[],
    skills: skills.length || !fallback ? skills : fallbackSkills.map((skill, index) => normalizeSkill(skill as Record<string, unknown>, index)),
    prompts,
    sessions: sessions.length || !fallback ? sessions : fallbackSessions.map((session, index) => normalizeSession(session as Record<string, unknown>, index)),
    rules: rulesList as Record<string, unknown>[],
    hooks: hooksList as Record<string, unknown>[],
    mcp: mcpList as Record<string, unknown>[],
    sources: [...agentCounts].map(([label, count]) => ({ label, count })).sort((a, b) => a.label.localeCompare(b.label)),
  };
}

export function fallbackData(): RuntimeData {
  return {
    agents: [],
    skills: fallbackSkills.map((skill, index) => normalizeSkill(skill as Record<string, unknown>, index)),
    prompts: [],
    sessions: fallbackSessions.map((session, index) => normalizeSession(session as Record<string, unknown>, index)),
    rules: [],
    hooks: [],
    mcp: [],
    sources: fallbackAgents,
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
