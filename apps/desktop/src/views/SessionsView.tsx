import { Tooltip as AppTooltip } from "../components/shared/Tooltip.tsx";
import { lazy, memo, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ChangeEvent, type ReactNode } from "react";
import { AlertCircle, ArrowDownToLine, ArrowUpRight, ArrowUpToLine, BarChart3, Check, ChevronDown, ChevronLeft, ChevronRight, ChevronRight as ChevronRightIcon, Filter, FolderOpen, GitFork, Info, LocateFixed, MessageSquarePlus, MessageSquareText, PanelRightClose, RefreshCw, Search, SearchX, Sparkles, TerminalSquare, Upload, X } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu, DropdownMenu, Popover } from "radix-ui";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import type { TokenMetricProps } from "../components/TokenStatusBar.tsx";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { Badge } from "../components/shared/Badge.tsx";
import { Button } from "../components/shared/Button.tsx";
import { CheckboxIndicator } from "../components/shared/CheckboxIndicator.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { CopyableSessionId } from "../features/sessions/CopyableSessionId.tsx";
import { CopyPathMenuItem, CopyTextMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { EmptyState } from "../components/shared/EmptyState.tsx";
import { InfoDropdownMenu } from "../components/shared/InfoDropdownMenu.tsx";
import { InfoSection } from "../components/shared/InfoSection.tsx";
import { IconButton } from "../components/shared/IconButton.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { LoadingState } from "../components/shared/LoadingState.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { SearchClearButton } from "../components/shared/SearchClearButton.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { StatefulButton } from "../components/shared/StatefulButton.tsx";
import { Toast } from "../components/shared/Toast.tsx";
import { useElementSize } from "../components/shared/useElementSize.ts";
import { findTextRanges } from "../components/shared/text-ranges.ts";
import { SkillRelationshipMap } from "../features/skills/SkillRelationshipMap.tsx";
import { createSessionTableColumns } from "../features/sessions/createSessionTableColumns.tsx";
import { SessionTitleText, TranscriptLinkText } from "../components/shared/TranscriptLinkText.tsx";
import "./SessionsView.css";
import { cacheRateTone } from "../lib/token-style.ts";
import {
  SESSION_FREEZE_COLUMN,
  TauriCommand,
  compactDateTime,
  compareSessions,
  createLatestRequestAuthority,
  dayGroupKey,
  formatDuration,
  formatSessionTitle,
  formatTranscriptPreview,
  formatUserPath,
  friendlyAgent,
  groupTranscriptItems,
  invokeCommand,
  isWebSource,
  logger,
  mergeTranscriptItems,
  resolveInitialSession,
  safeInvoke,
  sessionCacheRate,
  sessionKind,
  sessionProject,
  sessionProjectGroupKey,
  sessionResumeTargetForAgent,
  sessionWorkspace,
  sessionWorkspacePath,
  transcriptItemType,
  transcriptItemsSize,
} from "../lib/index.ts";
import type {
  JsonlTranscriptParseResult,
  TranscriptLocatorItem,
  TranscriptLocatorPage,
  TranscriptPage,
  TranscriptSearchHit,
  TranscriptSearchResult,
  TranscriptSearchScopes,
  SessionResumeTarget,
} from "../lib/index.ts";

const SessionTokenStatusBar = lazy(() => import("../components/SessionTokenStatusBar.tsx").then(({ SessionTokenStatusBar: component }) => ({ default: component })));

export type SessionRecord = {
  id: string;
  title: string;
  project?: string;
  projectPath?: string;
  repository?: string;
  repositoryPath?: string;
  repositoryUrl?: string;
  logicalProjectId?: string;
  logicalProjectName?: string;
  path?: string;
  agent?: string;
  startedAt?: string;
  updatedAt?: string;
  time?: string;
  startedLabel?: string;
  updatedLabel?: string;
  updatedDetailLabel?: string;
  messages?: number;
  firstUserMessage?: string;
  lastUserMessage?: string;
  lastAssistantMessage?: string;
  turnCount?: number;
  model?: string;
  mode?: string;
  approvalMode?: string;
  isRunEverything?: boolean;
  parentSessionId?: string;
  searchScore?: number;
  searchSnippet?: string;
  tokenUsage?: {
    inputTokens: number;
    cachedInputTokens: number;
    outputTokens: number;
    reasoningOutputTokens: number;
    totalTokens: number;
  };
};

type TranscriptItemRecord = {
  type?: string;
  kind?: string;
  body?: string;
  tag?: string;
  time?: string;
  command?: string;
  result?: string;
  durationMs?: string | number;
  linkedSessionId?: string;
  model?: string;
  effort?: string;
  mode?: string;
  tools?: TranscriptItemRecord[];
  callId?: string;
  startedAtMs?: number;
};

export type SkillLinkRecord = {
  skill_name?: string;
  skillName?: string;
  skill_path?: string;
  skillPath?: string;
  evidence_text?: string;
  evidenceText?: string;
  evidence_time?: string;
  evidenceTime?: string;
};

type SkillIndexStatus = {
  indexed?: number;
  failed?: number;
  running?: boolean;
};

const SESSION_SEARCH_DEBOUNCE_MS = 300;
const SESSION_LOCATOR_MIN_ITEMS = 4;
const SESSION_REFRESH_ERROR = "Could not refresh sessions. Try again.";
const IMPORTED_SESSION_AGENT = "Imported";
const TRANSCRIPT_CACHE_ITEM_LIMIT = 1_200;
const TRANSCRIPT_CACHE_CHARACTER_LIMIT = 8 * 1024 * 1024;

function resumeTargetLabel(target: SessionResumeTarget): string {
  return target === "app" ? "app" : "terminal";
}

type SessionResumeState = "idle" | "loading" | "success" | "error";
type ImportFeedbackState = "idle" | "loading" | "success" | "warning" | "error";
type ResumeFeedbackState = Exclude<SessionResumeState, "idle">;

type TranscriptImportWorkerResponse =
  | { ok: true; result: JsonlTranscriptParseResult }
  | { ok: false; error: string };

function parseImportedTranscript(
  file: File,
  workerRef: { current: { worker: Worker; cancel: () => void } | null },
): Promise<JsonlTranscriptParseResult> {
  const worker = new Worker(
    new URL("../workers/transcript-import.worker.ts", import.meta.url),
    { type: "module" },
  );
  return new Promise((resolve, reject) => {
    let settled = false;
    const cleanup = () => {
      if (workerRef.current?.worker === worker) workerRef.current = null;
      worker.terminate();
    };
    const cancel = () => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(new Error("Transcript import cancelled"));
    };
    worker.onmessage = (event: MessageEvent<TranscriptImportWorkerResponse>) => {
      if (settled) return;
      settled = true;
      cleanup();
      if (event.data.ok) resolve(event.data.result);
      else reject(new Error(event.data.error));
    };
    worker.onerror = (event) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(new Error(event.message || "Transcript import worker failed"));
    };
    workerRef.current = { worker, cancel };
    try {
      worker.postMessage(file);
    } catch (error) {
      logger.error("sessions transcript import worker failed", { error });
      settled = true;
      cleanup();
      reject(error);
    }
  });
}

function trimTranscriptCache(cache: Map<string, TranscriptPage>) {
  let itemCount = 0;
  let characterCount = 0;
  for (const page of cache.values()) {
    itemCount += page.items.length;
    characterCount += transcriptItemsSize(page.items);
  }
  while (
    cache.size > 1
    && (itemCount > TRANSCRIPT_CACHE_ITEM_LIMIT || characterCount > TRANSCRIPT_CACHE_CHARACTER_LIMIT)
  ) {
    const oldestKey = cache.keys().next().value;
    if (oldestKey === undefined) break;
    const oldest = cache.get(oldestKey);
    cache.delete(oldestKey);
    if (oldest) {
      itemCount -= oldest.items.length;
      characterCount -= transcriptItemsSize(oldest.items);
    }
  }
  if (cache.size === 1 && (itemCount > TRANSCRIPT_CACHE_ITEM_LIMIT || characterCount > TRANSCRIPT_CACHE_CHARACTER_LIMIT)) {
    const key = cache.keys().next().value;
    if (key !== undefined) cache.delete(key);
  }
}

type TranscriptSearchScope = "user" | "assistant" | "system" | "tool";

type TranscriptSearchScopeState = Record<TranscriptSearchScope, boolean>;

const TRANSCRIPT_SEARCH_SCOPES: Array<{
  id: TranscriptSearchScope;
  label: string;
  ariaLabel: string;
}> = [
  { id: "user", label: "User", ariaLabel: "Search user messages" },
  { id: "assistant", label: "Assistant", ariaLabel: "Search assistant messages" },
  { id: "system", label: "System", ariaLabel: "Search system messages" },
  { id: "tool", label: "Tools", ariaLabel: "Search tool calls" },
];

const DEFAULT_TRANSCRIPT_SEARCH_SCOPES: TranscriptSearchScopeState = {
  user: true,
  assistant: true,
  system: false,
  tool: false,
};

function transcriptSearchScope(type: string | undefined): TranscriptSearchScope {
  switch (type) {
    case "user":
    case "notification":
      return "user";
    case "context":
    case "compaction":
    case "model_config":
      return "system";
    case "tool":
    case "toolGroup":
      return "tool";
    case "assistant":
    case "thinking":
    case "reasoning":
    default:
      return "assistant";
  }
}

function transcriptSearchScopeLabel(scopes: TranscriptSearchScopeState) {
  const active = TRANSCRIPT_SEARCH_SCOPES.filter((scope) => scopes[scope.id]);
  if (active.length === TRANSCRIPT_SEARCH_SCOPES.length) return "All";
  if (active.length === 1) return active[0].label;
  return `${active.length} selected`;
}

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

type SkillEvidenceTarget = {
  key: string;
  groupKey?: string;
  index: number;
};

type TranscriptSearchTarget = SkillEvidenceTarget;

type KeyboardNavigationScope = "list" | "detail";

type KeyboardNavigationScopeRef = {
  current: KeyboardNavigationScope;
};

type SessionLocatorItem = {
  key: string;
  index: number;
  label: string;
  response: string;
};

const TRANSCRIPT_VIRTUAL_THRESHOLD = 120;
const TRANSCRIPT_VIRTUAL_OVERSCAN = 12;
const TRANSCRIPT_DEFAULT_ITEM_HEIGHT = 96;
const TRANSCRIPT_ITEM_VERTICAL_INSET = 6;
const TRANSCRIPT_BUBBLE_TOP_INSET = 22;
const SESSION_TABLE_ROW_HEIGHT = 72;
const SESSION_LOCATOR_ROW_HEIGHT = 10;
const SESSION_LOCATOR_OVERSCAN = 16;

function isChatTranscriptType(type: string | undefined) {
  return type === "user" || type === "assistant";
}

function estimatedTranscriptItemHeight(item: TranscriptItemRecord, previousItem?: TranscriptItemRecord): number {
  const type = transcriptItemType(item);
  const previousType = previousItem ? transcriptItemType(previousItem) : undefined;
  const topInset = isChatTranscriptType(type) && !isChatTranscriptType(previousType)
    ? TRANSCRIPT_BUBBLE_TOP_INSET
    : 0;
  switch (type) {
    case "user":
    case "assistant":
      return 120 + TRANSCRIPT_ITEM_VERTICAL_INSET + topInset;
    case "toolGroup":
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

function offsetIndex(offsets: number[], target: number): number {
  if (offsets.length <= 1) return 0;
  let low = 0;
  let high = offsets.length - 1;
  while (low < high) {
    const middle = Math.ceil((low + high) / 2);
    if (offsets[middle] <= target) low = middle;
    else high = middle - 1;
  }
  return Math.min(offsets.length - 2, Math.max(0, low));
}

function transcriptRangeForViewport(
  itemCount: number,
  offsets: number[],
  scrollTop: number,
  viewportHeight: number,
  virtualized: boolean,
) {
  if (!virtualized || itemCount === 0) return { start: 0, end: itemCount };
  const firstVisible = offsetIndex(offsets, Math.max(0, scrollTop));
  const lastVisible = offsetIndex(offsets, Math.max(0, scrollTop + viewportHeight));
  return {
    start: Math.max(0, firstVisible - TRANSCRIPT_VIRTUAL_OVERSCAN),
    end: Math.min(itemCount, lastVisible + TRANSCRIPT_VIRTUAL_OVERSCAN + 1),
  };
}

function useTranscriptVirtualizer(items: TranscriptItemRecord[], rootRef: { current: HTMLDivElement | null }, resetKey: string) {
  const virtualized = items.length >= TRANSCRIPT_VIRTUAL_THRESHOLD;
  const [renderScrollTop, setRenderScrollTop] = useState(0);
  const { size: viewportSize } = useElementSize<HTMLDivElement>(
    { width: 0, height: 720 },
    {
      ref: rootRef,
      enabled: virtualized,
      refreshKey: virtualized,
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ height }) => height > 0,
      isEqual: (current, next) => current.height === next.height,
    },
  );
  const [measurementVersion, setMeasurementVersion] = useState(0);
  const measuredHeightsRef = useRef(new Map<number, number>());
  const measuredNodesRef = useRef(new Map<number, HTMLElement>());
  const resizeObserverRef = useRef<ResizeObserver | null>(null);
  const scrollTopRef = useRef(0);
  const stickToBottomRef = useRef(false);
  const renderRangeRef = useRef({ start: 0, end: items.length });

  const offsets = useMemo(() => {
    const next = new Array<number>(items.length + 1).fill(0);
    for (let index = 0; index < items.length; index += 1) {
      next[index + 1] = next[index]
        + (measuredHeightsRef.current.get(index) ?? estimatedTranscriptItemHeight(items[index], items[index - 1]));
    }
    return next;
  }, [items, measurementVersion]);
  const offsetsRef = useRef(offsets);
  offsetsRef.current = offsets;
  const updateRenderRange = useCallback((nextScrollTop: number) => {
    const next = transcriptRangeForViewport(items.length, offsetsRef.current, nextScrollTop, viewportSize.height, virtualized);
    const current = renderRangeRef.current;
    if (current.start === next.start && current.end === next.end) return;
    renderRangeRef.current = next;
    setRenderScrollTop(nextScrollTop);
  }, [items.length, virtualized, viewportSize.height]);

  useLayoutEffect(() => {
    if (!stickToBottomRef.current) return;
    const root = rootRef.current;
    if (!root) return;
    const nextScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
    if (Math.abs(root.scrollTop - nextScrollTop) <= 1) return;
    root.scrollTop = nextScrollTop;
    scrollTopRef.current = nextScrollTop;
    updateRenderRange(nextScrollTop);
  }, [items.length, offsets, rootRef, updateRenderRange, viewportSize.height]);

  useEffect(() => {
    measuredHeightsRef.current.clear();
    scrollTopRef.current = 0;
    stickToBottomRef.current = false;
    renderRangeRef.current = { start: 0, end: items.length };
    setRenderScrollTop(0);
    if (rootRef.current) rootRef.current.scrollTop = 0;
    setMeasurementVersion((current) => current + 1);
  }, [resetKey, rootRef]);

  useEffect(() => {
    if (!virtualized) return undefined;
    const root = rootRef.current;
    if (!root) return undefined;
    let frame = 0;
    const updateScrollTop = () => {
      frame = 0;
      const nextScrollTop = root.scrollTop;
      scrollTopRef.current = nextScrollTop;
      if (Math.abs(nextScrollTop - Math.max(0, root.scrollHeight - root.clientHeight)) > 2) {
        stickToBottomRef.current = false;
      }
      updateRenderRange(nextScrollTop);
    };
    const onScroll = () => {
      if (frame === 0) frame = window.requestAnimationFrame(updateScrollTop);
    };
    updateScrollTop();
    root.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      root.removeEventListener("scroll", onScroll);
      if (frame !== 0) window.cancelAnimationFrame(frame);
    };
  }, [rootRef, updateRenderRange, virtualized]);

  useEffect(() => {
    if (!virtualized || typeof ResizeObserver === "undefined") return undefined;
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
          ?? (items[index] ? estimatedTranscriptItemHeight(items[index], items[index - 1]) : TRANSCRIPT_DEFAULT_ITEM_HEIGHT);
        if (Math.abs(previousHeight - nextHeight) < 1) continue;
        measuredHeightsRef.current.set(index, nextHeight);
        changed = true;
        if (layout[index] < scrollTopRef.current) anchorDelta += nextHeight - previousHeight;
      }
      if (!changed) return;
      const root = rootRef.current;
      if (root && anchorDelta !== 0) root.scrollTop += anchorDelta;
      setMeasurementVersion((current) => current + 1);
    });
    resizeObserverRef.current = observer;
    for (const node of measuredNodesRef.current.values()) observer.observe(node);
    return () => {
      observer.disconnect();
      resizeObserverRef.current = null;
    };
  }, [items, rootRef, virtualized]);

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

  const range = useMemo(() => {
    return transcriptRangeForViewport(items.length, offsets, renderScrollTop, viewportSize.height, virtualized);
  }, [items.length, offsets, renderScrollTop, viewportSize.height, virtualized]);

  useEffect(() => {
    renderRangeRef.current = range;
  }, [range.start, range.end]);

  const scrollToIndex = useCallback((index: number, behavior: ScrollBehavior = "auto") => {
    const root = rootRef.current;
    if (!root || items.length === 0) return;
    stickToBottomRef.current = false;
    const safeIndex = Math.min(items.length - 1, Math.max(0, index));
    const top = offsetsRef.current[safeIndex] ?? 0;
    root.scrollTo({ top, behavior });
    scrollTopRef.current = top;
    renderRangeRef.current = transcriptRangeForViewport(items.length, offsetsRef.current, top, viewportSize.height, virtualized);
    setRenderScrollTop(top);
  }, [items.length, rootRef, viewportSize.height, virtualized]);

  const scrollToBottom = useCallback((behavior: ScrollBehavior = "auto") => {
    const root = rootRef.current;
    if (!root) return;
    stickToBottomRef.current = true;
    const top = Math.max(0, root.scrollHeight - root.clientHeight);
    root.scrollTo({ top, behavior });
    scrollTopRef.current = top;
    updateRenderRange(top);
    setRenderScrollTop(top);
  }, [rootRef, updateRenderRange]);

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

