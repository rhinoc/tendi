import { Tooltip as AppTooltip } from "../components/shared/Tooltip.tsx";
import { lazy, memo, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ChangeEvent, type ReactNode } from "react";
import { AlertCircle, AppWindow, ArrowDownToLine, ArrowUpToLine, Check, ChevronLeft, ChevronRight, Filter, FolderOpen, GitFork, Info, LocateFixed, MessageSquarePlus, MessageSquareText, PanelRightClose, RefreshCw, Search, SearchX, Sparkles, TerminalSquare, Upload, X } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu, Dialog, DropdownMenu, Popover } from "radix-ui";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import { SortDirection } from "../lib/sort.ts";
import { getTabScrollPosition, setTabScrollPosition, useTabState } from "../lib/tab-state.ts";
import { useTranscriptNavigation } from "../lib/hooks/use-transcript-navigation.ts";
import {
  buildSessionLocatorItems,
  buildSessionLocatorItemsFromMetadata,
  DEFAULT_TRANSCRIPT_SEARCH_SCOPES,
  findTranscriptSearchTargets,
  transcriptGroupChildKey,
  transcriptItemKey,
  transcriptItemSearchQuery,
  TRANSCRIPT_SEARCH_SCOPES,
  type SessionLocatorItem,
  type TranscriptItemRecord,
  type TranscriptLoadMoreResult,
  type TranscriptSearchScopeState,
} from "../features/sessions/session-transcript-logic.ts";
import {
  useSessionTranscript,
  type SessionTranscriptTargetLoadOptions,
  type SessionTranscriptTargetLoadResult,
} from "../features/sessions/useSessionTranscript.ts";
import { useSessionTranscriptSearch } from "../features/sessions/useSessionTranscriptSearch.ts";
import { useSessionTranscriptNavigation } from "../features/sessions/useSessionTranscriptNavigation.ts";
import { sessionSourceIdentity, useInferredSessionResumeTargets } from "../features/sessions/useInferredSessionResumeTargets.ts";
import { useSessionListWorkspace } from "../features/sessions/useSessionListWorkspace.ts";
import { useSessionRemoteListWorkspace } from "../features/sessions/useSessionRemoteListWorkspace.ts";
import { useSessionResumeOperations } from "../features/sessions/useSessionResumeOperations.ts";
import { useSessionSkillLinks } from "../features/sessions/useSessionSkillLinks.ts";
import { ImportFeedbackState, useSessionImport } from "../features/sessions/useSessionImport.ts";
import { agentDefinitions } from "../lib/agent/index.ts";
import type { TokenMetricProps } from "../components/TokenStatusBar.tsx";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { Button } from "../components/shared/Button.tsx";
import { CheckboxIndicator } from "../components/shared/CheckboxIndicator.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { Disclosure } from "../components/shared/Disclosure.tsx";
import { CopyableSessionId } from "../features/sessions/CopyableSessionId.tsx";
import { CopyPathMenuItem, CopyTextMenuItem, OpenInEditorMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { DialogActionButton } from "../components/shared/DialogActionButton.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { InfoDropdownMenu } from "../components/shared/InfoDropdownMenu.tsx";
import { InfoSection } from "../components/shared/InfoSection.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadErrorState } from "../components/shared/LoadErrorState.tsx";
import { LoadingDots } from "../components/shared/LoadingDots.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { MenuContent } from "../components/shared/MenuContent.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { SearchClearButton } from "../components/shared/SearchClearButton.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { StatefulButton } from "../components/shared/StatefulButton.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { ToolCall } from "../components/shared/ToolCall.tsx";
import { useVirtualViewport } from "../components/shared/useVirtualViewport.ts";
import { findTextRanges } from "../components/shared/text-ranges.ts";
import { SkillRelationshipMap } from "../features/skills/SkillRelationshipMap.tsx";
import { createSessionTableColumns } from "../features/sessions/createSessionTableColumns.tsx";
import { SessionTitleText, TranscriptLinkText } from "../components/shared/TranscriptLinkText.tsx";
import { PretextText } from "../components/shared/PretextText.tsx";
import "./SessionsView.css";
import { cacheRateTone } from "../lib/token-style.ts";
import { TokenUsageSource } from "../lib/tokenizer-types.ts";
import { planSessionListLocation, SessionListLocationPlan, shouldShowSessionListLocator } from "../lib/session-list-locator.ts";
import { measureTranscriptTextHeight, type TranscriptTextLayout } from "../lib/pretext-layout.ts";
import { fixedVirtualRange, variableVirtualRangeFor } from "../lib/virtualization.ts";
import {
  SESSION_FREEZE_COLUMN,
  AsyncStatus,
  EMPTY_DISPLAY_VALUE,
  TauriCommand,
  copiedPathLabel,
  copiedValueLabel,
  copyPathLabel,
  copyValueLabel,
  compactDateTime,
  formatDuration,
  formatSessionTitle,
  formatTranscriptPreview,
  formatUserPath,
  friendlyAgent,
  groupTranscriptItems,
  isWebSource,
  logger,
  promptActionLabels,
  revealPathLabel,
  sessionExternalKey,
  resolveInitialSession,
  safeInvoke,
  sessionCacheRate,
  sessionTitleValue,
  sessionLogicalIdentity,
  sessionSourceExternalKey,
  sessionKind,
  sessionAppDeepLink,
  sessionResumeLabel,
  sessionResumeTargetForAgent,
  sessionResumeTargetForMenu,
  sessionResumeTargetsForMenu,
  sessionWorkspace,
  sessionWorkspacePath,
  SESSION_SEARCH_SORT,
  SessionResumeTarget,
  SessionKind,
  SessionSortKey,
  TranscriptGroupType,
  transcriptContextPreview,
  transcriptEvidenceSearchText,
  transcriptItemType,
  isTranscriptExecItem,
} from "../lib/index.ts";
import type {
  TranscriptLocatorItem,
  TranscriptLocatorPage,
  TranscriptPage,
  TranscriptSearchResult,
  TranscriptSearchScopes,
  MissingSessionProjectPolicy,
  ProjectSummary,
  SessionProjectSummary,
  SessionResumeOutcome,
  SessionRecord,
  SessionSkillLinkRecord,
} from "../lib/index.ts";
import type { SkillIndexStatus } from "../store/desktop-store.ts";
import {
  IMPORTED_SESSION_AGENT,
  selectSessionRelationsGraph,
  selectSessionSkillsConvergenceGraph,
  selectVisibleSessionProjectOptions,
  sessionPageContextKey,
  sessionPageForRow,
  selectSessionRelationships,
  mergeSessionListRows,
  sessionTableRowId,
  type GroupedSessionPage,
  type SessionListPageRequest,
  type SessionListPageResult,
} from "../controllers/session-controller.ts";

const SessionTokenStatusBar = lazy(() => import("../components/SessionTokenStatusBar.tsx").then(({ SessionTokenStatusBar: component }) => ({ default: component })));

const SESSION_SEARCH_DEBOUNCE_MS = 300;
const SESSION_LOCATOR_MIN_ITEMS = 4;
const SESSION_REFRESH_ERROR = "Could not refresh sessions. Try again.";
const TRANSCRIPT_IMPORT_PROVIDER_PLACEHOLDER = "__choose_transcript_provider__";
const TRANSCRIPT_IMPORT_PROVIDERS = agentDefinitions
  .filter((definition) => definition.transcriptParser)
  .map((definition) => ({ value: definition.id, label: definition.displayName }));
const EMPTY_SESSION_ROWS: SessionRecord[] = [];
const EMPTY_SESSION_PROJECT_OPTIONS: SessionListPageResult["projectOptions"] = [];
const EMPTY_GROUPED_SESSION_PAGES: GroupedSessionPage[] = [];

function sessionCacheMetrics(session: SessionRecord): TokenMetricProps[] {
  const usage = session.tokenUsage;
  const rate = sessionCacheRate(session);
  if (!usage || rate === undefined) return [];
  const value = `${rate.toFixed(1)}%`;
  return [{
    label: "Cache",
    value,
    title: `${usage.cachedInputTokens.toLocaleString()} cached of ${usage.inputTokens.toLocaleString()} input tokens`,
    tone: cacheRateTone(rate),
  }];
}

function reportedTokenSegments(session: SessionRecord) {
  const usage = session.tokenUsage;
  if (!usage) return null;
  const uncachedInputTokens = Math.max(0, usage.inputTokens - usage.cachedInputTokens);
  const nonReasoningOutputTokens = Math.max(0, usage.outputTokens - usage.reasoningOutputTokens);
  return [
    {
      label: "Input",
      value: usage.inputTokens,
      details: [
        { label: "Cached", value: usage.cachedInputTokens },
        { label: "Uncached", value: uncachedInputTokens },
      ],
    },
    {
      label: "Output",
      value: usage.outputTokens,
      details: [
        { label: "Reasoning", value: usage.reasoningOutputTokens },
        { label: "Other", value: nonReasoningOutputTokens },
      ],
    },
    {
      label: "Total",
      value: usage.totalTokens,
      details: [
        { label: "Input", value: usage.inputTokens },
        { label: "Output", value: usage.outputTokens },
      ],
    },
  ];
}

enum KeyboardNavigationScope {
  List = "list",
  Detail = "detail",
}

type KeyboardNavigationScopeRef = {
  current: KeyboardNavigationScope;
};

const TRANSCRIPT_VIRTUAL_THRESHOLD = 120;
const TRANSCRIPT_VIRTUAL_OVERSCAN = 12;
const TRANSCRIPT_DEFAULT_ITEM_HEIGHT = 96;
const TRANSCRIPT_ITEM_VERTICAL_INSET = 8;
const TRANSCRIPT_BUBBLE_TOP_INSET = 20;
const TRANSCRIPT_CHAT_VERTICAL_CHROME = 66;
const TRANSCRIPT_CHAT_FALLBACK_BODY_HEIGHT = 60;
const SESSION_TABLE_ROW_HEIGHT = 72;
const SESSION_LOCATOR_ROW_HEIGHT = 10;
const SESSION_LOCATOR_OVERSCAN = 16;

function isChatTranscriptType(type: string | undefined) {
  return type === "user" || type === "assistant";
}

function parseCssPixels(value: string, fallback: number) {
  const parsed = Number.parseFloat(value);
  return Number.isFinite(parsed) && parsed > 0 ? parsed : fallback;
}

function transcriptTextLayoutFor(root: HTMLDivElement | null, viewportWidth: number): TranscriptTextLayout | null {
  if (!root || viewportWidth <= 0) return null;
  const styles = getComputedStyle(root);
  const fontSize = parseCssPixels(styles.getPropertyValue("--text-ui"), 14);
  const lineHeight = parseCssPixels(styles.getPropertyValue("--leading-body"), 20);
  const horizontalPadding = parseCssPixels(styles.getPropertyValue("--transcript-inline-padding"), 16);
  const font = `${styles.fontWeight || "400"} ${fontSize}px ${styles.fontFamily}`;
  const transcriptContentWidth = Math.max(1, viewportWidth - horizontalPadding * 2);
  const maxBubbleWidth = Math.min(transcriptContentWidth * 0.8, 600);
  return {
    contentWidth: Math.max(1, maxBubbleWidth - horizontalPadding * 2 - 2),
    font,
    linkFont: `600 ${fontSize}px ${styles.fontFamily}`,
    lineHeight,
    letterSpacing: styles.letterSpacing === "normal" ? 0 : parseCssPixels(styles.letterSpacing, 0),
  };
}

function estimatedTranscriptItemHeight(
  item: TranscriptItemRecord,
  previousItem?: TranscriptItemRecord,
  textLayout?: TranscriptTextLayout | null,
): number {
  const type = transcriptItemType(item);
  const previousType = previousItem ? transcriptItemType(previousItem) : undefined;
  const topInset = isChatTranscriptType(type) && !isChatTranscriptType(previousType)
    ? TRANSCRIPT_BUBBLE_TOP_INSET
    : 0;
  switch (type) {
    case "user":
    case "assistant":
      return TRANSCRIPT_CHAT_VERTICAL_CHROME
        + (textLayout ? (measureTranscriptTextHeight(item.body, textLayout) ?? TRANSCRIPT_CHAT_FALLBACK_BODY_HEIGHT) : TRANSCRIPT_CHAT_FALLBACK_BODY_HEIGHT)
        + topInset;
    case TranscriptGroupType.ToolGroup:
      return 52 + TRANSCRIPT_ITEM_VERTICAL_INSET;
    case "thinking":
    case "reasoning":
      return 88 + TRANSCRIPT_ITEM_VERTICAL_INSET;
    case "compaction":
    case "model_config":
      return 42 + TRANSCRIPT_ITEM_VERTICAL_INSET;
    default:
      return TRANSCRIPT_DEFAULT_ITEM_HEIGHT + TRANSCRIPT_ITEM_VERTICAL_INSET;
  }
}

function transcriptIndexFromKey(key: string): number | null {
  const match = key.match(/(?:^|-)(\d+)(?:-|$)/);
  if (!match) return null;
  const index = Number(match[1]);
  return Number.isSafeInteger(index) ? index : null;
}

function isKeyboardNavigationIgnoredTarget(target: EventTarget | null) {
  if (!(target instanceof Element)) return false;
  return Boolean(target.closest(
    "input, textarea, select, button, a, [contenteditable=\"true\"], [role=\"button\"], [role=\"menuitem\"], [role=\"menuitemcheckbox\"], [role=\"menuitemradio\"]",
  ));
}

function transcriptRangeForViewport(
  itemCount: number,
  offsets: number[],
  scrollTop: number,
  viewportHeight: number,
  virtualized: boolean,
) {
  if (!virtualized || itemCount === 0) return { start: 0, end: itemCount };
  return variableVirtualRangeFor(
    offsets,
    scrollTop,
    viewportHeight,
    TRANSCRIPT_DEFAULT_ITEM_HEIGHT + TRANSCRIPT_ITEM_VERTICAL_INSET,
    TRANSCRIPT_VIRTUAL_OVERSCAN,
  );
}

function useTranscriptVirtualizer(
  items: TranscriptItemRecord[],
  rootRef: { current: HTMLDivElement | null },
  resetKey: string,
  scrollRestorationReady: boolean,
) {
  const virtualized = items.length >= TRANSCRIPT_VIRTUAL_THRESHOLD;
  const scrollRestorationKey = `sessions.transcript:${resetKey}`;
  const {
    size: viewportSize,
    readViewportSize,
  } = useVirtualViewport<HTMLDivElement>(
    { width: 0, height: 720 },
    {
      ref: rootRef,
      enabled: virtualized,
      refreshKey: items,
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ height }) => height > 0,
      isEqual: (current, next) => current.width === next.width && current.height === next.height,
    },
  );
  const [measurementVersion, setMeasurementVersion] = useState(0);
  const measuredHeightsRef = useRef(new Map<number, number>());
  const measuredNodesRef = useRef(new Map<number, HTMLElement>());
  const resizeObserverRef = useRef<ResizeObserver | null>(null);
  const stickToBottomRef = useRef(false);
  const stickToBottomFrameRef = useRef<number | null>(null);
  const scrollSyncFrameRef = useRef<number | null>(null);
  const restoredScrollKeyRef = useRef<string | undefined>(undefined);
  const scrollRestorationKeyRef = useRef(scrollRestorationKey);
  const [scrollOffset, setScrollOffset] = useState(0);
  const scrollOffsetRef = useRef(0);

  const rememberScrollPosition = useCallback((root: HTMLDivElement) => {
    setTabScrollPosition(scrollRestorationKey, {
      top: root.scrollTop,
      left: root.scrollLeft,
    });
  }, [scrollRestorationKey]);

  const transcriptTextLayout = useMemo(
    () => virtualized ? transcriptTextLayoutFor(rootRef.current, viewportSize.width) : null,
    [rootRef, viewportSize.width, virtualized],
  );
  const estimatedHeights = useMemo(
    () => items.map((item, index) => estimatedTranscriptItemHeight(item, items[index - 1], transcriptTextLayout)),
    [items, transcriptTextLayout],
  );
  const offsets = useMemo(() => {
    const next = new Array<number>(items.length + 1).fill(0);
    for (let index = 0; index < items.length; index += 1) {
      next[index + 1] = next[index]
        + (measuredHeightsRef.current.get(index) ?? estimatedHeights[index] ?? TRANSCRIPT_DEFAULT_ITEM_HEIGHT);
    }
    return next;
  }, [estimatedHeights, items.length, measurementVersion]);
  const offsetsRef = useRef(offsets);
  offsetsRef.current = offsets;
  const viewportHeight = readViewportSize();

  const range = useMemo(() => {
    return transcriptRangeForViewport(items.length, offsets, scrollOffset, viewportHeight, virtualized);
  }, [items.length, offsets, scrollOffset, viewportHeight, virtualized]);
  const renderedRangeRef = useRef(range);
  renderedRangeRef.current = range;

  const syncScrollPosition = useCallback(() => {
    const root = rootRef.current;
    const next = root ? Math.max(0, Number.isFinite(root.scrollTop) ? root.scrollTop : 0) : 0;
    scrollOffsetRef.current = next;
    const nextRange = transcriptRangeForViewport(
      items.length,
      offsetsRef.current,
      next,
      readViewportSize(),
      virtualized,
    );
    const currentRange = renderedRangeRef.current;
    if (currentRange.start !== nextRange.start || currentRange.end !== nextRange.end) {
      renderedRangeRef.current = nextRange;
      setScrollOffset(next);
    }
    return next;
  }, [items.length, readViewportSize, rootRef, virtualized]);

  const scheduleScrollSync = useCallback(() => {
    if (!virtualized || scrollSyncFrameRef.current !== null) return;
    scrollSyncFrameRef.current = window.requestAnimationFrame(() => {
      scrollSyncFrameRef.current = null;
      syncScrollPosition();
    });
  }, [syncScrollPosition, virtualized]);

  const scrollToCurrentBottom = useCallback(() => {
    if (!stickToBottomRef.current) return;
    const root = rootRef.current;
    if (!root) return;
    const nextScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
    if (Math.abs(root.scrollTop - nextScrollTop) <= 1) return;
    root.scrollTop = nextScrollTop;
    syncScrollPosition();
  }, [rootRef, syncScrollPosition]);

  const scheduleStickToBottom = useCallback(() => {
    if (stickToBottomFrameRef.current !== null) return;
    stickToBottomFrameRef.current = window.requestAnimationFrame(() => {
      stickToBottomFrameRef.current = null;
      scrollToCurrentBottom();
    });
  }, [scrollToCurrentBottom]);

  const cancelStickToBottom = useCallback(() => {
    stickToBottomRef.current = false;
    if (stickToBottomFrameRef.current !== null) {
      window.cancelAnimationFrame(stickToBottomFrameRef.current);
      stickToBottomFrameRef.current = null;
    }
  }, []);

  useLayoutEffect(() => {
    if (!stickToBottomRef.current) return;
    scheduleStickToBottom();
  }, [items.length, offsets, scheduleStickToBottom, viewportHeight]);

  useLayoutEffect(() => {
    const root = rootRef.current;
    if (root && scrollRestorationKeyRef.current !== scrollRestorationKey) {
      setTabScrollPosition(scrollRestorationKeyRef.current, {
        top: root.scrollTop,
        left: root.scrollLeft,
      });
    }
    scrollRestorationKeyRef.current = scrollRestorationKey;
    measuredHeightsRef.current.clear();
    cancelStickToBottom();
    restoredScrollKeyRef.current = undefined;
    if (root) root.scrollTop = 0;
    syncScrollPosition();
    setMeasurementVersion((current) => current + 1);
  }, [cancelStickToBottom, resetKey, rootRef, scrollRestorationKey, syncScrollPosition]);

  useLayoutEffect(() => {
    if (!scrollRestorationReady) return;
    if (restoredScrollKeyRef.current === scrollRestorationKey) return;
    const storedPosition = getTabScrollPosition(scrollRestorationKey);
    if (!storedPosition) {
      restoredScrollKeyRef.current = scrollRestorationKey;
      return;
    }
    const root = rootRef.current;
    if (!root || items.length === 0) return;
    const maxScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
    const maxScrollLeft = Math.max(0, root.scrollWidth - root.clientWidth);
    root.scrollTo({
      top: Math.min(storedPosition.top, maxScrollTop),
      left: Math.min(storedPosition.left, maxScrollLeft),
      behavior: "auto",
    });
    restoredScrollKeyRef.current = scrollRestorationKey;
    rememberScrollPosition(root);
    syncScrollPosition();
  }, [items.length, rememberScrollPosition, rootRef, scrollRestorationKey, scrollRestorationReady, syncScrollPosition]);

  useEffect(() => {
    const root = rootRef.current;
    if (!root) return undefined;
    const onScroll = () => {
      rememberScrollPosition(root);
      if (virtualized) scheduleScrollSync();
    };
    const onUserScrollStart = () => cancelStickToBottom();
    syncScrollPosition();
    root.addEventListener("scroll", onScroll, { passive: true });
    root.addEventListener("wheel", onUserScrollStart, { passive: true });
    root.addEventListener("touchstart", onUserScrollStart, { passive: true });
    root.addEventListener("pointerdown", onUserScrollStart, { passive: true });
    return () => {
      root.removeEventListener("scroll", onScroll);
      root.removeEventListener("wheel", onUserScrollStart);
      root.removeEventListener("touchstart", onUserScrollStart);
      root.removeEventListener("pointerdown", onUserScrollStart);
    };
  }, [cancelStickToBottom, rememberScrollPosition, rootRef, scheduleScrollSync, syncScrollPosition, virtualized]);

  useEffect(() => () => {
    if (stickToBottomFrameRef.current !== null) {
      window.cancelAnimationFrame(stickToBottomFrameRef.current);
    }
    if (scrollSyncFrameRef.current !== null) {
      window.cancelAnimationFrame(scrollSyncFrameRef.current);
    }
    const root = rootRef.current;
    if (root) setTabScrollPosition(scrollRestorationKeyRef.current, {
      top: root.scrollTop,
      left: root.scrollLeft,
    });
  }, [rootRef]);

  useEffect(() => {
    if (items.length === 0 || typeof ResizeObserver === "undefined") return undefined;
    const observer = new ResizeObserver((entries) => {
      const layout = offsetsRef.current;
      let changed = false;
      let anchorDelta = 0;
      for (const entry of entries) {
        const index = Number((entry.target as HTMLElement).dataset.transcriptIndex);
        if (!Number.isInteger(index)) continue;
        const nextHeight = entry.contentRect.height;
        if (nextHeight <= 0) continue;
        const previousHeight = measuredHeightsRef.current.get(index)
          ?? estimatedHeights[index]
          ?? TRANSCRIPT_DEFAULT_ITEM_HEIGHT;
        if (Math.abs(previousHeight - nextHeight) < 1) continue;
        measuredHeightsRef.current.set(index, nextHeight);
        changed = true;
        if (virtualized && layout[index] < scrollOffsetRef.current) anchorDelta += nextHeight - previousHeight;
      }
      if (!changed) return;
      const root = rootRef.current;
      if (root && stickToBottomRef.current) {
        scheduleStickToBottom();
      } else if (virtualized && root && anchorDelta !== 0) {
        root.scrollTop += anchorDelta;
      }
      if (virtualized) {
        syncScrollPosition();
        setMeasurementVersion((current) => current + 1);
      }
    });
    resizeObserverRef.current = observer;
    for (const node of measuredNodesRef.current.values()) observer.observe(node);
    return () => {
      observer.disconnect();
      resizeObserverRef.current = null;
    };
  }, [estimatedHeights, items, rootRef, scheduleStickToBottom, syncScrollPosition, virtualized, scrollOffsetRef]);

  const measureItem = useCallback((index: number, node: HTMLElement | null) => {
    const previous = measuredNodesRef.current.get(index);
    if (previous === node) return;
    if (previous) resizeObserverRef.current?.unobserve(previous);
    if (node) {
      measuredNodesRef.current.set(index, node);
      resizeObserverRef.current?.observe(node);
    } else {
      measuredNodesRef.current.delete(index);
    }
  }, []);

  const scrollToIndex = useCallback((index: number, behavior: ScrollBehavior = "auto") => {
    const root = rootRef.current;
    if (!root || items.length === 0) return;
    if (!Number.isSafeInteger(index) || index < 0 || index >= items.length) return;
    cancelStickToBottom();
    const top = offsetsRef.current[index] ?? 0;
    root.scrollTo({ top, behavior });
    rememberScrollPosition(root);
    syncScrollPosition();
  }, [cancelStickToBottom, items.length, rememberScrollPosition, rootRef, syncScrollPosition]);

  const scrollToBottom = useCallback((behavior: ScrollBehavior = "auto") => {
    const root = rootRef.current;
    if (!root) return;
    stickToBottomRef.current = true;
    const top = Math.max(0, root.scrollHeight - root.clientHeight);
    root.scrollTo({ top, behavior });
    rememberScrollPosition(root);
    syncScrollPosition();
    scheduleStickToBottom();
  }, [rememberScrollPosition, rootRef, scheduleStickToBottom, syncScrollPosition]);

  return {
    virtualized,
    range,
    rangeKey: `${range.start}:${range.end}:${measurementVersion}`,
    topSpacerHeight: offsets[range.start] ?? 0,
    bottomSpacerHeight: Math.max(0, (offsets[items.length] ?? 0) - (offsets[range.end] ?? 0)),
    measureItem,
    scrollToIndex,
    scrollToBottom,
  };
}

