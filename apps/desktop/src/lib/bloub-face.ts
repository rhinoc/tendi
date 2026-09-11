/*
 * Face geometry and motion adapted from jeremy-prt/bloub.
 * https://github.com/jeremy-prt/bloub
 *
 * MIT License
 * Copyright (c) 2026 Jérémy Perret
 *
 * Permission is hereby granted, free of charge, to any person obtaining a copy
 * of this software and associated documentation files (the "Software"), to deal
 * in the Software without restriction, including without limitation the rights
 * to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
 * copies of the Software, and to permit persons to whom the Software is
 * furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in all
 * copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
 * AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
 * LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
 * OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
 * SOFTWARE.
 */

export type BloubExpressionId =
  | "neutre"
  | "attentif"
  | "surpris"
  | "excite"
  | "heureux"
  | "hilare"
  | "colere"
  | "triste"
  | "effraye"
  | "mefiant"
  | "confus"
  | "curieux"
  | "fier"
  | "timide"
  | "blase"
  | "somnolent";

export type BloubFaceMood = BloubExpressionId | "thinking";

type Vec3 = [number, number, number];

type HeadGaze = {
  yaw: number;
  pitch: number;
  roll: number;
};

type EyeConfig = {
  w: number;
  h: number;
  tilt?: number;
  open: number;
};

type BotExpression = {
  id: BloubExpressionId;
  gaze: HeadGaze;
  split: number;
  eyes: [EyeConfig, EyeConfig];
};

type EyePose = {
  x: number;
  y: number;
  a: number;
  b: number;
  c: number;
  d: number;
  depth: number;
};

export type BloubEyeFrame = {
  d: string;
  matrix: string;
  opacity: number;
};

export type BloubDotFrame = {
  x: number;
  y: number;
  r: number;
  opacity: number;
};

export type BloubFaceFrame = {
  eyes: BloubEyeFrame[];
  dots: BloubDotFrame[];
};

const TAU = Math.PI * 2;
const FACE_SCALE = 8.6;
const MORPH_DURATION = 450;
const EYE_SPLIT = 15.46;
const EYE_W = 0.186;
const EYE_H = 0.412;
const REST_GAZE: HeadGaze = { yaw: 28.49, pitch: 28.62, roll: -13 };
const INWARD_GAZE = { yaw: -26, pitch: 10 };
const DOT_X = [-0.557, -0.013, 0.532] as const;
const DOT_R = 0.165;
const DOT_PEAK = 1.25;

const clamp = (value: number, low = 0, high = 1) => (
  value < low ? low : value > high ? high : value
);
const lerp = (from: number, to: number, amount: number) => from + (to - from) * amount;
const easeOutQuint = (value: number) => 1 - (1 - value) ** 5;
const roundShort = (value: number) => Math.round(value * 100) / 100;
const degrees = (value: number) => (value * Math.PI) / 180;

const BLOUB_SHAPE_SAMPLES = 64;
const BLOUB_SHAPE_RADIUS = 9;

const BLOUB_TRIANGLE_PROFILE = [
  0.7819, 0.8211, 0.8747, 0.9440, 1.0223, 1.0960, 1.1401, 1.1340,
  1.0808, 1.0047, 0.9265, 0.8603, 0.8104, 0.7730, 0.7450, 0.7273,
  0.7151, 0.7118, 0.7148, 0.7245, 0.7427, 0.7680, 0.8037, 0.8518,
  0.9148, 0.9876, 1.0583, 1.1073, 1.1109, 1.0667, 0.9940, 0.9164,
  0.8482, 0.7948, 0.7555, 0.7261, 0.7056, 0.6925, 0.6859, 0.6869,
  0.6938, 0.7084, 0.7305, 0.7615, 0.8040, 0.8595, 0.9311, 1.0092,
  1.0791, 1.1171, 1.1054, 1.0501, 0.9779, 0.9050, 0.8450, 0.7990,
  0.7656, 0.7413, 0.7258, 0.7160, 0.7146, 0.7204, 0.7330, 0.7528,
] as const;

