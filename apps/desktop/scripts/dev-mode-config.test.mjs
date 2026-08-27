import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopDir = join(scriptDir, "..");

test("Tauri dev uses the embedded daemon instead of starting the web daemon", () => {
  const config = JSON.parse(readFileSync(join(desktopDir, "src-tauri/tauri.conf.json"), "utf8"));
  assert.equal(config.build.beforeDevCommand, "npm run dev:vite -- --port 5187 --strictPort");
});

test("Vite dev cache is stable across process restarts", () => {
  const config = readFileSync(join(desktopDir, "vite.config.ts"), "utf8");
  assert.match(config, /const viteCacheDir = "node_modules\/\.vite-tendi";/);
  assert.doesNotMatch(config, /vite-tendi-\$\{process\.pid\}/);
});
