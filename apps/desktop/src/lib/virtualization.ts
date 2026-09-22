export type VirtualizationContract = {
  datasetEpoch: string;
  stableKey: string;
  count: number;
  estimate: number;
  measured?: readonly number[];
  scrollOffset: number;
  viewportSize: number;
  overscan: number;
};

export type VirtualRange = { start: number; end: number };

function lowerBound(values: readonly number[], target: number, start: number, end: number) {
  let low = Math.max(0, start);
  let high = Math.min(values.length, end);
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if ((values[middle] ?? 0) < target) low = middle + 1;
    else high = middle;
  }
  return low;
}

function upperBound(values: readonly number[], target: number, start: number, end: number) {
  let low = Math.max(0, start);
  let high = Math.min(values.length, end);
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if ((values[middle] ?? 0) <= target) low = middle + 1;
    else high = middle;
  }
  return low;
}

/**
 * Finds a virtual window from monotonically increasing item-end offsets.
 * Unlike a measured-item scan, this stays logarithmic as the data set grows.
 */
export function variableVirtualRangeFor(
  offsets: readonly number[],
  scrollOffset: number,
  viewportSize: number,
  estimate: number,
  overscan: number,
): VirtualRange {
  const count = Math.max(0, offsets.length - 1);
  if (count === 0) return { start: 0, end: 0 };

  const itemSize = Math.max(1, estimate);
  const viewport = Math.max(0, viewportSize);
  const buffer = Math.max(0, Math.floor(overscan)) * itemSize;
  const totalSize = Math.max(0, offsets[count] ?? 0);
  const boundedOffset = Math.min(
    Math.max(0, Number.isFinite(scrollOffset) ? scrollOffset : 0),
    Math.max(0, totalSize - viewport),
  );
  const bufferedStart = Math.max(0, boundedOffset - buffer);
  const bufferedEnd = Math.min(totalSize, boundedOffset + viewport + buffer);
  const start = Math.min(count, Math.max(0, upperBound(offsets, bufferedStart, 1, count + 1) - 1));
  const end = Math.min(count, Math.max(start, lowerBound(offsets, bufferedEnd, start + 1, count + 1)));
  return { start, end };
}

export function virtualRangeFor(contract: VirtualizationContract): VirtualRange {
  const count = Math.max(0, Math.floor(contract.count));
  const itemSize = Math.max(1, contract.estimate);
  const viewportSize = Math.max(0, contract.viewportSize);
  const overscan = Math.max(0, Math.floor(contract.overscan));
  const measured = contract.measured && contract.measured.length >= count
    ? contract.measured
    : undefined;
  if (measured) {
    const sizeAt = (index: number) => Math.max(1, measured[index] ?? itemSize);
    let totalSize = 0;
    for (let index = 0; index < count; index += 1) totalSize += sizeAt(index);
    const boundedOffset = Math.min(Math.max(0, contract.scrollOffset), Math.max(0, totalSize - viewportSize));
    const bufferedStart = Math.max(0, boundedOffset - overscan * itemSize);
    const bufferedEnd = boundedOffset + viewportSize + overscan * itemSize;
    let cursor = 0;
    let start = 0;
    while (start < count && cursor + sizeAt(start) <= bufferedStart) {
      cursor += sizeAt(start);
      start += 1;
    }
    let end = start;
    while (end < count && cursor < bufferedEnd) {
      cursor += sizeAt(end);
      end += 1;
    }
    return {
      start: Math.min(start, end),
      end: Math.max(start, end),
    };
  }
  const boundedOffset = Math.min(
    Math.max(0, contract.scrollOffset),
    Math.max(0, count * itemSize - viewportSize),
  );
  const start = Math.max(0, Math.floor(boundedOffset / itemSize) - overscan);
  const end = Math.min(count, Math.ceil((boundedOffset + viewportSize) / itemSize) + overscan);
  return {
    start: Math.min(start, end),
    end: Math.max(start, end),
  };
}

export function fixedVirtualRange(
  count: number,
  scrollOffset: number,
  viewportSize: number,
  itemSize: number,
  overscan: number,
) {
  return virtualRangeFor({
    datasetEpoch: "fixed",
    stableKey: "index",
    count,
    estimate: itemSize,
    scrollOffset,
    viewportSize,
    overscan,
  });
}
