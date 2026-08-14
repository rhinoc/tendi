import { Tooltip as AppTooltip } from "../components/shared/Tooltip.tsx";
import { lazy, memo, Suspense, useCallback, useEffect, useMemo, useRef, useState, type ChangeEvent, type ReactNode } from "react";
import { AlertCircle, ArrowUpRight, BarChart3, Check, ChevronDown, ChevronLeft, ChevronRight, ChevronRight as ChevronRightIcon, Filter, FolderOpen, GitFork, GitMerge, Info, MessageSquareText, PanelRightClose, RefreshCw, Search, Sparkles, TerminalSquare, Upload, X } from "lucide-react";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { ContextMenu, Dialog, DropdownMenu, Popover, Tooltip } from "radix-ui";

import { DataTable } from "../components/DataTable.tsx";
import type { ColumnDef, SortState } from "../components/DataTable.types";
import type { TokenMetricProps } from "../components/TokenStatusBar.tsx";
import { AgentBadge } from "../components/shared/AgentBadge.tsx";
import { CopyButton } from "../components/shared/CopyButton.tsx";
import { CopyableSessionId } from "../components/shared/CopyableSessionId.tsx";
import { CopyPathMenuItem, CopyTextMenuItem, RevealInFinderMenuItem } from "../components/shared/DataTableMenus.tsx";
import { DetailPanelHost } from "../components/shared/DetailPanelHost.tsx";
import { DialogActionBar } from "../components/shared/DialogActionBar.tsx";
import { DialogActionButton } from "../components/shared/DialogActionButton.tsx";
import { DialogTextField } from "../components/shared/DialogTextField.tsx";
import { InfoSection } from "../components/shared/InfoSection.tsx";
import { LoadingIcon } from "../components/shared/LoadingIcon.tsx";
import { LoadingInline } from "../components/shared/LoadingInline.tsx";
import { PageHeader } from "../components/shared/PageHeader.tsx";
import { RadialConvergenceChart } from "../components/shared/RadialConvergenceChart.tsx";
import { SearchField } from "../components/shared/SearchField.tsx";
import { SelectControl } from "../components/shared/SelectControl.tsx";
import { createSessionTableColumns } from "../components/shared/createSessionTableColumns.tsx";
import { TranscriptLinkText } from "../components/shared/TranscriptLinkText.tsx";
import "./SessionsView.css";
import { cacheRateTone } from "../lib/token-style.ts";
import { supportsResponseTokenUsage, type SessionAnalyticsDetail } from "../lib/analytics.ts";
import {
  SESSION_FREEZE_COLUMN,
  TauriCommand,
  compactDateTime,
  compareSessions,
  createLatestRequestAuthority,
  dayGroupKey,
  formatDuration,
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
import type { JsonlTranscriptParseResult, SessionProjectDelta, TranscriptPage } from "../lib/index.ts";

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
  system: true,
  tool: true,
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
};

type TranscriptSearchTarget = SkillEvidenceTarget;
type AnalyticsResponse = SessionAnalyticsDetail["responses"][number];

type SessionLocatorItem = {
  key: string;
  label: string;
  response: string;
};

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
          };
        }
      }
      continue;
    }
    if (evidenceMatchesItem(item, evidenceText, evidenceTime)) {
      return { key: transcriptItemKey(transcriptItemType(item) || "item", index) };
    }
  }
  return null;
}

function responseClock(value: unknown) {
  const text = `${value ?? ""}`.trim();
  const isoClock = text.match(/T(\d{2}:\d{2})/);
  if (isoClock) return isoClock[1];
  const clock = text.match(/\b(\d{1,2}):(\d{2})\b/);
  return clock ? `${clock[1].padStart(2, "0")}:${clock[2]}` : "";
}

function clockMinutes(value: string) {
  const match = value.match(/^(\d{2}):(\d{2})$/);
  if (!match) return null;
  return Number(match[1]) * 60 + Number(match[2]);
}

