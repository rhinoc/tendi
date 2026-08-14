export function formatTokenCount(value: number): string {
  if (!Number.isFinite(value)) return "0";
  if (value < 1000) return `${value}`;
  if (value < 10000) return `${(value / 1000).toFixed(1).replace(/\.0$/, "")}k`;
  if (value < 1000000) return `${Math.round(value / 1000)}k`;
  if (value < 10000000) return `${(value / 1000000).toFixed(1).replace(/\.0$/, "")}M`;
  return `${Math.round(value / 1000000)}M`;
}
