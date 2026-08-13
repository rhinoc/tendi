export type TokenTone = "tiny" | "small" | "medium" | "large" | "huge" | "massive";

export function tokenTone(value: number): TokenTone {
  if (value <= 200) return "tiny";
  if (value <= 1000) return "small";
  if (value <= 10000) return "medium";
  if (value <= 100000) return "large";
  if (value <= 200000) return "huge";
  return "massive";
}

export function tokenToneClass(value: number): string {
  return `tokenTone-${tokenTone(value)}`;
}

export function cacheRateTone(rate: number): TokenTone {
  if (rate >= 90) return "tiny";
  if (rate >= 70) return "small";
  if (rate >= 50) return "medium";
  if (rate >= 30) return "large";
  if (rate >= 10) return "huge";
  return "massive";
}
