import { spawnSync } from "node:child_process";
import {
  closeSync,
  existsSync,
  mkdirSync,
  openSync,
  readFileSync,
  statSync,
  writeFileSync,
  writeSync,
} from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { performance } from "node:perf_hooks";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const binary = join(root, "target/debug/tendi");
const perfBinary = join(root, "target/debug/tendi-perf");
const chartPerformanceRunner = join(root, "apps/desktop/scripts/chart-performance.mjs");
const outputDir = join(root, "target/perf");
const defaultBaselinePath = join(outputDir, "baseline.json");
const latestPath = join(outputDir, "latest.json");
const mib = 1024 * 1024;
const capturedOutputBufferBytes = 64 * mib;

const options = parseArgs(process.argv.slice(2));
const thresholds = {
  skillsMs: envNumber("TENDI_PERF_SKILLS_MS", 300),
  hooksMs: envNumber("TENDI_PERF_HOOKS_MS", 40),
  rulesMs: envNumber("TENDI_PERF_RULES_MS", 40),
  mcpMs: envNumber("TENDI_PERF_MCP_MS", 40),
  sessionsMs: envNumber("TENDI_PERF_SESSIONS_MS", 3_000),
  sessionsMaxMs: envNumber("TENDI_PERF_SESSIONS_MAX_MS", 8_000),
  sessionsRssBytes: envNumber("TENDI_PERF_SESSIONS_RSS_MIB", 56) * mib,
  sessionsOutputBytes: envNumber("TENDI_PERF_SESSIONS_OUTPUT_MIB", 8) * mib,
  syntheticMs: envNumber("TENDI_PERF_SYNTHETIC_MS", 600),
  syntheticRssBytes: envNumber("TENDI_PERF_SYNTHETIC_RSS_MIB", 16) * mib,
  transcriptMs: envNumber("TENDI_PERF_TRANSCRIPT_MS", 4_500),
  transcriptRssRatio: envNumber("TENDI_PERF_TRANSCRIPT_RSS_RATIO", 0.3),
  transcriptRssFloorBytes: envNumber("TENDI_PERF_TRANSCRIPT_RSS_FLOOR_MIB", 80) * mib,
  idleCpuAverage: envNumber("TENDI_PERF_IDLE_CPU_AVG", 1),
  idleCpuMax: envNumber("TENDI_PERF_IDLE_CPU_MAX", 5),
  primaryOverviewMs: envNumber("TENDI_PERF_PRIMARY_OVERVIEW_MS", 25),
  primaryOverviewRssBytes: envNumber("TENDI_PERF_PRIMARY_OVERVIEW_RSS_MIB", 24) * mib,
  primaryOverviewPayloadBytes: envNumber("TENDI_PERF_PRIMARY_OVERVIEW_PAYLOAD_MIB", 0.25) * mib,
  primaryPromptsMs: envNumber("TENDI_PERF_PRIMARY_PROMPTS_MS", 50),
  primaryPromptsRssBytes: envNumber("TENDI_PERF_PRIMARY_PROMPTS_RSS_MIB", 24) * mib,
  primaryPromptsPayloadBytes: envNumber("TENDI_PERF_PRIMARY_PROMPTS_PAYLOAD_MIB", 1.5) * mib,
  primaryConfigMs: envNumber("TENDI_PERF_PRIMARY_CONFIG_MS", 12),
  primaryConfigRssBytes: envNumber("TENDI_PERF_PRIMARY_CONFIG_RSS_MIB", 16) * mib,
  primaryConfigPayloadBytes: envNumber("TENDI_PERF_PRIMARY_CONFIG_PAYLOAD_MIB", 0.0625) * mib,
  primarySettingsMs: envNumber("TENDI_PERF_PRIMARY_SETTINGS_MS", 2),
  primarySettingsRssBytes: envNumber("TENDI_PERF_PRIMARY_SETTINGS_RSS_MIB", 16) * mib,
  primarySettingsPayloadBytes: envNumber("TENDI_PERF_PRIMARY_SETTINGS_PAYLOAD_MIB", 0.015625) * mib,
  secondarySessionPageMs: envNumber("TENDI_PERF_SECONDARY_SESSION_PAGE_MS", 35),
  secondarySessionPageRssBytes: envNumber("TENDI_PERF_SECONDARY_SESSION_PAGE_RSS_MIB", 24) * mib,
  secondarySessionPagePayloadBytes: envNumber("TENDI_PERF_SECONDARY_SESSION_PAGE_PAYLOAD_MIB", 1) * mib,
  secondaryLinkedSessionsMs: envNumber("TENDI_PERF_SECONDARY_LINKED_SESSIONS_MS", 25),
  secondaryLinkedSessionsRssBytes: envNumber("TENDI_PERF_SECONDARY_LINKED_SESSIONS_RSS_MIB", 24) * mib,
  secondaryLinkedSessionsPayloadBytes: envNumber("TENDI_PERF_SECONDARY_LINKED_SESSIONS_PAYLOAD_MIB", 0.75) * mib,
  secondarySkillFilesMs: envNumber("TENDI_PERF_SECONDARY_SKILL_FILES_MS", 10),
  secondarySkillFilesRssBytes: envNumber("TENDI_PERF_SECONDARY_SKILL_FILES_RSS_MIB", 16) * mib,
  secondarySkillFilesPayloadBytes: envNumber("TENDI_PERF_SECONDARY_SKILL_FILES_PAYLOAD_MIB", 0.125) * mib,
  secondaryRuleDetailMs: envNumber("TENDI_PERF_SECONDARY_RULE_DETAIL_MS", 15),
  secondaryRuleDetailRssBytes: envNumber("TENDI_PERF_SECONDARY_RULE_DETAIL_RSS_MIB", 16) * mib,
  secondaryRuleDetailPayloadBytes: envNumber("TENDI_PERF_SECONDARY_RULE_DETAIL_PAYLOAD_MIB", 0.25) * mib,
  secondaryHookDetailMs: envNumber("TENDI_PERF_SECONDARY_HOOK_DETAIL_MS", 20),
  secondaryHookDetailRssBytes: envNumber("TENDI_PERF_SECONDARY_HOOK_DETAIL_RSS_MIB", 16) * mib,
  secondaryHookDetailPayloadBytes: envNumber("TENDI_PERF_SECONDARY_HOOK_DETAIL_PAYLOAD_MIB", 0.25) * mib,
  secondaryConfigReadMs: envNumber("TENDI_PERF_SECONDARY_CONFIG_READ_MS", 10),
  secondaryConfigReadRssBytes: envNumber("TENDI_PERF_SECONDARY_CONFIG_READ_RSS_MIB", 16) * mib,
  secondaryConfigReadPayloadBytes: envNumber("TENDI_PERF_SECONDARY_CONFIG_READ_PAYLOAD_MIB", 0.25) * mib,
  tertiarySkillSaveMs: envNumber("TENDI_PERF_TERTIARY_SKILL_SAVE_MS", 45),
  tertiarySkillSaveRssBytes: envNumber("TENDI_PERF_TERTIARY_SKILL_SAVE_RSS_MIB", 24) * mib,
  tertiarySkillSavePayloadBytes: envNumber("TENDI_PERF_TERTIARY_SKILL_SAVE_PAYLOAD_MIB", 0.0625) * mib,
  tertiaryHookDeleteMs: envNumber("TENDI_PERF_TERTIARY_HOOK_DELETE_MS", 55),
  tertiaryHookDeleteRssBytes: envNumber("TENDI_PERF_TERTIARY_HOOK_DELETE_RSS_MIB", 24) * mib,
  tertiaryHookDeletePayloadBytes: envNumber("TENDI_PERF_TERTIARY_HOOK_DELETE_PAYLOAD_MIB", 0.25) * mib,
  tertiaryPromptCrudMs: envNumber("TENDI_PERF_TERTIARY_PROMPT_CRUD_MS", 40),
  tertiaryPromptCrudRssBytes: envNumber("TENDI_PERF_TERTIARY_PROMPT_CRUD_RSS_MIB", 24) * mib,
  tertiaryPromptCrudPayloadBytes: envNumber("TENDI_PERF_TERTIARY_PROMPT_CRUD_PAYLOAD_MIB", 1) * mib,
  tertiarySessionProjectsMs: envNumber("TENDI_PERF_TERTIARY_SESSION_PROJECTS_MS", 10),
  tertiarySessionProjectsRssBytes: envNumber("TENDI_PERF_TERTIARY_SESSION_PROJECTS_RSS_MIB", 24) * mib,
  tertiarySessionProjectsPayloadBytes: envNumber("TENDI_PERF_TERTIARY_SESSION_PROJECTS_PAYLOAD_MIB", 0.0625) * mib,
  tertiaryRuleSaveMs: envNumber("TENDI_PERF_TERTIARY_RULE_SAVE_MS", 40),
  tertiaryRuleSaveRssBytes: envNumber("TENDI_PERF_TERTIARY_RULE_SAVE_RSS_MIB", 16) * mib,
  tertiaryRuleSavePayloadBytes: envNumber("TENDI_PERF_TERTIARY_RULE_SAVE_PAYLOAD_MIB", 0.25) * mib,
  tertiarySettingsSaveMs: envNumber("TENDI_PERF_TERTIARY_SETTINGS_SAVE_MS", 3),
  tertiarySettingsSaveRssBytes: envNumber("TENDI_PERF_TERTIARY_SETTINGS_SAVE_RSS_MIB", 16) * mib,
  tertiarySettingsSavePayloadBytes: envNumber("TENDI_PERF_TERTIARY_SETTINGS_SAVE_PAYLOAD_MIB", 0.015625) * mib,
  // Production chart computations loaded through Vite SSR. Defaults leave headroom
  // above the current local p95 while remaining sensitive to a regression.
  chartOverviewTrendMs: envNumber("TENDI_PERF_CHART_OVERVIEW_TREND_MS", 70),
  chartOverviewTrendP95Ms: envNumber("TENDI_PERF_CHART_OVERVIEW_TREND_P95_MS", 120),
  chartOverviewTrendZoomedMs: envNumber("TENDI_PERF_CHART_OVERVIEW_TREND_ZOOMED_MS", 70),
  chartOverviewTrendZoomedP95Ms: envNumber("TENDI_PERF_CHART_OVERVIEW_TREND_ZOOMED_P95_MS", 120),
  chartSkillSessionProjectMs: envNumber("TENDI_PERF_CHART_SKILL_SESSION_PROJECT_MS", 15),
  chartSkillSessionProjectP95Ms: envNumber("TENDI_PERF_CHART_SKILL_SESSION_PROJECT_P95_MS", 30),
  chartRelationshipGraphMs: envNumber("TENDI_PERF_CHART_RELATIONSHIP_GRAPH_MS", 60),
  chartRelationshipGraphP95Ms: envNumber("TENDI_PERF_CHART_RELATIONSHIP_GRAPH_P95_MS", 100),
};

