import assert from "node:assert/strict";
import test from "node:test";

import { mcpDisplayName } from "../src/lib/mcp.ts";

test("uses the configured MCP identity for the primary display name", () => {
  assert.equal(mcpDisplayName({ name: "node_repl", server_name: "rmcp" }), "node_repl");
});

test("keeps the primary display name stable when probe metadata differs", () => {
  assert.equal(mcpDisplayName({ name: "runtime", server_name: "rmcp", server_title: "Computer Use" }), "runtime");
});
