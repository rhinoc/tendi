import { performance } from "node:perf_hooks";
import { Worker } from "node:worker_threads";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, resolve } from "node:path";
import { rm } from "node:fs/promises";
import { build } from "esbuild";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const tokenizerBundle = resolve(root, `.tmp-tendi-tokenizer-perf-${process.pid}.mjs`);
await build({
  entryPoints: [resolve(root, "src/lib/tokenizer.ts")],
  bundle: true,
  format: "esm",
  platform: "node",
  packages: "external",
  outfile: tokenizerBundle,
  logLevel: "silent",
});
const tokenizerUrl = pathToFileURL(tokenizerBundle).href;
const { transcriptTokenSegments } = await import(tokenizerUrl);

const filler = "large transcript payload ".repeat(180);
const items = Array.from({ length: 4_000 }, (_, index) => ({
  type: index % 2 === 0 ? "user" : "assistant",
  body: `${index === 3_998 ? "needle-token-input " : "ordinary-token-input "}${filler}`,
  ...(index % 5 === 0 ? { command: `node --check file-${index}.js`, result: filler } : {}),
}));
const skillLinks = [{ skill_name: "performance-test" }];

function nextTimerDelay(started) {
  return new Promise((resolveDelay) => {
    setTimeout(() => resolveDelay(performance.now() - started), 0);
  });
}

async function measureMainThread() {
  const started = performance.now();
  const timer = nextTimerDelay(started);
  const computeStarted = performance.now();
  const segments = transcriptTokenSegments(items, skillLinks);
  const computeMs = performance.now() - computeStarted;
  return {
    segments,
    computeMs,
    timerDelayMs: await timer,
  };
}

const worker = new Worker(`
  import { parentPort } from "node:worker_threads";
  import { transcriptTokenSegments } from ${JSON.stringify(tokenizerUrl)};
  parentPort.on("message", ({ items, skillLinks }) => {
    parentPort.postMessage(transcriptTokenSegments(items, skillLinks));
  });
`, { eval: true, type: "module" });

function measureWorker() {
  return new Promise(async (resolveResult, reject) => {
    const started = performance.now();
    const timer = nextTimerDelay(started);
    worker.once("error", reject);
    worker.once("message", async (segments) => {
      resolveResult({
        segments,
        wallMs: performance.now() - started,
        timerDelayMs: await timer,
      });
    });
    worker.postMessage({ items, skillLinks });
  });
}

const main = await measureMainThread();
const workerResult = await measureWorker();
await worker.terminate();
await rm(tokenizerBundle, { force: true });

console.log(JSON.stringify({
  itemCount: items.length,
  payloadCharacters: JSON.stringify(items).length,
  mainThread: {
    computeMs: Number(main.computeMs.toFixed(2)),
    timerDelayMs: Number(main.timerDelayMs.toFixed(2)),
    totalTokens: main.segments.find((segment) => segment.label === "Total")?.value ?? 0,
  },
  worker: {
    wallMs: Number(workerResult.wallMs.toFixed(2)),
    timerDelayMs: Number(workerResult.timerDelayMs.toFixed(2)),
    totalTokens: workerResult.segments.find((segment) => segment.label === "Total")?.value ?? 0,
  },
}, null, 2));
