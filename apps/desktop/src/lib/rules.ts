import { basename } from "./strings.ts";
import { friendlyAgent } from "./agents.ts";

export type RuleRecord = {
  agents?: string[] | null;
  agent?: string | null;
  kind?: string | null;
  scope?: string | null;
  order?: number | null;
  path?: string | null;
  source?: string | null;
};

export type RuleRow = { rule: RuleRecord };

export type SortState = { key: string; direction: "asc" | "desc" };

export function ruleAgents(rule: RuleRecord | null | undefined): string[] {
  if (Array.isArray(rule?.agents) && rule.agents.length > 0) return rule.agents;
  return rule?.agent ? [rule.agent] : [];
}

export function ruleTitle(rule: RuleRecord | null | undefined): string {
  return basename(rule?.path ?? rule?.source ?? rule?.kind ?? "Rule");
}

export function ruleKey(rule: RuleRecord | null | undefined, index = 0): string {
  const path = `${rule?.path ?? rule?.source ?? ""}`;
  return path
    ? path
    : `${rule?.kind ?? "rule"}:${rule?.scope ?? ""}:${index}`;
}

export function ruleSearchText(rule: RuleRecord | null | undefined): string {
  return [...ruleAgents(rule), rule?.kind, rule?.scope, rule?.order, rule?.path, rule?.source, ruleTitle(rule)]
    .map((value) => `${value ?? ""}`.toLowerCase())
    .join(" ");
}

export function ruleSortValue(item: RuleRow, key: string): string | number {
  const rule = item.rule;
  if (key === "source") return ruleTitle(rule).toLowerCase();
  if (key === "agents") return ruleAgents(rule).map((agent) => friendlyAgent(agent)).join(",").toLowerCase();
  if (key === "kind") return `${rule.kind ?? ""}`.toLowerCase();
  if (key === "scope") return `${rule.scope ?? ""}`.toLowerCase();
  if (key === "order") return Number(rule.order) || 0;
  return "";
}

export function compareRules(a: RuleRow, b: RuleRow, sort: SortState): number {
  const left = ruleSortValue(a, sort.key);
  const right = ruleSortValue(b, sort.key);
  const direction = sort.direction === "asc" ? 1 : -1;
  if (typeof left === "number" || typeof right === "number") {
    return ((Number(left) || 0) - (Number(right) || 0)) * direction;
  }
  return `${left}`.localeCompare(`${right}`) * direction;
}
