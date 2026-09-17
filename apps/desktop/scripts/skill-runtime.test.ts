import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("skill runtime tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  let listCalls = 0;
  let resolveInitial: ((rows: unknown[]) => void) | undefined;
  let resolveMutationRefresh: ((rows: unknown[]) => void) | undefined;
  let resolveInitialRefresh: ((response: unknown) => void) | undefined;
  let refreshCalls = 0;
  const invokeSkillsList = () => {
    listCalls += 1;
    if (listCalls === 1) return new Promise<unknown[]>((resolve) => { resolveInitial = resolve; });
    if (listCalls === 3) return new Promise<unknown[]>((resolve) => { resolveMutationRefresh = resolve; });
    return Promise.resolve([{ id: "fresh" }]);
  };
  const refreshSkills = () => {
    refreshCalls += 1;
    if (refreshCalls === 1) {
      return new Promise<unknown>((resolve) => { resolveInitialRefresh = resolve; });
    }
    return Promise.resolve({ skills: [{ id: "fresh" }], updates: null, updateCheck: "idle" });
  };
  mock.module("../src/lib/runtime-gateway.ts", {
    namedExports: {
      applySkillChange: async () => ({}),
      invokeSkillsList,
      refreshSkills,
      SkillUpdateCheckState: { Started: "started", AlreadyRunning: "already-running" },
    },
  });
  mock.module("../src/controllers/session-controller.ts", {
    namedExports: { coalesceSessionEventBuffer: () => ({ upserts: [], deleted: [] }) },
  });

  const { commitSkillVisibilityResult, createSkillCatalogRuntime, createSnapshotMutationRuntime } = await import("../src/lib/runtime-workflows.ts");

  test("visibility mutation keeps the current mtime for list ordering", () => {
    const patches: unknown[][] = [];
    const store = {
      getSnapshot: () => ({ catalogs: { data: { skills: [{ id: "skill", mtime: "before" }] } } }),
      actions: { patchSkills: (rows: unknown[]) => patches.push(rows) },
    };

    commitSkillVisibilityResult(store as never, { updated: [{ id: "skill", mtime: "after" }] } as never);

    assert.deepEqual(patches, [[{ id: "skill", mtime: "before" }]]);
  });

  test("forced skill refresh starts after an in-flight list without self-resolving", async () => {
    const replacements: unknown[][] = [];
    const runtime = createSkillCatalogRuntime({
      store: {
        actions: {
          replaceSkills: (rows: unknown[]) => replacements.push(rows),
          markDomainLoaded: () => undefined,
          setSkillUpdateReports: () => undefined,
        },
      } as never,
      setError: () => undefined,
      setChecking: () => undefined,
      setUpdateCheckActive: () => undefined,
    });

    void runtime.refreshList();
    const forced = runtime.refreshList(true);
    resolveInitial?.([{ id: "stale" }]);
    await forced;

    assert.equal(listCalls, 2);
    assert.deepEqual(replacements, [[{ id: "stale" }], [{ id: "fresh" }]]);
  });

  test("skill mutation invalidates and suppresses stale refreshes", async () => {
    const replacements: unknown[][] = [];
    const runtime = createSkillCatalogRuntime({
      store: {
        actions: {
          replaceSkills: (rows: unknown[]) => replacements.push(rows),
          markDomainLoaded: () => undefined,
          setSkillUpdateReports: () => undefined,
        },
      } as never,
      setError: () => undefined,
      setChecking: () => undefined,
      setUpdateCheckActive: () => undefined,
    });

    const inFlight = runtime.refreshList();
    const releaseMutation = runtime.beginMutation();
    assert.equal(await runtime.refreshList(true), null);
    resolveMutationRefresh?.([{ id: "stale-after-mutation" }]);
    await inFlight;
    assert.deepEqual(replacements, []);

    releaseMutation();
    await runtime.whenIdle();
    assert.equal(listCalls, 4);
    assert.deepEqual(replacements, [[{ id: "fresh" }]]);
  });

  test("skill mutation invalidates an in-flight refresh-and-updates snapshot", async () => {
    const replacements: unknown[][] = [];
    const checking: boolean[] = [];
    const runtime = createSkillCatalogRuntime({
      store: {
        actions: {
          replaceSkills: (rows: unknown[]) => replacements.push(rows),
          markDomainLoaded: () => undefined,
          setSkillUpdateReports: () => undefined,
        },
      } as never,
      setError: () => undefined,
      setChecking: (value) => checking.push(value),
      setUpdateCheckActive: () => undefined,
    });

    const inFlight = runtime.refreshListAndUpdates();
    const releaseMutation = runtime.beginMutation();
    resolveInitialRefresh?.({ skills: [{ id: "stale-refresh" }], updates: null, updateCheck: "idle" });
    await inFlight;
    assert.deepEqual(replacements, []);
    assert.deepEqual(checking, [true, false]);

    releaseMutation();
    await runtime.whenIdle();
    assert.equal(refreshCalls, 1);
    assert.equal(listCalls, 5);
    assert.deepEqual(replacements, [[{ id: "fresh" }]]);
  });

  test("generic catalog mutation runtime commits only the post-mutation snapshot", async () => {
    let calls = 0;
    let resolveInitial: ((rows: string[]) => void) | undefined;
    const commits: string[][] = [];
    const runtime = createSnapshotMutationRuntime({
      load: () => {
        calls += 1;
        if (calls === 1) return new Promise<string[]>((resolve) => { resolveInitial = resolve; });
        return Promise.resolve(["fresh"]);
      },
      commit: (rows) => commits.push(rows),
    });

    const initial = runtime.refresh();
    const releaseMutation = runtime.beginMutation();
    resolveInitial?.(["stale"]);
    await initial;
    assert.deepEqual(commits, []);

    releaseMutation();
    await runtime.whenIdle();
    assert.deepEqual(commits, [["fresh"]]);
  });

  test("generic runtime does not lose a forced refresh queued before a mutation", async () => {
    let calls = 0;
    let resolveInitial: ((rows: string[]) => void) | undefined;
    const commits: string[][] = [];
    const runtime = createSnapshotMutationRuntime({
      load: () => {
        calls += 1;
        if (calls === 1) return new Promise<string[]>((resolve) => { resolveInitial = resolve; });
        return Promise.resolve(["fresh"]);
      },
      commit: (rows) => commits.push(rows),
    });

    const initial = runtime.refresh();
    runtime.refresh(true);
    const releaseMutation = runtime.beginMutation();
    resolveInitial?.(["stale"]);
    await initial;
    releaseMutation();
    await runtime.whenIdle();

    assert.equal(calls, 2);
    assert.deepEqual(commits, [["fresh"]]);
  });
}
