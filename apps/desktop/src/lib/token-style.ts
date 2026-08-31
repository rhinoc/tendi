export enum TokenTone {
  Tiny = "tiny",
  Small = "small",
  Medium = "medium",
  Large = "large",
  Huge = "huge",
  Massive = "massive",
}

export function tokenTone(value: number): TokenTone {
  if (value <= 200) return TokenTone.Tiny;
  if (value <= 1000) return TokenTone.Small;
  if (value <= 10000) return TokenTone.Medium;
  if (value <= 100000) return TokenTone.Large;
  if (value <= 200000) return TokenTone.Huge;
  return TokenTone.Massive;
}

export function tokenToneClass(value: number): string {
  return `tokenTone-${tokenTone(value)}`;
}

export function cacheRateTone(rate: number): TokenTone {
  if (rate >= 90) return TokenTone.Tiny;
  if (rate >= 70) return TokenTone.Small;
  if (rate >= 50) return TokenTone.Medium;
  if (rate >= 30) return TokenTone.Large;
  if (rate >= 10) return TokenTone.Huge;
  return TokenTone.Massive;
}
