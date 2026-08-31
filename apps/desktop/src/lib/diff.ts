export enum DiffLineKind {
  Unchanged = "",
  Removed = "removed",
  Added = "added",
}

export type DiffSegment = { text: string; changed: boolean };

export type DiffLine = {
  kind: DiffLineKind;
  text: string;
  segments?: DiffSegment[];
};

enum SequenceEditKind {
  Equal = "equal",
  Removed = "removed",
  Added = "added",
}

enum MergeHunkSource {
  Local = "local",
  Incoming = "incoming",
}

type SequenceEdit<T> = {
  kind: SequenceEditKind;
  value: T;
};

type SequenceDiffResult<T> = {
  edits: SequenceEdit<T>[];
  bounded: boolean;
};

// Keep the diff path responsive for large files. Small edits still use the
// shortest edit script; unrelated large files get a coarse replacement.
const MAX_DIFF_WORK = 1_000_000;
const MAX_INLINE_DIFF_WORK = 200_000;
const MAX_DIFF_ITEMS = 4_000;

function commonPrefixLength<T>(before: readonly T[], after: readonly T[], equals: (left: T, right: T) => boolean) {
  const length = Math.min(before.length, after.length);
  let index = 0;
  while (index < length && equals(before[index], after[index])) index += 1;
  return index;
}

function commonSuffixLength<T>(
  before: readonly T[],
  after: readonly T[],
  prefixLength: number,
  equals: (left: T, right: T) => boolean,
) {
  const length = Math.min(before.length, after.length) - prefixLength;
  let index = 0;
  while (
    index < length
    && equals(before[before.length - index - 1], after[after.length - index - 1])
  ) index += 1;
  return index;
}

function appendEqual<T>(edits: SequenceEdit<T>[], values: readonly T[], start: number, end: number) {
  for (let index = start; index < end; index += 1) {
    edits.push({ kind: SequenceEditKind.Equal, value: values[index] });
  }
}

function normalizeChangedEditOrder<T>(edits: SequenceEdit<T>[]) {
  const normalized: SequenceEdit<T>[] = [];
  let index = 0;
  while (index < edits.length) {
    if (edits[index].kind === SequenceEditKind.Equal) {
      normalized.push(edits[index]);
      index += 1;
      continue;
    }
    let end = index;
    while (end < edits.length && edits[end].kind !== SequenceEditKind.Equal) end += 1;
    normalized.push(...edits.slice(index, end).filter((edit) => edit.kind === SequenceEditKind.Removed));
    normalized.push(...edits.slice(index, end).filter((edit) => edit.kind === SequenceEditKind.Added));
    index = end;
  }
  return normalized;
}

function coarseSequenceDiff<T>(
  before: readonly T[],
  after: readonly T[],
  prefixLength: number,
  suffixLength: number,
): SequenceDiffResult<T> {
  const edits: SequenceEdit<T>[] = [];
  appendEqual(edits, before, 0, prefixLength);
  for (let index = prefixLength; index < before.length - suffixLength; index += 1) {
    edits.push({ kind: SequenceEditKind.Removed, value: before[index] });
  }
  for (let index = prefixLength; index < after.length - suffixLength; index += 1) {
    edits.push({ kind: SequenceEditKind.Added, value: after[index] });
  }
  appendEqual(edits, before, before.length - suffixLength, before.length);
  return { edits, bounded: true };
}

