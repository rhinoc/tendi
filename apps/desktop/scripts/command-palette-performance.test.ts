import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import test from "node:test";

import { filterCommandPaletteItems, MAX_COMMAND_PALETTE_RESULTS } from "../src/lib/command-palette.ts";
import { fixedVirtualRange } from "../src/lib/virtualization.ts";

type TestItem = {
  id: number;
  label: string;
  detail: string;
  group: string;
  keywords: string[];
};

function makeItems(count: number): TestItem[] {
  return Array.from({ length: count }, (_, index) => ({
    id: index,
    label: `Prompt ${index.toString().padStart(6, "0")}`,
    detail: `Workspace project ${index % 37} with searchable content`,
    group: index % 4 === 0 ? "Prompts" : "Sessions",
    keywords: [`agent-${index % 9}`, `project-${index % 37}`],
  }));
}

test("caps command palette results at 512 and keeps exact matches first", () => {
  const items = makeItems(20_000);
  const results = filterCommandPaletteItems(items, "Prompt");

  assert.equal(results.length, MAX_COMMAND_PALETTE_RESULTS);
  assert.equal(results[0]?.label, "Prompt 000000");
  assert.equal(results.at(-1)?.label, "Prompt 000511");
});

test("filters and ranks a large tab index within the interactive budget", () => {
  const items = makeItems(50_000);
  const queries = ["project-12", "Prompt 4", "agent-7", "00042"];

  // Warm up the JIT and measure the same path used by the palette on each keystroke.
  for (const query of queries) filterCommandPaletteItems(items, query);
  const started = performance.now();
  for (const query of queries) filterCommandPaletteItems(items, query);
  const elapsedMs = performance.now() - started;

  assert.ok(elapsedMs < 1_000, `50k-item search took ${elapsedMs.toFixed(1)}ms`);
});

test("keeps the virtual result window bounded for 512 results", () => {
  const maxMountedRows = 512;
  let largestWindow = 0;
  for (const scrollOffset of [0, 360, 4_000, 18_000, 36_000]) {
    const range = fixedVirtualRange(maxMountedRows, scrollOffset, 420, 72, 5);
    largestWindow = Math.max(largestWindow, range.end - range.start);
    assert.ok(range.start >= 0 && range.end <= maxMountedRows);
  }

  assert.ok(largestWindow <= 20, `virtual list mounted ${largestWindow} rows`);
});
