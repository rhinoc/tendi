import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { existsSync, lstatSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { writeStdout } from "../apps/desktop/scripts/stdio.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const temp = mkdtempSync(join(tmpdir(), "tendi-cli-write-"));
const home = join(temp, "home");
const project = join(temp, "project");
const skillDir = join(project, ".agents/skills/demo");
const skillFile = join(skillDir, "SKILL.md");
const policyFile = join(skillDir, "agents/openai.yaml");
const linkSourceDir = join(temp, "local-skills/link-demo");
const linkTargetDir = join(home, ".codex/skills/link-demo");
const addSourceDir = join(temp, "add-source/skills/add-demo");
const addTargetDir = join(home, ".agents/skills/add-demo");
const bundledSkillDir = join(home, ".agents/skills/tendi");

function runTendi(args, options = {}) {
  const command = process.env.TENDI_TEST_BIN || "cargo";
  const commandArgs = process.env.TENDI_TEST_BIN
    ? args
    : ["run", "--quiet", "--manifest-path", join(root, "Cargo.toml"), "-p", "tendi-cli", "--", ...args];
  const result = spawnSync(
    command,
    commandArgs,
    {
      cwd: project,
      encoding: "utf8",
      input: options.input ?? "",
      env: { ...process.env, HOME: home },
      stdio: ["pipe", "pipe", "pipe"],
    },
  );
  const output = `${result.stdout ?? ""}${result.stderr ?? ""}`;
  if (result.status !== 0) {
    throw new Error(`tendi ${args.join(" ")} failed\n${output}`);
  }
  return output;
}

function parseJsonOutput(output) {
  const start = output.indexOf("{");
  assert.notEqual(start, -1, `missing JSON output:\n${output}`);
  return JSON.parse(output.slice(start));
}

try {
  mkdirSync(project, { recursive: true });
  const guide = runTendi(["skills", "guide"]);
  assert.match(guide, /# Tendi CLI/);
  assert.match(guide, /Recall a session/);

  const bundledDryRun = parseJsonOutput(runTendi(["setup", "skills", "--dry-run", "--json"]));
  assert.equal(bundledDryRun.name, "tendi");
  assert.equal(bundledDryRun.action, "install");
  assert.equal(existsSync(bundledSkillDir), false, "bundled skill dry-run created target");

  const bundledInstall = parseJsonOutput(runTendi(["setup", "skills", "--yes", "--json"]));
  assert.equal(bundledInstall.status.current, true);
  assert.equal(bundledInstall.applied, true);
  assert.match(readFileSync(join(bundledSkillDir, "SKILL.md"), "utf8"), /TENDI skills guide/);

  mkdirSync(join(skillDir, "agents"), { recursive: true });
  writeFileSync(
    skillFile,
    [
      "---",
      "name: demo",
      "description: Temporary smoke skill",
      "---",
      "",
      "# Demo",
      "",
      "Do a tiny local smoke.",
      "",
    ].join("\n"),
  );

  const original = readFileSync(skillFile, "utf8");

  const abortOutput = runTendi(["skills", "set", "demo", "--visibility", "manual"], { input: "n\n" });
  assert.match(abortOutput, /Apply these changes\? \[y\/N\]/);
  assert.match(abortOutput, /aborted/);
  assert.equal(readFileSync(skillFile, "utf8"), original, "aborted write changed SKILL.md");

  const dryRunOutput = runTendi(["skills", "set", "demo", "--visibility", "manual", "--dry-run"]);
  assert.match(dryRunOutput, /SKILL\.md/);
  assert.equal(readFileSync(skillFile, "utf8"), original, "dry-run changed SKILL.md");

  const applyOutput = runTendi(["skills", "set", "demo", "--visibility", "manual", "--yes"]);
  assert.match(applyOutput, /applied/);
  const updated = readFileSync(skillFile, "utf8");
  assert.match(updated, /disable-model-invocation: true/);
  assert.match(updated, /visibility: manual/);
  assert.match(readFileSync(policyFile, "utf8"), /allow_implicit_invocation: false/);

  const wrapAbort = runTendi(["skills", "wrap", "demo-router", "--from", "demo"], { input: "n\n" });
  assert.match(wrapAbort, /Apply these changes\? \[y\/N\]/);
  assert.match(wrapAbort, /aborted/);

  mkdirSync(linkSourceDir, { recursive: true });
  writeFileSync(
    join(linkSourceDir, "SKILL.md"),
    [
      "---",
      "name: link-demo",
      "description: Temporary linked smoke skill",
      "---",
      "",
      "# Link demo",
      "",
    ].join("\n"),
  );

  const linkDryRun = runTendi(["skills", "link", linkSourceDir, "--to", "codex", "--dry-run"]);
  assert.match(linkDryRun, /symlink/);
  assert.match(linkDryRun, /link-demo/);
  assert.equal(existsSync(linkTargetDir), false, "link dry-run created target");

  const linkApply = runTendi(["skills", "link", linkSourceDir, "--to", "codex", "--yes"]);
  assert.match(linkApply, /symlink/);
  assert.match(linkApply, /symlink-ok/);
  assert.equal(lstatSync(linkTargetDir).isSymbolicLink(), true, "link did not create a symlink");

  mkdirSync(addSourceDir, { recursive: true });
  writeFileSync(
    join(addSourceDir, "SKILL.md"),
    [
      "---",
      "name: add-demo",
      "description: Temporary added smoke skill",
      "---",
      "",
      "# Add demo",
      "",
    ].join("\n"),
  );

  const addList = runTendi(["skills", "add", join(temp, "add-source"), "--list"]);
  assert.match(addList, /add-demo/);
  assert.match(addList, /Temporary added smoke skill/);

  const addDryRun = runTendi([
    "skills",
    "add",
    join(temp, "add-source"),
    "--skill",
    "add-demo",
    "--to",
    "shared",
    "--dry-run",
  ]);
  assert.match(addDryRun, /A add-demo/);
  assert.equal(existsSync(addTargetDir), false, "add dry-run created target");

  const addApply = runTendi([
    "skills",
    "add",
    join(temp, "add-source"),
    "--skill",
    "add-demo",
    "--to",
    "shared",
    "--copy",
    "--yes",
  ]);
  assert.match(addApply, /copy-ok/);
  assert.equal(lstatSync(addTargetDir).isDirectory(), true, "add copy did not create target directory");

  writeStdout("cli write smoke ok");
} finally {
  rmSync(temp, { recursive: true, force: true });
}
