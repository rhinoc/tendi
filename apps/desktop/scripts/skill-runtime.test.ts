import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("skill runtime tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  let listCalls = 0;
  let resolveInitial: ((rows: unknown[]) => void) | undefined;
  const invokeSkillsList = () => {
    listCalls += 1;
    if (listCalls === 1) return new Promise<unknown[]>((resolve) => { resolveInitial = resolve; });
    return Promise.resolve([{ id: "fresh" }]);
  };
  mock.module("../src/lib/runtime-gateway.ts", {
    namedExports: {
      applySkillChange: async () => ({}),
      invokeSkillsList,
      refreshSkills: async () => ({ skills: [], updates: null, updateCheck: "idle" }),
      SkillUpdateCheckState: { Started: "started", AlreadyRunning: "already-running" },
    },
  });
  mock.module("../src/controllers/session-controller.ts", {
    namedExports: { coalesceSessionEventBuffer: () => ({ upserts: [], deleted: [] }) },
  });

  const { createSkillCatalogRuntime } = await import("../src/lib/runtime-workflows.ts");

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
}