function linkSkillName(link: SkillLinkRecord) {
  return link.skill_name ?? link.skillName ?? "";
}


function linkEvidenceText(link: SkillLinkRecord) {
  return `${link.evidence_text ?? link.evidenceText ?? ""}`.trim();
}


function linkEvidenceTime(link: SkillLinkRecord) {
  return `${link.evidence_time ?? link.evidenceTime ?? ""}`.trim();
}


function transcriptItemKey(prefix: string, index: string | number) {
  return `${prefix}-${index}`;
}


function transcriptItemText(item: TranscriptItemRecord) {
  return [item.body, item.command, item.result, item.tag, item.time]
    .map((value) => `${value ?? ""}`)
    .join("\n")
    .toLowerCase();
}


function evidenceMatchesItem(item: TranscriptItemRecord, evidenceText: string, evidenceTime: string) {
  const itemTime = `${item.time ?? ""}`.trim();
  if (evidenceTime && itemTime === evidenceTime) return true;
  const needle = evidenceText.toLowerCase();
  const command = `${item.command || item.body || ""}`.trim().toLowerCase();
  if (!needle) return false;
  return transcriptItemText(item).includes(needle) || (command && needle.includes(command));
}


function findSkillEvidenceTarget(transcriptItems: TranscriptItemRecord[], link: SkillLinkRecord): SkillEvidenceTarget | null {
  const evidenceText = linkEvidenceText(link);
  const evidenceTime = linkEvidenceTime(link);
  for (let index = 0; index < transcriptItems.length; index += 1) {
    const item = transcriptItems[index];
    if (transcriptItemType(item) === "toolGroup") {
      const tools = item.tools ?? [];
      for (let toolIndex = 0; toolIndex < tools.length; toolIndex += 1) {
        if (evidenceMatchesItem(tools[toolIndex], evidenceText, evidenceTime)) {
          return {
            key: transcriptItemKey("tool", `${index}-${toolIndex}`),
            groupKey: transcriptItemKey("tool-group", index),
            index,
          };
        }
      }
      continue;
    }
    if (evidenceMatchesItem(item, evidenceText, evidenceTime)) {
      return { key: transcriptItemKey(transcriptItemType(item) || "item", index), index };
    }
  }
  return null;
}

function findTranscriptSearchTargets(
  transcriptItems: TranscriptItemRecord[],
  query: string,
  scopes: TranscriptSearchScopeState,
): TranscriptSearchTarget[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];

  const matches: TranscriptSearchTarget[] = [];
  transcriptItems.forEach((item, index) => {
    const type = transcriptItemType(item);
    if (!scopes[transcriptSearchScope(type)]) return;

    if (type === "toolGroup") {
      (item.tools ?? []).forEach((tool, toolIndex) => {
        if (transcriptItemText(tool).includes(needle)) {
          matches.push({
            key: transcriptItemKey("tool", `${index}-${toolIndex}`),
            groupKey: transcriptItemKey("tool-group", index),
            index,
          });
        }
      });
      return;
    }
    if (transcriptItemText(item).includes(needle)) {
      matches.push({ key: transcriptItemKey(type || "item", index), index });
    }
  });
  return matches;
}

function transcriptSearchTargetForHit(
  transcriptItems: TranscriptItemRecord[],
  hit: TranscriptSearchHit,
): TranscriptSearchTarget | null {
  const item = transcriptItems[hit.groupIndex];
  if (!item) return null;
  const type = transcriptItemType(item);
  if (type === "toolGroup" && hit.toolIndex !== undefined && item.tools?.[hit.toolIndex]) {
    return {
      key: transcriptItemKey("tool", `${hit.groupIndex}-${hit.toolIndex}`),
      groupKey: transcriptItemKey("tool-group", hit.groupIndex),
      index: hit.groupIndex,
    };
  }
  return {
    key: transcriptItemKey(type || "item", hit.groupIndex),
    index: hit.groupIndex,
  };
}

function transcriptItemSearchQuery(
  item: TranscriptItemRecord,
  query: string,
  scopes: TranscriptSearchScopeState,
): string {
  if (!query || !scopes[transcriptSearchScope(transcriptItemType(item))]) return "";
  return query;
}

function buildSessionLocatorItems(transcriptItems: TranscriptItemRecord[]): SessionLocatorItem[] {
  const locatorItems: SessionLocatorItem[] = [];
  let pendingResponse: SessionLocatorItem | null = null;
  for (let index = 0; index < transcriptItems.length; index += 1) {
    const item = transcriptItems[index];
    const type = transcriptItemType(item);
    if (type === "user") {
      pendingResponse = {
        key: transcriptItemKey("user", index),
        index,
        label: `${item.body ?? ""}`.trim(),
        response: "",
      };
      locatorItems.push(pendingResponse);
    } else if (type === "assistant" && pendingResponse) {
      pendingResponse.response = `${item.body ?? ""}`.trim();
      pendingResponse = null;
    }
  }
  return locatorItems;
}

