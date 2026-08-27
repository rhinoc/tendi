import { useSyncExternalStore } from "react";

import type { OverviewAnalytics } from "../lib/analytics.ts";
import type { RuntimeData } from "../lib/data.ts";
import { applySkillUpdateReportsToData, countSkillUpdates } from "../lib/skill-updates.ts";
import type { SkillUpdateReport } from "../lib/skill-updates.ts";

export type { SkillUpdateReport } from "../lib/skill-updates.ts";

export type DesktopDomain = "agents" | "skills" | "sessions" | "prompts" | "rules" | "hooks" | "mcp";
export type InventoryKey = "skills" | "prompts" | "sessions" | "rules" | "hooks" | "mcp";
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
  inventory: {
    counts: Record<InventoryKey, number>;
    loaded: ReadonlySet<InventoryKey>;
    errors: ReadonlySet<InventoryKey>;
    hookReviewCount: number;
    skillUpdateCount: number;
  };
  skillUpdates: {
    checking: boolean;
    error: string;
    fresh: boolean;
    indexStatus: SkillIndexStatus | null;
    reports: ReadonlyMap<string, SkillUpdateReport>;
  };
  sessions: {
    listStatus: SessionListStatus;
    listError: string;
    refreshError: string;
    listLoaded: boolean;
    rowsAvailable: boolean;
  };
  analytics: {
    revision: number;
    ready: boolean;
    error: string;
    value: OverviewAnalytics | null;
  };
};

type StateUpdater<T> = T | ((current: T) => T);

function resolve<T>(current: T, next: StateUpdater<T>): T {
  return typeof next === "function"
    ? (next as (current: T) => T)(current)
    : next;
}

function emptyCounts(): Record<InventoryKey, number> {
  return {
    skills: 0,
    sessions: 0,
    prompts: 0,
    rules: 0,
    hooks: 0,
    mcp: 0,
  };
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
    inventory: {
      counts: emptyCounts(),
      loaded: new Set(),
      errors: new Set(),
      hookReviewCount: 0,
      skillUpdateCount: 0,
    },
    skillUpdates: {
      checking: false,
      error: "",
      fresh: false,
      indexStatus: null,
      reports: new Map(),
    },
    sessions: {
      listStatus: "loading",
      listError: "",
      refreshError: "",
      listLoaded: false,
      rowsAvailable: false,
    },
    analytics: {
      revision: 0,
      ready: false,
      error: "",
      value: null,
    },
  };
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
  resetInventory: (agentFilter?: string) => void;
  setInventoryCount: (domain: InventoryKey, count: number, secondaryCount?: number) => void;
  setInventoryError: (domain: InventoryKey, hasError: boolean) => void;
  setSkillUpdatesChecking: (checking: boolean) => void;
  setSkillUpdateError: (message: string) => void;
  setSkillIndexStatus: (status: SkillIndexStatus | null) => void;
  setSkillUpdateReports: (updates: SkillUpdateReport[], agentFilter?: string) => void;
  clearSkillUpdateReports: (names: string[], agentFilter?: string) => void;
  refreshSkillUpdateCount: (agentFilter?: string) => void;
  setSessionListState: (patch: Partial<DesktopStoreState["sessions"]>) => void;
  setAnalyticsRevision: (revision: number) => void;
  setAnalyticsReady: (ready: boolean) => void;
  setAnalyticsError: (message: string) => void;
  setAnalyticsValue: (value: OverviewAnalytics | null) => void;
};

class DesktopStore {
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
    resetInventory: (agentFilter = "All") => {
      this.update((current) => ({
        ...current,
        inventory: {
          ...current.inventory,
          loaded: new Set(),
          errors: new Set(),
          hookReviewCount: 0,
          skillUpdateCount: current.skillUpdates.fresh
            ? countSkillUpdates(current.catalogs.data, agentFilter)
            : 0,
        },
      }));
    },
    setInventoryCount: (domain, count, secondaryCount = 0) => {
      this.update((current) => {
        const loaded = updateSet(current.inventory.loaded, domain, true);
        const errors = updateSet(current.inventory.errors, domain, false);
        const hookReviewCount = domain === "hooks" ? secondaryCount : current.inventory.hookReviewCount;
        const skillUpdateCount = domain === "skills" && !current.skillUpdates.fresh
          ? secondaryCount
          : current.inventory.skillUpdateCount;
        if (
          current.inventory.counts[domain] === count
          && loaded === current.inventory.loaded
          && errors === current.inventory.errors
          && hookReviewCount === current.inventory.hookReviewCount
          && skillUpdateCount === current.inventory.skillUpdateCount
        ) return current;
        return {
          ...current,
          inventory: {
            ...current.inventory,
            counts: { ...current.inventory.counts, [domain]: count },
            loaded,
            errors,
            hookReviewCount,
            skillUpdateCount,
          },
        };
      });
    },
    setInventoryError: (domain, hasError) => {
      this.update((current) => {
        const errors = updateSet(current.inventory.errors, domain, hasError);
        if (errors === current.inventory.errors) return current;
        return { ...current, inventory: { ...current.inventory, errors } };
      });
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
    setSkillUpdateReports: (updates, agentFilter = "All") => {
      this.update((current) => {
        const reports = new Map(updates.map((update) => [update.name, update]));
        const data = applySkillUpdateReportsToData(current.catalogs.data, updates);
        const skillUpdateCount = countSkillUpdates(data, agentFilter);
        return {
          ...current,
          catalogs: { ...current.catalogs, data },
          inventory: { ...current.inventory, skillUpdateCount },
          skillUpdates: { ...current.skillUpdates, fresh: true, reports },
        };
      });
    },
    clearSkillUpdateReports: (names, agentFilter = "All") => {
      this.update((current) => {
        if (names.length === 0) return current;
        const reports = new Map(current.skillUpdates.reports);
        for (const name of names) reports.delete(name);
        const skillUpdateCount = countSkillUpdates(current.catalogs.data, agentFilter);
        return {
          ...current,
          inventory: { ...current.inventory, skillUpdateCount },
          skillUpdates: { ...current.skillUpdates, fresh: true, reports },
        };
      });
    },
    refreshSkillUpdateCount: (agentFilter = "All") => {
      this.update((current) => {
        if (!current.skillUpdates.fresh) return current;
        const skillUpdateCount = countSkillUpdates(current.catalogs.data, agentFilter);
        if (skillUpdateCount === current.inventory.skillUpdateCount) return current;
        return { ...current, inventory: { ...current.inventory, skillUpdateCount } };
      });
    },
    setSessionListState: (patch) => {
      this.update((current) => {
        const changed = (Object.keys(patch) as Array<keyof DesktopStoreState["sessions"]>)
          .some((key) => patch[key] !== undefined && patch[key] !== current.sessions[key]);
        if (!changed) return current;
        return { ...current, sessions: { ...current.sessions, ...patch } };
      });
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
    setAnalyticsValue: (value) => {
      this.update((current) => current.analytics.value === value
        ? current
        : { ...current, analytics: { ...current.analytics, value } });
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
