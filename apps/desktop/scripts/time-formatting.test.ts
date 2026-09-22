import assert from "node:assert/strict";
import test from "node:test";

import { compactDateTime, dayGroupKey } from "../src/lib/strings.ts";
import { compactTime, normalizeTranscript } from "../src/lib/transcript.ts";

test("formats absolute timestamps in the user's timezone", () => {
  const previousTimeZone = process.env.TZ;
  process.env.TZ = "Asia/Singapore";
  try {
    const timestamp = "2026-08-27T16:30:00Z";
    assert.equal(compactDateTime(timestamp), "8/28 00:30");
    assert.equal(compactDateTime(timestamp, { year: true }), "2026-08-28 00:30");
    assert.equal(compactTime(timestamp), "00:30");
    assert.equal(normalizeTranscript([{ kind: "assistant", body: "reply", time: timestamp }])[0].time, "00:30");
    assert.equal(dayGroupKey(timestamp), "2026-08-28");
  } finally {
    if (previousTimeZone === undefined) delete process.env.TZ;
    else process.env.TZ = previousTimeZone;
  }
});

test("preserves already compact transcript times when no timestamp can be parsed", () => {
  assert.equal(compactTime("00:30"), "00:30");
});
