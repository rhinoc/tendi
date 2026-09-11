export type CommandPaletteSearchItem = {
  label: string;
  detail?: string;
  group: string;
  keywords?: readonly string[];
};

export const MAX_COMMAND_PALETTE_RESULTS = 512;

export function fuzzyMatch(needle: string, haystack: string): boolean {
  if (!needle) return true;
  let needleIndex = 0;
  const normalizedNeedle = needle.toLowerCase();
  for (const character of haystack.toLowerCase()) {
    if (character === normalizedNeedle[needleIndex]) needleIndex += 1;
    if (needleIndex === normalizedNeedle.length) return true;
  }
  return false;
}

export function filterCommandPaletteItems<T extends CommandPaletteSearchItem>(
  items: readonly T[],
  query: string,
): T[] {
  const normalizedQuery = query.trim();
  if (!normalizedQuery) return items.slice(0, MAX_COMMAND_PALETTE_RESULTS);

  const lowerQuery = normalizedQuery.toLowerCase();
  const scored = items.flatMap((item, index) => {
    const values = [item.label, item.detail ?? "", item.group, ...(item.keywords ?? [])];
    const match = values.find((value) => fuzzyMatch(normalizedQuery, value));
    if (!match) return [];
    const lowerMatch = match.toLowerCase();
    const score = lowerMatch === lowerQuery
      ? 3_000_000
      : lowerMatch.startsWith(lowerQuery)
        ? 2_000_000 - match.length
        : lowerMatch.includes(lowerQuery)
          ? 1_000_000 - lowerMatch.indexOf(lowerQuery)
          : 100_000 - match.length;
    return [{ item, score, index }];
  });
  scored.sort((left, right) => right.score - left.score || left.index - right.index);
  return scored.slice(0, MAX_COMMAND_PALETTE_RESULTS).map(({ item }) => item);
}
