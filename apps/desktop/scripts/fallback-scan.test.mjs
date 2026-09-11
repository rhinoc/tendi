import assert from "node:assert/strict";
import { test } from "node:test";

import {
  normalizeSkillFileEntries,
  preferredSkillFileName,
} from "../src/lib/file-tree.ts";
import { mcpNeedsLogin, mcpStatusLabel, normalizeMcp } from "../src/lib/mcp.ts";

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

test("MCP rows preserve server metadata and data URI icons", () => {
  const row = normalizeMcp({
    agent: "cursor",
    name: "figma",
    scope: "project",
    transport: "cursor-plugin",
    enabled: true,
    status: "configured",
    path: "/tmp/SERVER_METADATA.json",
    trust_hash: "hash",
    server_title: "Figma",
    server_version: "2.2.107",
    icons: [
      { src: "data:image/svg+xml,%3Csvg%3E" },
      { src: "http://example.com/figma.svg" },
    ],
    tools: [{
      name: "get_design_context",
      title: "Get Design Context",
      input_schema: {
        type: "object",
        properties: { fileKey: { type: "string", description: "Figma file key" } },
        required: ["fileKey"],
      },
      icons: [],
    }],
  });

  assert.equal(row?.server_title, "Figma");
  assert.equal(row?.icons[0]?.src, "data:image/svg+xml,%3Csvg%3E");
  assert.equal(row?.icons[1]?.src, "http://example.com/figma.svg");
  assert.equal(row?.tools[0]?.title, "Get Design Context");
  assert.equal(row?.tools[0]?.input_schema?.properties?.fileKey?.type, "string");
  assert.equal(row?.tools[0]?.input_schema?.properties?.fileKey?.description, "Figma file key");
});

test("MCP authentication failures use the login marker", () => {
  assert.equal(mcpNeedsLogin("need-login"), true);
  assert.equal(mcpNeedsLogin("needs-auth"), true);
  assert.equal(mcpStatusLabel("need-login"), "Need login");
  assert.equal(mcpStatusLabel("configured"), "Configured");
});
