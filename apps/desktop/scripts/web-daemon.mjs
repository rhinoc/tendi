import { execFile, spawnSync } from "node:child_process";
import { existsSync, statSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);
const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopDir = resolve(scriptDir, "..");
const repoDir = resolve(desktopDir, "../..");
const port = Number(process.env.TENDI_WEB_BRIDGE_PORT || 5188);
const tendiBin = resolve(process.env.TENDI_BIN || join(repoDir, "target/debug/tendi"));
const commandCwd = resolve(process.env.TENDI_CWD || desktopDir);
const maxBuffer = 512 * 1024 * 1024;

let reportCache = null;
let reportCacheExpiresAt = 0;
let reportRequest = null;
let settings = {
  appearance: "system",
  lightTheme: "default",
  darkTheme: "default",
  developerMode: false,
  additionalSessionRoots: [],
  configProfiles: {},
};
const transcriptCache = new Map();
const skillAddPreviews = new Map();
let skillAddPreviewSequence = 0;

if (!existsSync(tendiBin)) {
  console.log(`[tendi] building local CLI at ${tendiBin}`);
  const result = spawnSync("cargo", ["build", "-p", "tendi-cli"], {
    cwd: repoDir,
    stdio: "inherit",
  });
  if (result.status !== 0 || !existsSync(tendiBin)) {
    throw new Error(`Tendi CLI build failed at ${tendiBin}. Set TENDI_BIN to an existing binary.`);
  }
}

async function runCli(args) {
  const { stdout } = await execFileAsync(tendiBin, args, {
    cwd: commandCwd,
    encoding: "utf8",
    maxBuffer,
    env: process.env,
  });
  return JSON.parse(stdout);
}

async function loadReport(refresh = false) {
  if (refresh) {
    reportCache = null;
    reportCacheExpiresAt = 0;
  }
  if (reportCache && reportCacheExpiresAt > Date.now()) return reportCache;
  if (!reportRequest) {
    reportRequest = runCli(["scan", "--json"])
      .then((report) => {
        reportCache = report;
        reportCacheExpiresAt = Date.now() + 1_000;
        return report;
      })
      .finally(() => {
        reportRequest = null;
      });
  }
  return reportRequest;
}

function reportRows(report, section, key = section) {
  const value = report?.[section];
  if (Array.isArray(value)) return value;
  return Array.isArray(value?.[key]) ? value[key] : [];
}

function sessionDate(session) {
  const value = session.started_at || session.startedAt || session.updated_at || session.updatedAt;
  const date = typeof value === "string" && value.length >= 10 ? value.slice(0, 10) : "";
  return /^\d{4}-\d{2}-\d{2}$/.test(date) ? date : null;
}

function buildAnalytics(report) {
  const sessions = reportRows(report, "sessions");
  const days = new Map();
  const emptyUsage = () => ({
    inputTokens: 0,
    cachedInputTokens: 0,
    cacheWriteInputTokens: 0,
    outputTokens: 0,
    reasoningOutputTokens: 0,
    totalTokens: 0,
  });
  for (const session of sessions) {
    const date = sessionDate(session);
    if (!date) continue;
    const day = days.get(date) ?? {
      date,
      usage: emptyUsage(),
      responses: 0,
      sessions: 0,
      sessionsByAgent: {},
      runs: { started: 0, completed: 0, unclosed: 0, totalMs: 0, maxMs: 0 },
      aborted: 0,
      compacted: 0,
      models: [],
      tools: [],
      skills: [],
      rateLimits: {},
    };
    const usage = session.token_usage || session.tokenUsage || {};
    const readUsage = (camel, snake) => Number(usage[camel] ?? usage[snake] ?? 0);
    const agent = `${session.agent || "unknown"}`;
    const model = `${session.model || "unknown"}`;
    const messages = Number(session.message_count ?? session.messages ?? 0);
    day.sessions += 1;
    day.sessionsByAgent[agent] = (day.sessionsByAgent[agent] ?? 0) + 1;
    day.responses += messages;
    day.runs.started += 1;
    day.runs.completed += 1;
    for (const [camel, snake] of [
      ["inputTokens", "input_tokens"],
      ["cachedInputTokens", "cached_input_tokens"],
      ["cacheWriteInputTokens", "cache_write_input_tokens"],
      ["outputTokens", "output_tokens"],
      ["reasoningOutputTokens", "reasoning_output_tokens"],
      ["totalTokens", "total_tokens"],
    ]) {
      day.usage[camel] += readUsage(camel, snake);
    }
    const modelEntry = day.models.find((entry) => entry.model === model);
    if (modelEntry) modelEntry.totalTokens += readUsage("totalTokens", "total_tokens");
    else day.models.push({ model, totalTokens: readUsage("totalTokens", "total_tokens") });
    days.set(date, day);
  }
  const sortedDays = [...days.values()].sort((left, right) => left.date.localeCompare(right.date));
  const summary = {
    usage: emptyUsage(),
    responses: 0,
    sessions: 0,
    runs: { started: 0, completed: 0, unclosed: 0, totalMs: 0, maxMs: 0 },
    aborted: 0,
    abortedRate: 0,
    compacted: 0,
    compactedSessions: 0,
  };
  for (const day of sortedDays) {
    for (const key of Object.keys(summary.usage)) summary.usage[key] += day.usage[key];
    summary.responses += day.responses;
    summary.sessions += day.sessions;
    summary.runs.started += day.runs.started;
    summary.runs.completed += day.runs.completed;
    summary.runs.unclosed += day.runs.unclosed;
    summary.runs.totalMs += day.runs.totalMs;
    summary.runs.maxMs = Math.max(summary.runs.maxMs, day.runs.maxMs);
    summary.aborted += day.aborted;
    summary.compacted += day.compacted;
  }
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
    capabilities: [{ agent: "local-cli", tokenUsage: true, reasoningTokens: true, explicitRuns: true, rateLimitHistory: false }],
    summary,
    days: sortedDays,
    tools: [],
    skills: [],
    warnings: [],
  };
}

