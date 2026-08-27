import assert from "node:assert/strict";
import test from "node:test";

import { loadOverviewCountWithRetry } from "../src/lib/overview-counts.ts";

test("reloads a domain when its count is not ready", async () => {
  let countAttempts = 0;
  let reloads = 0;

  const loaded = await loadOverviewCountWithRetry({
    loadCount: async () => {
      countAttempts += 1;
      return countAttempts === 2;
    },
    reloadDomain: async () => {
      reloads += 1;
    },
  });

  assert.equal(loaded, true);
  assert.equal(countAttempts, 2);
  assert.equal(reloads, 1);
});

test("stops after the bounded retry budget", async () => {
  let countAttempts = 0;
  let reloads = 0;

  const loaded = await loadOverviewCountWithRetry({
    loadCount: async () => {
      countAttempts += 1;
      return false;
    },
    reloadDomain: async () => {
      reloads += 1;
    },
  });

  assert.equal(loaded, false);
  assert.equal(countAttempts, 3);
  assert.equal(reloads, 2);
});