function buildSessionLocatorItemsFromMetadata(
  locatorMetadata: TranscriptLocatorItem[],
  indexOffset: number,
): SessionLocatorItem[] {
  return locatorMetadata.map((item) => {
    const index = item.index + indexOffset;
    return {
      key: transcriptItemKey("user", index),
      index,
      label: item.label,
      response: item.response,
    };
  });
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
  parentSession,
  childSessions,
  sessionTree,
  items,
  locatorMetadata,
  sessionSearchQuery,
  loading,
  hasMore,
  loadingMore,
  skillLinks,
  loadingSkillLinks,
  skillLinksLoaded,
  skillLinksError,
  onCollapse,
  onResume,
  resumeTarget,
  resumeState,
  onOpenSession,
  onOpenSkill,
  onLoadSkills,
  onLoadMore,
  onLoadAll,
  onReportError,
  searchTranscript,
  onSavePrompt,
  showSessionLocator,
  onLocateSession,
  keyboardNavigationScopeRef,
}: {
  session: SessionRecord;
  parentSession?: SessionRecord;
  childSessions: SessionRecord[];
  sessionTree: SessionRecord[];
  items: TranscriptItemRecord[];
  locatorMetadata?: TranscriptLocatorItem[];
  sessionSearchQuery?: string;
  loading: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  skillLinks: SkillLinkRecord[];
  loadingSkillLinks: boolean;
  skillLinksLoaded: boolean;
  skillLinksError?: string;
  onCollapse: () => void;
  onResume: (session: SessionRecord) => void;
  resumeTarget: SessionResumeTarget;
  resumeState: SessionResumeState;
  onOpenSession: (session: SessionRecord) => void;
  onOpenSkill?: (skillName: string) => void;
  onLoadSkills?: () => void;
  onLoadMore: () => Promise<TranscriptItemRecord[] | null>;
  onLoadAll: () => Promise<void>;
  onReportError?: (message: string) => void;
  searchTranscript?: (session: SessionRecord, query: string, scopes: TranscriptSearchScopes) => Promise<TranscriptSearchResult | null>;
  onSavePrompt?: (body: string) => Promise<boolean>;
  showSessionLocator: boolean;
  onLocateSession: () => void;
  keyboardNavigationScopeRef: KeyboardNavigationScopeRef;
}) {
  const transcriptItems = useMemo(() => {
    const grouped = groupTranscriptItems(items) as TranscriptItemRecord[];
    if (`${session.agent ?? ""}`.toLowerCase() !== "cursor") return grouped;
    const config = {
      model: session.model || undefined,
      mode: session.mode,
    };
    if (!config.model && !config.mode) return grouped;
    const configIndex = grouped.findIndex((item) => transcriptItemType(item) === "model_config");
    if (configIndex < 0) {
      return [{ type: "model_config", kind: "model_config", body: "", ...config }, ...grouped];
    }
    return grouped.map((item, index) => index === configIndex
      ? { ...item, ...config, model: item.model || config.model }
      : item);
  }, [items, session]);
  const syntheticCursorConfig = `${session.agent ?? ""}`.toLowerCase() === "cursor"
    && Boolean(session.model || session.mode)
    && !items.some((item) => transcriptItemType(item) === "model_config");
  const locatorItems = useMemo(() => locatorMetadata
    ? buildSessionLocatorItemsFromMetadata(locatorMetadata, syntheticCursorConfig ? 1 : 0)
    : buildSessionLocatorItems(transcriptItems), [locatorMetadata, syntheticCursorConfig, transcriptItems]);
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
    `${session.agent ?? ""}:${session.id}:${session.path ?? ""}`,
  );
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const [highlightedKey, setHighlightedKey] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [debouncedSearchQuery, setDebouncedSearchQuery] = useState("");
  const [searchScopes, setSearchScopes] = useState<TranscriptSearchScopeState>(DEFAULT_TRANSCRIPT_SEARCH_SCOPES);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchIndex, setSearchIndex] = useState(0);
  const [searchLoading, setSearchLoading] = useState(false);
  const [searchReadyQuery, setSearchReadyQuery] = useState("");
  const [searchResult, setSearchResult] = useState<TranscriptSearchResult | null>(null);
  const [searchError, setSearchError] = useState(false);
  const [jumpingToBottom, setJumpingToBottom] = useState(false);
  const [pendingTranscriptBottom, setPendingTranscriptBottom] = useState(false);
  const [transcriptScrollEdges, setTranscriptScrollEdges] = useState({ atTop: true, atBottom: true });
  const [pendingUserMessageTarget, setPendingUserMessageTarget] = useState<{ key: string; index: number; sessionKey: string } | null>(null);
  const [transcriptFocusRequest, setTranscriptFocusRequest] = useState<{
    target: TranscriptSearchTarget;
    preferSearchMatch: boolean;
    behavior: ScrollBehavior;
  } | null>(null);
  const highlightTimerRef = useRef(0);
  const transcriptNavigationKeyRef = useRef("");
  const transcriptNavigationPromiseRef = useRef(Promise.resolve());
  const transcriptNavigationSessionKeyRef = useRef("");
  const transcriptNavigationSessionKey = `${session.agent ?? ""}:${session.id}:${session.path ?? ""}`;
  transcriptNavigationSessionKeyRef.current = transcriptNavigationSessionKey;
  const transcriptItemsRef = useRef(transcriptItems);
  transcriptItemsRef.current = transcriptItems;
  const normalizedSessionSearchQuery = `${sessionSearchQuery ?? ""}`.trim().toLowerCase();
  useEffect(() => {
    window.clearTimeout(highlightTimerRef.current);
    setHighlightedKey("");
    setSearchQuery(normalizedSessionSearchQuery);
    setDebouncedSearchQuery(normalizedSessionSearchQuery);
    setSearchOpen(Boolean(normalizedSessionSearchQuery));
    setSearchLoading(false);
    setSearchReadyQuery("");
    setSearchResult(null);
    setSearchError(false);
    setTranscriptFocusRequest(null);
  }, [normalizedSessionSearchQuery, transcriptNavigationSessionKey]);
  const normalizedInputSearchQuery = searchQuery.trim().toLowerCase();
  const normalizedSearchQuery = debouncedSearchQuery;
  const searchReady = !normalizedSearchQuery || searchReadyQuery === normalizedSearchQuery;
  const remoteSearchActive = Boolean(
    searchTranscript
      && session.agent !== IMPORTED_SESSION_AGENT
      && session.path,
  );
  const searchTargets = useMemo(
    () => searchReady && !remoteSearchActive
      ? findTranscriptSearchTargets(transcriptItems, normalizedSearchQuery, searchScopes)
      : [],
    [normalizedSearchQuery, remoteSearchActive, searchReady, searchScopes, transcriptItems],
  );
  const searchResultCount = remoteSearchActive ? (searchResult?.hits.length ?? 0) : searchTargets.length;
  const clearMessageSearch = useCallback(() => {
    setSearchQuery("");
    setDebouncedSearchQuery("");
    setSearchScopes(DEFAULT_TRANSCRIPT_SEARCH_SCOPES);
    setSearchOpen(false);
  }, []);
  const setSearchScope = useCallback((scope: TranscriptSearchScope, checked: boolean) => {
    setSearchScopes((current) => {
      if (!checked && TRANSCRIPT_SEARCH_SCOPES.every((item) => item.id === scope || !current[item.id])) {
        return current;
      }
      return { ...current, [scope]: checked };
    });
  }, []);
  useEffect(() => () => window.clearTimeout(highlightTimerRef.current), []);
  useEffect(() => {
    if (!normalizedInputSearchQuery) {
      setDebouncedSearchQuery("");
      return;
    }
    const timer = window.setTimeout(() => {
      setDebouncedSearchQuery(normalizedInputSearchQuery);
    }, SESSION_SEARCH_DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [normalizedInputSearchQuery]);
  useEffect(() => {
    if (!remoteSearchActive) return;
    if (!normalizedSearchQuery) {
      setSearchLoading(false);
      setSearchReadyQuery("");
      setSearchResult(null);
      setSearchError(false);
      return;
    }
    let cancelled = false;
    setSearchLoading(true);
    setSearchReadyQuery("");
    setSearchResult(null);
    setSearchError(false);
    void searchTranscript?.(session, normalizedSearchQuery, searchScopes).then(
      (result) => {
        if (cancelled) return;
        setSearchLoading(false);
        setSearchReadyQuery(normalizedSearchQuery);
        if (result) setSearchResult(result);
        else {
          setSearchError(true);
          onReportError?.("Could not search messages in this session. Try again.");
        }
      },
      () => {
        if (cancelled) return;
        setSearchLoading(false);
        setSearchReadyQuery(normalizedSearchQuery);
        setSearchError(true);
        onReportError?.("Could not search messages in this session. Try again.");
      },
    );
    return () => {
      cancelled = true;
    };
  }, [normalizedSearchQuery, onReportError, remoteSearchActive, searchScopes, searchTranscript, session]);
  useEffect(() => {
    if (remoteSearchActive) return;
    if (!normalizedSearchQuery) {
      setSearchLoading(false);
      setSearchReadyQuery("");
      setSearchResult(null);
      setSearchError(false);
      return;
    }
    let cancelled = false;
    setSearchLoading(true);
    setSearchReadyQuery("");
    void onLoadAll().then(
      () => {
        if (cancelled) return;
        setSearchLoading(false);
        setSearchReadyQuery(normalizedSearchQuery);
      },
      () => {
        if (cancelled) return;
        setSearchLoading(false);
        setSearchReadyQuery(normalizedSearchQuery);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [hasMore, loading, normalizedSearchQuery, onLoadAll, remoteSearchActive]);
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
  }, [hasMore, loading, loadingMore, transcriptItems, updateTranscriptScrollEdges]);
  const focusTranscriptTarget = useCallback((target: TranscriptSearchTarget, preferSearchMatch = false, behavior: ScrollBehavior = "smooth") => {
    window.clearTimeout(highlightTimerRef.current);
    setHighlightedKey(target.key);
    const targetIndex = target.index ?? transcriptIndexFromKey(target.key);
    if (targetIndex !== null) scrollTranscriptToIndex(targetIndex);
    setTranscriptFocusRequest({ target, preferSearchMatch, behavior });
  }, [scrollTranscriptToIndex]);
  useLayoutEffect(() => {
    const request = transcriptFocusRequest;
    const root = transcriptRef.current;
    if (!request || !root) return;
    if (request.target.groupKey) {
      const group = root.querySelector(`[data-transcript-key="${cssEscape(request.target.groupKey)}"]`) as HTMLDetailsElement | null;
      if (!group) return;
      if (!group.open) group.open = true;
    }
    const node = root.querySelector(`[data-transcript-key="${cssEscape(request.target.key)}"]`);
    if (!node) return;
    const details = node.closest("details");
    if (details && !details.open) details.open = true;
    const destination = request.preferSearchMatch
      ? node.querySelector(".transcriptSearchMark") ?? node
      : node;
    setTranscriptFocusRequest(null);
    destination.scrollIntoView({ block: "center", behavior: request.behavior });
    highlightTimerRef.current = window.setTimeout(() => setHighlightedKey(""), 1800);
  }, [transcriptFocusRequest, transcriptItems, transcriptRenderRangeKey]);
  const ensureSearchHitLoaded = useCallback(async (hit: TranscriptSearchHit) => {
    let loadedItems = transcriptItemsRef.current;
    let previousLength = -1;
    while (groupTranscriptItems(loadedItems).length <= hit.groupIndex) {
      if (loadedItems.length === previousLength) break;
      previousLength = loadedItems.length;
      const nextItems = await onLoadMore();
      if (!nextItems) break;
      loadedItems = nextItems;
      transcriptItemsRef.current = nextItems;
    }
  }, [onLoadMore]);
  const ensureTranscriptIndexLoaded = useCallback(async (targetIndex: number) => {
    let loadedItems = transcriptItemsRef.current;
    let previousLength = -1;
    while (groupTranscriptItems(loadedItems).length <= targetIndex) {
      if (loadedItems.length === previousLength) break;
      previousLength = loadedItems.length;
      const nextItems = await onLoadMore();
      if (!nextItems) break;
      loadedItems = nextItems;
      transcriptItemsRef.current = nextItems;
    }
  }, [onLoadMore]);
  const scrollTranscriptToTop = useCallback(() => {
    const root = transcriptRef.current;
    if (!root) return;
    root.scrollTo({ top: 0, behavior: "smooth" });
  }, []);
  useLayoutEffect(() => {
    if (!pendingTranscriptBottom) return;
    scrollTranscriptToBottom();
    updateTranscriptScrollEdges();
    setPendingTranscriptBottom(false);
  }, [pendingTranscriptBottom, scrollTranscriptToBottom, transcriptItems, transcriptRenderRangeKey, updateTranscriptScrollEdges]);
  const jumpToBottom = useCallback(async () => {
    setJumpingToBottom(true);
    try {
      if (hasMore) await onLoadAll();
    } finally {
      setJumpingToBottom(false);
      setPendingTranscriptBottom(true);
    }
  }, [hasMore, onLoadAll]);
  const jumpToSkillEvidence = useCallback((link: SkillLinkRecord) => {
    const target = findSkillEvidenceTarget(transcriptItems, link);
    if (target) focusTranscriptTarget(target);
  }, [focusTranscriptTarget, transcriptItems]);
  const selectLocatorItem = useCallback((key: string, behavior?: ScrollBehavior) => {
    const index = transcriptIndexFromKey(key) ?? 0;
    keyboardNavigationScopeRef.current = "detail";
    transcriptNavigationKeyRef.current = key;
    void ensureTranscriptIndexLoaded(index).then(() => {
      focusTranscriptTarget({ key, index }, false, behavior);
    });
  }, [ensureTranscriptIndexLoaded, focusTranscriptTarget, keyboardNavigationScopeRef]);
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
  const moveUserMessage = useCallback((offset: number) => {
    if (locatorItems.length === 0) return;
    const currentKey = transcriptNavigationKeyRef.current;
    const currentIndex = locatorItems.findIndex((item) => item.key === currentKey);
    const baseIndex = currentIndex >= 0 ? currentIndex : visibleUserMessageIndex();
    const targetIndex = baseIndex < 0 ? (offset > 0 ? 0 : locatorItems.length - 1) : baseIndex + offset;
    if (targetIndex < 0 || targetIndex >= locatorItems.length) return;
    const target = locatorItems[targetIndex];
    const navigationSessionKey = transcriptNavigationSessionKeyRef.current;
    transcriptNavigationKeyRef.current = target.key;
    if (groupTranscriptItems(transcriptItemsRef.current).length > target.index) {
      focusTranscriptTarget({ key: target.key, index: target.index });
      return;
    }
    setPendingUserMessageTarget({ key: target.key, index: target.index, sessionKey: navigationSessionKey });
    const loadTarget = async () => {
      await ensureTranscriptIndexLoaded(target.index);
    };
    transcriptNavigationPromiseRef.current = transcriptNavigationPromiseRef.current.then(loadTarget, loadTarget);
  }, [ensureTranscriptIndexLoaded, focusTranscriptTarget, keyboardNavigationScopeRef, locatorItems, visibleUserMessageIndex]);
  useEffect(() => {
    const pending = pendingUserMessageTarget;
    if (!pending) return;
    if (
      keyboardNavigationScopeRef.current !== "detail"
      || pending.sessionKey !== transcriptNavigationSessionKey
    ) {
      setPendingUserMessageTarget(null);
      return;
    }
    if (groupTranscriptItems(transcriptItems).length <= pending.index) return;
    setPendingUserMessageTarget(null);
    focusTranscriptTarget({ key: pending.key, index: pending.index });
  }, [focusTranscriptTarget, keyboardNavigationScopeRef, pendingUserMessageTarget, transcriptItems, transcriptNavigationSessionKey]);
  useEffect(() => {
    if (!locatorItems.some((item) => item.key === transcriptNavigationKeyRef.current)) {
      transcriptNavigationKeyRef.current = "";
    }
  }, [locatorItems]);
  useEffect(() => {
    transcriptNavigationKeyRef.current = "";
    transcriptNavigationPromiseRef.current = Promise.resolve();
    setPendingUserMessageTarget(null);
    setTranscriptFocusRequest(null);
  }, [transcriptNavigationSessionKey]);
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
        keyboardNavigationScopeRef.current !== "detail"
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
  useEffect(() => {
    setSearchIndex(0);
  }, [normalizedSearchQuery, searchScopes, transcriptNavigationSessionKey]);
  useEffect(() => {
    if (!searchReady || searchLoading || searchError || searchResultCount === 0) return;
    if (remoteSearchActive) {
      const hit = searchResult?.hits[searchIndex];
      if (!hit) return;
      const visibleHit = syntheticCursorConfig
        ? { ...hit, groupIndex: hit.groupIndex + 1 }
        : hit;
      let cancelled = false;
      void ensureSearchHitLoaded(visibleHit).then(() => {
        if (cancelled) return;
        const grouped = groupTranscriptItems(transcriptItemsRef.current) as TranscriptItemRecord[];
        const target = transcriptSearchTargetForHit(grouped, visibleHit);
        if (target) focusTranscriptTarget(target, true);
      });
      return () => {
        cancelled = true;
      };
    }
    const target = searchTargets[searchIndex];
    if (target) focusTranscriptTarget(target, true);
  }, [ensureSearchHitLoaded, focusTranscriptTarget, remoteSearchActive, searchError, searchIndex, searchReady, searchResult, searchResultCount, searchLoading, searchTargets, syntheticCursorConfig, transcriptItems]);
  const moveSearchResult = useCallback((offset: number) => {
    if (searchResultCount === 0) return;
    setSearchIndex((current) => (current + offset + searchResultCount) % searchResultCount);
  }, [searchResultCount]);
  const openLinkedSession = useCallback((sessionId: string) => {
    const linkedSession = childSessions.find((child) => child.id === sessionId);
    if (linkedSession) onOpenSession(linkedSession);
  }, [childSessions, onOpenSession]);
  const targetLabel = resumeTargetLabel(sessionResumeTargetForAgent(resumeTarget, session.agent));
  return (
    <aside
      className="transcriptPanel"
      onPointerDownCapture={() => { keyboardNavigationScopeRef.current = "detail"; }}
      onFocusCapture={() => { keyboardNavigationScopeRef.current = "detail"; }}
    >
      <header className="threadHeader">
        <div className="threadTitleLine">
          <h2><SessionTitleText interactive={false} value={session.title} /></h2>
          <div className="threadHeaderActions">
            {showSessionLocator ? (
              <AppTooltip content="Locate session in list">
                <IconButton
                  type="button"
                  className="threadPanelToggle"
                  aria-label="Locate session in list"
                  onClick={onLocateSession}
                >
                  <LocateFixed size={15} aria-hidden="true" />
                </IconButton>
              </AppTooltip>
            ) : null}
            <AppTooltip content={resumeState === "loading"
                ? `Opening session in ${targetLabel}`
                : resumeState === "success"
                  ? `Session opened in ${targetLabel}`
                  : resumeState === "error"
                    ? `Could not open session in ${targetLabel}`
                    : `Resume session in ${targetLabel}`}>
              <StatefulButton
                size="sm"
                variant="ghost"
                className={`threadPanelToggle sessionResumeButton${resumeState === "success" ? " isSuccess" : resumeState === "error" ? " isError" : ""}`}
                width="30px"
                minWidth="30px"
                style={{ height: "30px", padding: 0, display: "grid", placeItems: "center", gap: 0 }}
                state={resumeState}
                aria-label={resumeState === "loading"
                  ? `Opening session in ${targetLabel}`
                  : resumeState === "success"
                    ? `Session opened in ${targetLabel}`
                    : resumeState === "error"
                      ? `Could not open session in ${targetLabel}`
                      : `Resume session in ${targetLabel}`}
                aria-busy={resumeState === "loading"}
                disabled={resumeState === "loading"}
                onClick={() => onResume(session)}
                loadingContent={<LoadingIcon size={15} />}
                successContent={<Check size={15} aria-hidden="true" />}
                errorContent={<AlertCircle size={15} aria-hidden="true" />}
              >
                {sessionResumeTargetForAgent(resumeTarget, session.agent) === "app"
                  ? <MessageSquareText size={15} aria-hidden="true" />
                  : <TerminalSquare size={15} aria-hidden="true" />}
              </StatefulButton>
            </AppTooltip>
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
              onJumpToEvidence={jumpToSkillEvidence}
            />
            <SessionInfoMenu session={session} />
            <IconButton className="threadPanelToggle" aria-label="Collapse session detail" onClick={onCollapse}><PanelRightClose size={16} /></IconButton>
          </div>
        </div>
        <div className="threadMeta">
          <span>{session.updatedDetailLabel || "-"}</span>
          <span>{session.messages} messages</span>
        </div>
        {searchOpen ? (
          <div className="transcriptSearch" role="search">
            <DropdownMenu.Root>
              <DropdownMenu.Trigger asChild>
                <button
                  type="button"
                  className="selectControlTrigger transcriptSearchScopeTrigger"
                  aria-label="Message search scope"
                >
                  <span>{transcriptSearchScopeLabel(searchScopes)}</span>
                  <ChevronDown size={12} />
                </button>
              </DropdownMenu.Trigger>
              <DropdownMenu.Portal>
                <DropdownMenu.Content
                  className="skillMenuContent transcriptSearchScopeMenu"
                  align="start"
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
                        className="skillMenuItem transcriptSearchScopeItem"
                        checked={active}
                        onCheckedChange={(checked) => setSearchScope(scope.id, checked === true)}
                        onSelect={(event) => event.preventDefault()}
                      >
                        <CheckboxIndicator checked={active} />
                        <span>{scope.label}</span>
                      </DropdownMenu.CheckboxItem>
                    );
                  })}
                </DropdownMenu.Content>
              </DropdownMenu.Portal>
            </DropdownMenu.Root>
            <Search size={14} aria-hidden="true" />
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
            {normalizedSearchQuery ? <span className="transcriptSearchCount">{searchLoading ? "…" : searchResultCount ? `${searchIndex + 1}/${searchResultCount}` : "0/0"}</span> : null}
            {normalizedSearchQuery ? (
              <>
                <button type="button" aria-label="Previous matching message" onClick={() => moveSearchResult(-1)} disabled={searchLoading || searchResultCount === 0}><ChevronLeft size={14} /></button>
                <button type="button" aria-label="Next matching message" onClick={() => moveSearchResult(1)} disabled={searchLoading || searchResultCount === 0}><ChevronRight size={14} /></button>
              </>
            ) : null}
            <button type="button" aria-label="Close message search" onClick={clearMessageSearch}><X size={13} /></button>
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
                const itemKey = transcriptItemKey(type === "toolGroup" ? "tool-group" : type || "item", index);
                const highlighted = transcriptItemIsHighlighted(item, itemKey, highlightedKey);
                return (
                  <div
                    className={`transcriptItemShell${highlighted ? " isHighlighted" : ""}`}
                    data-transcript-index={index}
                    ref={(node) => measureTranscriptItem(index, node)}
                    key={`${type}-${index}`}
                  >
                    <TranscriptItem
                      item={item}
                      itemKey={itemKey}
                      addTopSpacing={isChatTranscriptType(type) && !isChatTranscriptType(previousType)}
                      highlightedKey={highlightedKey}
                      searchQuery={transcriptItemSearchQuery(item, normalizedSearchQuery, searchScopes)}
                      onOpenLinkedSession={openLinkedSession}
                      onSavePrompt={onSavePrompt}
                    />
                  </div>
                );
              })}
              {transcriptBottomSpacerHeight > 0 ? <div className="transcriptVirtualSpacer" style={{ height: transcriptBottomSpacerHeight }} aria-hidden="true" /> : null}
              {hasMore ? (
                <>
                  {loadingMore ? <div className="sessionTranscriptLoadMore" aria-live="polite"><LoadingInline label="Loading more messages" /></div> : null}
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
            usageSource={hasReportedUsage ? "reported" : "estimated"}
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
  const { ref: locatorListRef, size: locatorSize } = useElementSize<HTMLDivElement>(
    { width: 0, height: 640 },
    {
      refreshKey: items.length,
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ height }) => height > 0,
      isEqual: (current, next) => current.height === next.height,
    },
  );
  const locatorScrollFrameRef = useRef(0);
  const [locatorScrollTop, setLocatorScrollTop] = useState(0);
  const dragRef = useRef<{ pointerId: number; key: string; moved: boolean } | null>(null);
  const suppressClickRef = useRef(false);
  const itemKeys = useMemo(() => items.map((item) => item.key).join("\0"), [items]);
  const itemKeySet = useMemo(() => new Set(itemKeys.split("\0").filter(Boolean)), [itemKeys]);
  const clearPreview = useCallback(() => setPreviewKey(""), []);
  const handleLocatorClick = useCallback((key: string) => {
    if (suppressClickRef.current) {
      suppressClickRef.current = false;
      return;
    }
    onSelect(key);
  }, [onSelect]);
  useEffect(() => () => {
    if (locatorScrollFrameRef.current !== 0) window.cancelAnimationFrame(locatorScrollFrameRef.current);
  }, []);
  const locatorStart = Math.max(0, Math.floor(locatorScrollTop / SESSION_LOCATOR_ROW_HEIGHT) - SESSION_LOCATOR_OVERSCAN);
  const locatorEnd = Math.min(
    items.length,
    Math.ceil((locatorScrollTop + locatorSize.height) / SESSION_LOCATOR_ROW_HEIGHT) + SESSION_LOCATOR_OVERSCAN,
  );

  useEffect(() => {
    if (loading || items.length < SESSION_LOCATOR_MIN_ITEMS) {
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
          onScroll={(event) => {
            if (locatorScrollFrameRef.current !== 0) return;
            const nextScrollTop = event.currentTarget.scrollTop;
            locatorScrollFrameRef.current = window.requestAnimationFrame(() => {
              locatorScrollFrameRef.current = 0;
              setLocatorScrollTop(nextScrollTop);
            });
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
            onSelect(key, "auto");
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
      open={previewOpen}
      content={(
        <>
          <strong><TranscriptLinkText interactive={false} value={formatTranscriptPreview(item.label) || "(No content)"} /></strong>
          {item.response ? <span><TranscriptLinkText interactive={false} value={formatTranscriptPreview(item.response)} /></span> : null}
        </>
      )}
      className="sessionLocatorPreview"
      unstyled
      side="right"
      align="center"
      sideOffset={-6}
      collisionPadding={8}
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
      <AppTooltip content={`${relationCount} related session${relationCount === 1 ? "" : "s"}`}>
        <Popover.Trigger asChild>
          <IconButton
            className="threadPanelToggle"
            aria-label={`Show ${relationCount} related session${relationCount === 1 ? "" : "s"}`}
          >
            <GitFork size={15} />
          </IconButton>
        </Popover.Trigger>
      </AppTooltip>
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
  onJumpToEvidence,
}: {
  session: SessionRecord;
  links?: SkillLinkRecord[];
  loading?: boolean;
  loaded?: boolean;
  error?: string;
  onLoad?: () => void;
  onOpenSkill?: (skillName: string) => void;
  onJumpToEvidence?: (link: SkillLinkRecord) => void;
}) {
  const handleOpenChange = (open: boolean) => {
    if (open) onLoad?.();
  };
  const disabledReason = loading
    ? "Loading skills used"
    : loaded && links.length === 0
      ? "No associated skills for this session"
      : !onLoad
        ? "Skill links unavailable"
        : "";

  if (disabledReason) {
    return (
      <AppTooltip content={disabledReason}><span className="sessionSkillsTriggerWrap">
        <IconButton className="threadPanelToggle" aria-label={disabledReason} disabled>
          <Sparkles size={15} />
        </IconButton>
      </span></AppTooltip>
    );
  }

  return (
    <Popover.Root onOpenChange={handleOpenChange}>
      <Popover.Trigger asChild>
        <IconButton className="threadPanelToggle" aria-label="Show skills used">
          <Sparkles size={15} />
        </IconButton>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className={`sessionSkillsPopover${links.length > 0 ? " hasChart" : ""}`}
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
            onOpenSkill={onOpenSkill}
            onJumpToEvidence={onJumpToEvidence}
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
  const currentName = `session:${sessionKey(session)}`;
  const relatedSessions = new Map<string, SessionRecord>();
  const treeByKey = new Map(sessionTree.map((treeSession) => [sessionKey(treeSession).toLowerCase(), treeSession]));
  const nodes = sessionTree.map((treeSession) => {
    const name = `session:${sessionKey(treeSession)}`;
    if (name !== currentName) relatedSessions.set(name, treeSession);
    return {
      name,
      label: formatSessionTitle(treeSession.title) || treeSession.id || "Untitled session",
      kind: sessionKind(treeSession) === "child" ? "session-child" : "session-parent",
    };
  });
  const edges: Array<{ from: string; to: string }> = [];
  for (const treeSession of sessionTree) {
    if (!treeSession.parentSessionId) continue;
    const parentKey = `${friendlyAgent(treeSession.agent)}:${treeSession.parentSessionId}`.toLowerCase();
    const parent = treeByKey.get(parentKey);
    if (parent) {
      edges.push({
        from: `session:${sessionKey(parent)}`,
        to: `session:${sessionKey(treeSession)}`,
      });
    }
  }

  return (
    <SkillRelationshipMap
      nodes={nodes}
      edges={edges}
      focusName={currentName}
      compact
      onOpenSkill={(name) => {
        const relatedSession = relatedSessions.get(name);
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
  links: SkillLinkRecord[];
  onOpenSkill?: (skillName: string) => void;
}) {
  const currentName = `session:${sessionKey(session)}`;
  const skillsByName = new Map<string, string>();
  for (const link of links) {
    const name = linkSkillName(link);
    if (name && !skillsByName.has(name)) skillsByName.set(name, `skill:${name}`);
  }
  const nodes = [
    { name: currentName, label: formatSessionTitle(session.title) || session.id || "Session", kind: sessionKind(session) === "child" ? "session-child" : "session-parent" },
    ...[...skillsByName.entries()].map(([label, name]) => ({ name, label, kind: "skill-used" })),
  ];
  const edges = [...skillsByName.values()].map((name) => ({ from: currentName, to: name }));
  return (
    <SkillRelationshipMap
      nodes={nodes}
      edges={edges}
      focusName={currentName}
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
  onOpenSkill,
  onJumpToEvidence,
}: {
  session: SessionRecord;
  links?: SkillLinkRecord[];
  loading?: boolean;
  error?: string;
  onOpenSkill?: (skillName: string) => void;
  onJumpToEvidence?: (link: SkillLinkRecord) => void;
}) {
  return (
    <section className="sessionSkillsUsed">
      <div className="sessionSkillsHeader">
        <span>Skills Used</span>
      </div>
      {loading ? (
        <div className="sessionSkillsEmpty"><LoadingInline label="Loading skills" /></div>
      ) : error ? null : links.length > 0 ? (
        <>
          <SessionSkillsConvergence session={session} links={links} onOpenSkill={onOpenSkill} />
          <div className="sessionSkillChips">
            {links.map((link) => (
              <div
                className="sessionSkillChip"
                key={`${link.skill_path ?? link.skillPath ?? linkSkillName(link)}`}
              >
                <AppTooltip content={linkEvidenceText(link)} onlyWhenTruncated><span>{linkSkillName(link)}</span></AppTooltip>
                <div className="sessionSkillChipActions">
                  <button
                    type="button"
                    aria-label={`Open ${linkSkillName(link)} skill`}
                    onClick={() => onOpenSkill?.(linkSkillName(link))}
                  >
                    <Sparkles size={12} />
                  </button>
                  <button
                    type="button"
                    aria-label={`Go to ${linkSkillName(link)} usage in transcript`}
                    onClick={() => onJumpToEvidence?.(link)}
                  >
                    <MessageSquareText size={12} />
                  </button>
                </div>
              </div>
            ))}
          </div>
        </>
      ) : (
        <div className="sessionSkillsEmpty">No observed skill reads</div>
      )}
    </section>
  );
}


export function SessionInfoMenu({ session }: { session: SessionRecord }) {
  const sessionId = `${session.id ?? ""}`;
  const transcriptPath = `${session.path ?? ""}`;
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
      title={<SessionTitleText interactive={false} value={session.title} />}
      contentClassName="sessionInfoContent"
    >
            <InfoSection label="Session ID">
                <CopyableSessionId sessionId={sessionId} className="inInfoMenu" />
            </InfoSection>
            <InfoSection label="Agent">
                <AgentBadge agent={friendlyAgent(session.agent)} small />
                <span className="sessionInfoAgentValue">{friendlyAgent(session.agent)}</span>
            </InfoSection>
            <InfoSection label="Workspace">
                <AppTooltip content={displayWorkspace} onlyWhenTruncated><code>{displayWorkspace}</code></AppTooltip>
                {hasWorkspacePath && (
                  <button
                    aria-label="Reveal workspace in Finder"
                    className="appButton appButton-icon"
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path: workspacePath })}
                  >
                    <FolderOpen size={13} />
                  </button>
                )}
                <CopyButton className="appButton appButton-icon" value={workspace} copyLabel="Copy workspace" copiedLabel="Workspace copied" />
            </InfoSection>
            <InfoSection label="Timeline" valueLine={false}>
              <div className="sessionTimeline">
                <div className="sessionTimelineItem">
                  <span className="sessionTimelineDot" aria-hidden="true" />
                  <div className="sessionTimelineText">
                    <strong>Started</strong>
                    <code>{compactDateTime(session.startedAt, { year: true }) || "-"}</code>
                  </div>
                </div>
                <div className="sessionTimelineItem">
                  <span className="sessionTimelineDot" aria-hidden="true" />
                  <div className="sessionTimelineText">
                    <strong>Updated</strong>
                    <code>{session.updatedDetailLabel || "-"}</code>
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
              <InfoSection label="Transcript">
                  <AppTooltip content={displayTranscriptPath} onlyWhenTruncated><code>{displayTranscriptPath}</code></AppTooltip>
                  <button
                    aria-label="Reveal transcript in Finder"
                    className="appButton appButton-icon"
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path: transcriptPath })}
                  >
                    <FolderOpen size={13} />
                  </button>
                  <CopyButton className="appButton appButton-icon" value={transcriptPath} copyLabel="Copy transcript path" copiedLabel="Transcript path copied" />
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
  onOpenLinkedSession?: (sessionId: string) => void;
  onSavePrompt?: (body: string) => Promise<boolean>;
};