function findResponseTarget(
  transcriptItems: TranscriptItemRecord[],
  response: AnalyticsResponse,
  allowApproximate = false,
): SkillEvidenceTarget | null {
  const candidates = transcriptItems.flatMap((item, index) => (
    transcriptItemType(item) === "assistant"
      ? [{ item, index }]
      : []
  ));
  if (candidates.length === 0) return null;

  const responseTime = responseClock(response.timestamp);
  if (responseTime) {
    const indexedCandidate = candidates[response.index - 1];
    if (indexedCandidate && responseClock(indexedCandidate.item.time) === responseTime) {
      return { key: transcriptItemKey("assistant", indexedCandidate.index) };
    }
    const exactCandidates = candidates.filter(({ item }) => responseClock(item.time) === responseTime);
    const exact = exactCandidates.length === 0 ? undefined : indexedCandidate
      ? exactCandidates.reduce((closest, candidate) => (
        Math.abs(candidate.index - indexedCandidate.index) < Math.abs(closest.index - indexedCandidate.index)
          ? candidate
          : closest
      ), exactCandidates[0])
      : exactCandidates[0];
    if (exact) return { key: transcriptItemKey("assistant", exact.index) };
  }

  if (!allowApproximate) return null;

  const responseMinutes = clockMinutes(responseTime);
  if (responseMinutes !== null) {
    let closest = candidates[0];
    let closestDistance = Number.POSITIVE_INFINITY;
    for (const candidate of candidates) {
      const itemMinutes = clockMinutes(responseClock(candidate.item.time));
      if (itemMinutes === null) continue;
      const distance = Math.abs(itemMinutes - responseMinutes);
      if (distance < closestDistance) {
        closest = candidate;
        closestDistance = distance;
      }
    }
    if (closestDistance < Number.POSITIVE_INFINITY) {
      return { key: transcriptItemKey("assistant", closest.index) };
    }
  }

  const fallback = candidates[Math.min(Math.max(response.index - 1, 0), candidates.length - 1)];
  return { key: transcriptItemKey("assistant", fallback.index) };
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
          });
        }
      });
      return;
    }
    if (transcriptItemText(item).includes(needle)) {
      matches.push({ key: transcriptItemKey(type || "item", index) });
    }
  });
  return matches;
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
  loadAnalytics,
}: {
  session: SessionRecord;
  parentSession?: SessionRecord;
  childSessions: SessionRecord[];
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
  loadAnalytics?: (session: SessionRecord) => Promise<SessionAnalyticsDetail | null>;
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
  const locatorItems = useMemo(() => buildSessionLocatorItems(transcriptItems), [transcriptItems]);
  const reportedSegments = useMemo(() => reportedTokenSegments(session), [session]);
  const hasReportedUsage = Boolean(session.tokenUsage);
  const cacheMetrics = useMemo(() => sessionCacheMetrics(session), [session]);
  const transcriptRef = useRef<HTMLDivElement | null>(null);
  const loadMoreRef = useRef<HTMLDivElement | null>(null);
  const transcriptItemsRef = useRef(items);
  const hasMoreRef = useRef(hasMore);
  const loadingMoreRef = useRef(loadingMore);
  transcriptItemsRef.current = transcriptItems;
  hasMoreRef.current = hasMore;
  loadingMoreRef.current = loadingMore;
  const searchInputRef = useRef<HTMLInputElement | null>(null);
  const [highlightedKey, setHighlightedKey] = useState("");
  const [searchQuery, setSearchQuery] = useState("");
  const [debouncedSearchQuery, setDebouncedSearchQuery] = useState("");
  const [searchScopes, setSearchScopes] = useState<TranscriptSearchScopeState>(DEFAULT_TRANSCRIPT_SEARCH_SCOPES);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchIndex, setSearchIndex] = useState(0);
  const [analytics, setAnalytics] = useState<SessionAnalyticsDetail | null>(null);
  const [analyticsLoading, setAnalyticsLoading] = useState(false);
  const analyticsKey = `${session.agent ?? ""}:${session.id}:${session.path ?? ""}`;
  const responseUsageAvailable = Boolean(
    loadAnalytics && session.path && supportsResponseTokenUsage(session.agent),
  );
  const analyticsRequestKeyRef = useRef(analyticsKey);
  analyticsRequestKeyRef.current = analyticsKey;
  const highlightTimerRef = useRef(0);
  const normalizedInputSearchQuery = searchQuery.trim().toLowerCase();
  const normalizedSearchQuery = debouncedSearchQuery;
  const searchTargets = useMemo(
    () => findTranscriptSearchTargets(transcriptItems, normalizedSearchQuery, searchScopes),
    [normalizedSearchQuery, searchScopes, transcriptItems],
  );
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
    setAnalytics(null);
    setAnalyticsLoading(false);
  }, [analyticsKey]);
  const loadSessionAnalytics = useCallback(async () => {
    if (analytics || analyticsLoading || !responseUsageAvailable || !loadAnalytics) return;
    const requestKey = analyticsKey;
    setAnalyticsLoading(true);
    try {
      const next = await loadAnalytics(session);
      if (analyticsRequestKeyRef.current === requestKey) setAnalytics(next);
    } finally {
      if (analyticsRequestKeyRef.current === requestKey) setAnalyticsLoading(false);
    }
  }, [analytics, analyticsKey, analyticsLoading, loadAnalytics, responseUsageAvailable, session]);
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
    if (!hasMore || loadingMore) return;
    const root = transcriptRef.current;
    const sentinel = loadMoreRef.current;
    if (!root || !sentinel) return;
    const observer = new IntersectionObserver((entries) => {
      if (entries.some((entry) => entry.isIntersecting)) void onLoadMore();
    }, { root, rootMargin: "0px 0px 320px 0px" });
    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hasMore, loadingMore, onLoadMore]);
  const focusTranscriptTarget = useCallback((target: TranscriptSearchTarget, preferSearchMatch = false, behavior: ScrollBehavior = "smooth") => {
    window.clearTimeout(highlightTimerRef.current);
    setHighlightedKey(target.key);
    window.requestAnimationFrame(() => {
      const root = transcriptRef.current;
      if (!root) return;
      if (target.groupKey) {
        const group = root.querySelector(`[data-transcript-key="${cssEscape(target.groupKey)}"]`) as HTMLDetailsElement | null;
        if (group) group.open = true;
      }
      const scrollToTarget = () => {
        const node = root.querySelector(`[data-transcript-key="${cssEscape(target.key)}"]`);
        if (!node) return;
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
      if (target.groupKey) window.requestAnimationFrame(scrollToTarget);
      else scrollToTarget();
    });
  }, []);
  const jumpToSkillEvidence = useCallback((link: SkillLinkRecord) => {
    const target = findSkillEvidenceTarget(transcriptItems, link);
    if (target) focusTranscriptTarget(target);
  }, [focusTranscriptTarget, transcriptItems]);
  const jumpToResponse = useCallback((response: AnalyticsResponse) => {
    const findTarget = (itemsToSearch: TranscriptItemRecord[], allowApproximate = false) => (
      findResponseTarget(itemsToSearch, response, allowApproximate)
    );
    const findLoadedTarget = async () => {
      let itemsToSearch = transcriptItemsRef.current;
      let target = findTarget(itemsToSearch);
      while (!target && hasMoreRef.current && !loadingMoreRef.current) {
        const loadedItems = await onLoadMore();
        if (!loadedItems) break;
        itemsToSearch = groupTranscriptItems(loadedItems) as TranscriptItemRecord[];
        target = findTarget(itemsToSearch);
      }
      return target ?? findTarget(itemsToSearch, true);
    };
    void findLoadedTarget().then((target) => {
      if (target) focusTranscriptTarget(target);
    });
    return true;
  }, [focusTranscriptTarget, onLoadMore]);
  const selectLocatorItem = useCallback((key: string, behavior?: ScrollBehavior) => {
    focusTranscriptTarget({ key }, false, behavior);
  }, [focusTranscriptTarget]);
  useEffect(() => {
    setSearchIndex(0);
  }, [normalizedSearchQuery, searchScopes, transcriptItems]);
  useEffect(() => {
    const target = searchTargets[searchIndex];
    if (target) focusTranscriptTarget(target, true);
  }, [focusTranscriptTarget, searchIndex, searchTargets]);
  const moveSearchResult = useCallback((offset: number) => {
    if (searchTargets.length === 0) return;
    setSearchIndex((current) => (current + offset + searchTargets.length) % searchTargets.length);
  }, [searchTargets.length]);
  const openLinkedSession = useCallback((sessionId: string) => {
    const linkedSession = childSessions.find((child) => child.id === sessionId);
    if (linkedSession) onOpenSession(linkedSession);
  }, [childSessions, onOpenSession]);
  return (
    <aside className="transcriptPanel">
      <header className="threadHeader">
        <div className="threadTitleLine">
          <h2>{session.title}</h2>
          <div className="threadHeaderActions">
            <AppTooltip content={resumeState === "loading"
                ? "Opening session in terminal"
                : resumeState === "success"
                  ? "Session opened in terminal"
                  : resumeState === "error"
                    ? "Could not open session in terminal"
                    : "Resume session in terminal"}><button
              className={`threadPanelToggle sessionResumeButton${resumeState === "success" ? " isSuccess" : resumeState === "error" ? " isError" : ""}`}
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
            >
              {resumeState === "loading"
                ? <LoadingIcon size={15} />
                : resumeState === "success"
                  ? <Check size={15} />
                  : resumeState === "error"
                    ? <AlertCircle size={15} />
                    : <TerminalSquare size={15} />}
            </button></AppTooltip>
            <SessionRelationsPopover
              parentSession={parentSession}
              childSessions={childSessions}
              onOpenSession={onOpenSession}
            />
            <SessionSkillsPopover
              links={skillLinks}
              loading={loadingSkillLinks}
              loaded={skillLinksLoaded}
              onLoad={onLoadSkills}
              onOpenSkill={onOpenSkill}
              onJumpToEvidence={jumpToSkillEvidence}
            />
            <SessionUsagePopover
              key={analyticsKey}
              analytics={analytics}
              loading={analyticsLoading}
              available={responseUsageAvailable}
              onLoad={loadSessionAnalytics}
              onJumpToResponse={jumpToResponse}
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
                        <span className={`selectionCheckbox ${active ? "isChecked" : ""}`} aria-hidden="true">
                          <DropdownMenu.ItemIndicator className="selectionCheckboxIndicator">
                            <Check size={12} strokeWidth={2.7} />
                          </DropdownMenu.ItemIndicator>
                        </span>
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
              aria-label={hasMore ? "Search loaded messages" : "Search messages in this session"}
              placeholder={hasMore ? "Search loaded messages" : "Search messages"}
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
            {normalizedSearchQuery ? <span className="transcriptSearchCount">{searchTargets.length ? `${searchIndex + 1}/${searchTargets.length}` : "0/0"}</span> : null}
            {normalizedSearchQuery ? (
              <>
                <button type="button" aria-label="Previous matching message" onClick={() => moveSearchResult(-1)} disabled={searchTargets.length === 0}><ChevronLeft size={14} /></button>
                <button type="button" aria-label="Next matching message" onClick={() => moveSearchResult(1)} disabled={searchTargets.length === 0}><ChevronRight size={14} /></button>
              </>
            ) : null}
            <button type="button" aria-label="Close message search" onClick={clearMessageSearch}><X size={13} /></button>
          </div>
        ) : null}
      </header>
      <div className={`transcript ${locatorItems.length >= SESSION_LOCATOR_MIN_ITEMS ? "withSessionLocator" : ""}`} ref={transcriptRef}>
        {loading ? <div className="emptyState"><LoadingInline label="Loading transcript" /></div> : transcriptItems.length > 0 ? (
          <>
            {transcriptItems.map((item, index) => {
              const itemKey = transcriptItemKey(transcriptItemType(item) === "toolGroup" ? "tool-group" : transcriptItemType(item) || "item", index);
              return (
                <div className="transcriptItemShell" key={`${transcriptItemType(item)}-${index}`}>
                  <TranscriptItem
                    item={item}
                    itemKey={itemKey}
                    highlightedKey={highlightedKey}
                    searchQuery={transcriptItemSearchQuery(item, normalizedSearchQuery, searchScopes)}
                    onOpenLinkedSession={openLinkedSession}
                  />
                </div>
              );
            })}
            {hasMore ? (
              <>
                {loadingMore ? <div className="sessionTranscriptLoadMore" aria-live="polite"><LoadingInline label="Loading more messages" /></div> : null}
                <div ref={loadMoreRef} className="sessionTranscriptLoadMoreSentinel" aria-hidden="true" />
              </>
            ) : null}
          </>
        ) : <div className="emptyState">No transcript items</div>}
      </div>
      <SessionLocator
        items={locatorItems}
        scrollRootRef={transcriptRef}
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
  scrollRootRef,
  onSelect,
}: {
  items: SessionLocatorItem[];
  scrollRootRef: { current: HTMLDivElement | null };
  onSelect: (key: string, behavior?: ScrollBehavior) => void;
}) {
  const [visibleKeys, setVisibleKeys] = useState<Set<string>>(() => new Set());
  const [previewKey, setPreviewKey] = useState("");
  const dragRef = useRef<{ pointerId: number; key: string; moved: boolean } | null>(null);
  const suppressClickRef = useRef(false);
  const itemKeys = useMemo(() => items.map((item) => item.key).join("\0"), [items]);

  useEffect(() => {
    if (items.length < SESSION_LOCATOR_MIN_ITEMS) {
      setVisibleKeys(new Set());
      return;
    }
    const root = scrollRootRef.current;
    if (!root) return;
    const visible = new Set<string>();
    const observer = new IntersectionObserver((entries) => {
      for (const entry of entries) {
        const key = (entry.target as HTMLElement).dataset.transcriptKey;
        if (!key) continue;
        if (entry.isIntersecting) visible.add(key);
        else visible.delete(key);
      }
      setVisibleKeys(new Set(visible));
    }, { root, threshold: 0 });
    for (const item of items) {
      const node = root.querySelector(`[data-transcript-key="${cssEscape(item.key)}"]`);
      if (node) observer.observe(node);
    }
    return () => {
      observer.disconnect();
    };
  }, [itemKeys, items, scrollRootRef]);

  if (items.length < SESSION_LOCATOR_MIN_ITEMS) return null;

  const locatorItemAtPoint = (x: number, y: number) => {
    const row = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-session-locator-item-id]");
    const key = row?.dataset.sessionLocatorItemId;
    return key && items.some((item) => item.key === key) ? key : "";
  };

  return (
    <nav className="sessionLocator" aria-label="User messages">
      <Tooltip.Provider delayDuration={0} skipDelayDuration={0}>
        <div
          className="sessionLocatorList"
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
          {items.map((item, index) => {
            const previewOpen = previewKey === item.key;
            return (
              <Tooltip.Root open={previewOpen} key={item.key}>
                <Tooltip.Trigger asChild>
                  <button
                    type="button"
                    className="sessionLocatorRow"
                    data-session-locator-item-id={item.key}
                    aria-current={visibleKeys.has(item.key) ? "true" : undefined}
                    aria-label={`Jump to user message ${index + 1}`}
                    onClick={() => {
                      if (suppressClickRef.current) {
                        suppressClickRef.current = false;
                        return;
                      }
                      onSelect(item.key);
                    }}
                    onFocus={() => setPreviewKey(item.key)}
                    onBlur={() => setPreviewKey("")}
                    onMouseEnter={() => setPreviewKey(item.key)}
                    onMouseLeave={() => setPreviewKey("")}
                  >
                    <span className="sessionLocatorMarker" />
                  </button>
                </Tooltip.Trigger>
                <Tooltip.Portal>
                  <Tooltip.Content
                    className="sessionLocatorPreview"
                    side="right"
                    align="center"
                    sideOffset={-6}
                    collisionPadding={8}
                  >
                    <strong>{item.label || "(No content)"}</strong>
                    {item.response ? <span>{item.response}</span> : null}
                  </Tooltip.Content>
                </Tooltip.Portal>
              </Tooltip.Root>
            );
          })}
        </div>
      </Tooltip.Provider>
    </nav>
  );
});


