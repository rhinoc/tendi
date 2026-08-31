import type { DesktopStore } from "../store/desktop-store.ts";
import { coalesceSessionEventBuffer } from "../controllers/session-controller.ts";
import {
  applySkillChange,
  invokeSkillsList,
  refreshSkills,
  SkillUpdateCheckState,
} from "./runtime-gateway.ts";
import type { CatalogMutationResponse, SkillChangeArgs, SkillChangeCommand, SkillChangeResponse, SkillRefreshResponse } from "./runtime-gateway.ts";
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

export function commitHookCommandResult(store: DesktopStore, result: CatalogMutationResponse): CatalogMutationResponse {
  store.actions.applyHookCommandResult(result);
  return result;
}

export function commitMcpCommandResult(store: DesktopStore, result: CatalogMutationResponse): CatalogMutationResponse {
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

export type SkillCatalogRuntime = {
  refreshList: (force?: boolean) => Promise<RawSkillRecord[] | null>;
  refreshListAndUpdates: () => Promise<SkillRefreshResponse | null>;
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

  const refreshList = (force = false): Promise<RawSkillRecord[] | null> => {
    if (!force && listInFlight) return listInFlight;
    if (force && forcedListInFlight) return forcedListInFlight;
    if (force && listInFlight) {
      const queued = listInFlight.then(() => refreshList(), () => refreshList());
      forcedListInFlight = queued;
      void queued.finally(() => {
        if (forcedListInFlight === queued) forcedListInFlight = null;
      });
      return queued;
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

  const refreshListAndUpdates = (): Promise<SkillRefreshResponse | null> => {
    if (refreshInFlight) return refreshInFlight;
    const revision = ++listRevision;
    deps.setChecking(true);
    const request = (async () => {
      try {
        const result = await refreshSkills();
        if (revision !== listRevision) return null;
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
    const refresh = refreshInFlight;
    const list = listInFlight;
    const forcedList = forcedListInFlight;
    await refresh;
    await list;
    await forcedList;
  };

  return { refreshList, refreshListAndUpdates, whenIdle };
}
