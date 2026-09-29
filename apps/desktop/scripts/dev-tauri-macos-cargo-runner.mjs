#!/usr/bin/env node

import { spawn, spawnSync } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const args = process.argv.slice(2);
if (args[0] !== "run") {
  process.stderr.write(`[tendi] unsupported Tauri dev runner command: ${args[0] ?? "(empty)"}\n`);
  process.exit(1);
}

const separator = args.indexOf("--");
const cargoOptions = args.slice(1, separator < 0 ? args.length : separator);
const targetIndex = cargoOptions.indexOf("--target");
const targetOption = targetIndex >= 0 ? cargoOptions[targetIndex + 1] : null;
const targetAssignment = cargoOptions.find((argument) => argument.startsWith("--target="));
const rustc = spawnSync("rustc", ["-vV"], { encoding: "utf8" });
const host = rustc.stdout?.match(/^host: (.+)$/m)?.[1];
const target = targetOption || targetAssignment?.slice("--target=".length) || host;

if (!target) {
  process.stderr.write("[tendi] could not determine the Rust target for the macOS dev runner\n");
  process.exit(1);
}

const scriptDir = dirname(fileURLToPath(import.meta.url));
const appRunner = resolve(scriptDir, "dev-tauri-macos-runner.mjs");
const runnerVariable = `CARGO_TARGET_${target.replace(/[^A-Za-z0-9]/g, "_").toUpperCase()}_RUNNER`;
const env = { ...process.env, [runnerVariable]: appRunner };
const targetDir = resolve(process.env.CARGO_TARGET_DIR || resolve(process.cwd(), "target"));
const outputDir = targetOption || targetAssignment
  ? resolve(targetDir, target)
  : targetDir;
const appExecutable = resolve(
  outputDir,
  "debug",
  "bundle",
  "macos",
  "Tendi.app",
  "Contents",
  "MacOS",
  "tendi-desktop",
);

function bundlePids() {
  const result = spawnSync("ps", ["-axo", "pid=,command="], { encoding: "utf8" });
  if (result.status !== 0) return [];
  return result.stdout.split("\n").flatMap((line) => {
    const match = line.trim().match(/^(\d+)\s+(.+)$/);
    if (!match || (match[2] !== appExecutable && !match[2].startsWith(`${appExecutable} `))) {
      return [];
    }
    return [Number.parseInt(match[1], 10)];
  });
}

async function terminateDevApp() {
  const managedPids = () => bundlePids();
  const signalPids = (pids, signal) => {
    for (const pid of pids) {
      try {
        process.kill(pid, signal);
      } catch (error) {
        if (error?.code !== "ESRCH") throw error;
      }
    }
  };

  signalPids(managedPids(), "SIGTERM");
  const deadline = Date.now() + 2_000;
  while (Date.now() < deadline && managedPids().length > 0) {
    await new Promise((done) => setTimeout(done, 100));
  }
  signalPids(managedPids(), "SIGKILL");
}

const child = spawn("cargo", args, {
  cwd: process.cwd(),
  env,
  stdio: "inherit",
});
let shuttingDown = false;
let finishing = false;

function shutdown(signal) {
  if (shuttingDown) return;
  shuttingDown = true;
  if (child.exitCode === null && child.signalCode === null) child.kill(signal);
}

async function finish(code) {
  if (finishing) return;
  finishing = true;
  await terminateDevApp();
  process.exit(code);
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP", "SIGQUIT"]) {
  process.once(signal, () => shutdown(signal));
}

child.once("error", (error) => {
  process.stderr.write(`[tendi] failed to start Cargo: ${error.message}\n`);
  void finish(1);
});

child.once("exit", (code, signal) => {
  void finish(code ?? (signal ? 1 : 0));
});
