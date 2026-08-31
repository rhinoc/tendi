import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { fixedVirtualRange, virtualRangeFor } from "../src/lib/virtualization.ts";

const dataTable = await readFile(new URL("../src/components/DataTable.tsx", import.meta.url), "utf8");

test("clamps a stale scroll position after a paged data set shrinks", () => {
  const range = fixedVirtualRange(50, 5_560, 240, 58, 4);

  assert.deepEqual(range, { start: 41, end: 50 });
  assert.ok(range.start < range.end);
});

test("keeps an in-range vertical virtual window unchanged", () => {
  assert.deepEqual(fixedVirtualRange(100, 1_160, 240, 58, 4), { start: 16, end: 29 });
});

test("renders the full locator when its data becomes shorter than the viewport", () => {
  assert.deepEqual(fixedVirtualRange(50, 5_560, 640, 10, 16), { start: 0, end: 50 });
});

test("clamps a stale horizontal virtual window after the data set shrinks", () => {
  assert.deepEqual(fixedVirtualRange(100, 20_000, 576, 24, 8), { start: 68, end: 100 });
});

test("keeps every consumer inside the shared virtualization contract", () => {
  const range = virtualRangeFor({
    datasetEpoch: "sessions:filtered:7",
    stableKey: "session:codex:abc",
    count: 10_000,
    estimate: 58,
    measured: [58, 92],
    scrollOffset: Number.MAX_SAFE_INTEGER,
    viewportSize: 640,
    overscan: 8,
  });

  assert.ok(0 <= range.start && range.start <= range.end && range.end <= 10_000);
});

test("retries a row locator after a virtual seek changes the mounted window", () => {
  const locatorEffectStart = dataTable.indexOf("useLayoutEffect(() => {\n    if (!scrollToRowId)");
  const locatorEffectEnd = dataTable.indexOf("\n\n  const renderGroupedRows", locatorEffectStart);
  const locatorEffect = dataTable.slice(locatorEffectStart, locatorEffectEnd);

  assert.ok(locatorEffectStart >= 0 && locatorEffectEnd > locatorEffectStart);
  assert.match(locatorEffect, /setScrollTop\(/);
  assert.match(locatorEffect, /\}, \[[^\]]*scrollTop[^\]]*\]\);/s);
});
