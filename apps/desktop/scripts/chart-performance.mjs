import { performance } from "node:perf_hooks";
import React from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { createServer } from "vite";
import { writeStderr, writeStdout } from "./stdio.mjs";

const appDir = new URL("..", import.meta.url).pathname;
const scenarios = new Set(["overview-trend", "overview-trend-zoomed", "skill-session-project", "relationship-graph"]);
const scenario = process.argv[2];
if (!scenarios.has(scenario)) {
  writeStderr("usage: node apps/desktop/scripts/chart-performance.mjs <overview-trend|overview-trend-zoomed|skill-session-project|relationship-graph>");
  process.exit(2);
}

const vite = await createServer({
  root: appDir,
  appType: "spa",
  logLevel: "error",
  server: { middlewareMode: true, hmr: false, ws: false },
});

try {
  const [{ groupAnalyticsDays, selectAnalyticsGranularity }, { OverviewTrendChart, buildTrendPeriodModel }, { SkillSessionProjectChart, groupSkillSessionProjectItems }, { SkillRelationshipMap, buildRelationshipGraphForPerformance }, { TooltipProvider }] = await Promise.all([
    vite.ssrLoadModule("/src/lib/analytics.ts"),
    vite.ssrLoadModule("/src/views/OverviewTrendChart.tsx"),
    vite.ssrLoadModule("/src/features/sessions/SkillSessionProjectChart.tsx"),
    vite.ssrLoadModule("/src/features/skills/SkillRelationshipMap.tsx"),
    vite.ssrLoadModule("/src/components/shared/Tooltip.tsx"),
  ]);
  const runner = {
    "overview-trend": () => runOverviewTrend(groupAnalyticsDays, selectAnalyticsGranularity, buildTrendPeriodModel, OverviewTrendChart, TooltipProvider),
    "overview-trend-zoomed": () => runOverviewTrend(groupAnalyticsDays, selectAnalyticsGranularity, buildTrendPeriodModel, OverviewTrendChart, TooltipProvider, "day"),
    "skill-session-project": () => runSkillSessionProject(groupSkillSessionProjectItems, SkillSessionProjectChart),
    "relationship-graph": () => runRelationshipGraph(buildRelationshipGraphForPerformance, SkillRelationshipMap),
  }[scenario];
  let last = runner();
  for (let index = 0; index < 2; index += 1) last = runner();
  const samplesMs = [];
  let checksum = 17;
  for (let index = 0; index < 9; index += 1) {
    const started = performance.now();
    last = runner();
    samplesMs.push(performance.now() - started);
    checksum = (Math.imul(checksum, 31) + last.checksum) >>> 0;
  }
  samplesMs.sort((left, right) => left - right);
  const p95Index = Math.min(samplesMs.length - 1, Math.ceil(samplesMs.length * 0.95) - 1);
  writeStdout(JSON.stringify({
    scenario,
    operationMs: samplesMs[Math.floor(samplesMs.length / 2)],
    p95Ms: samplesMs[p95Index],
    iterations: samplesMs.length,
    warmupIterations: 2,
    input: last.input,
    output: last.output,
    checksum,
  }));
} finally {
  await vite.close();
}