const results = [];
let baseline = null;
const baselinePath = options.baseline ?? (existsSync(defaultBaselinePath) ? defaultBaselinePath : null);
if (baselinePath) {
  baseline = JSON.parse(readFileSync(resolve(root, baselinePath), "utf8"));
}

console.log(`Tendi performance check (${options.profile})`);
console.log(`root: ${root}`);

if (!options.noBuild) {
  runRequired("build tendi-cli", "cargo", [
    "build",
    "--quiet",
    "-p",
    "tendi-cli",
    "--features",
    "perf-runner",
  ]);
}
if (!existsSync(binary)) failNow(`missing binary: ${binary}`);
if (!existsSync(perfBinary)) failNow(`missing binary: ${perfBinary}`);

benchmarkRepeated("skills-list", ["skills", "list", "--json"], thresholds.skillsMs);
benchmarkRepeated("hooks-list", ["hooks", "list", "--json"], thresholds.hooksMs);
benchmarkRepeated("rules-list", ["rules", "list", "--json"], thresholds.rulesMs);
benchmarkRepeated("mcp-list", ["mcp", "list", "--json"], thresholds.mcpMs);
benchmarkCoreScenario("primary-overview", {
  maxOperationMs: thresholds.primaryOverviewMs,
  maxRssBytes: thresholds.primaryOverviewRssBytes,
  maxPayloadBytes: thresholds.primaryOverviewPayloadBytes,
});
benchmarkCoreScenario("primary-prompts", {
  maxOperationMs: thresholds.primaryPromptsMs,
  maxRssBytes: thresholds.primaryPromptsRssBytes,
  maxPayloadBytes: thresholds.primaryPromptsPayloadBytes,
});
benchmarkCoreScenario("primary-config", {
  maxOperationMs: thresholds.primaryConfigMs,
  maxRssBytes: thresholds.primaryConfigRssBytes,
  maxPayloadBytes: thresholds.primaryConfigPayloadBytes,
});
benchmarkCoreScenario("primary-settings", {
  maxOperationMs: thresholds.primarySettingsMs,
  maxRssBytes: thresholds.primarySettingsRssBytes,
  maxPayloadBytes: thresholds.primarySettingsPayloadBytes,
});
benchmarkCoreScenario("secondary-session-page", {
  maxOperationMs: thresholds.secondarySessionPageMs,
  maxRssBytes: thresholds.secondarySessionPageRssBytes,
  maxPayloadBytes: thresholds.secondarySessionPagePayloadBytes,
});
benchmarkCoreScenario("secondary-linked-sessions", {
  maxOperationMs: thresholds.secondaryLinkedSessionsMs,
  maxRssBytes: thresholds.secondaryLinkedSessionsRssBytes,
  maxPayloadBytes: thresholds.secondaryLinkedSessionsPayloadBytes,
});
benchmarkCoreScenario("secondary-skill-files", {
  maxOperationMs: thresholds.secondarySkillFilesMs,
  maxRssBytes: thresholds.secondarySkillFilesRssBytes,
  maxPayloadBytes: thresholds.secondarySkillFilesPayloadBytes,
});
benchmarkCoreScenario("secondary-rule-detail", {
  maxOperationMs: thresholds.secondaryRuleDetailMs,
  maxRssBytes: thresholds.secondaryRuleDetailRssBytes,
  maxPayloadBytes: thresholds.secondaryRuleDetailPayloadBytes,
});
benchmarkCoreScenario("secondary-hook-detail", {
  maxOperationMs: thresholds.secondaryHookDetailMs,
  maxRssBytes: thresholds.secondaryHookDetailRssBytes,
  maxPayloadBytes: thresholds.secondaryHookDetailPayloadBytes,
});
benchmarkCoreScenario("secondary-config-read", {
  maxOperationMs: thresholds.secondaryConfigReadMs,
  maxRssBytes: thresholds.secondaryConfigReadRssBytes,
  maxPayloadBytes: thresholds.secondaryConfigReadPayloadBytes,
});
benchmarkCoreScenario("tertiary-skill-save", {
  maxOperationMs: thresholds.tertiarySkillSaveMs,
  maxRssBytes: thresholds.tertiarySkillSaveRssBytes,
  maxPayloadBytes: thresholds.tertiarySkillSavePayloadBytes,
});
benchmarkCoreScenario("tertiary-hook-delete", {
  maxOperationMs: thresholds.tertiaryHookDeleteMs,
  maxRssBytes: thresholds.tertiaryHookDeleteRssBytes,
  maxPayloadBytes: thresholds.tertiaryHookDeletePayloadBytes,
});
benchmarkCoreScenario("tertiary-prompt-crud", {
  maxOperationMs: thresholds.tertiaryPromptCrudMs,
  maxRssBytes: thresholds.tertiaryPromptCrudRssBytes,
  maxPayloadBytes: thresholds.tertiaryPromptCrudPayloadBytes,
});
benchmarkCoreScenario("tertiary-session-projects", {
  maxOperationMs: thresholds.tertiarySessionProjectsMs,
  maxRssBytes: thresholds.tertiarySessionProjectsRssBytes,
  maxPayloadBytes: thresholds.tertiarySessionProjectsPayloadBytes,
});
benchmarkCoreScenario("tertiary-rule-save", {
  maxOperationMs: thresholds.tertiaryRuleSaveMs,
  maxRssBytes: thresholds.tertiaryRuleSaveRssBytes,
  maxPayloadBytes: thresholds.tertiaryRuleSavePayloadBytes,
});
benchmarkCoreScenario("tertiary-settings-save", {
  maxOperationMs: thresholds.tertiarySettingsSaveMs,
  maxRssBytes: thresholds.tertiarySettingsSaveRssBytes,
  maxPayloadBytes: thresholds.tertiarySettingsSavePayloadBytes,
});
benchmarkFrontendScenario("chart-overview-trend", "overview-trend", {
  maxMedianMs: thresholds.chartOverviewTrendMs,
  maxP95Ms: thresholds.chartOverviewTrendP95Ms,
});
benchmarkFrontendScenario("chart-overview-trend-zoomed", "overview-trend-zoomed", {
  maxMedianMs: thresholds.chartOverviewTrendZoomedMs,
  maxP95Ms: thresholds.chartOverviewTrendZoomedP95Ms,
});
benchmarkFrontendScenario("chart-skill-session-project", "skill-session-project", {
  maxMedianMs: thresholds.chartSkillSessionProjectMs,
  maxP95Ms: thresholds.chartSkillSessionProjectP95Ms,
});
benchmarkFrontendScenario("chart-relationship-graph", "relationship-graph", {
  maxMedianMs: thresholds.chartRelationshipGraphMs,
  maxP95Ms: thresholds.chartRelationshipGraphP95Ms,
});

