import { spawn } from "node:child_process";

function hasExited(child) {
  return child.exitCode !== null || child.signalCode !== null;
}

export function spawnOwned(command, args = [], options = {}) {
  const { detached = false, ...spawnOptions } = options;
  return spawn(command, args, { ...spawnOptions, detached });
}

export function signalOwned(child, signal, { processGroup = false } = {}) {
  if (!child?.pid) return;

  if (processGroup && process.platform !== "win32") {
    try {
      process.kill(-child.pid, signal);
      return;
    } catch (error) {
      if (error?.code === "ESRCH") return;
    }
  }

  try {
    child.kill(signal);
  } catch (error) {
    if (error?.code !== "ESRCH") throw error;
  }
}

export function waitForExit(child, timeoutMs = 2_000) {
  if (hasExited(child)) return Promise.resolve(true);

  return new Promise((resolve) => {
    let settled = false;
    const finish = (exited) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      child.off("exit", onExit);
      child.off("error", onError);
      resolve(exited);
    };
    const onExit = () => finish(true);
    const onError = () => finish(true);
    const timer = setTimeout(() => finish(false), timeoutMs);

    child.once("exit", onExit);
    child.once("error", onError);
    if (hasExited(child)) finish(true);
  });
}

export async function stopOwned(child, options = {}) {
  if (!child?.pid) return;

  const { signal = "SIGTERM", processGroup = false, timeoutMs = 2_000 } = options;
  signalOwned(child, signal, { processGroup });
  if (await waitForExit(child, timeoutMs)) return;

  signalOwned(child, "SIGKILL", { processGroup });
  await waitForExit(child, timeoutMs);
}
