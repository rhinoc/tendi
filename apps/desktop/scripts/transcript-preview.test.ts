import { strict as assert } from "node:assert";
import test from "node:test";

import { transcriptContextPreview, transcriptEvidenceSearchText } from "../src/lib/transcript.ts";

test("skill context previews use the recognized skill name", () => {
  assert.equal(
    transcriptContextPreview(
      "<skills.selected_skill_instructions>\n<name>better</name>\n<path>/tmp/better/SKILL.md</path>\n</skills.selected_skill_instructions>",
      "Skill",
    ),
    "better",
  );
});

test("non-skill context previews keep the first non-empty line", () => {
  assert.equal(transcriptContextPreview("\nfirst line\nsecond line", "Developer"), "first line");
});

test("skill evidence matching removes the backend truncation marker", () => {
  assert.equal(
    transcriptEvidenceSearchText("prefix from the transcript\n... truncated"),
    "prefix from the transcript",
  );
  assert.equal(transcriptEvidenceSearchText("complete evidence"), "complete evidence");
});
