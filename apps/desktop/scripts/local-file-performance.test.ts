import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import test from "node:test";

import { findTextRanges } from "../src/components/shared/text-ranges.ts";

function legacyFindTextRanges(text: string, query: string) {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const haystack = `${text ?? ""}`.toLowerCase();
  const ranges: Array<{ from: number; to: number }> = [];
  let index = haystack.indexOf(needle);
  while (index >= 0) {
    ranges.push({ from: index, to: index + needle.length });
    index = haystack.indexOf(needle, index + needle.length);
  }
  return ranges;
}

test("large-file search preserves matches across bounded chunks", () => {
  const boundary = 64 * 1024;
  const prefix = "x".repeat(boundary - 2);
  const text = `${prefix}NEEDLE${"x".repeat(boundary + 3)}needle`;

  assert.deepEqual(findTextRanges(text, "needle"), [
    { from: boundary - 2, to: boundary + 4 },
    { from: boundary * 2 + 7, to: boundary * 2 + 13 },
  ]);
});

test("large-file search benchmark matches the legacy result", () => {
  const text = `${"x".repeat(64 * 1024)}needle`.repeat(256);
  const measure = (search: (value: string, query: string) => Array<{ from: number; to: number }>) => {
    globalThis.gc?.();
    const beforeRss = process.memoryUsage().rss;
    const started = performance.now();
    const ranges = search(text, "NEEDLE");
    const elapsedMs = performance.now() - started;
    const afterRss = process.memoryUsage().rss;
    return { ranges, elapsedMs, rssDeltaMiB: (afterRss - beforeRss) / (1024 * 1024) };
  };

  const before = measure(legacyFindTextRanges);
  const after = measure(findTextRanges);
  assert.deepEqual(after.ranges, before.ranges);
  process.stderr.write(
    `[large-file-search] bytes=${text.length} matches=${after.ranges.length} `
      + `before_ms=${before.elapsedMs.toFixed(2)} after_ms=${after.elapsedMs.toFixed(2)} `
      + `before_rss_delta_mib=${before.rssDeltaMiB.toFixed(2)} `
      + `after_rss_delta_mib=${after.rssDeltaMiB.toFixed(2)}\n`,
  );
});
