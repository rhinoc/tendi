import assert from "node:assert/strict";
import test from "node:test";

import { skillRelationList } from "../src/features/skills/SkillDependencyGraph.tsx";
import { buildRelationshipGraph } from "../src/features/skills/skill-relationship-layout.ts";

const skills = [
  { id: "parent-id", name: "parent", description: "", dependencies: [], dependents: [] },
  { id: "child-id", name: "child", description: "", dependencies: ["parent"], dependents: [] },
];

test("resolves skill relations by id and skill name without duplicates", () => {
  assert.deepEqual(
    skillRelationList(["parent-id", "parent"], skills).map((skill) => skill.name),
    ["parent"],
  );
});

test("builds relationship edges from skill names when ids are unavailable", () => {
  const graph = buildRelationshipGraph([
    { ...skills[0], id: "parent-node" },
    { ...skills[1], id: "child-node", dependencyIds: [] },
  ]);
  assert.deepEqual(
    graph.edges.map(({ from, to }) => [from, to]),
    [["parent-node", "child-node"]],
  );
});
