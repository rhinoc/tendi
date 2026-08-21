import assert from "node:assert/strict";
import test from "node:test";

import { SKILL_BADGE_TONES } from "../src/features/skills/skill-badge-tones.ts";

test("skill badge hierarchy keeps updates above wrapper metadata", () => {
  assert.deepEqual(SKILL_BADGE_TONES, {
    update: "warning",
    wrapper: "neutral",
  });
});
