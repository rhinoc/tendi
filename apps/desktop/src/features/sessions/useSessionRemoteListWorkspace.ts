import { useCallback, useEffect, useMemo, useState } from "react";

import { sessionTableRowId, type SessionListPageRequest, type SessionListPageResult } from "../../controllers/session-controller.ts";
import type { SessionRecord } from "../../lib/sessions.ts";

type PendingSessionLocate = {
  session: SessionRecord;
  reveal: boolean;
};

type UseSessionRemoteListWorkspaceOptions = {
  enabled: boolean;
  request: SessionListPageRequest;
  sessionMembershipKey: string;
  pageContextKey: string;
  listSessions: (request: SessionListPageRequest) => Promise<SessionListPageResult>;
  onLocated: (session: SessionRecord, page?: number) => void;
  onRevealRequired: (session: SessionRecord) => void;
  onError: (error: unknown) => void;
};

export function useSessionRemoteListWorkspace({
  enabled,
  request,
  sessionMembershipKey,
  pageContextKey,
  listSessions,
  onLocated,
  onRevealRequired,
  onError,
}: UseSessionRemoteListWorkspaceOptions) {
  const [remoteList, setRemoteList] = useState<SessionListPageResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [pendingLocate, setPendingLocate] = useState<PendingSessionLocate | null>(null);
  const requestWithLocate = useMemo(() => ({
    ...request,
    ...(pendingLocate ? { locate: pendingLocate.session } : {}),
  }), [pendingLocate, request]);

  useEffect(() => {
    if (!enabled) return undefined;
    let cancelled = false;
    setLoading(true);
    void listSessions(requestWithLocate)
      .then((result) => {
        if (cancelled) return;
        setRemoteList(result);
        setLoading(false);
        if (!pendingLocate) return;

        const targetId = sessionTableRowId(pendingLocate.session);
        const located = result.rows.some((session) => sessionTableRowId(session) === targetId);
        if (located) {
          onLocated(pendingLocate.session, result.page);
          setPendingLocate(null);
          return;
        }
        if (pendingLocate.reveal) {
          setPendingLocate(null);
          return;
        }

        onRevealRequired(pendingLocate.session);
        setPendingLocate({ session: pendingLocate.session, reveal: true });
      })
      .catch((error) => {
        if (cancelled) return;
        setLoading(false);
        onError(error);
      });
    return () => { cancelled = true; };
  }, [enabled, listSessions, onError, onLocated, onRevealRequired, pageContextKey, pendingLocate, requestWithLocate, sessionMembershipKey]);

  const locate = useCallback((session: SessionRecord, known: boolean, visible: boolean) => {
    if (!known) return;
    if (visible) {
      onLocated(session);
      return;
    }
    setPendingLocate({ session, reveal: false });
  }, [onLocated]);

  return { remoteList, loading, locate };
}