const BLOUB_HEXAGON_PROFILE = [
  0.9210, 0.9282, 0.9441, 0.9706, 0.9984, 1.0059, 0.9896, 0.9562,
  0.9290, 0.9124, 0.9047, 0.9058, 0.9157, 0.9349, 0.9642, 0.9873,
  0.9882, 0.9665, 0.9336, 0.9105, 0.8968, 0.8918, 0.8955, 0.9080,
  0.9293, 0.9611, 0.9820, 0.9812, 0.9590, 0.9282, 0.9089, 0.8978,
  0.8964, 0.9026, 0.9189, 0.9439, 0.9778, 0.9990, 0.9964, 0.9713,
  0.9439, 0.9274, 0.9196, 0.9206, 0.9308, 0.9502, 0.9799, 1.0121,
  1.0226, 1.0071, 0.9752, 0.9510, 0.9366, 0.9316, 0.9351, 0.9485,
  0.9711, 1.0026, 1.0213, 1.0155, 0.9863, 0.9547, 0.9347, 0.9232,
] as const;

function squircleProfile(): number[] {
  const exponent = 4.2;
  const profile = Array.from({ length: BLOUB_SHAPE_SAMPLES }, (_, index) => {
    const angle = (index / BLOUB_SHAPE_SAMPLES) * TAU;
    const cosine = Math.abs(Math.cos(angle));
    const sine = Math.abs(Math.sin(angle));
    return (cosine ** exponent + sine ** exponent) ** (-1 / exponent);
  });
  const peak = Math.max(...profile);
  return profile.map((radius) => (radius / peak) * 1.15);
}

function loadingShapePath(profile: readonly number[]): string {
  const points = Array.from({ length: BLOUB_SHAPE_SAMPLES }, (_, index) => {
    const angle = (index / BLOUB_SHAPE_SAMPLES) * TAU;
    const radius = (profile[index] ?? 1) * BLOUB_SHAPE_RADIUS;
    return {
      x: radius * Math.cos(angle),
      y: radius * Math.sin(angle),
    };
  });
  let path = `M${roundShort(points[0]!.x)} ${roundShort(points[0]!.y)}`;
  for (let index = 0; index < points.length; index += 1) {
    const previous = points[(index - 1 + points.length) % points.length]!;
    const current = points[index]!;
    const next = points[(index + 1) % points.length]!;
    const afterNext = points[(index + 2) % points.length]!;
    path += `C${roundShort(current.x + (next.x - previous.x) / 6)} ${roundShort(current.y + (next.y - previous.y) / 6)}`
      + ` ${roundShort(next.x - (afterNext.x - current.x) / 6)} ${roundShort(next.y - (afterNext.y - current.y) / 6)}`
      + ` ${roundShort(next.x)} ${roundShort(next.y)}`;
  }
  return `${path}Z`;
}

/** Profiles follow reference/bloub and share 64 points so every transition is a true morph. */
export const BLOUB_LOADING_SHAPE_PATHS = [
  loadingShapePath(new Array(BLOUB_SHAPE_SAMPLES).fill(1)),
  loadingShapePath(BLOUB_TRIANGLE_PROFILE),
  loadingShapePath(squircleProfile()),
  loadingShapePath(BLOUB_HEXAGON_PROFILE),
] as const;

export function bloubLoadingShapeSequence(startIndex: number): string[] {
  const count = BLOUB_LOADING_SHAPE_PATHS.length;
  const start = ((startIndex % count) + count) % count;
  return Array.from({ length: count + 1 }, (_, offset) => (
    BLOUB_LOADING_SHAPE_PATHS[(start + offset) % count]!
  ));
}

function loopNoise(time: number, period: number, seed = 0): number {
  const phase = (time / period) * TAU;
  return (
    0.55 * Math.sin(phase + seed)
    + 0.3 * Math.sin(2 * phase + seed * 1.7 + 1.1)
    + 0.15 * Math.sin(3 * phase + seed * 2.3 + 2.4)
  );
}

function createRng(seed: number) {
  let value = seed >>> 0;
  return () => {
    value = (value + 0x6d2b79f5) >>> 0;
    let mixed = Math.imul(value ^ (value >>> 15), 1 | value);
    mixed = (mixed + Math.imul(mixed ^ (mixed >>> 7), 61 | mixed)) ^ mixed;
    return ((mixed ^ (mixed >>> 14)) >>> 0) / 4294967296;
  };
}

function spin(first: Vec3, second: Vec3, angle: number): [Vec3, Vec3] {
  const cosine = Math.cos(angle);
  const sine = Math.sin(angle);
  return [
    [
      first[0] * cosine + second[0] * sine,
      first[1] * cosine + second[1] * sine,
      first[2] * cosine + second[2] * sine,
    ],
    [
      second[0] * cosine - first[0] * sine,
      second[1] * cosine - first[1] * sine,
      second[2] * cosine - first[2] * sine,
    ],
  ];
}

