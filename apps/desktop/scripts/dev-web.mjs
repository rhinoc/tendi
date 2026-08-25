import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnOwned, stopOwned } from "./process-lifecycle.mjs";
import { writeStderr, writeStdout } from "./stdio.mjs";

const desktopDir = resolve(fileURLToPath(new URL("..", import.meta.url)));
const node = process.execPath;
const bridgePort = "5188";
const bridgeScript = join(desktopDir, "scripts/web-daemon.mjs");
const viteScript = join(desktopDir, "node_modules/vite/bin/vite.js");
const bridgeStartupTimeoutMs = 60_000;
const children = [];
let shuttingDown = false;

if (!existsSync(viteScript)) {
  throw new Error(`Vite entrypoint not found at ${viteScript}. Install desktop dependencies first.`);
}

function start(label, script, args = []) {
  const child = spawnOwned(node, [script, ...args], {
    cwd: desktopDir,
    env: { ...process.env, TENDI_WEB_BRIDGE_PORT: bridgePort },
    stdio: "inherit",
  });
  child.on("exit", (code, signal) => {
    if (!shuttingDown) {
      writeStderr(`[tendi] ${label} exited (${code ?? signal})`);
      void shutdown(code || 1);
    }
  });
  children.push(child);
  return child;
}

async function waitForBridge() {
  const url = `http://127.0.0.1:${bridgePort}/health`;
  const deadline = Date.now() + bridgeStartupTimeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // The bridge is still starting.
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`Timed out waiting ${bridgeStartupTimeoutMs}ms for the web data bridge at ${url}`);
}

async function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  await Promise.all(children.map((child) => stopOwned(child)));
  process.exit(code);
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP", ...(process.platform === "win32" ? [] : ["SIGQUIT"])]) {
  process.once(signal, () => void shutdown());
}

start("web data bridge", bridgeScript);
try {
  await waitForBridge();
  writeStdout("[tendi] web mode ready: Vite + local data bridge");
  start("Vite", viteScript, ["--host", "127.0.0.1", ...process.argv.slice(2)]);
} catch (error) {
  writeStderr(`[tendi] web mode startup failed: ${error?.message || error}`);
  await shutdown(1);
}
