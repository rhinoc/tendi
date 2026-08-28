import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("desktop store behavior tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  mock.module("../src/lib/skill-updates.ts", {
    namedExports: {
      applySkillUpdateReportsToData: (data: unknown) => data,
      countSkillUpdates: () => 0,
    },
  });

  const {
    DesktopStore,
    selectAnalyticsValue,
    selectCatalogCounts,
    selectSessionListStatus,
  } = await import("../src/store/desktop-store.ts");

  function session(id: string) {
    return { id, agent: "codex", title: id, path: `/tmp/${id}` };
  }

  function withSessions(store: InstanceType<typeof DesktopStore>, sessions: ReturnType<typeof session>[]) {
    store.actions.updateData((current) => ({ ...current, sessions }));
  }

  test("derives the catalog count from rows and exposes no count writer", () => {
    const store = new DesktopStore();

    store.actions.updateData((current) => ({
      ...current,
      skills: [{ name: "one" }, { name: "two" }],
      sessions: [session("one")],
    }));

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

  test("keeps cached rows usable when a refresh fails", () => {
    const store = new DesktopStore();
    const cached = [session("cached")];

    withSessions(store, cached);
    store.actions.markDomainLoaded("sessions");
    store.actions.setDomainLoading("sessions", true);
    store.actions.setSessionRefreshError("refresh failed");
    store.actions.setDomainLoading("sessions", false);

    assert.equal(selectSessionListStatus(store.getSnapshot()), "loaded");
    assert.deepEqual(store.getSnapshot().catalogs.data.sessions, cached);
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
    assert.equal(selectAnalyticsValue(store.getSnapshot(), { ...key, range: 90 }), null);
    assert.equal(selectAnalyticsValue(store.getSnapshot(), { ...key, revision: 5 }), null);
    assert.throws(() => store.actions.setAnalyticsValue(value, null), /query key/);
  });
}