if (options.profile === "full") {
  const sessions = measureProcessRepeated(
    "sessions-list",
    ["sessions", "list", "--json"],
    3,
    true,
  );
  gate(
    "sessions-list",
    sessions.medianMs <= thresholds.sessionsMs
      && sessions.maxElapsedMs <= thresholds.sessionsMaxMs
      && sessions.peakRssBytes <= thresholds.sessionsRssBytes
      && sessions.outputBytes <= thresholds.sessionsOutputBytes,
    sessions,
    `median <= ${formatMs(thresholds.sessionsMs)}, max <= ${formatMs(thresholds.sessionsMaxMs)}, RSS <= ${formatBytes(thresholds.sessionsRssBytes)}, output <= ${formatBytes(thresholds.sessionsOutputBytes)}`,
  );

  const fixture = ensureSyntheticTranscript();
  const synthetic = measureProcess("synthetic-transcript", [
    "sessions",
    "transcript",
    fixture,
    "--agent",
    "codex",
    "--json",
  ]);
  gate(
    "synthetic-transcript",
    synthetic.elapsedMs <= thresholds.syntheticMs
      && synthetic.peakRssBytes <= thresholds.syntheticRssBytes,
    { ...synthetic, inputBytes: statSync(fixture).size },
    `<= ${formatMs(thresholds.syntheticMs)}, RSS <= ${formatBytes(thresholds.syntheticRssBytes)}`,
  );

  benchmarkLargestIndexedTranscript();
  if (options.appPid) await benchmarkIdleCpu(options.appPid);
  else skip("desktop-idle-cpu", "pass --app-pid <pid> while the app is idle");
}

