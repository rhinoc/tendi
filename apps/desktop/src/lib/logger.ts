import { invoke } from "@tauri-apps/api/core";

export type LogLevel = "debug" | "info" | "warn" | "error";
export type LogFields = Record<string, unknown>;

type TauriWindow = Window & {
  __TAURI_INTERNALS__?: {
    invoke?: unknown;
    transformCallback?: unknown;
  };
};

type LogEvent = {
  level: LogLevel;
  message: string;
  fields: LogFields;
};

let pending = Promise.resolve();

export const logger = {
  debug(message: string, fields?: LogFields): void {
    enqueue({ level: "debug", message, fields: fields ?? {} });
  },
  info(message: string, fields?: LogFields): void {
    enqueue({ level: "info", message, fields: fields ?? {} });
  },
  warn(message: string, fields?: LogFields): void {
    enqueue({ level: "warn", message, fields: fields ?? {} });
  },
  error(message: string, fields?: LogFields): void {
    enqueue({ level: "error", message, fields: fields ?? {} });
  },
};

function enqueue(event: LogEvent): void {
  const normalized = {
    ...event,
    fields: normalizeFields(event.fields),
  };
  pending = pending.then(() => writeLog(normalized)).catch(() => undefined);
}

async function writeLog(event: LogEvent): Promise<void> {
  if (isTauriRuntime()) {
    await invoke("log_event", event);
    return;
  }

  const response = await fetch("/__tendi/log", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(event),
  });
  if (!response.ok) {
    throw new Error(`Log request failed (${response.status})`);
  }
}

function isTauriRuntime(): boolean {
  if (typeof window === "undefined") return false;
  const internals = (window as TauriWindow).__TAURI_INTERNALS__;
  return typeof internals?.invoke === "function" && typeof internals.transformCallback === "function";
}

function normalizeFields(fields: LogFields): LogFields {
  const normalized: LogFields = {};
  const seen = new WeakSet<object>();
  for (const [key, value] of Object.entries(fields)) {
    normalized[key] = normalizeValue(value, seen);
  }
  return normalized;
}

function normalizeValue(value: unknown, seen: WeakSet<object>): unknown {
  if (value instanceof Error) {
    return {
      name: value.name,
      message: value.message,
      stack: value.stack,
    };
  }
  if (typeof value === "bigint") return value.toString();
  if (!value || typeof value !== "object") return value;
  if (seen.has(value)) return "[Circular]";
  seen.add(value);
  if (Array.isArray(value)) return value.map((item) => normalizeValue(item, seen));
  return Object.fromEntries(
    Object.entries(value).map(([key, item]) => [key, normalizeValue(item, seen)]),
  );
}
