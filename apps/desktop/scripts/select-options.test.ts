import assert from "node:assert/strict";
import test from "node:test";

import { resolveSelectValue } from "../src/lib/select-options.ts";

const options = [
  { value: "shared", label: "Shared" },
  { value: "claude", label: "Claude" },
];

test("preserves an explicit empty select value", () => {
  assert.equal(resolveSelectValue("", options), "");
});

test("preserves a valid select value", () => {
  assert.equal(resolveSelectValue("claude", options), "claude");
});

test("falls back to the first option for a stale value", () => {
  assert.equal(resolveSelectValue("cursor", options), "shared");
});

test("keeps a value when no options are available", () => {
  assert.equal(resolveSelectValue("shared", []), "shared");
});