const report = {
  schemaVersion: 2,
  createdAt: new Date().toISOString(),
  profile: options.profile,
  platform: `${process.platform}-${process.arch}`,
  thresholds,
  results,
};
mkdirSync(outputDir, { recursive: true });
writeFileSync(latestPath, `${JSON.stringify(report, null, 2)}\n`);
if (options.saveBaseline) {
  writeFileSync(defaultBaselinePath, `${JSON.stringify(report, null, 2)}\n`);
  console.log(`\nbaseline saved: ${defaultBaselinePath}`);
}

printSummary();
console.log(`result: ${latestPath}`);
if (results.some((result) => result.status === "fail")) process.exit(1);

function parseArgs(args) {
  const parsed = {
    profile: "full",
    noBuild: false,
    saveBaseline: false,
    baseline: null,
    appPid: null,
  };
  for (let index = 0; index < args.length; index += 1) {
    const arg = args[index];
    if (arg === "--profile") parsed.profile = args[++index];
    else if (arg === "--fast") parsed.profile = "fast";
    else if (arg === "--full") parsed.profile = "full";
    else if (arg === "--no-build") parsed.noBuild = true;
    else if (arg === "--save-baseline") parsed.saveBaseline = true;
    else if (arg === "--baseline") parsed.baseline = args[++index];
    else if (arg === "--app-pid") parsed.appPid = Number(args[++index]);
    else if (arg === "--help" || arg === "-h") {
      console.log(`Usage: node scripts/perf-check.mjs [options]

  --fast                    Run the pre-push checks
  --full                    Run local-data and memory checks (default)
  --no-build                Reuse target/debug/tendi and tendi-perf
  --app-pid <pid>           Include a 10-second idle CPU gate
  --save-baseline           Save target/perf/baseline.json
  --baseline <path>         Compare with a saved result

Thresholds can be overridden with TENDI_PERF_* environment variables.`);
      process.exit(0);
    } else failNow(`unknown argument: ${arg}`);
  }
  if (!new Set(["fast", "full"]).has(parsed.profile)) {
    failNow(`invalid profile: ${parsed.profile}`);
  }
  if (parsed.appPid !== null && (!Number.isInteger(parsed.appPid) || parsed.appPid <= 0)) {
    failNow("--app-pid must be a positive integer");
  }
  return parsed;
}

