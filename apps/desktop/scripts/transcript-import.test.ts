import assert from "node:assert/strict";
import test, { mock } from "node:test";
import { AgentTranscriptFormat } from "../src/lib/agent/types.ts";
import type { AgentDefinition } from "../src/lib/agent/types.ts";

if (typeof mock.module !== "function") {
  test("transcript import sorting requires Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  mock.module("../src/lib/agent/index.ts", {
    namedExports: { agentDefinitions: [] },
  });

  const { parseJsonlTranscriptForProvider } = await import("../src/lib/agent/transcript.ts");
  const calls: string[] = [];
  const definitions: AgentDefinition[] = [
    {
      id: "codex",
      aliases: ["codex"],
      displayName: "Codex",
      transcriptFormat: AgentTranscriptFormat.Codex,
      transcriptParser: (_value, _context) => {
        calls.push("codex");
        return true;
      },
    },
    {
      id: "claude",
      aliases: ["claude"],
      displayName: "Claude",
      transcriptFormat: AgentTranscriptFormat.Claude,
      transcriptParser: (_value, _context) => {
        calls.push("claude");
        return true;
      },
    },
  ];

  test("routes imported JSONL to one explicit provider", () => {
    const result = parseJsonlTranscriptForProvider([
      JSON.stringify({ type: "user", timestamp: "2026-08-28T11:49:00+08:00", message: "early" }),
      JSON.stringify({ type: "assistant", timestamp: "2026-08-28T03:57:03.099Z", message: "later" }),
    ].join("\n"), "codex", definitions);

    assert.deepEqual(calls, ["codex", "codex"]);
    assert.equal(result.startedAt, "2026-08-28T11:49:00+08:00");
    assert.equal(result.updatedAt, "2026-08-28T03:57:03.099Z");
  });
}
