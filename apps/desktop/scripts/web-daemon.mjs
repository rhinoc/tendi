import { randomBytes } from "node:crypto";
import { existsSync } from "node:fs";
import { createServer } from "node:http";
import { Readable } from "node:stream";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnOwned, stopOwned } from "./process-lifecycle.mjs";
import { writeStderr, writeStdout } from "./stdio.mjs";

const scriptDir = resolve(fileURLToPath(new URL(".", import.meta.url)));
const desktopDir = resolve(scriptDir, "..");
const repoDir = resolve(desktopDir, "../..");
const bridgePort = Number(process.env.TENDI_WEB_BRIDGE_PORT || 5188);
const daemonPort = Number(process.env.TENDI_DAEMON_PORT || bridgePort + 1);
const workspace = resolve(process.env.TENDI_CWD || desktopDir);
const cargoTargetDir = resolve(repoDir, process.env.CARGO_TARGET_DIR || "target");
const daemonBin = resolve(process.env.TENDI_DAEMON_BIN || join(cargoTargetDir, "debug/tendi-daemon"));
const token = process.env.TENDI_DAEMON_TOKEN || randomBytes(24).toString("hex");
let buildProcess = null;
let daemon = null;
let server = null;
let shuttingDown = false;

function waitForProcess(child) {
  return new Promise((resolveProcess, reject) => {
    child.once("error", reject);
    child.once("exit", (code, signal) => resolveProcess({ code, signal }));
  });
}

async function ensureDaemonBinary() {
  if (existsSync(daemonBin)) return;

  writeStdout(`[tendi] building Rust daemon at ${daemonBin}`);
  buildProcess = spawnOwned("cargo", ["build", "-p", "tendi-daemon"], {
    cwd: repoDir,
    stdio: "inherit",
  });
  try {
    const result = await waitForProcess(buildProcess);
    if (result.code !== 0 || !existsSync(daemonBin)) {
      throw new Error(`Tendi daemon build failed at ${daemonBin}. Set TENDI_DAEMON_BIN to an existing binary.`);
    }
  } finally {
    buildProcess = null;
  }
}

function close(code = 0) {
  if (shuttingDown) return;
  shuttingDown = true;

  server?.closeAllConnections?.();
  if (server?.listening) server.close();

  const ownedChildren = [buildProcess, daemon].filter(Boolean);
  void Promise.all(ownedChildren.map((child) => stopOwned(child))).finally(() => process.exit(code));
}

for (const signal of ["SIGINT", "SIGTERM", "SIGHUP", ...(process.platform === "win32" ? [] : ["SIGQUIT"])]) {
  process.once(signal, () => close());
}

async function startDaemon() {
  await ensureDaemonBinary();
  daemon = spawnOwned(daemonBin, [
    "--port", `${daemonPort}`,
    "--workspace", workspace,
    "--token", token,
  ], {
    cwd: repoDir,
    env: { ...process.env, TENDI_CWD: workspace, TENDI_DAEMON_TOKEN: token },
    stdio: "inherit",
  });

  daemon.on("exit", (code, signal) => {
    if (!shuttingDown) {
      writeStderr(`[tendi] Rust daemon exited (${code ?? signal})`);
      close(code || 1);
    }
  });
}

/* istanbul ignore next -- startup failures are covered by the lifecycle smoke test. */
function reportStartupFailure(error) {
  writeStderr(`[tendi] web data bridge startup failed: ${error?.message || error}`);
  close(1);
}

/* istanbul ignore next -- the server error is environment-dependent. */
function reportListenFailure(error) {
  writeStderr(`[tendi] web proxy failed to listen on 127.0.0.1:${bridgePort}: ${error?.message || error}`);
  close(1);
}

async function daemonFetch(path, init = {}) {
  return fetch(`http://127.0.0.1:${daemonPort}${path}`, {
    ...init,
    headers: {
      authorization: `Bearer ${token}`,
      ...(init.headers || {}),
    },
  });
}

async function waitForDaemon() {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    try {
      const response = await daemonFetch("/health");
      if (response.ok) return;
    } catch {
      // The Rust daemon is still starting.
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`Timed out waiting for Rust daemon on port ${daemonPort}`);
}

