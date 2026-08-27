import assert from "node:assert/strict";
import test from "node:test";

import { readRuleFile } from "../src/lib/rule-file.ts";

test("propagates rule file read failures to the caller", async () => {
  await assert.rejects(
    () => readRuleFile(async () => { throw new Error("rule file missing"); }),
    /rule file missing/,
  );
});

test("rejects incomplete rule file responses instead of publishing empty content", async () => {
  await assert.rejects(
    () => readRuleFile(async () => ({ content: "", sha256: null })),
    /Rule file response was incomplete/,
  );
});
