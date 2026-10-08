import { spawnSync } from "node:child_process";

export function parseMacOsDevAppPids(output, executable) {
  const expected = executable.toLowerCase();
  const bundlePaths = [expected, expected.replace("/tendi.app/", "/tendi-dev.app/")];
  return output.split("\n").flatMap((line) => {
    const match = line.trim().match(/^(\d+)\s+(.+)$/);
    if (!match) return [];
    const command = match[2].toLowerCase();
    if (!bundlePaths.some((path) => command === path || command.startsWith(`${path} `))) return [];
    return [Number.parseInt(match[1], 10)];
  });
}

export function runningMacOsDevAppPids(executable) {
  const result = spawnSync("ps", ["-axo", "pid=,command="], { encoding: "utf8" });
  return result.status === 0 ? parseMacOsDevAppPids(result.stdout, executable) : [];
}
