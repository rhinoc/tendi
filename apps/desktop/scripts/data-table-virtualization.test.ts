import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import { fixedVirtualRange, virtualRangeFor } from "../src/lib/virtualization.ts";

const dataTable = await readFile(new URL("../src/components/DataTable.tsx", import.meta.url), "utf8");
const dataTableCss = await readFile(new URL("../src/components/DataTable.css", import.meta.url), "utf8");
const collapsibleAccordion = await readFile(new URL("../src/components/shared/CollapsibleAccordion.tsx", import.meta.url), "utf8");
const tableColumns = await readFile(new URL("../src/lib/tableColumns.tsx", import.meta.url), "utf8");
const virtualViewport = await readFile(new URL("../src/components/shared/useVirtualViewport.ts", import.meta.url), "utf8");
const sessionsView = await readFile(new URL("../src/views/SessionsView.tsx", import.meta.url), "utf8");
const trendChart = await readFile(new URL("../src/views/OverviewTrendChart.tsx", import.meta.url), "utf8");

test("clamps a stale scroll position after a paged data set shrinks", () => {
  const range = fixedVirtualRange(50, 5_560, 240, 58, 4);

  assert.deepEqual(range, { start: 41, end: 50 });
  assert.ok(range.start < range.end);
});

test("keeps an in-range vertical virtual window unchanged", () => {
  assert.deepEqual(fixedVirtualRange(100, 1_160, 240, 58, 4), { start: 16, end: 29 });
});

test("uses the live viewport when the cached size is stale", () => {
  const cachedRange = fixedVirtualRange(50, 50, 720, 72, 4);
  const liveRange = fixedVirtualRange(50, 50, 1_050, 72, 4);

  assert.ok(liveRange.end > cachedRange.end);
  assert.match(dataTable, /const viewportHeight = readViewportSize\(\);/);
  assert.match(virtualViewport, /const next = axis === "horizontal" \? element\.clientWidth : element\.clientHeight;/);
  assert.match(dataTable, /viewportSize: viewportHeight/);
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
  assert.match(locatorEffect, /syncScrollPosition\(\);/);
  assert.match(locatorEffect, /\}, \[[^\]]*scrollTop[^\]]*\]\);/s);
});

test("clears the seek layer after the shared scroll position is committed", () => {
  assert.match(dataTable, /scheduleScrollSync\(\);/);
  assert.match(dataTable, /useLayoutEffect\(\(\) => \{\s*setVirtualSeekVisible\(false\);[\s\S]*?scrollTop/);
});

test("commits a missed virtual window before the next paint", () => {
  assert.match(dataTable, /if \(seekGap \|\| groupedSeekGap\) \{[\s\S]*?flushSync\(\(\) => \{[\s\S]*?syncScrollPosition\(\);/);
});

test("keeps the seek layer above frozen panes", () => {
  assert.match(dataTableCss, /\.dataTableVirtualSeek\s*\{[\s\S]*?z-index:\s*var\(--z-header-rule\);/);
});

test("allows session rows to skip table virtualization", () => {
  assert.match(dataTable, /enableVirtualization = true/);
  assert.match(dataTable, /const virtualizedRows = enableVirtualization &&/);
  assert.match(dataTable, /const virtualizedGroups = enableVirtualization &&/);
  assert.match(sessionsView, /enableVirtualization=\{false\}/);
});

test("synchronizes the virtual window with the browser scroll position after filtering", () => {
  const staleRange = fixedVirtualRange(100, 7_000, 347, 58, 4);
  const browserRange = fixedVirtualRange(100, 0, 347, 58, 4);

  assert.ok(staleRange.start > browserRange.start);
  assert.equal(browserRange.start, 0);
  assert.match(dataTable, /refreshKey: rows/);
  assert.match(dataTable, /if \(!virtualizedRows && !virtualizedGroups\) \{[\s\S]*?syncScrollPosition\(\);/);
  assert.match(virtualViewport, /const next = readScrollOffset\(\);[\s\S]*?setScrollOffset\(\(current\) => current === next/);
});

test("defers grouped scroll restoration until the group content is expanded", () => {
  assert.match(dataTable, /const groupingReady = !grouping\[0\]/);
  assert.match(dataTable, /storedPosition\.top > 0 && !groupingReady\) return undefined/);
  assert.match(dataTable, /groupRows\.every\(\(group\) => expandedGroupSet\.has/);
});

test("disables only the initial grouped accordion animation", () => {
  assert.match(dataTable, /initialGroupingMountRef\.current/);
  assert.match(dataTable, /reduceMotion=\{initialGroupingMountRef\.current\}/);
  assert.match(collapsibleAccordion, /reduceMotion = false/);
  assert.match(collapsibleAccordion, /const reduce = \(useReducedMotion\(\) \?\? false\) \|\| reduceMotion/);
});

test("restores the table viewport with direct scroll offsets", () => {
  const restoreEffectStart = dataTable.indexOf("const shouldRestore = Boolean(scrollRestorationKey");
  const restoreEffectEnd = dataTable.indexOf("\n    } else if (resetRequested)", restoreEffectStart);
  const restoreEffect = dataTable.slice(restoreEffectStart, restoreEffectEnd);

  assert.ok(restoreEffectStart >= 0 && restoreEffectEnd > restoreEffectStart);
  assert.match(restoreEffect, /scroll\.scrollTop =/);
  assert.match(restoreEffect, /scroll\.scrollLeft =/);
  assert.doesNotMatch(restoreEffect, /scroll\.scrollTo\(/);
});

test("keeps data-table viewport restoration instant even when smooth scrolling is configured elsewhere", () => {
  assert.match(dataTableCss, /\.dataTableBodyScroll\s*\{[\s\S]*?scroll-behavior:\s*auto;/);
});

test("coalesces locator scroll events by reading the live DOM position", () => {
  assert.match(sessionsView, /onScroll=\{\(\) => \{\s*scheduleScrollSync\(\);/);
  assert.doesNotMatch(sessionsView, /const nextScrollTop = event\.currentTarget\.scrollTop/);
});

test("uses the shared viewport for horizontal chart virtualization", () => {
  assert.match(trendChart, /useVirtualViewport/);
  assert.match(trendChart, /axis: "horizontal"/);
  assert.match(trendChart, /scheduleScrollSync\(\);/);
});

test("keeps fixed-width frozen-table columns from sizing to cell content", () => {
  assert.match(dataTable, /const scrollGridColumns = useMemo\(\s*\(\) => nonFrozenColumns\.map\(\(column\) => column\.width\)\.join\(" "\)/);
  assert.doesNotMatch(dataTable, /responsiveColumnTrack/);
});

test("keeps MCP source details out of the table columns", () => {
  const mcpColumns = tableColumns.slice(tableColumns.indexOf("export const mcpColumns"));
  assert.doesNotMatch(mcpColumns, /key: "source"/);
});
