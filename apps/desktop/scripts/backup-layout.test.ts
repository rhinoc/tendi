import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const backupViewCss = await readFile(new URL("../src/features/skills/BackupView.css", import.meta.url), "utf8");

test("Sync summary reserves the configured-state action stack while loading", () => {
  assert.match(backupViewCss, /\.settingsBackupActionStack\s*\{[\s\S]*grid-template-rows:\s*var\(--control-height\)\s+var\(--leading-ui\);/);
  assert.match(backupViewCss, /\.settingsBackupActionStack\s*\{[\s\S]*min-height:\s*calc\(var\(--control-height\)\s*\+\s*3px\s*\+\s*var\(--leading-ui\)\);/);
  assert.match(backupViewCss, /\.settingsBackupPrimaryActions\s*\{[\s\S]*height:\s*var\(--control-height\);/);
});
