import { useCallback, useRef, useState } from "react";

import { releaseHeldSession as releaseHeldSessionCommand } from "../../lib/runtime-gateway.ts";
import { sessionLogicalIdentity, type SessionRecord } from "../../lib/sessions.ts";
import { desktopStore } from "../../store/desktop-store.ts";

export function useSessionReleaseOperations(onError: (message: string) => void) {
  const pending = useRef(new Set<string>());
  const [loading, setLoading] = useState<Set<string>>(() => new Set());
  const isReleasing = useCallback(
    (session: Pick<SessionRecord, "id" | "agent">) => loading.has(sessionLogicalIdentity(session)),
    [loading],
  );
  const releaseHeldSession = useCallback(async (session: SessionRecord) => {
    const key = sessionLogicalIdentity(session);
    if (pending.current.has(key)) return;
    pending.current.add(key);
    setLoading(new Set(pending.current));
    try {
      await releaseHeldSessionCommand(session);
      desktopStore.actions.forgetHeldSession(session);
    } catch (error) {
      onError(error instanceof Error ? error.message : "Could not release this thread.");
    } finally {
      pending.current.delete(key);
      setLoading(new Set(pending.current));
    }
  }, [onError]);
  return { releaseHeldSession, isReleasing };
}
