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

export function splitSearchQueryTerms(query: string): string[] {
  return query.trim().split(/\s+/).filter(Boolean);
}

export function findTextRangesForQueryTerms(text: string, terms: readonly string[]): TextRange[] {
  const ranges = terms.flatMap((term) => findTextRanges(text, term));
  return ranges
    .sort((left, right) => left.from - right.from || right.to - left.to)
    .reduce<TextRange[]>((merged, range) => {
      const previous = merged.at(-1);
      if (!previous || range.from >= previous.to) {
        merged.push(range);
      } else {
        previous.to = Math.max(previous.to, range.to);
      }
      return merged;
    }, []);
}
