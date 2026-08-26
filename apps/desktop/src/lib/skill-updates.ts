import type { RuntimeData } from "./data.ts";

export type SkillUpdateReport = {
  name: string;
  status: string;
};

function sameAgentLabel(value: unknown, expected: string): boolean {
  return `${value ?? ""}`.trim().toLowerCase() === expected.trim().toLowerCase();
}

export function applySkillUpdateReportsToData(data: RuntimeData, updates: SkillUpdateReport[]): RuntimeData {
  if (!Array.isArray(updates) || updates.length === 0) return data;
  const byName = new Map(updates.map((update) => [update.name, update]));
  return {
    ...data,
    skills: data.skills.map((skill) => {
      const update = byName.get(skill.name);
      if (!update) return skill;
      return {
        ...skill,
        updateStatus: update.status,
        meta: update.status,
        statusTone: update.status === "update-available" ? "warn" : skill.statusTone,
      };
    }),
  };
}

export function countSkillUpdates(data: RuntimeData, agentFilter: string): number {
  return data.skills.filter((skill) => (
    skill.updateStatus === "update-available"
    && (agentFilter === "All" || skill.agents.some((agent) => sameAgentLabel(agent, agentFilter)))
  )).length;
}
