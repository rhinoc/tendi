import { Tooltip as AppTooltip } from "../components/shared/Tooltip.tsx";
import { lazy, memo, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type ReactNode } from "react";
import { AlertCircle, ArrowDownToLine, ArrowUpRight, ArrowUpToLine, BarChart3, Check, ChevronDown, ChevronLeft, ChevronRight, ChevronRight as ChevronRightIcon, Filter, FolderOpen, GitFork, GitMerge, Info, LocateFixed, MessageSquareText, PanelRightClose, RefreshCw, Search, SearchX, Sparkles, TerminalSquare, Upload, X } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu, Dialog, DropdownMenu, Popover } from "radix-ui";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import type { TokenMetricProps } from "../components/TokenStatusBar.tsx";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { CheckboxIndicator } from "../components/shared/CheckboxIndicator.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { CopyableSessionId } from "../features/sessions/CopyableSessionId.tsx";
import { CopyPathMenuItem, CopyTextMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogShell } from "../components/shared/DialogShell.tsx";
import { DialogStatefulButton } from "../components/shared/DialogStatefulButton.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
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
import { useElementSize } from "../components/shared/useElementSize.ts";
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
  friendlyAgent,
  groupTranscriptItems,
  isWebSource,
  mergeTranscriptItems,
  resolveInitialSession,
  safeInvoke,
  sessionCacheRate,
  sessionKind,
  sessionProject,
  sessionProjectGroupKey,
  sessionWorkspace,
  sessionWorkspacePath,
  transcriptItemType,
  transcriptItemsSize,
} from "../lib/index.ts";
import type {
  JsonlTranscriptParseResult,
  SessionProjectDelta,
  TranscriptPage,
  TranscriptSearchHit,
  TranscriptSearchResult,
  TranscriptSearchScopes,
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

type SkillLinkRecord = {
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
const IMPORTED_SESSION_AGENT = "Imported";
const TRANSCRIPT_CACHE_ITEM_LIMIT = 1_200;
const TRANSCRIPT_CACHE_CHARACTER_LIMIT = 8 * 1024 * 1024;
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
        { label: "Cached input", value: usage.cachedInputTokens },
        { label: "Uncached input", value: uncachedInputTokens },
      ],
    },
    {
      label: "Output",
      value: usage.outputTokens,
      details: [
        { label: "Reasoning output", value: usage.reasoningOutputTokens },
        { label: "Other output", value: nonReasoningOutputTokens },
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

  useEffect(() => {
    measuredHeightsRef.current.clear();
    scrollTopRef.current = 0;
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
    const safeIndex = Math.min(items.length - 1, Math.max(0, index));
    const top = offsetsRef.current[safeIndex] ?? 0;
    root.scrollTo({ top, behavior });
    scrollTopRef.current = top;
    renderRangeRef.current = transcriptRangeForViewport(items.length, offsetsRef.current, top, viewportSize.height, virtualized);
    setRenderScrollTop(top);
  }, [items.length, rootRef, viewportSize.height, virtualized]);

  return {
    virtualized,
    range,
    rangeKey: `${range.start}:${range.end}:${measurementVersion}`,
    topSpacerHeight: offsets[range.start] ?? 0,
    bottomSpacerHeight: Math.max(0, (offsets[items.length] ?? 0) - (offsets[range.end] ?? 0)),
    measureItem,
    scrollToIndex,
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

function highlightTranscriptText(value: string | undefined, query: string): ReactNode {
  const text = `${value ?? ""}`;
  const needle = query.trim().toLowerCase();
  if (!needle) return text;

  const parts: ReactNode[] = [];
  const haystack = text.toLowerCase();
  let offset = 0;
  let matchIndex = haystack.indexOf(needle, offset);
  while (matchIndex >= 0) {
    if (matchIndex > offset) parts.push(text.slice(offset, matchIndex));
    parts.push(<mark className="transcriptSearchMark" key={`${matchIndex}-${parts.length}`}>{text.slice(matchIndex, matchIndex + needle.length)}</mark>);
    offset = matchIndex + needle.length;
    matchIndex = haystack.indexOf(needle, offset);
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
  loading,
  hasMore,
  loadingMore,
  skillLinks,
  loadingSkillLinks,
  skillLinksLoaded,
  onCollapse,
  onResume,
  resumeState,
  onOpenSession,
  onOpenSkill,
  onLoadSkills,
  onLoadMore,
  onLoadAll,
  searchTranscript,
  showSessionLocator,
  onLocateSession,
}: {
  session: SessionRecord;
  parentSession?: SessionRecord;
  childSessions: SessionRecord[];
  sessionTree: SessionRecord[];
  items: TranscriptItemRecord[];
  loading: boolean;
  hasMore: boolean;
  loadingMore: boolean;
  skillLinks: SkillLinkRecord[];
  loadingSkillLinks: boolean;
  skillLinksLoaded: boolean;
  onCollapse: () => void;
  onResume: (session: SessionRecord) => void;
  resumeState: SessionResumeState;
  onOpenSession: (session: SessionRecord) => void;
  onOpenSkill?: (skillName: string) => void;
  onLoadSkills?: () => void;
  onLoadMore: () => Promise<TranscriptItemRecord[] | null>;
  onLoadAll: () => Promise<void>;
  searchTranscript?: (session: SessionRecord, query: string, scopes: TranscriptSearchScopes) => Promise<TranscriptSearchResult | null>;
  showSessionLocator: boolean;
  onLocateSession: () => void;
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
  const locatorItems = useMemo(() => buildSessionLocatorItems(transcriptItems), [transcriptItems]);
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
  const [transcriptScrollEdges, setTranscriptScrollEdges] = useState({ atTop: true, atBottom: true });
  const highlightTimerRef = useRef(0);
  const transcriptItemsRef = useRef(transcriptItems);
  transcriptItemsRef.current = transcriptItems;
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
        else setSearchError(true);
      },
      () => {
        if (cancelled) return;
        setSearchLoading(false);
        setSearchReadyQuery(normalizedSearchQuery);
        setSearchError(true);
      },
    );
    return () => {
      cancelled = true;
    };
  }, [normalizedSearchQuery, remoteSearchActive, searchScopes, searchTranscript, session]);
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
      if (!event.metaKey || event.key.toLowerCase() !== "f") return;
      event.preventDefault();
      setSearchOpen(true);
      window.requestAnimationFrame(() => searchInputRef.current?.focus());
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
    let attempts = 0;
    window.requestAnimationFrame(() => {
      const root = transcriptRef.current;
      if (!root) return;
      const scrollToTarget = () => {
        if (target.groupKey) {
          const group = root.querySelector(`[data-transcript-key="${cssEscape(target.groupKey)}"]`) as HTMLDetailsElement | null;
          if (!group) {
            if (targetIndex !== null && attempts < 3) {
              attempts += 1;
              window.requestAnimationFrame(scrollToTarget);
            }
            return;
          }
          if (!group.open) {
            group.open = true;
            window.requestAnimationFrame(scrollToTarget);
            return;
          }
        }
        const node = root.querySelector(`[data-transcript-key="${cssEscape(target.key)}"]`);
        if (!node) {
          if (targetIndex !== null && attempts < 3) {
            attempts += 1;
            window.requestAnimationFrame(scrollToTarget);
          }
          return;
        }
        const details = node.closest("details");
        if (details && !details.open) {
          details.open = true;
          if (preferSearchMatch) {
            window.requestAnimationFrame(scrollToTarget);
            return;
          }
        }
        const destination = preferSearchMatch
          ? node.querySelector(".transcriptSearchMark") ?? node
          : node;
        destination.scrollIntoView({ block: "center", behavior });
        highlightTimerRef.current = window.setTimeout(() => setHighlightedKey(""), 1800);
      };
      scrollToTarget();
    });
  }, [scrollTranscriptToIndex]);
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
  const scrollTranscriptToTop = useCallback(() => {
    const root = transcriptRef.current;
    if (!root) return;
    root.scrollTo({ top: 0, behavior: "smooth" });
  }, []);
  const scrollTranscriptToBottom = useCallback(() => {
    let frameCount = 0;
    const settle = () => {
      const root = transcriptRef.current;
      if (!root) return;
      root.scrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
      updateTranscriptScrollEdges();
      if (frameCount < 4) {
        frameCount += 1;
        window.requestAnimationFrame(settle);
      }
    };
    window.requestAnimationFrame(settle);
  }, [updateTranscriptScrollEdges]);
  const jumpToBottom = useCallback(async () => {
    setJumpingToBottom(true);
    try {
      if (hasMore) await onLoadAll();
    } finally {
      setJumpingToBottom(false);
      scrollTranscriptToBottom();
    }
  }, [hasMore, onLoadAll, scrollTranscriptToBottom]);
  const jumpToSkillEvidence = useCallback((link: SkillLinkRecord) => {
    const target = findSkillEvidenceTarget(transcriptItems, link);
    if (target) focusTranscriptTarget(target);
  }, [focusTranscriptTarget, transcriptItems]);
  const selectLocatorItem = useCallback((key: string, behavior?: ScrollBehavior) => {
    focusTranscriptTarget({ key, index: transcriptIndexFromKey(key) ?? 0 }, false, behavior);
  }, [focusTranscriptTarget]);
  useEffect(() => {
    setSearchIndex(0);
  }, [normalizedSearchQuery, searchScopes, transcriptItems]);
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
  }, [ensureSearchHitLoaded, focusTranscriptTarget, remoteSearchActive, searchError, searchIndex, searchReady, searchResult, searchResultCount, searchLoading, searchTargets, syntheticCursorConfig]);
  const moveSearchResult = useCallback((offset: number) => {
    if (searchResultCount === 0) return;
    setSearchIndex((current) => (current + offset + searchResultCount) % searchResultCount);
  }, [searchResultCount]);
  const openLinkedSession = useCallback((sessionId: string) => {
    const linkedSession = childSessions.find((child) => child.id === sessionId);
    if (linkedSession) onOpenSession(linkedSession);
  }, [childSessions, onOpenSession]);
  return (
    <aside className="transcriptPanel">
      <header className="threadHeader">
        <div className="threadTitleLine">
          <h2><SessionTitleText interactive={false} value={session.title} /></h2>
          <div className="threadHeaderActions">
            {showSessionLocator ? (
              <AppTooltip content="Locate session in list">
                <button
                  type="button"
                  className="threadPanelToggle"
                  aria-label="Locate session in list"
                  onClick={onLocateSession}
                >
                  <LocateFixed size={15} aria-hidden="true" />
                </button>
              </AppTooltip>
            ) : null}
            <AppTooltip content={resumeState === "loading"
                ? "Opening session in terminal"
                : resumeState === "success"
                  ? "Session opened in terminal"
                  : resumeState === "error"
                    ? "Could not open session in terminal"
                    : "Resume session in terminal"}>
              <StatefulButton
                size="sm"
                variant="ghost"
                className={`threadPanelToggle sessionResumeButton${resumeState === "success" ? " isSuccess" : resumeState === "error" ? " isError" : ""}`}
                width="30px"
                minWidth="30px"
                style={{ height: "30px", padding: 0, display: "grid", placeItems: "center", gap: 0 }}
                state={resumeState}
                aria-label={resumeState === "loading"
                  ? "Opening session in terminal"
                  : resumeState === "success"
                    ? "Session opened in terminal"
                    : resumeState === "error"
                      ? "Could not open session in terminal"
                      : "Resume session in terminal"}
                aria-busy={resumeState === "loading"}
                disabled={resumeState === "loading"}
                onClick={() => onResume(session)}
                loadingContent={<LoadingIcon size={15} />}
                successContent={<Check size={15} aria-hidden="true" />}
                errorContent={<AlertCircle size={15} aria-hidden="true" />}
              >
                <TerminalSquare size={15} aria-hidden="true" />
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
              onLoad={onLoadSkills}
              onOpenSkill={onOpenSkill}
              onJumpToEvidence={jumpToSkillEvidence}
            />
            <SessionInfoMenu session={session} />
            <button className="threadPanelToggle" aria-label="Collapse session detail" onClick={onCollapse}><PanelRightClose size={16} /></button>
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
                  className="transcriptSearchScopeTrigger"
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
                : searchError ? "Transcript search failed" : "Search messages in this session"}
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
            {normalizedSearchQuery ? <span className="transcriptSearchCount">{searchLoading ? "…" : searchError ? "!" : searchResultCount ? `${searchIndex + 1}/${searchResultCount}` : "0/0"}</span> : null}
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
        <div className={`transcript ${locatorItems.length >= SESSION_LOCATOR_MIN_ITEMS ? "withSessionLocator" : ""}`} ref={transcriptRef}>
          {loading ? <LoadingState label="Loading transcript" /> : transcriptItems.length > 0 ? (
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
          ) : <div className="emptyState">No transcript items</div>}
        </div>
        {(!transcriptScrollEdges.atTop || !transcriptScrollEdges.atBottom) ? (
          <div className="transcriptScrollControls" aria-label="Transcript navigation">
            {!transcriptScrollEdges.atTop ? (
              <button
                className="threadPanelToggle transcriptScrollTop"
                aria-label="Jump to top of transcript"
                disabled={loading}
                onClick={scrollTranscriptToTop}
              >
                <ArrowUpToLine size={15} />
              </button>
            ) : null}
            {!transcriptScrollEdges.atBottom ? (
              <button
                className="threadPanelToggle transcriptScrollBottom"
                aria-label={jumpingToBottom ? "Loading and jumping to bottom of transcript" : "Jump to bottom of transcript"}
                aria-busy={jumpingToBottom}
                disabled={loading || jumpingToBottom}
                onClick={() => { void jumpToBottom(); }}
              >
                {jumpingToBottom ? <LoadingIcon size={15} /> : <ArrowDownToLine size={15} />}
              </button>
            ) : null}
          </div>
        ) : null}
      </div>
      <SessionLocator
        items={locatorItems}
        loading={loading}
        scrollRootRef={transcriptRef}
        transcriptRenderRangeKey={transcriptRenderRangeKey}
        onSelect={selectLocatorItem}
      />
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
          <button
            className="threadPanelToggle"
            aria-label={`Show ${relationCount} related session${relationCount === 1 ? "" : "s"}`}
          >
            <GitFork size={15} />
          </button>
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
  onLoad,
  onOpenSkill,
  onJumpToEvidence,
}: {
  session: SessionRecord;
  links?: SkillLinkRecord[];
  loading?: boolean;
  loaded?: boolean;
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
        <button className="threadPanelToggle" aria-label={disabledReason} disabled>
          <Sparkles size={15} />
        </button>
      </span></AppTooltip>
    );
  }

  return (
    <Popover.Root onOpenChange={handleOpenChange}>
      <Popover.Trigger asChild>
        <button className="threadPanelToggle" aria-label="Show skills used">
          <Sparkles size={15} />
        </button>
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
  onOpenSkill,
  onJumpToEvidence,
}: {
  session: SessionRecord;
  links?: SkillLinkRecord[];
  loading?: boolean;
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
      ) : links.length > 0 ? (
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
  const hasWorkspacePath = Boolean(workspacePath);
  return (
    <InfoDropdownMenu
      trigger={(
        <button className="threadPanelToggle" aria-label="Show session info">
          <Info size={15} />
        </button>
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
                <strong>{friendlyAgent(session.agent)}</strong>
            </InfoSection>
            <InfoSection label="Workspace">
                <AppTooltip content={workspace} onlyWhenTruncated><code>{workspace}</code></AppTooltip>
                {hasWorkspacePath && (
                  <button
                    aria-label="Reveal workspace in Finder"
                    className="skillInfoIconButton"
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path: workspacePath })}
                  >
                    <FolderOpen size={13} />
                  </button>
                )}
                <CopyButton className="skillInfoIconButton" value={workspace} copyLabel="Copy workspace" copiedLabel="Workspace copied" />
            </InfoSection>
            <InfoSection label="Timeline" valueLine={false}>
              <div className="sessionTimeline">
                <div className="sessionTimelineItem">
                  <span className="sessionTimelineDot" aria-hidden="true" />
                  <div className="sessionTimelineText">
                    <strong>Started</strong>
                    <AppTooltip content={session.startedAt || ""}><code>{session.startedLabel || "-"}</code></AppTooltip>
                  </div>
                </div>
                <div className="sessionTimelineItem">
                  <span className="sessionTimelineDot" aria-hidden="true" />
                  <div className="sessionTimelineText">
                    <strong>Updated</strong>
                    <AppTooltip content={session.updatedAt || ""}><code>{session.updatedDetailLabel || "-"}</code></AppTooltip>
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
                  <AppTooltip content={transcriptPath} onlyWhenTruncated><code>{transcriptPath}</code></AppTooltip>
                  <button
                    aria-label="Reveal transcript in Finder"
                    className="skillInfoIconButton"
                    onClick={() => safeInvoke(TauriCommand.RevealInFinder, { path: transcriptPath })}
                  >
                    <FolderOpen size={13} />
                  </button>
                  <CopyButton className="skillInfoIconButton" value={transcriptPath} copyLabel="Copy transcript path" copiedLabel="Transcript path copied" />
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

export const TranscriptItem = memo(function TranscriptItem({
  item,
  itemKey,
  addTopSpacing,
  highlightedKey,
  searchQuery,
  onOpenLinkedSession,
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
              className="messageCopyButton"
              value={body}
              copyLabel={isUser ? "Copy user message" : "Copy model message"}
              copiedLabel="Message copied"
            />
          ) : null}
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
        <span className="contextBadge">{label}</span>
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
        <span className="thinkingBadge">{type}</span>
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
        {duration ? <span className="toolCallDuration">{duration}</span> : null}
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
        {item.tag ? <span className="toolCallTag">{item.tag}</span> : null}
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
        {duration ? <span className="toolCallDuration">{duration}</span> : null}
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
  searchTranscript,
  searchSessions,
  loadSessionSkillLinks,
  loadingSessions = false,
  onRefreshSessions,
  onResumeSession,
  onOpenSkill,
  activeSessionKey,
  skillIndexStatus,
  onSessionProjectsChanged,
}: {
  sessions: SessionRecord[];
  developerMode: boolean;
  loadTranscript: (session: SessionRecord, cursor?: string, knownSourceVersion?: string) => Promise<TranscriptPage>;
  searchTranscript?: (session: SessionRecord, query: string, scopes: TranscriptSearchScopes) => Promise<TranscriptSearchResult | null>;
  searchSessions?: (query: string) => Promise<SessionRecord[]>;
  loadSessionSkillLinks?: (session: SessionRecord) => Promise<SkillLinkRecord[]>;
  loadingSessions?: boolean;
  onRefreshSessions?: () => Promise<unknown>;
  onResumeSession?: (session: SessionRecord) => Promise<{ terminal?: string } | null | undefined>;
  onOpenSkill?: (skillName: string) => void;
  activeSessionKey?: string;
  skillIndexStatus?: SkillIndexStatus | null;
  onSessionProjectsChanged?: (delta: SessionProjectDelta) => void;
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
  const [nextTranscriptCursor, setNextTranscriptCursor] = useState<string | undefined>();
  const [loadingMoreTranscript, setLoadingMoreTranscript] = useState(false);
  const [skillLinks, setSkillLinks] = useState<SkillLinkRecord[]>([]);
  const [skillLinksKey, setSkillLinksKey] = useState("");
  const [loadingSkillLinks, setLoadingSkillLinks] = useState(false);
  const [loading, setLoading] = useState(false);
  const [searchingSessions, setSearchingSessions] = useState(false);
  const [searchRows, setSearchRows] = useState<SessionRecord[] | null>(null);
  const [searchRowsKey, setSearchRowsKey] = useState("");
  const [refreshing, setRefreshing] = useState(false);
  const [resumeFeedback, setResumeFeedback] = useState<Record<string, ResumeFeedbackState>>({});
  const [detailCollapsed, setDetailCollapsed] = useState(false);
  const [sessionLocatorRequest, setSessionLocatorRequest] = useState("");
  const [activeSessionInListViewport, setActiveSessionInListViewport] = useState<boolean | null>(null);
  const [selectedSessionIds, setSelectedSessionIds] = useState<string[]>([]);
  const [pendingSplitSessions, setPendingSplitSessions] = useState<SessionRecord[]>([]);
  const [splitProjectName, setSplitProjectName] = useState("");
  const [projectActionBusy, setProjectActionBusy] = useState(false);
  const [projectActionError, setProjectActionError] = useState("");
  const [selectedProjectKeys, setSelectedProjectKeys] = useState<string[]>([]);
  const [projectFilterQuery, setProjectFilterQuery] = useState("");
  const [projectFilterOpen, setProjectFilterOpen] = useState(false);
  const [debouncedQuery, setDebouncedQuery] = useState("");
  const transcriptItemsRef = useRef(items);
  const nextTranscriptCursorRef = useRef(nextTranscriptCursor);
  const loadingMoreTranscriptRef = useRef(loadingMoreTranscript);
  transcriptItemsRef.current = items;
  nextTranscriptCursorRef.current = nextTranscriptCursor;
  loadingMoreTranscriptRef.current = loadingMoreTranscript;
  const importInputRef = useRef<HTMLInputElement | null>(null);
  const sessionListBodyRef = useRef<HTMLDivElement | null>(null);
  const importWorkerRef = useRef<{ worker: Worker; cancel: () => void } | null>(null);
  const projectFilterInputRef = useRef<HTMLInputElement | null>(null);
  const importFeedbackTimerRef = useRef<number | undefined>(undefined);
  const resumeFeedbackTimerRef = useRef<Record<string, number>>({});
  const skillLinksRequestKeyRef = useRef("");
  const transcriptRequestAuthorityRef = useRef(createLatestRequestAuthority());
  const transcriptCacheRef = useRef(new Map<string, TranscriptPage>());
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
  useEffect(() => () => {
    clearImportFeedbackTimer();
    importWorkerRef.current?.cancel();
    importWorkerRef.current = null;
    Object.values(resumeFeedbackTimerRef.current).forEach((timer) => window.clearTimeout(timer));
  }, [clearImportFeedbackTimer]);
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

      if (normalizedProjectFilterQuery) {
        const leftRank = projectSearchRank(left.label, normalizedProjectFilterQuery);
        const rightRank = projectSearchRank(right.label, normalizedProjectFilterQuery);
        if (leftRank && rightRank) {
          return leftRank.distance - rightRank.distance
            || leftRank.start - rightRank.start
            || leftRank.length - rightRank.length;
        }
      } else {
        const lengthOrder = left.label.length - right.label.length;
        if (lengthOrder !== 0) return lengthOrder;
      }
      return left.label.localeCompare(right.label) || left.title.localeCompare(right.title);
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
  const selectedSessionRows = useMemo(() => {
    const selected = new Set(selectedSessionIds);
    return listSessionItems.filter((session) => selected.has(sessionTableRowId(session)));
  }, [listSessionItems, selectedSessionIds]);
  const allSessionItems = listSessionItems;
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
    }).finally(() => {
      if (!cancelled) setSearchingSessions(false);
    });
    return () => {
      cancelled = true;
    };
  }, [normalizedQuery, searchSessions]);
  const sessionMatchesQuery = useCallback((session: SessionRecord, currentQuery: string) => (
    [session.title, sessionProject(session), sessionWorkspace(session), session.agent, session.model, session.mode, session.approvalMode, session.isRunEverything, session.startedAt, session.updatedAt]
      .some((value) => `${value ?? ""}`.toLowerCase().includes(currentQuery))
  ), []);
  const matchedSessions = useMemo(() => {
    if (normalizedQuery && searchSessions) {
      const remoteRows = searchRowsKey === normalizedQuery ? (searchRows ?? []) : [];
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
  }, [listSessionItems, normalizedQuery, searchRows, searchRowsKey, searchSessions, selectedProjectKeySet, sessionMatchesQuery]);
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
    setCurrentPage(0);
  }, [activeSort?.direction, activeSort?.key, groupBy, normalizedQuery, pageSize, showChildSessions]);
  useEffect(() => {
    if (currentPage >= pageCount) setCurrentPage(pageCount - 1);
  }, [currentPage, pageCount]);
  useEffect(() => {
    if (activeRowId && !allSessionItems.some((session) => sessionTableRowId(session) === activeRowId)) setActiveRowId("");
  }, [activeRowId, allSessionItems]);
  useEffect(() => {
    if (!activeSessionKey) return;
    const next = allSessionItems.find((session) => sessionKey(session) === `${activeSessionKey}`.toLowerCase());
    if (next) {
      if (sessionKind(next) === "child") setShowChildSessions(true);
      setActiveRowId(sessionTableRowId(next));
      setDetailCollapsed(false);
    }
  }, [activeSessionKey, allSessionItems]);
  const openSession = useCallback((session: SessionRecord) => {
    setActiveRowId(sessionTableRowId(session));
    setDetailCollapsed(false);
  }, []);
  const refreshSessions = useCallback(async () => {
    if (refreshing || !onRefreshSessions) return;
    setRefreshing(true);
    try {
      await onRefreshSessions();
    } finally {
      setRefreshing(false);
    }
  }, [onRefreshSessions, refreshing]);
  const mergeSelectedProjects = useCallback(async (selectedRows: SessionRecord[]) => {
    if (projectActionBusy) return;
    const projectRows = [...selectedRows]
      .filter((session) => session.logicalProjectId)
      .sort((left, right) => `${right.updatedAt ?? ""}`.localeCompare(`${left.updatedAt ?? ""}`));
    const projectIds = [...new Set(projectRows.map((session) => session.logicalProjectId!))];
    if (projectIds.length < 2) return;
    setProjectActionBusy(true);
    setProjectActionError("");
    try {
      const delta = await safeInvoke<SessionProjectDelta>(TauriCommand.SessionsProjectMerge, {
        targetProjectId: projectIds[0],
        sourceProjectIds: projectIds.slice(1),
      });
      if (!delta) {
        setProjectActionError("Could not merge projects");
        return;
      }
      onSessionProjectsChanged?.(delta);
      setSelectedSessionIds([]);
    } finally {
      setProjectActionBusy(false);
    }
  }, [onSessionProjectsChanged, projectActionBusy]);
  const beginSplitProject = useCallback((selectedRows: SessionRecord[]) => {
    if (projectActionBusy || selectedRows.length === 0) return;
    const baseName = selectedRows[0].logicalProjectName || sessionProject(selectedRows[0]);
    setPendingSplitSessions(selectedRows);
    setSplitProjectName(`${baseName} split`);
    setProjectActionError("");
  }, [projectActionBusy]);
  const splitSelectedSessions = useCallback(async () => {
    const name = splitProjectName.trim();
    if (!name || projectActionBusy || pendingSplitSessions.length === 0) return;
    setProjectActionBusy(true);
    setProjectActionError("");
    try {
      const delta = await safeInvoke<SessionProjectDelta>(TauriCommand.SessionsProjectSplit, {
        name,
        sessions: pendingSplitSessions.map((session) => ({
          id: session.id,
          agent: `${session.agent ?? ""}`.toLowerCase(),
          path: session.path,
        })),
      });
      if (!delta) {
        setProjectActionError("Could not split sessions into a project");
        return;
      }
      onSessionProjectsChanged?.(delta);
      setPendingSplitSessions([]);
      setSelectedSessionIds([]);
    } finally {
      setProjectActionBusy(false);
    }
  }, [onSessionProjectsChanged, pendingSplitSessions, projectActionBusy, splitProjectName]);
  const importJsonl = useCallback(async (file: File) => {
    clearImportFeedbackTimer();
    setImportFeedback("loading");
    setImportError("");
    try {
      const parsed = await parseImportedTranscript(file, importWorkerRef);
      if (parsed.parsedCount === 0) {
        setImportError("No valid JSONL rows");
        finishImportFeedback("error");
        return;
      }
      if (parsed.items.length === 0) {
        setImportError("No renderable transcript items");
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
      setImportError(`Import failed: ${error instanceof Error ? error.message : `${error}`}`);
      finishImportFeedback("error");
    }
  }, [clearImportFeedbackTimer, finishImportFeedback]);
  const handleImportJsonlChange = useCallback((event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = "";
    if (!file) return;
    importJsonl(file);
  }, [importJsonl]);
  const resumeSession = useCallback(async (session: SessionRecord) => {
    if (session.agent === IMPORTED_SESSION_AGENT) {
      finishResumeFeedback(session.id, "error");
      return;
    }
    if (!onResumeSession) return;
    setResumeFeedback((current) => ({ ...current, [session.id]: "loading" }));
    try {
      const result = await onResumeSession(session);
      finishResumeFeedback(session.id, result?.terminal ? "success" : "error");
    } catch {
      finishResumeFeedback(session.id, "error");
    }
  }, [finishResumeFeedback, onResumeSession]);
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
    if (!activeSession) return;
    const targetRowId = sessionTableRowId(activeSession);
    const showChildren = showChildSessions || sessionKind(activeSession) === "child";
    const sessionsToShow = allSessionItems.filter((session) => showChildren || sessionKind(session) === "main");
    const sortedSessionsToShow = [...sessionsToShow].sort((left, right) => compareSessions(left, right, sort));
    const targetPage = groupBy
      ? buildGroupedSessionPages(sortedSessionsToShow, groupBy, pageSize)
        .findIndex((page) => page.rows.some((session) => sessionTableRowId(session) === targetRowId))
      : Math.floor(Math.max(0, sortedSessionsToShow.findIndex((session) => sessionTableRowId(session) === targetRowId)) / pageSize);

    setQuery("");
    setDebouncedQuery("");
    setSearchRows(null);
    setSearchRowsKey("");
    setSearchSort(null);
    setSelectedProjectKeys([]);
    setProjectFilterQuery("");
    if (showChildren && !showChildSessions) setShowChildSessions(true);
    setCurrentPage(Math.max(0, targetPage));
    setSessionLocatorRequest(targetRowId);
  }, [activeSession, allSessionItems, groupBy, pageSize, showChildSessions, sort]);
  const activeSessionForTranscriptRef = useRef(activeSession);
  activeSessionForTranscriptRef.current = activeSession;
  useEffect(() => {
    if (!sessionLocatorRequest) return undefined;
    let frame = 0;
    let attempts = 0;
    const locate = () => {
      frame = 0;
      const root = sessionListBodyRef.current?.querySelector<HTMLElement>(".dataTableBodyScroll");
      if (!root) {
        if (attempts < 20) {
          attempts += 1;
          frame = window.requestAnimationFrame(locate);
        } else {
          setSessionLocatorRequest("");
        }
        return;
      }
      const row = [...root.querySelectorAll<HTMLElement>("[data-row-id]")]
        .find((candidate) => candidate.dataset.rowId === sessionLocatorRequest);
      if (row) {
        const rootBounds = root.getBoundingClientRect();
        const rowBounds = row.getBoundingClientRect();
        const desiredTop = root.scrollTop
          + rowBounds.top
          - rootBounds.top
          - (root.clientHeight - rowBounds.height) / 2;
        const maxScrollTop = Math.max(0, root.scrollHeight - root.clientHeight);
        root.scrollTo({
          top: Math.min(maxScrollTop, Math.max(0, desiredTop)),
          behavior: "smooth",
        });
        setSessionLocatorRequest("");
        return;
      }

      const targetIndex = tableSessions.findIndex((session) => sessionTableRowId(session) === sessionLocatorRequest);
      if (targetIndex >= 0 && !groupBy) {
        const headerHeight = root.querySelector<HTMLElement>(".dataTableHeader")?.offsetHeight ?? 0;
        const desiredTop = headerHeight
          + targetIndex * SESSION_TABLE_ROW_HEIGHT
          - (root.clientHeight - SESSION_TABLE_ROW_HEIGHT) / 2;
        root.scrollTop = Math.min(
          Math.max(0, root.scrollHeight - root.clientHeight),
          Math.max(0, desiredTop),
        );
      }
      if (attempts < 20) {
        attempts += 1;
        frame = window.requestAnimationFrame(locate);
      } else {
        setSessionLocatorRequest("");
      }
    };
    frame = window.requestAnimationFrame(locate);
    return () => {
      if (frame !== 0) window.cancelAnimationFrame(frame);
    };
  }, [groupBy, sessionLocatorRequest, tableSessions]);
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
    if (sessionKind(session) === "child") setShowChildSessions(true);
    setQuery("");
    setDebouncedQuery("");
    setSearchRows(null);
    setSearchRowsKey("");
    setCurrentPage(0);
    setActiveRowId(sessionTableRowId(session));
    setDetailCollapsed(false);
  }, []);
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
  const columns = useMemo(
    (): ColumnDef<SessionRecord>[] => createSessionTableColumns({
      normalizedQuery,
      resumeSession,
      resumeState: getResumeState,
    } as { normalizedQuery: string; resumeSession: typeof resumeSession; resumeState: typeof getResumeState }) as ColumnDef<SessionRecord>[],
    [getResumeState, normalizedQuery, resumeSession],
  );
  const rowContextMenu = useCallback((session: SessionRecord) => {
    const transcriptPath = `${session.path ?? ""}`.trim();
    const workspacePath = sessionWorkspacePath(session);
    const canResume = session.agent !== IMPORTED_SESSION_AGENT
      && Boolean(session.id && session.agent && transcriptPath);
    return (
      <>
        <ContextMenu.Item
          className="skillMenuItem"
          disabled={!canResume}
          onSelect={() => { void resumeSession(session); }}
        >
          <TerminalSquare size={14} />
          Resume in terminal
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
    transcriptItemsRef.current = [];
    nextTranscriptCursorRef.current = undefined;
    setItems([]);
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
      transcriptCacheRef.current.delete(activeSessionTranscriptKey);
      transcriptCacheRef.current.set(activeSessionTranscriptKey, cached);
      setItems(cached.items);
      setNextTranscriptCursor(cached.nextCursor);
      setLoading(false);
    }
    loadTranscript(transcriptSession, undefined, cached?.sourceVersion).then((page) => {
      if (!transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) return;
      if (cached && page.unchanged) return;
      transcriptCacheRef.current.set(activeSessionTranscriptKey, page);
      trimTranscriptCache(transcriptCacheRef.current);
      transcriptItemsRef.current = page.items;
      nextTranscriptCursorRef.current = page.nextCursor;
      setItems(page.items);
      setNextTranscriptCursor(page.nextCursor);
    }).finally(() => {
      if (transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) {
        setLoading(false);
      }
    });
    return () => {
      transcriptRequestAuthorityRef.current.invalidate(requestRevision);
    };
  }, [activeImportedTranscript, activeSessionTranscriptKey, loadTranscript]);
  const loadMoreTranscript = useCallback(async (): Promise<TranscriptItemRecord[] | null> => {
    const cursor = nextTranscriptCursorRef.current;
    if (!activeSession || !cursor || loadingMoreTranscriptRef.current) return null;
    const requestRevision = transcriptRequestAuthorityRef.current.begin();
    const requestKey = activeSessionTranscriptKey;
    const currentItems = transcriptItemsRef.current;
    loadingMoreTranscriptRef.current = true;
    setLoadingMoreTranscript(true);
    try {
      const page = await loadTranscript(activeSession, cursor);
      if (!transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) return null;
      if (page.restartRequired) {
        const restarted = await loadTranscript(activeSession);
        if (!transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) return null;
        transcriptCacheRef.current.set(requestKey, restarted);
        trimTranscriptCache(transcriptCacheRef.current);
        transcriptItemsRef.current = restarted.items;
        nextTranscriptCursorRef.current = restarted.nextCursor;
        setItems(restarted.items);
        setNextTranscriptCursor(restarted.nextCursor);
        return restarted.items;
      }
      const cached = transcriptCacheRef.current.get(requestKey);
      const merged: TranscriptPage = {
        items: mergeTranscriptItems(cached?.items ?? currentItems, page.items),
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
      return merged.items;
    } finally {
      loadingMoreTranscriptRef.current = false;
      if (transcriptRequestAuthorityRef.current.isCurrent(requestRevision)) {
        setLoadingMoreTranscript(false);
      }
    }
  }, [activeSession, activeSessionTranscriptKey, loadTranscript]);
  const loadAllTranscript = useCallback((): Promise<void> => {
    const requestKey = activeSessionTranscriptKey;
    const existing = loadAllTranscriptInFlightRef.current;
    if (existing?.key === requestKey) return existing.promise;

    const loadAll = async () => {
      while (nextTranscriptCursorRef.current) {
        const cursorBefore = nextTranscriptCursorRef.current;
        const loaded = await loadMoreTranscript();
        if (!loaded) {
          if (!loadingMoreTranscriptRef.current) break;
          await new Promise<void>((resolve) => window.setTimeout(resolve, 0));
          continue;
        }
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
    setLoadingSkillLinks(false);
    skillLinksRequestKeyRef.current = "";
  }, [activeSessionLinkKey]);
  const loadActiveSessionSkillLinks = useCallback(async () => {
    if (!activeSession || !loadSessionSkillLinks || !activeSessionSkillLinksKey) return;
    if (activeSession.agent === IMPORTED_SESSION_AGENT) {
      setSkillLinks([]);
      setSkillLinksKey(activeSessionSkillLinksKey);
      setLoadingSkillLinks(false);
      return;
    }
    if (loadingSkillLinks) return;
    if (skillLinksKey === activeSessionSkillLinksKey && !loadingSkillLinks) return;
    const requestKey = activeSessionSkillLinksKey;
    skillLinksRequestKeyRef.current = requestKey;
    setLoadingSkillLinks(true);
    try {
      const links = await loadSessionSkillLinks(activeSession);
      if (skillLinksRequestKeyRef.current !== requestKey) return;
      setSkillLinks(Array.isArray(links) ? links : []);
      setSkillLinksKey(requestKey);
    } finally {
      if (skillLinksRequestKeyRef.current === requestKey) setLoadingSkillLinks(false);
    }
  }, [activeSession, activeSessionSkillLinksKey, loadSessionSkillLinks, loadingSkillLinks, skillLinksKey]);
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
            <IconButton
              aria-label="Refresh sessions"
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
              <SearchField placeholder="Search sessions" value={query} onChange={(event) => setQuery(event.target.value)} onClear={() => setQuery("")} />
              <DropdownMenu.Root
                open={projectFilterOpen}
                onOpenChange={(open) => {
                  setProjectFilterOpen(open);
                  if (!open) setProjectFilterQuery("");
                }}
              >
                <DropdownMenu.Trigger asChild>
                  <button
                    type="button"
                    className="sessionProjectFilter"
                    aria-label="Filter projects"
                  >
                    <Filter size={14} aria-hidden="true" />
                    <span>Filter</span>
                    {selectedProjectKeys.length > 0 ? <span className="sessionProjectFilterCount">{selectedProjectKeys.length}</span> : null}
                  </button>
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
                  </DropdownMenu.Content>
                </DropdownMenu.Portal>
              </DropdownMenu.Root>
            </div>
          </PageHeader>
          <div className="sessionListBody" ref={sessionListBodyRef}>
            <DataTable
              rows={tableSessions}
              columns={columns}
              getRowId={sessionTableRowId}
              getRowLabel={(session) => formatSessionTitle(session.title) || session.id}
              selectable={(session) => session.agent !== IMPORTED_SESSION_AGENT && Boolean(session.path)}
              selectedIds={selectedSessionIds}
              onSelectionChange={setSelectedSessionIds}
              enableMarquee
              freezeColumn={SESSION_FREEZE_COLUMN}
              defaultSort={{ key: "updatedAt", direction: "desc" }}
              sort={activeSort}
              onSortChange={handleSortChange}
              manualSorting
              rowHeight={SESSION_TABLE_ROW_HEIGHT}
              groupBy={groupBy}
              onGroupByChange={setGroupBy}
              onRowClick={openSession}
              rowContextMenu={rowContextMenu}
              rowProps={(session) => (activeRowId === sessionTableRowId(session) ? { className: "rowSelected" } : {})}
              loading={(loadingSessions || searchingSessions) && sortedSessions.length === 0}
              loadingLabel="Loading sessions"
              emptyState={(
                <>
                  <SearchX size={22} strokeWidth={1.75} aria-hidden="true" />
                  <span>No matching sessions</span>
                </>
              )}
              bottomBar={(selectedRows) => {
                const actionRows = selectedSessionRows.length > 0 ? selectedSessionRows : selectedRows;
                const projectCount = new Set(
                  actionRows.map((session) => session.logicalProjectId).filter(Boolean),
                ).size;
                return <>
                  <button
                    type="button"
                    disabled={projectCount < 2 || projectActionBusy}
                    onClick={() => mergeSelectedProjects(actionRows)}
                  >
                    <GitMerge size={14} /> Merge projects
                  </button>
                  <button
                    type="button"
                    disabled={projectActionBusy}
                    onClick={() => beginSplitProject(actionRows)}
                  >
                    <GitFork size={14} /> Split to project
                  </button>
                </>;
              }}
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
              {importError || projectActionError ? <AppTooltip content={importError || projectActionError} onlyWhenTruncated><span className="sessionPagerError" role="alert">{importError || projectActionError}</span></AppTooltip> : null}
              <SelectControl
                className="sessionPageSizeSelect"
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
            loading={loading}
            hasMore={Boolean(nextTranscriptCursor)}
            loadingMore={loadingMoreTranscript}
            skillLinks={skillLinks}
            loadingSkillLinks={loadingSkillLinks}
            skillLinksLoaded={skillLinksKey === activeSessionSkillLinksKey}
            onCollapse={() => setDetailCollapsed(true)}
            showSessionLocator={!activeSessionVisibleInList || activeSessionInListViewport === false}
            onLocateSession={locateActiveSession}
            onResume={resumeSession}
            resumeState={getResumeState(activeSession)}
            onOpenSession={openRelatedSession}
            onOpenSkill={onOpenSkill}
            onLoadSkills={loadActiveSessionSkillLinks}
            onLoadMore={loadMoreTranscript}
            onLoadAll={loadAllTranscript}
            searchTranscript={searchTranscript}
          />
        </DetailPanelHost>
      ) : null}
    </PanelGroup>
    <DialogShell
      open={pendingSplitSessions.length > 0}
      onOpenChange={(open) => {
        if (!open && !projectActionBusy) setPendingSplitSessions([]);
      }}
      descriptionId="session-project-split-description"
    >
      <Dialog.Title className="confirmDialogTitle">Split sessions into a project</Dialog.Title>
      <p id="session-project-split-description" className="confirmDialogDescription">
        Move {pendingSplitSessions.length} selected session{pendingSplitSessions.length === 1 ? "" : "s"} into a new logical project. Their workspace paths stay unchanged.
      </p>
      <DialogTextField label="Project name" value={splitProjectName} onChange={setSplitProjectName} placeholder="Project name" />
      {projectActionError ? <div className="dialogError">{projectActionError}</div> : null}
      <DialogActionBar cancelDisabled={projectActionBusy} onCancel={() => setPendingSplitSessions([])}>
        <DialogStatefulButton
          state={projectActionBusy ? "loading" : "idle"}
          loadingLabel="Splitting sessions"
          variant="primary"
          aria-label="Create project"
          disabled={!splitProjectName.trim()}
          onClick={splitSelectedSessions}
        >
          <><GitFork size={14} /><span>Create project</span></>
        </DialogStatefulButton>
      </DialogActionBar>
    </DialogShell>
    </>
  );
}