function envNumber(name, fallback) {
  const value = process.env[name];
  if (value === undefined) return fallback;
  const parsed = Number(value);
  if (!Number.isFinite(parsed) || parsed <= 0) failNow(`${name} must be a positive number`);
  return parsed;
}

function runRequired(label, command, args) {
  process.stdout.write(`\n==> ${label}\n`);
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit" });
  if (result.error) failNow(`${label}: ${result.error.message}`);
  if (result.status !== 0) failNow(`${label}: exit ${result.status}`);
}

function runBinary(args, captureStderr = false) {
  const started = performance.now();
  const result = spawnSync(binary, args, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "ignore", captureStderr ? "pipe" : "ignore"],
  });
  const elapsedMs = performance.now() - started;
  if (result.error) failNow(`${args.join(" ")}: ${result.error.message}`);
  if (result.status !== 0) {
    failNow(`${args.join(" ")}: exit ${result.status}\n${result.stderr ?? ""}`);
  }
  return { elapsedMs, stderr: result.stderr ?? "" };
}

function benchmarkRepeated(name, args, maxMedianMs) {
  runBinary(args);
  const samplesMs = [];
  for (let index = 0; index < 5; index += 1) samplesMs.push(runBinary(args).elapsedMs);
  samplesMs.sort((left, right) => left - right);
  const medianMs = samplesMs[Math.floor(samplesMs.length / 2)];
  gate(name, medianMs <= maxMedianMs, { medianMs, samplesMs }, `median <= ${formatMs(maxMedianMs)}`);
}

