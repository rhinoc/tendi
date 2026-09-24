import { useEffect, useState } from "react";

import { IMPORTED_SESSION_AGENT } from "../../controllers/session-controller.ts";
import { SessionResumeTarget, sessionIdentity, type SessionRecord } from "../../lib/sessions.ts";

type InferredResumeTarget = Exclude<SessionResumeTarget, SessionResumeTarget.Auto>;

export function sessionSourceIdentity(session: SessionRecord): string {
  return sessionIdentity(session);
}

export function useInferredSessionResumeTargets({
  sessions,
  activeSession,
  sessionResumeTarget,
  resolveSessionResumeTarget,
}: {
  sessions: SessionRecord[];
  activeSession: SessionRecord | undefined;
  sessionResumeTarget: SessionResumeTarget;
  resolveSessionResumeTarget: (session: SessionRecord) => Promise<InferredResumeTarget>;
}) {
  const [inferredTargets, setInferredTargets] = useState<Record<string, InferredResumeTarget>>({});

  useEffect(() => {
    if (sessionResumeTarget !== SessionResumeTarget.Auto) {
      setInferredTargets((current) => Object.keys(current).length === 0 ? current : {});
      return;
    }
    let cancelled = false;
    const resumableSessions = [...new Map(
      [...sessions, ...(activeSession ? [activeSession] : [])]
        .filter((session) => (
          session.agent !== IMPORTED_SESSION_AGENT
          && Boolean(session.id && session.agent && session.path.trim())
        ))
        .map((session) => [sessionSourceIdentity(session), session] as const),
    ).values()];
    void Promise.all(resumableSessions.map(async (session) => (
      [sessionSourceIdentity(session), await resolveSessionResumeTarget(session)] as const
    ))).then((entries) => {
      if (cancelled) return;
      const retainedKeys = new Set(resumableSessions.map(sessionSourceIdentity));
      setInferredTargets((current) => {
        const next = Object.fromEntries(
          Object.entries(current).filter(([key]) => retainedKeys.has(key)),
        );
        for (const [key, target] of entries) next[key] = target;
        return next;
      });
    });
    return () => {
      cancelled = true;
    };
  }, [activeSession, resolveSessionResumeTarget, sessionResumeTarget, sessions]);

  return inferredTargets;
}
