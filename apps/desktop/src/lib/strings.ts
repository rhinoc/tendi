export function basename(value: unknown): string {
  return `${value ?? ""}`.split("/").filter(Boolean).pop() || `${value ?? ""}`;
}

export function titleValue(value: unknown): string {
  if (value === null || value === undefined || value === "") return "Unknown";
  return `${value}`.slice(0, 1).toUpperCase() + `${value}`.slice(1);
}

export function compactCommand(value: unknown): string {
  const text = Array.isArray(value) ? value.join(" ") : `${value ?? ""}`;
  return text.length > 90 ? `${text.slice(0, 87)}...` : text;
}

export function formatUserPath(value: unknown): string {
  const path = `${value ?? ""}`;
  const homePrefix = path.match(/^(?:\/(?:Users|home)\/[^/]+|\/root|[A-Za-z]:[\\/]Users[\\/][^\\/]+)(?=[\\/]|$)/i);
  return homePrefix ? `~${path.slice(homePrefix[0].length)}` : path;
}

function parseDateTimeParts(value: unknown) {
  const text = `${value ?? ""}`.trim();
  if (!text) return null;

  const partsFromDate = (date: Date) => {
    if (Number.isNaN(date.getTime())) return null;
    return {
      year: date.getFullYear(),
      month: date.getMonth() + 1,
      day: date.getDate(),
      hour: String(date.getHours()).padStart(2, "0"),
      minute: String(date.getMinutes()).padStart(2, "0"),
    };
  };

  if (/^\d+$/.test(text)) return partsFromDate(new Date(Number(text) * 1000));

  const match = text.match(/^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?/);
  if (!match) return null;
  const [, year, month, day, hour = "00", minute = "00", second = "00", zone] = match;

  if (zone) {
    const normalized = text.replace(/([+-]\d{2})(\d{2})$/, "$1:$2");
    return partsFromDate(new Date(normalized));
  }

  return partsFromDate(new Date(Number(year), Number(month) - 1, Number(day), Number(hour), Number(minute), Number(second)));
}

export function compactDateTime(value: unknown, options: { year?: boolean } = {}): string {
  const parts = parseDateTimeParts(value);
  if (parts) {
    const { year, month, day, hour, minute } = parts;
    if (options.year) {
      const monthLabel = String(month).padStart(2, "0");
      const dayLabel = String(day).padStart(2, "0");
      return `${year}-${monthLabel}-${dayLabel} ${hour}:${minute}`;
    }
    return `${month}/${day} ${hour}:${minute}`;
  }

  const text = `${value ?? ""}`.trim();
  if (!text) return "";
  const normalized = text.replace(/,\s+/g, " ");
  return normalized.length > 16 ? normalized.slice(0, 16) : normalized;
}

export function dayGroupKey(value: unknown): string {
  const parts = parseDateTimeParts(value);
  if (!parts) return "";
  const { year, month, day } = parts;
  return `${year}-${String(month).padStart(2, "0")}-${String(day).padStart(2, "0")}`;
}

export function formatDayGroupLabel(key: unknown): string {
  if (!key) return "Unknown";
  const match = `${key}`.match(/^(\d{4})-(\d{2})-(\d{2})$/);
  if (!match) return `${key}`;
  const [, year, month, day] = match;
  return `${Number(year)}/${Number(month)}/${Number(day)}`;
}

export function formatDuration(value: unknown): string {
  if (value === null || value === undefined || value === "") return "";
  if (typeof value === "string" && !/^\d+(\.\d+)?$/.test(value.trim())) return value;
  const ms = Number(value);
  if (!Number.isFinite(ms)) return "";
  if (ms < 1000) return `${Math.max(0, Math.round(ms))} ms`;
  if (ms < 10_000) return `${(ms / 1000).toFixed(1)} s`;
  return `${Math.round(ms / 1000)} s`;
}
