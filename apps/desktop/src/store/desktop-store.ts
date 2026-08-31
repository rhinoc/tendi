import { useCallback, useRef, useSyncExternalStore } from "react";

import type { OverviewAnalytics } from "../lib/analytics.ts";
import { Appearance, readCachedAppearance, readCachedFontFamily, readCachedThemePreferences, type FontFamily, type ThemePreferences } from "../lib/appearance.ts";
import { readCachedAppIcon, type AppIcon } from "../lib/app-icon.ts";
import { emptyRuntimeData, type RuntimeData } from "../lib/data.ts";
import type { PromptRecord } from "../lib/prompt-model.ts";
import type { ProjectSummary, SessionProjectSummary } from "../lib/projects.ts";
import { MissingSessionProjectPolicy } from "../lib/projects.ts";
import { SessionResumeTarget, type SessionIdentityRecord } from "../lib/sessions.ts";
import { DOMAIN_KEYS, RuntimeDomainKey, type DomainKey } from "../lib/domain.ts";
import { applySkillUpdateReportsToData } from "../lib/skill-updates.ts";
import type { SkillUpdateReport } from "../lib/skill-updates.ts";
import { decideRevision } from "../lib/runtime-contract.ts";
import { applyDomainSnapshot, applyHookCommandResult as reduceHookCommandResult, applyMcpCommandResult as reduceMcpCommandResult, applyPromptRecord, applyRuleCommandResult as reduceRuleCommandResult, buildCatalogIndexes } from "../controllers/catalog-controller.ts";
import { applySkillPatch, applySkillSnapshot, applySkillVisibility, clearSkillUpdateAvailability } from "../controllers/skill-controller.ts";
import { applySessionDelta } from "../controllers/session-controller.ts";
import { selectOverviewCounts, selectOverviewHookReviewCount, selectOverviewSkillUpdateCount } from "../controllers/overview-controller.ts";
import type { CatalogIndexes, RawDomainRow } from "../controllers/controller-types.ts";

export type { SkillUpdateReport } from "../lib/skill-updates.ts";

export type DesktopDomain = RuntimeDomainKey;
export type DesktopSettingsValues = {
  appearance: Appearance;
  themePreferences: ThemePreferences;
  fontFamily: FontFamily;
  appIcon: AppIcon;
  terminal: string;
  editor: string;
  additionalSessionRoots: string[];
  developerMode: boolean;
  sessionResumeTarget: SessionResumeTarget;
  missingSessionProjectPolicy: MissingSessionProjectPolicy;
  configProfiles: Record<string, string>;
};
export enum DomainLoadStatus {
  Idle = "idle",
  Loading = "loading",
  Ready = "ready",
  Error = "error",
}
export enum SessionListStatus {
  Loading = "loading",
  Loaded = "loaded",
  Error = "error",
}
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
export type DomainRevisionState = Partial<Record<DesktopDomain, number>>;
export enum DomainRevisionDecision {
  Accepted = "accepted",
  Stale = "stale",
  Resync = "resync",
}

