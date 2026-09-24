import { ruleKey, ruleSearchText, RuleScope, type RuleRecord } from "../lib/rules.ts";

export type RuleListItem = { key: string; rule: RuleRecord };
export type RuleTableItem = RuleListItem & { id: string };

export type RuleListView = {
  items: RuleListItem[];
  filteredItems: RuleListItem[];
  tableRows: RuleTableItem[];
};

type RuleListViewCacheEntry = { query: string; view: RuleListView };
const ruleListViewCache = new WeakMap<readonly RuleRecord[], RuleListViewCacheEntry[]>();

export function selectRuleListView(rows: readonly RuleRecord[], query: string): RuleListView {
  const normalizedQuery = query.trim().toLowerCase();
  const cached = ruleListViewCache.get(rows)?.find((entry) => entry.query === normalizedQuery);
  if (cached) return cached.view;
  const items = rows.map((rule) => ({ key: ruleKey(rule), rule }));
  const filteredItems = normalizedQuery
    ? items.filter((item) => ruleSearchText(item.rule).includes(normalizedQuery))
    : items;
  const tableRows = filteredItems.map((item) => ({ ...item, id: item.key }));
  const view = { items, filteredItems, tableRows };
  const entries = ruleListViewCache.get(rows) ?? [];
  entries.push({ query: normalizedQuery, view });
  ruleListViewCache.set(rows, entries);
  return view;
}

type ReferencedSkill = {
  id?: string;
  name: string;
  paths?: readonly { scope?: string | null }[];
};

function normalizeSkillName(value: string): string {
  return value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-+|-+$/g, "");
}

function preferReferencedSkill<T extends ReferencedSkill>(matches: T[], rule: RuleRecord): T | undefined {
  if (matches.length === 1) return matches[0];
  if (rule.scope === RuleScope.Project) {
    const projectMatches = matches.filter((skill) =>
      (skill.paths ?? []).some((path) => path.scope === "project"),
    );
    if (projectMatches.length === 1) return projectMatches[0];
  }
  const globalMatches = matches.filter((skill) => (skill.paths ?? []).every((path) => path.scope !== "project"));
  return globalMatches.length === 1 ? globalMatches[0] : undefined;
}

export function ruleSkillReferences<T extends ReferencedSkill>(
  content: string,
  skills: readonly T[],
  rule: RuleRecord | null,
): T[] {
  if (!rule) return [];
  const byName = new Map<string, T[]>();
  for (const skill of skills) {
    const key = normalizeSkillName(skill.name);
    const group = byName.get(key) ?? [];
    group.push(skill);
    byName.set(key, group);
  }
  const references = new Map<string, T>();
  for (const match of content.matchAll(/(?:^|[^\w])[$/]([a-zA-Z0-9][a-zA-Z0-9_-]*)/g)) {
    const candidates = byName.get(normalizeSkillName(match[1]));
    if (!candidates?.length) continue;
    const preferred = preferReferencedSkill(candidates, rule);
    if (!preferred?.id) continue;
    references.set(preferred.id, preferred);
  }
  return [...references.values()].sort((left, right) => left.name.localeCompare(right.name));
}
