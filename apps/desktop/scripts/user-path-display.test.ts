import assert from "node:assert/strict";
import test from "node:test";

import { formatUserPath } from "../src/lib/strings.ts";

test("user home paths are displayed with a tilde", () => {
  assert.equal(formatUserPath("/Users/ryan/.codex/config.toml"), "~/.codex/config.toml");
  assert.equal(formatUserPath("/home/ryan/.config/tendi"), "~/.config/tendi");
  assert.equal(formatUserPath("C:\\Users\\Ryan\\.codex\\config.toml"), "~\\.codex\\config.toml");
});

test("home roots and already compact paths stay stable", () => {
  assert.equal(formatUserPath("/Users/ryan"), "~");
  assert.equal(formatUserPath("/root"), "~");
  assert.equal(formatUserPath("~/.codex/config.toml"), "~/.codex/config.toml");
});

test("paths outside a user home are unchanged", () => {
  assert.equal(formatUserPath("/etc/claude-code/settings.json"), "/etc/claude-code/settings.json");
  assert.equal(formatUserPath("relative/SKILL.md"), "relative/SKILL.md");
  assert.equal(formatUserPath("https://github.com/openai/codex"), "https://github.com/openai/codex");
  assert.equal(formatUserPath(null), "");
});
