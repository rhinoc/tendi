import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { createTranscriptNavigationAuthority, type TranscriptNavigationIntent } from "../transcript-navigation.ts";

export type TranscriptNavigationTarget = {
  key: string;
  groupKey?: string;
  index?: number;
};

function expandDisclosure(element: HTMLElement): boolean {
  const toggle = element.querySelector<HTMLButtonElement>(":scope > .disclosureSummaryRow > .disclosureSummary");
  if (toggle?.getAttribute("aria-expanded") === "false") {
    toggle.click();
    return true;
  }
  if (element instanceof HTMLDetailsElement && !element.open) {
    element.open = true;
    return true;
  }
  return false;
}

type TranscriptFocusRequest = {
  target: TranscriptNavigationTarget;
  preferSearchMatch: boolean;
  behavior: ScrollBehavior;
  navigationRevision: number;
};

export function useTranscriptNavigation({
  rootRef,
  items,
  renderRangeKey,
  scrollToIndex,
  scrollToBottom,
  indexFromKey,
  updateScrollEdges,
}: {
  rootRef: { current: HTMLElement | null };
  items: readonly unknown[];
  renderRangeKey: string;
  scrollToIndex: (index: number, behavior?: ScrollBehavior) => void;
  scrollToBottom: () => void;
  indexFromKey: (key: string) => number | null;
  updateScrollEdges: () => void;
}) {
  const authorityRef = useRef<ReturnType<typeof createTranscriptNavigationAuthority> | null>(null);
  if (!authorityRef.current) authorityRef.current = createTranscriptNavigationAuthority();
  const [highlightedKey, setHighlightedKey] = useState("");
  const [focusRequest, setFocusRequest] = useState<TranscriptFocusRequest | null>(null);
  const [focusRetryRevision, setFocusRetryRevision] = useState(0);
  const [pendingBottom, setPendingBottom] = useState(false);
  const [jumpingToBottom, setJumpingToBottom] = useState(false);
  const highlightTimerRef = useRef(0);
  const focusRetryFrameRef = useRef<number | null>(null);
  const scrolledFocusRevisionRef = useRef<number | null>(null);
  const authority = authorityRef.current;

  const beginNavigation = useCallback((intent: TranscriptNavigationIntent) => {
    const revision = authority.begin(intent);
    setFocusRequest(null);
    setPendingBottom(false);
    scrolledFocusRevisionRef.current = null;
    return revision;
  }, [authority]);

  const isCurrentNavigation = useCallback((revision: number) => authority.isCurrent(revision), [authority]);
  const currentNavigationIntent = useCallback(() => authority.currentIntent(), [authority]);

  const clearHighlight = useCallback(() => {
    window.clearTimeout(highlightTimerRef.current);
    setHighlightedKey("");
  }, []);

  const focusTarget = useCallback((
    target: TranscriptNavigationTarget,
    preferSearchMatch = false,
    behavior: ScrollBehavior = "smooth",
    navigationRevision?: number,
  ) => {
    if (navigationRevision === undefined || !authority.isCurrent(navigationRevision)) return;
    window.clearTimeout(highlightTimerRef.current);
    setHighlightedKey(target.key);
    setFocusRequest({ target, preferSearchMatch, behavior, navigationRevision });
  }, [authority]);

  const retryFocusAfterDisclosureChange = useCallback(() => {
    if (focusRetryFrameRef.current !== null) return;
    focusRetryFrameRef.current = window.requestAnimationFrame(() => {
      focusRetryFrameRef.current = null;
      setFocusRetryRevision((revision) => revision + 1);
    });
  }, []);

  useLayoutEffect(() => {
    const request = focusRequest;
    const root = rootRef.current;
    if (!request || !root) return;
    if (!authority.isCurrent(request.navigationRevision)) {
      setFocusRequest(null);
      return;
    }

    const targetIndex = request.target.index ?? indexFromKey(request.target.key);
    if (targetIndex !== null && targetIndex !== undefined
      && scrolledFocusRevisionRef.current !== request.navigationRevision) {
      scrolledFocusRevisionRef.current = request.navigationRevision;
      scrollToIndex(targetIndex);
    }

    if (request.target.groupKey) {
      const group = root.querySelector<HTMLElement>(`[data-transcript-key="${escapeSelectorValue(request.target.groupKey)}"]`);
      if (!group) return;
      if (expandDisclosure(group)) {
        retryFocusAfterDisclosureChange();
        return;
      }
    }

    const node = root.querySelector(`[data-transcript-key="${escapeSelectorValue(request.target.key)}"]`);
    if (!node) return;
    if (request.preferSearchMatch && node.classList.contains("disclosure")) {
      if (expandDisclosure(node as HTMLElement)) {
        retryFocusAfterDisclosureChange();
        return;
      }
    }
    const details = node.closest("details");
    if (details && !details.open) details.open = true;
    const destination = request.preferSearchMatch
      ? node.querySelector(".transcriptSearchMark") ?? node
      : node;
    setFocusRequest(null);
    const destinationBounds = destination.getBoundingClientRect();
    const rootBounds = root.getBoundingClientRect();
    const centeredTop = root.scrollTop
      + destinationBounds.top
      - rootBounds.top
      - (root.clientHeight - destinationBounds.height) / 2;
    root.scrollTo({ top: Math.max(0, centeredTop), behavior: request.behavior });
    highlightTimerRef.current = window.setTimeout(() => setHighlightedKey(""), 1800);
  }, [authority, focusRequest, focusRetryRevision, indexFromKey, items, renderRangeKey, retryFocusAfterDisclosureChange, rootRef, scrollToIndex]);

  useLayoutEffect(() => {
    if (!pendingBottom) return;
    scrollToBottom();
    updateScrollEdges();
    setPendingBottom(false);
  }, [items, pendingBottom, renderRangeKey, scrollToBottom, updateScrollEdges]);

  useEffect(() => () => {
    window.clearTimeout(highlightTimerRef.current);
    if (focusRetryFrameRef.current !== null) window.cancelAnimationFrame(focusRetryFrameRef.current);
  }, []);

  const scrollToTop = useCallback(() => {
    beginNavigation("top");
    scrollToIndex(0, "smooth");
  }, [beginNavigation, scrollToIndex]);

  const jumpToBottom = useCallback(async (hasMore: boolean, loadAll: () => Promise<void>) => {
    const navigationRevision = beginNavigation("bottom");
    scrollToBottom();
    setJumpingToBottom(true);
    try {
      if (hasMore) await loadAll();
    } finally {
      setJumpingToBottom(false);
      if (authority.isCurrent(navigationRevision)) setPendingBottom(true);
    }
  }, [authority, beginNavigation, scrollToBottom]);

  return {
    beginNavigation,
    isCurrentNavigation,
    currentNavigationIntent,
    focusTarget,
    highlightedKey,
    clearHighlight,
    jumpingToBottom,
    scrollToTop,
    jumpToBottom,
  };
}

function escapeSelectorValue(value: string) {
  return window.CSS?.escape ? window.CSS.escape(value) : value.replace(/["\\]/g, "\\$&");
}
