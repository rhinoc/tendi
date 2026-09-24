import type { DesktopStore } from "../store/desktop-store.ts";
import { coalesceSessionEventBuffer } from "../controllers/session-controller.ts";
import {
  applySkillChange,
  invokeSkillsList,
  refreshSkills,
  SkillUpdateCheckState,
} from "./runtime-gateway.ts";
import type { CatalogMutationResponse, McpMutationResponse, SkillChangeArgs, SkillChangeCommand, SkillChangeResponse, SkillRefreshResponse } from "./runtime-gateway.ts";
import type { RawSkillRecord } from "./skills.ts";
import type { SessionIdentityRecord } from "./sessions.ts";
import type { RawDomainRow } from "../controllers/controller-types.ts";
import { RuntimeDomainKey } from "./domain.ts";

export async function applySkillChangeAndCommit(
  store: DesktopStore,
  command: SkillChangeCommand,
  args: SkillChangeArgs,
): Promise<SkillChangeResponse> {
  const result = await applySkillChange(command, args);
  const nextSkills = result.updated ?? result.skills;
  if (nextSkills) store.actions.patchSkills(nextSkills);
  return result;
}

export function commitSkillRows(
  store: DesktopStore,
  rows: readonly RawSkillRecord[],
  options: { patch?: boolean; deleted?: readonly string[] } = {},
): void {
  if (options.patch) store.actions.patchSkills(rows, options.deleted);
  else store.actions.replaceSkills(rows);
}

export function commitSkillChangeResult(store: DesktopStore, result: SkillChangeResponse): void {
  const nextSkills = result.updated ?? result.skills;
  if (nextSkills) commitSkillRows(store, nextSkills, { patch: true });
}

/**
 * Commit a visibility mutation without treating its filesystem materialization
 * timestamp as a content update. The visibility action can rewrite a target
 * file, which changes mtime even though the skill itself was not edited.
 */
export function commitSkillVisibilityResult(store: DesktopStore, result: SkillChangeResponse): void {
  const nextSkills = result.updated ?? result.skills;
  if (!nextSkills) return;
  const currentMtimes = new Map(
    store.getSnapshot().catalogs.data.skills.map((skill) => [skill.id, skill.mtime] as const),
  );
  const stabilized = nextSkills.map((skill) => {
    const id = typeof skill.id === "string" ? skill.id : null;
    return id && currentMtimes.has(id)
      ? { ...skill, mtime: currentMtimes.get(id) ?? null }
      : skill;
  });
  commitSkillRows(store, stabilized, { patch: true });
}

export function commitHookCommandResult(store: DesktopStore, result: CatalogMutationResponse): CatalogMutationResponse {
  store.actions.applyHookCommandResult(result);
  return result;
}

export function commitMcpCommandResult(store: DesktopStore, result: McpMutationResponse): McpMutationResponse {
  store.actions.applyMcpCommandResult(result);
  return result;
}

export function commitRuleCommandResult<T>(store: DesktopStore, result: T): T {
  store.actions.applyRuleCommandResult(result);
  return result;
}

export function commitSessionEventBuffer(
  store: DesktopStore,
  recent: readonly RawDomainRow[],
  watch: readonly RawDomainRow[],
  deleted: readonly SessionIdentityRecord[],
): void {
  const buffer = coalesceSessionEventBuffer(recent, watch, deleted);
  store.actions.applySessionDelta(buffer.upserts, buffer.deleted);
}

export type SnapshotMutationRuntime<T> = {
  refresh: (force?: boolean) => Promise<T | null>;
  beginMutation: () => () => void;
  whenIdle: () => Promise<void>;
};

/**
 * Owns the authority for a mutable snapshot.
 *
 * A list response is only allowed to commit if it belongs to the current
 * revision. Starting a mutation invalidates every older response and queues
 * refreshes requested while the mutation is in progress until its result has
 * been committed.
 */
