import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const backupCss = await readFile(join(packageRoot, "src/features/skills/BackupView.css"), "utf8");
const backupView = await readFile(join(packageRoot, "src/features/skills/BackupView.tsx"), "utf8");

assert.doesNotMatch(backupCss, /\.backupContentItemRow(?:\s*>\s*div)?\s+span\s*\{/);
assert.match(backupCss, /\.backupDetailsDialog \{[\s\S]*height: min\(720px, calc\(100vh - 64px\)\);[\s\S]*display: flex;[\s\S]*flex-direction: column;/);
assert.match(backupCss, /\.backupDetailsBody \{[\s\S]*flex: 1 1 auto;[\s\S]*min-height: 0;[\s\S]*overflow: hidden;/);
assert.match(backupCss, /\.backupDetailsHomeBody \.settingsBackupVersionList \{[\s\S]*min-height: 0;[\s\S]*max-height: none;[\s\S]*flex: 1 1 auto;/);
assert.match(backupCss, /\.backupDetailsCategoryBody \{\s*overflow: auto;/s);
assert.match(backupView, /className=\{`backupDetailsBody \$\{activeCategory \? "backupDetailsCategoryBody" : "backupDetailsHomeBody"\}`\}/);
assert.match(backupView, /className="backupDetailsSection backupHistorySection"/);
assert.doesNotMatch(backupView, /Global managed skills|Global MCP configuration|Global rules|Global hooks/);
assert.doesNotMatch(backupView, /No global items/);
assert.match(backupView, /<strong className="dataCellTitle">\{item\.label\}<\/strong>/);
assert.match(backupView, /function backupCatalogSubtitle\(category: BackupCategory, item: BackupCatalogItem\)/);
assert.doesNotMatch(backupView, /Agent files|files: BackupCategorySelection/);
assert.doesNotMatch(backupView, /deviceLabel|Device label|setDeviceLabel/);
assert.match(backupView, /if \(category === "mcp"\) return ""/);
assert.match(backupView, /formatUserPath\(item\.detail\)/);
assert.match(backupView, /compactCommand\(item\.detail\)/);
assert.match(backupView, /subtitle \? <span className="dataCellSubLine">/);
assert.match(backupView, /const subtitle = backupCatalogSubtitle\(activeCategory\.key, item\)/);
