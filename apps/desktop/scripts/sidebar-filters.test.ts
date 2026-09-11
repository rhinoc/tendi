import assert from "node:assert/strict";
import test from "node:test";

import {
  normalizeSidebarFilters,
  persistSidebarFilters,
  readCachedSidebarFilters,
  SIDEBAR_FILTERS_STORAGE_KEY,
} from "../src/lib/sidebar-filters.ts";
import { ProjectScopeFilter } from "../src/lib/projects.ts";

test("normalizes sidebar filters independently", () => {
  assert.deepEqual(normalizeSidebarFilters({ agent: " Claude ", scope: "project" }), {
    agent: "Claude",
    scope: ProjectScopeFilter.Project,
  });
  assert.deepEqual(normalizeSidebarFilters({ agent: "", scope: "invalid" }), {
    agent: "All",
    scope: ProjectScopeFilter.All,
  });
});

test("persists and restores sidebar filters", () => {
  const storage = new Map<string, string>();
  const globalWithWindow = globalThis as typeof globalThis & { window?: unknown };
  const previousWindow = globalWithWindow.window;
  Object.defineProperty(globalWithWindow, "window", {
    configurable: true,
    value: {
      localStorage: {
        getItem: (key: string) => storage.get(key) ?? null,
        setItem: (key: string, value: string) => storage.set(key, value),
      },
    },
  });
  try {
    persistSidebarFilters({ agent: " Codex ", scope: ProjectScopeFilter.Global });
    assert.equal(storage.has(SIDEBAR_FILTERS_STORAGE_KEY), true);
    assert.deepEqual(readCachedSidebarFilters(), {
      agent: "Codex",
      scope: ProjectScopeFilter.Global,
    });
  } finally {
    Object.defineProperty(globalWithWindow, "window", { configurable: true, value: previousWindow });
  }
});
