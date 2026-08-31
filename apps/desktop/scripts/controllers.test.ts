import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("controller tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  mock.module("../src/lib/agent/index.ts", {
    namedExports: { agentDefinition: () => undefined },
  });
  mock.module("../src/lib/agent/catalog.ts", {
    namedExports: { agentIcons: {} },
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
      normalizeSkill: () => undefined,
      skillSection: () => "",
    },
  });

  const {
    buildSkillInstallViewModel,
    expandSkillDependencies,
  } = await import("../src/controllers/skill-controller.ts");
  const {
    reconcileCollection,
    applyHookCommandResult,
    applyMcpCommandResult,
  } = await import("../src/controllers/catalog-controller.ts");
  const { emptyRuntimeData } = await import("../src/lib/data.ts");
  const { toRawDomainRows } = await import("../src/lib/raw-domain.ts");
  const { isMcpMutationDelta } = await import("../src/lib/mcp.ts");
  const {
    applySessionDelta,
    coalesceSessionEventBuffer,
    selectSessionListView,
  } = await import("../src/controllers/session-controller.ts");

  test("reconciles unchanged catalog rows by reference", () => {
    const first = { id: "one", value: "same" };
    const second = { id: "two", value: "old" };
    const next = reconcileCollection(
      [first, second],
      [{ id: "one", value: "same" }, { id: "two", value: "new" }],
      (row) => row.id,
    );
    assert.equal(next[0], first);
    assert.notEqual(next[1], second);
  });

  test("expands skill dependencies once without looping on cycles", () => {
    assert.deepEqual(
      expandSkillDependencies(
        ["root"],
        new Map([["root", ["dep"]], ["dep", ["root"]]]),
        new Set(["root", "dep"]),
      ),
      ["dep", "root"],
    );
  });

  test("builds skill install filters and search results from one view model", () => {
    const available = [
      { name: "root", description: "Root", relative_path: "root", dependencies: ["dep"] },
      { name: "dep", description: "Dependency", relative_path: "dep", dependencies: [] },
    ];
    const view = buildSkillInstallViewModel(
      available,
      [{ name: "root", status: "planned" }, { name: "dep", status: "already-exists" }],
      ["root"],
      "all",
      "depend",
      true,
    );
    assert.deepEqual(view.selected, ["dep", "root"]);
    assert.equal(view.selectedHasExisting, true);
    assert.deepEqual(view.searchMatches, ["dep"]);
    assert.equal(view.operationByName.get("dep")?.status, "replace");
  });

  test("returns the final session rows after remote search and paging", () => {
    const base = [
      { id: "one", agent: "codex", title: "one", path: "/one", projectPath: "/repo", updatedAt: "2026-08-29T00:00:00Z" },
      { id: "two", agent: "codex", title: "two", path: "/two", projectPath: "/repo", updatedAt: "2026-08-28T00:00:00Z" },
    ];
    const view = selectSessionListView({
      sessions: base,
      searchRows: [base[1]],
      searchRowsKey: "needle\0codex\u0000one\u0000/one\0codex\u0000two\u0000/two",
      query: "needle",
      remoteSearch: true,
      sort: { key: "updatedAt", direction: "desc" },
      pageSize: 1,
      groupBy: null,
      showChildSessions: false,
      selectedProjectKeys: [],
      projectFilterQuery: "",
      missingSessionProjectPolicy: "show",
      projects: [],
      sessionProjects: [],
    });
    assert.deepEqual(view.tableSessions.map((session) => session.id), ["two"]);
    assert.equal(view.pageCount, 1);
  });

  test("keeps raw session rows until the controller write boundary", () => {
    const raw = toRawDomainRows([{
      id: " session-1 ",
      agent: "codex",
      path: " /sessions/session-1 ",
      title: "Raw title",
      started_at: "2026-08-29T00:00:00Z",
    }], "test session rows")[0];
    const buffered = coalesceSessionEventBuffer([raw], [], []);
    assert.equal(buffered.upserts[0], raw);

    const sessions = applySessionDelta([], buffered.upserts);
    assert.equal(sessions[0].id, "session-1");
    assert.equal(sessions[0].path, "/sessions/session-1");
    assert.equal(sessions[0].startedAt, "2026-08-29T00:00:00Z");
    assert.equal("started_at" in sessions[0], false);
  });

  test("does not re-normalize existing canonical rows during a session delta", () => {
    const current = emptyRuntimeData();
    current.sessions = [{
      id: "existing",
      title: "Existing",
      agent: "codex",
      path: "/sessions/existing",
      project: "/repo",
      projectPath: "/repo",
      startedAt: "2026-08-28T00:00:00Z",
      updatedAt: "2026-08-29T00:00:00Z",
      messages: 12,
      firstUserMessage: "hello",
    }];
    const next = applySessionDelta(current.sessions, toRawDomainRows([{
      id: "incoming",
      agent: "codex",
      path: "/sessions/incoming",
      title: "Incoming",
      updated_at: "2026-08-30T00:00:00Z",
      message_count: 1,
    }], "test incoming session rows"));
    assert.equal(next[0], current.sessions[0]);
    assert.equal(next[0].projectPath, "/repo");
    assert.equal(next[0].messages, 12);
    assert.equal(next[1].id, "incoming");
    assert.equal(next[1].updatedAt, "2026-08-30T00:00:00Z");
  });

  test("keeps canonical hook rows while applying a typed mutation result", () => {
    const raw = toRawDomainRows([{
      agent: " codex ",
      event: " session-start ",
      path: " /hooks.json ",
      trust_hash: " hash ",
      enabled: true,
      needs_review: false,
    }], "test hook rows")[0];
    const current = emptyRuntimeData();
    current.hooks = [{
      agent: "codex",
      event: "session-start",
      path: "/hooks.json",
      trust_hash: "hash",
      enabled: false,
      needs_review: false,
    }];
    const next = applyHookCommandResult(current, { updated: [raw] });
    assert.equal(next?.hooks.length, 1);
    assert.notEqual(next?.hooks[0], current.hooks[0]);
    assert.equal(next?.hooks[0]?.agent, "codex");
    assert.equal(next?.hooks[0]?.path, "/hooks.json");
  });

  test("keeps canonical MCP rows while applying a typed mutation result", () => {
    const raw = toRawDomainRows([{
      agent: " codex ",
      name: " server ",
      scope: " global ",
      transport: " stdio ",
      enabled: true,
      status: " ready ",
      path: " /mcp.json ",
      trust_hash: " hash ",
    }], "test MCP rows")[0];
    const current = emptyRuntimeData();
    current.mcp = [{
      agent: "codex",
      name: "server",
      scope: "global",
      transport: "stdio",
      enabled: true,
      status: "offline",
      path: "/mcp.json",
      trust_hash: "hash",
      server_path: [],
    }];
    const next = applyMcpCommandResult(current, { updated: [raw] });
    assert.equal(next?.mcp.length, 1);
    assert.notEqual(next?.mcp[0], current.mcp[0]);
    assert.equal(next?.mcp[0]?.agent, "codex");
    assert.equal(next?.mcp[0]?.path, "/mcp.json");
  });

  test("accepts the typed MCP mutation object and rejects the removed array shape", () => {
    assert.equal(isMcpMutationDelta({ updated: [] }), true);
    assert.equal(isMcpMutationDelta([]), false);
  });
}
