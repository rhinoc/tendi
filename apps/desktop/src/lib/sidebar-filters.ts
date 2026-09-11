import { ALL_AGENT_FILTER } from "./agents.ts";
import { ProjectScopeFilter, normalizeProjectScopeFilter } from "./projects.ts";

export const SIDEBAR_FILTERS_STORAGE_KEY = "tendi.sidebar.filters";

export type SidebarFilterState = {
  agent: string;
  scope: ProjectScopeFilter;
};

const DEFAULT_SIDEBAR_FILTERS: SidebarFilterState = {
  agent: ALL_AGENT_FILTER,
  scope: ProjectScopeFilter.All,
};

export function normalizeSidebarFilters(value: unknown): SidebarFilterState {
  const record = value && typeof value === "object" ? value as Record<string, unknown> : {};
  const agent = typeof record.agent === "string" ? record.agent.trim() : "";
  return {
    agent: agent || DEFAULT_SIDEBAR_FILTERS.agent,
    scope: normalizeProjectScopeFilter(record.scope),
  };
}

export function readCachedSidebarFilters(): SidebarFilterState {
  if (typeof window === "undefined") return { ...DEFAULT_SIDEBAR_FILTERS };
  try {
    const raw = window.localStorage.getItem(SIDEBAR_FILTERS_STORAGE_KEY);
    return normalizeSidebarFilters(raw ? JSON.parse(raw) : null);
  } catch {
    return { ...DEFAULT_SIDEBAR_FILTERS };
  }
}

export function persistSidebarFilters(filters: SidebarFilterState): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(SIDEBAR_FILTERS_STORAGE_KEY, JSON.stringify(normalizeSidebarFilters(filters)));
  } catch {
    // The in-memory filters remain the source of truth if browser storage is unavailable.
  }
}