function sequenceDiff<T>(
  before: readonly T[],
  after: readonly T[],
  equals: (left: T, right: T) => boolean,
  maxWork = MAX_DIFF_WORK,
): SequenceDiffResult<T> {
  if (before.length === 0 && after.length === 0) return { edits: [], bounded: false };
  if (before.length === 0) {
    return { edits: after.map((value) => ({ kind: SequenceEditKind.Added, value })), bounded: false };
  }
  if (after.length === 0) {
    return { edits: before.map((value) => ({ kind: SequenceEditKind.Removed, value })), bounded: false };
  }

  const prefixLength = commonPrefixLength(before, after, equals);
  const suffixLength = commonSuffixLength(before, after, prefixLength, equals);
  const beforeMiddleLength = before.length - prefixLength - suffixLength;
  const afterMiddleLength = after.length - prefixLength - suffixLength;
  if (beforeMiddleLength === 0 || afterMiddleLength === 0) {
    return coarseSequenceDiff(before, after, prefixLength, suffixLength);
  }
  if (
    Math.max(beforeMiddleLength, afterMiddleLength) > MAX_DIFF_ITEMS
    || beforeMiddleLength * afterMiddleLength > maxWork
  ) {
    return coarseSequenceDiff(before, after, prefixLength, suffixLength);
  }

  const max = beforeMiddleLength + afterMiddleLength;
  const offset = max + 1;
  let vector = new Int32Array(max * 2 + 3);
  vector[offset + 1] = 0;
  const trace: Int32Array[] = [];
  let endDistance = 0;
  let endX = 0;

  search: for (let distance = 0; distance <= max; distance += 1) {
    trace.push(vector.slice());
    for (let diagonal = -distance; diagonal <= distance; diagonal += 2) {
      let x: number;
      if (
        diagonal === -distance
        || (diagonal !== distance && vector[offset + diagonal - 1] < vector[offset + diagonal + 1])
      ) {
        x = vector[offset + diagonal + 1];
      } else {
        x = vector[offset + diagonal - 1] + 1;
      }
      let y = x - diagonal;
      while (
        x < beforeMiddleLength
        && y < afterMiddleLength
        && equals(before[prefixLength + x], after[prefixLength + y])
      ) {
        x += 1;
        y += 1;
      }
      vector[offset + diagonal] = x;
      if (x >= beforeMiddleLength && y >= afterMiddleLength) {
        endDistance = distance;
        endX = x;
        break search;
      }
    }
  }

  const middleEdits: SequenceEdit<T>[] = [];
  let x = endX;
  let y = endX - (beforeMiddleLength - afterMiddleLength);
  for (let distance = endDistance; distance > 0; distance -= 1) {
    const previous = trace[distance];
    const diagonal = x - y;
    const previousDiagonal = (
      diagonal === -distance
      || (diagonal !== distance && previous[offset + diagonal - 1] < previous[offset + diagonal + 1])
    ) ? diagonal + 1 : diagonal - 1;
    const previousX = previous[offset + previousDiagonal];
    const previousY = previousX - previousDiagonal;
    while (x > previousX && y > previousY) {
      middleEdits.push({ kind: SequenceEditKind.Equal, value: before[prefixLength + x - 1] });
      x -= 1;
      y -= 1;
    }
    if (x === previousX) {
      middleEdits.push({ kind: SequenceEditKind.Added, value: after[prefixLength + y - 1] });
      y -= 1;
    } else {
      middleEdits.push({ kind: SequenceEditKind.Removed, value: before[prefixLength + x - 1] });
      x -= 1;
    }
  }
  while (x > 0 && y > 0) {
    middleEdits.push({ kind: SequenceEditKind.Equal, value: before[prefixLength + x - 1] });
    x -= 1;
    y -= 1;
  }
  while (x > 0) {
    middleEdits.push({ kind: SequenceEditKind.Removed, value: before[prefixLength + x - 1] });
    x -= 1;
  }
  while (y > 0) {
    middleEdits.push({ kind: SequenceEditKind.Added, value: after[prefixLength + y - 1] });
    y -= 1;
  }
  middleEdits.reverse();

  const edits: SequenceEdit<T>[] = [];
  appendEqual(edits, before, 0, prefixLength);
  edits.push(...middleEdits);
  appendEqual(edits, before, before.length - suffixLength, before.length);
  return { edits: normalizeChangedEditOrder(edits), bounded: false };
}

function markChangedLines(lines: DiffLine[]) {
  for (const line of lines) {
    if (line.kind) line.segments = [{ text: line.text, changed: true }];
  }
  return lines;
}

export function inlineDiffSegments(before: string, after: string): { removed: DiffSegment[]; added: DiffSegment[] } {
  const beforeChars = [...before];
  const afterChars = [...after];
  const removed: DiffSegment[] = [];
  const added: DiffSegment[] = [];
  const pushSegment = (segments: DiffSegment[], text: string, changed: boolean) => {
    if (!text) return;
    const last = segments[segments.length - 1];
    if (last?.changed === changed) last.text += text;
    else segments.push({ text, changed });
  };
  for (const edit of sequenceDiff(beforeChars, afterChars, Object.is, MAX_INLINE_DIFF_WORK).edits) {
    if (edit.kind === SequenceEditKind.Equal) {
      pushSegment(removed, edit.value, false);
      pushSegment(added, edit.value, false);
    } else if (edit.kind === SequenceEditKind.Removed) {
      pushSegment(removed, edit.value, true);
    } else {
      pushSegment(added, edit.value, true);
    }
  }
  return { removed, added };
}

