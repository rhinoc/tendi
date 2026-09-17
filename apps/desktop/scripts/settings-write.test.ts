import assert from "node:assert/strict";
import test from "node:test";
import { createSettingsPatchWriter } from "../src/lib/settings-write.ts";
import type { SettingsPayload } from "../src/lib/settings.ts";
import { validateRequest } from "../src/lib/generated/runtime-validators.ts";

test("settings RPC accepts field patches and rejects profile replacement", () => {
  assert.doesNotThrow(() => validateRequest("settings_save", { developerMode: false }));
  assert.doesNotThrow(() => validateRequest("settings_save", { terminal: "custom", additionalSessionRoots: [] }));
  assert.throws(() => validateRequest("settings_save", { configProfiles: { codex: "old" } }));
  assert.throws(() => validateRequest("settings_save", { appearance: null }));
});

function deferred() {
  let resolve!: (value: SettingsPayload) => void;
  let reject!: (error: Error) => void;
  const promise = new Promise<SettingsPayload>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

test("overlapping setting saves preserve intent order while unrelated fields run concurrently", async () => {
  const first = deferred();
  const calls: unknown[] = [];
  const write = createSettingsPatchWriter(async (patch) => {
    calls.push(patch);
    if (patch.terminal === "first") return first.promise;
    return patch as SettingsPayload;
  });
  const pending = write({ terminal: "first" });
  const later = write({ terminal: "second" });
  await write({ editor: "custom" });
  assert.deepEqual(calls, [{ terminal: "first" }, { editor: "custom" }]);
  first.resolve({ terminal: "first" } as SettingsPayload);
  await Promise.all([pending, later]);
  assert.deepEqual(calls[2], { terminal: "second" });
});

test("a failed patch does not poison subsequent saves", async () => {
  const first = deferred();
  const calls: unknown[] = [];
  const write = createSettingsPatchWriter(async (patch) => {
    calls.push(patch);
    if (patch.terminal === "first") return first.promise;
    return patch as SettingsPayload;
  });
  const pending = write({ terminal: "first" });
  const rejected = assert.rejects(pending, /failed/);
  const later = write({ terminal: "second" });
  first.reject(new Error("failed"));
  await rejected;
  assert.equal((await later).terminal, "second");
  assert.equal(calls.length, 2);
});

test("a multi-field patch waits for all overlapping saves", async () => {
  const first = deferred();
  const second = deferred();
  const calls: unknown[] = [];
  const write = createSettingsPatchWriter(async (patch) => {
    calls.push(patch);
    if (patch.terminal === "first") return first.promise;
    if (patch.editor === "first") return second.promise;
    return patch as SettingsPayload;
  });
  const a = write({ terminal: "first" });
  const b = write({ editor: "first" });
  const both = write({ terminal: "last", editor: "last" });
  await Promise.resolve();
  first.resolve({} as SettingsPayload);
  await a;
  assert.equal(calls.length, 2);
  second.resolve({} as SettingsPayload);
  await Promise.all([b, both]);
  assert.deepEqual(calls[2], { terminal: "last", editor: "last" });
});