function runOverviewTrend(groupAnalyticsDays, selectAnalyticsGranularity, buildTrendPeriodModel, OverviewTrendChart, TooltipProvider, forcedGranularity = null) {
  const days = Array.from({ length: 365 }, (_, index) => {
    const date = new Date(Date.UTC(2025, 0, 1 + index)).toISOString().slice(0, 10);
    return {
      date,
      usage: {
        inputTokens: 100_000 + index * 17,
        cachedInputTokens: 30_000 + index * 7,
        cacheWriteInputTokens: 0,
        outputTokens: 20_000 + index * 11,
        reasoningOutputTokens: 1_000,
        totalTokens: 120_000 + index * 28,
      },
      responses: 20 + index % 13,
      sessions: 5 + index % 17,
      runs: { started: 10 + index % 7, completed: 8 + index % 5, unclosed: index % 2, totalMs: 1000, maxMs: 5000 },
      aborted: index % 3,
      compacted: index % 4,
      models: Array.from({ length: 6 }, (_, modelIndex) => ({ model: `model-${modelIndex}`, totalTokens: 10_000 + index * (modelIndex + 1) })),
      tools: Array.from({ length: 10 }, (_, toolIndex) => ({ name: `tool-${toolIndex}`, server: `server-${toolIndex % 2}`, calls: 1 + ((index + toolIndex) % 9) })),
      skills: Array.from({ length: 8 }, (_, skillIndex) => ({ name: `skill-${skillIndex}`, server: "local", calls: 1 + ((index + skillIndex) % 7) })),
      rateLimits: {},
    };
  });
  const granularity = forcedGranularity ?? selectAnalyticsGranularity(days.length);
  const computeStarted = performance.now();
  const periods = groupAnalyticsDays(days, granularity);
  const totals = new Map();
  for (const period of periods) {
    for (const model of period.models) totals.set(model.model, (totals.get(model.model) ?? 0) + model.totalTokens);
  }
  const topCategories = [...totals.entries()]
    .sort((left, right) => right[1] - left[1] || left[0].localeCompare(right[0]))
    .slice(0, 4)
    .map(([key, value]) => ({ key, label: key, value }));
  const max = Math.max(1, ...periods.map((period) => period.totalTokens));
  const rungUnit = max / 28;
  let checksum = 17;
  const renderedStart = periods.length > 80 ? Math.max(0, periods.length - 64 - 8) : 0;
  const renderedPeriods = periods.slice(renderedStart);
  let renderedRungs = 0;
  for (let localIndex = 0; localIndex < renderedPeriods.length; localIndex += 1) {
    const model = buildTrendPeriodModel(renderedPeriods[localIndex], renderedStart + localIndex, "tokens", topCategories, totals.size > topCategories.length, rungUnit);
    renderedRungs += model.rungs.length;
    checksum = (Math.imul(checksum, 31) + model.rungs.length + model.tooltipSegments.length) >>> 0;
  }
  const computeMs = performance.now() - computeStarted;
  const markup = renderToStaticMarkup(React.createElement(TooltipProvider, null,
    React.createElement(OverviewTrendChart, {
      analytics: {
      revision: 1,
      generatedAt: "2026-08-14T00:00:00Z",
      daysRequested: days.length,
      rankDays: days.length,
      coverage: { first: days[0].date, last: days.at(-1).date, totalSessions: 1, analyzedSessions: 1, indexingSessions: 0 },
      capabilities: [],
      summary: {
        usage: days.reduce((usage, day) => ({
          inputTokens: usage.inputTokens + day.usage.inputTokens,
          cachedInputTokens: usage.cachedInputTokens + day.usage.cachedInputTokens,
          cacheWriteInputTokens: 0,
          outputTokens: usage.outputTokens + day.usage.outputTokens,
          reasoningOutputTokens: usage.reasoningOutputTokens + day.usage.reasoningOutputTokens,
          totalTokens: usage.totalTokens + day.usage.totalTokens,
        }), { inputTokens: 0, cachedInputTokens: 0, cacheWriteInputTokens: 0, outputTokens: 0, reasoningOutputTokens: 0, totalTokens: 0 }),
        responses: 1,
        sessions: 1,
        runs: { started: 1, completed: 1, unclosed: 0, totalMs: 1, maxMs: 1 },
        aborted: 0,
        abortedRate: 0,
        compacted: 0,
        compactedSessions: 0,
      },
      days,
      tools: [],
      skills: [],
      warnings: [],
      },
      granularity,
      hasOlder: false,
      loadingOlder: false,
      metric: "tokens",
      onLoadOlder: () => {},
    }),
  ));
  return {
    input: { days: days.length, modelsPerDay: 6, toolsPerDay: 10, skillsPerDay: 8, requestedGranularity: forcedGranularity ?? "auto" },
    output: { periods: periods.length, granularity, categories: totals.size, topCategories: topCategories.length, computeMs, renderedPeriods: renderedPeriods.length, virtualized: periods.length > 80, renderedHtmlBytes: Buffer.byteLength(markup), renderedRungs },
    checksum: (Math.imul(checksum, 31) + markup.length) >>> 0,
  };
}

function runSkillSessionProject(groupItems, SkillSessionProjectChart) {
  const items = Array.from({ length: 2_000 }, (_, index) => ({
    key: `chart-row-${index}`,
    skillKey: `skill-${index % 80}`,
    skillLabel: `Skill ${index % 80}`,
    sessionLabel: `Session ${index}`,
    projectKey: `project-${index % 120}`,
    projectLabel: `Project ${index % 120}`,
  }));
  const groupStarted = performance.now();
  const skills = groupItems(items, (item) => [item.skillKey, item.skillLabel]);
  const projects = groupItems(items, (item) => [item.projectKey, item.projectLabel]);
  const groupMs = performance.now() - groupStarted;
  let checksum = 17;
  for (const group of skills) checksum = (Math.imul(checksum, 31) + group.count) >>> 0;
  for (const group of projects) checksum = (Math.imul(checksum, 31) + group.count) >>> 0;
  const renderedItems = items.slice(0, 50);
  const markup = renderToStaticMarkup(React.createElement(SkillSessionProjectChart, {
    items: renderedItems,
    ariaLabel: "Performance fixture",
  }));
  return {
    input: { items: items.length, distinctSkills: 80, distinctProjects: 120 },
    output: { sessionNodes: items.length, skillNodes: skills.length, projectNodes: projects.length, links: items.length * 2, groupMs, renderedSessionNodes: renderedItems.length, renderedHtmlBytes: Buffer.byteLength(markup) },
    checksum: (Math.imul(checksum, 31) + markup.length) >>> 0,
  };
}

function runRelationshipGraph(buildGraph, SkillRelationshipMap) {
  const nodes = Array.from({ length: 180 }, (_, index) => ({ name: `node-${index}`, label: `Node ${index}` }));
  const edges = Array.from({ length: 720 }, (_, index) => ({
    from: `node-${index % 180}`,
    to: `node-${(index * 37 + 11) % 180}`,
    key: `edge-${index}`,
  })).filter((edge) => edge.from !== edge.to);
  const layoutStarted = performance.now();
  const graph = buildGraph(nodes, edges);
  const layoutMs = performance.now() - layoutStarted;
  let checksum = graph.nodes.length + graph.edges.length;
  for (const node of graph.nodes.slice(0, 8)) checksum = (Math.imul(checksum, 31) + Math.round(node.x) + Math.round(node.y)) >>> 0;
  const markup = renderToStaticMarkup(React.createElement(SkillRelationshipMap, {
    nodes,
    edges,
  }));
  return {
    input: { nodes: nodes.length, suppliedEdges: edges.length, layoutIterations: 220 },
    output: { nodes: graph.nodes.length, edges: graph.edges.length, layoutMs, renderedHtmlBytes: Buffer.byteLength(markup) },
    checksum: (Math.imul(checksum, 31) + markup.length) >>> 0,
  };
}
