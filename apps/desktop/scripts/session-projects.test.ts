import assert from "node:assert/strict";
import test from "node:test";

import { sessionProjectOptionForPaths } from "../src/lib/projects.ts";
import type { ProjectSummary, SessionProjectSummary } from "../src/lib/projects.ts";

const missingProject: SessionProjectSummary = {
  id: "legacy-project",
  name: "tutti",
  missing: true,
  paths: ["/Users/ryan/Documents/tutti"],
};

test("hide policy removes missing project options without changing session identity", () => {
  assert.equal(
    sessionProjectOptionForPaths({
      key: "legacy-project",
      label: "tutti",
      title: "/Users/ryan/Documents/tutti",
      logicalProjectId: "legacy-project",
      workspacePath: "/Users/ryan/Documents/tutti",
    }, "hide", [missingProject], []),
    null,
  );
});

test("merge-by-name maps a missing project only to a unique scanned project", () => {
  const scannedProject: ProjectSummary = {
    id: "scanned-tutti",
    name: "tutti",
    rootPath: "/Users/ryan/dev/tutti",
  };
  const option = sessionProjectOptionForPaths(
    {
      key: "legacy-project",
      label: "tutti",
      title: "/Users/ryan/Documents/tutti",
      logicalProjectId: "legacy-project",
      workspacePath: "/Users/ryan/Documents/tutti",
    },
    "merge-by-name",
    [missingProject],
    [scannedProject],
  );

  assert.equal(option?.key, JSON.stringify(["scanned-project", "scanned-tutti"]));
  assert.equal(option?.label, "tutti");
});

test("merge-by-name keeps the historical project separate when names are ambiguous", () => {
  const projects: ProjectSummary[] = [
    { id: "scanned-one", name: "tutti", rootPath: "/Users/ryan/dev/one/tutti" },
    { id: "scanned-two", name: "tutti", rootPath: "/Users/ryan/dev/two/tutti" },
  ];

  assert.equal(
    sessionProjectOptionForPaths({
      key: "legacy-project",
      label: "tutti",
      title: "/Users/ryan/Documents/tutti",
      logicalProjectId: "legacy-project",
      workspacePath: "/Users/ryan/Documents/tutti",
    }, "merge-by-name", [missingProject], projects)?.key,
    "legacy-project",
  );
});