function eyePoses(gaze: HeadGaze, scale: number, split = EYE_SPLIT): [EyePose, EyePose] {
  let forward: Vec3 = [0, 0, 1];
  let right: Vec3 = [1, 0, 0];
  let down: Vec3 = [0, 1, 0];

  [forward, right] = spin(forward, right, degrees(gaze.yaw));
  [down, forward] = spin(down, forward, degrees(gaze.pitch));
  [right, down] = spin(right, down, degrees(gaze.roll));

  const build = (side: number): EyePose => {
    const [eyeForward, eyeRight] = spin(forward, right, degrees(split * side));
    return {
      x: eyeForward[0] * scale,
      y: eyeForward[1] * scale,
      a: eyeRight[0],
      b: eyeRight[1],
      c: down[0],
      d: down[1],
      depth: eyeForward[2],
    };
  };

  return [build(-1), build(1)];
}

const blinkRng = createRng(0x5eed);
const blinks: number[] = [];
let nextBlinkAt = 1.4;

function extendBlinkSchedule(through: number): void {
  while (nextBlinkAt <= through + 0.18) {
    blinks.push(nextBlinkAt);
    nextBlinkAt += 1.9 + blinkRng() * 2.7;
    if (blinkRng() < 0.18) {
      blinks.push(nextBlinkAt);
      nextBlinkAt += 0.24;
    }
  }
}

function blinkLid(time: number): number {
  extendBlinkSchedule(time);
  let low = 0;
  let high = blinks.length;
  while (low < high) {
    const middle = (low + high) >>> 1;
    if ((blinks[middle] ?? Number.POSITIVE_INFINITY) <= time) low = middle + 1;
    else high = middle;
  }
  const start = blinks[low - 1];
  if (start === undefined) return 1;
  const progress = (time - start) / 0.18;
  if (progress < 0 || progress > 1) return 1;
  return progress < 0.45 ? 1 - progress / 0.45 : (progress - 0.45) / 0.55;
}

function liveliness(time: number) {
  return {
    yaw: loopNoise(time, 11.3, 0.4) * 5.5 + loopNoise(time, 3.7, 2.1) * 1.6,
    pitch: loopNoise(time, 9.1, 1.3) * 4.2 + loopNoise(time, 4.3, 0.7) * 1.3,
    roll: loopNoise(time, 13.7, 3.2) * 2.2,
    lid: blinkLid(time),
    driftX: loopNoise(time, 7.9, 1.9) * 0.006,
    driftY: loopNoise(time, 5.3, 0.3) * 0.007,
  };
}

const eye = (w: number, h: number, tilt = 0, open = 1): EyeConfig => ({ w, h, tilt, open });
const pair = (w: number, h: number, tilt = 0, open = 1): [EyeConfig, EyeConfig] => [
  eye(w, h, tilt, open),
  eye(w, h, -tilt, open),
];

const expressions: BotExpression[] = [
  { id: "neutre", gaze: { ...REST_GAZE }, split: EYE_SPLIT, eyes: [eye(EYE_W, EYE_H), eye(EYE_W, EYE_H)] },
  { id: "attentif", gaze: { yaw: 4, pitch: 5, roll: -4 }, split: 16, eyes: pair(0.21, 0.44) },
  { id: "surpris", gaze: { yaw: 3, pitch: -3, roll: 0 }, split: 19, eyes: pair(0.45, 0.47) },
  { id: "excite", gaze: { yaw: 6, pitch: -14, roll: 0 }, split: 19.5, eyes: pair(0.4, 0.56, -10) },
  { id: "heureux", gaze: { yaw: 5, pitch: 9, roll: 0 }, split: 17, eyes: pair(0.27, 0.17, 14) },
  { id: "hilare", gaze: { yaw: 4, pitch: 14, roll: 0 }, split: 18, eyes: pair(0.34, 0.13, 20) },
  { id: "colere", gaze: { yaw: 3, pitch: 7, roll: 0 }, split: 17, eyes: pair(0.34, 0.15, 30) },
  { id: "triste", gaze: { yaw: 3, pitch: -13, roll: 0 }, split: 16, eyes: pair(0.22, 0.4, -28) },
  { id: "effraye", gaze: { yaw: 2, pitch: -20, roll: 0 }, split: 20.5, eyes: pair(0.4, 0.6) },
  { id: "mefiant", gaze: { yaw: 12, pitch: 6, roll: -6 }, split: 16, eyes: [eye(0.21, 0.4), eye(0.22, 0.15)] },
  { id: "confus", gaze: { yaw: -14, pitch: 3, roll: 8 }, split: 16.5, eyes: [eye(0.2, 0.44, -18), eye(0.28, 0.17, 14)] },
  { id: "curieux", gaze: { yaw: 16, pitch: -9, roll: -15 }, split: 16.5, eyes: [eye(0.24, 0.46, -8), eye(0.2, 0.38, -8)] },
  { id: "fier", gaze: { yaw: 5, pitch: 17, roll: 0 }, split: 17, eyes: pair(0.3, 0.15, 18) },
  { id: "timide", gaze: { yaw: -19, pitch: -14, roll: -7 }, split: 14, eyes: pair(0.17, 0.3) },
  { id: "blase", gaze: { yaw: -22, pitch: 2, roll: 0 }, split: 16, eyes: pair(0.3, 0.12) },
  { id: "somnolent", gaze: { yaw: 6, pitch: -9, roll: -3 }, split: 16, eyes: pair(0.2, 0.42, 0, 0.42) },
];

