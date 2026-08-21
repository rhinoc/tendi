import assert from "node:assert/strict";
import test from "node:test";

import { sortSidebarSources } from "../src/lib/sidebar-sources.ts";

test("puts installed agents before catalog-only agents", () => {
  const sources = sortSidebarSources([
    { label: "AiderDesk", count: 0, installed: false, order: 0 },
    { label: "Codex", count: 0, installed: true, order: 18 },
    { label: "Amp", count: 0, installed: false, order: 1 },
    { label: "Cursor", count: 2, installed: true, order: 27 },
  ]);

  assert.deepEqual(sources.map((source) => source.label), ["Codex", "Cursor", "AiderDesk", "Amp"]);
});

test("keeps catalog order within each installation group", () => {
  const sources = sortSidebarSources([
    { label: "Cursor", count: 0, installed: true, order: 27 },
    { label: "Codex", count: 0, installed: true, order: 18 },
    { label: "Amp", count: 0, installed: false, order: 1 },
    { label: "AiderDesk", count: 0, installed: false, order: 0 },
  ]);

  assert.deepEqual(sources.map((source) => source.label), ["Codex", "Cursor", "AiderDesk", "Amp"]);
});
