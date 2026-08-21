import { existsSync, statSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { performance } from "node:perf_hooks";
import { createServer } from "vite";
import { writeStdout } from "./stdio.mjs";

const appDir = resolve(new URL("..", import.meta.url).pathname);
const repoDir = resolve(appDir, "../..");
const tendiBin = process.env.TENDI_BIN || join(repoDir, "target/debug/tendi");
const port = Number(process.env.TENDI_REAL_DATA_PORT || 5195);

function loadPlaywrightCore() {
  const require = createRequire(import.meta.url);
  try {
    return require("playwright-core");
  } catch {
    return createRequire("/opt/homebrew/lib/node_modules/playwright/package.json")("playwright-core");
  }
}

function chromiumExecutablePath() {
  const cached = join(
    homedir(),
    "Library/Caches/ms-playwright/chromium-1223/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
  );
  if (existsSync(cached)) return cached;
  const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  return existsSync(chrome) ? chrome : undefined;
}

function runJson(args) {
  const started = performance.now();
  const result = spawnSync(tendiBin, args, {
    cwd: repoDir,
    encoding: "utf8",
    maxBuffer: 512 * 1024 * 1024,
  });
  const stdout = result.stdout || "";
  if (result.status !== 0) {
    throw new Error(`${tendiBin} ${args.join(" ")} failed (${result.status}): ${(result.stderr || "").slice(0, 600)}`);
  }
  return {
    value: JSON.parse(stdout),
    command: args[1] === "transcript" ? "sessions transcript <largest-real-session> --agent codex --json" : args.join(" "),
    elapsedMs: Number((performance.now() - started).toFixed(1)),
    stdoutBytes: Buffer.byteLength(stdout),
  };
}

function sessionDate(session) {
  const value = session.started_at || session.startedAt || session.updated_at || session.updatedAt;
  const date = typeof value === "string" && value.length >= 10 ? value.slice(0, 10) : "";
  return /^\d{4}-\d{2}-\d{2}$/.test(date) ? date : null;
}

function buildRealAnalytics(sessions) {
  const days = new Map();
  for (const session of sessions) {
    const date = sessionDate(session);
    if (!date) continue;
    const day = days.get(date) || {
      date,
      usage: { inputTokens: 0, cachedInputTokens: 0, cacheWriteInputTokens: 0, outputTokens: 0, reasoningOutputTokens: 0, totalTokens: 0 },
      responses: 0,
      sessions: 0,
      runs: { started: 0, completed: 0, unclosed: 0, totalMs: 0, maxMs: 0 },
      aborted: 0,
      compacted: 0,
      models: [],
      tools: [],
      skills: [],
      rateLimits: {},
    };
    const usage = session.token_usage || session.tokenUsage || {};
    const messages = Number(session.message_count || session.messages || 0);
    const usageValue = (camelKey, snakeKey) => Number(usage[camelKey] ?? usage[snakeKey] ?? 0);
    day.sessions += 1;
    day.responses += messages;
    day.runs.started += 1;
    day.runs.completed += 1;
    for (const [camelKey, snakeKey] of [
      ["inputTokens", "input_tokens"],
      ["cachedInputTokens", "cached_input_tokens"],
      ["cacheWriteInputTokens", "cache_write_input_tokens"],
      ["outputTokens", "output_tokens"],
      ["reasoningOutputTokens", "reasoning_output_tokens"],
      ["totalTokens", "total_tokens"],
    ]) {
      day.usage[camelKey] += usageValue(camelKey, snakeKey);
    }
    const model = String(session.model || "unknown");
    const modelEntry = day.models.find((entry) => entry.model === model);
    if (modelEntry) modelEntry.totalTokens += usageValue("totalTokens", "total_tokens");
    else day.models.push({ model, totalTokens: usageValue("totalTokens", "total_tokens") });
    days.set(date, day);
  }
  const sortedDays = [...days.values()].sort((left, right) => left.date.localeCompare(right.date));
  const summary = sortedDays.reduce((total, day) => ({
    usage: Object.fromEntries(Object.keys(total.usage).map((key) => [key, total.usage[key] + day.usage[key]])),
    responses: total.responses + day.responses,
    sessions: total.sessions + day.sessions,
    runs: {
      started: total.runs.started + day.runs.started,
      completed: total.runs.completed + day.runs.completed,
      unclosed: total.runs.unclosed + day.runs.unclosed,
      totalMs: total.runs.totalMs + day.runs.totalMs,
      maxMs: Math.max(total.runs.maxMs, day.runs.maxMs),
    },
    aborted: total.aborted + day.aborted,
    abortedRate: 0,
    compacted: total.compacted + day.compacted,
    compactedSessions: total.compactedSessions + (day.compacted ? 1 : 0),
  }), {
    usage: { inputTokens: 0, cachedInputTokens: 0, cacheWriteInputTokens: 0, outputTokens: 0, reasoningOutputTokens: 0, totalTokens: 0 },
    responses: 0,
    sessions: 0,
    runs: { started: 0, completed: 0, unclosed: 0, totalMs: 0, maxMs: 0 },
    aborted: 0,
    abortedRate: 0,
    compacted: 0,
    compactedSessions: 0,
  });
  return {
    revision: 1,
    generatedAt: new Date().toISOString(),
    daysRequested: 365,
    rankDays: 30,
    coverage: {
      first: sortedDays[0]?.date,
      last: sortedDays.at(-1)?.date,
      totalSessions: sessions.length,
      analyzedSessions: sessions.length,
      indexingSessions: 0,
    },
    capabilities: [{ agent: "real-data", tokenUsage: true, reasoningTokens: true, explicitRuns: true, rateLimitHistory: false }],
    summary,
    days: sortedDays,
    tools: [],
    skills: [],
    warnings: [],
  };
}

function loadSnapshot() {
  const scan = runJson(["scan", "--json"]);
  const report = scan.value;
  const sessions = report.sessions?.sessions || [];
  const existing = sessions
    .map((session) => {
      try {
        return { session, size: statSync(session.path).size };
      } catch {
        return null;
      }
    })
    .filter(Boolean)
    .sort((left, right) => right.size - left.size);
  const largest = existing[0];
  if (!largest) throw new Error("No readable real session transcript found");
  const agent = String(largest.session.agent || "codex").toLowerCase();
  const transcript = runJson(["sessions", "transcript", largest.session.path, "--agent", agent, "--json"]);
  const allItems = Array.isArray(transcript.value) ? transcript.value : [];
  const locatorItems = [];
  let pendingLocatorItem = null;
  let groupedIndex = 0;
  let previousWasTool = false;
  for (const item of allItems) {
    const type = `${item.type || item.kind || ""}`;
    const itemIndex = type === "tool" && previousWasTool ? groupedIndex - 1 : groupedIndex++;
    if (type === "user") {
      pendingLocatorItem = {
        index: itemIndex,
        label: `${item.body || ""}`.trim(),
        response: "",
      };
      locatorItems.push(pendingLocatorItem);
    } else if (type === "assistant" && pendingLocatorItem) {
      pendingLocatorItem.response = `${item.body || ""}`.trim();
      pendingLocatorItem = null;
    }
    previousWasTool = type === "tool";
  }
  const pageSize = 160;
  const transcriptPages = [];
  for (let index = 0; index < allItems.length; index += pageSize) {
    const pageIndex = transcriptPages.length;
    const items = allItems.slice(index, index + pageSize);
    transcriptPages.push({
      items,
      locatorItems: pageIndex === 0 ? locatorItems : [],
      warnings: [],
      nextCursor: index + pageSize < allItems.length ? `real:${pageIndex + 1}` : null,
      done: index + pageSize >= allItems.length,
      sourceVersion: `${largest.size}:${Math.floor(statSync(largest.session.path).mtimeMs)}`,
      unchanged: false,
      restartRequired: false,
    });
  }
  const search = runJson(["sessions", "search", "rollout", "--json"]);
  return {
    report,
    sessions,
    transcriptPages,
    selectedSession: largest.session,
    searchRows: Array.isArray(search.value) ? search.value : [],
    analytics: buildRealAnalytics(sessions),
    commands: [scan, transcript, search].map(({ value: _value, ...meta }) => meta),
    source: {
      sessions: sessions.length,
      skills: report.skills?.skills?.length || 0,
      rules: report.rules?.rules?.length || 0,
      hooks: report.hooks?.hooks?.length || 0,
      mcp: report.mcp?.servers?.length || 0,
      largestTranscriptBytes: largest.size,
      largestTranscriptItems: allItems.length,
    },
  };
}

const snapshot = loadSnapshot();
const { chromium } = loadPlaywrightCore();
const server = await createServer({
  root: appDir,
  appType: "spa",
  logLevel: "error",
  server: { host: "127.0.0.1", port, strictPort: true },
});
let browser;

try {
  await server.listen();
  browser = await chromium.launch({ headless: true, executablePath: chromiumExecutablePath() });
  const page = await browser.newPage({ viewport: { width: 1440, height: 1000 } });
  await page.addInitScript(({ report, transcriptPages, searchRows, analytics }) => {
    window.__TENDI_REAL_PERF__ = { longTasks: [], longAnimationFrames: [] };
    const supported = PerformanceObserver.supportedEntryTypes || [];
    if (supported.includes("longtask")) {
      new PerformanceObserver((list) => window.__TENDI_REAL_PERF__.longTasks.push(...list.getEntries().map((entry) => entry.duration))).observe({ type: "longtask", buffered: true });
    }
    if (supported.includes("long-animation-frame")) {
      new PerformanceObserver((list) => window.__TENDI_REAL_PERF__.longAnimationFrames.push(...list.getEntries().map((entry) => entry.duration))).observe({ type: "long-animation-frame", buffered: true });
    }
    const callbacks = new Map();
    let nextCallbackId = 1;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      invoke: async (command, args = {}) => {
        if (command === "plugin:event|listen") {
          callbacks.set(args.handler, args.event);
          return args.handler;
        }
        if (command === "plugin:event|unlisten") return null;
        if (command === "settings_get") return { appearance: "system", lightTheme: "default", darkTheme: "default", additionalSessionRoots: [], configProfiles: {} };
        if (command === "scan") return report;
        if (command === "skills_list" || command === "skills_refresh") return report.skills?.skills || [];
        if (command === "sessions_list") return report.sessions?.sessions || [];
        if (command === "rules_list") return report.rules?.rules || [];
        if (command === "hooks_list") return report.hooks?.hooks || [];
        if (command === "mcp_list") return report.mcp?.servers || [];
        if (command === "agents_list") return report.agents?.agents || [];
        if (command === "prompts_list") return report.prompts?.prompts || [];
        if (command === "analytics_overview") return analytics;
        if (command === "analytics_revision") return { revision: analytics.revision };
        if (command === "sessions_search") return searchRows;
        if (command === "session_skill_links") return [];
        if (command === "sessions_scan_start") {
          queueMicrotask(() => {
            for (const [handler, event] of callbacks) {
              const callback = window[`__TAURI_CALLBACK_${handler}`];
              if (typeof callback === "function" && event === "sessions://scan") {
                callback({ event, payload: { generation: 1, phase: "backfill", upserts: [], deleted: [], scanned: report.sessions?.sessions?.length || 0, complete: true } });
              }
            }
          });
          return 1;
        }
        if (command === "session_transcript") {
          const cursor = typeof args.cursor === "string" && args.cursor.startsWith("real:") ? Number(args.cursor.slice(5)) : 0;
          return transcriptPages[cursor] || transcriptPages[0] || { items: [], warnings: [], nextCursor: null, done: true };
        }
        if (command === "session_transcript_search") {
          const query = `${args.query || ""}`.trim().toLowerCase();
          const scopes = args.scopes || { user: true, assistant: true, system: false, tool: false };
          const hits = [];
          const items = transcriptPages.flatMap((page) => page.items || []);
          const itemType = (item) => `${item.type || item.kind || "assistant"}`;
          const scopeFor = (type) => {
            if (["user", "notification"].includes(type)) return "user";
            if (["context", "compaction", "model_config"].includes(type)) return "system";
            if (["tool", "toolGroup"].includes(type)) return "tool";
            return "assistant";
          };
          const itemText = (item) => [item.body, item.tag, item.command, item.result, item.model, item.effort, item.callId]
            .map((value) => `${value || ""}`)
            .join("\n")
            .toLowerCase();
          let groupIndex = 0;
          for (let index = 0; index < items.length;) {
            const firstType = itemType(items[index]);
            if (firstType === "tool") {
              let toolIndex = 0;
              while (index < items.length && itemType(items[index]) === "tool") {
                if (scopes.tool && itemText(items[index]).includes(query)) hits.push({ groupIndex, toolIndex });
                index += 1;
                toolIndex += 1;
              }
              groupIndex += 1;
              continue;
            }
            if (scopes[scopeFor(firstType)] && itemText(items[index]).includes(query)) hits.push({ groupIndex });
            index += 1;
            groupIndex += 1;
          }
          return {
            hits,
            warnings: [],
            sourceVersion: transcriptPages[0]?.sourceVersion || "",
          };
        }
        return null;
      },
      transformCallback: (callback) => {
        const id = nextCallbackId++;
        window[`__TAURI_CALLBACK_${id}`] = callback;
        return id;
      },
      unregisterCallback: (id) => { delete window[`__TAURI_CALLBACK_${id}`]; callbacks.delete(id); },
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    };
  }, snapshot);

  const started = performance.now();
  await page.goto(`http://127.0.0.1:${port}/`);
  await page.getByRole("heading", { name: "Overview" }).waitFor({ timeout: 10000 }).catch(() => {});
  const firstViewMs = Number((performance.now() - started).toFixed(1));
  await page.locator(".overviewTrendViewport").waitFor({ timeout: 10000 }).catch(() => {});
  const chartMetrics = await page.evaluate(async () => {
    const viewport = document.querySelector(".overviewTrendViewport");
    if (!viewport) return { missing: true };
    window.__TENDI_REAL_PERF__.longTasks.length = 0;
    window.__TENDI_REAL_PERF__.longAnimationFrames.length = 0;
    const startedAt = performance.now();
    const steps = 12;
    for (let index = 0; index < steps; index += 1) {
      viewport.scrollLeft = (viewport.scrollWidth - viewport.clientWidth) * (index / (steps - 1));
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
    const perf = window.__TENDI_REAL_PERF__;
    return {
      missing: false,
      elapsedMs: Number((performance.now() - startedAt).toFixed(1)),
      periods: document.querySelectorAll(".overviewTrendBarButton").length,
      rungs: document.querySelectorAll(".overviewTrendRung").length,
      emptyMessage: document.querySelector(".overviewTrendEmptyMessage")?.textContent?.trim() || null,
      chartText: document.querySelector(".overviewTrendBlock")?.textContent?.trim().slice(0, 180) || null,
      scrollWidth: viewport.scrollWidth,
      clientWidth: viewport.clientWidth,
      longTasks: perf.longTasks.length,
      maxLongTaskMs: Math.max(0, ...perf.longTasks),
      longAnimationFrames: perf.longAnimationFrames.length,
      maxLongAnimationFrameMs: Math.max(0, ...perf.longAnimationFrames),
    };
  });

  await page.getByRole("button", { name: "Sessions", exact: true }).click();
  await page.getByRole("heading", { name: "Sessions", exact: true }).waitFor({ timeout: 10000 });
  await page.locator(".dataRow").first().waitFor({ timeout: 10000 });
  const listMetrics = await page.evaluate(async () => {
    const scroll = document.querySelector(".dataTableBodyScroll");
    if (!scroll) return { missing: true };
    window.__TENDI_REAL_PERF__.longTasks.length = 0;
    window.__TENDI_REAL_PERF__.longAnimationFrames.length = 0;
    const startedAt = performance.now();
    const steps = 24;
    for (let index = 0; index < steps; index += 1) {
      scroll.scrollTop = (scroll.scrollHeight - scroll.clientHeight) * (index / (steps - 1));
      await new Promise((resolve) => requestAnimationFrame(resolve));
    }
    const perf = window.__TENDI_REAL_PERF__;
    return {
      missing: false,
      elapsedMs: Number((performance.now() - startedAt).toFixed(1)),
      rows: document.querySelectorAll(".dataRow").length,
      scrollHeight: scroll.scrollHeight,
      clientHeight: scroll.clientHeight,
      longTasks: perf.longTasks.length,
      maxLongTaskMs: Math.max(0, ...perf.longTasks),
      longAnimationFrames: perf.longAnimationFrames.length,
      maxLongAnimationFrameMs: Math.max(0, ...perf.longAnimationFrames),
    };
  });

  const groupButton = page.getByRole("button", { name: "Group by Agent", exact: true });
  let groupedListMetrics = { missing: true };
  if (await groupButton.count()) {
    await groupButton.click();
    await page.locator(".dataGroup").first().waitFor({ timeout: 10000 });
    groupedListMetrics = await page.evaluate(async () => {
      const scroll = document.querySelector(".dataTableBodyScroll");
      if (!scroll) return { missing: true };
      window.__TENDI_REAL_PERF__.longTasks.length = 0;
      window.__TENDI_REAL_PERF__.longAnimationFrames.length = 0;
      const startedAt = performance.now();
      const steps = 24;
      for (let index = 0; index < steps; index += 1) {
        scroll.scrollTop = (scroll.scrollHeight - scroll.clientHeight) * (index / (steps - 1));
        await new Promise((resolve) => requestAnimationFrame(resolve));
      }
      const perf = window.__TENDI_REAL_PERF__;
      return {
        missing: false,
        elapsedMs: Number((performance.now() - startedAt).toFixed(1)),
        groups: document.querySelectorAll(".dataGroup").length,
        rows: document.querySelectorAll(".dataRow").length,
        scrollHeight: scroll.scrollHeight,
        clientHeight: scroll.clientHeight,
        longTasks: perf.longTasks.length,
        maxLongTaskMs: Math.max(0, ...perf.longTasks),
        longAnimationFrames: perf.longAnimationFrames.length,
        maxLongAnimationFrameMs: Math.max(0, ...perf.longAnimationFrames),
      };
    });
  }

  await page.locator(".dataRow").first().click();
  await page.locator(".transcriptPanel").waitFor({ timeout: 10000 });
  const transcriptBefore = await page.evaluate(() => ({ items: document.querySelectorAll("[data-transcript-index]").length, textBytes: document.querySelector(".transcript")?.textContent?.length || 0 }));
  await page.keyboard.press("Meta+f");
  const searchInput = page.getByRole("textbox", { name: "Search messages" });
  await searchInput.waitFor({ timeout: 3000 });
  const searchStarted = performance.now();
  await page.evaluate(() => { window.__TENDI_REAL_SEARCH_STARTED__ = performance.now(); });
  await searchInput.fill("message");
  await page.waitForTimeout(450);
  await page.waitForFunction(() => {
    const input = document.querySelector('input[aria-label*="Search messages"], input[aria-label*="Loading all messages"]');
    return input?.getAttribute("aria-busy") !== "true" && Boolean(document.querySelector(".transcriptSearchCount"));
  }, { timeout: 30000 });
  const searchMetrics = await page.evaluate(() => ({
    elapsedMs: Number((performance.now() - window.__TENDI_REAL_SEARCH_STARTED__).toFixed(1)),
    renderedItems: document.querySelectorAll("[data-transcript-index]").length,
    transcriptTextBytes: document.querySelector(".transcript")?.textContent?.length || 0,
    searchCount: document.querySelector(".transcriptSearchCount")?.textContent || "",
  })).catch(() => null);
  if (searchMetrics) searchMetrics.externalElapsedMs = Number((performance.now() - searchStarted).toFixed(1));
  const finalPerf = await page.evaluate(() => window.__TENDI_REAL_PERF__);

  writeStdout(JSON.stringify({
    source: snapshot.source,
    analyticsInput: {
      days: snapshot.analytics.days.length,
      first: snapshot.analytics.coverage.first || null,
      last: snapshot.analytics.coverage.last || null,
    },
    rustCommands: snapshot.commands,
    web: {
      firstViewMs,
      chart: chartMetrics,
      list: listMetrics,
      groupedList: groupedListMetrics,
      transcriptBefore,
      search: searchMetrics,
      performanceObservers: finalPerf,
    },
  }, null, 2));
} finally {
  await browser?.close().catch(() => {});
  await server.close().catch(() => {});
}