function linkSkillName(link: SessionSkillLinkRecord) {
  return link.skill_name;
}


function linkEvidenceText(link: SessionSkillLinkRecord) {
  return transcriptEvidenceSearchText(link.evidence_text);
}


const transcriptObjectIdentity = new WeakMap<object, string>();
let nextTranscriptObjectIdentity = 0;

function transcriptSemanticHash(values: string[]) {
  let hash = 2166136261;
  for (const value of values) {
    for (let index = 0; index < value.length; index += 1) {
      hash ^= value.charCodeAt(index);
      hash = Math.imul(hash, 16777619);
    }
  }
  return (hash >>> 0).toString(36);
}

function transcriptReactKey(item: TranscriptItemRecord, type = transcriptItemType(item)) {
  const existing = transcriptObjectIdentity.get(item);
  if (existing !== undefined) return existing;
  if (item.callId?.trim()) {
    const key = `${type}:call:${item.callId}`;
    transcriptObjectIdentity.set(item, key);
    return key;
  }
  const semanticIdentity = [
    item.time ?? "",
    item.tag ?? "",
    item.command ?? "",
    item.body,
    item.model ?? "",
    item.effort ?? "",
  ];
  if (semanticIdentity.some(Boolean)) {
    const key = `${type}:content:${transcriptSemanticHash(semanticIdentity)}`;
    transcriptObjectIdentity.set(item, key);
    return key;
  }
  const key = `${type}:object:${nextTranscriptObjectIdentity++}`;
  transcriptObjectIdentity.set(item, key);
  return key;
}

function transcriptGroupReactKey(tools: TranscriptItemRecord[]) {
  return `tool-group:${tools[0] ? transcriptReactKey(tools[0], transcriptItemType(tools[0])) : "empty"}`;
}


function highlightTranscriptText(value: string | undefined, query: string): ReactNode {
  const text = `${value ?? ""}`;
  const ranges = findTextRanges(text, query);
  if (ranges.length === 0) return text;

  const parts: ReactNode[] = [];
  let offset = 0;
  for (const range of ranges) {
    if (range.from > offset) parts.push(text.slice(offset, range.from));
    parts.push(<mark className="transcriptSearchMark" key={`${range.from}-${parts.length}`}>{text.slice(range.from, range.to)}</mark>);
    offset = range.to;
  }
  if (offset < text.length) parts.push(text.slice(offset));
  return parts;
}

