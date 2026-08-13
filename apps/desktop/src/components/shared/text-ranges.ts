export type TextRange = {
  from: number;
  to: number;
};

export function findTextRanges(text: string, query: string): TextRange[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const haystack = `${text ?? ""}`.toLowerCase();
  const ranges: TextRange[] = [];
  let index = haystack.indexOf(needle);
  while (index >= 0) {
    ranges.push({ from: index, to: index + needle.length });
    index = haystack.indexOf(needle, index + needle.length);
  }
  return ranges;
}
