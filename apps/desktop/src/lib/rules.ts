import { basename } from "./strings.ts";
import { friendlyAgent } from "./agents.ts";
import { SortDirection } from "./sort.ts";

export type RuleRecord = {
  agents: string[];
  kind: string;
  scope: string;
  order: number;
  path: string;
  sha256: string;
};

export enum RuleScope {
  Global = "global",
  Project = "project",
}

export type RuleRow = { rule: RuleRecord };

export type SortState = { key: string; direction: SortDirection };

export function ruleAgents(rule: RuleRecord): string[] {
  return rule.agents;
}

export function ruleTitle(rule: { path?: string | null } | null | undefined): string {
  return basename(rule?.path ?? "");
}

export function ruleKey(rule: Pick<RuleRecord, "path">): string {
  return rule.path;
}

function requiredString(value: unknown): string | undefined {
  if (typeof value !== "string") return undefined;
  const normalized = value.trim();
  return normalized ? normalized : undefined;
}

export function normalizeRule(rule: Record<string, unknown>): RuleRecord | undefined {
  const path = requiredString(rule.path);
  const kind = requiredString(rule.kind);
  const scope = requiredString(rule.scope);
  const sha256 = requiredString(rule.sha256);
  const order = typeof rule.order === "number" && Number.isInteger(rule.order) && rule.order >= 0
    ? rule.order
    : undefined;
  if (!path || !kind || !scope || !sha256 || order === undefined || !Array.isArray(rule.agents) || rule.agents.length === 0) return undefined;
  const agents = rule.agents.filter((agent): agent is string => typeof agent === "string" && agent.trim().length > 0);
  if (agents.length !== rule.agents.length) return undefined;
  return { agents, kind, scope, order, path, sha256 };
}

export function ruleSearchText(rule: RuleRecord): string {
  return [...rule.agents, rule.kind, rule.scope, rule.order, rule.path, ruleTitle(rule)]
    .map((value) => `${value}`.toLowerCase())
    .join(" ");
}

export function ruleSortValue(item: RuleRow, key: string): string | number {
  const rule = item.rule;
  if (key === "source") return ruleTitle(rule).toLowerCase();
  if (key === "agents") return ruleAgents(rule).map((agent) => friendlyAgent(agent)).join(",").toLowerCase();
  if (key === "kind") return rule.kind.toLowerCase();
  if (key === "scope") return rule.scope.toLowerCase();
  if (key === "order") return rule.order;
  return "";
}

export function compareRules(a: RuleRow, b: RuleRow, sort: SortState): number {
  const left = ruleSortValue(a, sort.key);
  const right = ruleSortValue(b, sort.key);
  const direction = sort.direction === SortDirection.Asc ? 1 : -1;
  if (typeof left === "number" || typeof right === "number") {
    return ((Number(left) || 0) - (Number(right) || 0)) * direction;
  }
  return `${left}`.localeCompare(`${right}`) * direction;
}
