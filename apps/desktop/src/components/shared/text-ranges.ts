export type TextRange = {
  from: number;
  to: number;
};

const SEARCH_CHUNK_SIZE = 64 * 1024;

export function findTextRanges(text: string, query: string): TextRange[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const source = `${text ?? ""}`;
  const ranges: TextRange[] = [];
  const chunkSize = Math.max(SEARCH_CHUNK_SIZE, needle.length);
  const overlap = Math.max(needle.length - 1, 0);

  for (let start = 0; start < source.length;) {
    const end = Math.min(source.length, start + chunkSize);
    const haystack = source.slice(start, Math.min(source.length, end + overlap)).toLowerCase();
    let index = haystack.indexOf(needle);
    while (index >= 0) {
      const absolute = start + index;
      if (absolute < end) ranges.push({ from: absolute, to: absolute + needle.length });
      index = haystack.indexOf(needle, index + needle.length);
    }
    if (end === source.length) break;
    start = end;
  }
  return ranges;
}