function cssEscape(value: string) {
  return window.CSS?.escape ? window.CSS.escape(value) : `${value}`.replace(/["\\]/g, "\\$&");
}

export function TranscriptPanel({
  session,
  childSessions,
  sessionTree,
  items,
  locatorMetadata,
  sessionSearchQuery,
  loading,
  hasMore,
  loadingMore,
  scrollRestorationReady,
  skillLinks,
  loadingSkillLinks,
  skillLinksLoaded,
  skillLinksError,
  onCollapse,
  onOpenSession,
  onOpenSkill,
  onLoadSkills,
  onLoadMore,
  loadUntilTarget,
  onLoadAll,
  onReportError,
  searchTranscript,
  onSavePrompt,
  keyboardNavigationScopeRef,
}: {
  session: SessionRecord;
  childSessions: SessionRecord[];
  sessionTree: SessionRecord[];
  items: TranscriptItemRecord[];
  locatorMetadata?: TranscriptLocatorItem[];
  sessionSearchQuery?: string;
  loading: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  scrollRestorationReady: boolean;
  skillLinks: SessionSkillLinkRecord[];
  loadingSkillLinks: boolean;
  skillLinksLoaded: boolean;
  skillLinksError?: string;
  onCollapse: () => void;
  onOpenSession: (session: SessionRecord) => void;
  onOpenSkill?: (skillPath: string) => void;
  onLoadSkills?: () => void;
  onLoadMore: () => Promise<TranscriptLoadMoreResult>;
  loadUntilTarget: (options: SessionTranscriptTargetLoadOptions) => Promise<SessionTranscriptTargetLoadResult>;
  onLoadAll: () => Promise<void>;
  onReportError?: (message: string) => void;
  searchTranscript?: (session: SessionRecord, query: string, scopes: TranscriptSearchScopes) => Promise<TranscriptSearchResult | null>;
  onSavePrompt?: (body: string) => Promise<boolean>;
  keyboardNavigationScopeRef: KeyboardNavigationScopeRef;
}) {
  const transcriptItems = useMemo(() => {
    return groupTranscriptItems(items) as TranscriptItemRecord[];
  }, [items]);
  const locatorItems = useMemo(() => locatorMetadata
    ? buildSessionLocatorItemsFromMetadata(locatorMetadata, 0)
    : buildSessionLocatorItems(transcriptItems), [locatorMetadata, transcriptItems]);
  const initialTranscriptLoading = loading && transcriptItems.length === 0;
  const reportedSegments = useMemo(() => reportedTokenSegments(session), [session]);
  const hasReportedUsage = Boolean(session.tokenUsage);
  const cacheMetrics = useMemo(() => sessionCacheMetrics(session), [session]);
  const transcriptRef = useRef<HTMLDivElement | null>(null);
  const loadMoreRef = useRef<HTMLDivElement | null>(null);
  const {
    range: transcriptRenderRange,
    rangeKey: transcriptRenderRangeKey,
    topSpacerHeight: transcriptTopSpacerHeight,
    bottomSpacerHeight: transcriptBottomSpacerHeight,
    measureItem: measureTranscriptItem,
    scrollToIndex: scrollTranscriptToIndex,
    scrollToBottom: scrollTranscriptToBottom,
  } = useTranscriptVirtualizer(
    transcriptItems,
    transcriptRef,
    sessionExternalKey(session),
    scrollRestorationReady,
  );
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const [searchQuery, setSearchQuery] = useState("");
  const [debouncedSearchQuery, setDebouncedSearchQuery] = useState("");
  const [searchScopes, setSearchScopes] = useState<TranscriptSearchScopeState>(DEFAULT_TRANSCRIPT_SEARCH_SCOPES);
  const [searchOpen, setSearchOpen] = useState(false);
  const [transcriptScrollEdges, setTranscriptScrollEdges] = useState({ atTop: true, atBottom: true });
  const reportedSearchErrorRevisionRef = useRef(0);
  const transcriptSearchScrollSnapshotRef = useRef<{ top: number; distanceFromBottom: number } | null>(null);
  const transcriptNavigationSessionKey = `${session.agent}:${session.id}:${session.path}`;
  const normalizedSessionSearchQuery = `${sessionSearchQuery ?? ""}`.trim().toLowerCase();
  const updateTranscriptScrollEdges = useCallback(() => {
    const root = transcriptRef.current;
    if (!root) return;
    const maxScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
    const next = {
      atTop: root.scrollTop <= 2,
      atBottom: root.scrollTop >= maxScrollTop - 2,
    };
    setTranscriptScrollEdges((current) => (
      current.atTop === next.atTop && current.atBottom === next.atBottom ? current : next
    ));
  }, []);
  const {
    beginNavigation: beginTranscriptNavigation,
    isCurrentNavigation,
    currentNavigationIntent,
    focusTarget: focusTranscriptTarget,
    highlightedKey,
    clearHighlight,
    jumpingToBottom,
    scrollToTop: scrollTranscriptToTop,
    jumpToBottom: runJumpToBottom,
  } = useTranscriptNavigation({
    rootRef: transcriptRef,
    items: transcriptItems,
    renderRangeKey: transcriptRenderRangeKey,
    scrollToIndex: scrollTranscriptToIndex,
    scrollToBottom: scrollTranscriptToBottom,
    indexFromKey: transcriptIndexFromKey,
    updateScrollEdges: updateTranscriptScrollEdges,
  });
  const captureTranscriptSearchScroll = useCallback(() => {
    const root = transcriptRef.current;
    if (!root) return;
    const maxScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
    transcriptSearchScrollSnapshotRef.current = {
      top: root.scrollTop,
      distanceFromBottom: maxScrollTop - root.scrollTop,
    };
    root.scrollTo({ top: root.scrollTop, behavior: "auto" });
  }, []);
  const transcriptSearchContextRef = useRef({
    query: normalizedSessionSearchQuery,
    sessionKey: transcriptNavigationSessionKey,
  });
  useLayoutEffect(() => {
    const previous = transcriptSearchContextRef.current;
    if (previous.sessionKey !== transcriptNavigationSessionKey) {
      transcriptSearchScrollSnapshotRef.current = null;
    } else if (previous.query && !normalizedSessionSearchQuery && searchOpen) {
      captureTranscriptSearchScroll();
    }
    transcriptSearchContextRef.current = {
      query: normalizedSessionSearchQuery,
      sessionKey: transcriptNavigationSessionKey,
    };
  }, [captureTranscriptSearchScroll, normalizedSessionSearchQuery, searchOpen, transcriptNavigationSessionKey]);
  useEffect(() => {
    beginTranscriptNavigation(normalizedSessionSearchQuery ? "search" : "idle");
    clearHighlight();
    setSearchQuery(normalizedSessionSearchQuery);
    setDebouncedSearchQuery(normalizedSessionSearchQuery);
    setSearchOpen(Boolean(normalizedSessionSearchQuery));
  }, [beginTranscriptNavigation, clearHighlight, normalizedSessionSearchQuery, transcriptNavigationSessionKey]);
  const normalizedInputSearchQuery = searchQuery.trim().toLowerCase();
  const normalizedSearchQuery = debouncedSearchQuery;
  const selectedSearchScopeCount = TRANSCRIPT_SEARCH_SCOPES.filter((scope) => searchScopes[scope.id]).length;
  const {
    loading: searchLoading,
    ready: searchReady,
    result: searchResult,
    error: searchError,
    errorRevision: searchErrorRevision,
    remoteSearch: remoteSearchActive,
  } = useSessionTranscriptSearch({
    session,
    sessionKey: transcriptNavigationSessionKey,
    query: normalizedSearchQuery,
    scopes: searchScopes,
    hasMore,
    transcriptLoading: loading,
    searchTranscript,
    loadAll: onLoadAll,
  });
  useEffect(() => {
    if (searchErrorRevision === reportedSearchErrorRevisionRef.current) return;
    reportedSearchErrorRevisionRef.current = searchErrorRevision;
    onReportError?.("Could not search messages in this session. Try again.");
  }, [onReportError, searchErrorRevision]);
  const searchTargets = useMemo(
    () => searchReady && !remoteSearchActive
      ? findTranscriptSearchTargets(transcriptItems, normalizedSearchQuery, searchScopes)
      : [],
    [normalizedSearchQuery, remoteSearchActive, searchReady, searchScopes, transcriptItems],
  );
  const markDetailKeyboardScope = useCallback(() => {
    keyboardNavigationScopeRef.current = KeyboardNavigationScope.Detail;
  }, [keyboardNavigationScopeRef]);
  const isDetailKeyboardScope = useCallback(() => (
    keyboardNavigationScopeRef.current === KeyboardNavigationScope.Detail
  ), [keyboardNavigationScopeRef]);
  const visibleUserMessageIndex = useCallback(() => {
    const root = transcriptRef.current;
    if (!root) return -1;
    const rootBounds = root.getBoundingClientRect();
    let firstAfterViewport = -1;
    let lastBeforeViewport = -1;
    let firstVisible = -1;
    let firstVisibleTop = Number.POSITIVE_INFINITY;
    locatorItems.forEach((item, index) => {
      const node = root.querySelector<HTMLElement>(`[data-transcript-key="${cssEscape(item.key)}"]`);
      if (!node) return;
      const bounds = node.getBoundingClientRect();
      if (bounds.bottom > rootBounds.top + 1 && firstAfterViewport < 0) firstAfterViewport = index;
      if (bounds.top < rootBounds.top) lastBeforeViewport = index;
      if (
        bounds.bottom > rootBounds.top + 1
        && bounds.top < rootBounds.bottom - 1
        && bounds.top < firstVisibleTop
      ) {
        firstVisible = index;
        firstVisibleTop = bounds.top;
      }
    });
    return firstVisible >= 0 ? firstVisible : firstAfterViewport >= 0 ? firstAfterViewport : lastBeforeViewport;
  }, [locatorItems]);
  const {
    searchIndex,
    searchResultCount,
    jumpingSkillPath,
    jumpToSkillEvidence,
    selectLocatorItem,
    moveUserMessage,
    moveSearchResult,
  } = useSessionTranscriptNavigation({
    sessionKey: transcriptNavigationSessionKey,
    navigationSearchQuery: normalizedSessionSearchQuery,
    transcript: {
      items: transcriptItems,
      locatorItems,
      loadUntilTarget,
    },
    search: {
      query: normalizedSearchQuery,
      scopes: searchScopes,
      targets: searchTargets,
      ready: searchReady,
      loading: searchLoading,
      error: Boolean(searchError),
      result: searchResult,
      remote: remoteSearchActive,
    },
    navigation: {
      beginNavigation: beginTranscriptNavigation,
      isCurrentNavigation,
      currentNavigationIntent,
      focusTarget: focusTranscriptTarget,
      indexFromKey: transcriptIndexFromKey,
    },
    keyboard: {
      markDetailKeyboardScope,
      isDetailKeyboardScope,
      visibleUserMessageIndex,
    },
    onReportError,
  });
  const clearMessageSearch = useCallback(() => {
    beginTranscriptNavigation("idle");
    captureTranscriptSearchScroll();
    clearHighlight();
    setSearchQuery("");
    setDebouncedSearchQuery("");
    setSearchScopes(DEFAULT_TRANSCRIPT_SEARCH_SCOPES);
    setSearchOpen(false);
  }, [beginTranscriptNavigation, captureTranscriptSearchScroll, clearHighlight]);
  const setSearchScope = useCallback((scope: keyof TranscriptSearchScopeState, checked: boolean) => {
    beginTranscriptNavigation("search");
    setSearchScopes((current) => {
      if (!checked && TRANSCRIPT_SEARCH_SCOPES.every((item) => item.id === scope || !current[item.id])) {
        return current;
      }
      return { ...current, [scope]: checked };
    });
  }, [beginTranscriptNavigation]);
  useEffect(() => {
    if (!normalizedInputSearchQuery) {
      beginTranscriptNavigation("idle");
      setDebouncedSearchQuery("");
      return;
    }
    beginTranscriptNavigation("search");
    const timer = window.setTimeout(() => {
      setDebouncedSearchQuery(normalizedInputSearchQuery);
    }, SESSION_SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [beginTranscriptNavigation, normalizedInputSearchQuery]);
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "f") return;
      event.preventDefault();
      event.stopPropagation();
      setSearchOpen(true);
      window.requestAnimationFrame(() => {
        searchInputRef.current?.focus();
        searchInputRef.current?.select();
      });
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, []);
  useEffect(() => {
    if (!hasMore || loadingMore || searchLoading || jumpingToBottom) return;
    const root = transcriptRef.current;
    const sentinel = loadMoreRef.current;
    if (!root || !sentinel) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) void onLoadMore();
    }, { root, rootMargin: "0px 0px 320px 0px" });
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore, jumpingToBottom, loadingMore, onLoadMore, searchLoading]);
  useEffect(() => {
    const root = transcriptRef.current;
    if (!root) return;
    updateTranscriptScrollEdges();
    root.addEventListener("scroll", updateTranscriptScrollEdges, { passive: true });
    return () => root.removeEventListener("scroll", updateTranscriptScrollEdges);
  }, [updateTranscriptScrollEdges]);
  useEffect(() => {
    const frame = window.requestAnimationFrame(updateTranscriptScrollEdges);
    return () => window.cancelAnimationFrame(frame);
  }, [hasMore, loading, loadingMore, transcriptItems, transcriptRenderRangeKey, updateTranscriptScrollEdges]);
  useLayoutEffect(() => {
    const snapshot = transcriptSearchScrollSnapshotRef.current;
    if (!snapshot || searchOpen) return;
    const root = transcriptRef.current;
    if (!root) return;
    const maxScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
    const nextScrollTop = snapshot.distanceFromBottom <= 2
      ? maxScrollTop
      : Math.min(snapshot.top, maxScrollTop);
    root.scrollTo({ top: nextScrollTop, behavior: "auto" });
    transcriptSearchScrollSnapshotRef.current = null;
    updateTranscriptScrollEdges();
  }, [searchOpen, updateTranscriptScrollEdges]);
  const jumpToBottom = useCallback(
    () => runJumpToBottom(hasMore, onLoadAll),
    [hasMore, onLoadAll, runJumpToBottom],
  );
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
      keyboardNavigationScopeRef.current !== KeyboardNavigationScope.Detail
        || (event.key !== "ArrowUp" && event.key !== "ArrowDown")
        || event.defaultPrevented
        || event.metaKey
        || event.ctrlKey
        || event.altKey
        || event.shiftKey
        || isKeyboardNavigationIgnoredTarget(event.target)
      ) return;
      event.preventDefault();
      moveUserMessage(event.key === "ArrowUp" ? -1 : 1);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [keyboardNavigationScopeRef, moveUserMessage]);
  const transcriptReactKeyCounts = new Map<string, number>();
  const openLinkedSession = useCallback((sessionId: string) => {
    const linkedSession = childSessions.find((child) => child.id === sessionId);
    if (linkedSession) onOpenSession(linkedSession);
  }, [childSessions, onOpenSession]);
  return (
    <aside
      className="transcriptPanel"
      onPointerDownCapture={() => { keyboardNavigationScopeRef.current = KeyboardNavigationScope.Detail; }}
      onFocusCapture={() => { keyboardNavigationScopeRef.current = KeyboardNavigationScope.Detail; }}
    >
      <header className="threadHeader">
        <div className="threadTitleLine">
          <h2><SessionTitleText interactive={false} value={sessionTitleValue(session)} /></h2>
          <div className="threadHeaderActions">
            <SessionRelationsPopover
              session={session}
              sessionTree={sessionTree}
              onOpenSession={onOpenSession}
            />
            <SessionSkillsPopover
              session={session}
              links={skillLinks}
              loading={loadingSkillLinks}
              loaded={skillLinksLoaded}
              error={skillLinksError}
              onLoad={onLoadSkills}
              onOpenSkill={onOpenSkill}
              jumpingSkillPath={jumpingSkillPath}
              onJumpToEvidence={jumpToSkillEvidence}
            />
            <SessionInfoMenu session={session} />
            <IconButton className="threadPanelToggle" aria-label="Collapse session detail" onClick={onCollapse}><PanelRightClose size={16} /></IconButton>
          </div>
        </div>
        <div className="threadMeta">
          <span>{compactDateTime(session.updatedAt, { year: true }) || EMPTY_DISPLAY_VALUE}</span>
          <span className="threadMessageCount">{session.messages === undefined ? EMPTY_DISPLAY_VALUE : `${session.messages} messages`}</span>
        </div>
        {searchOpen ? (
          <div className="transcriptSearch" role="search">
            <Search size={14} aria-hidden="true" />
            <div className="transcriptSearchInput">
              <input
                ref={searchInputRef}
                aria-label={searchLoading
                  ? remoteSearchActive ? "Searching messages in this session" : "Loading all messages for search"
                  : "Search messages in this session"}
                aria-busy={searchLoading}
                placeholder="Search messages"
                value={searchQuery}
                onChange={(event) => setSearchQuery(event.target.value)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") {
                    event.preventDefault();
                    if (normalizedInputSearchQuery !== normalizedSearchQuery) {
                      setDebouncedSearchQuery(normalizedInputSearchQuery);
                      return;
                    }
                    moveSearchResult(event.shiftKey ? -1 : 1);
                  }
                  if (event.key === "Escape") {
                    clearMessageSearch();
                  }
                }}
              />
              <SearchClearButton value={searchQuery} onClear={() => setSearchQuery("")} ariaLabel="Clear message search" />
            </div>
            <div className="transcriptSearchActions">
              {normalizedSearchQuery ? <span className="transcriptSearchCount">{searchLoading ? "…" : searchResultCount ? `${searchIndex + 1}/${searchResultCount}` : "0/0"}</span> : null}
              {normalizedSearchQuery ? (
                <>
                  <button type="button" aria-label="Previous matching message" onClick={() => moveSearchResult(-1)} disabled={searchLoading || searchResultCount === 0}><ChevronLeft size={14} /></button>
                  <button type="button" aria-label="Next matching message" onClick={() => moveSearchResult(1)} disabled={searchLoading || searchResultCount === 0}><ChevronRight size={14} /></button>
                </>
              ) : null}
              <DropdownMenu.Root>
                <DropdownMenu.Trigger asChild>
                  <IconButton
                    className={`transcriptSearchFilter${selectedSearchScopeCount > 0 ? " filled" : ""}`}
                    aria-label={selectedSearchScopeCount > 0 ? `Filter messages, ${selectedSearchScopeCount} selected` : "Filter messages"}
                    aria-pressed={selectedSearchScopeCount > 0}
                  >
                    <Filter size={15} aria-hidden="true" />
                    {selectedSearchScopeCount > 0 ? <span className="transcriptSearchFilterCount" aria-hidden="true">{selectedSearchScopeCount}</span> : null}
                  </IconButton>
                </DropdownMenu.Trigger>
                <DropdownMenu.Portal>
                  <MenuContent
                    className="transcriptSearchFilterMenu"
                    align="end"
                    sideOffset={6}
                    data-no-drag
                    onCloseAutoFocus={(event) => {
                      event.preventDefault();
                      searchInputRef.current?.focus();
                    }}
                  >
                    {TRANSCRIPT_SEARCH_SCOPES.map((scope) => {
                      const active = searchScopes[scope.id];
                      return (
                        <DropdownMenu.CheckboxItem
                          key={scope.id}
                          className="menuItem transcriptSearchFilterItem"
                          checked={active}
                          onCheckedChange={(checked) => setSearchScope(scope.id, checked === true)}
                          onSelect={(event) => event.preventDefault()}
                        >
                          <CheckboxIndicator checked={active} />
                          <span>{scope.label}</span>
                        </DropdownMenu.CheckboxItem>
                      );
                    })}
                  </MenuContent>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
              <button type="button" aria-label="Close message search" onClick={clearMessageSearch}><X size={13} /></button>
            </div>
          </div>
        ) : null}
      </header>
      <div className="transcriptViewport">
        <div className="transcript" ref={transcriptRef}>
          {initialTranscriptLoading ? <LoadingState label="Loading transcript" /> : transcriptItems.length > 0 ? (
            <>
              {transcriptTopSpacerHeight > 0 ? <div className="transcriptVirtualSpacer" style={{ height: transcriptTopSpacerHeight }} aria-hidden="true" /> : null}
              {transcriptItems.slice(transcriptRenderRange.start, transcriptRenderRange.end).map((item, relativeIndex) => {
                const index = transcriptRenderRange.start + relativeIndex;
                const type = transcriptItemType(item);
                const previousType = index > 0 ? transcriptItemType(transcriptItems[index - 1]) : undefined;
                const itemKey = transcriptItemKey(type === TranscriptGroupType.ToolGroup ? "tool-group" : type, index);
                const reactKeyBase = type === TranscriptGroupType.ToolGroup
                  ? transcriptGroupReactKey(item.tools ?? [])
                  : transcriptReactKey(item, type);
                const reactKeyOccurrence = transcriptReactKeyCounts.get(reactKeyBase) ?? 0;
                transcriptReactKeyCounts.set(reactKeyBase, reactKeyOccurrence + 1);
                const highlighted = transcriptItemIsHighlighted(item, itemKey, highlightedKey);
                return (
                  <div
                    className={`transcriptItemShell${highlighted ? " isHighlighted" : ""}`}
                    data-transcript-index={index}
                    ref={(node) => measureTranscriptItem(index, node)}
                    key={reactKeyOccurrence === 0 ? reactKeyBase : `${reactKeyBase}:${reactKeyOccurrence}`}
                  >
                    <TranscriptItem
                      item={item}
                      itemKey={itemKey}
                      addTopSpacing={isChatTranscriptType(type) && !isChatTranscriptType(previousType)}
                      highlightedKey={highlightedKey}
                      searchQuery={transcriptItemSearchQuery(item, normalizedSearchQuery, searchScopes)}
                      searchScopes={searchScopes}
                      onOpenLinkedSession={openLinkedSession}
                      onSavePrompt={onSavePrompt}
                    />
                  </div>
                );
              })}
              {transcriptBottomSpacerHeight > 0 ? <div className="transcriptVirtualSpacer" style={{ height: transcriptBottomSpacerHeight }} aria-hidden="true" /> : null}
              {hasMore ? (
                <>
                  {loadingMore ? <div className="sessionTranscriptLoadMore" role="status" aria-label="Loading more messages"><LoadingDots size={15} /></div> : null}
                  <div ref={loadMoreRef} className="sessionTranscriptLoadMoreSentinel" aria-hidden="true" />
                </>
              ) : null}
            </>
          ) : <EmptyState compact title="No transcript items" />}
        </div>
        {(!transcriptScrollEdges.atTop || !transcriptScrollEdges.atBottom) ? (
          <div className="transcriptScrollControls" aria-label="Transcript navigation">
            {!transcriptScrollEdges.atTop ? (
              <IconButton
                className="threadPanelToggle transcriptScrollTop"
                aria-label="Jump to top of transcript"
                disabled={loading}
                onClick={scrollTranscriptToTop}
              >
                <ArrowUpToLine size={15} />
              </IconButton>
            ) : null}
            {!transcriptScrollEdges.atBottom ? (
              <IconButton
                className="threadPanelToggle transcriptScrollBottom"
                aria-label={jumpingToBottom ? "Loading and jumping to bottom of transcript" : "Jump to bottom of transcript"}
                aria-busy={jumpingToBottom}
                disabled={loading || jumpingToBottom}
                onClick={() => { void jumpToBottom(); }}
              >
                {jumpingToBottom ? <LoadingIcon size={15} /> : <ArrowDownToLine size={15} />}
              </IconButton>
            ) : null}
          </div>
        ) : null}
      </div>
      {initialTranscriptLoading ? null : (
        <SessionLocator
          items={locatorItems}
          loading={loading}
          scrollRootRef={transcriptRef}
          transcriptRenderRangeKey={transcriptRenderRangeKey}
          onSelect={selectLocatorItem}
        />
      )}
      <Suspense fallback={null}>
        {hasReportedUsage || !hasMore ? (
          <SessionTokenStatusBar
            items={items}
            skillLinks={skillLinks}
            reportedSegments={reportedSegments}
            metrics={cacheMetrics}
            usageSource={hasReportedUsage ? TokenUsageSource.Reported : TokenUsageSource.Estimated}
          />
        ) : null}
      </Suspense>
    </aside>
  );
}