export function createSnapshotMutationRuntime<T>(deps: {
  load: () => Promise<T>;
  commit: (value: T) => void;
  onError?: (error: unknown) => void;
}): SnapshotMutationRuntime<T> {
  let revision = 0;
  let inFlight: Promise<T | null> | null = null;
  let forcedInFlight: Promise<T | null> | null = null;
  let mutationDepth = 0;
  let refreshRequestedAfterMutation = false;

  const flushQueuedRefresh = (): void => {
    if (mutationDepth > 0 || !refreshRequestedAfterMutation) return;
    const pending = forcedInFlight ?? inFlight;
    if (pending) {
      void pending.then(flushQueuedRefresh);
      return;
    }
    refreshRequestedAfterMutation = false;
    void startRefresh();
  };

  const startRefresh = (): Promise<T | null> => {
    if (mutationDepth > 0) {
      refreshRequestedAfterMutation = true;
      return Promise.resolve(null);
    }
    const requestRevision = ++revision;
    const request = (async () => {
      try {
        const value = await deps.load();
        if (requestRevision !== revision) return null;
        deps.commit(value);
        return value;
      } catch (error) {
        if (requestRevision === revision) deps.onError?.(error);
        return null;
      }
    })();
    inFlight = request;
    void request.then(() => {
      if (inFlight === request) inFlight = null;
    });
    return request;
  };

  const refresh = (force = false): Promise<T | null> => {
    if (mutationDepth > 0) {
      refreshRequestedAfterMutation = true;
      return Promise.resolve(null);
    }
    if (force && forcedInFlight) return forcedInFlight;
    if (!force && (inFlight || forcedInFlight)) return forcedInFlight ?? inFlight!;
    if (force && inFlight) {
      const queued = inFlight.then(() => startRefresh());
      forcedInFlight = queued;
      void queued.then(() => {
        if (forcedInFlight === queued) forcedInFlight = null;
      });
      return queued;
    }
    return startRefresh();
  };

  const beginMutation = (): (() => void) => {
    if (mutationDepth === 0 && (inFlight || forcedInFlight)) refreshRequestedAfterMutation = true;
    mutationDepth += 1;
    // The mutation result is the newest local authority until a later refresh
    // confirms the on-disk snapshot.
    revision += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      mutationDepth -= 1;
      if (mutationDepth === 0) flushQueuedRefresh();
    };
  };

  const whenIdle = async () => {
    while (inFlight || forcedInFlight) {
      const current = [inFlight, forcedInFlight];
      await Promise.all(current.filter((request): request is Promise<T | null> => request !== null));
    }
  };

  return { refresh, beginMutation, whenIdle };
}

export type SkillCatalogRuntime = {
  refreshList: (force?: boolean) => Promise<RawSkillRecord[] | null>;
  refreshListAndUpdates: () => Promise<SkillRefreshResponse | null>;
  beginMutation: () => () => void;
  whenIdle: () => Promise<void>;
};

