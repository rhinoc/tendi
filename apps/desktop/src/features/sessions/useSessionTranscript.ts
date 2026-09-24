import { useCallback, useEffect, useRef, useState } from "react";
import { createLatestRequestAuthority, groupTranscriptItems, mergeTranscriptItems, type TranscriptLocatorPage, type TranscriptPage } from "../../lib/transcript.ts";
import type { SessionRecord } from "../../lib/sessions.ts";
import { logger } from "../../lib/logger.ts";
import { loadTranscriptTarget, type LoadTranscriptTargetResult } from "../../lib/transcript-navigation.ts";
import {
  asTranscriptPage,
  mergeTranscriptPage,
  preserveTranscriptTail,
  transcriptItemsSharePrefix,
  trimTranscriptCache,
  type TranscriptItemRecord,
  type TranscriptLoadMoreResult,
} from "./session-transcript-logic.ts";

type UseSessionTranscriptOptions = {
  session: SessionRecord | null;
  transcriptKey: string;
  logicalKey: string;
  refreshKey: string;
  importedItems?: TranscriptItemRecord[];
  loadTranscript: (session: SessionRecord, cursor?: string, knownSourceVersion?: string) => Promise<TranscriptPage>;
  loadTranscriptLocator?: (session: SessionRecord) => Promise<TranscriptLocatorPage>;
  onError: (message: string) => void;
};

type LocatorState = {
  key: string;
  items: TranscriptLocatorPage["locatorItems"];
};

export type SessionTranscriptTargetLoadOptions = {
  isTargetLoaded: (items: TranscriptItemRecord[]) => boolean;
  isCurrent?: () => boolean;
  onItemsLoaded?: (items: TranscriptItemRecord[]) => void;
};

export type SessionTranscriptTargetLoadResult = LoadTranscriptTargetResult<
  TranscriptItemRecord,
  TranscriptLoadMoreResult["status"]
>;

