import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const checkerPath = "scripts/check-layer-boundaries.mjs";
const allowedDirectories = ["crates/tendi-core/src/migrations/"];
const allowedBoundaryFiles = new Set([
  "crates/tendi-core/src/lib.rs",
  "crates/tendi-core/src/storage.rs",
]);
const sourceRoot = "crates/tendi-core/src/";
const forbiddenPattern = /\b(?:use|pub\s+use|mod|pub\s+mod|path\s*=)[^\n]*\bmigrations?\b/i;
const skippedDirectories = new Set([".git", "node_modules", "target"]);

const files = execFileSync(
  "git",
  ["ls-files", "--cached", "--others", "--exclude-standard", "-z"],
  { cwd: root },
)
  .toString()
  .split("\0")
  .filter(Boolean)
  .filter((relativePath) => relativePath !== checkerPath)
  .filter((relativePath) => relativePath.startsWith(sourceRoot))
  .filter((relativePath) => !relativePath.split("/").some((part) => skippedDirectories.has(part)));

const violations = [];

for (const relativePath of files) {
  if (
    allowedDirectories.some((directory) => relativePath.startsWith(directory))
    || allowedBoundaryFiles.has(relativePath)
  ) continue;

  let text;
  try {
    text = readFileSync(resolve(root, relativePath), "utf8");
  } catch {
    continue;
  }

  if (text.includes("\0")) continue;

  text.split(/\r?\n/).forEach((line, index) => {
    if (forbiddenPattern.test(line)) {
      violations.push(`${relativePath}:${index + 1}: ${line.trim()}`);
    }
  });
}

if (violations.length > 0) {
  console.error("layer boundary check failed");
  console.error(`forbidden terms: ${forbiddenPattern}`);
  console.error(`allowed directories: ${allowedDirectories.join(", ")}`);
  console.error(violations.join("\n"));
  process.exitCode = 1;
} else {
  console.log("layer boundary check passed");
}
