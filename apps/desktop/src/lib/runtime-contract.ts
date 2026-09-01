import type { JsonValue, RuntimeEventEnvelope, SkillVisibility as RuntimeSkillVisibility } from "./generated/runtime-types.ts";

export type ScopeKey = string;
export type Revision = number;
export type InstallationId = string;

export function omitUndefinedProperties(value: unknown): unknown {
  if (Array.isArray(value)) return value.map(omitUndefinedProperties);
  if (value !== null && typeof value === "object") {
    const result: Record<string, unknown> = {};
    for (const [key, nested] of Object.entries(value)) {
      if (nested !== undefined) result[key] = omitUndefinedProperties(nested);
    }
    return result;
  }
  return value;
}

export function normalizeRuntimeSkillVisibility(value: string): RuntimeSkillVisibility {
  const normalized = value.trim().toLowerCase();
  if (normalized === "auto" || normalized === "manual" || normalized === "off" || normalized === "mixed") return normalized;
  throw new Error("Invalid skill visibility");
}

export type SessionKey = {
  provider: string;
  namespace: string;
  nativeId: string;
};

export type SourceLocator = {
  provider: string;
  path: string;
  nativeId?: string | null;
};

export type DomainSnapshot<T> = {
  scopeKey: ScopeKey;
  domain: string;
  revision: Revision;
  sourceVersion?: string | null;
  schemaVersion: number;
  snapshotId: string;
  payload: T;
};

export type DaemonEvent<T = JsonValue> = Omit<RuntimeEventEnvelope, "payload"> & { payload: T };

export type RevisionedEvent<T> = {
  scopeKey: ScopeKey;
  domain: string;
  operationId: string;
  baseRevision: Revision;
  revision: Revision;
  sourceVersion?: string | null;
  payload: T;
};

export type RevisionDecision<T> = {
  accepted: boolean;
  needsResync: boolean;
  payload?: T;
};

export function decideRevision<T>(
  localRevision: Revision,
  event: RevisionedEvent<T>,
): RevisionDecision<T> {
  if (event.revision <= localRevision) {
    return { accepted: false, needsResync: false };
  }
  if (event.baseRevision !== localRevision) {
    return { accepted: false, needsResync: true };
  }
  return { accepted: true, needsResync: false, payload: event.payload };
}

export function snapshotRevision<T>(snapshot: DomainSnapshot<T>): Revision {
  return snapshot.revision;
}
