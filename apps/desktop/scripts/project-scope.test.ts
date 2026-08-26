import assert from "node:assert/strict";
import test from "node:test";

import { scopeNameForPath, type ProjectSummary } from "../src/lib/projects.ts";

const projects: ProjectSummary[] = [
  { id: "tendi", name: "tendi", rootPath: "/Users/ryan/dev/tendi" },
];

test("scope paths under a registered project use the project name", () => {
  assert.equal(scopeNameForPath("/Users/ryan/dev/tendi/apps/desktop/mcp.json", projects), "tendi");
});

test("scope paths outside registered projects use the explicit Global branch", () => {
  assert.equal(scopeNameForPath("/Users/ryan/.claude/settings.json", projects), "Global");
});

test("scope paths without a source fail loudly", () => {
  assert.throws(
    () => scopeNameForPath("", projects),
    /Scope invariant violated: scope source path is missing/,
  );
});

test("matched projects without names fail loudly", () => {
  assert.throws(
    () => scopeNameForPath("/Users/ryan/dev/tendi/apps/desktop/mcp.json", [{ id: "missing-name", rootPath: "/Users/ryan/dev/tendi" }]),
    /Scope invariant violated: matched project is missing a name/,
  );
});