function benchmarkCoreScenario(name, limits) {
  const measured = measureExecutable(name, perfBinary, [name], true);
  let scenario;
  try {
    scenario = JSON.parse(measured.stdout.trim());
  } catch (error) {
    failNow(`${name}: invalid tendi-perf output: ${error.message}`);
  }
  for (const key of ["operationMs", "payloadBytes", "count"]) {
    if (!Number.isFinite(scenario[key]) || scenario[key] < 0) {
      failNow(`${name}: invalid ${key}: ${scenario[key]}`);
    }
  }
  const metrics = {
    operationMs: scenario.operationMs,
    processElapsedMs: measured.elapsedMs,
    peakRssBytes: measured.peakRssBytes,
    payloadBytes: scenario.payloadBytes,
    count: scenario.count,
  };
  gate(
    name,
    metrics.operationMs <= limits.maxOperationMs
      && metrics.peakRssBytes <= limits.maxRssBytes
      && metrics.payloadBytes <= limits.maxPayloadBytes,
    metrics,
    `operation <= ${formatMs(limits.maxOperationMs)}, RSS <= ${formatBytes(limits.maxRssBytes)}, payload <= ${formatBytes(limits.maxPayloadBytes)}`,
  );
}

function benchmarkFrontendScenario(name, scenario, limits) {
  if (!existsSync(chartPerformanceRunner)) failNow(`missing chart performance runner: ${chartPerformanceRunner}`);
  const measured = measureExecutable(
    name,
    process.execPath,
    [chartPerformanceRunner, scenario],
    true,
  );
  let scenarioResult;
  try {
    scenarioResult = JSON.parse(measured.stdout.trim());
  } catch (error) {
    failNow(`${name}: invalid chart performance output: ${error.message}`);
  }
  for (const key of ["operationMs", "p95Ms", "iterations", "warmupIterations", "checksum"]) {
    if (!Number.isFinite(scenarioResult[key]) || scenarioResult[key] < 0) {
      failNow(`${name}: invalid ${key}: ${scenarioResult[key]}`);
    }
  }
  if (scenarioResult.scenario !== scenario) {
    failNow(`${name}: expected scenario ${scenario}, got ${scenarioResult.scenario}`);
  }
  const metrics = {
    operationMs: scenarioResult.operationMs,
    p95Ms: scenarioResult.p95Ms,
    processElapsedMs: measured.elapsedMs,
    peakRssBytes: measured.peakRssBytes,
    outputBytes: measured.outputBytes,
    iterations: scenarioResult.iterations,
    warmupIterations: scenarioResult.warmupIterations,
    input: scenarioResult.input,
    output: scenarioResult.output,
    checksum: scenarioResult.checksum,
  };
  gate(
    name,
    metrics.operationMs <= limits.maxMedianMs && metrics.p95Ms <= limits.maxP95Ms,
    metrics,
    `median <= ${formatMs(limits.maxMedianMs)}, p95 <= ${formatMs(limits.maxP95Ms)}`,
  );
}

function measureProcess(name, args, captureOutput = false) {
  const measured = measureExecutable(name, binary, args, captureOutput);
  if (captureOutput) delete measured.stdout;
  return measured;
}

function measureExecutable(name, executable, args, captureOutput = false) {
  const time = process.platform === "darwin" ? "/usr/bin/time" : "/usr/bin/time";
  if (!existsSync(time)) {
    const started = performance.now();
    const plain = spawnSync(executable, args, {
      cwd: root,
      encoding: "utf8",
      maxBuffer: capturedOutputBufferBytes,
      stdio: ["ignore", captureOutput ? "pipe" : "ignore", "pipe"],
    });
    if (plain.error) failNow(`${name}: ${plain.error.message}`);
    if (plain.status !== 0) failNow(`${name}: exit ${plain.status}\n${plain.stderr ?? ""}`);
    return {
      elapsedMs: performance.now() - started,
      peakRssBytes: 0,
      rssUnavailable: true,
      ...(captureOutput
        ? { outputBytes: Buffer.byteLength(plain.stdout ?? ""), stdout: plain.stdout ?? "" }
        : {}),
    };
  }
  const timeArgs = process.platform === "darwin"
    ? ["-l", executable, ...args]
    : ["-v", executable, ...args];
  const started = performance.now();
  const result = spawnSync(time, timeArgs, {
    cwd: root,
    encoding: "utf8",
    maxBuffer: capturedOutputBufferBytes,
    stdio: ["ignore", captureOutput ? "pipe" : "ignore", "pipe"],
  });
  const elapsedMs = performance.now() - started;
  if (result.error) failNow(`${name}: ${result.error.message}`);
  if (result.status !== 0) failNow(`${name}: exit ${result.status}\n${result.stderr ?? ""}`);
  const peakRssBytes = parsePeakRss(result.stderr ?? "");
  if (!peakRssBytes) failNow(`${name}: could not parse peak RSS from /usr/bin/time`);
  const outputBytes = captureOutput ? Buffer.byteLength(result.stdout ?? "") : undefined;
  return {
    elapsedMs,
    peakRssBytes,
    ...(captureOutput ? { outputBytes, stdout: result.stdout ?? "" } : {}),
  };
}

