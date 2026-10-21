import { useCallback, useRef, useState } from "react";

import { sessionLogicalIdentity, type SessionRecord } from "../../lib/sessions.ts";

type SessionArchiveHandler = (session: SessionRecord, archived: boolean) => Promise<void>;

export function useSessionArchiveOperations({ onSetArchived, onError }: {
  onSetArchived?: SessionArchiveHandler;
  onError: (message: string) => void;
}) {
  const pending = useRef(new Set<string>());
  const [loading, setLoading] = useState<Set<string>>(() => new Set());
  const isArchiving = useCallback(
    (session: SessionRecord) => loading.has(sessionLogicalIdentity(session)),
    [loading],
  );
  const toggleArchive = useCallback(async (session: SessionRecord) => {
    if (!onSetArchived || !session.archiveSupported || typeof session.archived !== "boolean") return;
    const key = sessionLogicalIdentity(session);
    if (pending.current.has(key)) return;
    pending.current.add(key);
    setLoading(new Set(pending.current));
    try {
      await onSetArchived(session, !session.archived);
    } catch (error) {
      onError(error instanceof Error ? error.message : "Could not change session archive status. Try again.");
    } finally {
      pending.current.delete(key);
      setLoading(new Set(pending.current));
    }
  }, [onError, onSetArchived]);
  return { toggleArchive, isArchiving };
}
