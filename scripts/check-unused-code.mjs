#!/usr/bin/env node

import { spawnSync } from "node:child_process";
import path from "node:path";
import { fileURLToPath } from "node:url";

const repoRoot = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

const checks = [
  {
    name: "Rust dead code",
    command: "cargo",
    args: [
      "clippy",
      "--workspace",
      "--all-targets",
      "--quiet",
      "--",
      "-A",
      "clippy::all",
      "-D",
      "dead_code",
    ],
  },
  {
    name: "Desktop unused TypeScript",
    command: "pnpm",
    args: [
      "--dir",
      "apps/desktop",
      "exec",
      "tsc",
      "--noEmit",
      "--noUnusedLocals",
      "--noUnusedParameters",
    ],
  },
];

for (const check of checks) {
  console.log(`\n[unused-code] ${check.name}`);
  const result = spawnSync(check.command, check.args, {
    cwd: repoRoot,
    stdio: "inherit",
  });

  if (result.error) {
    console.error(`[unused-code] failed to start ${check.command}: ${result.error.message}`);
    process.exit(1);
  }

  if (result.status !== 0) {
    process.exit(result.status ?? 1);
  }
}

console.log("\n[unused-code] passed");
