export type TextRange = {
  from: number;
  to: number;
};

const SEARCH_CHUNK_SIZE = 64 * 1024;

function escapeRegExp(value: string) {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

export function findTextRanges(text: string, query: string): TextRange[] {
  const needle = query.trim();
  if (!needle) return [];
  const source = `${text ?? ""}`;
  const ranges: TextRange[] = [];
  const chunkSize = Math.max(SEARCH_CHUNK_SIZE, needle.length);
  const overlap = Math.max(needle.length - 1, 0);
  const matcher = new RegExp(escapeRegExp(needle), "giu");

  for (let start = 0; start < source.length;) {
    const end = Math.min(source.length, start + chunkSize);
    const haystack = source.slice(start, Math.min(source.length, end + overlap));
    matcher.lastIndex = 0;
    let match = matcher.exec(haystack);
    while (match) {
      const absolute = start + match.index;
      if (absolute < end) ranges.push({ from: absolute, to: absolute + match[0].length });
      if (match[0].length === 0) matcher.lastIndex += 1;
      match = matcher.exec(haystack);
    }
    if (end === source.length) break;
    start = end;
  }
  return ranges;
}