const expressionById = new Map(expressions.map((expression) => [expression.id, expression]));

function expressionFor(mood: BloubFaceMood, lookInward: boolean): BotExpression {
  const expression = expressionById.get(mood === "thinking" ? "neutre" : mood) ?? expressions[0]!;
  if (!lookInward) return expression;
  return {
    ...expression,
    gaze: { ...INWARD_GAZE, roll: expression.gaze.roll },
  };
}

function blendEye(from: EyeConfig, to: EyeConfig, amount: number): EyeConfig {
  return {
    w: lerp(from.w, to.w, amount),
    h: lerp(from.h, to.h, amount),
    tilt: lerp(from.tilt ?? 0, to.tilt ?? 0, amount),
    open: lerp(from.open, to.open, amount),
  };
}

function blendExpression(from: BotExpression, to: BotExpression, amount: number): BotExpression {
  return {
    id: to.id,
    gaze: {
      yaw: lerp(from.gaze.yaw, to.gaze.yaw, amount),
      pitch: lerp(from.gaze.pitch, to.gaze.pitch, amount),
      roll: lerp(from.gaze.roll, to.gaze.roll, amount),
    },
    split: lerp(from.split, to.split, amount),
    eyes: [blendEye(from.eyes[0], to.eyes[0], amount), blendEye(from.eyes[1], to.eyes[1], amount)],
  };
}

function capsulePath(width: number, height: number): string {
  const halfWidth = Math.max(width, 0.01) / 2;
  const halfHeight = Math.max(height, 0.01) / 2;
  const radius = Math.min(halfWidth, halfHeight);
  return (
    `M${roundShort(-halfWidth)} ${roundShort(-halfHeight + radius)}`
    + `A${roundShort(radius)} ${roundShort(radius)} 0 0 1 ${roundShort(-halfWidth + radius)} ${roundShort(-halfHeight)}`
    + `L${roundShort(halfWidth - radius)} ${roundShort(-halfHeight)}`
    + `A${roundShort(radius)} ${roundShort(radius)} 0 0 1 ${roundShort(halfWidth)} ${roundShort(-halfHeight + radius)}`
    + `L${roundShort(halfWidth)} ${roundShort(halfHeight - radius)}`
    + `A${roundShort(radius)} ${roundShort(radius)} 0 0 1 ${roundShort(halfWidth - radius)} ${roundShort(halfHeight)}`
    + `L${roundShort(-halfWidth + radius)} ${roundShort(halfHeight)}`
    + `A${roundShort(radius)} ${roundShort(radius)} 0 0 1 ${roundShort(-halfWidth)} ${roundShort(halfHeight - radius)}Z`
  );
}