function normalizeAgent(agent) {
  const value = `${agent || "codex"}`.toLowerCase();
  return ["codex", "cursor", "claude", "shared"].includes(value) ? value : "codex";
}

async function loadTranscript(path, agent) {
  const file = statSync(path);
  const key = `${path}\0${agent}`;
  const sourceVersion = `${file.size}:${Math.floor(file.mtimeMs)}`;
  const cached = transcriptCache.get(key);
  if (cached?.sourceVersion === sourceVersion) return cached;
  const items = await runCli(["sessions", "transcript", path, "--agent", normalizeAgent(agent), "--json"]);
  const result = { items: Array.isArray(items) ? items : [], sourceVersion };
  transcriptCache.set(key, result);
  return result;
}

function skillAddCliArgs(args, mode) {
  const source = `${args.source || ""}`.trim();
  if (!source) throw new Error("skill source must not be empty");
  const cliArgs = [
    "skills",
    "add",
    source,
    "--to",
    `${args.target || "shared"}`,
    "--scope",
    `${args.scope || "global"}`,
    "--visibility",
    `${args.visibility || "auto"}`,
    "--json",
  ];
  if (args.copy) cliArgs.push("--copy");
  if (args.overwrite) cliArgs.push("--overwrite");
  for (const skill of Array.isArray(args.skills) ? args.skills : []) {
    cliArgs.push("--skill", `${skill}`);
  }
  if (mode === "preview") cliArgs.push("--dry-run");
  else cliArgs.push("--yes");
  return cliArgs;
}

function getSkillAddPreview(args) {
  const previewId = `${args.previewId || ""}`;
  const preview = skillAddPreviews.get(previewId);
  if (!preview
    || preview.source !== `${args.source || ""}`
    || preview.target !== `${args.target || ""}`
    || preview.scope !== `${args.scope || ""}`
    || preview.copy !== Boolean(args.copy)
    || preview.visibility !== `${args.visibility || ""}`) {
    throw new Error("skill add options changed; preview the installation again");
  }
  return { previewId, preview };
}

function transcriptItemType(item) {
  return `${item.type || item.kind || "assistant"}`;
}

function transcriptItemText(item) {
  return [item.body, item.tag, item.command, item.result, item.model, item.effort, item.callId]
    .map((value) => `${value || ""}`)
    .join("\n")
    .toLowerCase();
}

function transcriptScope(type) {
  if (["user", "notification"].includes(type)) return "user";
  if (["context", "compaction", "model_config"].includes(type)) return "system";
  if (["tool", "toolGroup"].includes(type)) return "tool";
  return "assistant";
}

