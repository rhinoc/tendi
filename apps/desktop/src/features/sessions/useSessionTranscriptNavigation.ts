import { useCallback, useEffect, useRef, useState } from "react";

import type { SessionSkillLinkRecord } from "../../lib/sessions.ts";
import { groupTranscriptItems, type TranscriptSearchHit, type TranscriptSearchResult } from "../../lib/transcript.ts";
import { logger } from "../../lib/logger.ts";
import type { TranscriptNavigationTarget } from "../../lib/hooks/use-transcript-navigation.ts";
import type { TranscriptNavigationIntent } from "../../lib/transcript-navigation.ts";
import {
  findSkillEvidenceTarget,
  transcriptSearchIndexForBackendGroups,
  transcriptSearchTargetForHit,
  type SessionLocatorItem,
  type TranscriptItemRecord,
  type TranscriptSearchScopeState,
  type TranscriptSearchTarget,
} from "./session-transcript-logic.ts";
import type {
  SessionTranscriptTargetLoadOptions,
  SessionTranscriptTargetLoadResult,
} from "./useSessionTranscript.ts";

type PendingUserMessageTarget = {
  key: string;
  index: number;
  sessionKey: string;
  navigationRevision: number;
};

type NavigationAdapter = {
  beginNavigation: (intent: TranscriptNavigationIntent) => number;
  isCurrentNavigation: (revision: number) => boolean;
  currentNavigationIntent: () => TranscriptNavigationIntent;
  focusTarget: (
    target: TranscriptNavigationTarget,
    preferSearchMatch?: boolean,
    behavior?: ScrollBehavior,
    navigationRevision?: number,
  ) => void;
  indexFromKey: (key: string) => number | null;
};

type TranscriptData = {
  items: TranscriptItemRecord[];
  locatorItems: SessionLocatorItem[];
  loadUntilTarget: (options: SessionTranscriptTargetLoadOptions) => Promise<SessionTranscriptTargetLoadResult>;
};

type TranscriptSearchState = {
  query: string;
  scopes: TranscriptSearchScopeState;
  targets: TranscriptSearchTarget[];
  ready: boolean;
  loading: boolean;
  error: boolean;
  result: TranscriptSearchResult | null;
  remote: boolean;
};

type KeyboardAdapter = {
  markDetailKeyboardScope: () => void;
  isDetailKeyboardScope: () => boolean;
  visibleUserMessageIndex: () => number;
};

type UseSessionTranscriptNavigationOptions = {
  sessionKey: string;
  navigationSearchQuery: string;
  transcript: TranscriptData;
  search: TranscriptSearchState;
  navigation: NavigationAdapter;
  keyboard: KeyboardAdapter;
  onReportError?: (message: string) => void;
};

