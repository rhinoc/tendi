import assert from "node:assert/strict";
import test from "node:test";

import { mcpCopy } from "../src/lib/mcp-copy.ts";

test("keeps MCP data-list copy consistent", () => {
  assert.deepEqual(mcpCopy, {
    selectVisibleLabel: "Select visible MCP servers from toolbar",
    selectionLabel: "servers",
    loadingLabel: "Loading MCP servers",
    emptyTitle: "No MCP servers found",
    emptyDescription: "Adjust the agent filter to see more.",
  });
});