export function inlineCommonCount(before: string, after: string): number {
  const beforeChars = [...before];
  const afterChars = [...after];
  const work = beforeChars.length * afterChars.length;
  if (work > MAX_INLINE_DIFF_WORK) {
    const prefix = commonPrefixLength(beforeChars, afterChars, Object.is);
    const suffix = commonSuffixLength(beforeChars, afterChars, prefix, Object.is);
    return prefix + suffix;
  }
  const shorter = beforeChars.length <= afterChars.length ? beforeChars : afterChars;
  const longer = beforeChars.length <= afterChars.length ? afterChars : beforeChars;
  const table = new Uint32Array(shorter.length + 1);
  for (const value of longer) {
    let diagonal = 0;
    for (let index = 1; index <= shorter.length; index += 1) {
      const previous = table[index];
      table[index] = value === shorter[index - 1]
        ? diagonal + 1
        : Math.max(table[index], table[index - 1]);
      diagonal = previous;
    }
  }
  return table[shorter.length];
}

export function shouldPairInlineDiff(before: string, after: string, commonCount: number): boolean {
  const longest = Math.max([...before].length, [...after].length);
  if (longest === 0) return true;
  return commonCount >= Math.min(4, longest) || commonCount / longest >= 0.35;
}

export function addInlineDiffSegments(lines: DiffLine[]): DiffLine[] {
  const result = lines.map((line) => ({ ...line }));
  for (let index = 0; index < result.length; index += 1) {
    if (!result[index].kind) continue;
    let cursor = index;
    while (result[cursor]?.kind) cursor += 1;
    const hunk = result.slice(index, cursor);
    const removedLines = hunk.filter((line) => line.kind === DiffLineKind.Removed);
    const unmatchedAddedLines = hunk.filter((line) => line.kind === DiffLineKind.Added);

    for (const removedLine of removedLines) {
      let bestMatch: DiffLine | null = null;
      let bestCommonCount = 0;
      for (const addedLine of unmatchedAddedLines) {
        const commonCount = inlineCommonCount(removedLine.text, addedLine.text);
        if (commonCount > bestCommonCount) {
          bestMatch = addedLine;
          bestCommonCount = commonCount;
        }
      }
      if (bestMatch && shouldPairInlineDiff(removedLine.text, bestMatch.text, bestCommonCount)) {
        const segments = inlineDiffSegments(removedLine.text, bestMatch.text);
        removedLine.segments = segments.removed;
        bestMatch.segments = segments.added;
        unmatchedAddedLines.splice(unmatchedAddedLines.indexOf(bestMatch), 1);
      }
    }
    for (const line of hunk) {
      if (!line.segments) line.segments = [{ text: line.text, changed: true }];
    }
    index = cursor - 1;
  }
  for (const line of result) {
    if (line.kind && !line.segments) line.segments = [{ text: line.text, changed: true }];
  }
  return result;
}

export function diffPreview(before: string, after: string): DiffLine[] {
  const beforeLines = before.split("\n");
  const afterLines = after.split("\n");
  if (before === after) return afterLines.map((text) => ({ kind: DiffLineKind.Unchanged, text }));

  const result = sequenceDiff(beforeLines, afterLines, Object.is);
  const lines = result.edits.map((edit) => ({
    kind: edit.kind === SequenceEditKind.Equal ? DiffLineKind.Unchanged : edit.kind === SequenceEditKind.Removed ? DiffLineKind.Removed : DiffLineKind.Added,
    text: edit.value,
  } as DiffLine));
  return result.bounded ? markChangedLines(lines) : addInlineDiffSegments(lines);
}

export function currentLineDiffMap(before: string, after: string) {
  const diffLines = diffPreview(before, after);
  const lineStates = new Map<number, { kind: string; segments: DiffSegment[] }>();
  const removedBefore = new Map<number, DiffLine[]>();
  let currentLine = 1;
  let pendingRemoved: DiffLine[] = [];

  for (const line of diffLines) {
    if (line.kind === DiffLineKind.Removed) {
      pendingRemoved.push(line);
      continue;
    }
    if (pendingRemoved.length) {
      removedBefore.set(currentLine, pendingRemoved);
      pendingRemoved = [];
    }
    if (line.kind) {
      lineStates.set(currentLine, {
        kind: line.kind,
        segments: line.segments ?? [{ text: line.text, changed: Boolean(line.kind) }],
      });
    }
    currentLine += 1;
  }
  if (pendingRemoved.length) {
    removedBefore.set(currentLine, pendingRemoved);
  }

  return { lineStates, removedBefore };
}

type MergeHunk = {
  start: number;
  end: number;
  replacement: string[];
};