export function useSessionTranscriptNavigation({
  sessionKey,
  navigationSearchQuery,
  transcript,
  search,
  navigation,
  keyboard,
  onReportError,
}: UseSessionTranscriptNavigationOptions) {
  const { items: transcriptItems, locatorItems, loadUntilTarget } = transcript;
  const {
    query: searchQuery,
    scopes: searchScopes,
    targets: searchTargets,
    ready: searchReady,
    loading: searchLoading,
    error: searchError,
    result: searchResult,
    remote: remoteSearchActive,
  } = search;
  const {
    beginNavigation,
    isCurrentNavigation,
    currentNavigationIntent,
    focusTarget,
    indexFromKey,
  } = navigation;
  const { markDetailKeyboardScope, isDetailKeyboardScope, visibleUserMessageIndex } = keyboard;
  const [searchIndex, setSearchIndex] = useState(0);
  const [jumpingSkillPath, setJumpingSkillPath] = useState("");
  const [pendingUserMessageTarget, setPendingUserMessageTarget] = useState<PendingUserMessageTarget | null>(null);
  const transcriptItemsRef = useRef(transcriptItems);
  transcriptItemsRef.current = transcriptItems;
  const transcriptNavigationKeyRef = useRef("");
  const transcriptNavigationPromiseRef = useRef(Promise.resolve());
  const searchResultCount = remoteSearchActive ? (searchResult?.hits.length ?? 0) : searchTargets.length;

  const ensureSearchHitLoaded = useCallback((hit: TranscriptSearchHit, navigationRevision: number) => (
    loadUntilTarget({
      isTargetLoaded: (loadedItems) => (
        transcriptSearchIndexForBackendGroups(loadedItems).groupCount > hit.groupIndex
      ),
      isCurrent: () => isCurrentNavigation(navigationRevision),
      onItemsLoaded: (loadedItems) => { transcriptItemsRef.current = loadedItems; },
    })
  ), [isCurrentNavigation, loadUntilTarget, transcriptItemsRef]);

  const ensureTranscriptIndexLoaded = useCallback((targetIndex: number, navigationRevision: number) => {
    if (!Number.isSafeInteger(targetIndex) || targetIndex < 0) {
      return Promise.resolve({
        items: transcriptItemsRef.current,
        loaded: false,
        status: "exhausted" as const,
        cancelled: false,
      });
    }
    return loadUntilTarget({
      isTargetLoaded: (loadedItems) => loadedItems.length > targetIndex,
      isCurrent: () => isCurrentNavigation(navigationRevision),
      onItemsLoaded: (loadedItems) => { transcriptItemsRef.current = loadedItems; },
    });
  }, [isCurrentNavigation, loadUntilTarget, transcriptItemsRef]);

  const jumpToSkillEvidence = useCallback(async (link: SessionSkillLinkRecord) => {
    if (jumpingSkillPath) return;
    const navigationRevision = beginNavigation("skill");
    setJumpingSkillPath(link.skill_path);
    try {
      const result = await loadUntilTarget({
        isTargetLoaded: (loadedItems) => Boolean(findSkillEvidenceTarget(loadedItems, link)),
        isCurrent: () => isCurrentNavigation(navigationRevision),
        onItemsLoaded: (loadedItems) => { transcriptItemsRef.current = loadedItems; },
      });
      if (result.cancelled) return;
      const target = findSkillEvidenceTarget(result.items, link);
      if (target) {
        focusTarget(target, false, "smooth", navigationRevision);
      } else if (isCurrentNavigation(navigationRevision)) {
        onReportError?.(`Could not find ${link.skill_name} usage in this transcript.`);
      }
    } finally {
      setJumpingSkillPath("");
    }
  }, [beginNavigation, focusTarget, isCurrentNavigation, jumpingSkillPath, loadUntilTarget, onReportError, transcriptItemsRef]);

  const selectLocatorItem = useCallback((key: string, behavior?: ScrollBehavior) => {
    const index = indexFromKey(key);
    if (index === null) return;
    const navigationRevision = beginNavigation("locator");
    markDetailKeyboardScope();
    transcriptNavigationKeyRef.current = key;
    void ensureTranscriptIndexLoaded(index, navigationRevision).then((result) => {
      if (result.cancelled) return;
      if (result.loaded) focusTarget({ key, index }, false, behavior, navigationRevision);
      else if (result.status === "exhausted" && isCurrentNavigation(navigationRevision)) {
        logger.warn("sessions transcript locator target exhausted", {
          targetIndex: index,
          loadedGroupCount: transcriptItemsRef.current.length,
          locatorItemCount: locatorItems.length,
        });
        onReportError?.("Could not locate this message in the transcript.");
      }
    });
  }, [beginNavigation, ensureTranscriptIndexLoaded, focusTarget, indexFromKey, isCurrentNavigation, locatorItems.length, markDetailKeyboardScope, onReportError, transcriptItemsRef]);

  const moveUserMessage = useCallback((offset: number) => {
    if (locatorItems.length === 0) return;
    const currentKey = transcriptNavigationKeyRef.current;
    const currentIndex = locatorItems.findIndex((item) => item.key === currentKey);
    const baseIndex = currentIndex >= 0 ? currentIndex : visibleUserMessageIndex();
    const targetIndex = baseIndex < 0 ? (offset > 0 ? 0 : locatorItems.length - 1) : baseIndex + offset;
    if (targetIndex < 0 || targetIndex >= locatorItems.length) return;
    const target = locatorItems[targetIndex];
    const navigationRevision = beginNavigation("locator");
    transcriptNavigationKeyRef.current = target.key;
    if (groupTranscriptItems(transcriptItemsRef.current).length > target.index) {
      focusTarget({ key: target.key, index: target.index }, false, "smooth", navigationRevision);
      return;
    }
    const navigationSessionKey = sessionKey;
    setPendingUserMessageTarget({
      key: target.key,
      index: target.index,
      sessionKey: navigationSessionKey,
      navigationRevision,
    });
    const loadTarget = async () => {
      const result = await ensureTranscriptIndexLoaded(target.index, navigationRevision);
      if (result.cancelled || !isCurrentNavigation(navigationRevision)) return;
      if (!result.loaded) {
        setPendingUserMessageTarget((current) => (
          current?.key === target.key
            && current.sessionKey === navigationSessionKey
            && current.navigationRevision === navigationRevision
            ? null
            : current
        ));
      }
    };
    transcriptNavigationPromiseRef.current = transcriptNavigationPromiseRef.current.then(loadTarget, loadTarget);
  }, [beginNavigation, ensureTranscriptIndexLoaded, focusTarget, isCurrentNavigation, locatorItems, sessionKey, transcriptItemsRef, visibleUserMessageIndex]);

  useEffect(() => {
    const pending = pendingUserMessageTarget;
    if (!pending) return;
    if (
      !isDetailKeyboardScope()
      || pending.sessionKey !== sessionKey
      || !isCurrentNavigation(pending.navigationRevision)
    ) {
      setPendingUserMessageTarget(null);
      return;
    }
    if (groupTranscriptItems(transcriptItems).length <= pending.index) return;
    setPendingUserMessageTarget(null);
    focusTarget({ key: pending.key, index: pending.index }, false, "smooth", pending.navigationRevision);
  }, [focusTarget, isCurrentNavigation, isDetailKeyboardScope, pendingUserMessageTarget, sessionKey, transcriptItems]);

  useEffect(() => {
    if (!locatorItems.some((item) => item.key === transcriptNavigationKeyRef.current)) {
      transcriptNavigationKeyRef.current = "";
    }
  }, [locatorItems]);

  useEffect(() => {
    beginNavigation(navigationSearchQuery ? "search" : "idle");
    transcriptNavigationKeyRef.current = "";
    transcriptNavigationPromiseRef.current = Promise.resolve();
    setPendingUserMessageTarget(null);
  }, [beginNavigation, navigationSearchQuery, sessionKey]);

  useEffect(() => {
    setSearchIndex(0);
  }, [searchQuery, searchScopes, sessionKey]);

  useEffect(() => {
    if (!searchReady || searchLoading || searchError || searchResultCount === 0) return;
    if (currentNavigationIntent() !== "idle" && currentNavigationIntent() !== "search") return;
    const navigationRevision = beginNavigation("search");
    if (remoteSearchActive) {
      const hit = searchResult?.hits[searchIndex];
      if (!hit) return;
      let cancelled = false;
      void ensureSearchHitLoaded(hit, navigationRevision).then((result) => {
        if (
          cancelled
          || result.cancelled
          || !isCurrentNavigation(navigationRevision)
          || currentNavigationIntent() !== "search"
        ) return;
        const target = transcriptSearchTargetForHit(result.items, hit);
        if (target) focusTarget(target, true, "smooth", navigationRevision);
      });
      return () => {
        cancelled = true;
      };
    }
    const target = searchTargets[searchIndex];
    if (target) focusTarget(target, true, "smooth", navigationRevision);
  }, [
    beginNavigation,
    currentNavigationIntent,
    ensureSearchHitLoaded,
    focusTarget,
    isCurrentNavigation,
    remoteSearchActive,
    searchError,
    searchIndex,
    searchLoading,
    searchReady,
    searchResult,
    searchResultCount,
    searchTargets,
    transcriptItems,
  ]);

  const moveSearchResult = useCallback((offset: number) => {
    if (searchResultCount === 0) return;
    beginNavigation("search");
    setSearchIndex((current) => (current + offset + searchResultCount) % searchResultCount);
  }, [beginNavigation, searchResultCount]);

  return {
    searchIndex,
    searchResultCount,
    jumpingSkillPath,
    jumpToSkillEvidence,
    selectLocatorItem,
    moveUserMessage,
    moveSearchResult,
  };
}
