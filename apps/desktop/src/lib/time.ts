export function timestampMs(value: unknown): number | undefined {
  if (typeof value === "number") return Number.isFinite(value) ? value : undefined;
  const time = Date.parse(String(value ?? ""));
  return Number.isFinite(time) ? time : undefined;
}

export type DisplayDateTimeParts = {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
};

let cachedUserDateTimeFormatter: { timeZone: string; formatter: Intl.DateTimeFormat } | undefined;

function displayDate(value: unknown): Date | undefined {
  const text = `${value ?? ""}`.trim();
  if (!text) return undefined;

  if (/^\d+$/.test(text)) {
    const date = new Date(Number(text) * 1000);
    return Number.isNaN(date.getTime()) ? undefined : date;
  }

  const match = text.match(/^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2})(?:\.\d+)?)?(Z|[+-]\d{2}:?\d{2})?)?/);
  if (!match) return undefined;
  const [, year, month, day, hour = "00", minute = "00", second = "00", zone] = match;
  const date = zone
    ? new Date(text.replace(/([+-]\d{2})(\d{2})$/, "$1:$2"))
    : new Date(Number(year), Number(month) - 1, Number(day), Number(hour), Number(minute), Number(second));
  return Number.isNaN(date.getTime()) ? undefined : date;
}

function userDateTimeParts(date: Date): DisplayDateTimeParts {
  const timeZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  if (!cachedUserDateTimeFormatter || cachedUserDateTimeFormatter.timeZone !== timeZone) {
    cachedUserDateTimeFormatter = {
      timeZone,
      formatter: new Intl.DateTimeFormat("en-US", {
        day: "2-digit",
        hour: "2-digit",
        hourCycle: "h23",
        minute: "2-digit",
        month: "2-digit",
        timeZone,
        year: "numeric",
      }),
    };
  }
  const parts = cachedUserDateTimeFormatter.formatter.formatToParts(date);
  const values = Object.fromEntries(parts.map((part) => [part.type, part.value]));
  return {
    year: Number(values.year),
    month: Number(values.month),
    day: Number(values.day),
    hour: Number(values.hour),
    minute: Number(values.minute),
  };
}

export function displayDateTimeParts(value: unknown): DisplayDateTimeParts | undefined {
  const date = displayDate(value);
  return date ? userDateTimeParts(date) : undefined;
}

export function formatLocalTime(value: unknown): string {
  const parts = displayDateTimeParts(value);
  return parts ? `${String(parts.hour).padStart(2, "0")}:${String(parts.minute).padStart(2, "0")}` : "";
}

export function compareTimestamps(left: unknown, right: unknown): number {
  const leftMs = timestampMs(left);
  const rightMs = timestampMs(right);
  if (leftMs !== undefined && rightMs !== undefined) return leftMs - rightMs;
  if (leftMs !== undefined) return 1;
  if (rightMs !== undefined) return -1;
  return String(left ?? "").localeCompare(String(right ?? ""));
}
