export function timestampMs(value: unknown): number | undefined {
  if (typeof value === "number") return Number.isFinite(value) ? value : undefined;
  const time = Date.parse(String(value ?? ""));
  return Number.isFinite(time) ? time : undefined;
}

export function compareTimestamps(left: unknown, right: unknown): number {
  const leftMs = timestampMs(left);
  const rightMs = timestampMs(right);
  if (leftMs !== undefined && rightMs !== undefined) return leftMs - rightMs;
  if (leftMs !== undefined) return 1;
  if (rightMs !== undefined) return -1;
  return String(left ?? "").localeCompare(String(right ?? ""));
}
