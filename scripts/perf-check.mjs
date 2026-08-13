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
const outputDir = join(root, "target/perf");
const defaultBaselinePath = join(outputDir, "baseline.json");
const latestPath = join(outputDir, "latest.json");
const mib = 1024 * 1024;

const options = parseArgs(process.argv.slice(2));
const thresholds = {
  skillsMs: envNumber("TENDI_PERF_SKILLS_MS", 1_500),
  hooksMs: envNumber("TENDI_PERF_HOOKS_MS", 500),
  sessionsMs: envNumber("TENDI_PERF_SESSIONS_MS", 12_000),
  sessionsRssBytes: envNumber("TENDI_PERF_SESSIONS_RSS_MIB", 192) * mib,
  syntheticMs: envNumber("TENDI_PERF_SYNTHETIC_MS", 5_000),
  syntheticRssBytes: envNumber("TENDI_PERF_SYNTHETIC_RSS_MIB", 96) * mib,
  transcriptRssRatio: envNumber("TENDI_PERF_TRANSCRIPT_RSS_RATIO", 0.65),
  transcriptRssFloorBytes: envNumber("TENDI_PERF_TRANSCRIPT_RSS_FLOOR_MIB", 128) * mib,
  idleCpuAverage: envNumber("TENDI_PERF_IDLE_CPU_AVG", 5),
  idleCpuMax: envNumber("TENDI_PERF_IDLE_CPU_MAX", 25),
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
  runRequired("build tendi-cli", "cargo", ["build", "--quiet", "-p", "tendi-cli"]);
}
if (!existsSync(binary)) failNow(`missing binary: ${binary}`);

benchmarkRepeated("skills-list", ["skills", "list", "--json"], thresholds.skillsMs);
benchmarkRepeated("hooks-list", ["hooks", "list", "--json"], thresholds.hooksMs);

if (options.profile === "full") {
  const sessions = measureProcess("sessions-list", ["sessions", "list", "--json"]);
  gate(
    "sessions-list",
    sessions.elapsedMs <= thresholds.sessionsMs
      && sessions.peakRssBytes <= thresholds.sessionsRssBytes,
    sessions,
    `<= ${formatMs(thresholds.sessionsMs)}, RSS <= ${formatBytes(thresholds.sessionsRssBytes)}`,
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
  schemaVersion: 1,
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
  --no-build                Reuse target/debug/tendi
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

function measureProcess(name, args) {
  const time = process.platform === "darwin" ? "/usr/bin/time" : "/usr/bin/time";
  if (!existsSync(time)) {
    const plain = runBinary(args);
    return { elapsedMs: plain.elapsedMs, peakRssBytes: 0, rssUnavailable: true };
  }
  const timeArgs = process.platform === "darwin"
    ? ["-l", binary, ...args]
    : ["-v", binary, ...args];
  const started = performance.now();
  const result = spawnSync(time, timeArgs, {
    cwd: root,
    encoding: "utf8",
    stdio: ["ignore", "ignore", "pipe"],
  });
  const elapsedMs = performance.now() - started;
  if (result.error) failNow(`${name}: ${result.error.message}`);
  if (result.status !== 0) failNow(`${name}: exit ${result.status}\n${result.stderr ?? ""}`);
  const peakRssBytes = parsePeakRss(result.stderr ?? "");
  if (!peakRssBytes) failNow(`${name}: could not parse peak RSS from /usr/bin/time`);
  return { elapsedMs, peakRssBytes };
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
    measured.peakRssBytes <= rssLimit,
    { ...measured, inputBytes: selected.size, inputPath: selected.path },
    `RSS <= max(${formatBytes(thresholds.transcriptRssFloorBytes)}, input × ${thresholds.transcriptRssRatio})`,
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
  for (const key of ["medianMs", "elapsedMs", "peakRssBytes", "averageCpu", "maxCpu"]) {
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
  if (Number.isFinite(result.elapsedMs)) values.push(formatMs(result.elapsedMs));
  if (Number.isFinite(result.peakRssBytes)) values.push(`RSS ${formatBytes(result.peakRssBytes)}`);
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
