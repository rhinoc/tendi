import type { RuntimeData } from "./data.ts";
import { applySkillUpdateReports } from "./skills.ts";

export type SkillUpdateReport = {
  name: string;
  status: string;
};

function sameAgentLabel(value: unknown, expected: string): boolean {
  return `${value ?? ""}`.trim().toLowerCase() === expected.trim().toLowerCase();
}

export function applySkillUpdateReportsToData(data: RuntimeData, updates: SkillUpdateReport[]): RuntimeData {
  return applySkillUpdateReports(data, updates);
}

export function countSkillUpdates(data: RuntimeData, agentFilter: string): number {
  return data.skills.filter((skill) => (
    skill.updateAvailability === "update-available"
    && (agentFilter === "All" || skill.agents.some((agent) => sameAgentLabel(agent, agentFilter)))
  )).length;
}
