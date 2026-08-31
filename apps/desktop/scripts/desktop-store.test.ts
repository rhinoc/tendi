import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("desktop store behavior tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  mock.module("../src/lib/agent/index.ts", {
    namedExports: { agentDefinition: () => undefined },
  });
  mock.module("../src/lib/agent/catalog.ts", {
    namedExports: { agentIcons: {} },
  });
  mock.module("lucide-react", {
    namedExports: { Share2: () => null },
  });
  mock.module("../src/lib/app-icon.ts", {
    namedExports: { readCachedAppIcon: () => "sakura-pop" },
  });
  mock.module("../src/lib/skills.ts", {
    namedExports: {
      SkillVisibility: { Auto: "Auto", Manual: "Manual", Off: "Off", Mixed: "Mixed" },
      SkillOperationStatus: {
        Planned: "planned",
        Ready: "ready",
        AlreadyExists: "already-exists",
        AlreadyInstalled: "already-installed",
        Replace: "replace",
      },
      normalizeSkill: (row: unknown) => {
        const skill = row as Record<string, unknown>;
        return { ...skill, id: skill.id ?? skill.name, agents: [], paths: [] };
      },
      skillSection: () => "",
    },
  });
  const hookRefs: Array<{ current: unknown }> = [];
  let hookIndex = 0;
  let latestSnapshot: (() => unknown) | null = null;
  mock.module("react", {
    namedExports: {
      createElement: () => null,
      useCallback: (callback: unknown) => callback,
      useRef: (initial: unknown) => {
        const ref = hookRefs[hookIndex] ?? { current: initial };
        hookRefs[hookIndex] = ref;
        hookIndex += 1;
        return ref;
      },
      useSyncExternalStore: (_subscribe: unknown, getSnapshot: () => unknown) => {
        latestSnapshot = getSnapshot;
        return getSnapshot();
      },
    },
  });
  mock.module("../src/lib/skill-updates.ts", {
    namedExports: {
      applySkillUpdateReportsToData: (data: unknown) => data,
    },
  });

  const {
    DesktopStore,
    desktopStore,
    selectAnalyticsDisplayValue,
    selectAnalyticsValue,
    selectCatalogCounts,
    selectSessionListStatus,
    useDesktopStore,
  } = await import("../src/store/desktop-store.ts");
  const { toRawDomainRows } = await import("../src/lib/raw-domain.ts");

  function session(id: string) {
    return toRawDomainRows([{ id, agent: "codex", title: id, path: `/tmp/${id}` }], "test session rows")[0];
  }

  function withSessions(store: InstanceType<typeof DesktopStore>, sessions: ReturnType<typeof session>[]) {
    store.actions.commitDomainSnapshot("sessions", sessions);
  }

  test("derives the catalog count from rows and exposes no count writer", () => {
    const store = new DesktopStore();

    store.actions.commitDomainSnapshot("skills", toRawDomainRows([{ name: "one" }, { name: "two" }], "test skill rows"));
    store.actions.commitDomainSnapshot("sessions", [session("one")]);

    assert.deepEqual(selectCatalogCounts(store.getSnapshot().catalogs.data), {
      skills: 2,
      sessions: 1,
      prompts: 0,
      rules: 0,
      hooks: 0,
      mcp: 0,
    });
    assert.equal("setInventoryCount" in store.actions, false);
  });

  test("reports a loaded session after the first rows load", () => {
    const store = new DesktopStore();

    store.actions.setDomainLoading("sessions", true);
    withSessions(store, [session("first")]);
    assert.equal(selectSessionListStatus(store.getSnapshot()), "loading");

    store.actions.markDomainLoaded("sessions");
    store.actions.setDomainLoading("sessions", false);
    assert.equal(selectSessionListStatus(store.getSnapshot()), "loaded");
  });

  test("accepts only the next domain revision and requests resync for gaps", () => {
    const store = new DesktopStore();

    assert.equal(store.actions.acceptDomainRevision("sessions", 0, 1), "accepted");
    assert.equal(store.getSnapshot().catalogs.revisions.sessions, 1);
    assert.equal(store.actions.acceptDomainRevision("sessions", 0, 2), "resync");
    assert.equal(store.getSnapshot().catalogs.revisions.sessions, 1);
    assert.equal(store.actions.acceptDomainRevision("sessions", 1, 1), "stale");
    assert.equal(store.getSnapshot().catalogs.revisions.sessions, 1);
    assert.equal(store.actions.acceptDomainRevision("sessions", 1, 2), "accepted");
    assert.equal(store.getSnapshot().catalogs.revisions.sessions, 2);
  });

  test("keeps cached rows usable when a refresh fails", () => {
    const store = new DesktopStore();
    const cached = [session("cached")];

    withSessions(store, cached);
    store.actions.markDomainLoaded("sessions");
    store.actions.setDomainLoading("sessions", true);
    store.actions.setSessionRefreshError("refresh failed");
    store.actions.setDomainLoading("sessions", false);

    assert.equal(selectSessionListStatus(store.getSnapshot()), "loaded");
    assert.deepEqual(
      store.getSnapshot().catalogs.data.sessions.map((item) => ({ id: item.id, path: item.path })),
      cached.map((item) => ({ id: item.id, path: item.path })),
    );
    assert.equal(store.getSnapshot().sessions.refreshError, "refresh failed");
  });

  test("retry returns a cached session list to loading and then loaded", () => {
    const store = new DesktopStore();

    withSessions(store, [session("cached")]);
    store.actions.markDomainLoaded("sessions");
    store.actions.setSessionRefreshError("refresh failed");

    store.actions.markDomainLoaded("sessions", false);
    store.actions.setSessionRefreshError("");
    store.actions.setDomainLoading("sessions", true);
    assert.equal(selectSessionListStatus(store.getSnapshot()), "loading");

    withSessions(store, [session("fresh")]);
    store.actions.markDomainLoaded("sessions");
    store.actions.setDomainLoading("sessions", false);
    assert.equal(selectSessionListStatus(store.getSnapshot()), "loaded");
    assert.equal(store.getSnapshot().sessions.refreshError, "");
  });

  test("only returns an analytics value for its exact agent, range, and revision", () => {
    const store = new DesktopStore();
    const value = { revision: 4 };
    const key = { agent: "All", range: 30, revision: 4 };

    store.actions.setAnalyticsValue(value, key);

    assert.equal(selectAnalyticsValue(store.getSnapshot(), key), value);
    assert.equal(selectAnalyticsDisplayValue(store.getSnapshot(), { ...key, revision: 5 }), value);
    assert.equal(selectAnalyticsValue(store.getSnapshot(), { ...key, range: 90 }), null);
    assert.equal(selectAnalyticsDisplayValue(store.getSnapshot(), { ...key, agent: "Claude" }), null);
    assert.equal(selectAnalyticsDisplayValue(store.getSnapshot(), { ...key, range: 90 }), null);
    assert.equal(selectAnalyticsValue(store.getSnapshot(), { ...key, revision: 5 }), null);
    assert.throws(() => store.actions.setAnalyticsValue(value, null), /query key/);
  });

  test("keeps a selected snapshot stable when an unrelated store field changes", () => {
    hookRefs.length = 0;
    hookIndex = 0;
    latestSnapshot = null;
    useDesktopStore((state) => state.analytics);
    const before = latestSnapshot?.();

    desktopStore.actions.commitDomainSnapshot("skills", toRawDomainRows([{ name: "unrelated" }], "test skill rows"));

    const after = latestSnapshot?.();
    assert.equal(after, before);
  });
}
