export function textMatchRank(value: unknown, query: string): number {
  const text = `${value ?? ""}`.toLowerCase();
  if (!text || !query || !text.includes(query)) return 0;
  if (text === query) return 4;
  if (text.startsWith(query)) return 3;
  return 2;
}

export function boostedTextMatchRank(value: unknown, query: string, boost: number): number {
  const rank = textMatchRank(value, query);
  return rank ? rank + boost : 0;
}
