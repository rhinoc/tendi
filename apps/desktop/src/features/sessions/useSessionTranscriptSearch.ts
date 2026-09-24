import { useEffect, useRef, useState } from "react";
import { IMPORTED_SESSION_AGENT } from "../../controllers/session-controller.ts";
import type { SessionRecord } from "../../lib/sessions.ts";
import type { TranscriptSearchResult, TranscriptSearchScopes } from "../../lib/transcript.ts";

type UseSessionTranscriptSearchOptions = {
  session: SessionRecord;
  sessionKey: string;
  query: string;
  scopes: TranscriptSearchScopes;
  hasMore: boolean;
  transcriptLoading: boolean;
  searchTranscript?: (
    session: SessionRecord,
    query: string,
    scopes: TranscriptSearchScopes,
  ) => Promise<TranscriptSearchResult | null>;
  loadAll: () => Promise<void>;
};

export function useSessionTranscriptSearch({
  session,
  sessionKey,
  query,
  scopes,
  hasMore,
  transcriptLoading,
  searchTranscript,
  loadAll,
}: UseSessionTranscriptSearchOptions) {
  const remoteSearch = Boolean(
    searchTranscript
    && session.agent !== IMPORTED_SESSION_AGENT
    && session.path,
  );
  const sessionRef = useRef(session);
  sessionRef.current = session;
  const [loading, setLoading] = useState(false);
  const [readyQuery, setReadyQuery] = useState("");
  const [result, setResult] = useState<TranscriptSearchResult | null>(null);
  const [error, setError] = useState(false);
  const [errorRevision, setErrorRevision] = useState(0);

  useEffect(() => {
    if (!remoteSearch) return;
    if (!query) {
      setLoading(false);
      setReadyQuery("");
      setResult(null);
      setError(false);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setReadyQuery("");
    setResult(null);
    setError(false);

    if (searchTranscript) {
      void searchTranscript(sessionRef.current, query, scopes).then((nextResult) => {
        if (cancelled) return;
        setLoading(false);
        setReadyQuery(query);
        if (nextResult) {
          setResult(nextResult);
        } else {
          setError(true);
          setErrorRevision((revision) => revision + 1);
        }
      }, () => {
        if (cancelled) return;
        setLoading(false);
        setReadyQuery(query);
        setError(true);
        setErrorRevision((revision) => revision + 1);
      });
    }

    return () => {
      cancelled = true;
    };
  }, [query, remoteSearch, scopes, searchTranscript, sessionKey]);

  useEffect(() => {
    if (remoteSearch) return;
    if (!query) {
      setLoading(false);
      setReadyQuery("");
      setResult(null);
      setError(false);
      return;
    }

    let cancelled = false;
    setLoading(true);
    setReadyQuery("");
    void loadAll().then(() => {
      if (cancelled) return;
      setLoading(false);
      setReadyQuery(query);
    }, () => {
      if (cancelled) return;
      setLoading(false);
      setReadyQuery(query);
    });
    return () => {
      cancelled = true;
    };
  }, [hasMore, loadAll, query, remoteSearch, transcriptLoading]);

  return {
    loading,
    ready: !query || readyQuery === query,
    readyQuery,
    result,
    error,
    errorRevision,
    remoteSearch,
  };
}