export function useSessionTranscript({
  session,
  transcriptKey,
  logicalKey,
  refreshKey,
  importedItems,
  loadTranscript,
  loadTranscriptLocator,
  onError,
}: UseSessionTranscriptOptions) {
  const [items, setItems] = useState<TranscriptItemRecord[]>([]);
  const [nextCursor, setNextCursor] = useState<string | undefined>();
  const [loading, setLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [locatorState, setLocatorState] = useState<LocatorState | null>(null);

  const itemsRef = useRef(items);
  const nextCursorRef = useRef(nextCursor);
  const sessionRef = useRef(session);
  const onErrorRef = useRef(onError);
  const loadTranscriptRef = useRef(loadTranscript);
  const loadTranscriptLocatorRef = useRef(loadTranscriptLocator);
  itemsRef.current = items;
  nextCursorRef.current = nextCursor;
  sessionRef.current = session;
  onErrorRef.current = onError;
  loadTranscriptRef.current = loadTranscript;
  loadTranscriptLocatorRef.current = loadTranscriptLocator;

  const requestAuthorityRef = useRef(createLatestRequestAuthority());
  const cacheRef = useRef(new Map<string, TranscriptPage>());
  const loadedIdentityRef = useRef("");
  const loadedLogicalIdentityRef = useRef("");
  const sourceVersionRef = useRef("");
  const locatorRequestAuthorityRef = useRef(createLatestRequestAuthority());
  const locatorCacheRef = useRef(new Map<string, TranscriptLocatorPage>());
  const pendingLocatorRef = useRef<{ key: string; session: SessionRecord } | null>(null);
  const locatorInFlightRef = useRef<{ key: string; promise: Promise<void> } | null>(null);
  const initialLoadInFlightRef = useRef<{
    key: string;
    promise: Promise<TranscriptLoadMoreResult["status"]>;
  } | null>(null);
  const loadMoreInFlightRef = useRef<{ key: string; promise: Promise<TranscriptLoadMoreResult> } | null>(null);
  const loadAllInFlightRef = useRef<{ key: string; promise: Promise<void> } | null>(null);

  const drainLocator = useCallback(() => {
    const loadLocator = loadTranscriptLocatorRef.current;
    const currentSession = sessionRef.current;
    if (!loadLocator || !currentSession) return;
    const pending = pendingLocatorRef.current;
    if (!pending || pending.key !== transcriptKey) return;
    const existing = locatorInFlightRef.current;
    if (existing?.key === pending.key) return;
    pendingLocatorRef.current = null;
    const requestRevision = locatorRequestAuthorityRef.current.begin();
    const promise = loadLocator(pending.session).then((page) => {
      if (!locatorRequestAuthorityRef.current.isCurrent(requestRevision)) return;
      if (pending.key !== transcriptKey) return;
      const currentSourceVersion = sourceVersionRef.current;
      if (currentSourceVersion && page.sourceVersion && page.sourceVersion !== currentSourceVersion) return;
      locatorCacheRef.current.set(pending.key, page);
      setLocatorState({ key: pending.key, items: page.locatorItems });
    }).catch((error) => {
      if (locatorRequestAuthorityRef.current.isCurrent(requestRevision)) {
        logger.warn("sessions transcript locator load failed", { error });
      }
    });
    locatorInFlightRef.current = { key: pending.key, promise };
    void promise.then(() => {
      if (locatorInFlightRef.current?.promise === promise) locatorInFlightRef.current = null;
    });
  }, [transcriptKey]);

  const queueLocator = useCallback((targetSession: SessionRecord, key: string) => {
    const cached = locatorCacheRef.current.get(key);
    if (cached) {
      setLocatorState({ key, items: cached.locatorItems });
      return;
    }
    if (locatorInFlightRef.current?.key === key) return;
    pendingLocatorRef.current = { key, session: targetSession };
    drainLocator();
  }, [drainLocator]);

  useEffect(() => {
    drainLocator();
  }, [drainLocator]);

  const setCurrentItems = useCallback((nextItems: TranscriptItemRecord[]) => {
    itemsRef.current = nextItems;
    setItems(nextItems);
  }, []);

  const setCurrentCursor = useCallback((cursor: string | undefined) => {
    nextCursorRef.current = cursor;
    setNextCursor(cursor);
  }, []);

  const loadMoreRef = useRef<() => Promise<TranscriptLoadMoreResult>>(
    () => Promise.resolve({ items: itemsRef.current, status: "exhausted" }),
  );
  const transcriptKeyRef = useRef(transcriptKey);
  transcriptKeyRef.current = transcriptKey;

  const loadMore = useCallback((): Promise<TranscriptLoadMoreResult> => {
    const requestKey = transcriptKey;
    const existing = loadMoreInFlightRef.current;
    if (existing?.key === requestKey) return existing.promise;

    const initial = initialLoadInFlightRef.current;
    if (initial?.key === requestKey) {
      return initial.promise.then((status) => {
        if (initialLoadInFlightRef.current?.promise === initial.promise) {
          initialLoadInFlightRef.current = null;
        }
        if (status !== "loaded") return { items: itemsRef.current, status };
        if (!sessionRef.current || !nextCursorRef.current) {
          return { items: itemsRef.current, status: "exhausted" as const };
        }
        return loadMoreRef.current();
      });
    }

    const currentSession = sessionRef.current;
    const cursor = nextCursorRef.current;
    if (!currentSession || !cursor || requestKey !== transcriptKeyRef.current) {
      return Promise.resolve({ items: itemsRef.current, status: "exhausted" as const });
    }

    const requestRevision = requestAuthorityRef.current.begin();
    const currentItems = itemsRef.current;
    setLoadingMore(true);
    const promise = (async (): Promise<TranscriptLoadMoreResult> => {
      try {
        const page = await loadTranscriptRef.current(currentSession, cursor);
        if (!requestAuthorityRef.current.isCurrent(requestRevision)) {
          return { items: currentItems, status: "cancelled" };
        }
        if (page.restartRequired) {
          const restarted = await loadTranscriptRef.current(currentSession);
          if (!requestAuthorityRef.current.isCurrent(requestRevision)) {
            return { items: currentItems, status: "cancelled" };
          }
          locatorRequestAuthorityRef.current.begin();
          locatorCacheRef.current.delete(requestKey);
          setLocatorState(null);
          sourceVersionRef.current = restarted.sourceVersion;
          cacheRef.current.set(requestKey, restarted);
          trimTranscriptCache(cacheRef.current);
          setCurrentItems(restarted.items);
          setCurrentCursor(restarted.nextCursor);
          queueLocator(currentSession, requestKey);
          return {
            items: restarted.items,
            status: restarted.nextCursor ? "loaded" : "exhausted",
          };
        }

        const sourceChanged = Boolean(
          sourceVersionRef.current
          && page.sourceVersion
          && page.sourceVersion !== sourceVersionRef.current,
        );
        if (sourceChanged) {
          locatorRequestAuthorityRef.current.begin();
          locatorCacheRef.current.delete(requestKey);
          setLocatorState(null);
        }
        sourceVersionRef.current = page.sourceVersion;
        const cached = cacheRef.current.get(requestKey);
        const merged = mergeTranscriptPage(cached, currentItems, page);
        cacheRef.current.delete(requestKey);
        cacheRef.current.set(requestKey, merged);
        trimTranscriptCache(cacheRef.current);
        setCurrentItems(merged.items);
        setCurrentCursor(merged.nextCursor);
        if (sourceChanged) queueLocator(currentSession, requestKey);
        return {
          items: merged.items,
          status: merged.nextCursor ? "loaded" : "exhausted",
        };
      } catch (error) {
        if (requestAuthorityRef.current.isCurrent(requestRevision)) {
          logger.warn("sessions transcript page load failed", { error });
          onErrorRef.current("Could not load session details. Try again.");
        }
        return { items: currentItems, status: "failed" };
      } finally {
        if (requestAuthorityRef.current.isCurrent(requestRevision)) setLoadingMore(false);
      }
    })();
    loadMoreInFlightRef.current = { key: requestKey, promise };
    void promise.then(() => {
      if (loadMoreInFlightRef.current?.promise === promise) loadMoreInFlightRef.current = null;
    });
    return promise;
  }, [queueLocator, setCurrentCursor, setCurrentItems, transcriptKey]);
  loadMoreRef.current = loadMore;

  const loadAll = useCallback((): Promise<void> => {
    const requestKey = transcriptKey;
    const existing = loadAllInFlightRef.current;
    if (existing?.key === requestKey) return existing.promise;
    const promise = (async () => {
      while (nextCursorRef.current && requestKey === transcriptKeyRef.current) {
        const cursorBefore = nextCursorRef.current;
        const result = await loadMoreRef.current();
        if (result.status === "failed" || result.status === "cancelled") break;
        if (nextCursorRef.current === cursorBefore) break;
      }
    })();
    loadAllInFlightRef.current = { key: requestKey, promise };
    void promise.then(() => {
      if (loadAllInFlightRef.current?.promise === promise) loadAllInFlightRef.current = null;
    });
    return promise;
  }, [transcriptKey]);

  const loadUntilTarget = useCallback(({ isTargetLoaded, isCurrent, onItemsLoaded }: SessionTranscriptTargetLoadOptions) => (
    loadTranscriptTarget({
      getItems: () => groupTranscriptItems(itemsRef.current) as TranscriptItemRecord[],
      loadMore: async () => {
        const page = await loadMoreRef.current();
        return {
          status: page.status,
          items: groupTranscriptItems(page.items) as TranscriptItemRecord[],
        };
      },
      isTargetLoaded,
      isCurrent,
      onItemsLoaded,
      loadedStatus: "loaded",
    })
  ), []);

  useEffect(() => {
    const transcriptSession = sessionRef.current;
    const requestRevision = requestAuthorityRef.current.begin();
    locatorRequestAuthorityRef.current.begin();
    pendingLocatorRef.current = null;
    const identityChanged = loadedIdentityRef.current !== transcriptKey;
    const logicalIdentityChanged = loadedLogicalIdentityRef.current !== logicalKey;
    if (identityChanged) {
      loadedIdentityRef.current = transcriptKey;
      loadedLogicalIdentityRef.current = logicalKey;
      sourceVersionRef.current = "";
      if (logicalIdentityChanged) itemsRef.current = [];
      nextCursorRef.current = undefined;
      if (logicalIdentityChanged) setItems([]);
      setLocatorState(null);
      setNextCursor(undefined);
    }
    loadMoreInFlightRef.current = null;
    loadAllInFlightRef.current = null;
    initialLoadInFlightRef.current = null;
    setLoadingMore(false);
    setLoading(Boolean(transcriptSession));
    if (!transcriptSession) {
      loadedIdentityRef.current = "";
      loadedLogicalIdentityRef.current = "";
      return () => requestAuthorityRef.current.invalidate(requestRevision);
    }
    if (importedItems) {
      setCurrentItems(importedItems);
      setLoading(false);
      return () => requestAuthorityRef.current.invalidate(requestRevision);
    }

    const cached = cacheRef.current.get(transcriptKey);
    if (identityChanged && cached) {
      setCurrentItems(cached.items);
      nextCursorRef.current = cached.nextCursor;
      if (cached.locatorItems.length > 0) {
        const cachedLocator: TranscriptLocatorPage = {
          locatorItems: cached.locatorItems,
          warnings: cached.warnings,
          sourceVersion: cached.sourceVersion,
        };
        locatorCacheRef.current.set(transcriptKey, cachedLocator);
        setLocatorState({ key: transcriptKey, items: cached.locatorItems });
      }
      cacheRef.current.delete(transcriptKey);
      cacheRef.current.set(transcriptKey, cached);
      setNextCursor(cached.nextCursor);
      setLoading(false);
    }

    const knownSourceVersion = sourceVersionRef.current || cached?.sourceVersion;
    const previousTranscriptItemCount = itemsRef.current.length;
    let initialLoadStatus: TranscriptLoadMoreResult["status"] = "loaded";
    const initialLoadPromise = loadTranscriptRef.current(transcriptSession, undefined, knownSourceVersion).then(async (page) => {
      if (!requestAuthorityRef.current.isCurrent(requestRevision)) {
        initialLoadStatus = "cancelled";
        return;
      }
      if (page.unchanged) {
        sourceVersionRef.current = page.sourceVersion || knownSourceVersion || "";
        queueLocator(transcriptSession, transcriptKey);
        return;
      }
      let refreshedPage = page;
      const shouldReloadAll = previousTranscriptItemCount > page.items.length;
      if (shouldReloadAll) {
        let next = page.nextCursor;
        let restartCount = 0;
        let allItems = [...page.items];
        let allWarnings = [...page.warnings];
        let latestSourceVersion = page.sourceVersion;
        while (next) {
          const nextPage = await loadTranscriptRef.current(transcriptSession, next);
          if (!requestAuthorityRef.current.isCurrent(requestRevision)) {
            initialLoadStatus = "cancelled";
            return;
          }
          if (nextPage.restartRequired) {
            if (restartCount >= 1) throw new Error("Transcript changed while refreshing");
            restartCount += 1;
            const restarted = await loadTranscriptRef.current(transcriptSession);
            if (!requestAuthorityRef.current.isCurrent(requestRevision)) {
              initialLoadStatus = "cancelled";
              return;
            }
            allItems = [...restarted.items];
            allWarnings = [...restarted.warnings];
            latestSourceVersion = restarted.sourceVersion;
            next = restarted.nextCursor;
            refreshedPage = restarted;
            continue;
          }
          allItems = mergeTranscriptItems(allItems, nextPage.items);
          allWarnings.push(...nextPage.warnings);
          latestSourceVersion = nextPage.sourceVersion || latestSourceVersion;
          next = nextPage.nextCursor;
        }
        refreshedPage = asTranscriptPage(refreshedPage, allItems, latestSourceVersion, allWarnings, next);
      }
      const sourceChanged = Boolean(
        sourceVersionRef.current
        && refreshedPage.sourceVersion
        && refreshedPage.sourceVersion !== sourceVersionRef.current,
      );
      const sourcePrefixRewritten = sourceChanged
        && !transcriptItemsSharePrefix(itemsRef.current, refreshedPage.items);
      if (sourcePrefixRewritten) {
        locatorRequestAuthorityRef.current.begin();
        locatorCacheRef.current.delete(transcriptKey);
        setLocatorState(null);
      }
      const refreshedItems = shouldReloadAll
        ? refreshedPage.items
        : preserveTranscriptTail(itemsRef.current, refreshedPage.items);
      const pageToCache = refreshedItems === refreshedPage.items
        ? refreshedPage
        : { ...refreshedPage, items: refreshedItems };
      cacheRef.current.set(transcriptKey, pageToCache);
      trimTranscriptCache(cacheRef.current);
      setCurrentItems(refreshedItems);
      setCurrentCursor(pageToCache.nextCursor);
      sourceVersionRef.current = pageToCache.sourceVersion;
      if (pageToCache.locatorItems.length > 0) {
        const locatorPage: TranscriptLocatorPage = {
          locatorItems: pageToCache.locatorItems,
          warnings: pageToCache.warnings,
          sourceVersion: pageToCache.sourceVersion,
        };
        locatorCacheRef.current.set(transcriptKey, locatorPage);
        setLocatorState({ key: transcriptKey, items: pageToCache.locatorItems });
      }
      queueLocator(transcriptSession, transcriptKey);
    }).catch((error) => {
      initialLoadStatus = "failed";
      if (requestAuthorityRef.current.isCurrent(requestRevision)) {
        logger.warn("sessions transcript load failed", {
          requestRevision,
          id: transcriptSession.id,
          error,
        });
        onErrorRef.current("Could not load session details. Try again.");
      }
    }).finally(() => {
      if (requestAuthorityRef.current.isCurrent(requestRevision)) setLoading(false);
    }).then(() => initialLoadStatus);
    initialLoadInFlightRef.current = { key: transcriptKey, promise: initialLoadPromise };
    void initialLoadPromise.then(() => {
      if (initialLoadInFlightRef.current?.promise === initialLoadPromise) initialLoadInFlightRef.current = null;
    });
    return () => requestAuthorityRef.current.invalidate(requestRevision);
  }, [importedItems, loadTranscript, loadTranscriptLocator, logicalKey, queueLocator, refreshKey, setCurrentCursor, setCurrentItems, transcriptKey]);

  return {
    items,
    locatorItems: locatorState?.key === transcriptKey ? locatorState.items : undefined,
    loading,
    loadingMore,
    hasMore: Boolean(nextCursor),
    scrollRestorationReady: Boolean(transcriptKey && loadedIdentityRef.current === transcriptKey && !loading),
    loadMore,
    loadAll,
    loadUntilTarget,
  };
}
