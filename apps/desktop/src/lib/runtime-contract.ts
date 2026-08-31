import type { JsonValue, RuntimeEventEnvelope } from "./generated/runtime-types.ts";

export type ScopeKey = string;
export type Revision = number;
export type InstallationId = string;

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