function transcriptItemIsHighlighted(item: TranscriptItemRecord, itemKey: string, highlightedKey: string) {
  const type = transcriptItemType(item);
  if (type !== "toolGroup") return highlightedKey === itemKey;
  const groupIndex = itemKey.slice("tool-group-".length);
  return highlightedKey === itemKey
    || highlightedKey.startsWith(`tool-${groupIndex}-`);
}

function transcriptHighlightState(props: TranscriptItemProps) {
  return transcriptItemIsHighlighted(props.item, props.itemKey, props.highlightedKey);
}

type SavePromptButtonState = "idle" | "loading" | "success" | "error";

function MessageSavePromptButton({ body, onSave }: { body: string; onSave: (body: string) => Promise<boolean> }) {
  const [state, setState] = useState<SavePromptButtonState>("idle");
  const resetTimerRef = useRef<number | undefined>(undefined);
  const clearResetTimer = useCallback(() => {
    if (resetTimerRef.current !== undefined) {
      window.clearTimeout(resetTimerRef.current);
      resetTimerRef.current = undefined;
    }
  }, []);
  useEffect(() => clearResetTimer, [clearResetTimer]);
  const save = useCallback(async () => {
    if (state === "loading" || state === "success") return;
    clearResetTimer();
    setState("loading");
    let saved = false;
    try {
      saved = await onSave(body);
    } catch {
      saved = false;
    }
    setState(saved ? "success" : "error");
    resetTimerRef.current = window.setTimeout(() => {
      setState("idle");
      resetTimerRef.current = undefined;
    }, saved ? 1600 : 2200);
  }, [body, clearResetTimer, onSave, state]);
  return (
    <StatefulButton
      state={state}
      size="sm"
      width={20}
      minWidth={20}
      variant="ghost"
      className="messageActionButton messageSavePromptButton"
      aria-label="Save as prompt"
      disabled={state === "success"}
      onClick={() => { void save(); }}
      loadingLabel="Saving prompt"
      successLabel="Prompt saved"
      errorLabel="Could not save prompt"
      loadingContent={<LoadingIcon size={14} />}
      successContent={<Check size={14} aria-hidden="true" />}
      errorContent={<AlertCircle size={14} aria-hidden="true" />}
      style={{ height: "20px", padding: 0, display: "grid", placeItems: "center", gap: 0 }}
    >
      <MessageSquarePlus size={14} aria-hidden="true" />
    </StatefulButton>
  );
}

