const inFlight = new Map<string, Promise<unknown>>();

function stableSerialize(value: unknown): string {
  if (value === null || typeof value !== "object") return JSON.stringify(value);
  if (Array.isArray(value)) return `[${value.map(stableSerialize).join(",")}]`;
  const record = value as Record<string, unknown>;
  return `{${Object.keys(record)
    .filter((key) => record[key] !== undefined)
    .sort()
    .map((key) => `${JSON.stringify(key)}:${stableSerialize(record[key])}`)
    .join(",")}}`;
}

export function singleFlight<T>(key: string, task: () => Promise<T>): Promise<T> {
  const existing = inFlight.get(key);
  if (existing) return existing as Promise<T>;

  const promise = Promise.resolve().then(task);
  inFlight.set(key, promise);
  const release = () => {
    if (inFlight.get(key) === promise) inFlight.delete(key);
  };
  promise.then(release, release);
  return promise;
}

export function singleFlightKey(scope: string, value: unknown): string {
  return `${scope}:${stableSerialize(value)}`;
}
