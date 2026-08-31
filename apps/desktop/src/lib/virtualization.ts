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
