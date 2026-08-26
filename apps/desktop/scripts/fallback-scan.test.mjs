import assert from "node:assert/strict";
import { test } from "node:test";

import {
  normalizeSkillFileEntries,
  preferredSkillFileName,
} from "../src/lib/file-tree.ts";
import { normalizeMcp } from "../src/lib/mcp.ts";

test("skill file selection stays empty when the file list has no valid file", () => {
  assert.equal(preferredSkillFileName([]), undefined);
  assert.equal(preferredSkillFileName(normalizeSkillFileEntries([{
    relative_path: "SKILL.md",
    kind: "file",
  }])), undefined);
});

test("skill file data does not invent missing fields", () => {
  assert.deepEqual(normalizeSkillFileEntries([{
    relative_path: "notes.md",
    kind: "file",
    path: "/tmp/notes.md",
  }]), [{ name: "notes.md", kind: "file", path: "/tmp/notes.md" }]);
  assert.deepEqual(normalizeSkillFileEntries([{
    relative_path: "notes.md",
  }]), []);
});

test("MCP rows with no transport stay invalid", () => {
  assert.equal(normalizeMcp({
    agent: "codex",
    name: "broken",
    scope: "global",
    transport: "",
    enabled: true,
    status: "configured",
    path: "/tmp/config.toml",
    trust_hash: "hash",
  }), undefined);
});
