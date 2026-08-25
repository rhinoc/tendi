import assert from "node:assert/strict";
import test from "node:test";
import { spawnOwned, stopOwned } from "./process-lifecycle.mjs";

function waitForChildPid(child) {
  return new Promise((resolve, reject) => {
    let output = "";
    const onData = (chunk) => {
      output += chunk;
      const pid = Number.parseInt(output, 10);
      if (Number.isInteger(pid) && pid > 0) {
        child.stdout.off("data", onData);
        resolve(pid);
      }
    };
    child.stdout.on("data", onData);
    child.once("error", reject);
    child.once("exit", (code, signal) => reject(new Error(`fixture exited (${code ?? signal})`)));
  });
}

function isRunning(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

test("stopping an owned process group also stops its descendants", async () => {
  const child = spawnOwned(process.execPath, [
    "-e",
    "const { spawn } = require('node:child_process'); const grandchild = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], { stdio: 'ignore' }); console.log(grandchild.pid); setInterval(() => {}, 1000);",
  ], {
    detached: process.platform !== "win32",
    stdio: ["ignore", "pipe", "ignore"],
  });
  let grandchildPid = null;

  try {
    grandchildPid = await waitForChildPid(child);
    assert.equal(isRunning(grandchildPid), true);
    await stopOwned(child, { processGroup: process.platform !== "win32" });
    assert.equal(isRunning(child.pid), false);
    assert.equal(isRunning(grandchildPid), false);
  } finally {
    if (grandchildPid && isRunning(grandchildPid)) process.kill(grandchildPid, "SIGKILL");
    if (isRunning(child.pid)) process.kill(child.pid, "SIGKILL");
  }
});