export const TranscriptItem = memo(function TranscriptItem({
  item,
  itemKey,
  addTopSpacing,
  highlightedKey,
  searchQuery,
  onOpenLinkedSession,
  onSavePrompt,
}: TranscriptItemProps) {
  const type = transcriptItemType(item);
  const highlighted = highlightedKey === itemKey;
  if (type === "toolGroup") return <ToolCallGroup tools={item.tools ?? []} itemKey={itemKey} highlightedKey={highlightedKey} searchQuery={searchQuery} onOpenLinkedSession={onOpenLinkedSession} />;
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
  const body = `${item.body ?? ""}`;
  const copyable = type === "user" || type === "assistant";
  return (
    <div className={`chatLine ${isUser ? "fromUser" : "fromAgent"} ${addTopSpacing ? "withTopSpacing" : ""} ${highlighted ? "transcriptTarget" : ""}`} data-transcript-key={itemKey}>
      <div className="chatMessage">
        <div className="bubble">
          <p><TranscriptLinkText query={searchQuery} value={body} /></p>
        </div>
        <div className="bubbleFooter">
          {item.time?.trim() ? <time>{item.time}</time> : <span />}
          {copyable ? (
            <CopyButton
              className="messageActionButton"
              value={body}
              copyLabel={isUser ? "Copy user message" : "Copy model message"}
              copiedLabel="Message copied"
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
      {item.model ? <span className="modelConfigField"><span>Model</span><code>{item.model}</code></span> : null}
      {item.effort ? <span className="modelConfigField"><span>Effort</span><code>{item.effort}</code></span> : null}
      {item.mode ? <span className="modelConfigField"><span>Mode</span><code>{item.mode}</code></span> : null}
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
  const body = `${item.body ?? ""}`;
  const label = `${item.tag || "Context"}`;
  const contextKind = label === "Developer" || label === "System" ? label.toLowerCase() : "generic";
  const preview = body.split(/\r?\n/).find((line) => line.trim())?.trim() || label;
  return (
    <details
      className={`thinkingBlock contextBlock ${contextKind} ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="thinkingSummary">
        <Badge tone="neutral">{label}</Badge>
        <AppTooltip content={preview} onlyWhenTruncated><span className="thinkingPreview">{highlightTranscriptText(preview, searchQuery)}</span></AppTooltip>
        {item.time ? <time>{item.time}</time> : null}
        <ChevronRightIcon className="toolCallChevron" size={14} />
      </summary>
      {open ? <div className="thinkingDetails">
        <pre>{highlightTranscriptText(body || "-", searchQuery)}</pre>
      </div> : null}
    </details>
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
  const body = `${item.body ?? ""}`;
  const preview = body.split(/\r?\n/).find((line) => line.trim())?.trim() || type;
  return (
    <details
      className={`thinkingBlock ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="thinkingSummary">
        <Badge tone="success">{type}</Badge>
        <AppTooltip content={preview} onlyWhenTruncated><span className="thinkingPreview">{highlightTranscriptText(preview, searchQuery)}</span></AppTooltip>
        {item.time ? <time>{item.time}</time> : null}
        <ChevronRightIcon className="toolCallChevron" size={14} />
      </summary>
      {open ? <div className="thinkingDetails">
        <pre>{highlightTranscriptText(body || "-", searchQuery)}</pre>
      </div> : null}
    </details>
  );
}


export function ToolCallGroup({
  tools,
  itemKey,
  highlightedKey,
  searchQuery,
  onOpenLinkedSession,
}: {
  tools: TranscriptItemRecord[];
  itemKey: string;
  highlightedKey: string;
  searchQuery: string;
  onOpenLinkedSession?: (sessionId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const totalDuration = tools.reduce((total, item) => {
    const value = Number(item.durationMs);
    return Number.isFinite(value) ? total + value : total;
  }, 0);
  const duration = totalDuration > 0 ? formatDuration(totalDuration) : "";
  return (
    <details
      className="toolCallGroup"
      data-transcript-key={itemKey}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="toolCallGroupSummary">
        <span>已运行 {tools.length} 个命令</span>
        {duration ? <Badge tone="neutral" mono>{duration}</Badge> : null}
        <ChevronRightIcon className="toolCallChevron" size={14} />
      </summary>
      {open ? <div className="toolCallGroupDetails">
        {tools.map((tool, index) => {
          const groupIndex = `${itemKey ?? ""}`.split("-").pop();
          const toolKey = transcriptItemKey("tool", `${groupIndex}-${index}`);
          return (
            <ToolCall
              item={tool}
              itemKey={toolKey}
              highlighted={highlightedKey === toolKey}
              searchQuery={searchQuery}
              onOpenLinkedSession={onOpenLinkedSession}
              key={`${tool.command || tool.body || "tool"}-${index}`}
              nested
            />
          );
        })}
      </div> : null}
    </details>
  );
}


export function ToolCall({
  item,
  nested = false,
  itemKey,
  highlighted = false,
  searchQuery,
  onOpenLinkedSession,
}: {
  item: TranscriptItemRecord;
  nested?: boolean;
  itemKey: string;
  highlighted?: boolean;
  searchQuery: string;
  onOpenLinkedSession?: (sessionId: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const command = item.command || item.body || "";
  const result = item.result || "";
  const duration = formatDuration(item.durationMs);
  return (
    <details
      className={`toolCall ${nested ? "nested" : ""} ${highlighted ? "transcriptTarget" : ""}`}
      data-transcript-key={itemKey}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <summary className="toolCallSummary">
        {item.tag ? <Badge tone="neutral" mono>{item.tag}</Badge> : null}
        <AppTooltip content={command} onlyWhenTruncated><code>{highlightTranscriptText(command || "tool call", searchQuery)}</code></AppTooltip>
        {item.linkedSessionId && onOpenLinkedSession ? (
          <button
            type="button"
            className="toolCallSessionLink"
            aria-label="Open child session"
            onClick={(event) => {
              event.preventDefault();
              event.stopPropagation();
              onOpenLinkedSession(item.linkedSessionId!);
            }}
          >
            <ArrowUpRight size={13} />
          </button>
        ) : null}
        {duration ? <Badge tone="neutral" mono>{duration}</Badge> : null}
        <ChevronRightIcon className="toolCallChevron" size={14} />
      </summary>
      {open ? <div className="toolCallDetails">
        <div className="toolCallBlock">
          <div className="toolCallLabel">Command</div>
          <pre>{highlightTranscriptText(command || "-", searchQuery)}</pre>
        </div>
        <div className="toolCallBlock">
          <div className="toolCallLabel">Return</div>
          <pre>{highlightTranscriptText(result || "No output", searchQuery)}</pre>
        </div>
      </div> : null}
    </details>
  );
}


function sessionKey(session: SessionRecord | null | undefined) {
  if (!session) return "";
  return `${friendlyAgent(session.agent).toLowerCase()}:${session.id ?? ""}`;
}

function sessionTableRowId(session: SessionRecord) {
  return JSON.stringify([session.agent ?? "", session.id, session.path ?? ""]);
}

type GroupedSessionPage = {
  rows: SessionRecord[];
  start: number;
  end: number;
  groupCount: number;
};

type SessionProjectOption = {
  key: string;
  label: string;
  title: string;
  count: number;
};

type ProjectSearchRank = {
  distance: number;
  start: number;
  length: number;
};

function projectSearchRank(label: string, query: string): ProjectSearchRank | null {
  const value = [...label.toLowerCase()];
  const needle = [...query.toLowerCase()];
  if (needle.length === 0) return null;

  const required = new Map<string, number>();
  for (const character of needle) required.set(character, (required.get(character) ?? 0) + 1);

  const found = new Map<string, number>();
  let complete = 0;
  let left = 0;
  let best: ProjectSearchRank | null = null;
  for (let right = 0; right < value.length; right += 1) {
    const character = value[right];
    const requiredCount = required.get(character) ?? 0;
    if (requiredCount === 0) continue;
    const nextCount = (found.get(character) ?? 0) + 1;
    found.set(character, nextCount);
    if (nextCount <= requiredCount) complete += 1;

    while (complete === needle.length && left <= right) {
      const candidate = {
        distance: right - left + 1,
        start: left,
        length: value.length,
      } satisfies ProjectSearchRank;
      if (!best || candidate.distance < best.distance || (candidate.distance === best.distance && candidate.start < best.start)) {
        best = candidate;
      }

      const leftCharacter = value[left];
      const leftRequiredCount = required.get(leftCharacter) ?? 0;
      if (leftRequiredCount > 0) {
        const leftCount = (found.get(leftCharacter) ?? 0) - 1;
        found.set(leftCharacter, leftCount);
        if (leftCount < leftRequiredCount) complete -= 1;
      }
      left += 1;
    }
  }
  return best;
}

function sessionGroupKeyForPaging(session: SessionRecord, groupBy: string) {
  if (groupBy === "agent") return friendlyAgent(session.agent);
  if (groupBy === "project") return sessionProjectGroupKey(session);
  if (groupBy === "startedAt") return dayGroupKey(session.startedAt) || "Unknown";
  if (groupBy === "updatedAt") return dayGroupKey(session.updatedAt) || "Unknown";
  const value = session[groupBy as keyof SessionRecord];
  return `${value ?? "Unknown"}`;
}

function buildGroupedSessionPages(
  sessions: SessionRecord[],
  groupBy: string | null,
  pageSize: number,
): GroupedSessionPage[] {
  if (!groupBy) return [];

  const groups: SessionRecord[][] = [];
  const groupIndex = new Map<string, SessionRecord[]>();
  for (const session of sessions) {
    const key = sessionGroupKeyForPaging(session, groupBy);
    let group = groupIndex.get(key);
    if (!group) {
      group = [];
      groupIndex.set(key, group);
      groups.push(group);
    }
    group.push(session);
  }

  const pages: GroupedSessionPage[] = [];
  let pageRows: SessionRecord[] = [];
  let pageStart = 0;
  let pageGroupCount = 0;

  for (const group of groups) {
    if (pageRows.length > 0 && pageRows.length >= pageSize) {
      pages.push({
        rows: pageRows,
        start: pageStart,
        end: pageStart + pageRows.length,
        groupCount: pageGroupCount,
      });
      pageStart += pageRows.length;
      pageRows = [];
      pageGroupCount = 0;
    }
    pageRows.push(...group);
    pageGroupCount += 1;
  }

  if (pageRows.length > 0 || sessions.length === 0) {
    pages.push({
      rows: pageRows,
      start: pageStart,
      end: pageStart + pageRows.length,
      groupCount: pageGroupCount,
    });
  }

  return pages;
}


export function SessionsView({
  sessions: sessionItems,
  developerMode,
  loadTranscript,
  loadTranscriptLocator,
  searchTranscript,
  searchSessions,
  loadSessionSkillLinks,
  loadingSessions = false,
  sessionListError = "",
  sessionRefreshError = "",
  onRefreshSessions,
  onResumeSession,
  sessionResumeTarget,
  onOpenSkill,
  activeSessionKey,
  skillIndexStatus,
  onSavePrompt,
}: {
  sessions: SessionRecord[];
  developerMode: boolean;
  loadTranscript: (session: SessionRecord, cursor?: string, knownSourceVersion?: string) => Promise<TranscriptPage>;
  loadTranscriptLocator?: (session: SessionRecord) => Promise<TranscriptLocatorPage>;
  searchTranscript?: (session: SessionRecord, query: string, scopes: TranscriptSearchScopes) => Promise<TranscriptSearchResult | null>;
  searchSessions?: (query: string) => Promise<SessionRecord[]>;
  loadSessionSkillLinks?: (session: SessionRecord) => Promise<SkillLinkRecord[]>;
  loadingSessions?: boolean;
  sessionListError?: string;
  sessionRefreshError?: string;
  onRefreshSessions?: () => Promise<unknown>;
  onResumeSession?: (session: SessionRecord) => Promise<{ terminal?: string; target?: SessionResumeTarget } | null | undefined>;
  sessionResumeTarget: SessionResumeTarget;
  onOpenSkill?: (skillName: string) => void;
  activeSessionKey?: string;
  skillIndexStatus?: SkillIndexStatus | null;
  onSavePrompt?: (body: string) => Promise<boolean>;
}) {
  const [activeRowId, setActiveRowId] = useState(() => {
    const initialSession = resolveInitialSession(sessionItems, activeSessionKey);
    return initialSession ? sessionTableRowId(initialSession) : "";
  });
  const [importedSessions, setImportedSessions] = useState<SessionRecord[]>([]);
  const [importedTranscripts, setImportedTranscripts] = useState<Record<string, TranscriptItemRecord[]>>({});
  const [importFeedback, setImportFeedback] = useState<ImportFeedbackState>("idle");
  const [importError, setImportError] = useState("");
  const [query, setQuery] = useState("");
  const [sort, setSort] = useState<SortState>({ key: "updatedAt", direction: "desc" });
  const [searchSort, setSearchSort] = useState<SortState | null>(null);
  const [currentPage, setCurrentPage] = useState(0);
  const [pageSize, setPageSize] = useState(50);
  const [groupBy, setGroupBy] = useState<string | null>(null);
  const [showChildSessions, setShowChildSessions] = useState(false);
  const [items, setItems] = useState<TranscriptItemRecord[]>([]);
  const [transcriptLocatorState, setTranscriptLocatorState] = useState<{
    key: string;
    items: TranscriptLocatorItem[];
  } | null>(null);
  const [nextTranscriptCursor, setNextTranscriptCursor] = useState<string | undefined>();
  const [loadingMoreTranscript, setLoadingMoreTranscript] = useState(false);
  const [skillLinks, setSkillLinks] = useState<SkillLinkRecord[]>([]);
  const [skillLinksKey, setSkillLinksKey] = useState("");
  const [loadingSkillLinks, setLoadingSkillLinks] = useState(false);
  const [skillLinksError, setSkillLinksError] = useState("");
  const [loading, setLoading] = useState(false);
  const [searchingSessions, setSearchingSessions] = useState(false);
  const [searchRows, setSearchRows] = useState<SessionRecord[] | null>(null);
  const [searchRowsKey, setSearchRowsKey] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [refreshActionError, setRefreshActionError] = useState("");
  const [resumeFeedback, setResumeFeedback] = useState<Record<string, ResumeFeedbackState>>({});
  const [sessionToast, setSessionToast] = useState("");
  const [detailCollapsed, setDetailCollapsed] = useState(false);
  const [sessionLocatorRequest, setSessionLocatorRequest] = useState("");
  const [activeSessionInListViewport, setActiveSessionInListViewport] = useState<boolean | null>(null);
  const [selectedProjectKeys, setSelectedProjectKeys] = useState<string[]>([]);
  const [projectFilterQuery, setProjectFilterQuery] = useState("");
  const [projectFilterOpen, setProjectFilterOpen] = useState(false);
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const transcriptItemsRef = useRef(items);
  const nextTranscriptCursorRef = useRef(nextTranscriptCursor);
  transcriptItemsRef.current = items;
  nextTranscriptCursorRef.current = nextTranscriptCursor;
  const importInputRef = useRef<HTMLInputElement | null>(null);
  const sessionListBodyRef = useRef<HTMLDivElement | null>(null);
  const keyboardNavigationScopeRef = useRef<KeyboardNavigationScope>("list");
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "f") return;
      if (keyboardNavigationScopeRef.current !== "list") return;
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
  const preserveLocatedSessionPageRef = useRef(false);
  const importWorkerRef = useRef<{ worker: Worker; cancel: () => void } | null>(null);
  const projectFilterInputRef = useRef<HTMLInputElement | null>(null);
  const importFeedbackTimerRef = useRef<number | undefined>(undefined);
  const resumeFeedbackTimerRef = useRef<Record<string, number>>({});
  const sessionToastTimerRef = useRef<number | undefined>(undefined);
  const skillLinksRequestKeyRef = useRef("");
  const transcriptRequestAuthorityRef = useRef(createLatestRequestAuthority());
  const transcriptCacheRef = useRef(new Map<string, TranscriptPage>());
  const transcriptLocatorRequestAuthorityRef = useRef(createLatestRequestAuthority());
  const transcriptLocatorCacheRef = useRef(new Map<string, TranscriptLocatorPage>());
  const pendingTranscriptLocatorRef = useRef<{ key: string; session: SessionRecord } | null>(null);
  const transcriptSourceVersionRef = useRef("");
  const loadMoreTranscriptInFlightRef = useRef<{ key: string; promise: Promise<TranscriptItemRecord[] | null> } | null>(null);
  const loadAllTranscriptInFlightRef = useRef<{ key: string; promise: Promise<void> } | null>(null);
  const normalizedInputQuery = query.trim().toLowerCase();
  const normalizedQuery = debouncedQuery;
  const remoteSearchActive = Boolean(normalizedQuery && searchSessions);
  const clearImportFeedbackTimer = useCallback(() => {
    if (importFeedbackTimerRef.current !== undefined) {
      window.clearTimeout(importFeedbackTimerRef.current);
      importFeedbackTimerRef.current = undefined;
    }
  }, []);
  const finishImportFeedback = useCallback((state: Exclude<ImportFeedbackState, "idle" | "loading">) => {
    clearImportFeedbackTimer();
    setImportFeedback(state);
    if (state === "success") {
      importFeedbackTimerRef.current = window.setTimeout(() => {
        setImportFeedback("idle");
        importFeedbackTimerRef.current = undefined;
      }, 1600);
    }
  }, [clearImportFeedbackTimer]);
  const finishResumeFeedback = useCallback((sessionId: string, state: Exclude<SessionResumeState, "idle" | "loading">) => {
    const existingTimer = resumeFeedbackTimerRef.current[sessionId];
    if (existingTimer !== undefined) {
      window.clearTimeout(existingTimer);
    }
    setResumeFeedback((current) => ({ ...current, [sessionId]: state }));
    resumeFeedbackTimerRef.current[sessionId] = window.setTimeout(() => {
      setResumeFeedback((current) => {
        const next = { ...current };
        delete next[sessionId];
        return next;
      });
      delete resumeFeedbackTimerRef.current[sessionId];
    }, 1800);
  }, []);
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
  useEffect(() => () => {
    clearImportFeedbackTimer();
    dismissSessionError();
    importWorkerRef.current?.cancel();
    importWorkerRef.current = null;
    Object.values(resumeFeedbackTimerRef.current).forEach((timer) => window.clearTimeout(timer));
  }, [clearImportFeedbackTimer, dismissSessionError]);
  const listSessionItems = useMemo(() => [...importedSessions, ...sessionItems], [importedSessions, sessionItems]);
  const projectSourceSessionItems = useMemo(
    () => showChildSessions ? listSessionItems : listSessionItems.filter((session) => sessionKind(session) === "main"),
    [listSessionItems, showChildSessions],
  );
  const projectOptions = useMemo<SessionProjectOption[]>(() => {
    const options = new Map<string, SessionProjectOption>();
    for (const session of projectSourceSessionItems) {
      const key = sessionProjectGroupKey(session);
      const option = options.get(key);
      if (option) {
        option.count += 1;
      } else {
        options.set(key, {
          key,
          label: sessionProject(session),
          title: session.repositoryUrl || sessionWorkspace(session),
          count: 1,
        });
      }
    }
    return [...options.values()].sort((left, right) => left.label.localeCompare(right.label) || left.title.localeCompare(right.title));
  }, [projectSourceSessionItems]);
  const selectedProjectKeySet = useMemo(() => new Set(selectedProjectKeys), [selectedProjectKeys]);
  const visibleProjectOptions = useMemo(() => {
    const normalizedProjectFilterQuery = projectFilterQuery.trim().toLowerCase();
    const filtered = projectOptions.filter((option) => (
      selectedProjectKeySet.has(option.key)
      || !normalizedProjectFilterQuery
      || projectSearchRank(option.label, normalizedProjectFilterQuery) !== null
    ));
    return [...filtered].sort((left, right) => {
      const leftSelected = selectedProjectKeySet.has(left.key);
      const rightSelected = selectedProjectKeySet.has(right.key);
      if (leftSelected !== rightSelected) return leftSelected ? -1 : 1;

      return right.count - left.count || left.label.localeCompare(right.label) || left.title.localeCompare(right.title);
    });
  }, [projectFilterQuery, projectOptions, selectedProjectKeySet]);
  useEffect(() => {
    const availableKeys = new Set(projectOptions.map((option) => option.key));
    setSelectedProjectKeys((current) => {
      const next = current.filter((key) => availableKeys.has(key));
      return next.length === current.length ? current : next;
    });
  }, [projectOptions]);
  useEffect(() => {
    if (!projectFilterOpen) return;
    window.requestAnimationFrame(() => projectFilterInputRef.current?.focus());
  }, [projectFilterOpen]);
  const currentSearchRows = useMemo(
    () => normalizedQuery && searchSessions && searchRowsKey === normalizedQuery ? (searchRows ?? []) : [],
    [normalizedQuery, searchRows, searchRowsKey, searchSessions],
  );
  const allSessionItems = useMemo(() => {
    const merged = new Map(listSessionItems.map((session) => [sessionTableRowId(session), session]));
    for (const session of currentSearchRows) merged.set(sessionTableRowId(session), session);
    return [...merged.values()];
  }, [currentSearchRows, listSessionItems]);
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
    if (!normalizedQuery || !searchSessions) {
      setSearchRows(null);
      setSearchRowsKey("");
      setSearchingSessions(false);
      return;
    }

    let cancelled = false;
    setSearchingSessions(true);
    searchSessions(normalizedQuery).then((rows) => {
      if (cancelled) return;
      setSearchRows(rows);
      setSearchRowsKey(normalizedQuery);
    }).catch((error) => {
      if (!cancelled) {
        logger.warn("sessions search failed", { error });
        setSearchRows([]);
        setSearchRowsKey(normalizedQuery);
        showSessionError("Could not search sessions. Try again.");
      }
    }).finally(() => {
      if (!cancelled) setSearchingSessions(false);
    });
    return () => {
      cancelled = true;
    };
  }, [normalizedQuery, searchSessions, showSessionError]);
  useEffect(() => {
    if (sessionListError && sessionItems.length === 0) {
      showSessionError("Could not load sessions. Try again.");
    }
  }, [sessionItems.length, sessionListError, showSessionError]);
  const sessionMatchesQuery = useCallback((session: SessionRecord, currentQuery: string) => (
    [session.title, sessionProject(session), sessionWorkspace(session), session.agent, session.model, session.mode, session.approvalMode, session.isRunEverything, session.startedAt, session.updatedAt]
      .some((value) => `${value ?? ""}`.toLowerCase().includes(currentQuery))
  ), []);
  const matchedSessions = useMemo(() => {
    if (normalizedQuery && searchSessions) {
      const remoteRows = currentSearchRows;
      const matches = new Map<string, SessionRecord>();
      for (const session of listSessionItems) {
        if (sessionMatchesQuery(session, normalizedQuery)) {
          matches.set(sessionTableRowId(session), session);
        }
      }
      for (const session of remoteRows) matches.set(sessionTableRowId(session), session);
      return [...matches.values()].filter((session) => selectedProjectKeySet.size === 0 || selectedProjectKeySet.has(sessionProjectGroupKey(session)));
    }
    if (!normalizedQuery) return selectedProjectKeySet.size === 0
      ? listSessionItems
      : listSessionItems.filter((session) => selectedProjectKeySet.has(sessionProjectGroupKey(session)));
    return listSessionItems.filter((session) => (
      sessionMatchesQuery(session, normalizedQuery)
      && (selectedProjectKeySet.size === 0 || selectedProjectKeySet.has(sessionProjectGroupKey(session)))
    ));
  }, [currentSearchRows, listSessionItems, normalizedQuery, searchSessions, selectedProjectKeySet, sessionMatchesQuery]);
  const childSessionCount = useMemo(
    () => matchedSessions.filter((session) => sessionKind(session) === "child").length,
    [matchedSessions],
  );
  const filteredSessions = useMemo(
    () => showChildSessions
      ? matchedSessions
      : matchedSessions.filter((session) => sessionKind(session) === "main"),
    [matchedSessions, showChildSessions],
  );
  const activeSort = remoteSearchActive ? searchSort ?? sort : sort;
  const sortedSessions = useMemo(
    () => activeSort
      ? [...filteredSessions].sort((a, b) => compareSessions(a, b, activeSort))
      : filteredSessions,
    [activeSort, filteredSessions],
  );
  const groupedPages = useMemo(
    () => buildGroupedSessionPages(sortedSessions, groupBy, pageSize),
    [groupBy, pageSize, sortedSessions],
  );
  const pageCount = groupBy
    ? Math.max(1, groupedPages.length)
    : Math.max(1, Math.ceil(sortedSessions.length / pageSize));
  const groupedPage = groupedPages[Math.min(currentPage, Math.max(0, groupedPages.length - 1))];
  const pageStart = groupBy
    ? (groupedPage?.start ?? 0)
    : sortedSessions.length === 0 ? 0 : currentPage * pageSize;
  const pageEnd = groupBy
    ? (groupedPage?.end ?? 0)
    : Math.min(pageStart + pageSize, sortedSessions.length);
  const pagedSessions = useMemo(
    () => sortedSessions.slice(pageStart, pageEnd),
    [pageEnd, pageStart, sortedSessions],
  );
  const tableSessions = groupBy ? (groupedPage?.rows ?? []) : pagedSessions;
  const locateSessionInList = useCallback((session: SessionRecord) => {
    const targetRowId = sessionTableRowId(session);
    const showChildren = showChildSessions || sessionKind(session) === "child";
    const sessionsToShow = allSessionItems.filter((candidate) => showChildren || sessionKind(candidate) === "main");
    const sortedSessionsToShow = [...sessionsToShow].sort((left, right) => compareSessions(left, right, sort));
    const targetPage = groupBy
      ? buildGroupedSessionPages(sortedSessionsToShow, groupBy, pageSize)
        .findIndex((page) => page.rows.some((candidate) => sessionTableRowId(candidate) === targetRowId))
      : Math.floor(Math.max(0, sortedSessionsToShow.findIndex((candidate) => sessionTableRowId(candidate) === targetRowId)) / pageSize);

    keyboardNavigationScopeRef.current = "list";
    if (normalizedQuery || (showChildren && !showChildSessions)) {
      preserveLocatedSessionPageRef.current = true;
    }
    setQuery("");
    setDebouncedQuery("");
    setSearchRows(null);
    setSearchRowsKey("");
    setSearchSort(null);
    setSelectedProjectKeys([]);
    setProjectFilterQuery("");
    if (showChildren && !showChildSessions) setShowChildSessions(true);
    setCurrentPage(Math.max(0, targetPage));
    setActiveRowId(targetRowId);
    setSessionLocatorRequest(targetRowId);
  }, [allSessionItems, groupBy, normalizedQuery, pageSize, showChildSessions, sort]);
  const completeSessionLocator = useCallback((rowId: string) => {
    setSessionLocatorRequest((current) => current === rowId ? "" : current);
  }, []);
  const moveSession = useCallback((offset: number) => {
    if (sortedSessions.length === 0) return;
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
  }, [activeRowId, currentPage, groupBy, groupedPages, pageSize, sortedSessions]);
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (
        keyboardNavigationScopeRef.current !== "list"
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
        direction: next.key === "title" || next.key === "project" ? "asc" : "desc",
      } satisfies SortState;
    };
    if (remoteSearchActive) {
      setSearchSort(applySortChange);
      return;
    }
    setSort(applySortChange);
  }, [remoteSearchActive]);
  useEffect(() => {
    setSearchSort(null);
  }, [normalizedQuery]);
  useEffect(() => {
    if (preserveLocatedSessionPageRef.current) {
      preserveLocatedSessionPageRef.current = false;
      return;
    }
    setCurrentPage(0);
  }, [activeSort?.direction, activeSort?.key, groupBy, normalizedQuery, pageSize, showChildSessions]);
  useEffect(() => {
    if (currentPage >= pageCount) setCurrentPage(pageCount - 1);
  }, [currentPage, pageCount]);
  useEffect(() => {
    if (activeRowId && !allSessionItems.some((session) => sessionTableRowId(session) === activeRowId)) setActiveRowId("");
  }, [activeRowId, allSessionItems]);
  useEffect(() => {
    const normalizedKey = `${activeSessionKey ?? ""}`.toLowerCase();
    if (!normalizedKey) {
      handledExternalSessionKeyRef.current = "";
      return;
    }
    if (handledExternalSessionKeyRef.current === normalizedKey) return;
    const next = allSessionItems.find((session) => sessionKey(session) === normalizedKey);
    if (next) {
      handledExternalSessionKeyRef.current = normalizedKey;
      locateSessionInList(next);
      setDetailCollapsed(false);
    }
  }, [activeSessionKey, allSessionItems, locateSessionInList]);
  const openSession = useCallback((session: SessionRecord) => {
    keyboardNavigationScopeRef.current = "list";
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
      if (sessionItems.length === 0) showSessionError(SESSION_REFRESH_ERROR);
    } finally {
      setRefreshing(false);
    }
  }, [onRefreshSessions, refreshing, sessionItems.length, showSessionError]);
  const importJsonl = useCallback(async (file: File) => {
    clearImportFeedbackTimer();
    setImportFeedback("loading");
    setImportError("");
    try {
      const parsed = await parseImportedTranscript(file, importWorkerRef);
      if (parsed.parsedCount === 0) {
        setImportError("Could not import JSONL transcript");
        showSessionError("Could not import JSONL transcript. Check the file and try again.");
        finishImportFeedback("error");
        return;
      }
      if (parsed.items.length === 0) {
        setImportError("Could not import JSONL transcript");
        showSessionError("Could not import JSONL transcript. Check the file and try again.");
        finishImportFeedback("error");
        return;
      }
      const id = `import:${file.name}:${file.lastModified}:${Date.now()}`;
      const startedAt = parsed.startedAt ?? new Date(file.lastModified || Date.now()).toISOString();
      const updatedAt = parsed.updatedAt ?? startedAt;
      const userMessages = parsed.items
        .filter((item) => transcriptItemType(item) === "user")
        .map((item) => formatTranscriptPreview(item.body))
        .filter(Boolean);
      const assistantMessages = parsed.items
        .filter((item) => transcriptItemType(item) === "assistant")
        .map((item) => formatTranscriptPreview(item.body))
        .filter(Boolean);
      const session: SessionRecord = {
        id,
        title: formatTranscriptPreview(parsed.title) || file.name,
        project: parsed.project || "Imported JSONL",
        projectPath: parsed.project || "Imported JSONL",
        agent: IMPORTED_SESSION_AGENT,
        startedAt,
        updatedAt,
        time: updatedAt,
        startedLabel: compactDateTime(startedAt),
        updatedLabel: compactDateTime(updatedAt),
        updatedDetailLabel: compactDateTime(updatedAt, { year: true }),
        messages: parsed.items.length,
        firstUserMessage: userMessages[0],
        lastUserMessage: userMessages.at(-1),
        lastAssistantMessage: assistantMessages.at(-1),
        tokenUsage: parsed.tokenUsage,
      };
      setImportedTranscripts((current) => ({ ...current, [id]: parsed.items as TranscriptItemRecord[] }));
      setImportedSessions((current) => [session, ...current]);
      setActiveRowId(sessionTableRowId(session));
      setDetailCollapsed(false);
      setImportError(parsed.warnings.length ? `${parsed.warnings.length} invalid lines skipped` : "");
      finishImportFeedback(parsed.warnings.length ? "warning" : "success");
    } catch (error) {
      logger.warn("sessions transcript import failed", { error });
      setImportError("Could not import JSONL transcript");
      showSessionError("Could not import JSONL transcript. Check the file and try again.");
      finishImportFeedback("error");
    }
  }, [clearImportFeedbackTimer, finishImportFeedback, showSessionError]);
  const handleImportJsonlChange = useCallback((event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    importJsonl(file);
  }, [importJsonl]);
  const resumeSession = useCallback(async (session: SessionRecord) => {
    if (session.agent === IMPORTED_SESSION_AGENT) {
      finishResumeFeedback(session.id, "error");
      showSessionError("Imported sessions cannot be opened.");
      return;
    }
    if (!onResumeSession) {
      finishResumeFeedback(session.id, "error");
      showSessionError("Resume is unavailable.");
      return;
    }
    dismissSessionError();
    setResumeFeedback((current) => ({ ...current, [session.id]: "loading" }));
    try {
      const result = await onResumeSession(session);
      if (result?.terminal || result?.target) {
        finishResumeFeedback(session.id, "success");
      } else {
        finishResumeFeedback(session.id, "error");
        showSessionError("Could not open session. Try again.");
      }
    } catch (error) {
      finishResumeFeedback(session.id, "error");
        logger.warn("sessions resume failed", { error });
      showSessionError("Could not open session. Try again.");
    }
  }, [dismissSessionError, finishResumeFeedback, onResumeSession, showSessionError]);
  const getResumeState = useCallback((session: SessionRecord): SessionResumeState => (
    resumeFeedback[session.id] ?? "idle"
  ), [resumeFeedback]);
  const activeSession = useMemo(
    () => allSessionItems.find((session) => sessionTableRowId(session) === activeRowId),
    [activeRowId, allSessionItems],
  );
  const activeSessionVisibleInList = Boolean(
    activeSession && tableSessions.some((session) => sessionTableRowId(session) === sessionTableRowId(activeSession)),
  );
  useEffect(() => {
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
    scheduleMeasure();
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
  const activeSessionForTranscriptRef = useRef(activeSession);
  activeSessionForTranscriptRef.current = activeSession;
  const activeParentSession = useMemo(() => {
    if (!activeSession?.parentSessionId) return undefined;
    const parentKey = `${friendlyAgent(activeSession.agent)}:${activeSession.parentSessionId}`.toLowerCase();
    return allSessionItems.find((session) => sessionKey(session) === parentKey);
  }, [activeSession, allSessionItems]);
  const activeChildSessions = useMemo(() => {
    if (!activeSession) return [];
    const activeAgent = friendlyAgent(activeSession.agent).toLowerCase();
    const activeSessionId = activeSession.id.toLowerCase();
    return allSessionItems.filter((session) => (
      friendlyAgent(session.agent).toLowerCase() === activeAgent
      && session.parentSessionId?.toLowerCase() === activeSessionId
    ));
  }, [activeSession, allSessionItems]);
  const activeSessionTree = useMemo(() => {
    if (!activeSession) return [];
    const activeAgent = friendlyAgent(activeSession.agent).toLowerCase();
    const sameAgentSessions = allSessionItems.filter((session) => friendlyAgent(session.agent).toLowerCase() === activeAgent);
    const byId = new Map(sameAgentSessions.map((session) => [session.id.toLowerCase(), session]));
    let rootSession = activeSession;
    const visited = new Set<string>();
    while (rootSession.parentSessionId && !visited.has(rootSession.id.toLowerCase())) {
      visited.add(rootSession.id.toLowerCase());
      const parent = byId.get(rootSession.parentSessionId.toLowerCase());
      if (!parent) break;
      rootSession = parent;
    }

    const childrenByParent = new Map<string, SessionRecord[]>();
    for (const candidate of sameAgentSessions) {
      if (!candidate.parentSessionId) continue;
      const siblings = childrenByParent.get(candidate.parentSessionId.toLowerCase()) ?? [];
      siblings.push(candidate);
      childrenByParent.set(candidate.parentSessionId.toLowerCase(), siblings);
    }

    const tree: SessionRecord[] = [];
    const pending = [rootSession];
    const treeKeys = new Set<string>();
    while (pending.length > 0) {
      const current = pending.shift();
      if (!current || treeKeys.has(current.id.toLowerCase())) continue;
      treeKeys.add(current.id.toLowerCase());
      tree.push(current);
      pending.push(...(childrenByParent.get(current.id.toLowerCase()) ?? []));
    }
    return tree;
  }, [activeSession, allSessionItems]);
  const openRelatedSession = useCallback((session: SessionRecord) => {
    locateSessionInList(session);
    keyboardNavigationScopeRef.current = "detail";
    setDetailCollapsed(false);
  }, [locateSessionInList]);
  const activeImportedTranscript = activeSession ? importedTranscripts[activeSession.id] : undefined;
  const activeSessionTranscriptKey = useMemo(() => (
    activeSession
      ? `${activeSession.agent ?? ""}:${activeSession.path ?? ""}:${activeSession.id ?? ""}:${activeSession.updatedAt ?? ""}:${activeSession.messages ?? ""}`
      : ""
  ), [activeSession?.agent, activeSession?.id, activeSession?.messages, activeSession?.path, activeSession?.updatedAt]);
  const activeSessionLinkKey = useMemo(() => (
    activeSession ? sessionKey(activeSession) : ""
  ), [activeSession?.agent, activeSession?.id]);
  const activeSessionSkillLinksKey = useMemo(() => {
    if (!activeSession) return "";
    return [
      activeSessionLinkKey,
      skillIndexStatus?.indexed ?? 0,
      skillIndexStatus?.failed ?? 0,
    ].join(":");
  }, [activeSession, activeSessionLinkKey, skillIndexStatus?.failed, skillIndexStatus?.indexed]);
  const skillLinksLoading = loadingSkillLinks || Boolean(
    activeSession
      && activeSession.agent !== IMPORTED_SESSION_AGENT
      && skillLinksKey !== activeSessionSkillLinksKey,
  );
  const activeTranscriptLocatorItems = transcriptLocatorState?.key === activeSessionTranscriptKey
    ? transcriptLocatorState.items
    : undefined;
  const drainTranscriptLocator = useCallback(() => {
    if (loadingSessions || !loadTranscriptLocator) return;
    const pending = pendingTranscriptLocatorRef.current;
    if (!pending || pending.key !== activeSessionTranscriptKey) return;
    pendingTranscriptLocatorRef.current = null;
    const requestRevision = transcriptLocatorRequestAuthorityRef.current.begin();
    void loadTranscriptLocator(pending.session).then((page) => {
      if (!transcriptLocatorRequestAuthorityRef.current.isCurrent(requestRevision)) return;
      if (pending.key !== activeSessionTranscriptKey) return;
      const currentSourceVersion = transcriptSourceVersionRef.current;
      if (currentSourceVersion && page.sourceVersion && page.sourceVersion !== currentSourceVersion) return;
      transcriptLocatorCacheRef.current.set(pending.key, page);
      setTranscriptLocatorState({ key: pending.key, items: page.locatorItems });
    }).catch((error) => {
      if (transcriptLocatorRequestAuthorityRef.current.isCurrent(requestRevision)) {
        logger.warn("sessions transcript locator load failed", { error });
      }
    });
  }, [activeSessionTranscriptKey, loadTranscriptLocator, loadingSessions]);
  const queueTranscriptLocator = useCallback((session: SessionRecord, key: string) => {
    const cached = transcriptLocatorCacheRef.current.get(key);
    if (cached) {
      setTranscriptLocatorState({ key, items: cached.locatorItems });
      return;
    }
    pendingTranscriptLocatorRef.current = { key, session };
    drainTranscriptLocator();
  }, [drainTranscriptLocator]);
  useEffect(() => {
    drainTranscriptLocator();
  }, [drainTranscriptLocator]);
  const columns = useMemo(
    (): ColumnDef<SessionRecord>[] => createSessionTableColumns({
      normalizedQuery,
      resumeSession,
      resumeState: getResumeState,
      resumeTarget: sessionResumeTarget,
    } as { normalizedQuery: string; resumeSession: typeof resumeSession; resumeState: typeof getResumeState; resumeTarget: SessionResumeTarget }) as ColumnDef<SessionRecord>[],
    [getResumeState, normalizedQuery, resumeSession, sessionResumeTarget],
  );
  const rowContextMenu = useCallback((session: SessionRecord) => {
    const transcriptPath = `${session.path ?? ""}`.trim();
    const workspacePath = sessionWorkspacePath(session);
    const target = sessionResumeTargetForAgent(sessionResumeTarget, session.agent);
    const targetLabel = resumeTargetLabel(target);
    const canResume = session.agent !== IMPORTED_SESSION_AGENT
      && Boolean(session.id && session.agent && transcriptPath);
    return (
      <>
        <ContextMenu.Item
          className="skillMenuItem"
          disabled={!canResume}
          onSelect={() => { void resumeSession(session); }}
        >
          {target === "app"
            ? <MessageSquareText size={14} />
            : <TerminalSquare size={14} />}
          Resume in {targetLabel}
        </ContextMenu.Item>
        <CopyTextMenuItem Menu={ContextMenu} text={session.id} label="Copy session ID" />
        <CopyPathMenuItem Menu={ContextMenu} path={transcriptPath} label="Copy transcript path" />
        <RevealInFinderMenuItem Menu={ContextMenu} path={transcriptPath} label="Reveal transcript in Finder" />
        <CopyPathMenuItem Menu={ContextMenu} path={workspacePath} label="Copy workspace path" />
        <RevealInFinderMenuItem Menu={ContextMenu} path={workspacePath} label="Reveal workspace in Finder" />
      </>
    );
  }, [resumeSession]);
  useEffect(() => {
    const transcriptSession = activeSessionForTranscriptRef.current;
    const requestRevision = transcriptRequestAuthorityRef.current.begin();
    transcriptLocatorRequestAuthorityRef.current.begin();
    pendingTranscriptLocatorRef.current = null;
    transcriptSourceVersionRef.current = "";
    transcriptItemsRef.current = [];
    nextTranscriptCursorRef.current = undefined;
    setItems([]);
    setTranscriptLocatorState(null);
    setNextTranscriptCursor(undefined);
    setLoadingMoreTranscript(false);
    setLoading(Boolean(transcriptSession));
    if (!transcriptSession) return;
    if (activeImportedTranscript) {
      transcriptItemsRef.current = activeImportedTranscript;
      setItems(activeImportedTranscript);
      setLoading(false);
      return;
    }
    const cached = transcriptCacheRef.current.get(activeSessionTranscriptKey);
    if (cached) {
      transcriptItemsRef.current = cached.items;
      nextTranscriptCursorRef.current = cached.nextCursor;
      if (cached.locatorItems.length > 0) {
        const cachedLocator: TranscriptLocatorPage = {
          locatorItems: cached.locatorItems,
          warnings: cached.warnings,
          sourceVersion: cached.sourceVersion,
        };
        transcriptLocatorCacheRef.current.set(activeSessionTranscriptKey, cachedLocator);
        setTranscriptLocatorState({ key: activeSessionTranscriptKey, items: cached.locatorItems });
      }
      transcriptCacheRef.current.delete(activeSessionTranscriptKey);
      transcriptCacheRef.current.set(activeSessionTranscriptKey, cached);
      setItems(cached.items);
      setNextTranscriptCursor(cached.nextCursor);
      setLoading(false);
    }
    loadTranscript(transcriptSession, undefined, cached?.sourceVersion).then((page) => {
      if (!transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) return;
      if (cached && page.unchanged) {
        transcriptSourceVersionRef.current = cached.sourceVersion;
        queueTranscriptLocator(transcriptSession, activeSessionTranscriptKey);
        return;
      }
      transcriptCacheRef.current.set(activeSessionTranscriptKey, page);
      trimTranscriptCache(transcriptCacheRef.current);
      transcriptItemsRef.current = page.items;
      nextTranscriptCursorRef.current = page.nextCursor;
      transcriptSourceVersionRef.current = page.sourceVersion;
      if (page.locatorItems.length > 0) {
        const locatorPage: TranscriptLocatorPage = {
          locatorItems: page.locatorItems,
          warnings: page.warnings,
          sourceVersion: page.sourceVersion,
        };
        transcriptLocatorCacheRef.current.set(activeSessionTranscriptKey, locatorPage);
        setTranscriptLocatorState({ key: activeSessionTranscriptKey, items: page.locatorItems });
      }
      setItems(page.items);
      setNextTranscriptCursor(page.nextCursor);
      queueTranscriptLocator(transcriptSession, activeSessionTranscriptKey);
    }).catch((error) => {
      if (transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) {
        logger.warn("sessions transcript load failed", {
          requestRevision,
          id: transcriptSession.id,
          error,
        });
      }
      if (transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) {
        showSessionError("Could not load session details. Try again.");
      }
    }).finally(() => {
      if (transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) {
        setLoading(false);
      }
    });
    return () => {
      transcriptRequestAuthorityRef.current.invalidate(requestRevision);
    };
  }, [activeImportedTranscript, activeSessionTranscriptKey, loadTranscript, queueTranscriptLocator, showSessionError]);
  const loadMoreTranscript = useCallback((): Promise<TranscriptItemRecord[] | null> => {
    const requestKey = activeSessionTranscriptKey;
    const existing = loadMoreTranscriptInFlightRef.current;
    if (existing?.key === requestKey) return existing.promise;
    const cursor = nextTranscriptCursorRef.current;
    if (!activeSession || !cursor) return Promise.resolve(null);
    const requestRevision = transcriptRequestAuthorityRef.current.begin();
    const currentItems = transcriptItemsRef.current;
    setLoadingMoreTranscript(true);
    const promise = (async () => {
      try {
        const page = await loadTranscript(activeSession, cursor);
        if (!transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) return null;
        if (page.restartRequired) {
          const restarted = await loadTranscript(activeSession);
          if (!transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) return null;
          transcriptLocatorRequestAuthorityRef.current.begin();
          transcriptLocatorCacheRef.current.delete(requestKey);
          setTranscriptLocatorState(null);
          transcriptSourceVersionRef.current = restarted.sourceVersion;
          transcriptCacheRef.current.set(requestKey, restarted);
          trimTranscriptCache(transcriptCacheRef.current);
          transcriptItemsRef.current = restarted.items;
          nextTranscriptCursorRef.current = restarted.nextCursor;
          setItems(restarted.items);
          setNextTranscriptCursor(restarted.nextCursor);
          queueTranscriptLocator(activeSession, requestKey);
          return restarted.items;
        }
        const sourceChanged = Boolean(
          transcriptSourceVersionRef.current
            && page.sourceVersion
            && page.sourceVersion !== transcriptSourceVersionRef.current,
        );
        if (sourceChanged) {
          transcriptLocatorRequestAuthorityRef.current.begin();
          transcriptLocatorCacheRef.current.delete(requestKey);
          setTranscriptLocatorState(null);
        }
        transcriptSourceVersionRef.current = page.sourceVersion;
        const cached = transcriptCacheRef.current.get(requestKey);
        const merged: TranscriptPage = {
          items: mergeTranscriptItems(cached?.items ?? currentItems, page.items),
          locatorItems: cached?.locatorItems ?? page.locatorItems,
          warnings: [...(cached?.warnings ?? []), ...page.warnings],
          nextCursor: page.nextCursor,
          done: page.done,
          sourceVersion: page.sourceVersion,
          restartRequired: false,
          unchanged: false,
        };
        transcriptCacheRef.current.delete(requestKey);
        transcriptCacheRef.current.set(requestKey, merged);
        trimTranscriptCache(transcriptCacheRef.current);
        transcriptItemsRef.current = merged.items;
        nextTranscriptCursorRef.current = merged.nextCursor;
        setItems(merged.items);
        setNextTranscriptCursor(merged.nextCursor);
        if (sourceChanged) queueTranscriptLocator(activeSession, requestKey);
        return merged.items;
      } catch (error) {
        if (transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) {
          logger.warn("sessions transcript page load failed", { error });
          showSessionError("Could not load session details. Try again.");
        }
        return null;
      } finally {
        if (transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) {
          setLoadingMoreTranscript(false);
        }
      }
    })();
    loadMoreTranscriptInFlightRef.current = { key: requestKey, promise };
    void promise.then(
      () => {
        if (loadMoreTranscriptInFlightRef.current?.promise === promise) loadMoreTranscriptInFlightRef.current = null;
      },
      () => {
        if (loadMoreTranscriptInFlightRef.current?.promise === promise) loadMoreTranscriptInFlightRef.current = null;
      },
    );
    return promise;
  }, [activeSession, activeSessionTranscriptKey, loadTranscript, queueTranscriptLocator, showSessionError]);
  const loadAllTranscript = useCallback((): Promise<void> => {
    const requestKey = activeSessionTranscriptKey;
    const existing = loadAllTranscriptInFlightRef.current;
    if (existing?.key === requestKey) return existing.promise;

    const loadAll = async () => {
      while (nextTranscriptCursorRef.current) {
        const cursorBefore = nextTranscriptCursorRef.current;
        const loaded = await loadMoreTranscript();
        if (!loaded) break;
        if (nextTranscriptCursorRef.current === cursorBefore) break;
      }
    };
    const promise = loadAll();
    loadAllTranscriptInFlightRef.current = { key: requestKey, promise };
    void promise.then(
      () => {
        if (loadAllTranscriptInFlightRef.current?.promise === promise) loadAllTranscriptInFlightRef.current = null;
      },
      () => {
        if (loadAllTranscriptInFlightRef.current?.promise === promise) loadAllTranscriptInFlightRef.current = null;
      },
    );
    return promise;
  }, [activeSessionTranscriptKey, loadMoreTranscript]);
  useEffect(() => {
    setSkillLinks([]);
    setSkillLinksKey("");
    setSkillLinksError("");
    setLoadingSkillLinks(false);
    skillLinksRequestKeyRef.current = "";
  }, [activeSessionLinkKey]);
  const loadActiveSessionSkillLinks = useCallback(async () => {
    if (!activeSession || !loadSessionSkillLinks || !activeSessionSkillLinksKey) return;
    if (activeSession.agent === IMPORTED_SESSION_AGENT) {
      setSkillLinks([]);
      setSkillLinksError("");
      setSkillLinksKey(activeSessionSkillLinksKey);
      setLoadingSkillLinks(false);
      return;
    }
    if (loadingSkillLinks) return;
    if (skillLinksKey === activeSessionSkillLinksKey && !loadingSkillLinks) return;
    const requestKey = activeSessionSkillLinksKey;
    skillLinksRequestKeyRef.current = requestKey;
    setLoadingSkillLinks(true);
    setSkillLinksError("");
    try {
      const links = await loadSessionSkillLinks(activeSession);
      if (skillLinksRequestKeyRef.current !== requestKey) return;
      setSkillLinks(Array.isArray(links) ? links : []);
      setSkillLinksKey(requestKey);
    } catch (error) {
      if (skillLinksRequestKeyRef.current === requestKey) {
        logger.warn("sessions skill links load failed", { error });
        setSkillLinksError("Could not load skills used by this session.");
        showSessionError("Could not load skills used by this session. Try again.");
      }
    } finally {
      if (skillLinksRequestKeyRef.current === requestKey) setLoadingSkillLinks(false);
    }
  }, [activeSession, activeSessionSkillLinksKey, loadSessionSkillLinks, loadingSkillLinks, showSessionError, skillLinksKey]);
  useEffect(() => {
    loadActiveSessionSkillLinks();
  }, [loadActiveSessionSkillLinks]);

  const importButtonStateClass = importFeedback === "success"
    ? "isSuccess"
    : importFeedback === "warning"
      ? "isWarning"
      : importFeedback === "error"
        ? "isError"
        : "";
  const importButtonLabel = importFeedback === "loading"
    ? "Importing JSONL transcript"
    : importFeedback === "success"
      ? "JSONL transcript imported"
      : importFeedback === "warning"
        ? "JSONL transcript imported with warnings"
        : importFeedback === "error"
          ? importError || "Could not import JSONL transcript"
          : "Import JSONL transcript";
  return (
    <>
    {sessionToast ? <Toast tone="error" message={sessionToast} onDismiss={dismissSessionError} /> : null}
    <PanelGroup className="sessionsLayout" orientation="horizontal">
      <Panel className="sessionListPanel" defaultSize="62%" minSize="390px">
        <div className="sessionListPane">
          <PageHeader title="Sessions" compact>
            {developerMode ? (
              <>
                <IconButton
                  className={`sessionImportButton ${importButtonStateClass}`}
                  aria-label={importButtonLabel}
                  aria-busy={importFeedback === "loading"}
                  disabled={importFeedback === "loading"}
                  onClick={() => importInputRef.current?.click()}
                >
                  {importFeedback === "loading"
                    ? <LoadingIcon size={16} />
                    : importFeedback === "success"
                      ? <Check size={16} />
                      : importFeedback === "warning" || importFeedback === "error"
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
            <AppTooltip content={sessionRefreshError || refreshActionError || undefined}>
              <IconButton
                className={`sessionRefreshButton${sessionRefreshError || refreshActionError ? " isError" : ""}`}
                aria-label={sessionRefreshError || refreshActionError ? "Refresh sessions (last attempt failed)" : "Refresh sessions"}
                aria-busy={refreshing}
                onClick={refreshSessions}
                disabled={refreshing}
              >
                {refreshing ? <LoadingIcon size={16} /> : <RefreshCw size={16} />}
              </IconButton>
            </AppTooltip>
            <IconButton
              className={showChildSessions ? "filled" : ""}
              aria-label={showChildSessions ? "Hide child sessions" : `Show ${childSessionCount} child sessions`}
              aria-pressed={showChildSessions}
              onClick={() => setShowChildSessions((visible) => !visible)}
            >
              <GitFork size={16} />
            </IconButton>
            <div className="sessionSearchControls">
              <SearchField pageSearch placeholder="Search sessions" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
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
                  <DropdownMenu.Content
                    className="skillMenuContent sessionProjectFilterMenu"
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
                        const active = selectedProjectKeySet.has(option.key);
                        return (
                          <DropdownMenu.CheckboxItem
                            key={option.key}
                            className="skillMenuItem sessionProjectFilterItem"
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
                              : <AppTooltip content={option.title} onlyWhenTruncated><span className="sessionProjectFilterItemLabel">{option.label}</span></AppTooltip>}
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
                        size="sm"
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
                  </DropdownMenu.Content>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
            </div>
          </PageHeader>
          <div
            className="sessionListBody"
            ref={sessionListBodyRef}
            onPointerDownCapture={() => { keyboardNavigationScopeRef.current = "list"; }}
            onFocusCapture={() => { keyboardNavigationScopeRef.current = "list"; }}
          >
            <DataTable
              rows={tableSessions}
              columns={columns}
              getRowId={sessionTableRowId}
              freezeColumn={SESSION_FREEZE_COLUMN}
              defaultSort={{ key: "updatedAt", direction: "desc" }}
              sort={activeSort}
              onSortChange={handleSortChange}
              manualSorting
              rowHeight={SESSION_TABLE_ROW_HEIGHT}
              scrollToRowId={sessionLocatorRequest}
              onScrollToRowComplete={completeSessionLocator}
              groupBy={groupBy}
              onGroupByChange={setGroupBy}
              onRowClick={openSession}
              rowContextMenu={rowContextMenu}
              rowProps={(session) => (activeRowId === sessionTableRowId(session) ? { className: "rowSelected" } : {})}
              loading={(loadingSessions || searchingSessions) && sortedSessions.length === 0}
              loadingLabel="Loading sessions"
              emptyState={<EmptyState icon={<SearchX size={22} strokeWidth={1.75} />} iconTone="muted" title="No matching sessions" />}
            />
          </div>
          <div className="sessionPager">
            <div className="sessionPagerInfo">
              <span>
                {sortedSessions.length === 0
                  ? "0 sessions"
                  : groupBy
                    ? `${pageStart + 1}-${pageEnd} of ${sortedSessions.length} sessions · ${groupedPage?.groupCount ?? 0} groups`
                    : `${pageStart + 1}-${pageEnd} of ${sortedSessions.length}`}
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
                disabled={currentPage === 0}
              >
                <ChevronLeft size={15} />
              </IconButton>
              <span>{currentPage + 1} / {pageCount}</span>
              <IconButton
                aria-label="Next page"
                onClick={() => setCurrentPage((page) => Math.min(pageCount - 1, page + 1))}
                disabled={currentPage >= pageCount - 1}
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
          railLabel={formatSessionTitle(activeSession.title) || activeSession.id}
          hasSelection
          emptyState={null}
          expandedDefaultSize="38%"
          panelClassName="transcriptPanel"
        >
          <TranscriptPanel
            session={activeSession}
            parentSession={activeParentSession}
            childSessions={activeChildSessions}
            sessionTree={activeSessionTree}
              items={items}
              locatorMetadata={activeTranscriptLocatorItems}
              sessionSearchQuery={normalizedQuery}
              loading={loading}
            onReportError={showSessionError}
            hasMore={Boolean(nextTranscriptCursor)}
            loadingMore={loadingMoreTranscript}
            skillLinks={skillLinks}
              loadingSkillLinks={skillLinksLoading}
              skillLinksLoaded={skillLinksKey === activeSessionSkillLinksKey}
              skillLinksError={skillLinksError}
            onCollapse={() => {
              keyboardNavigationScopeRef.current = "list";
              setDetailCollapsed(true);
            }}
            keyboardNavigationScopeRef={keyboardNavigationScopeRef}
            showSessionLocator={!activeSessionVisibleInList || activeSessionInListViewport === false}
            onLocateSession={locateActiveSession}
            onResume={resumeSession}
            resumeTarget={sessionResumeTarget}
            resumeState={getResumeState(activeSession)}
            onOpenSession={openRelatedSession}
            onOpenSkill={onOpenSkill}
            onLoadSkills={loadActiveSessionSkillLinks}
            onLoadMore={loadMoreTranscript}
            onLoadAll={loadAllTranscript}
            searchTranscript={searchTranscript}
            onSavePrompt={onSavePrompt}
          />
        </DetailPanelHost>
      ) : null}
    </PanelGroup>
    </>
  );
}