export type DesktopStoreState = {
  catalogs: {
    data: RuntimeData;
    agentTargets: AgentTargetOption[];
    loadingDomains: ReadonlySet<DesktopDomain>;
    loadedDomains: ReadonlySet<DesktopDomain>;
    errors: DomainErrorState;
    revisions: DomainRevisionState;
    retryRevision: number;
    indexes: CatalogIndexes;
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
  workspace: {
    projects: ProjectSummary[];
    sessionProjects: SessionProjectSummary[];
  };
  settings: {
    values: DesktopSettingsValues;
    loading: boolean;
    error: string;
  };
  analytics: {
    revision: number;
    ready: boolean;
    error: string;
    value: OverviewAnalytics | null;
    valueQueryKey: AnalyticsQueryKey | null;
  };
};

function createInitialState(): DesktopStoreState {
  const data = emptyRuntimeData();
  return {
    catalogs: {
      data,
      agentTargets: [],
      loadingDomains: new Set(),
      loadedDomains: new Set(),
      errors: {},
      revisions: {},
      retryRevision: 0,
      indexes: buildCatalogIndexes(data),
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
    workspace: {
      projects: [],
      sessionProjects: [],
    },
    settings: {
      values: {
        appearance: readCachedAppearance(),
        themePreferences: readCachedThemePreferences(),
        fontFamily: readCachedFontFamily(),
        appIcon: readCachedAppIcon(),
        terminal: "auto",
        editor: "vscode",
        additionalSessionRoots: [],
        developerMode: false,
        sessionResumeTarget: SessionResumeTarget.Auto,
        missingSessionProjectPolicy: MissingSessionProjectPolicy.Show,
        configProfiles: {},
      },
      loading: true,
      error: "",
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

export function selectCatalogCounts(data: RuntimeData): Record<DomainKey, number> {
  return selectOverviewCounts(data);
}

export function selectCatalogCountLoadedDomains(state: DesktopStoreState): ReadonlySet<DomainKey> {
  return state.catalogs.indexes.loadedDomains;
}

export function selectCatalogCountErrors(state: DesktopStoreState): ReadonlySet<DomainKey> {
  return state.catalogs.indexes.errorDomains;
}

export function selectHookReviewCount(data: RuntimeData): number {
  return selectOverviewHookReviewCount(data);
}

export function selectSkillUpdateCount(data: RuntimeData, agentFilter: string): number {
  return selectOverviewSkillUpdateCount(data, agentFilter);
}

export function selectSessionListStatus(state: DesktopStoreState): SessionListStatus {
  const hasRows = state.catalogs.data.sessions.length > 0;
  if (state.catalogs.loadingDomains.has(RuntimeDomainKey.Sessions)) return SessionListStatus.Loading;
  if (!hasRows && state.catalogs.errors[RuntimeDomainKey.Sessions]) return SessionListStatus.Error;
  if (state.catalogs.loadedDomains.has(RuntimeDomainKey.Sessions) || hasRows) return SessionListStatus.Loaded;
  return SessionListStatus.Loading;
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

export function selectAnalyticsDisplayValue(
  state: DesktopStoreState,
  queryKey: AnalyticsQueryKey,
): OverviewAnalytics | null {
  const storedQueryKey = state.analytics.valueQueryKey;
  if (!storedQueryKey || storedQueryKey.agent !== queryKey.agent || storedQueryKey.range !== queryKey.range) {
    return null;
  }
  return state.analytics.value;
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
  commitDomainSnapshot: (domain: DesktopDomain, rows: readonly RawDomainRow[]) => void;
  commitSessionSnapshot: (rows: readonly RawDomainRow[]) => void;
  applyHookCommandResult: (result: unknown) => unknown;
  applyMcpCommandResult: (result: unknown) => unknown;
  applyRuleCommandResult: (result: unknown) => unknown;
  applySessionDelta: (upserts: readonly RawDomainRow[], deleted?: readonly SessionIdentityRecord[]) => void;
  replaceSkills: (rows: readonly RawDomainRow[]) => void;
  patchSkills: (rows: readonly RawDomainRow[], deleted?: readonly string[]) => void;
  setSkillVisibility: (selectors: readonly string[], visibility: import("../lib/skills.ts").SkillVisibility) => void;
  clearSkillUpdates: (selectors: readonly string[]) => void;
  setPromptRecord: (record: PromptRecord) => void;
  applyPromptRecord: (record: RawDomainRow, bodyFallback?: string) => void;
  removePrompts: (ids: readonly string[]) => void;
  patchRuleSha256: (path: string, sha256: string) => void;
  setAgentTargets: (targets: AgentTargetOption[]) => void;
  setDomainLoading: (domain: DesktopDomain, loading: boolean) => void;
  setDomainError: (domain: DesktopDomain, message: string) => void;
  markDomainLoaded: (domain: DesktopDomain, loaded?: boolean) => void;
  setDomainRevision: (domain: DesktopDomain, revision: number) => void;
  acceptDomainRevision: (
    domain: DesktopDomain,
    baseRevision: number,
    revision: number,
  ) => DomainRevisionDecision;
  bumpDomainRetryRevision: () => void;
  setSkillUpdatesChecking: (checking: boolean) => void;
  setSkillUpdateError: (message: string) => void;
  setSkillIndexStatus: (status: SkillIndexStatus | null) => void;
  setSkillUpdateReports: (updates: SkillUpdateReport[]) => void;
  setSessionRefreshError: (message: string) => void;
  setProjects: (projects: ProjectSummary[]) => void;
  setSessionProjects: (projects: SessionProjectSummary[]) => void;
  patchSettings: (patch: Partial<DesktopSettingsValues>) => void;
  setSettingsLoading: (loading: boolean) => void;
  setSettingsError: (message: string) => void;
  setAnalyticsRevision: (revision: number) => void;
  setAnalyticsReady: (ready: boolean) => void;
  setAnalyticsError: (message: string) => void;
  setAnalyticsValue: (value: OverviewAnalytics | null, queryKey: AnalyticsQueryKey | null) => void;
};

export class DesktopStore {
  private state: DesktopStoreState = createInitialState();
  private listeners = new Set<() => void>();

  readonly actions: DesktopStoreActions = {
    commitDomainSnapshot: (domain, rows) => {
      this.update((current) => this.commitDomainSnapshotState(current, domain, rows));
    },
    commitSessionSnapshot: (rows) => {
      this.update((current) => {
        const next = this.commitDomainSnapshotState(current, RuntimeDomainKey.Sessions, rows);
        return next.sessions.refreshError === ""
          ? next
          : { ...next, sessions: { refreshError: "" } };
      });
    },
    applyHookCommandResult: (result) => {
      this.update((current) => {
        const data = reduceHookCommandResult(current.catalogs.data, result);
        return data ? this.commitCatalogData(current, RuntimeDomainKey.Hooks, data) : current;
      });
      return result;
    },
    applyMcpCommandResult: (result) => {
      this.update((current) => {
        const data = reduceMcpCommandResult(current.catalogs.data, result);
        return data ? this.commitCatalogData(current, RuntimeDomainKey.Mcp, data) : current;
      });
      return result;
    },
    applyRuleCommandResult: (result) => {
      this.update((current) => {
        const data = reduceRuleCommandResult(current.catalogs.data, result);
        return data ? this.commitCatalogData(current, RuntimeDomainKey.Rules, data) : current;
      });
      return result;
    },
    applySessionDelta: (upserts, deleted = []) => {
      this.update((current) => {
        const sessions = applySessionDelta(current.catalogs.data.sessions, upserts, deleted);
        if (sessions === current.catalogs.data.sessions) return current;
        return this.withCatalogData(current, { ...current.catalogs.data, sessions });
      });
    },
    replaceSkills: (rows) => {
      this.update((current) => {
        const data = applySkillSnapshot(current.catalogs.data, rows);
        return data === current.catalogs.data ? current : this.withCatalogData(current, data);
      });
    },
    patchSkills: (rows, deleted = []) => {
      this.update((current) => {
        const data = applySkillPatch(current.catalogs.data, rows, deleted);
        return data === current.catalogs.data ? current : this.withCatalogData(current, data);
      });
    },
    setSkillVisibility: (selectors, visibility) => {
      this.update((current) => {
        const data = applySkillVisibility(current.catalogs.data, selectors, visibility);
        return data === current.catalogs.data ? current : this.withCatalogData(current, data);
      });
    },
    clearSkillUpdates: (selectors) => {
      this.update((current) => {
        const data = clearSkillUpdateAvailability(current.catalogs.data, selectors);
        return data === current.catalogs.data ? current : this.withCatalogData(current, data);
      });
    },
    setPromptRecord: (record) => {
      this.update((current) => {
        const existing = current.catalogs.data.prompts.find((prompt) => prompt.id === record.id);
        const nextRecord = record.body ? record : { ...record, body: existing?.body ?? "" };
        const prompts = [nextRecord, ...current.catalogs.data.prompts.filter((prompt) => prompt.id !== record.id)];
        return this.withCatalogData(current, { ...current.catalogs.data, prompts });
      });
    },
    applyPromptRecord: (record, bodyFallback) => {
      this.update((current) => {
        const data = applyPromptRecord(current.catalogs.data, record, bodyFallback);
        return data === current.catalogs.data ? current : this.withCatalogData(current, data);
      });
      this.actions.markDomainLoaded(RuntimeDomainKey.Prompts);
    },
    removePrompts: (ids) => {
      const selected = new Set(ids);
      this.update((current) => {
        const prompts = current.catalogs.data.prompts.filter((prompt) => !selected.has(prompt.id));
        return prompts.length === current.catalogs.data.prompts.length
          ? current
          : this.withCatalogData(current, { ...current.catalogs.data, prompts });
      });
    },
    patchRuleSha256: (path, sha256) => {
      this.update((current) => {
        const rules = current.catalogs.data.rules.map((rule) => rule.path === path ? { ...rule, sha256 } : rule);
        return rules.every((rule, index) => rule === current.catalogs.data.rules[index])
          ? current
          : this.withCatalogData(current, { ...current.catalogs.data, rules });
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
        const errorDomains = new Set(current.catalogs.indexes.errorDomains);
        if (domain !== RuntimeDomainKey.Agents) {
          if (message) errorDomains.add(domain);
          else errorDomains.delete(domain);
        }
        return {
          ...current,
          catalogs: {
            ...current.catalogs,
            errors,
            indexes: this.withCatalogIndexes(current.catalogs.indexes, current.catalogs.data, undefined, errorDomains),
          },
        };
      });
    },
    markDomainLoaded: (domain, loaded = true) => {
      this.update((current) => {
        const loadedDomains = updateSet(current.catalogs.loadedDomains, domain, loaded);
        if (loadedDomains === current.catalogs.loadedDomains) return current;
        const indexLoadedDomains = new Set(current.catalogs.indexes.loadedDomains);
        if (domain !== RuntimeDomainKey.Agents) {
          if (loaded) indexLoadedDomains.add(domain);
          else indexLoadedDomains.delete(domain);
        }
        return {
          ...current,
          catalogs: {
            ...current.catalogs,
            loadedDomains,
            indexes: this.withCatalogIndexes(current.catalogs.indexes, current.catalogs.data, indexLoadedDomains),
          },
        };
      });
    },
    setDomainRevision: (domain, revision) => {
      if (!Number.isSafeInteger(revision) || revision < 0) return;
      this.update((current) => current.catalogs.revisions[domain] === revision
        ? current
        : {
          ...current,
          catalogs: {
            ...current.catalogs,
            revisions: { ...current.catalogs.revisions, [domain]: revision },
          },
        });
    },
    acceptDomainRevision: (domain, baseRevision, revision) => {
      let decision: DomainRevisionDecision = DomainRevisionDecision.Stale;
      this.update((current) => {
        const localRevision = current.catalogs.revisions[domain] ?? 0;
        const result = decideRevision(localRevision, {
          scopeKey: "desktop",
          domain,
          operationId: "event",
          baseRevision,
          revision,
          payload: null,
        });
        if (result.needsResync) {
          decision = DomainRevisionDecision.Resync;
          return current;
        }
        if (!result.accepted) {
          decision = DomainRevisionDecision.Stale;
          return current;
        }
        decision = DomainRevisionDecision.Accepted;
        return {
          ...current,
          catalogs: {
            ...current.catalogs,
            revisions: { ...current.catalogs.revisions, [domain]: revision },
          },
        };
      });
      return decision;
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
          catalogs: { ...current.catalogs, data, indexes: this.withCatalogIndexes(current.catalogs.indexes, data) },
          skillUpdates: { ...current.skillUpdates, fresh: true },
        };
      });
    },
    setSessionRefreshError: (refreshError) => {
      this.update((current) => current.sessions.refreshError === refreshError
        ? current
        : { ...current, sessions: { refreshError } });
    },
    setProjects: (projects) => {
      this.update((current) => current.workspace.projects === projects
        ? current
        : { ...current, workspace: { ...current.workspace, projects } });
    },
    setSessionProjects: (sessionProjects) => {
      this.update((current) => current.workspace.sessionProjects === sessionProjects
        ? current
        : { ...current, workspace: { ...current.workspace, sessionProjects } });
    },
    patchSettings: (patch) => {
      this.update((current) => ({
        ...current,
        settings: {
          ...current.settings,
          values: { ...current.settings.values, ...patch },
        },
      }));
    },
    setSettingsLoading: (loading) => {
      this.update((current) => current.settings.loading === loading
        ? current
        : { ...current, settings: { ...current.settings, loading } });
    },
    setSettingsError: (error) => {
      this.update((current) => current.settings.error === error
        ? current
        : { ...current, settings: { ...current.settings, error } });
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

  private withCatalogIndexes(
    previous: CatalogIndexes,
    data: RuntimeData,
    loadedDomains?: ReadonlySet<DomainKey>,
    errorDomains?: ReadonlySet<DomainKey>,
  ): CatalogIndexes {
    const next = buildCatalogIndexes(data, previous);
    const loaded = loadedDomains ?? previous.loadedDomains;
    const errors = errorDomains ?? previous.errorDomains;
    if (next.loadedDomains === loaded && next.errorDomains === errors) return next;
    return { ...next, loadedDomains: loaded, errorDomains: errors };
  }

  private withCatalogData(current: DesktopStoreState, data: RuntimeData): DesktopStoreState {
    const loadedDomains = new Set(current.catalogs.indexes.loadedDomains);
    for (const domain of DOMAIN_KEYS) {
      if (data[domain].length > 0) loadedDomains.add(domain);
    }
    return {
      ...current,
      catalogs: {
        ...current.catalogs,
        data,
        indexes: this.withCatalogIndexes(current.catalogs.indexes, data, loadedDomains),
      },
    };
  }

  private commitCatalogData(current: DesktopStoreState, domain: DesktopDomain, data: RuntimeData): DesktopStoreState {
    const base = data === current.catalogs.data ? current : this.withCatalogData(current, data);
    const loadedDomains = updateSet(base.catalogs.loadedDomains, domain, true);
    const errors = updateDomainErrors(base.catalogs.errors, domain, "");
    const errorDomains = new Set(base.catalogs.indexes.errorDomains);
    const loadedIndexDomains = new Set(base.catalogs.indexes.loadedDomains);
    if (domain !== RuntimeDomainKey.Agents) {
      loadedIndexDomains.add(domain);
      errorDomains.delete(domain);
    }
    const indexes = this.withCatalogIndexes(base.catalogs.indexes, data, loadedIndexDomains, errorDomains);
    if (base.catalogs.loadedDomains === loadedDomains && base.catalogs.errors === errors && base.catalogs.indexes === indexes) return base;
    return {
      ...base,
      catalogs: {
        ...base.catalogs,
        loadedDomains,
        errors,
        indexes,
      },
    };
  }

  private commitDomainSnapshotState(current: DesktopStoreState, domain: DesktopDomain, rows: readonly RawDomainRow[]): DesktopStoreState {
    const data = applyDomainSnapshot(current.catalogs.data, domain, rows);
    return this.commitCatalogData(current, domain, data);
  }

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
  const selectorRef = useRef(selector);
  selectorRef.current = selector;
  const selectedValueRef = useRef<{ value: T } | null>(null);
  const getSelectedSnapshot = useCallback(() => {
    const value = selectorRef.current(desktopStore.getSnapshot());
    const previous = selectedValueRef.current;
    if (previous && Object.is(previous.value, value)) return previous.value;
    selectedValueRef.current = { value };
    return value;
  }, []);
  return useSyncExternalStore(
    desktopStore.subscribe,
    getSelectedSnapshot,
    getSelectedSnapshot,
  );
}

export function selectCatalogData(state: DesktopStoreState): RuntimeData {
  return state.catalogs.data;
}
