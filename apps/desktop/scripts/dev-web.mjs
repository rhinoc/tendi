import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { writeStderr, writeStdout } from "./stdio.mjs";

const desktopDir = resolve(fileURLToPath(new URL("..", import.meta.url)));
const node = process.execPath;
const bridgePort = "5188";
const bridgeScript = join(desktopDir, "scripts/web-daemon.mjs");
const viteScript = join(desktopDir, "node_modules/vite/bin/vite.js");
const children = [];
let shuttingDown = false;

if (!existsSync(viteScript)) {
  throw new Error(`Vite entrypoint not found at ${viteScript}. Install desktop dependencies first.`);
}

function start(label, script, args = []) {
  const child = spawn(node, [script, ...args], {
    cwd: desktopDir,
    env: { ...process.env, TENDI_WEB_BRIDGE_PORT: bridgePort },
    stdio: "inherit",
  });
  child.on("exit", (code, signal) => {
    if (!shuttingDown) {
      writeStderr(`[tendi] ${label} exited (${code ?? signal})`);
      shutdown(code || 1);
    }
  });
  children.push(child);
  return child;
}

async function waitForBridge() {
  const url = `http://127.0.0.1:${bridgePort}/health`;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch {
      // The bridge is still starting.
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`Timed out waiting for the web data bridge at ${url}`);
}

function shutdown(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;
  for (const child of children) child.kill("SIGTERM");
  setTimeout(() => process.exit(code), 100);
}

process.once("SIGINT", () => shutdown());
process.once("SIGTERM", () => shutdown());

start("web data bridge", bridgeScript);
await waitForBridge();
writeStdout("[tendi] web mode ready: Vite + local data bridge");
start("Vite", viteScript, ["--host", "127.0.0.1", ...process.argv.slice(2)]);