export type ThreeWayMergeResult = {
  content: string;
  hasConflicts: boolean;
};

function changedHunks(base: string[], variant: string): MergeHunk[] {
  const hunks: MergeHunk[] = [];
  let baseIndex = 0;
  let start = -1;
  let replacement: string[] = [];
  const finish = () => {
    if (start < 0) return;
    hunks.push({ start, end: baseIndex, replacement });
    start = -1;
    replacement = [];
  };
  for (const line of diffPreview(base.join("\n"), variant)) {
    if (line.kind === DiffLineKind.Unchanged) {
      finish();
      baseIndex += 1;
    } else if (line.kind === DiffLineKind.Removed) {
      if (start < 0) start = baseIndex;
      baseIndex += 1;
    } else {
      if (start < 0) start = baseIndex;
      replacement.push(line.text);
    }
  }
  finish();
  return hunks;
}

function hunksOverlap(left: MergeHunk, right: MergeHunk) {
  if (left.start === left.end && right.start === right.end) return left.start === right.start;
  if (left.start === left.end) return left.start >= right.start && left.start <= right.end;
  if (right.start === right.end) return right.start >= left.start && right.start <= left.end;
  return left.start < right.end && right.start < left.end;
}

function mergeHunkGroups(local: MergeHunk[], incoming: MergeHunk[]) {
  const all = [
    ...local.map((hunk) => ({ ...hunk, source: MergeHunkSource.Local })),
    ...incoming.map((hunk) => ({ ...hunk, source: MergeHunkSource.Incoming })),
  ].sort((left, right) => left.start - right.start || left.end - right.end);
  const groups: typeof all[number][][] = [];
  for (const hunk of all) {
    const group = groups[groups.length - 1];
    if (!group || !group.some((member) => hunksOverlap(member, hunk))) {
      groups.push([hunk]);
    } else {
      group.push(hunk);
    }
  }
  return groups;
}

function renderHunks(base: string[], start: number, end: number, hunks: MergeHunk[]) {
  const relevant = hunks
    .filter((hunk) => hunk.start >= start && hunk.end <= end)
    .sort((left, right) => left.start - right.start || left.end - right.end);
  const result: string[] = [];
  let cursor = start;
  for (const hunk of relevant) {
    result.push(...base.slice(cursor, hunk.start));
    result.push(...hunk.replacement);
    cursor = hunk.end;
  }
  result.push(...base.slice(cursor, end));
  return result;
}

export function mergeThreeWay(baseContent: string, localContent: string, incomingContent: string): ThreeWayMergeResult {
  if (localContent === incomingContent) return { content: localContent, hasConflicts: false };
  if (localContent === baseContent) return { content: incomingContent, hasConflicts: false };
  if (incomingContent === baseContent) return { content: localContent, hasConflicts: false };

  const base = baseContent.split("\n");
  const localHunks = changedHunks(base, localContent);
  const incomingHunks = changedHunks(base, incomingContent);
  const groups = mergeHunkGroups(localHunks, incomingHunks);
  const merged: string[] = [];
  let cursor = 0;
  let hasConflicts = false;

  for (const group of groups) {
    const start = Math.min(...group.map((hunk) => hunk.start));
    const end = Math.max(...group.map((hunk) => hunk.end));
    merged.push(...base.slice(cursor, start));
    const belongsToGroup = (hunk: MergeHunk, source: MergeHunkSource) => group.some(
      (member) => member.source === source
        && member.start === hunk.start
        && member.end === hunk.end
        && member.replacement.join("\n") === hunk.replacement.join("\n"),
    );
    const local = renderHunks(base, start, end, localHunks.filter((hunk) => belongsToGroup(hunk, MergeHunkSource.Local)));
    const incoming = renderHunks(base, start, end, incomingHunks.filter((hunk) => belongsToGroup(hunk, MergeHunkSource.Incoming)));
    const original = base.slice(start, end);
    if (local.join("\n") === incoming.join("\n")) {
      merged.push(...local);
    } else if (local.join("\n") === original.join("\n")) {
      merged.push(...incoming);
    } else if (incoming.join("\n") === original.join("\n")) {
      merged.push(...local);
    } else {
      hasConflicts = true;
      merged.push(
        "<<<<<<< local",
        ...local,
        "||||||| base",
        ...original,
        "=======",
        ...incoming,
        ">>>>>>> incoming",
      );
    }
    cursor = end;
  }
  merged.push(...base.slice(cursor));
  return { content: merged.join("\n"), hasConflicts };
}
