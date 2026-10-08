import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "vite";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const desktopDir = join(scriptDir, "..");

test("Tauri dev uses the embedded daemon instead of starting the web daemon", () => {
  const config = JSON.parse(readFileSync(join(desktopDir, "src-tauri/tauri.conf.json"), "utf8"));
  assert.equal(config.build.beforeDevCommand, "npm run dev:vite -- --port 5187 --strictPort");
});

test("Vite dev cache is stable across process restarts", () => {
  const config = readFileSync(join(desktopDir, "vite.config.ts"), "utf8");
  assert.match(config, /const viteCacheDir = "node_modules\/\.vite-tendi-dev";/);
  assert.doesNotMatch(config, /vite-tendi-\$\{process\.pid\}/);
});

test("Dev module responses prevent persistent WebView caching across restarts", async () => {
  const cacheDir = await mkdtemp(join(desktopDir, "node_modules/.vite-cache-test-"));
  try {
    for (let restart = 0; restart < 2; restart += 1) {
      const server = await createServer({
        root: desktopDir,
        configFile: join(desktopDir, "vite.config.ts"),
        cacheDir,
        logLevel: "silent",
        optimizeDeps: { entries: [], include: ["@codemirror/state"] },
        server: { host: "127.0.0.1", port: 0, warmup: { clientFiles: [] } },
      });
      try {
        await server.listen();
        const origin = server.resolvedUrls.local[0];
        const source = await fetch(new URL("src/components/shared/codemirror-search.ts", origin));
        assert.equal(source.headers.get("cache-control"), "no-store");
        const statePath = (await source.text()).match(/from "([^"]*\/@codemirror_state\.js\?v=[^"]+)"/)?.[1];
        assert.ok(statePath, "State must use the optimized dependency URL");
        const dependency = await fetch(new URL(statePath, origin));
        assert.equal(dependency.status, 200);
        assert.equal(dependency.headers.get("cache-control"), "no-store");
        const chunkPath = (await dependency.text()).match(/from "([^"]*\/chunk-[^"]+\.js\?v=[^"]+)"/)?.[1];
        assert.ok(chunkPath, "State must resolve its shared chunk");
        const chunk = await fetch(new URL(chunkPath, origin));
        assert.equal(chunk.status, 200);
        assert.equal(chunk.headers.get("cache-control"), "no-store");
      } finally {
        await server.close();
      }
    }
  } finally {
    await rm(cacheDir, { recursive: true, force: true });
  }
});