export function SessionUsagePopover({
  analytics,
  loading,
  available,
  onLoad,
  onJumpToResponse,
}: {
  analytics: SessionAnalyticsDetail | null;
  loading: boolean;
  available: boolean;
  onLoad: () => void;
  onJumpToResponse?: (response: AnalyticsResponse) => boolean;
}) {
  const [open, setOpen] = useState(false);
  if (!available) {
    return (
      <AppTooltip content="Response token usage unavailable for this provider"><button
        className="threadPanelToggle"
        aria-label="Response token usage unavailable"
        disabled
      >
        <BarChart3 size={15} />
      </button></AppTooltip>
    );
  }
  const responses = analytics?.responses.slice(-100).reverse() ?? [];
  const reasoningTokens = Boolean(analytics?.capabilities.reasoningTokens);
  const jumpToResponse = (response: AnalyticsResponse) => {
    if (!onJumpToResponse?.(response)) return;
    setOpen(false);
  };
  return (
    <Popover.Root open={open} onOpenChange={(nextOpen) => { setOpen(nextOpen); if (nextOpen) onLoad(); }}>
      <AppTooltip content="Response token usage">
        <Popover.Trigger asChild>
          <button
            className="threadPanelToggle"
            aria-label="Show response token usage"
          >
            <BarChart3 size={15} />
          </button>
        </Popover.Trigger>
      </AppTooltip>
      <Popover.Portal>
        <Popover.Content
          className="sessionUsagePopover"
          align="end"
          sideOffset={8}
          data-no-drag
          onMouseDown={(event) => event.stopPropagation()}
        >
          <div className="sessionUsageHeader">
            <div>
              <strong>Response usage</strong>
              <span>{analytics ? `${analytics.responses.length.toLocaleString()} responses` : "Local transcript data"}</span>
            </div>
            {analytics?.responses.length && analytics.responses.length > 100 ? <span>Latest 100 shown</span> : null}
          </div>
          {loading ? (
            <LoadingInline label="Analyzing transcript" />
          ) : analytics && !analytics.capabilities.tokenUsage ? (
            <p className="sessionUsageEmpty">This provider transcript does not expose token usage.</p>
          ) : responses.length ? (
            <div className="sessionUsageTableWrap">
              <table className="sessionUsageTable">
                <thead>
                  <tr>
                    <th>#</th><th>Time</th><th>Model</th><th>Input</th><th>Cached</th><th>Output</th><th>Reasoning</th><th>Total</th><th>Cumulative</th>
                  </tr>
                </thead>
                <tbody>
                  {responses.map((response) => (
                    <tr
                      key={`${response.index}:${response.timestamp}`}
                      className={onJumpToResponse ? "sessionUsageRowClickable" : undefined}
                      role={onJumpToResponse ? "button" : undefined}
                      tabIndex={onJumpToResponse ? 0 : undefined}
                      aria-label={onJumpToResponse ? `Jump to response ${response.index}` : undefined}
                      onClick={onJumpToResponse ? () => jumpToResponse(response) : undefined}
                      onKeyDown={onJumpToResponse ? (event) => {
                        if (event.key !== "Enter" && event.key !== " ") return;
                        event.preventDefault();
                        jumpToResponse(response);
                      } : undefined}
                    >
                      <td>{response.index}</td>
                      <td>{compactDateTime(response.timestamp)}</td>
                      <AppTooltip content={response.model} onlyWhenTruncated><td>{response.model || "—"}</td></AppTooltip>
                      <td>{response.usage.inputTokens.toLocaleString()}</td>
                      <td>{response.usage.cachedInputTokens.toLocaleString()}</td>
                      <td>{response.usage.outputTokens.toLocaleString()}</td>
                      <td>{reasoningTokens ? response.usage.reasoningOutputTokens.toLocaleString() : "—"}</td>
                      <td>{response.usage.totalTokens.toLocaleString()}</td>
                      <td>{response.cumulative.totalTokens.toLocaleString()}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : (
            <p className="sessionUsageEmpty">{analytics ? "No response usage found." : "Usage could not be loaded."}</p>
          )}
          {analytics?.malformedLines ? <p className="sessionUsageNote">Skipped {analytics.malformedLines} malformed lines.</p> : null}
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}


export function SessionRelationsPopover({
  parentSession,
  childSessions,
  onOpenSession,
}: {
  parentSession?: SessionRecord;
  childSessions: SessionRecord[];
  onOpenSession: (session: SessionRecord) => void;
}) {
  const relationCount = childSessions.length + (parentSession ? 1 : 0);
  if (relationCount === 0) return null;

  const relationButton = (session: SessionRecord, role: "Parent" | "Child") => (
    <Popover.Close asChild key={`${role}-${sessionKey(session)}`}>
      <AppTooltip content={`Open ${role.toLowerCase()} session ${session.id}`}><button
        type="button"
        className="sessionRelationButton"
        onClick={() => onOpenSession(session)}
      >
        <span className="sessionRelationRole">{role}</span>
        <span className="sessionRelationText">
          <strong>{session.title}</strong>
          <code>{session.id}</code>
        </span>
        <ChevronRight size={14} aria-hidden="true" />
      </button></AppTooltip>
    </Popover.Close>
  );

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
          <SessionRelationsConvergence parentSession={parentSession} childSessions={childSessions} />
          <div className="sessionRelationsList">
            {parentSession ? relationButton(parentSession, "Parent") : null}
            {childSessions.map((session) => relationButton(session, "Child"))}
          </div>
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}


export function SessionSkillsPopover({
  links = [],
  loading = false,
  loaded = false,
  onLoad,
  onOpenSkill,
  onJumpToEvidence,
}: {
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
      <AppTooltip content="Skills used">
        <Popover.Trigger asChild>
          <button className="threadPanelToggle" aria-label="Show skills used">
            <Sparkles size={15} />
          </button>
        </Popover.Trigger>
      </AppTooltip>
      <Popover.Portal>
        <Popover.Content
          className={`sessionSkillsPopover${links.length > 0 ? " hasChart" : ""}`}
          align="end"
          sideOffset={8}
          data-no-drag
          onMouseDown={(event) => event.stopPropagation()}
        >
          <SessionSkillsUsed
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


function SessionSkillsConvergence({ links }: { links: SkillLinkRecord[] }) {
  const nodes = links.map((link, index) => {
    const name = linkSkillName(link) || "Unnamed skill";
    return {
      key: `${link.skill_path ?? link.skillPath ?? name}-${index}`,
      label: name,
    };
  });
  return (
    <RadialConvergenceChart
      nodes={nodes}
      centerLabel="SKILLS"
      ariaLabel={`${links.length} skills used in this session`}
    />
  );
}


function SessionRelationsConvergence({
  parentSession,
  childSessions,
}: {
  parentSession?: SessionRecord;
  childSessions: SessionRecord[];
}) {
  const nodes = [
    ...(parentSession ? [{
      key: `parent-${sessionKey(parentSession)}`,
      label: `Parent · ${parentSession.title || parentSession.id || "Untitled session"}`,
    }] : []),
    ...childSessions.map((session) => ({
      key: `child-${sessionKey(session)}`,
      label: `Child · ${session.title || session.id || "Untitled session"}`,
    })),
  ];
  return (
    <RadialConvergenceChart
      nodes={nodes}
      centerLabel="LINKED"
      ariaLabel={`${nodes.length} related sessions linked to this session`}
    />
  );
}


export function SessionSkillsUsed({
  links = [],
  loading = false,
  onOpenSkill,
  onJumpToEvidence,
}: {
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
          <SessionSkillsConvergence links={links} />
          <div className="sessionSkillChips">
            {links.map((link) => (
              <div
                className="sessionSkillChip"
                key={`${link.skill_path ?? link.skillPath ?? linkSkillName(link)}`}
              >
                <AppTooltip content={linkEvidenceText(link)} onlyWhenTruncated><span>{linkSkillName(link)}</span></AppTooltip>
                <div className="sessionSkillChipActions">
                  <AppTooltip content="Open skill"><button
                    type="button"
                    aria-label={`Open ${linkSkillName(link)} skill`}
                    onClick={() => onOpenSkill?.(linkSkillName(link))}
                  >
                    <Sparkles size={12} />
                  </button></AppTooltip>
                  <AppTooltip content="Go to session message"><button
                    type="button"
                    aria-label={`Go to ${linkSkillName(link)} usage in transcript`}
                    onClick={() => onJumpToEvidence?.(link)}
                  >
                    <MessageSquareText size={12} />
                  </button></AppTooltip>
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
    <DropdownMenu.Root>
      <DropdownMenu.Trigger asChild>
        <button className="threadPanelToggle" aria-label="Show session info">
          <Info size={15} />
        </button>
      </DropdownMenu.Trigger>
      <DropdownMenu.Portal>
        <DropdownMenu.Content className="skillInfoContent sessionInfoContent" align="end" sideOffset={8} data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <div className="skillInfoHeader">
            <span>Session info</span>
            <strong>{session.title}</strong>
          </div>
          <div className="skillInfoSections">
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
          </div>
        </DropdownMenu.Content>
      </DropdownMenu.Portal>
    </DropdownMenu.Root>
  );
}


type TranscriptItemProps = {
  item: TranscriptItemRecord;
  itemKey: string;
  highlightedKey: string;
  searchQuery: string;
  onOpenLinkedSession?: (sessionId: string) => void;
};

function transcriptHighlightState(props: TranscriptItemProps) {
  const type = transcriptItemType(props.item);
  if (type !== "toolGroup") return props.highlightedKey === props.itemKey;
  const groupIndex = props.itemKey.slice("tool-group-".length);
  return props.highlightedKey === props.itemKey
    || props.highlightedKey.startsWith(`tool-${groupIndex}-`);
}

export const TranscriptItem = memo(function TranscriptItem({
  item,
  itemKey,
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
    <div className={`chatLine ${isUser ? "fromUser" : "fromAgent"} ${highlighted ? "transcriptTarget" : ""}`} data-transcript-key={itemKey}>
      <div className="bubble">
        <p><TranscriptLinkText query={searchQuery} value={body} /></p>
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
          <AppTooltip content="Open child session"><button
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
          </button></AppTooltip>
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
  loadTranscript,
  loadSessionAnalytics,
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
  loadTranscript: (session: SessionRecord, cursor?: string, knownSourceVersion?: string) => Promise<TranscriptPage>;
  loadSessionAnalytics?: (session: SessionRecord) => Promise<SessionAnalyticsDetail | null>;
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
  const importWorkerRef = useRef<{ worker: Worker; cancel: () => void } | null>(null);
  const projectFilterInputRef = useRef<HTMLInputElement | null>(null);
  const importFeedbackTimerRef = useRef<number | undefined>(undefined);
  const resumeFeedbackTimerRef = useRef<Record<string, number>>({});
  const skillLinksRequestKeyRef = useRef("");
  const transcriptRequestAuthorityRef = useRef(createLatestRequestAuthority());
  const transcriptCacheRef = useRef(new Map<string, TranscriptPage>());
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
    if (activeRowId && !sortedSessions.some((session) => sessionTableRowId(session) === activeRowId)) setActiveRowId("");
  }, [activeRowId, sortedSessions]);
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
        .map((item) => `${item.body ?? ""}`.trim())
        .filter(Boolean);
      const assistantMessages = parsed.items
        .filter((item) => transcriptItemType(item) === "assistant")
        .map((item) => `${item.body ?? ""}`.trim())
        .filter(Boolean);
      const session: SessionRecord = {
        id,
        title: parsed.title || file.name,
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
        <ContextMenu.Separator className="skillMenuSeparator" />
        <CopyTextMenuItem Menu={ContextMenu} text={session.id} label="Copy session ID" />
        <ContextMenu.Separator className="skillMenuSeparator" />
        <CopyPathMenuItem Menu={ContextMenu} path={transcriptPath} label="Copy transcript path" />
        <RevealInFinderMenuItem Menu={ContextMenu} path={transcriptPath} label="Reveal transcript in Finder" />
        <ContextMenu.Separator className="skillMenuSeparator" />
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
            <AppTooltip content={importButtonLabel}><button
              className={`iconButton sessionImportButton ${importButtonStateClass}`}
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
            </button></AppTooltip>
            <input
              ref={importInputRef}
              className="sessionImportInput"
              type="file"
              accept=".jsonl,application/jsonl,application/x-ndjson,text/plain"
              onChange={handleImportJsonlChange}
            />
            <button
              className="iconButton"
              aria-label="Refresh sessions"
              onClick={refreshSessions}
              disabled={refreshing}
            >
              {refreshing ? <LoadingIcon size={16} /> : <RefreshCw size={16} />}
            </button>
            <AppTooltip content={showChildSessions ? "Hide child sessions" : `Show ${childSessionCount} child sessions`}><button
              className={`iconButton ${showChildSessions ? "filled" : ""}`}
              aria-label={showChildSessions ? "Hide child sessions" : `Show ${childSessionCount} child sessions`}
              aria-pressed={showChildSessions}
              onClick={() => setShowChildSessions((visible) => !visible)}
            >
              <GitFork size={16} />
            </button></AppTooltip>
            <div className="sessionSearchControls">
              <SearchField placeholder="Search sessions" value={query} onChange={(event) => setQuery(event.target.value)} />
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
                            <span className={`selectionCheckbox ${active ? "isChecked" : ""}`} aria-hidden="true">
                              <DropdownMenu.ItemIndicator className="selectionCheckboxIndicator">
                                <Check size={12} strokeWidth={2.7} />
                              </DropdownMenu.ItemIndicator>
                            </span>
                            {isWebSource(option.title.trim())
                              ? <span className="sessionProjectFilterItemLabel">{option.label}</span>
                              : <AppTooltip content={option.title}><span className="sessionProjectFilterItemLabel">{option.label}</span></AppTooltip>}
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
          <div className="sessionListBody">
            <DataTable
              rows={tableSessions}
              columns={columns}
              getRowId={sessionTableRowId}
              getRowLabel={(session) => session.title}
              selectable={(session) => session.agent !== IMPORTED_SESSION_AGENT && Boolean(session.path)}
              selectedIds={selectedSessionIds}
              onSelectionChange={setSelectedSessionIds}
              enableMarquee
              freezeColumn={SESSION_FREEZE_COLUMN}
              defaultSort={{ key: "updatedAt", direction: "desc" }}
              sort={activeSort}
              onSortChange={handleSortChange}
              manualSorting
              rowHeight={72}
              groupBy={groupBy}
              onGroupByChange={setGroupBy}
              onRowClick={openSession}
              rowContextMenu={rowContextMenu}
              rowProps={(session) => (activeRowId === sessionTableRowId(session) ? { className: "rowSelected" } : {})}
              loading={(loadingSessions || searchingSessions) && sortedSessions.length === 0}
              loadingLabel={<LoadingInline label="Loading sessions" />}
              emptyState="No matching sessions. Clear filters or adjust your search."
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
              <button
                className="iconButton"
                aria-label="Previous page"
                onClick={() => setCurrentPage((page) => Math.max(0, page - 1))}
                disabled={currentPage === 0}
              >
                <ChevronLeft size={15} />
              </button>
              <span>{currentPage + 1} / {pageCount}</span>
              <button
                className="iconButton"
                aria-label="Next page"
                onClick={() => setCurrentPage((page) => Math.min(pageCount - 1, page + 1))}
                disabled={currentPage >= pageCount - 1}
              >
                <ChevronRight size={15} />
              </button>
            </div>
          </div>
        </div>
      </Panel>
      {activeSession ? (
        <DetailPanelHost
          collapsed={detailCollapsed}
          onExpand={() => setDetailCollapsed(false)}
          expandLabel="Expand session detail"
          railLabel={activeSession.title}
          hasSelection
          emptyState={null}
          expandedDefaultSize="38%"
          panelClassName="transcriptPanel"
        >
          <TranscriptPanel
            session={activeSession}
            parentSession={activeParentSession}
            childSessions={activeChildSessions}
            items={items}
            loading={loading}
            hasMore={Boolean(nextTranscriptCursor)}
            loadingMore={loadingMoreTranscript}
            skillLinks={skillLinks}
            loadingSkillLinks={loadingSkillLinks}
            skillLinksLoaded={skillLinksKey === activeSessionSkillLinksKey}
            onCollapse={() => setDetailCollapsed(true)}
            onResume={resumeSession}
            resumeState={getResumeState(activeSession)}
            onOpenSession={openRelatedSession}
            onOpenSkill={onOpenSkill}
            onLoadSkills={loadActiveSessionSkillLinks}
            onLoadMore={loadMoreTranscript}
            loadAnalytics={loadSessionAnalytics}
          />
        </DetailPanelHost>
      ) : null}
    </PanelGroup>
    <Dialog.Root
      open={pendingSplitSessions.length > 0}
      onOpenChange={(open) => {
        if (!open && !projectActionBusy) setPendingSplitSessions([]);
      }}
    >
      <Dialog.Portal>
        <Dialog.Overlay className="dialogOverlay" />
        <Dialog.Content className="confirmDialogPanel" aria-describedby="session-project-split-description" data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <Dialog.Title className="confirmDialogTitle">Split sessions into a project</Dialog.Title>
          <p id="session-project-split-description" className="confirmDialogDescription">
            Move {pendingSplitSessions.length} selected session{pendingSplitSessions.length === 1 ? "" : "s"} into a new logical project. Their workspace paths stay unchanged.
          </p>
          <DialogTextField label="Project name" value={splitProjectName} onChange={setSplitProjectName} placeholder="Project name" />
          {projectActionError ? <div className="addSkillError">{projectActionError}</div> : null}
          <DialogActionBar cancelDisabled={projectActionBusy} onCancel={() => setPendingSplitSessions([])}>
            <DialogActionButton variant="primary" disabled={!splitProjectName.trim() || projectActionBusy} onClick={splitSelectedSessions}>
              {projectActionBusy ? <LoadingIcon size={14} /> : <GitFork size={14} />}
              {projectActionBusy ? "Splitting" : "Create project"}
            </DialogActionButton>
          </DialogActionBar>
        </Dialog.Content>
      </Dialog.Portal>
    </Dialog.Root>
    </>
  );
}
