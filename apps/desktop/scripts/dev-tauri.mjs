import { spawnSync } from "node:child_process";
import {
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnOwned, stopOwned } from "./process-lifecycle.mjs";
import { writeStderr, writeStdout } from "./stdio.mjs";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopDir = resolve(scriptDir, "..");
const repoDir = resolve(desktopDir, "../..");
const requestedTargetDir = process.env.CARGO_TARGET_DIR;
const targetDir = requestedTargetDir
  ? resolve(process.cwd(), requestedTargetDir)
  : resolve(repoDir, "target", "tauri-dev");
const devEnv = {
  ...process.env,
  CARGO_TARGET_DIR: targetDir,
  CARGO_INCREMENTAL: "0",
};
const lockDir = resolve(targetDir, ".tendi-tauri-dev.lock");
const lockPidFile = resolve(lockDir, "pid");

function processIsRunning(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

function readLockPid() {
  try {
    const pid = Number.parseInt(readFileSync(lockPidFile, "utf8").trim(), 10);
    return Number.isInteger(pid) && pid > 0 ? pid : null;
  } catch {
    return null;
  }
}

function removeStaleLock() {
  rmSync(lockDir, { recursive: true, force: true });
}

function acquireLock() {
  mkdirSync(targetDir, { recursive: true });

  try {
    mkdirSync(lockDir);
  } catch (error) {
    if (error?.code !== "EEXIST") throw error;

    const pid = readLockPid();
    if (pid && processIsRunning(pid)) {
      throw new Error(`Tauri dev target is already in use by process ${pid}: ${targetDir}`);
    }

    removeStaleLock();
    mkdirSync(lockDir);
  }

  writeFileSync(lockPidFile, `${process.pid}\n`);
}

function releaseLock() {
  const pid = readLockPid();
  if (pid === process.pid) removeStaleLock();
}

try {
  acquireLock();
} catch (error) {
  writeStderr(`[tendi] ${error.message}`);
  process.exit(1);
}

writeStdout(`[tendi] CARGO_TARGET_DIR=${targetDir}`);

const cliBuild = spawnSync("cargo", ["build", "-p", "tendi-cli"], {
  cwd: repoDir,
  env: devEnv,
  stdio: "inherit",
});
if (cliBuild.error || cliBuild.status !== 0) {
  releaseLock();
  writeStderr(`[tendi] failed to build the dev CLI${cliBuild.error ? `: ${cliBuild.error.message}` : ""}`);
  process.exit(cliBuild.status || 1);
}

const tauriCommand = process.platform === "win32" ? "tauri.cmd" : "tauri";
const tauriArgs = ["dev", ...process.argv.slice(2)];
const hasCustomRunner = tauriArgs.some((argument) => (
  argument === "--runner" || argument === "-r" || argument.startsWith("--runner=")
));
if (process.platform === "darwin" && !hasCustomRunner) {
  tauriArgs.splice(1, 0, "--runner", resolve(scriptDir, "dev-tauri-macos-cargo-runner.mjs"));
}
const managesMacOsDevApp = process.platform === "darwin" && !hasCustomRunner;
const forwardedArgs = process.argv.slice(2);
const targetAssignment = forwardedArgs.find((argument) => argument.startsWith("--target="));
const targetIndex = forwardedArgs.indexOf("--target");
const cargoTarget = targetAssignment
  ? targetAssignment.slice("--target=".length)
  : targetIndex >= 0
    ? forwardedArgs[targetIndex + 1]
    : null;
const appOutputDir = cargoTarget ? resolve(targetDir, cargoTarget) : targetDir;
const devAppExecutable = resolve(
  appOutputDir,
  "debug",
  "bundle",
  "macos",
  "tendi.app",
  "Contents",
  "MacOS",
  "tendi-desktop",
);

function runningDevAppPids() {
  if (!managesMacOsDevApp) return [];
  const result = spawnSync("ps", ["-axo", "pid=,command="], { encoding: "utf8" });
  if (result.status !== 0) return [];
  return result.stdout.split("\n").flatMap((line) => {
    const match = line.trim().match(/^(\d+)\s+(.+)$/);
    if (!match || (match[2] !== devAppExecutable && !match[2].startsWith(`${devAppExecutable} `))) {
      return [];
    }
    return [Number.parseInt(match[1], 10)];
  });
}

async function stopDevApp() {
  const managedPids = () => runningDevAppPids();
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

if (managesMacOsDevApp) await stopDevApp();

const child = spawnOwned(tauriCommand, tauriArgs, {
  cwd: desktopDir,
  env: {
    ...devEnv,
    TENDI_CWD: process.env.TENDI_CWD || desktopDir,
  },
  stdio: "inherit",
});
let shuttingDown = false;

async function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  await stopOwned(child);
  await stopDevApp();
  releaseLock();
  process.exit(code);
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP", ...(process.platform === "win32" ? [] : ["SIGQUIT"])]) {
  process.once(signal, () => void shutdown());
}
process.once("exit", releaseLock);

child.once("error", (error) => {
  writeStderr(`[tendi] failed to start Tauri: ${error.message}`);
  void shutdown(1);
});

child.once("exit", (code, signal) => {
  void shutdown(code ?? (signal ? 1 : 0));
});
