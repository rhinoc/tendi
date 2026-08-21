import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { existsSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { writeStdout } from "../apps/desktop/scripts/stdio.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const desktop = join(root, "apps/desktop");
const captureMaxBuffer = 64 * 1024 * 1024;

function run(label, command, args, options = {}) {
  process.stdout.write(`\n==> ${label}\n`);
  const output = execFileSync(command, args, {
    cwd: options.cwd ?? root,
    encoding: "utf8",
    stdio: options.capture ? ["ignore", "pipe", "pipe"] : "inherit",
    env: { ...process.env, ...options.env },
  });
  return output ?? "";
}

function runText(label, command, args, options = {}) {
  process.stdout.write(`\n==> ${label}\n`);
  const output = execFileSync(command, args, {
    cwd: options.cwd ?? root,
    encoding: "utf8",
    stdio: ["ignore", "pipe", "pipe"],
    env: { ...process.env, ...options.env },
  });
  process.stdout.write(output);
  return output;
}

function tendi(args) {
  return runText(`tendi ${args.join(" ")}`, "cargo", [
    "run",
    "-p",
    "tendi-cli",
    "--",
    ...args,
  ]);
}

function tendiCapture(args) {
  process.stdout.write(`\n==> tendi ${args.join(" ")}\n`);
  return execFileSync("cargo", ["run", "-p", "tendi-cli", "--", ...args], {
    cwd: root,
    encoding: "utf8",
    maxBuffer: captureMaxBuffer,
    stdio: ["ignore", "pipe", "pipe"],
  });
}

function sqliteCount(table) {
  const db = join(
    process.env.HOME,
    "Library/Application Support/tendi/tendi.sqlite3",
  );
  assert.ok(existsSync(db), `missing sqlite database at ${db}`);
  return Number(
    execFileSync("sqlite3", [db, `select count(*) from ${table};`], {
      encoding: "utf8",
    }).trim(),
  );
}

run("cargo test -p tendi-core", "cargo", ["test", "-p", "tendi-core"]);
run("cargo check", "cargo", ["check"]);
run("node scripts/perf-check.mjs --fast", "node", ["scripts/perf-check.mjs", "--fast"]);

const scanSummary = tendi(["scan"]);
for (const label of ["agents", "skills", "sessions", "rules", "hooks", "mcp servers", "roots", "db"]) {
  assert.match(scanSummary, new RegExp(`^${label}: .+`, "m"), `plain scan missing ${label}`);
}

const scan = JSON.parse(tendiCapture(["scan", "--json"]));
const plainCount = (label) => {
  const match = scanSummary.match(new RegExp(`^${label}: (\\d+)$`, "m"));
  assert.ok(match, `plain scan missing numeric ${label}`);
  return Number(match[1]);
};
const agentKinds = new Set(scan.agents.agents.map((agent) => agent.kind));
for (const agent of ["codex", "cursor", "claude"]) {
  assert.ok(agentKinds.has(agent), `scan missing ${agent} agent`);
  assert.ok(
    scan.agents.agents.find((item) => item.kind === agent)?.installed,
    `scan found ${agent} but did not mark it installed`,
  );
}
assert.ok(scan.skills.skills.some((skill) => skill.name === "lark-im"), "missing lark-im skill");
assert.ok(scan.skills.skills.some((skill) => skill.name === "lark-doc"), "missing lark-doc skill");
assert.ok(scan.sessions.sessions.length > 0, "expected sessions");
assert.ok(scan.rules.rules.length > 0, "expected rules");
assert.ok(scan.hooks.hooks.length > 0, "expected hooks");
assert.ok(scan.mcp.servers.length > 0, "expected mcp servers");
assert.equal(plainCount("agents"), scan.agents.agents.length, "plain scan agents count mismatch");
assert.equal(plainCount("skills"), scan.skills.skills.length, "plain scan skills count mismatch");
assert.equal(plainCount("sessions"), scan.sessions.sessions.length, "plain scan sessions count mismatch");
assert.equal(plainCount("rules"), scan.rules.rules.length, "plain scan rules count mismatch");
assert.equal(plainCount("hooks"), scan.hooks.hooks.length, "plain scan hooks count mismatch");
assert.equal(plainCount("mcp servers"), scan.mcp.servers.length, "plain scan mcp count mismatch");
writeStdout(
  [
    `agents=${scan.agents.agents.length}`,
    `skills=${scan.skills.skills.length}`,
    `sessions=${scan.sessions.sessions.length}`,
    `rules=${scan.rules.rules.length}`,
    `hooks=${scan.hooks.hooks.length}`,
    `mcp=${scan.mcp.servers.length}`,
  ].join(" "),
);

assert.equal(sqliteCount("skills"), scan.skills.skills.length, "sqlite skills count mismatch");
assert.equal(sqliteCount("agents"), scan.agents.agents.length, "sqlite agents count mismatch");
assert.equal(sqliteCount("sessions"), scan.sessions.sessions.length, "sqlite sessions count mismatch");
assert.equal(sqliteCount("rules"), scan.rules.rules.length, "sqlite rules count mismatch");
assert.equal(sqliteCount("hooks"), scan.hooks.hooks.length, "sqlite hooks count mismatch");
assert.equal(sqliteCount("mcp_servers"), scan.mcp.servers.length, "sqlite mcp count mismatch");

const skillsList = tendi(["skills", "list"]);
assert.match(skillsList, /lark-im/);
assert.match(skillsList, /manual/);

tendi(["skills", "set", "lark-*", "--visibility", "manual", "--dry-run"]);
const wrapPreview = tendi(["skills", "wrap", "lark", "--from", "lark-*", "--dry-run"]);
assert.match(wrapPreview, /lark\/SKILL\.md/);

run("node scripts/cli-write-smoke.mjs", "node", ["scripts/cli-write-smoke.mjs"]);
run("npm run e2e:align", "npm", ["run", "e2e:align"], { cwd: desktop });
run("npm run build", "npm", ["run", "build"], { cwd: desktop });
run("npm run build:tauri", "npm", ["run", "build:tauri"], { cwd: desktop });

writeStdout("\nacceptance ok");
