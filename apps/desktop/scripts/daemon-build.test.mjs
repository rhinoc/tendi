import assert from "node:assert/strict";
import { mkdtempSync, mkdirSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import test from "node:test";
import {
  DEFAULT_DAEMON_TARGET_DIR,
  daemonNeedsBuild,
  resolveDaemonTargetDir,
} from "./daemon-build.mjs";

function fixture() {
  const root = mkdtempSync(join(tmpdir(), "tendi-daemon-build-"));
  const cargoToml = join(root, "Cargo.toml");
  const source = join(root, "crates", "tendi-core", "src", "storage.rs");
  const binary = join(root, "target", "debug", "tendi-daemon");
  mkdirSync(join(root, "crates", "tendi-core", "src"), { recursive: true });
  mkdirSync(join(root, "target", "debug"), { recursive: true });
  writeFileSync(cargoToml, "[workspace]\n");
  writeFileSync(source, "fn main() {}\n");
  writeFileSync(binary, "daemon\n");
  return { root, cargoToml, source, binary };
}

function setMtime(path, seconds) {
  utimesSync(path, seconds, seconds);
}

test("uses a target separate from the inherited Tauri Cargo target", () => {
  const repoDir = "/repo";
  assert.equal(
    resolveDaemonTargetDir(repoDir),
    resolve(repoDir, DEFAULT_DAEMON_TARGET_DIR),
  );
});

test("allows an explicit daemon target directory", () => {
  assert.equal(
    resolveDaemonTargetDir("/repo", "build/daemon-target"),
    "/repo/build/daemon-target",
  );
});

test("requires a build when the daemon binary is missing", () => {
  const { root, binary } = fixture();
  try {
    rmSync(binary);
    assert.equal(daemonNeedsBuild(root, binary), true);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("does not rebuild when the daemon is newer than Rust inputs", () => {
  const { root, cargoToml, source, binary } = fixture();
  try {
    setMtime(cargoToml, 10);
    setMtime(source, 10);
    setMtime(binary, 20);
    assert.equal(daemonNeedsBuild(root, binary), false);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("detects nested Rust source changes", () => {
  const { root, cargoToml, source, binary } = fixture();
  try {
    setMtime(cargoToml, 10);
    setMtime(source, 30);
    setMtime(binary, 20);
    assert.equal(daemonNeedsBuild(root, binary), true);
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});