function measureProcessRepeated(name, args, count, captureOutput = false) {
  const samples = Array.from(
    { length: count },
    () => measureProcess(name, args, captureOutput),
  );
  const elapsedSamplesMs = samples
    .map((sample) => sample.elapsedMs)
    .sort((left, right) => left - right);
  const peakRssSamplesBytes = samples.map((sample) => sample.peakRssBytes);
  return {
    medianMs: elapsedSamplesMs[Math.floor(elapsedSamplesMs.length / 2)],
    maxElapsedMs: Math.max(...elapsedSamplesMs),
    peakRssBytes: Math.max(...peakRssSamplesBytes),
    ...(captureOutput
      ? { outputBytes: Math.max(...samples.map((sample) => sample.outputBytes)) }
      : {}),
    elapsedSamplesMs,
    peakRssSamplesBytes,
  };
}

function parsePeakRss(stderr) {
  if (process.platform === "darwin") {
    const match = stderr.match(/^\s*(\d+)\s+maximum resident set size$/m);
    return match ? Number(match[1]) : 0;
  }
  const match = stderr.match(/Maximum resident set size \(kbytes\):\s*(\d+)/);
  return match ? Number(match[1]) * 1024 : 0;
}

function ensureSyntheticTranscript() {
  const fixtureDir = join(root, "target/perf-fixtures");
  const fixture = join(fixtureDir, "ignored-events-96m.jsonl");
  const targetBytes = 96 * mib;
  if (existsSync(fixture) && statSync(fixture).size >= targetBytes) return fixture;
  mkdirSync(fixtureDir, { recursive: true });
  const line = `${JSON.stringify({
    timestamp: "2026-08-13T00:00:00Z",
    type: "event_msg",
    payload: { type: "perf_fixture_ignored", body: "x".repeat(128 * 1024) },
  })}\n`;
  const descriptor = openSync(fixture, "w");
  try {
    let written = 0;
    while (written < targetBytes) written += writeSync(descriptor, line);
  } finally {
    closeSync(descriptor);
  }
  return fixture;
}

function benchmarkLargestIndexedTranscript() {
  const db = process.platform === "darwin"
    ? join(homedir(), "Library/Application Support/tendi/tendi.sqlite3")
    : join(homedir(), ".local/share/tendi/tendi.sqlite3");
  if (!existsSync(db)) {
    skip("largest-transcript", `database not found: ${db}`);
    return;
  }
  const query = [
    "SELECT json_object(",
    "  'path', session_path, 'agent', agent, 'size', file_size",
    ") FROM session_search_index",
    "ORDER BY file_size DESC LIMIT 1;",
  ].join(" ");
  const row = spawnSync("sqlite3", [db, query], { encoding: "utf8" });
  if (row.status !== 0 || !row.stdout.trim()) {
    skip("largest-transcript", "no indexed transcript row");
    return;
  }
  const selected = JSON.parse(row.stdout.trim());
  if (!existsSync(selected.path)) {
    skip("largest-transcript", `missing transcript: ${selected.path}`);
    return;
  }
  if (selected.size < 64 * mib) {
    skip("largest-transcript", `largest transcript is only ${formatBytes(selected.size)}`);
    return;
  }
  const measured = measureProcess("largest-transcript", [
    "sessions",
    "transcript",
    selected.path,
    "--agent",
    selected.agent,
    "--json",
  ]);
  const rssLimit = Math.max(
    thresholds.transcriptRssFloorBytes,
    selected.size * thresholds.transcriptRssRatio,
  );
  gate(
    "largest-transcript",
    measured.elapsedMs <= thresholds.transcriptMs && measured.peakRssBytes <= rssLimit,
    { ...measured, inputBytes: selected.size, inputPath: selected.path },
    `<= ${formatMs(thresholds.transcriptMs)}, RSS <= max(${formatBytes(thresholds.transcriptRssFloorBytes)}, input × ${thresholds.transcriptRssRatio})`,
  );
}

