import assert from "node:assert/strict";
import test, { mock } from "node:test";

import { ProjectScopeFilter, type ProjectSummary } from "../src/lib/projects.ts";
import type { CatalogView } from "../src/controllers/catalog-controller.ts";

if (typeof mock.module !== "function") {
  test("project scope filter tests require Node module mocks", { skip: "run with --experimental-test-module-mocks" }, () => {});
} else {
  mock.module("../src/lib/agent/index.ts", {
    namedExports: { agentDefinition: () => undefined },
  });
  mock.module("../src/lib/agent/catalog.ts", {
    namedExports: { agentIcons: {} },
  });
  mock.module("../src/lib/skills.ts", {
    namedExports: { normalizeSkill: () => undefined },
  });

  const { selectProjectScopeView } = await import("../src/controllers/catalog-controller.ts");
  type NormalizedSkill = CatalogView["skills"][number];

const projects: ProjectSummary[] = [{ id: "demo", name: "demo", rootPath: "/repo" }];

function skill(id: string, scope: string): NormalizedSkill {
  return {
    id,
    section: "Local",
    name: id,
    description: "",
    tags: [],
    dependencies: [],
    dependents: [],
    dependencyIds: [],
    dependentIds: [],
    isWrapper: false,
    agents: ["Codex"],
    visibility: "Auto",
    isSystem: false,
    statusTone: "ok",
    source: "local",
    installTargets: [],
    trackingStatus: "local",
    updateAvailability: "unknown",
    paths: [{
      path: `/${id}/SKILL.md`,
      root: `/${id}`,
      scope,
      agent: "codex",
      install_target: `codex:${scope}`,
      source_kind: "local",
    }],
  };
}

const view: CatalogView = {
  agents: [],
  skills: [skill("global-skill", "global"), skill("project-skill", "project")],
  prompts: [],
  sessions: [],
  rules: [
    { agents: ["codex"], kind: "always", scope: "global", order: 0, path: "/global.md", sha256: "global" },
    { agents: ["codex"], kind: "always", scope: "project", order: 1, path: "/repo/project.md", sha256: "project" },
  ],
  hooks: [
    { agent: "codex", event: "global", enabled: true, needs_review: false, path: "/global/hooks.json", trust_hash: "global" },
    { agent: "codex", event: "project", enabled: true, needs_review: false, path: "/repo/hooks.json", trust_hash: "project" },
  ],
  mcp: [
    { agent: "codex", name: "global", scope: "global", transport: "stdio", enabled: true, status: "ready", path: "/global/mcp.json", trust_hash: "global" },
    { agent: "codex", name: "project", scope: "demo", transport: "stdio", enabled: true, status: "ready", path: "/repo/mcp.json", trust_hash: "project" },
  ],
  sourceIndex: [],
};

test("all scope keeps the catalog view unchanged", () => {
  assert.equal(selectProjectScopeView(view, ProjectScopeFilter.All, projects), view);
});

test("global scope keeps only global catalog content", () => {
  const filtered = selectProjectScopeView(view, ProjectScopeFilter.Global, projects);
  assert.deepEqual(filtered.skills.map((item) => item.id), ["global-skill"]);
  assert.deepEqual(filtered.rules.map((item) => item.path), ["/global.md"]);
  assert.deepEqual(filtered.hooks.map((item) => item.path), ["/global/hooks.json"]);
  assert.deepEqual(filtered.mcp.map((item) => item.name), ["global"]);
});

test("project scope keeps only project catalog content", () => {
  const filtered = selectProjectScopeView(view, ProjectScopeFilter.Project, projects);
  assert.deepEqual(filtered.skills.map((item) => item.id), ["project-skill"]);
  assert.deepEqual(filtered.rules.map((item) => item.path), ["/repo/project.md"]);
  assert.deepEqual(filtered.hooks.map((item) => item.path), ["/repo/hooks.json"]);
  assert.deepEqual(filtered.mcp.map((item) => item.name), ["project"]);
});
}
