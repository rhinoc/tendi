import { useCallback, useEffect, useRef, useState } from "react";

import { logger } from "../../lib/logger.ts";
import type { SessionRecord, SessionSkillLinkRecord } from "../../lib/sessions.ts";

type UseSessionSkillLinksOptions = {
  session: SessionRecord | null;
  sessionIdentityKey: string;
  requestKey: string;
  loadSessionSkillLinks?: (session: SessionRecord) => Promise<SessionSkillLinkRecord[]>;
  importedAgent: string;
  onToastError: (message: string) => void;
};

const SKILL_LINK_LOAD_ERROR = "Could not load skills used by this session.";
const SKILL_LINK_TOAST_ERROR = "Could not load skills used by this session. Try again.";

export function useSessionSkillLinks({
  session,
  sessionIdentityKey,
  requestKey,
  loadSessionSkillLinks,
  importedAgent,
  onToastError,
}: UseSessionSkillLinksOptions) {
  const [links, setLinks] = useState<SessionSkillLinkRecord[]>([]);
  const [loadedKey, setLoadedKey] = useState("");
  const [attemptKey, setAttemptKey] = useState("");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const requestKeyRef = useRef("");

  useEffect(() => {
    setLinks([]);
    setLoadedKey("");
    setAttemptKey("");
    setError("");
    setLoading(false);
    requestKeyRef.current = "";
  }, [sessionIdentityKey]);

  const load = useCallback(async (force = false) => {
    if (!session || !loadSessionSkillLinks || !requestKey) return;
    if (session.agent === importedAgent) {
      setLinks([]);
      setError("");
      setLoadedKey(requestKey);
      setLoading(false);
      return;
    }
    if (loading) return;
    if (!force && (loadedKey === requestKey || attemptKey === requestKey)) return;

    const currentRequestKey = requestKey;
    requestKeyRef.current = currentRequestKey;
    setAttemptKey(currentRequestKey);
    setLoading(true);
    setError("");
    try {
      const nextLinks = await loadSessionSkillLinks(session);
      if (requestKeyRef.current !== currentRequestKey) return;
      setLinks(Array.isArray(nextLinks) ? nextLinks : []);
      setLoadedKey(currentRequestKey);
    } catch (loadError) {
      if (requestKeyRef.current === currentRequestKey) {
        logger.warn("sessions skill links load failed", { error: loadError });
        setError(SKILL_LINK_LOAD_ERROR);
        onToastError(SKILL_LINK_TOAST_ERROR);
      }
    } finally {
      if (requestKeyRef.current === currentRequestKey) setLoading(false);
    }
  }, [attemptKey, importedAgent, loadSessionSkillLinks, loadedKey, loading, onToastError, requestKey, session]);

  useEffect(() => {
    void load();
  }, [load]);

  const retry = useCallback(() => {
    void load(true);
  }, [load]);

  return {
    links,
    loading,
    loaded: loadedKey === requestKey,
    error,
    retry,
  };
}