const SessionLocator = memo(function SessionLocator({
  items,
  loading,
  scrollRootRef,
  transcriptRenderRangeKey,
  onSelect,
}: {
  items: SessionLocatorItem[];
  loading: boolean;
  scrollRootRef: { current: HTMLDivElement | null };
  transcriptRenderRangeKey: string;
  onSelect: (key: string, behavior?: ScrollBehavior) => void;
}) {
  const [visibleKeys, setVisibleKeys] = useState<Set<string>>(() => new Set());
  const [previewKey, setPreviewKey] = useState("");
  const locatorListRef = useRef<HTMLDivElement | null>(null);
  const {
    scrollOffset: locatorScrollTop,
    readViewportSize,
    scheduleScrollSync,
  } = useVirtualViewport<HTMLDivElement>(
    { width: 0, height: 640 },
    {
      ref: locatorListRef,
      refreshKey: items,
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ height }) => height > 0,
      isEqual: (current, next) => current.height === next.height,
    },
  );
  const dragRef = useRef<{ pointerId: number; key: string; moved: boolean } | null>(null);
  const suppressClickRef = useRef(false);
  const itemKeySet = useMemo(() => new Set(items.map((item) => item.key)), [items]);
  const clearPreview = useCallback(() => setPreviewKey(""), []);
  const handleLocatorClick = useCallback((key: string) => {
    if (suppressClickRef.current) {
      suppressClickRef.current = false;
      return;
    }
    onSelect(key);
  }, [onSelect]);
  const locatorViewportHeight = readViewportSize();
  const { start: locatorStart, end: locatorEnd } = fixedVirtualRange(
    items.length,
    locatorScrollTop,
    locatorViewportHeight,
    SESSION_LOCATOR_ROW_HEIGHT,
    SESSION_LOCATOR_OVERSCAN,
  );

  useEffect(() => {
    if ((loading && items.length === 0) || items.length < SESSION_LOCATOR_MIN_ITEMS) {
      setVisibleKeys((current) => current.size === 0 ? current : new Set());
      return;
    }
    const root = scrollRootRef.current;
    if (!root) return;
    const visible = new Set<string>();
    let publishFrame = 0;
    const publishVisibleKeys = () => {
      publishFrame = 0;
      setVisibleKeys((current) => {
        if (current.size === visible.size && [...current].every((key) => visible.has(key))) return current;
        return new Set(visible);
      });
    };
    const observer = new IntersectionObserver((entries) => {
      let changed = false;
      for (const entry of entries) {
        const key = (entry.target as HTMLElement).dataset.transcriptKey;
        if (!key) continue;
        if (entry.isIntersecting) {
          if (!visible.has(key)) {
            visible.add(key);
            changed = true;
          }
        } else if (visible.delete(key)) {
          changed = true;
        }
      }
      if (changed && publishFrame === 0) {
        publishFrame = window.requestAnimationFrame(publishVisibleKeys);
      }
    }, { root, threshold: 0 });
    for (const node of root.querySelectorAll<HTMLElement>("[data-transcript-key]")) {
      if (node.dataset.transcriptKey && itemKeySet.has(node.dataset.transcriptKey)) {
        observer.observe(node);
      }
    }
    return () => {
      observer.disconnect();
      if (publishFrame !== 0) window.cancelAnimationFrame(publishFrame);
    };
  }, [itemKeySet, items.length, loading, scrollRootRef, transcriptRenderRangeKey]);

  if (items.length < SESSION_LOCATOR_MIN_ITEMS) return null;

  const locatorItemAtPoint = (x: number, y: number) => {
    const row = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-session-locator-item-id]");
    const key = row?.dataset.sessionLocatorItemId;
    return key && itemKeySet.has(key) ? key : "";
  };

  return (
    <nav className="sessionLocator" aria-label="User messages">
      <div
          ref={locatorListRef}
          className="sessionLocatorList"
          onScroll={() => {
            scheduleScrollSync();
          }}
          onPointerDown={(event) => {
            if (event.button !== 0) return;
            const key = locatorItemAtPoint(event.clientX, event.clientY);
            if (!key) return;
            dragRef.current = { pointerId: event.pointerId, key, moved: false };
            event.currentTarget.setPointerCapture?.(event.pointerId);
          }}
          onPointerMove={(event) => {
            const drag = dragRef.current;
            if (!drag || drag.pointerId !== event.pointerId || event.buttons % 2 === 0) return;
            const key = locatorItemAtPoint(event.clientX, event.clientY);
            if (!key || key === drag.key) return;
            dragRef.current = { ...drag, key, moved: true };
            setPreviewKey(key);
            onSelect(key, SessionResumeTarget.Auto);
          }}
          onPointerUp={(event) => {
            const drag = dragRef.current;
            if (!drag || drag.pointerId !== event.pointerId) return;
            dragRef.current = null;
            if (!drag.moved) onSelect(drag.key);
            suppressClickRef.current = true;
            event.currentTarget.releasePointerCapture?.(event.pointerId);
            window.setTimeout(() => {
              suppressClickRef.current = false;
            }, 0);
          }}
          onPointerCancel={() => {
            dragRef.current = null;
            suppressClickRef.current = false;
          }}
        >
          {locatorStart > 0 ? <div className="sessionLocatorVirtualSpacer" style={{ height: locatorStart * SESSION_LOCATOR_ROW_HEIGHT }} aria-hidden="true" /> : null}
          {items.slice(locatorStart, locatorEnd).map((item, relativeIndex) => {
            const index = locatorStart + relativeIndex;
            return (
              <SessionLocatorRow
                key={item.key}
                item={item}
                index={index}
                previewOpen={previewKey === item.key}
                current={visibleKeys.has(item.key)}
                onClick={handleLocatorClick}
                onPreview={setPreviewKey}
                onClearPreview={clearPreview}
              />
            );
          })}
          {locatorEnd < items.length ? <div className="sessionLocatorVirtualSpacer" style={{ height: (items.length - locatorEnd) * SESSION_LOCATOR_ROW_HEIGHT }} aria-hidden="true" /> : null}
      </div>
    </nav>
  );
});

const SessionLocatorRow = memo(function SessionLocatorRow({
  item,
  index,
  previewOpen,
  current,
  onClick,
  onPreview,
  onClearPreview,
}: {
  item: SessionLocatorItem;
  index: number;
  previewOpen: boolean;
  current: boolean;
  onClick: (key: string) => void;
  onPreview: (key: string) => void;
  onClearPreview: () => void;
}) {
  return (
    <AppTooltip
      content={(
        <>
          <strong><TranscriptLinkText interactive={false} value={formatTranscriptPreview(item.label) || EMPTY_DISPLAY_VALUE} /></strong>
          {item.response ? <span><TranscriptLinkText interactive={false} value={formatTranscriptPreview(item.response) || EMPTY_DISPLAY_VALUE} /></span> : null}
        </>
      )}
      open={previewOpen}
      side="right"
      align="center"
      sideOffset={-6}
      collisionPadding={8}
      className="sessionLocatorPreview"
      unstyled
    >
      <button
        type="button"
        className="sessionLocatorRow"
        data-session-locator-item-id={item.key}
        aria-current={current ? "true" : undefined}
        aria-label={`Jump to user message ${index + 1}`}
        onClick={() => onClick(item.key)}
        onFocus={() => onPreview(item.key)}
        onBlur={onClearPreview}
        onMouseEnter={() => onPreview(item.key)}
        onMouseLeave={onClearPreview}
      >
        <span className="sessionLocatorMarker" />
      </button>
    </AppTooltip>
  );
});


export function SessionRelationsPopover({
  session,
  sessionTree,
  onOpenSession,
}: {
  session: SessionRecord;
  sessionTree: SessionRecord[];
  onOpenSession: (session: SessionRecord) => void;
}) {
  const relationCount = Math.max(0, sessionTree.length - 1);
  if (relationCount === 0) return null;

  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <IconButton
          className="threadPanelToggle"
          aria-label={`Show ${relationCount} related session${relationCount === 1 ? "" : "s"}`}
        >
          <GitFork size={15} />
        </IconButton>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className="sessionRelationsPopover hasChart"
          align="end"
          sideOffset={8}
          data-no-drag
          onMouseDown={(event) => event.stopPropagation()}
        >
          <div className="sessionRelationsHeader">Related sessions</div>
          <SessionRelationsConvergence
            session={session}
            sessionTree={sessionTree}
            onOpenSession={onOpenSession}
          />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}


