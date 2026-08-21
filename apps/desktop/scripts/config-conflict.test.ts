import assert from "node:assert/strict";
import test from "node:test";

import { mergeThreeWay } from "../src/lib/diff.ts";

test("three-way merge keeps independent local and disk changes", () => {
  const result = mergeThreeWay(
    "theme = \"light\"\nmodel = \"one\"\n",
    "theme = \"dark\"\nmodel = \"one\"\n",
    "theme = \"light\"\nmodel = \"two\"\n",
  );
  assert.equal(result.hasConflicts, false);
  assert.equal(result.content, "theme = \"dark\"\nmodel = \"two\"\n");
});

test("three-way merge emits a resolvable conflict for overlapping changes", () => {
  const result = mergeThreeWay(
    "model = \"one\"\n",
    "model = \"local\"\n",
    "model = \"disk\"\n",
  );
  assert.equal(result.hasConflicts, true);
  assert.match(result.content, /<<<<<<< local/);
  assert.match(result.content, /\|\|\|\|\|\|\| base/);
  assert.match(result.content, />>>>>>> incoming/);
});

test("three-way merge returns the changed side when the other side stayed at base", () => {
  const result = mergeThreeWay("one\ntwo\n", "one\nlocal\n", "one\ntwo\n");
  assert.deepEqual(result, { content: "one\nlocal\n", hasConflicts: false });
});
