import { useSyncExternalStore } from "react";

import type { OverviewAnalytics } from "../lib/analytics.ts";
import type { RuntimeData } from "../lib/data.ts";
import { DOMAIN_KEYS, type DomainKey, type RuntimeDomainKey } from "../lib/domain.ts";
import { applySkillUpdateReportsToData, countSkillUpdates } from "../lib/skill-updates.ts";
import type { SkillUpdateReport } from "../lib/skill-updates.ts";

export type { SkillUpdateReport } from "../lib/skill-updates.ts";

export type DesktopDomain = RuntimeDomainKey;
export type DomainLoadStatus = "idle" | "loading" | "ready" | "error";
export type SessionListStatus = "loading" | "loaded" | "error";
export type AgentTargetOption = {
  id: string;
  displayName: string;
  supportsGlobal: boolean;
  globalPath?: string;
};
export type SkillIndexStatus = {
  total?: number;
  indexed?: number;
  pending?: number;
  failed?: number;
  running?: boolean;
  last_indexed_at?: string | null;
};

export type AnalyticsQueryKey = {
  agent: string;
  range: number;
  revision: number;
};

function sameSkillIndexStatus(left: SkillIndexStatus | null, right: SkillIndexStatus | null) {
  if (left === right) return true;
  if (!left || !right) return false;
  return left.total === right.total
    && left.indexed === right.indexed
    && left.pending === right.pending
    && left.failed === right.failed
    && left.running === right.running
    && left.last_indexed_at === right.last_indexed_at;
}

export type DomainErrorState = Partial<Record<DesktopDomain, string>>;

export type DesktopStoreState = {
  catalogs: {
    data: RuntimeData;
    agentTargets: AgentTargetOption[];
    loadingDomains: ReadonlySet<DesktopDomain>;
    loadedDomains: ReadonlySet<DesktopDomain>;
    errors: DomainErrorState;
    retryRevision: number;
  };
  skillUpdates: {
    checking: boolean;
    error: string;
    fresh: boolean;
    indexStatus: SkillIndexStatus | null;
  };
  sessions: {
    refreshError: string;
  };
  analytics: {
    revision: number;
    ready: boolean;
    error: string;
    value: OverviewAnalytics | null;
    valueQueryKey: AnalyticsQueryKey | null;
  };
};

type StateUpdater<T> = T | ((current: T) => T);

function resolve<T>(current: T, next: StateUpdater<T>): T {
  return typeof next === "function"
    ? (next as (current: T) => T)(current)
    : next;
}

