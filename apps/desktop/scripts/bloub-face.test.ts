import assert from "node:assert/strict";
import test from "node:test";

import { BLOUB_LOADING_SHAPE_PATHS, bloubLoadingShapeSequence, BloubFaceEngine } from "../src/lib/bloub-face.ts";

function screenYScale(matrix: string): number {
  const values = /matrix\(([^)]+)\)/.exec(matrix)?.[1]?.split(",").map(Number);
  assert.ok(values);
  return Math.hypot(values[1] ?? 0, values[3] ?? 0);
}

test("renders the measured calm and attentive faces", () => {
  const engine = new BloubFaceEngine("neutre");
  const calm = engine.sample(1_000, true);
  assert.equal(calm.eyes.length, 2);
  assert.equal(calm.dots.length, 0);

  engine.setMood("attentif", 1_000);
  const attentive = engine.sample(1_000, true);
  assert.equal(attentive.eyes.length, 2);
  assert.notDeepEqual(attentive.eyes, calm.eyes);
});

test("morphs expressions without jumping at the state boundary", () => {
  const engine = new BloubFaceEngine("neutre");
  const before = engine.sample(1_000);
  engine.setMood("attentif", 1_000);
  const boundary = engine.sample(1_000);
  const settled = engine.sample(1_450);

  assert.deepEqual(boundary, before);
  assert.notDeepEqual(settled.eyes, before.eyes);
});

test("keeps blinking after fifteen minutes", () => {
  const engine = new BloubFaceEngine("neutre");
  const open = engine.sample(902_000).eyes[0];
  const closed = engine.sample(902_190).eyes[0];

  assert.ok(open);
  assert.ok(closed);
  assert.ok(screenYScale(closed.matrix) < screenYScale(open.matrix) * 0.2);
});

test("crossfades the face into the three-dot thinking state", () => {
  const engine = new BloubFaceEngine("attentif");
  engine.setMood("thinking", 2_000);
  const thinking = engine.sample(2_000, true);

  assert.equal(thinking.eyes.length, 0);
  assert.equal(thinking.dots.length, 3);
  assert.ok(thinking.dots.every((dot) => dot.opacity > 0));
});

test("starts the loading shape cycle from the current shape", () => {
  const sequence = bloubLoadingShapeSequence(2);

  assert.equal(BLOUB_LOADING_SHAPE_PATHS.length, 4);
  assert.equal(sequence.length, 5);
  assert.equal(sequence[0], BLOUB_LOADING_SHAPE_PATHS[2]);
  assert.equal(sequence[1], BLOUB_LOADING_SHAPE_PATHS[3]);
  assert.equal(sequence[4], BLOUB_LOADING_SHAPE_PATHS[2]);
  assert.ok(sequence.every((path) => path.match(/C/g)?.length === 64));
});
