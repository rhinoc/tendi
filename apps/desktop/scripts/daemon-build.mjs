import { existsSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";

const daemonCrates = ["tendi-core", "tendi-daemon"];
export const DEFAULT_DAEMON_TARGET_DIR = join("target", "web-dev");

export function resolveDaemonTargetDir(repoDir, configuredTargetDir) {
  return resolve(repoDir, configuredTargetDir || DEFAULT_DAEMON_TARGET_DIR);
}

function fileMtimeMs(path) {
  try {
    return statSync(path).mtimeMs;
  } catch {
    return 0;
  }
}

function latestMtimeMs(path) {
  let stats;
  try {
    stats = statSync(path);
  } catch {
    return 0;
  }

  if (stats.isFile()) return stats.mtimeMs;
  if (!stats.isDirectory()) return 0;

  let latest = 0;
  let entries;
  try {
    entries = readdirSync(path, { withFileTypes: true });
  } catch {
    return latest;
  }

  for (const entry of entries) {
    const child = join(path, entry.name);
    if (entry.isDirectory()) {
      latest = Math.max(latest, latestMtimeMs(child));
    } else if (entry.isFile()) {
      latest = Math.max(latest, fileMtimeMs(child));
    }
  }
  return latest;
}

export function latestDaemonInputMtimeMs(repoDir) {
  const inputs = [
    join(repoDir, "Cargo.toml"),
    join(repoDir, "Cargo.lock"),
    ...daemonCrates.map((crate) => join(repoDir, "crates", crate)),
  ];
  return inputs.reduce((latest, input) => Math.max(latest, latestMtimeMs(input)), 0);
}

export function daemonNeedsBuild(repoDir, daemonBin) {
  if (!existsSync(daemonBin)) return true;
  return latestDaemonInputMtimeMs(repoDir) > fileMtimeMs(daemonBin);
}