function readBody(request) {
  return new Promise((resolveBody, reject) => {
    const chunks = [];
    request.on("data", (chunk) => chunks.push(chunk));
    request.on("end", () => resolveBody(Buffer.concat(chunks)));
    request.on("error", reject);
  });
}

function sendJson(response, status, value) {
  const body = JSON.stringify(value);
  response.writeHead(status, {
    "content-type": "application/json; charset=utf-8",
    "cache-control": "no-store",
    "access-control-allow-origin": "*",
    "access-control-allow-headers": "content-type",
  });
  response.end(body);
}

server = createServer(async (request, response) => {
  if (request.method === "OPTIONS") {
    response.writeHead(204, { "access-control-allow-origin": "*", "access-control-allow-headers": "content-type" });
    response.end();
    return;
  }
  if (request.method === "GET" && request.url === "/health") {
    try {
      const daemonResponse = await daemonFetch("/health");
      const body = await daemonResponse.text();
      response.writeHead(daemonResponse.status, { "content-type": "application/json; charset=utf-8", "cache-control": "no-store" });
      response.end(body);
    } catch (error) {
      sendJson(response, 503, { ok: false, error: { code: "DAEMON_UNAVAILABLE", message: `${error}` } });
    }
    return;
  }
  if (request.method === "GET" && request.url === "/__tendi/events") {
    const controller = new AbortController();
    const abortUpstream = () => controller.abort();
    request.once("aborted", abortUpstream);
    response.once("close", abortUpstream);
    response.once("error", abortUpstream);
    try {
      const daemonResponse = await daemonFetch("/v1/events", { signal: controller.signal });
      response.writeHead(daemonResponse.status, {
        "content-type": daemonResponse.headers.get("content-type") || "text/event-stream; charset=utf-8",
        "cache-control": "no-cache",
        "access-control-allow-origin": "*",
        "access-control-allow-headers": "content-type, authorization",
        connection: "keep-alive",
      });
      response.write(": connected\n\n");
      if (daemonResponse.body) {
        Readable.fromWeb(daemonResponse.body).on("error", () => response.destroy()).pipe(response);
      } else {
        response.end();
      }
    } catch (error) {
      if (!response.headersSent) {
        sendJson(response, 502, { ok: false, error: { code: "DAEMON_UNAVAILABLE", message: `${error}` } });
      } else {
        response.destroy();
      }
    }
    return;
  }
  if (request.method === "POST" && request.url === "/__tendi/log") {
    try {
      const body = await readBody(request);
      const daemonResponse = await daemonFetch("/v1/log", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body,
      });
      const responseBody = await daemonResponse.text();
      response.writeHead(daemonResponse.status, {
        "content-type": "application/json; charset=utf-8",
        "cache-control": "no-store",
        "access-control-allow-origin": "*",
      });
      response.end(responseBody);
    } catch (error) {
      sendJson(response, 502, { ok: false, error: { code: "DAEMON_UNAVAILABLE", message: `${error}` } });
    }
    return;
  }
  if (request.method !== "POST" || request.url !== "/__tendi/invoke") {
    sendJson(response, 404, { ok: false, error: { code: "NOT_FOUND", message: "not found" } });
    return;
  }
  try {
    const body = await readBody(request);
    const daemonResponse = await daemonFetch("/v1/rpc", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body,
    });
    const responseBody = await daemonResponse.text();
    response.writeHead(daemonResponse.status, {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "access-control-allow-origin": "*",
    });
    response.end(responseBody);
  } catch (error) {
    sendJson(response, 502, { ok: false, error: { code: "DAEMON_UNAVAILABLE", message: `${error}` } });
  }
});

server.on("error", reportListenFailure);

try {
  await startDaemon();
  await waitForDaemon();
  server.listen(bridgePort, "127.0.0.1", () => {
    writeStdout(`[tendi] web proxy listening on http://127.0.0.1:${bridgePort}`);
    writeStdout(`[tendi] Rust daemon workspace=${workspace}`);
  });
} catch (error) {
  reportStartupFailure(error);
}