export function createSkillCatalogRuntime(deps: {
  store: DesktopStore;
  setError: (message: string) => void;
  setChecking: (checking: boolean) => void;
  setUpdateCheckActive: (active: boolean) => void;
  onError?: (message: string, error: unknown) => void;
}): SkillCatalogRuntime {
  let listRevision = 0;
  let listInFlight: Promise<RawSkillRecord[] | null> | null = null;
  let forcedListInFlight: Promise<RawSkillRecord[] | null> | null = null;
  let refreshInFlight: Promise<SkillRefreshResponse | null> | null = null;
  let mutationDepth = 0;
  let refreshRequestedAfterMutation = false;

  const flushQueuedRefresh = (): void => {
    if (mutationDepth > 0 || !refreshRequestedAfterMutation) return;
    const pending = forcedListInFlight ?? refreshInFlight ?? listInFlight;
    if (pending) {
      void pending.then(flushQueuedRefresh);
      return;
    }
    refreshRequestedAfterMutation = false;
    void refreshList(true);
  };

  const beginMutation = (): (() => void) => {
    if (mutationDepth === 0 && (listInFlight || forcedListInFlight || refreshInFlight)) {
      refreshRequestedAfterMutation = true;
    }
    mutationDepth += 1;
    // Any snapshot already in flight was requested before the mutation and
    // must not be allowed to overwrite its optimistic result.
    listRevision += 1;
    let released = false;
    return () => {
      if (released) return;
      released = true;
      mutationDepth -= 1;
      if (mutationDepth === 0) flushQueuedRefresh();
    };
  };

  const startListRefresh = (): Promise<RawSkillRecord[] | null> => {
    if (mutationDepth > 0) {
      refreshRequestedAfterMutation = true;
      return Promise.resolve(null);
    }
    const revision = ++listRevision;
    const request = (async () => {
      try {
        const skills = await invokeSkillsList();
        if (revision !== listRevision) return null;
        deps.store.actions.replaceSkills(skills);
        deps.store.actions.markDomainLoaded(RuntimeDomainKey.Skills);
        deps.setError("");
        return skills;
      } catch (error) {
        if (revision === listRevision) deps.setError(`${error}`);
        deps.onError?.("skills list refresh failed", error);
        return null;
      }
    })();
    listInFlight = request;
    void request.finally(() => {
      if (listInFlight === request) listInFlight = null;
    });
    return request;
  };

  const refreshList = (force = false): Promise<RawSkillRecord[] | null> => {
    if (mutationDepth > 0) {
      refreshRequestedAfterMutation = true;
      return Promise.resolve(null);
    }
    if (!force && (forcedListInFlight || listInFlight)) return forcedListInFlight ?? listInFlight!;
    if (force && forcedListInFlight) return forcedListInFlight;
    if (force && listInFlight) {
      const queued = listInFlight.then(
        () => {
          if (mutationDepth > 0) {
            refreshRequestedAfterMutation = true;
            return null;
          }
          return startListRefresh();
        },
        () => {
          if (mutationDepth > 0) {
            refreshRequestedAfterMutation = true;
            return null;
          }
          return startListRefresh();
        },
      );
      forcedListInFlight = queued;
      void queued.finally(() => {
        if (forcedListInFlight === queued) forcedListInFlight = null;
      });
      return queued;
    }
    return startListRefresh();
  };

  const refreshListAndUpdates = (): Promise<SkillRefreshResponse | null> => {
    if (mutationDepth > 0) {
      refreshRequestedAfterMutation = true;
      return Promise.resolve(null);
    }
    if (refreshInFlight) return refreshInFlight;
    const revision = ++listRevision;
    deps.setChecking(true);
    let request!: Promise<SkillRefreshResponse | null>;
    request = (async () => {
      try {
        const result = await refreshSkills();
        if (revision !== listRevision) {
          if (refreshInFlight === request) deps.setChecking(false);
          return null;
        }
        deps.store.actions.replaceSkills(result.skills);
        if (result.updates) deps.store.actions.setSkillUpdateReports(result.updates);
        deps.store.actions.markDomainLoaded(RuntimeDomainKey.Skills);
        deps.setError("");
        const running = result.updateCheck === SkillUpdateCheckState.Started || result.updateCheck === SkillUpdateCheckState.AlreadyRunning;
        deps.setUpdateCheckActive(running);
        deps.setChecking(running);
        return result;
      } catch (error) {
        if (revision === listRevision) {
          deps.setError(`${error}`);
          deps.setChecking(false);
        }
        deps.setUpdateCheckActive(false);
        deps.onError?.("skills refresh failed", error);
        return null;
      }
    })();
    refreshInFlight = request;
    void request.finally(() => {
      if (refreshInFlight === request) refreshInFlight = null;
    });
    return request;
  };

  const whenIdle = async () => {
    while (refreshInFlight || listInFlight || forcedListInFlight) {
      const refresh = refreshInFlight;
      const list = listInFlight;
      const forcedList = forcedListInFlight;
      await refresh;
      await list;
      await forcedList;
    }
  };

  return {
    beginMutation,
    refreshList,
    refreshListAndUpdates,
    whenIdle,
  };
}
