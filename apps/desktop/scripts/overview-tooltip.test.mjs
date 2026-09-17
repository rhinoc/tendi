import assert from "node:assert/strict";
import { createServer } from "vite";
import { chromium } from "playwright-core";
import { fileURLToPath } from "node:url";
import test from "node:test";

const appDir = fileURLToPath(new URL("..", import.meta.url));
const chromePath = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

const harness = `
import React from "react";
import { createRoot } from "react-dom/client";
import { TooltipProvider } from "/src/components/shared/Tooltip.tsx";
import { OverviewTrendChart } from "/src/views/OverviewTrendChart.tsx";

const usage = {
  inputTokens: 1_000,
  cachedInputTokens: 200,
  cacheWriteInputTokens: 0,
  outputTokens: 300,
  reasoningOutputTokens: 0,
  totalTokens: 1_300,
};
const cost = {
  inputUsd: 0,
  cachedInputUsd: 0,
  cacheWriteInputUsd: 0,
  outputUsd: 0,
  totalUsd: 0,
};
const day = {
  date: "2026-09-15",
  usage,
  cost,
  responses: 2,
  sessions: 1,
  runs: { started: 2, completed: 2, unclosed: 0, totalMs: 1_000, maxMs: 700 },
  aborted: 0,
  compacted: 0,
  models: [],
  projects: [
    { id: "project-a", name: "project-a", usage: { ...usage, totalTokens: 800 }, responses: 1, cost },
    { id: "project-b", name: "project-b", usage: { ...usage, totalTokens: 500 }, responses: 1, cost },
  ],
  tools: [],
  skills: [],
  rateLimits: {},
};
const analytics = {
  revision: 1,
  generatedAt: "2026-09-15T00:00:00Z",
  daysRequested: 1,
  rankDays: 1,
  coverage: { first: day.date, last: day.date, totalSessions: 1, analyzedSessions: 1, indexingSessions: 0 },
  capabilities: [],
  summary: {
    usage,
    cost,
    responses: 2,
    sessions: 1,
    runs: day.runs,
    aborted: 0,
    abortedRate: 0,
    compacted: 0,
    compactedSessions: 0,
  },
  days: [day],
  tools: [],
  skills: [],
  warnings: [],
};

createRoot(document.getElementById("root")).render(
  React.createElement(
    TooltipProvider,
    null,
    React.createElement(OverviewTrendChart, {
      analytics,
      granularity: "day",
      hasOlder: false,
      loadingOlder: false,
      metric: "projects",
      onLoadOlder: () => {},
    }),
  ),
);
`;

test("keeps Overview tooltip details when the pointer enters the tooltip", async () => {
  const server = await createServer({
    root: appDir,
    appType: "spa",
    logLevel: "silent",
    plugins: [{
      name: "overview-tooltip-test-harness",
      resolveId(id) {
        return id === "/__overview-tooltip-harness.tsx" ? "\0overview-tooltip-harness" : null;
      },
      load(id) {
        return id === "\0overview-tooltip-harness" ? harness : null;
      },
    }],
    server: { host: "127.0.0.1", port: 0 },
  });
  await server.listen();
  const port = server.httpServer.address().port;
  const browser = await chromium.launch({ headless: true, executablePath: chromePath });

  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${port}/`, { waitUntil: "domcontentloaded" });
    await page.evaluate(async () => {
      document.body.innerHTML = '<div id="root"></div>';
      await import("/__overview-tooltip-harness.tsx");
    });

    const bar = page.locator('[data-trend-index="0"]');
    await bar.waitFor();
    await bar.hover();
    await page.waitForTimeout(350);

    const tooltip = page.locator('[role="tooltip"]');
    await tooltip.waitFor();
    const before = (await tooltip.textContent()) ?? "";
    assert.match(before, /project-a/);
    assert.match(before, /project-b/);

    const box = await tooltip.boundingBox();
    assert.ok(box, "tooltip should have a visible bounding box");
    await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await page.waitForTimeout(100);

    assert.equal(await tooltip.count(), 1);
    const after = (await tooltip.textContent()) ?? "";
    assert.equal(after, before);
  } finally {
    await browser.close();
    await server.close();
  }
});

test("prefetches enough periods for the initial viewport without a load button", async () => {
  const server = await createServer({
    root: appDir,
    appType: "spa",
    logLevel: "silent",
    plugins: [{
      name: "overview-prefetch-test-harness",
      resolveId(id) {
        return id === "/__overview-prefetch-harness.tsx" ? "\0overview-prefetch-harness" : null;
      },
      load(id) {
        return id === "\0overview-prefetch-harness"
          ? harness.replace("hasOlder: false", "hasOlder: true").replace("onLoadOlder: () => {},", "onLoadOlder: (range) => { document.body.dataset.prefetchRange = String(range); },")
          : null;
      },
    }],
    server: { host: "127.0.0.1", port: 0 },
  });
  await server.listen();
  const port = server.httpServer.address().port;
  const browser = await chromium.launch({ headless: true, executablePath: chromePath });

  try {
    const page = await browser.newPage();
    await page.goto(`http://127.0.0.1:${port}/`, { waitUntil: "domcontentloaded" });
    await page.evaluate(async () => {
      document.body.innerHTML = '<div id="root"></div>';
      await import("/__overview-prefetch-harness.tsx");
    });

    await page.waitForFunction(() => Boolean(document.body.dataset.prefetchRange));
    const prefetchRange = Number(await page.locator("body").getAttribute("data-prefetch-range"));
    assert.ok(Number.isInteger(prefetchRange) && prefetchRange > 1);
    assert.equal(await page.getByRole("button", { name: /load older/i }).count(), 0);
  } finally {
    await browser.close();
    await server.close();
  }
});
