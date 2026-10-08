import assert from "node:assert/strict";
import test from "node:test";
import { startAsyncSubscription } from "../src/controllers/async-subscription.ts";

test("a completed subscription cleans up after its effect was replaced", async () => {
  let finishFirst!: (cleanup: () => void) => void;
  let firstHandler!: (event: number) => void;
  let secondHandler!: (event: number) => void;
  let firstCleanups = 0;
  let secondCleanups = 0;
  const received: number[] = [];
  const onError = (error: unknown) => assert.fail(`${error}`);

  const first = startAsyncSubscription<number>((handler) => {
    firstHandler = handler;
    return new Promise((resolve) => { finishFirst = resolve; });
  }, (event) => received.push(event), onError);
  first.dispose();

  const second = startAsyncSubscription<number>(async (handler) => {
    secondHandler = handler;
    return () => { secondCleanups += 1; };
  }, (event) => received.push(event), onError);
  await second.ready;
  finishFirst(() => { firstCleanups += 1; });
  await first.ready;

  firstHandler(1);
  secondHandler(2);
  assert.deepEqual(received, [2]);
  assert.equal(firstCleanups, 1);
  second.dispose();
  assert.equal(secondCleanups, 1);
});
