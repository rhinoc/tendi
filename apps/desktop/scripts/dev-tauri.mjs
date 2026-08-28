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
const child = spawnOwned(tauriCommand, ["dev", ...process.argv.slice(2)], {
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
  releaseLock();
  await stopOwned(child);
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
