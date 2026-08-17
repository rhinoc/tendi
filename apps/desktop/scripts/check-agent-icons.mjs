import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const DESKTOP_ROOT = fileURLToPath(new URL("../", import.meta.url));
const REPOSITORY_ROOT = join(DESKTOP_ROOT, "../..");
const TARGETS_FILE = join(REPOSITORY_ROOT, "crates/tendi-core/src/skill_targets.rs");
const ICONS_FILE = join(DESKTOP_ROOT, "src/lib/agent/catalog.ts");
const CORE_AGENT_FILES = [
  "src/lib/agent/codex.ts",
  "src/lib/agent/claude.ts",
  "src/lib/agent/cursor.ts",
].map((path) => join(DESKTOP_ROOT, path));

const explicitlyUnavailable = new Set(["jazz", "kode", "terramind"]);
const normalize = (value) => value.toLowerCase().replace(/[^a-z0-9]+/g, "");

const [targetsSource, iconsSource, ...coreAgentSources] = await Promise.all([
  readFile(TARGETS_FILE, "utf8"),
  readFile(ICONS_FILE, "utf8"),
  ...CORE_AGENT_FILES.map((path) => readFile(path, "utf8")),
]);

const targets = [...targetsSource.matchAll(/target!\(\s*"([^"]+)"/g)]
  .map((match) => match[1])
  .filter((target) => !["shared", "universal"].includes(target));
const iconKeys = new Set([
  ...[...iconsSource.matchAll(/^\s{2}([a-z0-9]+):/gm)].map((match) => match[1]),
  ...coreAgentSources.flatMap((source) => [...source.matchAll(/aliases:\s*\[([^\]]+)/g)]
    .flatMap((match) => [...match[1].matchAll(/"([^"]+)"/g)].map((alias) => normalize(alias[1])))),
]);
const missing = targets.filter((target) => !iconKeys.has(normalize(target)));
const undocumented = missing.filter((target) => !explicitlyUnavailable.has(normalize(target)));
const staleUnavailable = [...explicitlyUnavailable].filter((target) => iconKeys.has(target));

if (undocumented.length > 0 || staleUnavailable.length > 0) {
  if (undocumented.length > 0) {
    console.error(`Missing agent icon mappings: ${undocumented.join(", ")}`);
  }
  if (staleUnavailable.length > 0) {
    console.error(`Remove now-available agents from explicitlyUnavailable: ${staleUnavailable.join(", ")}`);
  }
  process.exitCode = 1;
} else {
  console.log(`Agent icon coverage OK: ${targets.length - missing.length}/${targets.length}; explicitly unavailable: ${[...explicitlyUnavailable].join(", ")}`);
}
