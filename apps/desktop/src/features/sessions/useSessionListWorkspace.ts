import { useEffect, useMemo, useState } from "react";

import { selectSessionListView, type SessionListControllerInput, type SessionListView } from "../../controllers/session-controller.ts";
import { logger } from "../../lib/logger.ts";
import type { SessionRecord } from "../../lib/sessions.ts";
import type { SessionArchiveFilter } from "../../lib/generated/runtime-types.ts";
import type { TranscriptSearchScopes } from "../../lib/transcript.ts";

type SessionListWorkspaceOptions = Omit<SessionListControllerInput, "remoteSearch" | "searchRows" | "searchRowsKey"> & {
  enabled: boolean;
  searchScopes: TranscriptSearchScopes;
  archiveFilter: SessionArchiveFilter;
  searchSessions: (query: string, searchScopes: TranscriptSearchScopes, archiveFilter: SessionArchiveFilter) => Promise<SessionRecord[]>;
  onSearchError: (message: string) => void;
};

export function useSessionListWorkspace({
  enabled,
  searchScopes,
  archiveFilter,
  searchSessions,
  onSearchError,
  ...listInput
}: SessionListWorkspaceOptions): {
  localListView: SessionListView | null;
  searchingSessions: boolean;
} {
  const [searchingSessions, setSearchingSessions] = useState(false);
  const [searchRows, setSearchRows] = useState<SessionRecord[] | null>(null);
  const [searchRowsKey, setSearchRowsKey] = useState("");
  const {
    sessions,
    importedSessions,
    query,
    searchSort,
    sort,
    pageSize,
    groupBy,
    showChildSessions,
    selectedProjectKeys,
    projectFilterQuery,
    missingSessionProjectPolicy,
    projects,
    sessionProjects,
    currentPage,
    pageSelectionContextKey,
    searchFilterKey,
    startedAfter,
  } = listInput;

  const localListView = useMemo(() => {
    if (!enabled) return null;
    return selectSessionListView({
      sessions,
      importedSessions,
      searchRows: searchRows ?? undefined,
      searchRowsKey,
      query,
      remoteSearch: true,
      searchSort,
      sort,
      pageSize,
      groupBy,
      showChildSessions,
      selectedProjectKeys,
      projectFilterQuery,
      missingSessionProjectPolicy,
      projects,
      sessionProjects,
      currentPage,
      pageSelectionContextKey,
      searchFilterKey,
      startedAfter,
      archiveFilter,
    });
  }, [
    currentPage,
    archiveFilter,
    enabled,
    groupBy,
    importedSessions,
    missingSessionProjectPolicy,
    pageSelectionContextKey,
    searchFilterKey,
    startedAfter,
    pageSize,
    projectFilterQuery,
    projects,
    query,
    searchRows,
    searchRowsKey,
    searchSort,
    selectedProjectKeys,
    sessionProjects,
    sessions,
    showChildSessions,
    sort,
  ]);

  useEffect(() => {
    if (!enabled || !query) {
      setSearchRows(null);
      setSearchRowsKey("");
      setSearchingSessions(false);
      return;
    }

    let cancelled = false;
    setSearchingSessions(true);
    setSearchRows([]);
    const searchRequestKey = localListView?.searchRequestKey ?? "";
    setSearchRowsKey(searchRequestKey);

    const runSearch = async () => {
      try {
        const rows = await searchSessions(query, searchScopes, archiveFilter);
        if (cancelled) return;
        setSearchRows(rows);
        if (!cancelled) setSearchingSessions(false);
      } catch (error) {
        if (!cancelled) {
          logger.warn("sessions search failed", { error });
          setSearchingSessions(false);
          onSearchError("Could not search sessions. Try again.");
        }
      }
    };
    void runSearch();
    return () => {
      cancelled = true;
    };
  }, [archiveFilter, enabled, localListView?.searchRequestKey, onSearchError, query, searchScopes, searchSessions]);

  return { localListView, searchingSessions };
}