async function benchmarkIdleCpu(pid) {
  const samples = [];
  for (let index = 0; index < 10; index += 1) {
    const result = spawnSync("ps", ["-p", String(pid), "-o", "%cpu="], { encoding: "utf8" });
    if (result.status !== 0 || !result.stdout.trim()) {
      failNow(`desktop-idle-cpu: process ${pid} is not running`);
    }
    samples.push(Number(result.stdout.trim()));
    if (index < 9) await new Promise((resolveDelay) => setTimeout(resolveDelay, 1_000));
  }
  const averageCpu = samples.reduce((sum, value) => sum + value, 0) / samples.length;
  const maxCpu = Math.max(...samples);
  gate(
    "desktop-idle-cpu",
    averageCpu <= thresholds.idleCpuAverage && maxCpu <= thresholds.idleCpuMax,
    { averageCpu, maxCpu, samples, pid },
    `average <= ${thresholds.idleCpuAverage}%, max <= ${thresholds.idleCpuMax}%`,
  );
}

function gate(name, passed, metrics, threshold) {
  const result = { name, status: passed ? "pass" : "fail", threshold, ...metrics };
  const previous = baseline?.results?.find((item) => item.name === name);
  if (previous) result.baseline = comparableMetrics(previous, result);
  results.push(result);
  console.log(`${passed ? "PASS" : "FAIL"} ${name}: ${formatMetrics(result)} (${threshold})`);
}

function skip(name, reason) {
  results.push({ name, status: "skip", reason });
  console.log(`SKIP ${name}: ${reason}`);
}

function comparableMetrics(previous, current) {
  const comparison = {};
  for (const key of [
    "medianMs",
    "maxElapsedMs",
    "elapsedMs",
    "operationMs",
    "p95Ms",
    "processElapsedMs",
    "peakRssBytes",
    "outputBytes",
    "payloadBytes",
    "averageCpu",
    "maxCpu",
  ]) {
    if (Number.isFinite(previous[key]) && Number.isFinite(current[key]) && previous[key] !== 0) {
      comparison[key] = {
        value: previous[key],
        changePercent: ((current[key] - previous[key]) / previous[key]) * 100,
      };
    }
  }
  return comparison;
}

function formatMetrics(result) {
  const values = [];
  if (Number.isFinite(result.medianMs)) values.push(`median ${formatMs(result.medianMs)}`);
  if (Number.isFinite(result.maxElapsedMs)) values.push(`elapsed max ${formatMs(result.maxElapsedMs)}`);
  if (Number.isFinite(result.elapsedMs)) values.push(formatMs(result.elapsedMs));
  if (Number.isFinite(result.operationMs)) values.push(`operation ${formatMs(result.operationMs)}`);
  if (Number.isFinite(result.p95Ms)) values.push(`p95 ${formatMs(result.p95Ms)}`);
  if (Number.isFinite(result.processElapsedMs)) values.push(`process ${formatMs(result.processElapsedMs)}`);
  if (Number.isFinite(result.peakRssBytes)) values.push(`RSS ${formatBytes(result.peakRssBytes)}`);
  if (Number.isFinite(result.outputBytes)) values.push(`output ${formatBytes(result.outputBytes)}`);
  if (Number.isFinite(result.payloadBytes)) values.push(`payload ${formatBytes(result.payloadBytes)}`);
  if (Number.isFinite(result.averageCpu)) values.push(`CPU avg ${result.averageCpu.toFixed(1)}%`);
  if (Number.isFinite(result.maxCpu)) values.push(`max ${result.maxCpu.toFixed(1)}%`);
  const baselineChanges = Object.entries(result.baseline ?? {})
    .map(([key, value]) => `${key} ${signed(value.changePercent)} vs baseline`);
  return [...values, ...baselineChanges].join(", ");
}

function printSummary() {
  console.log("\nSummary");
  for (const result of results) {
    console.log(`  ${result.status.toUpperCase().padEnd(4)} ${result.name}`);
  }
}

function formatMs(value) {
  return `${Math.round(value)} ms`;
}

function formatBytes(value) {
  return `${(value / mib).toFixed(1)} MiB`;
}

function signed(value) {
  return `${value >= 0 ? "+" : ""}${value.toFixed(1)}%`;
}

function failNow(message) {
  console.error(`ERROR ${message}`);
  process.exit(2);
}
