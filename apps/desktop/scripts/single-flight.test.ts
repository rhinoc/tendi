import assert from "node:assert/strict";
import test from "node:test";

import { singleFlight, singleFlightKey } from "../src/lib/single-flight.ts";

test("singleFlight shares an active operation and releases it after completion", async () => {
  let calls = 0;
  let resolveOperation: ((value: string) => void) | undefined;
  const operation = () => {
    calls += 1;
    return new Promise<string>((resolve) => { resolveOperation = resolve; });
  };

  const first = singleFlight("test", operation);
  const second = singleFlight("test", operation);
  assert.strictEqual(first, second);
  await Promise.resolve();
  assert.equal(calls, 1);

  resolveOperation?.("done");
  assert.equal(await first, "done");
  assert.equal(await singleFlight("test", async () => "next"), "next");
});

test("singleFlightKey is stable for object key order", () => {
  assert.equal(
    singleFlightKey("command", { b: 2, a: 1 }),
    singleFlightKey("command", { a: 1, b: 2 }),
  );
});