async function invokeCommand(command, args = {}) {
  if (command === "scan") return loadReport(true);
  if (command === "settings_get") return settings;
  if (command === "settings_save") {
    settings = { ...settings, ...args };
    return settings;
  }
  if (command === "agents_list") return reportRows(await loadReport(), "agents", "agents");
  if (command === "skills_list") return reportRows(await loadReport(), "skills", "skills");
  if (command === "skills_refresh") return { skills: reportRows(await loadReport(true), "skills", "skills"), updateCheck: "not-started" };
  if (command === "skills_targets") return runCli(["skills", "targets", "--json"]);
  if (command === "skills_add") {
    if (args.dryRun) {
      const plan = await runCli(skillAddCliArgs(args, "preview"));
      const previewId = `web-add-${skillAddPreviewSequence++}`;
      skillAddPreviews.set(previewId, {
        source: `${args.source || ""}`,
        target: `${args.target || ""}`,
        scope: `${args.scope || ""}`,
        copy: Boolean(args.copy),
        visibility: `${args.visibility || ""}`,
        plan,
      });
      return { applied: false, plan, previewId };
    }
    const { previewId } = getSkillAddPreview(args);
    const result = await runCli(skillAddCliArgs(args, "install"));
    skillAddPreviews.delete(previewId);
    return result;
  }
  if (command === "skills_add_preview_read") {
    const previewId = `${args.previewId || ""}`;
    const preview = skillAddPreviews.get(previewId);
    if (!preview) throw new Error("skill add preview expired; preview the installation again");
    const skill = preview.plan?.available?.find((item) => item.name === args.skillName);
    if (!skill?.path) throw new Error(`skill ${JSON.stringify(args.skillName)} is not in the current preview`);
    return {
      name: skill.name,
      relativePath: "SKILL.md",
      content: await readFile(join(skill.path, "SKILL.md"), "utf8"),
    };
  }
  if (command === "sessions_list") return reportRows(await loadReport(), "sessions", "sessions");
  if (command === "sessions_scan_start") {
    await loadReport(true);
    return null;
  }
  if (command === "sessions_search") return runCli(["sessions", "search", `${args.query || ""}`, "--json"]);
  if (command === "rules_list") return reportRows(await loadReport(), "rules", "rules");
  if (command === "hooks_list") return reportRows(await loadReport(), "hooks", "hooks");
  if (command === "mcp_list") return reportRows(await loadReport(), "mcp", "servers");
  if (command === "prompts_list") return [];
  if (command === "analytics_overview") return buildAnalytics(await loadReport());
  if (command === "analytics_revision") return 1;
  if (command === "session_skill_index_status") return { indexed: 0, failed: 0, running: false };
  if (command === "session_skill_index_run") return { indexed: 0, failed: 0, running: false };
  if (command === "session_skill_links" || command === "skill_session_links") return [];
  if (command === "skills_updates") return { updateCheck: "not-started" };
  if (command === "session_transcript") {
    const transcript = await loadTranscript(`${args.path || ""}`, args.agent);
    const limit = Math.max(1, Math.min(1_000, Number(args.limit) || 160));
    const pageIndex = typeof args.cursor === "string" && args.cursor.startsWith("web:")
      ? Number(args.cursor.slice(4)) || 0
      : 0;
    const start = pageIndex * limit;
    const items = transcript.items.slice(start, start + limit);
    const done = start + limit >= transcript.items.length;
    return {
      items: args.knownSourceVersion === transcript.sourceVersion ? [] : items,
      warnings: [],
      nextCursor: done ? null : `web:${pageIndex + 1}`,
      done,
      sourceVersion: transcript.sourceVersion,
      restartRequired: false,
      unchanged: args.knownSourceVersion === transcript.sourceVersion,
    };
  }
  if (command === "session_transcript_search") {
    const transcript = await loadTranscript(`${args.path || ""}`, args.agent);
    const query = `${args.query || ""}`.trim().toLowerCase();
    const scopes = args.scopes || { user: true, assistant: true, system: false, tool: false };
    const hits = [];
    let groupIndex = 0;
    for (let index = 0; index < transcript.items.length;) {
      const firstType = transcriptItemType(transcript.items[index]);
      if (firstType === "tool") {
        let toolIndex = 0;
        while (index < transcript.items.length && transcriptItemType(transcript.items[index]) === "tool") {
          if (scopes.tool && transcriptItemText(transcript.items[index]).includes(query)) hits.push({ groupIndex, toolIndex });
          index += 1;
          toolIndex += 1;
        }
        groupIndex += 1;
        continue;
      }
      if (scopes[transcriptScope(firstType)] && transcriptItemText(transcript.items[index]).includes(query)) hits.push({ groupIndex });
      index += 1;
      groupIndex += 1;
    }
    return { hits, warnings: [], sourceVersion: transcript.sourceVersion };
  }
  return null;
}

function readBody(request) {
  return new Promise((resolveBody, reject) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => resolveBody(Buffer.concat(chunks).toString("utf8")));
    request.on("error", reject);
  });
}

function sendJson(response, status, value) {
  const body = JSON.stringify(value);
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "access-control-allow-origin": "*",
  });
  response.end(body);
}

const server = createServer(async (request, response) => {
  if (request.method === "OPTIONS") {
    response.writeHead(204, { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" });
    response.end();
    return;
  }
  if (request.method === "GET" && request.url === "/health") {
    sendJson(response, 200, { ok: true, pid: process.pid, cwd: commandCwd });
    return;
  }
  if (request.method !== "POST" || request.url !== "/__tendi/invoke") {
    sendJson(response, 404, { ok: false, error: "Not found" });
    return;
  }
  try {
    const body = JSON.parse(await readBody(request));
    const result = await invokeCommand(body.command, body.args || {});
    sendJson(response, 200, { ok: true, result });
  } catch (error) {
    sendJson(response, 500, { ok: false, error: error instanceof Error ? error.message : `${error}` });
  }
});

server.listen(port, "127.0.0.1", () => {
  console.log(`[tendi] web data bridge listening on http://127.0.0.1:${port}`);
  console.log(`[tendi] web data bridge cwd=${commandCwd}`);
});

function close() {
  server.close(() => process.exit(0));
}

process.once("SIGINT", close);
process.once("SIGTERM", close);
