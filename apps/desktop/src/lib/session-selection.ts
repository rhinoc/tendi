import { agentIdentityKey } from "./agents.ts";

type SessionSelectionRecord = {
  id: string;
  agent?: string;
  path?: string;
};

export function canonicalSessionAgent(agent: unknown): string {
  return agentIdentityKey(agent);
}

export function sessionExternalKey(session: SessionSelectionRecord): string {
  return `${canonicalSessionAgent(session.agent)}:${session.id}`;
}

export function sessionSourceExternalKey(session: SessionSelectionRecord): string {
  return `${sessionExternalKey(session)}:${session.path ?? ""}`;
}

export function resolveInitialSessionId(
  sessions: SessionSelectionRecord[],
  activeSessionKey?: string,
): string {
  return resolveInitialSession(sessions, activeSessionKey)?.id ?? "";
}

export function resolveInitialSession<T extends SessionSelectionRecord>(
  sessions: T[],
  activeSessionKey?: string,
): T | undefined {
  const requestedKey = `${activeSessionKey ?? ""}`.toLowerCase();
  if (requestedKey) {
    const requested = sessions.find((session) => (
      (session.path && sessionSourceExternalKey(session) === requestedKey)
      || sessionExternalKey(session) === requestedKey
    ));
    return requested;
  }
  return sessions[0];
}