function initialData(): RuntimeData {
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

function createInitialState(): DesktopStoreState {
  return {
    catalogs: {
      data: initialData(),
      agentTargets: [],
      loadingDomains: new Set(),
      loadedDomains: new Set(),
      errors: {},
      retryRevision: 0,
    },
    skillUpdates: {
      checking: false,
      error: "",
      fresh: false,
      indexStatus: null,
    },
    sessions: {
      refreshError: "",
    },
    analytics: {
      revision: 0,
      ready: false,
      error: "",
      value: null,
      valueQueryKey: null,
    },
  };
}

function inventoryRows(data: RuntimeData, domain: DomainKey): unknown[] {
  return data[domain] as unknown[];
}

export function selectCatalogCounts(data: RuntimeData): Record<DomainKey, number> {
  return Object.fromEntries(DOMAIN_KEYS.map((domain) => [domain, inventoryRows(data, domain).length])) as Record<DomainKey, number>;
}

export function selectCatalogCountLoadedDomains(state: DesktopStoreState): ReadonlySet<DomainKey> {
  return new Set(DOMAIN_KEYS.filter((domain) => (
    state.catalogs.loadedDomains.has(domain) || inventoryRows(state.catalogs.data, domain).length > 0
  )));
}

export function selectCatalogCountErrors(state: DesktopStoreState): ReadonlySet<DomainKey> {
  return new Set(DOMAIN_KEYS.filter((domain) => Boolean(state.catalogs.errors[domain])));
}

export function selectHookReviewCount(data: RuntimeData): number {
  return data.hooks.filter((hook) => hook.needs_review === true).length;
}

export function selectSkillUpdateCount(data: RuntimeData, agentFilter: string): number {
  return countSkillUpdates(data, agentFilter);
}

export function selectSessionListStatus(state: DesktopStoreState): SessionListStatus {
  const hasRows = state.catalogs.data.sessions.length > 0;
  if (state.catalogs.loadingDomains.has("sessions")) return "loading";
  if (!hasRows && state.catalogs.errors.sessions) return "error";
  if (state.catalogs.loadedDomains.has("sessions") || hasRows) return "loaded";
  return "loading";
}

function sameAnalyticsQueryKey(left: AnalyticsQueryKey | null, right: AnalyticsQueryKey | null): boolean {
  if (left === right) return true;
  if (!left || !right) return false;
  return left.agent === right.agent && left.range === right.range && left.revision === right.revision;
}

export function selectAnalyticsValue(
  state: DesktopStoreState,
  queryKey: AnalyticsQueryKey,
): OverviewAnalytics | null {
  return sameAnalyticsQueryKey(state.analytics.valueQueryKey, queryKey) ? state.analytics.value : null;
}

function updateSet<T>(current: ReadonlySet<T>, value: T, present: boolean): ReadonlySet<T> {
  if (current.has(value) === present) return current;
  const next = new Set(current);
  if (present) next.add(value);
  else next.delete(value);
  return next;
}

function updateDomainErrors(
  current: DomainErrorState,
  domain: DesktopDomain,
  message: string,
): DomainErrorState {
  if (!message && !current[domain]) return current;
  if (current[domain] === message) return current;
  const next = { ...current };
  if (message) next[domain] = message;
  else delete next[domain];
  return next;
}

export type DesktopStoreActions = {
  updateData: (updater: StateUpdater<RuntimeData>) => void;
  setAgentTargets: (targets: AgentTargetOption[]) => void;
  setDomainLoading: (domain: DesktopDomain, loading: boolean) => void;
  setDomainError: (domain: DesktopDomain, message: string) => void;
  markDomainLoaded: (domain: DesktopDomain, loaded?: boolean) => void;
  bumpDomainRetryRevision: () => void;
  setSkillUpdatesChecking: (checking: boolean) => void;
  setSkillUpdateError: (message: string) => void;
  setSkillIndexStatus: (status: SkillIndexStatus | null) => void;
  setSkillUpdateReports: (updates: SkillUpdateReport[]) => void;
  setSessionRefreshError: (message: string) => void;
  setAnalyticsRevision: (revision: number) => void;
  setAnalyticsReady: (ready: boolean) => void;
  setAnalyticsError: (message: string) => void;
  setAnalyticsValue: (value: OverviewAnalytics | null, queryKey: AnalyticsQueryKey | null) => void;
};

export class DesktopStore {
  private state: DesktopStoreState = createInitialState();
  private listeners = new Set<() => void>();

  readonly actions: DesktopStoreActions = {
    updateData: (updater) => {
      this.update((current) => {
        const data = resolve(current.catalogs.data, updater);
        if (data === current.catalogs.data) return current;
        return { ...current, catalogs: { ...current.catalogs, data } };
      });
    },
    setAgentTargets: (agentTargets) => {
      this.update((current) => current.catalogs.agentTargets === agentTargets
        ? current
        : { ...current, catalogs: { ...current.catalogs, agentTargets } });
    },
    setDomainLoading: (domain, loading) => {
      this.update((current) => {
        const loadingDomains = updateSet(current.catalogs.loadingDomains, domain, loading);
        if (loadingDomains === current.catalogs.loadingDomains) return current;
        return { ...current, catalogs: { ...current.catalogs, loadingDomains } };
      });
    },
    setDomainError: (domain, message) => {
      this.update((current) => {
        const errors = updateDomainErrors(current.catalogs.errors, domain, message);
        if (errors === current.catalogs.errors) return current;
        return { ...current, catalogs: { ...current.catalogs, errors } };
      });
    },
    markDomainLoaded: (domain, loaded = true) => {
      this.update((current) => {
        const loadedDomains = updateSet(current.catalogs.loadedDomains, domain, loaded);
        if (loadedDomains === current.catalogs.loadedDomains) return current;
        return { ...current, catalogs: { ...current.catalogs, loadedDomains } };
      });
    },
    bumpDomainRetryRevision: () => {
      this.update((current) => ({
        ...current,
        catalogs: {
          ...current.catalogs,
          retryRevision: current.catalogs.retryRevision + 1,
        },
      }));
    },
    setSkillUpdatesChecking: (checking) => {
      this.update((current) => current.skillUpdates.checking === checking
        ? current
        : { ...current, skillUpdates: { ...current.skillUpdates, checking } });
    },
    setSkillUpdateError: (message) => {
      this.update((current) => current.skillUpdates.error === message
        ? current
        : { ...current, skillUpdates: { ...current.skillUpdates, error: message } });
    },
    setSkillIndexStatus: (indexStatus) => {
      this.update((current) => sameSkillIndexStatus(current.skillUpdates.indexStatus, indexStatus)
        ? current
        : { ...current, skillUpdates: { ...current.skillUpdates, indexStatus } });
    },
    setSkillUpdateReports: (updates) => {
      this.update((current) => {
        const data = applySkillUpdateReportsToData(current.catalogs.data, updates);
        return {
          ...current,
          catalogs: { ...current.catalogs, data },
          skillUpdates: { ...current.skillUpdates, fresh: true },
        };
      });
    },
    setSessionRefreshError: (refreshError) => {
      this.update((current) => current.sessions.refreshError === refreshError
        ? current
        : { ...current, sessions: { refreshError } });
    },
    setAnalyticsRevision: (revision) => {
      this.update((current) => {
        const nextRevision = Math.max(current.analytics.revision, revision);
        if (nextRevision === current.analytics.revision) return current;
        return { ...current, analytics: { ...current.analytics, revision: nextRevision } };
      });
    },
    setAnalyticsReady: (ready) => {
      this.update((current) => current.analytics.ready === ready
        ? current
        : { ...current, analytics: { ...current.analytics, ready } });
    },
    setAnalyticsError: (message) => {
      this.update((current) => current.analytics.error === message
        ? current
        : { ...current, analytics: { ...current.analytics, error: message } });
    },
    setAnalyticsValue: (value, valueQueryKey) => {
      if (value !== null && valueQueryKey === null) {
        throw new Error("Analytics values require a query key");
      }
      this.update((current) => current.analytics.value === value
        && sameAnalyticsQueryKey(current.analytics.valueQueryKey, valueQueryKey)
        ? current
        : { ...current, analytics: { ...current.analytics, value, valueQueryKey } });
    },
  };

  getSnapshot = (): DesktopStoreState => this.state;

  subscribe = (listener: () => void): (() => void) => {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  };

  private update(updater: (current: DesktopStoreState) => DesktopStoreState) {
    const next = updater(this.state);
    if (next === this.state) return;
    this.state = next;
    for (const listener of this.listeners) listener();
  }
}

export const desktopStore = new DesktopStore();

export function useDesktopStore<T>(selector: (state: DesktopStoreState) => T): T {
  const state = useSyncExternalStore(
    desktopStore.subscribe,
    desktopStore.getSnapshot,
    desktopStore.getSnapshot,
  );
  return selector(state);
}

export function selectCatalogData(state: DesktopStoreState): RuntimeData {
  return state.catalogs.data;
}
