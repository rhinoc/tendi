import assert from "node:assert/strict";
import { chmodSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const desktopDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const devTauriScript = join(desktopDir, "scripts", "dev-tauri.mjs");

const fakeCommand = `#!/usr/bin/env node
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { basename } from "node:path";

const recordPath = process.env.TENDI_TEST_RECORD;
const records = existsSync(recordPath) ? JSON.parse(readFileSync(recordPath, "utf8")) : [];
records.push({
  command: basename(process.argv[1]),
  args: process.argv.slice(2),
  cargoIncremental: process.env.CARGO_INCREMENTAL,
  cargoTargetDir: process.env.CARGO_TARGET_DIR,
});
writeFileSync(recordPath, JSON.stringify(records));
`;

test("disables incremental compilation for both dev Cargo entry points", () => {
  const root = mkdtempSync(join(tmpdir(), "tendi-dev-tauri-"));
  const binDir = join(root, "bin");
  const recordPath = join(root, "records.json");
  const targetDir = join(root, "target");
  const originalPath = process.env.PATH || "";

  try {
    const cargoPath = join(binDir, "cargo");
    const tauriPath = join(binDir, "tauri");
    mkdirSync(binDir);
    writeFileSync(recordPath, "[]");
    writeFileSync(cargoPath, fakeCommand);
    writeFileSync(tauriPath, fakeCommand);
    chmodSync(cargoPath, 0o755);
    chmodSync(tauriPath, 0o755);

    const result = spawnSync(process.execPath, [devTauriScript], {
      cwd: desktopDir,
      env: {
        ...process.env,
        PATH: `${binDir}:${originalPath}`,
        CARGO_TARGET_DIR: targetDir,
        CARGO_INCREMENTAL: "1",
        TENDI_TEST_RECORD: recordPath,
      },
      encoding: "utf8",
    });

    assert.equal(result.status, 0, result.stderr || result.stdout);
    assert.deepEqual(JSON.parse(readFileSync(recordPath, "utf8")), [
      {
        command: "cargo",
        args: ["build", "-p", "tendi-cli"],
        cargoIncremental: "0",
        cargoTargetDir: targetDir,
      },
      {
        command: "tauri",
        args: ["dev"],
        cargoIncremental: "0",
        cargoTargetDir: targetDir,
      },
    ]);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
