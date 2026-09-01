import assert from "node:assert/strict";
import test, { mock } from "node:test";

if (typeof mock.module !== "function") {
  test("session sorting requires Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  mock.module("../src/lib/agent/index.ts", {
    namedExports: { agentDefinition: () => undefined },
  });
  mock.module("../src/lib/agent/catalog.ts", {
    namedExports: { agentIcons: {} },
  });

  const { compareSessions, normalizeSession, sessionIdentity, sessionLogicalIdentity } = await import("../src/lib/sessions.ts");
  const { resolveInitialSession, sessionExternalKey, sessionSourceExternalKey } = await import("../src/lib/session-selection.ts");

  test("keeps the canonical agent key in normalized session records", () => {
    const normalized = normalizeSession({
      id: "session-1",
      agent: "Codex",
      path: "/tmp/session-1.jsonl",
    });

    assert.equal(normalized?.agent, "codex");
  });

  function session(id: string, startedAt: string, updatedAt: string) {
    return { id, agent: "codex", title: id, path: `/tmp/${id}`, startedAt, updatedAt };
  }

  test("sorts session timestamps by instant across timezone offsets", () => {
    const sessions = [
      session("cursor-1149", "2026-08-28T03:49:00Z", "2026-08-28T11:49:00+08:00"),
      session("codex-1157", "2026-08-28T03:57:00Z", "2026-08-28T03:57:03.099Z"),
      session("cursor-1143", "2026-08-28T03:43:00Z", "2026-08-28T11:43:00+08:00"),
    ];

    assert.deepEqual(
      [...sessions]
        .sort((left, right) => compareSessions(left, right, { key: "updatedAt", direction: "desc" }))
        .map((item) => item.id),
      ["codex-1157", "cursor-1149", "cursor-1143"],
    );
    assert.deepEqual(
      [...sessions]
        .sort((left, right) => compareSessions(left, right, { key: "startedAt", direction: "asc" }))
        .map((item) => item.id),
      ["cursor-1143", "cursor-1149", "codex-1157"],
    );
  });

  test("keeps logical identity stable when a session moves to a different source path", () => {
    const metadata = session("session-1", "2026-08-28T03:49:00Z", "2026-08-28T03:50:00Z");
    const transcript = { ...metadata, path: "/sessions/session-1.jsonl" };

    assert.equal(sessionLogicalIdentity(metadata), sessionLogicalIdentity(transcript));
    assert.notEqual(sessionIdentity(metadata), sessionIdentity(transcript));
  });

  test("matches external session selections without lowercasing native ids or source paths", () => {
    const session = {
      id: "ABC-123",
      agent: "codex",
      title: "Case-sensitive session",
      path: "/Users/Ryan/.codex/sessions/ABC-123.jsonl",
    };

    assert.equal(resolveInitialSession([session], sessionExternalKey(session)), session);
    assert.equal(resolveInitialSession([session], sessionSourceExternalKey(session)), session);
  });
}
