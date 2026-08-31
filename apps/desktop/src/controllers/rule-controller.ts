import { ruleKey, ruleSearchText, type RuleRecord } from "../lib/rules.ts";

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