export function SessionSkillsPopover({
  session,
  links = [],
  loading = false,
  loaded = false,
  error = "",
  onLoad,
  onOpenSkill,
  jumpingSkillPath = "",
  onJumpToEvidence,
}: {
  session: SessionRecord;
  links?: SessionSkillLinkRecord[];
  loading?: boolean;
  loaded?: boolean;
  error?: string;
  onLoad?: () => void;
  onOpenSkill?: (skillPath: string) => void;
  jumpingSkillPath?: string;
  onJumpToEvidence?: (link: SessionSkillLinkRecord) => void | Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const pendingOpenRef = useRef(false);
  const sessionIdentity = sessionKey(session);

  useEffect(() => {
    pendingOpenRef.current = false;
    setOpen(false);
  }, [sessionIdentity]);

  useEffect(() => {
    if (!pendingOpenRef.current || loading || !loaded) return;
    if (links.length === 0) {
      pendingOpenRef.current = false;
      return;
    }
    pendingOpenRef.current = false;
    setOpen(true);
  }, [links.length, loaded, loading]);

  const handleOpenChange = (open: boolean) => {
    if (open && !loaded) {
      pendingOpenRef.current = true;
      onLoad?.();
      return;
    }
    if (!open) pendingOpenRef.current = false;
    setOpen(open);
    if (open) onLoad?.();
  };
  const handleJumpToEvidence = async (link: SessionSkillLinkRecord) => {
    await onJumpToEvidence?.(link);
    setOpen(false);
  };
  const loadingWithoutLinks = loading && links.length === 0;
  const disabledReason = loadingWithoutLinks
    ? "Loading skills used"
    : loaded && !loading && links.length === 0
      ? "No associated skills for this session"
      : !onLoad
        ? "Skill links unavailable"
        : "";

  if (disabledReason) {
    return (
      <AppTooltip content={disabledReason}><span className="sessionSkillsTriggerWrap">
        <IconButton className="threadPanelToggle" aria-label={disabledReason} aria-busy={loadingWithoutLinks || undefined} disabled>
          {loadingWithoutLinks ? <LoadingIcon size={15} /> : <Sparkles size={15} />}
        </IconButton>
      </span></AppTooltip>
    );
  }

  return (
    <Popover.Root open={open} onOpenChange={handleOpenChange}>
      <Popover.Trigger asChild>
        <IconButton className="threadPanelToggle" aria-label="Show skills used">
          <Sparkles size={15} />
        </IconButton>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className={`sessionSkillsPopover${loading || links.length > 0 ? " hasChart" : ""}`}
          align="end"
          sideOffset={8}
          data-no-drag
          onMouseDown={(event) => event.stopPropagation()}
        >
          <SessionSkillsUsed
            session={session}
            links={links}
            loading={loading}
            error={error}
            onRetry={onLoad}
            onOpenSkill={onOpenSkill}
            jumpingSkillPath={jumpingSkillPath}
            onJumpToEvidence={handleJumpToEvidence}
          />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}


function SessionRelationsConvergence({
  session,
  sessionTree,
  onOpenSession,
}: {
  session: SessionRecord;
  sessionTree: SessionRecord[];
  onOpenSession: (session: SessionRecord) => void;
}) {
  const graph = selectSessionRelationsGraph(session, sessionTree);

  return (
    <SkillRelationshipMap
      nodes={graph.nodes}
      edges={graph.edges}
      focusName={graph.focusName}
      compact
      onOpenSkill={(name) => {
        const relatedSession = graph.relatedSessions.get(name);
        if (relatedSession) onOpenSession(relatedSession);
      }}
    />
  );
}


function SessionSkillsConvergence({
  session,
  links,
  onOpenSkill,
}: {
  session: SessionRecord;
  links: SessionSkillLinkRecord[];
  onOpenSkill?: (skillPath: string) => void;
}) {
  const graph = selectSessionSkillsConvergenceGraph(session, links);
  return (
    <SkillRelationshipMap
      nodes={graph.nodes}
      edges={graph.edges}
      focusName={graph.focusName}
      compact
      onOpenSkill={(name) => {
        if (name.startsWith("skill:")) onOpenSkill?.(name.slice("skill:".length));
      }}
    />
  );
}


export function SessionSkillsUsed({
  session,
  links = [],
  loading = false,
  error = "",
  onRetry,
  onOpenSkill,
  jumpingSkillPath = "",
  onJumpToEvidence,
}: {
  session: SessionRecord;
  links?: SessionSkillLinkRecord[];
  loading?: boolean;
  error?: string;
  onRetry?: () => void;
  onOpenSkill?: (skillPath: string) => void;
  jumpingSkillPath?: string;
  onJumpToEvidence?: (link: SessionSkillLinkRecord) => void | Promise<void>;
}) {
  return (
    <section className="sessionSkillsUsed">
      <div className="sessionSkillsHeader">
        <span>Skills used</span>
      </div>
      {loading && links.length === 0 ? (
        <div className="sessionSkillsEmpty"><LoadingInline label="Loading skills" /></div>
      ) : error && links.length === 0 ? (
        <LoadErrorState message={error} onRetry={onRetry} />
      ) : links.length > 0 ? (
        <div className="sessionSkillsContent">
          <SessionSkillsConvergence session={session} links={links} onOpenSkill={onOpenSkill} />
          <div className="sessionSkillChips">
            {links.map((link) => (
              <div
                className="sessionSkillChip"
                key={link.skill_path}
              >
                <AppTooltip content={linkEvidenceText(link)} onlyWhenTruncated><span>{linkSkillName(link)}</span></AppTooltip>
                <div className="sessionSkillChipActions">
                  <button
                    type="button"
                    aria-label={`Open ${linkSkillName(link)} skill`}
                    onClick={() => onOpenSkill?.(link.skill_path)}
                  >
                    <Sparkles size={12} />
                  </button>
                  <button
                    type="button"
                    aria-label={`Go to ${linkSkillName(link)} usage in transcript`}
                    aria-busy={jumpingSkillPath === link.skill_path || undefined}
                    disabled={Boolean(jumpingSkillPath)}
                    onClick={() => { void onJumpToEvidence?.(link); }}
                  >
                    {jumpingSkillPath === link.skill_path ? <LoadingIcon size={12} /> : <MessageSquareText size={12} />}
                  </button>
                </div>
              </div>
            ))}
          </div>
          {loading ? (
            <div className="sessionSkillsStatusOverlay" role="status" aria-live="polite" aria-label="Refreshing skills">
              <LoadingIcon size={15} />
            </div>
          ) : error ? (
            <div className="sessionSkillsStatusOverlay">
              <Toast tone="error" message={error} />
            </div>
          ) : null}
        </div>
      ) : (
        <div className="sessionSkillsEmpty">No observed skill reads</div>
      )}
    </section>
  );
}


export function SessionInfoMenu({ session }: { session: SessionRecord }) {
  const sessionId = session.id;
  const transcriptPath = session.path;
  const workspacePath = sessionWorkspacePath(session);
  const workspace = workspacePath || sessionWorkspace(session);
  const displayWorkspace = formatUserPath(workspace);
  const displayTranscriptPath = formatUserPath(transcriptPath);
  const hasWorkspacePath = Boolean(workspacePath);
  return (
    <InfoDropdownMenu
      trigger={(
        <IconButton className="threadPanelToggle" aria-label="Show session info">
          <Info size={15} />
        </IconButton>
      )}
      label="Session info"
      title={<SessionTitleText interactive={false} value={sessionTitleValue(session)} />}
      contentClassName="sessionInfoContent"
    >
            <InfoSection label="Session ID">
                <CopyableSessionId sessionId={sessionId} className="inInfoMenu" />
            </InfoSection>
            <InfoSection label="Agent">
                <AgentBadge agent={friendlyAgent(session.agent)} small />
                <span className="sessionInfoAgentValue">{friendlyAgent(session.agent)}</span>
            </InfoSection>
            <InfoSection label="Workspace" className="sessionInfoPath">
                <AppTooltip content={displayWorkspace} onlyWhenTruncated><code>{displayWorkspace}</code></AppTooltip>
                {hasWorkspacePath && (
                  <IconButton
                    aria-label={revealPathLabel("workspace")}
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path: workspacePath })}
                  >
                    <FolderOpen size={13} />
                  </IconButton>
                )}
                <CopyButton iconOnly value={workspace} copyLabel={copyPathLabel("workspace")} copiedLabel={copiedPathLabel("workspace")} />
            </InfoSection>
            <InfoSection label="Timeline" valueLine={false}>
              <div className="sessionTimeline">
                <div className="sessionTimelineItem">
                  <span className="sessionTimelineDot" aria-hidden="true" />
                  <div className="sessionTimelineText">
                    <strong>Started</strong>
                    <code>{compactDateTime(session.startedAt, { year: true }) || EMPTY_DISPLAY_VALUE}</code>
                  </div>
                </div>
                <div className="sessionTimelineItem">
                  <span className="sessionTimelineDot" aria-hidden="true" />
                  <div className="sessionTimelineText">
                    <strong>Updated</strong>
                    <code>{compactDateTime(session.updatedAt, { year: true }) || EMPTY_DISPLAY_VALUE}</code>
                  </div>
                </div>
              </div>
            </InfoSection>
            {session.model && (
              <InfoSection label="Model">
                  <AppTooltip content={session.model} onlyWhenTruncated><code>{session.model}</code></AppTooltip>
              </InfoSection>
            )}
            {transcriptPath && (
              <InfoSection label="Transcript" className="sessionInfoPath">
                  <AppTooltip content={displayTranscriptPath} onlyWhenTruncated><code>{displayTranscriptPath}</code></AppTooltip>
                  <IconButton
                    aria-label={revealPathLabel("transcript")}
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path: transcriptPath })}
                  >
                    <FolderOpen size={13} />
                  </IconButton>
                  <CopyButton iconOnly value={transcriptPath} copyLabel={copyPathLabel("transcript")} copiedLabel={copiedPathLabel("transcript")} />
              </InfoSection>
            )}
    </InfoDropdownMenu>
  );
}


type TranscriptItemProps = {
  item: TranscriptItemRecord;
  itemKey: string;
  addTopSpacing: boolean;
  highlightedKey: string;
  searchQuery: string;
  searchScopes: TranscriptSearchScopeState;
  onOpenLinkedSession?: (sessionId: string) => void;
  onSavePrompt?: (body: string) => Promise<boolean>;
};

function transcriptItemIsHighlighted(item: TranscriptItemRecord, itemKey: string, highlightedKey: string) {
  const type = transcriptItemType(item);
  if (type !== TranscriptGroupType.ToolGroup) return highlightedKey === itemKey;
  const groupIndex = itemKey.slice("tool-group-".length);
  return highlightedKey === itemKey
    || (item.tools ?? []).some((child, childIndex) => (
      highlightedKey === transcriptGroupChildKey(groupIndex, childIndex, child)
    ));
}

function transcriptHighlightState(props: TranscriptItemProps) {
  return transcriptItemIsHighlighted(props.item, props.itemKey, props.highlightedKey);
}

function MessageSavePromptButton({ body, onSave }: { body: string; onSave: (body: string) => Promise<boolean> }) {
  const [state, setState] = useState<AsyncStatus>(AsyncStatus.Idle);
  const resetTimerRef = useRef<number | undefined>(undefined);
  const clearResetTimer = useCallback(() => {
    if (resetTimerRef.current !== undefined) {
      window.clearTimeout(resetTimerRef.current);
      resetTimerRef.current = undefined;
    }
  }, []);
  useEffect(() => clearResetTimer, [clearResetTimer]);
  const save = useCallback(async () => {
    if (state === AsyncStatus.Loading || state === AsyncStatus.Success) return;
    clearResetTimer();
    setState(AsyncStatus.Loading);
    let saved = false;
    try {
      saved = await onSave(body);
    } catch {
      saved = false;
    }
    setState(saved ? AsyncStatus.Success : AsyncStatus.Error);
    resetTimerRef.current = window.setTimeout(() => {
      setState(AsyncStatus.Idle);
      resetTimerRef.current = undefined;
    }, saved ? 1600 : 2200);
  }, [body, clearResetTimer, onSave, state]);
  return (
    <StatefulButton
      state={state}
      size="sm"
      width="var(--control-icon-size-compact)"
      minWidth="var(--control-icon-size-compact)"
      variant="ghost"
      className="messageActionButton messageCopyButton messageSavePromptButton"
      aria-label="Save as prompt"
      disabled={state === AsyncStatus.Success}
      onClick={() => { void save(); }}
      loadingLabel={promptActionLabels.saving}
      successLabel={promptActionLabels.saved}
      errorLabel={promptActionLabels.saveFailed}
      loadingContent={<LoadingIcon size={13} />}
      successContent={<Check size={13} strokeWidth={2.6} aria-hidden="true" />}
      errorContent={<AlertCircle size={13} strokeWidth={2.2} aria-hidden="true" />}
      style={{ height: "var(--control-icon-size-compact)", padding: 0, display: "grid", placeItems: "center", gap: 0 }}
    >
      <MessageSquarePlus size={13} aria-hidden="true" />
    </StatefulButton>
  );
}

export const TranscriptItem = memo(function TranscriptItem({
  item,
  itemKey,
  addTopSpacing,
  highlightedKey,
  searchQuery,
  searchScopes,
  onOpenLinkedSession,
  onSavePrompt,
}: TranscriptItemProps) {
  const type = transcriptItemType(item);
  const highlighted = highlightedKey === itemKey;
  if (type === TranscriptGroupType.ToolGroup) return <ToolCallGroup tools={item.tools ?? []} itemKey={itemKey} highlightedKey={highlightedKey} searchQuery={searchQuery} searchScopes={searchScopes} onOpenLinkedSession={onOpenLinkedSession} />;
  if (type === "tool") {
    return <ToolCall item={item} itemKey={itemKey} highlighted={highlighted} searchQuery={searchQuery} onOpenLinkedSession={onOpenLinkedSession} />;
  }
  if (type === "thinking" || type === "reasoning") {
    return <ThinkingBlock item={item} itemKey={itemKey} highlighted={highlighted} searchQuery={searchQuery} />;
  }
  if (type === "context") {
    return <ContextBlock item={item} itemKey={itemKey} highlighted={highlighted} searchQuery={searchQuery} />;
  }
  if (type === "compaction") {
    return <CompactionMarker item={item} itemKey={itemKey} highlighted={highlighted} />;
  }
  if (type === "model_config") {
    return <ModelConfigMarker item={item} itemKey={itemKey} highlighted={highlighted} />;
  }
  const isUser = type === "user";
  const body = item.body;
  const copyable = type === "user" || type === "assistant";
  return (
    <div className={`chatLine ${isUser ? "fromUser" : "fromAgent"} ${addTopSpacing ? "withTopSpacing" : ""} ${highlighted ? "transcriptTarget" : ""}`} data-transcript-key={itemKey}>
      <div className="chatMessage">
        <div className="bubble">
          <PretextText content={body} markdown={false} query={searchQuery} />
        </div>
        <div className="bubbleFooter">
          {item.time?.trim() ? <time>{item.time}</time> : <span />}
          {copyable ? (
            <CopyButton
              className="messageActionButton messageCopyButton"
              value={body}
              iconSize={13}
              copyLabel={copyValueLabel(isUser ? "user message" : "assistant message")}
              copiedLabel={copiedValueLabel("message")}
            />
          ) : null}
          {isUser && onSavePrompt ? <MessageSavePromptButton body={body} onSave={onSavePrompt} /> : null}
        </div>
      </div>
    </div>
  );
}, (previous, next) => (
  previous.item === next.item
  && previous.itemKey === next.itemKey
  && previous.addTopSpacing === next.addTopSpacing
  && previous.searchQuery === next.searchQuery
  && previous.searchScopes === next.searchScopes
  && previous.onOpenLinkedSession === next.onOpenLinkedSession
  && previous.onSavePrompt === next.onSavePrompt
  && (
    transcriptHighlightState(previous) === transcriptHighlightState(next)
    && (!transcriptHighlightState(previous) || previous.highlightedKey === next.highlightedKey)
  )
));

