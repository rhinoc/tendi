import assert from "node:assert/strict";
import test from "node:test";
import { parseMacOsDevAppPids } from "./macos-dev-app-process.mjs";

test("finds the same macOS dev app when the process path uses different bundle casing", () => {
  const executable = "/tmp/target/bundle/macos/Tendi.app/Contents/MacOS/tendi-desktop";
  const output = [
    "101 /tmp/target/bundle/macos/tendi.app/Contents/MacOS/tendi-desktop",
    "102 /tmp/target/bundle/macos/Tendi.app/Contents/MacOS/tendi-desktop --test",
    "103 /tmp/target/bundle/macos/tendi.app/Contents/MacOS/tendi-desktop-other",
    "104 /tmp/other/bundle/macos/tendi.app/Contents/MacOS/tendi-desktop",
    "105 /tmp/target/bundle/macos/tendi-dev.app/Contents/MacOS/tendi-desktop",
    "106 /tmp/target/bundle/macos/tendi-preview.app/Contents/MacOS/tendi-desktop",
  ].join("\n");
  assert.deepEqual(parseMacOsDevAppPids(output, executable), [101, 102, 105]);
});