function sampleEyes(expression: BotExpression, now: number, opacity: number, reduceMotion: boolean): BloubEyeFrame[] {
  if (opacity <= 0.001) return [];
  const life = reduceMotion
    ? { yaw: 0, pitch: 0, roll: 0, lid: 1, driftX: 0, driftY: 0 }
    : liveliness(now / 1000);
  const gaze = {
    yaw: expression.gaze.yaw + life.yaw,
    pitch: expression.gaze.pitch + life.pitch,
    roll: expression.gaze.roll + life.roll,
  };
  const poses = eyePoses(gaze, FACE_SCALE, expression.split);
  const frames: BloubEyeFrame[] = [];

  for (let index = 0; index < 2; index += 1) {
    const pose = poses[index];
    const config = expression.eyes[index];
    if (!pose || !config || pose.depth <= 0.02) continue;
    const angle = degrees(config.tilt ?? 0);
    const cosine = Math.cos(angle);
    const sine = Math.sin(angle);
    const axisX = pose.a * cosine + pose.c * sine;
    const axisY = pose.b * cosine + pose.d * sine;
    const crossX = -pose.a * sine + pose.c * cosine;
    const crossY = -pose.b * sine + pose.d * cosine;
    const lid = 0.06 + 0.94 * clamp(Math.min(life.lid, config.open));
    frames.push({
      d: capsulePath(config.w * FACE_SCALE, config.h * FACE_SCALE),
      matrix: `matrix(${roundShort(axisX)},${roundShort(axisY * lid)},${roundShort(crossX)},${roundShort(crossY * lid)},${roundShort(pose.x + life.driftX * FACE_SCALE)},${roundShort(pose.y + life.driftY * FACE_SCALE)})`,
      opacity: opacity * clamp(pose.depth / 0.12),
    });
  }

  return frames;
}

function dotPulse(time: number, index: number): number {
  const phase = ((((time - index * 0.5) / 1.5) % 1) + 1) % 1;
  const pulse = phase < 0.5 ? 0.5 - 0.5 * Math.cos(phase * TAU) : 0;
  return clamp(pulse * 2);
}

function sampleThinkingDots(now: number, opacity: number, reduceMotion: boolean): BloubDotFrame[] {
  if (opacity <= 0.001) return [];
  const time = reduceMotion ? 1.1 : now / 1000;
  return DOT_X.map((position, index) => {
    const pulse = dotPulse(time, index);
    return {
      x: position * FACE_SCALE,
      y: 0,
      r: DOT_R * (1 + (DOT_PEAK - 1) * pulse) * FACE_SCALE,
      opacity: opacity * (0.55 + 0.45 * pulse),
    };
  });
}

export class BloubFaceEngine {
  private mood: BloubFaceMood;
  private previousMood: BloubFaceMood;
  private lookInward: boolean;
  private fromExpression: BotExpression;
  private targetExpression: BotExpression;
  private changedAt = 0;

  constructor(initialMood: BloubFaceMood = "neutre", lookInward = false) {
    const expression = expressionFor(initialMood, lookInward);
    this.mood = initialMood;
    this.previousMood = initialMood;
    this.lookInward = lookInward;
    this.fromExpression = expression;
    this.targetExpression = expression;
  }

  setMood(mood: BloubFaceMood, now: number, lookInward = false): void {
    if (mood === this.mood && lookInward === this.lookInward) return;
    const currentExpression = this.expressionAt(now);
    this.previousMood = this.mood;
    this.mood = mood;
    this.lookInward = lookInward;
    this.fromExpression = currentExpression;
    if (mood !== "thinking") {
      this.targetExpression = expressionFor(mood, lookInward);
    }
    this.changedAt = now;
  }

  sample(now: number, reduceMotion = false): BloubFaceFrame {
    const amount = reduceMotion ? 1 : easeOutQuint(clamp((now - this.changedAt) / MORPH_DURATION));
    const enteringThinking = this.mood === "thinking";
    const leavingThinking = this.previousMood === "thinking" && !enteringThinking;
    const eyeOpacity = enteringThinking ? 1 - amount : leavingThinking ? amount : 1;
    const dotOpacity = enteringThinking ? amount : leavingThinking ? 1 - amount : 0;
    const expression = enteringThinking ? this.fromExpression : this.expressionAt(now, reduceMotion);
    return {
      eyes: sampleEyes(expression, now, eyeOpacity, reduceMotion),
      dots: sampleThinkingDots(now, dotOpacity, reduceMotion),
    };
  }

  private expressionAt(now: number, reduceMotion = false): BotExpression {
    if (this.mood === "thinking") return this.fromExpression;
    const amount = reduceMotion ? 1 : easeOutQuint(clamp((now - this.changedAt) / MORPH_DURATION));
    return blendExpression(this.fromExpression, this.targetExpression, amount);
  }
}