function CompactionMarker({
  item,
  itemKey,
  highlighted,
}: {
  item: TranscriptItemRecord;
  itemKey: string;
  highlighted: boolean;
}) {
  return (
    <div
      aria-label="Context compacted"
      className={`compactionMarker ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      role="separator"
    >
      <span>Context compacted</span>
      {item.time ? <time>{item.time}</time> : null}
    </div>
  );
}

function ModelConfigMarker({
  item,
  itemKey,
  highlighted,
}: {
  item: TranscriptItemRecord;
  itemKey: string;
  highlighted: boolean;
}) {
  return (
    <div
      aria-label="Model configuration changed"
      className={`modelConfigMarker ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      role="note"
    >
      {item.model ? <span className="modelConfigField"><span>Model</span><span className="modelConfigValue">{item.model}</span></span> : null}
      {item.effort ? <span className="modelConfigField"><span>Effort</span><span className="modelConfigValue">{item.effort}</span></span> : null}
      {item.mode ? <span className="modelConfigField"><span>Mode</span><span className="modelConfigValue">{item.mode}</span></span> : null}
      {item.time ? <time>{item.time}</time> : null}
    </div>
  );
}


export function ContextBlock({
  item,
  itemKey,
  highlighted = false,
  searchQuery,
}: {
  item: TranscriptItemRecord;
  itemKey: string;
  highlighted?: boolean;
  searchQuery: string;
}) {
  const [open, setOpen] = useState(false);
  const body = item.body;
  const label = `${item.tag ?? ""}`;
  const contextKind = label === "Developer" || label === "System" ? label.toLowerCase() : "generic";
  const preview = transcriptContextPreview(body, label) || EMPTY_DISPLAY_VALUE;
  return (
    <Disclosure
      className={`thinkingBlock contextBlock ${contextKind} ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      open={open}
      onOpenChange={setOpen}
      summaryClassName="thinkingSummary"
      detailsClassName="thinkingDetails"
      summarySize="comfortable"
      detailsId={`context-details-${itemKey.replace(/[^a-zA-Z0-9_-]/g, "-")}`}
      summary={(
        <>
          <Badge tone="neutral">{label}</Badge>
          <span className="thinkingPreview">{highlightTranscriptText(preview, searchQuery)}</span>
          {item.time ? <time>{item.time}</time> : null}
        </>
      )}
    >
      <pre>{highlightTranscriptText(body || EMPTY_DISPLAY_VALUE, searchQuery)}</pre>
    </Disclosure>
  );
}


export function ThinkingBlock({
  item,
  itemKey,
  highlighted = false,
  searchQuery,
}: {
  item: TranscriptItemRecord;
  itemKey: string;
  highlighted?: boolean;
  searchQuery: string;
}) {
  const [open, setOpen] = useState(false);
  const type = transcriptItemType(item) === "thinking" ? "Thinking" : "Reasoning";
  const body = item.body;
  const preview = body.split(/\r?\n/).find((line) => line.trim())?.trim() || EMPTY_DISPLAY_VALUE;
  return (
    <Disclosure
      className={`thinkingBlock ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      open={open}
      onOpenChange={setOpen}
      summaryClassName="thinkingSummary"
      detailsClassName="thinkingDetails"
      summarySize="comfortable"
      detailsId={`thinking-details-${itemKey.replace(/[^a-zA-Z0-9_-]/g, "-")}`}
      summary={(
        <>
          <Badge tone="neutral">{type}</Badge>
          <span className="thinkingPreview">{highlightTranscriptText(preview, searchQuery)}</span>
          {item.time ? <time>{item.time}</time> : null}
        </>
      )}
    >
      <pre>{highlightTranscriptText(body || EMPTY_DISPLAY_VALUE, searchQuery)}</pre>
    </Disclosure>
  );
}


export function ToolCallGroup({
  tools,
  itemKey,
  highlightedKey,
  searchQuery,
  searchScopes,
  onOpenLinkedSession,
}: {
  tools: TranscriptItemRecord[];
  itemKey: string;
  highlightedKey: string;
  searchQuery: string;
  searchScopes: TranscriptSearchScopeState;
  onOpenLinkedSession?: (sessionId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const commandCount = tools.filter(isTranscriptExecItem).length || tools.length;
  const totalDuration = tools.reduce((total, item) => {
    const value = Number(item.durationMs);
    return Number.isFinite(value) ? total + value : total;
  }, 0);
  const duration = totalDuration > 0 ? formatDuration(totalDuration) : "";
  const childReactKeyCounts = new Map<string, number>();
  const groupIndex = itemKey.slice("tool-group-".length);
  return (
    <Disclosure
      className="toolCallGroup"
      data-transcript-key={itemKey}
      open={open}
      onOpenChange={setOpen}
      summaryClassName="toolCallGroupSummary"
      detailsClassName="toolCallGroupDetails"
      detailsId={`tool-group-details-${itemKey.replace(/[^a-zA-Z0-9_-]/g, "-")}`}
      contentPadding="inset"
      summary={(
        <>
          <span>Ran {commandCount} {commandCount === 1 ? "command" : "commands"}</span>
          {duration ? <Badge tone="neutral" className="toolCallDuration">{duration}</Badge> : null}
        </>
      )}
    >
      <>
        {tools.map((tool, index) => {
          const toolKey = transcriptGroupChildKey(groupIndex, index, tool);
          const type = transcriptItemType(tool);
          const reactKeyBase = transcriptReactKey(tool, type);
          const reactKeyOccurrence = childReactKeyCounts.get(reactKeyBase) ?? 0;
          childReactKeyCounts.set(reactKeyBase, reactKeyOccurrence + 1);
          const childSearchQuery = transcriptItemSearchQuery(tool, searchQuery, searchScopes);
          if (type === "thinking" || type === "reasoning") {
            return (
              <ThinkingBlock
                item={tool}
                itemKey={toolKey}
                highlighted={highlightedKey === toolKey}
                searchQuery={childSearchQuery}
                key={reactKeyOccurrence === 0 ? reactKeyBase : `${reactKeyBase}:${reactKeyOccurrence}`}
              />
            );
          }
          return (
            <ToolCall
              item={tool}
              itemKey={toolKey}
              highlighted={highlightedKey === toolKey}
              searchQuery={childSearchQuery}
              onOpenLinkedSession={onOpenLinkedSession}
              key={reactKeyOccurrence === 0 ? reactKeyBase : `${reactKeyBase}:${reactKeyOccurrence}`}
              nested
            />
          );
        })}
      </>
    </Disclosure>
  );
}



function sessionKey(session: SessionRecord | null | undefined) {
  if (!session) return "";
  return sessionSourceExternalKey({
    agent: session.agent,
    id: session.id,
    path: session.path,
  });
}

export function SessionsView({
  sessions: sessionItems,
  developerMode,
  loadTranscript,
  loadTranscriptLocator,
  searchTranscript,
  listSessions,
  searchSessions,
  sessionAgentFilter,
  loadSessionSkillLinks,
  skillIndexStatus,
  loadingSessions = false,
  sessionListError = "",
  sessionRefreshError = "",
  onRefreshSessions,
  onResumeSession,
  resolveSessionResumeTarget,
  sessionResumeTarget,
  missingSessionProjectPolicy,
  projects = [],
  sessionProjects = [],
  onOpenSkill,
  activeSessionKey,
  onSavePrompt,
}: {
  sessions: SessionRecord[];
  developerMode: boolean;
  loadTranscript: (session: SessionRecord, cursor?: string, knownSourceVersion?: string) => Promise<TranscriptPage>;
  loadTranscriptLocator?: (session: SessionRecord) => Promise<TranscriptLocatorPage>;
  searchTranscript?: (session: SessionRecord, query: string, scopes: TranscriptSearchScopes) => Promise<TranscriptSearchResult | null>;
  listSessions: (request: SessionListPageRequest) => Promise<SessionListPageResult>;
  searchSessions: (query: string) => Promise<SessionRecord[]>;
  sessionAgentFilter?: string;
  loadSessionSkillLinks?: (session: SessionRecord) => Promise<SessionSkillLinkRecord[]>;
  skillIndexStatus?: SkillIndexStatus | null;
  loadingSessions?: boolean;
  sessionListError?: string;
  sessionRefreshError?: string;
  onRefreshSessions?: () => Promise<number | null>;
  onResumeSession?: (
    session: SessionRecord,
    target?: Exclude<SessionResumeTarget, SessionResumeTarget.Auto>,
    options?: { reconcile?: boolean },
  ) => Promise<SessionResumeOutcome | null | undefined>;
  resolveSessionResumeTarget: (session: SessionRecord) => Promise<Exclude<SessionResumeTarget, SessionResumeTarget.Auto>>;
  sessionResumeTarget: SessionResumeTarget;
  missingSessionProjectPolicy: MissingSessionProjectPolicy;
  projects?: ProjectSummary[];
  sessionProjects?: SessionProjectSummary[];
  onOpenSkill?: (skillPath: string) => void;
  activeSessionKey?: string;
  onSavePrompt?: (body: string) => Promise<boolean>;
}) {
  const initialSession = resolveInitialSession(sessionItems, activeSessionKey);
  const [activeRowId, setActiveRowId] = useTabState(
    "sessions.activeRowId",
    initialSession ? sessionTableRowId(initialSession) : "",
  );
  const [transcriptImportProvider, setTranscriptImportProvider] = useState("");
  const [query, setQuery] = useTabState("sessions.query", "");
  const [sort, setSort] = useTabState<SortState>("sessions.sort", { key: SessionSortKey.UpdatedAt, direction: SortDirection.Desc });
  const [searchSort, setSearchSort] = useTabState<SortState | null>("sessions.searchSort", null);
  const [pageSelection, setPageSelection] = useTabState("sessions.pageSelection", { contextKey: "", page: 0 });
  const [pageSize, setPageSize] = useTabState("sessions.pageSize", 50);
  const [groupBy, setGroupBy] = useTabState<string | null>("sessions.groupBy", null);
  const [showChildSessions, setShowChildSessions] = useTabState("sessions.showChildSessions", false);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshActionError, setRefreshActionError] = useState("");
  const [sessionToast, setSessionToast] = useState("");
  const [detailCollapsed, setDetailCollapsed] = useTabState("sessions.detailCollapsed", false);
  const [sessionLocatorRequest, setSessionLocatorRequest] = useState("");
  const [activeSessionInListViewport, setActiveSessionInListViewport] = useState<boolean | null>(null);
  const [selectedProjectKeys, setSelectedProjectKeys] = useTabState<string[]>("sessions.selectedProjectKeys", []);
  const [projectFilterQuery, setProjectFilterQuery] = useState("");
  const [projectFilterOpen, setProjectFilterOpen] = useState(false);
  const [debouncedQuery, setDebouncedQuery] = useState(() => query.trim().toLowerCase());
  const importInputRef = useRef<HTMLInputElement | null>(null);
  const sessionListBodyRef = useRef<HTMLDivElement | null>(null);
  const keyboardNavigationScopeRef = useRef<KeyboardNavigationScope>(KeyboardNavigationScope.List);
  const pendingRemotePageEdgeRef = useRef<"first" | "last" | null>(null);
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "f") return;
      if (keyboardNavigationScopeRef.current !== KeyboardNavigationScope.List) return;
      const target = event.target instanceof Element ? event.target : null;
      const activeElement = document.activeElement;
      const isInLocalFindPane = (element: Element | null) => Boolean(element?.closest(".codePane, .transcriptPanel, [role=\"dialog\"]"));
      if (isInLocalFindPane(target) || isInLocalFindPane(activeElement)) return;
      const searchInput = document.querySelector<HTMLInputElement>("[data-page-search] input");
      if (!searchInput) return;
      event.preventDefault();
      event.stopPropagation();
      searchInput.focus();
      searchInput.select();
    };
    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [keyboardNavigationScopeRef]);
  const handledExternalSessionKeyRef = useRef("");
  const projectFilterInputRef = useRef<HTMLInputElement | null>(null);
  const sessionToastTimerRef = useRef<number | undefined>(undefined);
  const normalizedInputQuery = query.trim().toLowerCase();
  const normalizedQuery = debouncedQuery;
  const initialNormalizedQueryRef = useRef(true);
  const remoteSearchActive = Boolean(normalizedQuery);
  const showSessionError = useCallback((message: string) => {
    if (sessionToastTimerRef.current !== undefined) {
      window.clearTimeout(sessionToastTimerRef.current);
    }
    setSessionToast(message);
    sessionToastTimerRef.current = window.setTimeout(() => {
      setSessionToast("");
      sessionToastTimerRef.current = undefined;
    }, 5000);
  }, []);
  const dismissSessionError = useCallback(() => {
    if (sessionToastTimerRef.current !== undefined) {
      window.clearTimeout(sessionToastTimerRef.current);
      sessionToastTimerRef.current = undefined;
    }
    setSessionToast("");
  }, []);
  const handleImportedSession = useCallback((session: SessionRecord) => {
    setActiveRowId(sessionTableRowId(session));
    setDetailCollapsed(false);
  }, [setActiveRowId, setDetailCollapsed]);
  const {
    importedSessions,
    importedTranscripts,
    importFeedback,
    importError,
    importJsonl,
  } = useSessionImport({
    providerId: transcriptImportProvider,
    showSessionError,
    onImported: handleImportedSession,
  });
  useEffect(() => () => {
    if (sessionToastTimerRef.current !== undefined) window.clearTimeout(sessionToastTimerRef.current);
  }, []);
  const useLocalSessionList = importedSessions.length > 0;
  const allSessionItems = useMemo(() => {
    const byId = new Map<string, SessionRecord>();
    for (const session of [...importedSessions, ...sessionItems]) byId.set(sessionTableRowId(session), session);
    return [...byId.values()];
  }, [importedSessions, sessionItems]);
  const sessionMembershipKey = useMemo(
    () => sessionItems.map((session) => sessionLogicalIdentity(session)).join("\u0001"),
    [sessionItems],
  );
  const activeSort = remoteSearchActive ? searchSort ?? SESSION_SEARCH_SORT : sort;
  const pageContextKey = sessionPageContextKey(
    activeSort,
    groupBy,
    normalizedQuery,
    pageSize,
    selectedProjectKeys.join("\0"),
    showChildSessions,
  );
  const requestedPage = pageSelection.contextKey === pageContextKey ? pageSelection.page : 0;
  const { localListView, searchingSessions } = useSessionListWorkspace({
    enabled: useLocalSessionList,
    sessions: sessionItems,
    importedSessions,
    query: normalizedQuery,
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
    currentPage: pageSelection.page,
    pageSelectionContextKey: pageSelection.contextKey,
    searchSessions,
    onSearchError: showSessionError,
  });
  const remoteListBaseRequest = useMemo<SessionListPageRequest>(() => ({
    query: normalizedQuery,
    agent: sessionAgentFilter,
    sort: activeSort,
    groupBy,
    page: requestedPage,
    pageSize,
    showChildSessions,
    selectedProjectKeys,
  }), [activeSort, groupBy, normalizedQuery, pageSize, requestedPage, selectedProjectKeys, sessionAgentFilter, showChildSessions]);
  const onRemoteSessionLocated = useCallback((session: SessionRecord, page?: number) => {
    const targetRowId = sessionTableRowId(session);
    if (page !== undefined) setPageSelection({ contextKey: pageContextKey, page });
    setActiveRowId(targetRowId);
    setSessionLocatorRequest(targetRowId);
  }, [pageContextKey]);
  const revealRemoteSessionForLocate = useCallback((session: SessionRecord) => {
    const showChildren = showChildSessions || sessionKind(session) === SessionKind.Child;
    setQuery("");
    setDebouncedQuery("");
    setSearchSort(null);
    setSelectedProjectKeys([]);
    setProjectFilterQuery("");
    if (showChildren && !showChildSessions) setShowChildSessions(true);
    setPageSelection({
      contextKey: sessionPageContextKey(sort, groupBy, "", pageSize, "", showChildren),
      page: 0,
    });
  }, [groupBy, pageSize, setSelectedProjectKeys, setSearchSort, setShowChildSessions, setPageSelection, showChildSessions, sort]);
  const onRemoteSessionListError = useCallback((listError: unknown) => {
    logger.warn("sessions list failed", { error: listError });
    showSessionError("Could not load sessions. Try again.");
  }, [showSessionError]);
  const {
    remoteList,
    loading: loadingSessionListPage,
    locate: locateRemoteSession,
  } = useSessionRemoteListWorkspace({
    enabled: !useLocalSessionList,
    request: remoteListBaseRequest,
    sessionMembershipKey,
    pageContextKey,
    listSessions,
    onLocated: onRemoteSessionLocated,
    onRevealRequired: revealRemoteSessionForLocate,
    onError: onRemoteSessionListError,
  });
  const sessionSearchLoading = Boolean(
    normalizedInputQuery
      && (normalizedInputQuery !== normalizedQuery
        || (useLocalSessionList ? searchingSessions : loadingSessionListPage)),
  );
  const projectOptions = localListView?.projectOptions ?? remoteList?.projectOptions ?? EMPTY_SESSION_PROJECT_OPTIONS;
  const visibleProjectOptions = localListView?.visibleProjectOptions
    ?? selectVisibleSessionProjectOptions(projectOptions, selectedProjectKeys, projectFilterQuery);
  const childSessionCount = localListView?.childSessionCount ?? remoteList?.childSessionCount ?? 0;
  const currentPage = localListView?.currentPage ?? requestedPage;
  const sortedSessions = localListView?.sortedSessions ?? remoteList?.rows ?? EMPTY_SESSION_ROWS;
  const groupedPages = localListView?.groupedPages ?? EMPTY_GROUPED_SESSION_PAGES;
  const pageCount = localListView?.pageCount ?? remoteList?.pageCount ?? 1;
  const boundedCurrentPage = localListView?.boundedCurrentPage ?? remoteList?.page ?? 0;
  const pageStart = localListView?.pageStart ?? remoteList?.pageStart ?? 0;
  const pageEnd = localListView?.pageEnd ?? remoteList?.pageEnd ?? 0;
  const baseTableSessions = localListView?.tableSessions ?? remoteList?.rows ?? EMPTY_SESSION_ROWS;
  const tableSessions = useMemo(
    () => useLocalSessionList ? baseTableSessions : mergeSessionListRows(baseTableSessions, allSessionItems),
    [allSessionItems, baseTableSessions, useLocalSessionList],
  );
  const sessionResultCount = localListView?.sortedSessions.length ?? remoteList?.total ?? 0;
  const groupedPage = localListView
    ? groupedPages[boundedCurrentPage]
    : groupBy ? { rows: tableSessions, start: pageStart, end: pageEnd, groupCount: remoteList?.groupCount ?? 0 } : undefined;
  useEffect(() => {
    if (!useLocalSessionList && remoteList === null) return;
    const availableKeys = new Set(projectOptions.map((option) => option.key));
    setSelectedProjectKeys((current) => {
      const next = current.filter((key) => availableKeys.has(key));
      return next.length === current.length ? current : next;
    });
  }, [projectOptions, remoteList, setSelectedProjectKeys, useLocalSessionList]);
  useEffect(() => {
    if (!projectFilterOpen) return;
    window.requestAnimationFrame(() => projectFilterInputRef.current?.focus());
  }, [projectFilterOpen]);
  const pageSizeOptions = useMemo(
    () => [25, 50, 100].map((value) => ({ value: `${value}`, label: `${value}` })),
    [],
  );
  useEffect(() => {
    if (!normalizedInputQuery) {
      setDebouncedQuery("");
      return;
    }

    const timer = window.setTimeout(() => {
      setDebouncedQuery(normalizedInputQuery);
    }, SESSION_SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [normalizedInputQuery]);
  useEffect(() => {
    if (sessionListError && !sessionRefreshError && sessionItems.length === 0) {
      showSessionError("Could not load sessions. Try again.");
    }
  }, [sessionItems.length, sessionListError, sessionRefreshError, showSessionError]);
  useEffect(() => {
    if (sessionRefreshError) showSessionError(sessionRefreshError);
  }, [sessionRefreshError, showSessionError]);
  const setCurrentPage = useCallback((next: number | ((current: number) => number)) => {
    setPageSelection((current) => {
      const currentPageForContext = current.contextKey === pageContextKey ? current.page : 0;
      const nextPage = typeof next === "function" ? next(currentPageForContext) : next;
      return current.contextKey === pageContextKey && current.page === nextPage
        ? current
        : { contextKey: pageContextKey, page: nextPage };
    });
  }, [pageContextKey]);
  useLayoutEffect(() => {
    setPageSelection((current) => current.contextKey === pageContextKey
      ? current
      : { contextKey: pageContextKey, page: 0 });
  }, [pageContextKey]);
  const revealSessionInList = useCallback((session: SessionRecord) => {
    const targetRowId = sessionTableRowId(session);
    const showChildren = showChildSessions || sessionKind(session) === SessionKind.Child;
    const targetPage = sessionPageForRow(allSessionItems, targetRowId, sort, groupBy, pageSize, showChildren);
    if (targetPage < 0) return;

    keyboardNavigationScopeRef.current = KeyboardNavigationScope.List;
    setQuery("");
    setDebouncedQuery("");
    setSearchSort(null);
    setSelectedProjectKeys([]);
    setProjectFilterQuery("");
    if (showChildren && !showChildSessions) setShowChildSessions(true);
    setPageSelection({
      contextKey: sessionPageContextKey(sort, groupBy, "", pageSize, "", showChildren),
      page: targetPage,
    });
    setActiveRowId(targetRowId);
    setSessionLocatorRequest(targetRowId);
  }, [allSessionItems, groupBy, pageSize, showChildSessions, sort]);
  const locateSessionInList = useCallback((session: SessionRecord) => {
    const targetRowId = sessionTableRowId(session);
    if (!useLocalSessionList) {
      if (!allSessionItems.some((candidate) => sessionTableRowId(candidate) === targetRowId)) return;
      keyboardNavigationScopeRef.current = KeyboardNavigationScope.List;
      const visible = tableSessions.some((candidate) => sessionTableRowId(candidate) === targetRowId);
      locateRemoteSession(session, true, visible);
      return;
    }
    const plan = planSessionListLocation({
      targetRowId,
      currentPageRowIds: tableSessions.map(sessionTableRowId),
      currentResultRowIds: sortedSessions.map(sessionTableRowId),
      allRowIds: allSessionItems.map(sessionTableRowId),
    });
    if (plan === SessionListLocationPlan.Missing) return;
    if (plan === SessionListLocationPlan.Reveal) {
      revealSessionInList(session);
      return;
    }

    if (plan === SessionListLocationPlan.Page) {
      const targetIndex = sortedSessions.findIndex((candidate) => sessionTableRowId(candidate) === targetRowId);
      const targetPage = groupBy
        ? groupedPages.findIndex((page) => page.rows.some((candidate) => sessionTableRowId(candidate) === targetRowId))
        : targetIndex < 0 ? -1 : Math.floor(targetIndex / pageSize);
      if (targetPage < 0) return;
      setCurrentPage(targetPage);
    }
    keyboardNavigationScopeRef.current = KeyboardNavigationScope.List;
    setActiveRowId(targetRowId);
    setSessionLocatorRequest(targetRowId);
  }, [allSessionItems, groupBy, groupedPages, locateRemoteSession, pageSize, revealSessionInList, setCurrentPage, sortedSessions, tableSessions, useLocalSessionList]);
  const completeSessionLocator = useCallback((rowId: string) => {
    setSessionLocatorRequest((current) => current === rowId ? "" : current);
  }, []);
  const moveSession = useCallback((offset: number) => {
    if (sessionResultCount === 0) return;
    if (!useLocalSessionList) {
      const currentIndex = tableSessions.findIndex((session) => sessionTableRowId(session) === activeRowId);
      const targetIndex = currentIndex < 0
        ? (offset > 0 ? 0 : tableSessions.length - 1)
        : currentIndex + offset;
      if (targetIndex < 0 || targetIndex >= tableSessions.length) {
        const nextPage = boundedCurrentPage + (offset > 0 ? 1 : -1);
        if (nextPage < 0 || nextPage >= pageCount) return;
        pendingRemotePageEdgeRef.current = offset > 0 ? "first" : "last";
        setCurrentPage(nextPage);
        return;
      }
      const targetId = sessionTableRowId(tableSessions[targetIndex]);
      setActiveRowId(targetId);
      setDetailCollapsed(false);
      setSessionLocatorRequest(targetId);
      return;
    }
    const currentIndex = sortedSessions.findIndex((session) => sessionTableRowId(session) === activeRowId);
    const targetIndex = currentIndex < 0
      ? (offset > 0 ? 0 : sortedSessions.length - 1)
      : currentIndex + offset;
    if (targetIndex < 0 || targetIndex >= sortedSessions.length) return;
    const target = sortedSessions[targetIndex];
    const targetId = sessionTableRowId(target);
    const targetPage = groupBy
      ? groupedPages.findIndex((page) => page.rows.some((session) => sessionTableRowId(session) === targetId))
      : Math.floor(targetIndex / pageSize);
    if (targetPage >= 0 && targetPage !== currentPage) setCurrentPage(targetPage);
    setActiveRowId(targetId);
    setDetailCollapsed(false);
    setSessionLocatorRequest(targetId);
  }, [activeRowId, boundedCurrentPage, currentPage, groupBy, groupedPages, pageCount, pageSize, sessionResultCount, setCurrentPage, sortedSessions, tableSessions, useLocalSessionList]);
  useEffect(() => {
    if (useLocalSessionList || tableSessions.length === 0 || !pendingRemotePageEdgeRef.current) return;
    const target = pendingRemotePageEdgeRef.current === "first"
      ? tableSessions[0]
      : tableSessions[tableSessions.length - 1];
    const targetId = sessionTableRowId(target);
    pendingRemotePageEdgeRef.current = null;
    setActiveRowId(targetId);
    setDetailCollapsed(false);
    setSessionLocatorRequest(targetId);
  }, [tableSessions, useLocalSessionList]);
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
        keyboardNavigationScopeRef.current !== KeyboardNavigationScope.List
        || (event.key !== "ArrowUp" && event.key !== "ArrowDown")
        || event.defaultPrevented
        || event.metaKey
        || event.ctrlKey
        || event.altKey
        || event.shiftKey
        || isKeyboardNavigationIgnoredTarget(event.target)
      ) return;
      event.preventDefault();
      moveSession(event.key === "ArrowUp" ? -1 : 1);
    };
    window.addEventListener("keydown", handleKeyDown);
    return () => window.removeEventListener("keydown", handleKeyDown);
  }, [moveSession]);
  const handleSortChange = useCallback((next: SortState) => {
    const applySortChange = (current: SortState | null) => {
      if (current?.key === next.key) return next;
      return {
        key: next.key,
        direction: next.key === SessionSortKey.Title || next.key === SessionSortKey.Project ? SortDirection.Asc : SortDirection.Desc,
      } satisfies SortState;
    };
    if (remoteSearchActive) {
      setSearchSort(applySortChange);
      return;
    }
    setSort(applySortChange);
  }, [remoteSearchActive]);
  useEffect(() => {
    if (initialNormalizedQueryRef.current) {
      initialNormalizedQueryRef.current = false;
      return;
    }
    setSearchSort(null);
  }, [normalizedQuery, setSearchSort]);
  useLayoutEffect(() => {
    if (currentPage >= pageCount) setCurrentPage(pageCount - 1);
  }, [currentPage, pageCount, setCurrentPage]);
  useEffect(() => {
    if (activeRowId && !allSessionItems.some((session) => sessionTableRowId(session) === activeRowId)) setActiveRowId("");
  }, [activeRowId, allSessionItems]);
  useEffect(() => {
    const normalizedKey = `${activeSessionKey ?? ""}`.trim();
    if (!normalizedKey) {
      handledExternalSessionKeyRef.current = "";
      return;
    }
    if (handledExternalSessionKeyRef.current === normalizedKey) return;
    const next = resolveInitialSession(allSessionItems, normalizedKey);
    if (next) {
      handledExternalSessionKeyRef.current = normalizedKey;
      locateSessionInList(next);
      setDetailCollapsed(false);
    }
  }, [activeSessionKey, allSessionItems, locateSessionInList]);
  const openSession = useCallback((session: SessionRecord) => {
    keyboardNavigationScopeRef.current = KeyboardNavigationScope.List;
    setActiveRowId(sessionTableRowId(session));
    setDetailCollapsed(false);
  }, []);
  const refreshSessions = useCallback(async () => {
    if (refreshing || !onRefreshSessions) return;
    setRefreshActionError("");
    setRefreshing(true);
    try {
      await onRefreshSessions();
    } catch (error) {
      logger.warn("sessions refresh failed", { error });
      setRefreshActionError(SESSION_REFRESH_ERROR);
      showSessionError(SESSION_REFRESH_ERROR);
    } finally {
      setRefreshing(false);
    }
  }, [onRefreshSessions, refreshing, showSessionError]);
  const handleImportJsonlChange = useCallback((event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    importJsonl(file);
  }, [importJsonl]);
  const {
    pendingResumeConflict,
    repairingResume,
    resumeSession,
    repairAndResume,
    cancelResumeConflict,
    getResumeState,
  } = useSessionResumeOperations({ onResumeSession, showSessionError, dismissSessionError });
  const activeSession = useMemo(
    () => allSessionItems.find((session) => sessionTableRowId(session) === activeRowId),
    [activeRowId, allSessionItems],
  );
  const inferredResumeTargets = useInferredSessionResumeTargets({
    sessions: tableSessions,
    activeSession,
    sessionResumeTarget,
    resolveSessionResumeTarget,
  });
  const activeSessionVisibleInList = Boolean(
    activeSession && tableSessions.some((session) => sessionTableRowId(session) === sessionTableRowId(activeSession)),
  );
  useLayoutEffect(() => {
    let frame = 0;
    const root = sessionListBodyRef.current?.querySelector<HTMLElement>(".dataTableBodyScroll");
    const measure = () => {
      frame = 0;
      if (!activeSession || !activeSessionVisibleInList || !root) {
        setActiveSessionInListViewport(false);
        return;
      }
      const rowId = sessionTableRowId(activeSession);
      const row = [...root.querySelectorAll<HTMLElement>("[data-row-id]")]
        .find((candidate) => candidate.dataset.rowId === rowId);
      if (!row) {
        setActiveSessionInListViewport(false);
        return;
      }
      const rootBounds = root.getBoundingClientRect();
      const header = root.querySelector<HTMLElement>(".dataTableHeader");
      const headerBounds = header?.getBoundingClientRect();
      const visibleTop = Math.max(rootBounds.top, headerBounds?.bottom ?? rootBounds.top);
      const rowBounds = row.getBoundingClientRect();
      const visible = rowBounds.top >= visibleTop - 1
        && rowBounds.bottom <= rootBounds.bottom + 1;
      setActiveSessionInListViewport((current) => current === visible ? current : visible);
    };
    const scheduleMeasure = () => {
      if (frame !== 0) return;
      frame = window.requestAnimationFrame(measure);
    };
    if (!root) {
      measure();
      return undefined;
    }
    measure();
    root.addEventListener("scroll", scheduleMeasure, { passive: true });
    window.addEventListener("resize", scheduleMeasure);
    const resizeObserver = typeof ResizeObserver === "undefined" ? null : new ResizeObserver(scheduleMeasure);
    resizeObserver?.observe(root);
    return () => {
      root.removeEventListener("scroll", scheduleMeasure);
      window.removeEventListener("resize", scheduleMeasure);
      resizeObserver?.disconnect();
      if (frame !== 0) window.cancelAnimationFrame(frame);
    };
  }, [activeSession, activeSessionVisibleInList, tableSessions]);
  const locateActiveSession = useCallback(() => {
    if (activeSession) locateSessionInList(activeSession);
  }, [activeSession, locateSessionInList]);
  const sessionRelationships = useMemo(
    () => selectSessionRelationships(allSessionItems, activeSession),
    [activeSession, allSessionItems],
  );
  const { childSessions: activeChildSessions, tree: activeSessionTree } = sessionRelationships;
  const openRelatedSession = useCallback((session: SessionRecord) => {
    locateSessionInList(session);
    keyboardNavigationScopeRef.current = KeyboardNavigationScope.Detail;
    setDetailCollapsed(false);
  }, [locateSessionInList]);
  const activeImportedTranscript = activeSession ? importedTranscripts[activeSession.id] : undefined;
  const activeSessionLogicalKey = useMemo(() => (
    activeSession ? sessionExternalKey(activeSession) : ""
  ), [activeSession?.agent, activeSession?.id]);
  const activeSessionTranscriptKey = useMemo(() => (
    activeSession
      ? `${activeSession.agent}:${activeSession.path}:${activeSession.id}`
      : ""
  ), [activeSession?.agent, activeSession?.id, activeSession?.path]);
  const activeSessionTranscriptRefreshKey = useMemo(() => (
    activeSession
      ? `${activeSessionTranscriptKey}:${activeSession.updatedAt ?? ""}:${activeSession.messages ?? ""}`
      : ""
  ), [activeSession?.messages, activeSession?.updatedAt, activeSessionTranscriptKey]);
  const {
    items,
    locatorItems: activeTranscriptLocatorItems,
    loading,
    loadingMore: loadingMoreTranscript,
    hasMore: hasMoreTranscript,
    scrollRestorationReady,
    loadMore: loadMoreTranscript,
    loadAll: loadAllTranscript,
    loadUntilTarget: loadTranscriptUntilTarget,
  } = useSessionTranscript({
    session: activeSession ?? null,
    transcriptKey: activeSessionTranscriptKey,
    logicalKey: activeSessionLogicalKey,
    refreshKey: activeSessionTranscriptRefreshKey,
    importedItems: activeImportedTranscript,
    loadTranscript,
    loadTranscriptLocator,
    onError: showSessionError,
  });
  const activeSessionLinkKey = useMemo(() => (
    activeSession ? sessionKey(activeSession) : ""
  ), [activeSession?.agent, activeSession?.id, activeSession?.path]);
  const activeSessionSkillLinksKey = useMemo(() => {
    if (!activeSession) return "";
    return [
      activeSessionLinkKey,
      skillIndexStatus?.last_indexed_at ?? "",
    ].join(":");
  }, [activeSession, activeSessionLinkKey, skillIndexStatus?.last_indexed_at]);
  const {
    links: skillLinks,
    loading: skillLinksLoading,
    loaded: skillLinksLoaded,
    error: skillLinksError,
    retry: retryActiveSessionSkillLinks,
  } = useSessionSkillLinks({
    session: activeSession ?? null,
    sessionIdentityKey: activeSessionLinkKey,
    requestKey: activeSessionSkillLinksKey,
    loadSessionSkillLinks,
    importedAgent: IMPORTED_SESSION_AGENT,
    onToastError: showSessionError,
  });
  const columns = useMemo(
    (): ColumnDef<SessionRecord>[] => createSessionTableColumns({
      normalizedQuery,
      resumeSession,
      resumeState: getResumeState,
      resumeTarget: sessionResumeTarget,
      resumeTargetForSession: (session) => inferredResumeTargets[sessionSourceIdentity(session as SessionRecord)],
    }),
    [getResumeState, inferredResumeTargets, normalizedQuery, resumeSession, sessionResumeTarget],
  );
  const rowContextMenu = useCallback((session: SessionRecord) => {
    const transcriptPath = session.path.trim();
    const workspacePath = sessionWorkspacePath(session);
    const deeplink = sessionAppDeepLink(session);
    const configuredTarget = sessionResumeTargetForAgent(sessionResumeTarget, session.agent);
    const target = sessionResumeTargetForMenu(
      configuredTarget,
      inferredResumeTargets[sessionSourceIdentity(session)],
    );
    const canResume = session.agent !== IMPORTED_SESSION_AGENT
      && Boolean(session.id && session.agent && transcriptPath);
    const resumeTargets = sessionResumeTargetsForMenu({
      terminal: canResume,
      app: canResume && Boolean(deeplink),
    });
    return (
      <>
        {resumeTargets.length > 1 ? (
          <ContextMenu.Sub>
            <ContextMenu.SubTrigger className="menuItem menuSubTrigger">
              <MessageSquareText size={14} />
              <span>Resume in</span>
              <ChevronRight className="menuSubIcon" size={14} />
            </ContextMenu.SubTrigger>
            <ContextMenu.Portal>
              <ContextMenu.SubContent className="menuContent" sideOffset={8}>
                {resumeTargets.map((resumeTarget) => (
                  <ContextMenu.Item
                    className="menuItem"
                    key={resumeTarget}
                    onSelect={() => { void resumeSession(session, resumeTarget); }}
                  >
                    {resumeTarget === SessionResumeTarget.App ? <AppWindow size={14} /> : <TerminalSquare size={14} />}
                    {resumeTarget === SessionResumeTarget.App ? "App" : "Terminal"}
                  </ContextMenu.Item>
                ))}
              </ContextMenu.SubContent>
            </ContextMenu.Portal>
          </ContextMenu.Sub>
        ) : (
          <ContextMenu.Item
            className="menuItem"
            disabled={!canResume}
            aria-busy={target === SessionResumeTarget.Auto || undefined}
            onSelect={() => { void resumeSession(session); }}
          >
            {target === SessionResumeTarget.App ? <AppWindow size={14} /> : target === SessionResumeTarget.Terminal ? <TerminalSquare size={14} /> : <LoadingIcon size={14} />}
            {sessionResumeLabel(AsyncStatus.Idle, target)}
          </ContextMenu.Item>
        )}
        <ContextMenu.Separator className="menuSeparator" />
        <CopyTextMenuItem Menu={ContextMenu} text={session.id} label="Copy session ID" />
        {deeplink && <CopyTextMenuItem Menu={ContextMenu} text={deeplink} label="Copy deeplink" />}
        <ContextMenu.Separator className="menuSeparator" />
        <OpenInEditorMenuItem Menu={ContextMenu} path={transcriptPath} />
        <CopyPathMenuItem Menu={ContextMenu} path={transcriptPath} label={copyPathLabel("transcript")} />
        <RevealInFinderMenuItem Menu={ContextMenu} path={transcriptPath} label={revealPathLabel("transcript")} />
        <ContextMenu.Separator className="menuSeparator" />
        <CopyPathMenuItem Menu={ContextMenu} path={workspacePath} label={copyPathLabel("workspace")} />
        <RevealInFinderMenuItem Menu={ContextMenu} path={workspacePath} label={revealPathLabel("workspace")} />
      </>
    );
  }, [inferredResumeTargets, resumeSession, sessionResumeTarget]);
  const importButtonStateClass = importFeedback === ImportFeedbackState.Success
    ? "isSuccess"
    : importFeedback === ImportFeedbackState.Warning
      ? "isWarning"
      : importFeedback === ImportFeedbackState.Error
        ? "isError"
        : "";
  const importButtonLabel = importFeedback === ImportFeedbackState.Loading
    ? "Importing JSONL transcript"
    : importFeedback === ImportFeedbackState.Success
      ? "JSONL transcript imported"
      : importFeedback === ImportFeedbackState.Warning
        ? "JSONL transcript imported with warnings"
        : importFeedback === ImportFeedbackState.Error
          ? importError || "Could not import JSONL transcript"
          : "Import JSONL transcript";
  const showSessionListLocator = shouldShowSessionListLocator({
    hasActiveSession: Boolean(activeSession),
    detailCollapsed,
    activeSessionInListViewport,
  });
  return (
    <>
    {sessionToast ? <Toast tone="error" message={sessionToast} onDismiss={dismissSessionError} /> : null}
    <PanelGroup className="sessionsLayout" orientation="horizontal">
      <Panel className="sessionListPanel" defaultSize="62%" minSize="390px">
        <div className="sessionListPane">
          <PageHeader title="Sessions" compact>
            {developerMode ? (
              <>
                <SelectControl
                  label="Transcript provider"
                  value={transcriptImportProvider || TRANSCRIPT_IMPORT_PROVIDER_PLACEHOLDER}
                  onValueChange={(value) => setTranscriptImportProvider(
                    value === TRANSCRIPT_IMPORT_PROVIDER_PLACEHOLDER ? "" : value,
                  )}
                  options={[
                    { value: TRANSCRIPT_IMPORT_PROVIDER_PLACEHOLDER, label: "Choose provider" },
                    ...TRANSCRIPT_IMPORT_PROVIDERS,
                  ]}
                />
                <IconButton
                  className={`sessionImportButton ${importButtonStateClass}`}
                  aria-label={importButtonLabel}
                  aria-busy={importFeedback === ImportFeedbackState.Loading}
                  disabled={importFeedback === ImportFeedbackState.Loading || !transcriptImportProvider}
                  onClick={() => importInputRef.current?.click()}
                >
                  {importFeedback === ImportFeedbackState.Loading
                    ? <LoadingIcon size={16} />
                    : importFeedback === ImportFeedbackState.Success
                      ? <Check size={16} />
                      : importFeedback === ImportFeedbackState.Warning || importFeedback === ImportFeedbackState.Error
                        ? <AlertCircle size={16} />
                        : <Upload size={16} />}
                </IconButton>
                <input
                  ref={importInputRef}
                  className="sessionImportInput"
                  type="file"
                  accept=".jsonl,application/jsonl,application/x-ndjson,text/plain"
                  onChange={handleImportJsonlChange}
                />
              </>
            ) : null}
            <IconButton
              className={`sessionRefreshButton${sessionRefreshError || refreshActionError ? " isError" : ""}`}
              aria-label={sessionRefreshError || refreshActionError ? "Refresh sessions (last attempt failed)" : "Refresh sessions"}
              aria-busy={refreshing}
              onClick={refreshSessions}
              disabled={refreshing}
            >
              {refreshing ? <LoadingIcon size={16} /> : <RefreshCw size={16} />}
            </IconButton>
            <IconButton
              className={showChildSessions ? "filled" : ""}
              aria-label={showChildSessions ? "Hide child sessions" : `Show ${childSessionCount} child sessions`}
              aria-pressed={showChildSessions}
              onClick={() => setShowChildSessions((visible) => !visible)}
            >
              <GitFork size={16} />
            </IconButton>
            <div className="sessionSearchControls">
              <SearchField
                pageSearch
                placeholder="Search sessions"
                value={query}
                loading={sessionSearchLoading}
                onChange={(event) => setQuery(event.target.value)}
                onClear={() => setQuery("")}
              />
              <DropdownMenu.Root
                open={projectFilterOpen}
                onOpenChange={(open) => {
                  setProjectFilterOpen(open);
                  if (!open) setProjectFilterQuery("");
                }}
              >
                <DropdownMenu.Trigger asChild>
                  <IconButton
                    className={`sessionProjectFilter${selectedProjectKeys.length > 0 ? " filled" : ""}`}
                    aria-label={selectedProjectKeys.length > 0 ? `Filter projects, ${selectedProjectKeys.length} selected` : "Filter projects"}
                    aria-pressed={selectedProjectKeys.length > 0}
                  >
                    <Filter size={16} aria-hidden="true" />
                    {selectedProjectKeys.length > 0 ? <span className="sessionProjectFilterCount" aria-hidden="true">{selectedProjectKeys.length}</span> : null}
                  </IconButton>
                </DropdownMenu.Trigger>
                <DropdownMenu.Portal>
                  <MenuContent
                    className="sessionProjectFilterMenu"
                    align="end"
                    sideOffset={6}
                    data-no-drag
                  >
                    <div className="sessionProjectFilterSearch" onClick={(event) => event.stopPropagation()}>
                      <Search size={13} aria-hidden="true" />
                      <input
                        ref={projectFilterInputRef}
                        aria-label="Filter projects"
                        placeholder="Filter projects"
                        value={projectFilterQuery}
                        onChange={(event) => setProjectFilterQuery(event.target.value)}
                        onKeyDown={(event) => {
                          if (event.key !== "Escape") event.stopPropagation();
                        }}
                      />
                      <SearchClearButton value={projectFilterQuery} onClear={() => setProjectFilterQuery("")} ariaLabel="Clear project filter" />
                    </div>
                    <div className="sessionProjectFilterOptions">
                      {visibleProjectOptions.map((option) => {
                        const active = selectedProjectKeys.includes(option.key);
                        return (
                          <DropdownMenu.CheckboxItem
                            key={option.key}
                            className="menuItem sessionProjectFilterItem"
                            checked={active}
                            onCheckedChange={(checked) => {
                              setSelectedProjectKeys((current) => checked
                                ? current.includes(option.key) ? current : [...current, option.key]
                                : current.filter((key) => key !== option.key));
                            }}
                            onSelect={(event) => event.preventDefault()}
                          >
                            <CheckboxIndicator checked={active} />
                            {isWebSource(option.title.trim())
                              ? <span className="sessionProjectFilterItemLabel">{option.label}</span>
                              : <span className="sessionProjectFilterItemLabel">{option.label}</span>}
                            <span className="sessionProjectFilterItemCount">{option.count}</span>
                          </DropdownMenu.CheckboxItem>
                        );
                      })}
                      {visibleProjectOptions.length === 0 ? <span className="sessionProjectFilterEmpty">No matching projects</span> : null}
                    </div>
                    <div className="sessionProjectFilterFooter">
                      <span>{selectedProjectKeys.length} active</span>
                      <Button
                        variant="ghost"
                        size="compact"
                        className="sessionProjectFilterClearButton"
                        disabled={selectedProjectKeys.length === 0}
                        onClick={(event) => {
                          setSelectedProjectKeys([]);
                          setProjectFilterOpen(false);
                          setProjectFilterQuery("");
                          event.currentTarget.blur();
                        }}
                      >
                        <X size={14} aria-hidden="true" />
                        Clear all
                      </Button>
                    </div>
                  </MenuContent>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
            </div>
          </PageHeader>
          <div
            className="sessionListBody"
            ref={sessionListBodyRef}
            aria-busy={loadingSessionListPage || undefined}
            onPointerDownCapture={() => { keyboardNavigationScopeRef.current = KeyboardNavigationScope.List; }}
            onFocusCapture={() => { keyboardNavigationScopeRef.current = KeyboardNavigationScope.List; }}
          >
            <DataTable
              rows={tableSessions}
              columns={columns}
              getRowId={sessionTableRowId}
              getRowLabel={(session) => formatSessionTitle(sessionTitleValue(session))}
              freezeColumn={SESSION_FREEZE_COLUMN}
              defaultSort={{ key: SessionSortKey.UpdatedAt, direction: SortDirection.Desc }}
              sort={activeSort}
              onSortChange={handleSortChange}
              manualSorting
              rowHeight={SESSION_TABLE_ROW_HEIGHT}
              enableVirtualization={false}
              scrollRestorationKey="sessions.list"
              scrollResetKey={`${pageContextKey}\u0000${boundedCurrentPage}`}
              scrollToRowId={sessionLocatorRequest}
              onScrollToRowComplete={completeSessionLocator}
              groupBy={groupBy}
              onGroupByChange={setGroupBy}
              onRowClick={openSession}
              rowContextMenu={rowContextMenu}
              rowProps={(session) => (activeRowId === sessionTableRowId(session) ? { className: "rowSelected" } : {})}
              loading={(loadingSessions || searchingSessions || loadingSessionListPage) && sessionResultCount === 0}
              loadingLabel="Loading sessions"
              emptyState={<EmptyState icon={<SearchX size={22} strokeWidth={1.75} />} iconTone="muted" title="No matching sessions" />}
            />
            {showSessionListLocator ? (
              <IconButton
                className="sessionListLocator"
                aria-label="Locate session in list"
                onClick={locateActiveSession}
              >
                <LocateFixed size={15} aria-hidden="true" />
              </IconButton>
            ) : null}
          </div>
          <div className="sessionPager">
            <div className="sessionPagerInfo">
              <span>
                {sessionResultCount === 0
                  ? "0 sessions"
                  : groupBy
                    ? `${pageStart + 1}-${pageEnd} of ${sessionResultCount} sessions · ${groupedPage?.groupCount ?? 0} groups`
                    : `${pageStart + 1}-${pageEnd} of ${sessionResultCount}`}
              </span>
              <SelectControl
                label="Rows per page"
                value={`${pageSize}`}
                onValueChange={(value: string) => setPageSize(Number(value))}
                options={pageSizeOptions}
                contentClassName="sessionPageSizeSelectContent"
              />
            </div>
            <div className="sessionPagerControls">
              <IconButton
                aria-label="Previous page"
                onClick={() => setCurrentPage((page) => Math.max(0, page - 1))}
                disabled={boundedCurrentPage === 0}
              >
                <ChevronLeft size={15} />
              </IconButton>
              <span>{boundedCurrentPage + 1} / {pageCount}</span>
              <IconButton
                aria-label="Next page"
                onClick={() => setCurrentPage((page) => Math.min(pageCount - 1, page + 1))}
                disabled={boundedCurrentPage >= pageCount - 1}
              >
                <ChevronRight size={15} />
              </IconButton>
            </div>
          </div>
        </div>
      </Panel>
      {activeSession ? (
        <DetailPanelHost
          collapsed={detailCollapsed}
          onExpand={() => setDetailCollapsed(false)}
          expandLabel="Expand session detail"
          railLabel={formatSessionTitle(sessionTitleValue(activeSession))}
          hasSelection
          emptyState={null}
          expandedDefaultSize="38%"
          panelClassName="transcriptPanel"
        >
          <TranscriptPanel
            session={activeSession}
            childSessions={activeChildSessions}
            sessionTree={activeSessionTree}
              items={items}
              locatorMetadata={activeTranscriptLocatorItems}
              sessionSearchQuery={normalizedQuery}
            loading={loading}
            scrollRestorationReady={scrollRestorationReady}
            onReportError={showSessionError}
            hasMore={hasMoreTranscript}
            loadingMore={loadingMoreTranscript}
            skillLinks={skillLinks}
              loadingSkillLinks={skillLinksLoading}
              skillLinksLoaded={skillLinksLoaded}
              skillLinksError={skillLinksError}
            onCollapse={() => {
              keyboardNavigationScopeRef.current = KeyboardNavigationScope.List;
              setDetailCollapsed(true);
            }}
            keyboardNavigationScopeRef={keyboardNavigationScopeRef}
            onOpenSession={openRelatedSession}
            onOpenSkill={onOpenSkill}
            onLoadSkills={retryActiveSessionSkillLinks}
            onLoadMore={loadMoreTranscript}
            loadUntilTarget={loadTranscriptUntilTarget}
            onLoadAll={loadAllTranscript}
            searchTranscript={searchTranscript}
            onSavePrompt={onSavePrompt}
          />
        </DetailPanelHost>
      ) : null}
    </PanelGroup>
    <DialogShell
      open={Boolean(pendingResumeConflict)}
      onOpenChange={(open) => {
        if (!open && !repairingResume) cancelResumeConflict();
      }}
      descriptionId="session-resume-conflict-description"
    >
      <Dialog.Title className="confirmDialogTitle">Resume active session?</Dialog.Title>
      <Dialog.Description id="session-resume-conflict-description" className="confirmDialogDescription">
        This {pendingResumeConflict ? friendlyAgent(pendingResumeConflict.session.agent) : "agent"} session is still open in another process. Close it first, then archive and unarchive it before resuming.
      </Dialog.Description>
      <div className="confirmDialogActions">
        <DialogActionButton
          variant="primary"
          onClick={() => { void repairAndResume(); }}
          disabled={repairingResume}
          aria-busy={repairingResume}
          aria-label="Archive, unarchive, and resume"
          autoFocus={!repairingResume}
          style={{ minWidth: "220px" }}
        >
          {repairingResume ? <LoadingIcon size={15} /> : "Archive, unarchive, and resume"}
        </DialogActionButton>
        <DialogActionButton
          variant="secondary"
          onClick={cancelResumeConflict}
          disabled={repairingResume}
        >
          Cancel
        </DialogActionButton>
      </div>
    </DialogShell>
    </>
  );
}
