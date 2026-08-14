import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { Group as PanelGroup, Panel } from "react-resizable-panels";
import { Accordion, Checkbox, ContextMenu, Dialog, DropdownMenu, Select, ToggleGroup } from "radix-ui";
import {
  ArrowLeft,
  Bot,
  Bold,
  Check,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleDot,
  Code,
  Code2,
  Columns3,
  Copy,
  ExternalLink,
  FilePlus,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  GitBranch,
  Globe,
  Hammer,
  Heading1,
  Info,
  Italic,
  Link,
  List,
  ListOrdered,
  ListTodo,
  Minus,
  MoreHorizontal,
  PackagePlus,
  Pencil,
  PanelLeftClose,
  PanelLeftOpen,
  PanelRightClose,
  PanelRightOpen,
  Plus,
  Quote,
  RefreshCw,
  Rows3,
  Search,
  Save,
  Settings,
  Share2,
  ShieldCheck,
  Sparkles,
  Table as TableIcon,
  TableColumnsSplit,
  TableRowsSplit,
  TerminalSquare,
  Trash2,
  Unlink,
  Wrench,
  X,
} from "lucide-react";
import antigravityIcon from "@lobehub/icons-static-svg/icons/antigravity-color.svg";
import claudeIcon from "@lobehub/icons-static-svg/icons/claude-color.svg";
import clineIcon from "@lobehub/icons-static-svg/icons/cline.svg";
import codebuddyIcon from "@lobehub/icons-static-svg/icons/codebuddy-color.svg";
import codexIcon from "@lobehub/icons-static-svg/icons/codex-color.svg";
import githubIcon from "@lobehub/icons-static-svg/icons/github.svg";
import copilotIcon from "@lobehub/icons-static-svg/icons/githubcopilot.svg";
import cursorIcon from "@lobehub/icons-static-svg/icons/cursor.svg";
import geminiCliIcon from "@lobehub/icons-static-svg/icons/geminicli-color.svg";
import hermesIcon from "@lobehub/icons-static-svg/icons/hermesagent.svg";
import kimiIcon from "@lobehub/icons-static-svg/icons/kimi-color.svg";
import opencodeIcon from "@lobehub/icons-static-svg/icons/opencode.svg";
import qoderIcon from "@lobehub/icons-static-svg/icons/qoder-color.svg";
import qwenIcon from "@lobehub/icons-static-svg/icons/qwen-color.svg";
import stepfunIcon from "@lobehub/icons-static-svg/icons/stepfun-color.svg";
import traeIcon from "@lobehub/icons-static-svg/icons/trae-color.svg";

import type { ComponentProps } from "react";
import type { RuntimeData } from "./lib/data.ts";
import { applyAppearance, listenForSystemAppearanceChange, normalizeAppearance, normalizeThemePreferences, readCachedAppearance, readCachedThemePreferences, type Appearance, type ColorTheme, type ThemePreferences } from "./lib/appearance.ts";
import { COLLAPSED_SIDEBAR_SIZE, SIDEBAR_SIZE, SkillChangeCommand, SkillVisibility, TauriCommand, applySessionDelta, applySessionProjectDelta, applySkillUpdateReports, applyVisibilityState, clearSkillUpdateAvailability, fallbackAgents, hookSourcePath, hookTrustHash, initialData, mergeSessionRows, mergeSkillListPreservingUpdates, navItems, normalizeDomainRows, normalizeReport, normalizeSession, normalizeTranscriptPage, safeInvoke, sameAgent, sessionIdentity, sessionLaunchPayload, sessionLogicalIdentity } from "./lib/index.ts";
import type { BundledSkillStatus, CliInstallStatus, SessionAnalyticsDetail, SessionIdentityRecord, SessionProjectDelta, TranscriptPage } from "./lib/index.ts";
import { mcpColumns } from "./lib/tableColumns.tsx";
import { PlaceholderView } from "./components/shared/PlaceholderView.tsx";
import { Sidebar } from "./components/shared/Sidebar.tsx";
import { SkillEditorView } from "./components/shared/SkillEditorView.tsx";
import type { SessionRecord } from "./views/SessionsView.tsx";
import type { SkillInstallResult, SkillRecord } from "./views/SkillsView.tsx";
import type { OverviewNavId } from "./views/OverviewView.tsx";

type ViewId =
  | "overview"
  | "skills"
  | "prompts"
  | "sessions"
  | "rules"
  | "hooks"
  | "mcp"
  | "config"
  | "settings"
  | "skillDetail"
  | "ruleDetail";

const loadConfirmSkillChangesDialog = () => import("./components/shared/ConfirmSkillChangesDialog.tsx");
const loadBundledSkillInstallDialog = () => import("./components/shared/BundledSkillInstallDialog.tsx");
const loadRuleEditorView = () => import("./components/shared/RuleEditorView.tsx");
const loadSettingsView = () => import("./components/shared/SettingsView.tsx");
const loadDataListView = () => import("./views/McpView.tsx");
const loadHooksView = () => import("./views/HooksView.tsx");
const loadConfigView = () => import("./views/ConfigView.tsx");
const loadPromptsView = () => import("./views/PromptsView.tsx");
const loadRulesView = () => import("./views/RulesView.tsx");
const loadSessionsView = () => import("./views/SessionsView.tsx");
const loadSkillsView = () => import("./views/SkillsView.tsx");
const loadOverviewView = () => import("./views/OverviewView.tsx");

const ConfirmSkillChangesDialog = lazy(() => loadConfirmSkillChangesDialog().then(({ ConfirmSkillChangesDialog: component }) => ({ default: component })));
const BundledSkillInstallDialog = lazy(() => loadBundledSkillInstallDialog().then(({ BundledSkillInstallDialog: component }) => ({ default: component })));
const RuleEditorView = lazy(() => loadRuleEditorView().then(({ RuleEditorView: component }) => ({ default: component })));
const SettingsView = lazy(() => loadSettingsView().then(({ SettingsView: component }) => ({ default: component })));
const DataListView = lazy(() => loadDataListView().then(({ DataListView: component }) => ({ default: component })));
const HooksView = lazy(() => loadHooksView().then(({ HooksView: component }) => ({ default: component })));
const ConfigView = lazy(() => loadConfigView().then(({ ConfigView: component }) => ({ default: component })));
const PromptsView = lazy(() => loadPromptsView().then(({ PromptsView: component }) => ({ default: component })));
const RulesView = lazy(() => loadRulesView().then(({ RulesView: component }) => ({ default: component })));
const SessionsView = lazy(() => loadSessionsView().then(({ SessionsView: component }) => ({ default: component })));
const SkillsView = lazy(() => loadSkillsView().then(({ SkillsView: component }) => ({ default: component })));
const OverviewView = lazy(() => loadOverviewView().then(({ OverviewView: component }) => ({ default: component })));

const viewPreloaders: Partial<Record<ViewId, () => Promise<unknown>>> = {
  overview: loadOverviewView,
  skills: loadSkillsView,
  prompts: loadPromptsView,
  sessions: loadSessionsView,
  rules: loadRulesView,
  hooks: loadHooksView,
  mcp: loadDataListView,
  config: loadConfigView,
  settings: loadSettingsView,
  ruleDetail: loadRuleEditorView,
};

const SESSION_EVENT_FLUSH_MS = 200;


type SkillPreview = {
  summary?: string;
  plan?: Record<string, unknown>;
  previewId?: string;
  refreshRequired?: boolean;
  skills?: SkillRecord[];
};
type SkillUpdateReport = { name: string; status: string };
type SkillRefreshResult = { skills: SkillRecord[]; updateCheck?: string };
type SkillUpdateCheckEvent = {
  status: "completed" | "failed";
  skills?: SkillRecord[];
  updates?: SkillUpdateReport[];
  error?: string | null;
};

type SkillIndexStatus = {
  indexed?: number;
  failed?: number;
  running?: boolean;
};

type SessionScanEvent = {
  generation: number;
  phase: "recent" | "backfill" | "watch" | "error";
  upserts?: Array<Record<string, unknown>>;
  deleted?: SessionIdentityRecord[];
  scanned?: number;
  complete?: boolean;
  error?: string | null;
};

type PendingSkillChange = {
  command: SkillChangeCommand;
  args: Record<string, unknown>;
  preview?: SkillPreview | null;
  previewError?: string;
  applyError?: string;
  onApplied?: () => void;
};

type DomainRow = Record<string, unknown>;

type DomainKey = "skills" | "prompts" | "sessions" | "rules" | "hooks" | "mcp";

function hookDeleteArgs(hook: DomainRow) {
  return {
    path: hookSourcePath(hook),
    expectedTrustHash: hookTrustHash(hook),
    event: hook.event ?? "",
    matcher: hook.matcher ?? null,
    hookType: hook.hook_type ?? hook.hookType ?? null,
    command: hook.command ?? hook.script ?? null,
    url: hook.url ?? null,
    prompt: hook.prompt ?? null,
    filter: hook.filter ?? null,
    statusMessage: hook.status_message ?? hook.statusMessage ?? null,
  };
}

const domainListCommands: Record<DomainKey, TauriCommand> = {
  skills: TauriCommand.SkillsList,
  prompts: TauriCommand.PromptsList,
  sessions: TauriCommand.SessionsList,
  rules: TauriCommand.RulesList,
  hooks: TauriCommand.HooksList,
  mcp: TauriCommand.McpList,
};

function isDomainKey(value: string): value is DomainKey {
  return value in domainListCommands;
}

function deletePreviewRelatedNames(preview: SkillPreview | null | undefined) {
  const plan = (preview as Record<string, unknown> | null | undefined)?.plan as Record<string, unknown> | undefined;
  return ["dependencies", "dependents"].flatMap((key) => {
    const relations = plan?.[key];
    if (!Array.isArray(relations)) return [];
    return relations.flatMap((relation) => {
      const related = (relation as { related?: unknown }).related;
      return Array.isArray(related) ? related.map((name) => `${name}`) : [];
    });
  });
}

export function App() {
  const [view, setView] = useState<ViewId>("overview");
  const [activeSkill, setActiveSkill] = useState<SkillRecord | null>(null);
  const [activeRule, setActiveRule] = useState<DomainRow | null>(null);
  const [activeSessionKey, setActiveSessionKey] = useState("");
  const [skillIndexStatus, setSkillIndexStatus] = useState<SkillIndexStatus | null>(null);
  const [data, setData] = useState<RuntimeData>(() => initialData());
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [agentFilter, setAgentFilter] = useState("All");
  const [pendingSkillChange, setPendingSkillChange] = useState<PendingSkillChange | null>(null);
  const [bundledSkillPrompt, setBundledSkillPrompt] = useState<BundledSkillStatus | null>(null);
  const [bundledSkillBusy, setBundledSkillBusy] = useState(false);
  const [bundledSkillError, setBundledSkillError] = useState("");
  const [applyingSkillChange, setApplyingSkillChange] = useState(false);
  const [applyingSkillUpdates, setApplyingSkillUpdates] = useState(false);
  const [skillUpdateError, setSkillUpdateError] = useState("");
  const [checkingSkillUpdates, setCheckingSkillUpdates] = useState(false);
  const [analyticsRevision, setAnalyticsRevision] = useState(0);
  const [analyticsRevisionReady, setAnalyticsRevisionReady] = useState(false);
  const [loadingDomains, setLoadingDomains] = useState(() => new Set<string>(["skills"]));
  const [appearance, setAppearance] = useState<Appearance>(() => readCachedAppearance());
  const [themePreferences, setThemePreferences] = useState<ThemePreferences>(() => readCachedThemePreferences());
  const appearanceChangeRevision = useRef(0);
  const loadedDomains = useRef(new Set<string>());
  const domainLoadInFlight = useRef(new Map<DomainKey, Promise<void>>());
  const promptsRefreshInFlight = useRef<Promise<unknown[] | null> | null>(null);
  const sessionsRefreshInFlight = useRef<Promise<unknown> | null>(null);
  const sessionEventReady = useRef<Promise<void>>(Promise.resolve());
  const sessionEventUnlisten = useRef<UnlistenFn | null>(null);
  const sessionEventFlushTimer = useRef<number | undefined>(undefined);
  const sessionScanGeneration = useRef(0);
  const completedSessionScans = useRef(new Set<number>());
  const sessionScanWaiters = useRef(new Map<number, Array<() => void>>());
  const pendingRecentSessions = useRef(new Map<string, SessionRecord>());
  const pendingWatchSessions = useRef(new Map<string, SessionRecord>());
  const pendingDeletedSessions = useRef(new Map<string, SessionIdentityRecord>());
  const currentView = useRef<ViewId>(view);
  const skillListRevision = useRef(0);
  const skillUpdateCheckRevision = useRef(0);
  const prefetchedViews = useRef(new Set<ViewId>());
  const prefetchTimers = useRef(new Map<ViewId, number>());
  const activeNav = navItems.find((item) => item.id === view);
  const filteredData = useMemo(() => {
    if (agentFilter === "All") return data;
    return {
      ...data,
      skills: data.skills.filter((skill) => {
        const agents = skill.agents;
        return Array.isArray(agents) && agents.some((agent) => sameAgent(agent, agentFilter));
      }),
      prompts: data.prompts,
      sessions: data.sessions.filter((session) => sameAgent(session.agent, agentFilter)),
      rules: data.rules.filter((rule) => sameAgent(rule.agent, agentFilter)),
      hooks: data.hooks.filter((hook) => sameAgent(hook.agent, agentFilter)),
      mcp: data.mcp.filter((server) => sameAgent(server.agent, agentFilter)),
    };
  }, [agentFilter, data]);

  useEffect(() => {
    applyAppearance(appearance, themePreferences);
    if (appearance !== "system") return;
    return listenForSystemAppearanceChange(() => applyAppearance("system", themePreferences));
  }, [appearance, themePreferences]);

  const changeAppearance = useCallback((nextAppearance: Appearance) => {
    appearanceChangeRevision.current += 1;
    setAppearance(nextAppearance);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const revision = appearanceChangeRevision.current;
    safeInvoke<{ appearance?: unknown; lightTheme?: unknown; darkTheme?: unknown }>(TauriCommand.SettingsGet).then((settings) => {
      if (!cancelled && settings && revision === appearanceChangeRevision.current) {
        setAppearance(normalizeAppearance(settings.appearance));
        setThemePreferences(normalizeThemePreferences({ light: settings.lightTheme as ColorTheme, dark: settings.darkTheme as ColorTheme }));
      }
    });
    return () => { cancelled = true; };
  }, []);

  const refreshSkillList = async ({ preserveUpdates = true }: { preserveUpdates?: boolean } = {}) => {
    const revision = ++skillListRevision.current;
    const skills = await safeInvoke(TauriCommand.SkillsList);
    if (revision !== skillListRevision.current || !skills) return null;
    setData((current) => {
      const normalized = normalizeReport({ ...current, skills }, { fallback: false });
      return preserveUpdates
        ? {
            ...normalized,
            skills: mergeSkillListPreservingUpdates(current.skills, normalized.skills),
          }
        : normalized;
    });
    loadedDomains.current.add("skills");
    return skills;
  };

  const refreshSkillListAndUpdates = async () => {
    const revision = ++skillListRevision.current;
    setCheckingSkillUpdates(true);
    const result = await safeInvoke<SkillRefreshResult>(TauriCommand.SkillsRefresh);
    if (revision !== skillListRevision.current || !result) {
      if (revision === skillListRevision.current) setCheckingSkillUpdates(false);
      return null;
    }
    setData((current) => normalizeReport({ ...current, skills: result.skills }, { fallback: false }));
    loadedDomains.current.add("skills");
    if (result.updateCheck !== "started" && result.updateCheck !== "already-running") {
      setCheckingSkillUpdates(false);
    }
    return result;
  };

  const applyInstalledSkills = (result: SkillInstallResult) => {
    if (!result.skills) return;
    setData((current) => normalizeReport({ ...current, skills: result.skills }, { fallback: false }));
  };

  const refreshSkillUpdates = async () => {
    const revision = ++skillUpdateCheckRevision.current;
    setCheckingSkillUpdates(true);
    const result = await safeInvoke<{ updateCheck?: string }>(TauriCommand.SkillsUpdates, { check: true });
    if (revision !== skillUpdateCheckRevision.current || !result) {
      if (revision === skillUpdateCheckRevision.current) setCheckingSkillUpdates(false);
      return;
    }
    if (result.updateCheck !== "started" && result.updateCheck !== "already-running") {
      setCheckingSkillUpdates(false);
    }
  };

  const refreshSkills = async () => {
    setDomainLoading("skills", true);
    try {
      await refreshSkillListAndUpdates();
    } finally {
      setDomainLoading("skills", false);
    }
  };

  const setDomainRows = (domain: DomainKey, rows: unknown[]) => {
    setData((current) => normalizeDomainRows(current, domain, rows));
    loadedDomains.current.add(domain);
  };

  const refreshPrompts = useCallback(async () => {
    if (!promptsRefreshInFlight.current) {
      promptsRefreshInFlight.current = (async () => {
        const rows = await safeInvoke(TauriCommand.PromptsList) as unknown[] | null;
        if (rows) setDomainRows("prompts", rows);
        return rows;
      })().finally(() => {
        promptsRefreshInFlight.current = null;
      });
    }
    return promptsRefreshInFlight.current;
  }, []);

  const applyPromptSaved = useCallback((prompt: unknown) => {
    if (!prompt || typeof prompt !== "object") return;
    const id = `${(prompt as Record<string, unknown>).id ?? ""}`;
    setData((current) => normalizeReport({
      ...current,
      prompts: [prompt, ...current.prompts.filter((item) => item.id !== id)],
    }, { fallback: false }));
    loadedDomains.current.add("prompts");
  }, []);

  const removePrompts = useCallback((ids: string[]) => {
    const deleted = new Set(ids);
    setData((current) => ({
      ...current,
      prompts: current.prompts.filter((prompt) => !deleted.has(prompt.id)),
    }));
  }, []);

  const deleteHook = useCallback(async (hook: DomainRow) => {
    try {
      const rows = await invoke(TauriCommand.HookDelete, hookDeleteArgs(hook));
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      console.warn(`tendi command failed: ${TauriCommand.HookDelete}`, error);
      return { error: `${error}` };
    }
  }, []);

  const deleteHooks = useCallback(async (hooks: DomainRow[]) => {
    try {
      const rows = await invoke(TauriCommand.HookDeleteMany, {
        requests: hooks.map(hookDeleteArgs),
      });
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      console.warn(`tendi command failed: ${TauriCommand.HookDeleteMany}`, error);
      return { error: `${error}` };
    }
  }, []);

  const setHookEnabled = useCallback(async (hook: DomainRow, enabled: boolean) => {
    try {
      const rows = await invoke(TauriCommand.HookSetEnabled, {
        path: hookSourcePath(hook),
        expectedTrustHash: hookTrustHash(hook),
        event: hook.event ?? "",
        matcher: hook.matcher ?? null,
        hookType: hook.hook_type ?? hook.hookType ?? null,
        command: hook.command ?? hook.script ?? null,
        url: hook.url ?? null,
        prompt: hook.prompt ?? null,
        filter: hook.filter ?? null,
        statusMessage: hook.status_message ?? hook.statusMessage ?? null,
        enabled,
      });
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      console.warn(`tendi command failed: ${TauriCommand.HookSetEnabled}`, error);
      return { error: `${error}` };
    }
  }, []);

  const reviewHook = useCallback(async (hook: DomainRow) => {
    try {
      const rows = await invoke(TauriCommand.HookReview, {
        path: hookSourcePath(hook),
        expectedTrustHash: hookTrustHash(hook),
        event: hook.event ?? "",
        matcher: hook.matcher ?? null,
        hookType: hook.hook_type ?? hook.hookType ?? null,
        command: hook.command ?? hook.script ?? null,
        url: hook.url ?? null,
        prompt: hook.prompt ?? null,
        filter: hook.filter ?? null,
        statusMessage: hook.status_message ?? hook.statusMessage ?? null,
      });
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      console.warn(`tendi command failed: ${TauriCommand.HookReview}`, error);
      return { error: `${error}` };
    }
  }, []);

  const setSessionRows = useCallback((rows: unknown[], { markLoaded = true } = {}) => {
    setData((current) => {
      const nextSessions = mergeSessionRows(current.sessions, rows as Array<Record<string, unknown>>);
      if (nextSessions === current.sessions) return current;
      return { ...current, sessions: nextSessions };
    });
    if (markLoaded) loadedDomains.current.add("sessions");
  }, []);

  const applySessionProjectsChanged = useCallback((delta: SessionProjectDelta) => {
    setData((current) => {
      const sessions = applySessionProjectDelta(current.sessions, delta);
      return sessions === current.sessions ? current : { ...current, sessions };
    });
  }, []);

  const flushSessionEventBuffer = useCallback(() => {
    const recent = [...pendingRecentSessions.current.values()];
    const watch = [...pendingWatchSessions.current.values()];
    const deleted = [...pendingDeletedSessions.current.values()];
    pendingRecentSessions.current.clear();
    pendingWatchSessions.current.clear();
    pendingDeletedSessions.current.clear();
    sessionEventFlushTimer.current = undefined;
    if (recent.length > 0 || watch.length > 0 || deleted.length > 0) {
      const upserts = new Map<string, SessionRecord>();
      for (const session of recent) upserts.set(sessionLogicalIdentity(session), session);
      for (const session of watch) upserts.set(sessionLogicalIdentity(session), session);
      setData((current) => {
        const sessions = applySessionDelta(
          current.sessions,
          [...upserts.values()] as unknown as Array<Record<string, unknown>>,
          deleted,
        );
        return sessions === current.sessions ? current : { ...current, sessions };
      });
    }
  }, []);

  const scheduleSessionEventFlush = useCallback(() => {
    if (sessionEventFlushTimer.current !== undefined) return;
    sessionEventFlushTimer.current = window.setTimeout(
      flushSessionEventBuffer,
      SESSION_EVENT_FLUSH_MS,
    );
  }, [flushSessionEventBuffer]);

  const finishSessionScanWaiters = useCallback((generation: number) => {
    completedSessionScans.current.add(generation);
    const waiters = sessionScanWaiters.current.get(generation) ?? [];
    sessionScanWaiters.current.delete(generation);
    waiters.forEach((resolve) => resolve());
  }, []);

  const handleSessionScanEvent = useCallback((event: SessionScanEvent) => {
    if (event.generation < sessionScanGeneration.current) return;
    sessionScanGeneration.current = event.generation;
    const upserts = (event.upserts ?? []).map((row, index) => normalizeSession(row, index) as SessionRecord);
    const deleted = event.deleted ?? [];
    if (event.phase === "recent") {
      for (const session of upserts) {
        const key = sessionLogicalIdentity(session);
        pendingRecentSessions.current.set(key, session);
      }
    } else if (event.phase === "watch") {
      for (const session of upserts) {
        const key = sessionLogicalIdentity(session);
        pendingWatchSessions.current.set(key, session);
      }
    }
    for (const identity of deleted) {
      const key = sessionIdentity({
        id: `${identity.id ?? ""}`,
        agent: `${identity.agent ?? ""}`,
        path: `${identity.path ?? ""}`,
      });
      pendingDeletedSessions.current.set(key, identity);
    }
    scheduleSessionEventFlush();

    if (event.phase === "recent" && event.complete) {
      if (sessionEventFlushTimer.current !== undefined) {
        window.clearTimeout(sessionEventFlushTimer.current);
      }
      flushSessionEventBuffer();
    }

    if (event.phase === "backfill" && event.complete) {
      void (async () => {
        if (currentView.current === "sessions") {
          const rows = await safeInvoke(TauriCommand.SessionsList) as unknown[] | null;
          if (Array.isArray(rows)) setSessionRows(rows);
        }
        const status = await safeInvoke(TauriCommand.SessionSkillIndexRun, { force: false });
        if (status) setSkillIndexStatus(status as SkillIndexStatus);
        finishSessionScanWaiters(event.generation);
      })();
    } else if (event.phase === "error" && event.complete) {
      finishSessionScanWaiters(event.generation);
    }
  }, [finishSessionScanWaiters, flushSessionEventBuffer, scheduleSessionEventFlush, setSessionRows]);

  useEffect(() => {
    currentView.current = view;
  }, [view]);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    const applyRevision = (revision: number) => {
      if (!disposed) setAnalyticsRevision((current) => Math.max(current, revision));
    };
    const ready = listen<{ revision: number }>("analytics://revision", ({ payload }) => {
      applyRevision(payload.revision);
    }).then(async (cleanup) => {
      if (disposed) {
        cleanup();
        return;
      }
      unlisten = cleanup;
      const revision = await safeInvoke<number>(TauriCommand.AnalyticsRevision);
      if (revision !== null) applyRevision(revision);
      if (!disposed) setAnalyticsRevisionReady(true);
    });
    return () => {
      disposed = true;
      unlisten?.();
      void ready;
    };
  }, []);

  useEffect(() => {
    let disposed = false;
    const ready = listen<SessionScanEvent>("sessions://scan", ({ payload }) => {
      if (!disposed) handleSessionScanEvent(payload);
    }).then((unlisten) => {
      if (disposed) unlisten();
      else sessionEventUnlisten.current = unlisten;
    });
    sessionEventReady.current = ready;
    return () => {
      disposed = true;
      sessionEventUnlisten.current?.();
      sessionEventUnlisten.current = null;
      if (sessionEventFlushTimer.current !== undefined) {
        window.clearTimeout(sessionEventFlushTimer.current);
        sessionEventFlushTimer.current = undefined;
      }
    };
  }, [handleSessionScanEvent]);

  useEffect(() => {
    let disposed = false;
    let unlisten: UnlistenFn | null = null;
    const ready = listen<SkillUpdateCheckEvent>("skills://updates", ({ payload }) => {
      if (disposed) return;
      if (payload.status === "completed") {
        setData((current) => {
          return applySkillUpdateReports(current, payload.updates ?? []);
        });
        setSkillUpdateError("");
      } else {
        setSkillUpdateError(payload.error || "Update check failed");
      }
      setCheckingSkillUpdates(false);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
      void ready;
    };
  }, []);

  const refreshSessionsFromScan = useCallback(() => {
    if (!sessionsRefreshInFlight.current) {
      pendingRecentSessions.current.clear();
      pendingWatchSessions.current.clear();
      pendingDeletedSessions.current.clear();
      sessionsRefreshInFlight.current = sessionEventReady.current
        .then(() => safeInvoke<number>(TauriCommand.SessionsScanStart))
        .then((generation) => {
          if (generation === null) return null;
          sessionScanGeneration.current = Math.max(sessionScanGeneration.current, generation);
          if (completedSessionScans.current.has(generation)) return generation;
          return new Promise<number>((resolve) => {
            const waiters = sessionScanWaiters.current.get(generation) ?? [];
            waiters.push(() => resolve(generation));
            sessionScanWaiters.current.set(generation, waiters);
          });
        })
        .finally(() => {
          sessionsRefreshInFlight.current = null;
        });
    }
    return sessionsRefreshInFlight.current;
  }, []);

  const refreshSkillIndexStatus = useCallback(async () => {
    const status = await safeInvoke(TauriCommand.SessionSkillIndexStatus);
    if (status) setSkillIndexStatus(status);
    return status;
  }, []);

  const setDomainLoading = (domain: string, loading: boolean) => {
    setLoadingDomains((current) => {
      if (current.has(domain) === loading) return current;
      const next = new Set(current);
      if (loading) next.add(domain);
      else next.delete(domain);
      return next;
    });
  };

  useEffect(() => {
    let cancelled = false;
    safeInvoke(TauriCommand.AgentsList).then((agents) => {
      if (!cancelled && Array.isArray(agents)) {
        setData((current) => normalizeReport({ ...current, agents }, { fallback: false }));
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    safeInvoke<BundledSkillStatus>(TauriCommand.BundledSkillStatus).then((status) => {
      if (!cancelled && status?.shouldPrompt) setBundledSkillPrompt(status);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const dismissBundledSkillPrompt = async () => {
    if (bundledSkillBusy) return;
    setBundledSkillBusy(true);
    setBundledSkillError("");
    try {
      await invoke(TauriCommand.BundledSkillPromptDismiss);
      setBundledSkillPrompt(null);
    } catch (error) {
      setBundledSkillError(`${error}`);
    } finally {
      setBundledSkillBusy(false);
    }
  };

  const installBundledSkill = async () => {
    if (bundledSkillBusy) return;
    setBundledSkillBusy(true);
    setBundledSkillError("");
    try {
      const cliStatus = await invoke<CliInstallStatus>(TauriCommand.CliInstall);
      if (cliStatus.state !== "installed" || !cliStatus.pathConfigured) {
        throw new Error(cliStatus.detail || "The Tendi CLI is not available on PATH.");
      }
      await invoke(TauriCommand.BundledSkillInstall);
      setBundledSkillPrompt(null);
      void refreshSkillList({ preserveUpdates: false });
    } catch (error) {
      setBundledSkillError(`${error}`);
    } finally {
      setBundledSkillBusy(false);
    }
  };

  useEffect(() => {
    const loadOneDomain = async (domain: DomainKey) => {
      if (loadedDomains.current.has(domain)) return;
      const existing = domainLoadInFlight.current.get(domain);
      if (existing) {
        await existing;
        return;
      }
      const request = (async () => {
        setDomainLoading(domain, true);
        try {
          if (domain === "skills") {
            await refreshSkillListAndUpdates();
            return;
          }
          if (domain === "sessions") {
            const cachedRows = await safeInvoke(TauriCommand.SessionsList) as unknown[] | null;
            if (cachedRows) {
              setSessionRows(cachedRows, { markLoaded: view !== "overview" });
            }
            if (view === "overview") return;
            await refreshSessionsFromScan();
            return;
          }
          if (domain === "prompts") {
            await refreshPrompts();
            return;
          }
          const rows = await safeInvoke(domainListCommands[domain]) as unknown[] | null;
          if (rows) setDomainRows(domain, rows);
        } finally {
          setDomainLoading(domain, false);
        }
      })();
      domainLoadInFlight.current.set(domain, request);
      try {
        await request;
      } finally {
        if (domainLoadInFlight.current.get(domain) === request) {
          domainLoadInFlight.current.delete(domain);
        }
      }
    };

    if (view === "overview") {
      const domains: DomainKey[] = ["skills", "sessions", "prompts", "rules", "hooks", "mcp"];
      let timer = 0;
      let frame = 0;
      frame = window.requestAnimationFrame(() => {
        timer = window.setTimeout(() => {
          void Promise.all(domains.map((domain) => loadOneDomain(domain)));
        }, 0);
      });
      return () => {
        window.cancelAnimationFrame(frame);
        window.clearTimeout(timer);
        for (const domain of domains) setDomainLoading(domain, false);
      };
    }

    const domain = view === "skillDetail" ? "skills" : view === "ruleDetail" ? "rules" : view;
    if (!isDomainKey(domain)) return;
    let timer = 0;
    let frame = 0;
    frame = window.requestAnimationFrame(() => {
      timer = window.setTimeout(() => {
        void loadOneDomain(domain);
      }, 0);
    });
    return () => {
      window.cancelAnimationFrame(frame);
      window.clearTimeout(timer);
      setDomainLoading(domain, false);
    };
  }, [refreshPrompts, refreshSessionsFromScan, setSessionRows, view]);

  useEffect(() => {
    if (typeof window === "undefined") return undefined;
    if (skillIndexStatus === null) {
      void refreshSkillIndexStatus();
      return undefined;
    }
    if (!skillIndexStatus.running) return undefined;
    let cancelled = false;
    const timer = window.setInterval(async () => {
      if (cancelled) return;
      await refreshSkillIndexStatus();
    }, 4000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [refreshSkillIndexStatus, skillIndexStatus]);

  const navigate = useCallback((next: ViewId) => {
    setView(next);
    setActiveSkill(null);
    setActiveRule(null);
  }, []);

  const prefetchView = useCallback((next: ViewId) => {
    const loader = viewPreloaders[next];
    if (!loader || prefetchedViews.current.has(next) || prefetchTimers.current.has(next)) return;
    const timer = window.setTimeout(() => {
      prefetchTimers.current.delete(next);
      prefetchedViews.current.add(next);
      void loader().catch(() => {
        prefetchedViews.current.delete(next);
      });
    }, 120);
    prefetchTimers.current.set(next, timer);
  }, []);

  const cancelPrefetchView = useCallback((next: ViewId) => {
    const timer = prefetchTimers.current.get(next);
    if (timer === undefined) return;
    window.clearTimeout(timer);
    prefetchTimers.current.delete(next);
  }, []);

  useEffect(() => () => {
    for (const timer of prefetchTimers.current.values()) window.clearTimeout(timer);
    prefetchTimers.current.clear();
  }, []);

  const loadTranscript = useCallback(async (session: SessionRecord, cursor?: string, knownSourceVersion?: string): Promise<TranscriptPage> => {
    if (!session?.path) return { items: [], warnings: [], done: true, sourceVersion: "", restartRequired: false, unchanged: false };
    const page = await safeInvoke(TauriCommand.SessionTranscript, {
      path: session.path,
      agent: session.agent,
      cursor,
      limit: 160,
      knownSourceVersion,
    });
    return page
      ? normalizeTranscriptPage(page)
      : { items: [], warnings: [], done: true, sourceVersion: "", restartRequired: false, unchanged: false };
  }, []);

  const loadSessionAnalytics = useCallback(async (session: SessionRecord) => {
    if (!session?.id || !session.path || !session.agent) return null;
    return safeInvoke<SessionAnalyticsDetail>(TauriCommand.SessionAnalytics, {
      sessionId: session.id,
      agent: session.agent,
      path: session.path,
    });
  }, []);

  const searchSessions = useCallback(async (query: string) => {
    const rows = await safeInvoke(TauriCommand.SessionsSearch, { query }) as unknown[] | null;
    return Array.isArray(rows)
      ? rows.map((row, index) => normalizeSession(row as Record<string, unknown>, index)) as SessionRecord[]
      : [];
  }, []);

  const loadSessionSkillLinks = useCallback(async (session: SessionRecord) => {
    if (!session?.id) return [];
    const links = await safeInvoke(TauriCommand.SessionSkillLinks, { sessionId: session.id, agent: session.agent });
    return Array.isArray(links) ? links : [];
  }, []);

  const openSkillByName = useCallback((skillName: string) => {
    const skill = data.skills.find((item) => item.name === skillName) as SkillRecord | undefined;
    if (skill) setActiveSkill(skill);
    setView(skill ? "skillDetail" : "skills");
  }, [data.skills]);

  const openSessionFromLink = useCallback((link: DomainRow) => {
    setActiveSessionKey(`${link.agent ?? ""}:${link.session_id ?? link.sessionId ?? ""}`);
    setView("sessions");
  }, []);

  const resumeSessionInTerminal = useCallback(async (session: SessionRecord) => {
    return safeInvoke(TauriCommand.SessionResumeInTerminal, { session: sessionLaunchPayload(session) }) as Promise<
      { terminal?: string } | null | undefined
    >;
  }, []);

  const previewAndApply = async (
    command: SkillChangeCommand,
    args: Record<string, unknown>,
    { onApplied }: { onApplied?: () => void } = {},
  ) => {
    const isUpdate = command === SkillChangeCommand.UpdateMany;
    const isDelete = command === SkillChangeCommand.DeleteMany;
    if (isUpdate || isDelete) {
      if (isDelete) {
        setPendingSkillChange({ command, args, preview: null, onApplied });
      } else {
        setApplyingSkillUpdates(true);
        setSkillUpdateError("");
        setPendingSkillChange({ command, args, preview: null, onApplied });
      }
    }
    try {
      const preview = isUpdate || isDelete
        ? await invoke<SkillPreview>(command, { ...args, dryRun: true })
        : await safeInvoke(command, { ...args, dryRun: true }) as SkillPreview | null;
      if (!preview) return;
      if (isUpdate) {
        setPendingSkillChange((current) => current?.command === command ? { ...current, preview } : current);
      } else if (isDelete) {
        setPendingSkillChange((current) => current?.command === command && current.args === args ? { ...current, preview } : current);
      } else {
        setPendingSkillChange({ command, args, preview, onApplied });
      }
    } catch (error) {
      const message = `${error}`;
      setSkillUpdateError(message);
      if (isUpdate) setPendingSkillChange((current) => current?.command === command ? { ...current, previewError: message } : current);
      if (isDelete) setPendingSkillChange((current) => current?.command === command && current.args === args ? { ...current, previewError: message } : current);
    } finally {
      if (isUpdate) setApplyingSkillUpdates(false);
    }
  };

  const applySkillChange = async (command: SkillChangeCommand, args: Record<string, unknown>) => {
    const result = await invoke<SkillPreview>(command, { ...args, dryRun: false });
    if (result.skills) {
      setData((current) => normalizeReport({ ...current, skills: result.skills }, { fallback: false }));
    }
    return result;
  };

  const closeSkillChangeDialog = (open: boolean) => {
    if (!open && !applyingSkillChange) setPendingSkillChange(null);
  };

  const confirmSkillChange = async () => {
    if (!pendingSkillChange || applyingSkillChange) return;
    setApplyingSkillChange(true);
    const { command, args, onApplied } = pendingSkillChange;
    setPendingSkillChange((current) => current ? { ...current, applyError: undefined } : current);
    if (command === SkillChangeCommand.UpdateMany) setSkillUpdateError("");
    const previewId = command === SkillChangeCommand.DeleteMany
      ? pendingSkillChange.preview?.previewId
      : command === SkillChangeCommand.UpdateMany
        ? pendingSkillChange.preview?.previewId
        : undefined;
    try {
      const result = await invoke<SkillPreview>(command, { ...args, previewId, dryRun: false });
      if (result.skills) {
        setData((current) => normalizeReport({ ...current, skills: result.skills }, { fallback: false }));
      } else if (command === SkillChangeCommand.DeleteMany) {
        const names = new Set((args.names as unknown[] | undefined)?.map((name) => `${name}`) ?? []);
        setData((current) => normalizeReport({
          ...current,
          skills: current.skills.filter((skill) => !names.has(skill.name)),
        }, { fallback: false }));
      } else if (command === SkillChangeCommand.UpdateMany) {
        const names = (args.names as unknown[] | undefined)?.map((name) => `${name}`) ?? [];
        setData((current) => clearSkillUpdateAvailability(current, names));
      }
      setPendingSkillChange(null);
      onApplied?.();
      if (!result.skills && (command !== SkillChangeCommand.DeleteMany || result?.refreshRequired)) {
        void refreshSkillList({ preserveUpdates: false });
      }
    } catch (error) {
      const message = `${error}`;
      setPendingSkillChange((current) => current?.command === command ? { ...current, applyError: message } : current);
      if (command === SkillChangeCommand.UpdateMany) setSkillUpdateError(message);
    } finally {
      setApplyingSkillChange(false);
    }
  };

  const confirmSkillChangeWithRelated = async () => {
    if (!pendingSkillChange || applyingSkillChange || pendingSkillChange.command !== SkillChangeCommand.DeleteMany) return;
    const currentNames = Array.isArray(pendingSkillChange.args.names)
      ? pendingSkillChange.args.names.map((name) => `${name}`)
      : [];
    const names = [...new Set([...currentNames, ...deletePreviewRelatedNames(pendingSkillChange.preview)])];
    if (names.length === currentNames.length) return confirmSkillChange();
    setPendingSkillChange((current) => current ? { ...current, args: { ...current.args, names }, applyError: undefined } : current);
    setApplyingSkillChange(true);
    try {
      const result = await invoke<SkillPreview>(pendingSkillChange.command, { ...pendingSkillChange.args, names, dryRun: false });
      if (result.skills) {
        setData((current) => normalizeReport({ ...current, skills: result.skills }, { fallback: false }));
      } else {
        const deletedNames = new Set(names);
        setData((current) => normalizeReport({
          ...current,
          skills: current.skills.filter((skill) => !deletedNames.has(skill.name)),
        }, { fallback: false }));
      }
      setPendingSkillChange(null);
      pendingSkillChange.onApplied?.();
      if (result?.refreshRequired) void refreshSkillList();
    } catch (error) {
      const message = `${error}`;
      setPendingSkillChange((current) => current ? { ...current, applyError: message } : current);
    } finally {
      setApplyingSkillChange(false);
    }
  };

  const applyVisibility = async (names: string[], visibility: SkillVisibility) => {
    setData((current) => applyVisibilityState(current, names, visibility));
    const result = await safeInvoke<SkillPreview>(SkillChangeCommand.Set, { names, visibility, dryRun: false });
    if (result?.skills) {
      setData((current) => normalizeReport({ ...current, skills: result.skills }, { fallback: false }));
    }
    if (!result) await refreshSkillList();
  };

  const forceSidebarResizeHover =
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("forceSidebarResizeHover") === "1";
  const sidebarSize = sidebarCollapsed ? COLLAPSED_SIDEBAR_SIZE : SIDEBAR_SIZE;

  return (
    <main className="appShell">
      {bundledSkillPrompt ? (
        <Suspense fallback={null}>
          <BundledSkillInstallDialog
            open
            target={bundledSkillPrompt.target}
            busy={bundledSkillBusy}
            error={bundledSkillError}
            onInstall={() => { void installBundledSkill(); }}
            onDismiss={() => { void dismissBundledSkillPrompt(); }}
          />
        </Suspense>
      ) : null}
      {pendingSkillChange ? (
        <Suspense fallback={null}>
          <ConfirmSkillChangesDialog
            open
            command={pendingSkillChange.command}
            preview={pendingSkillChange.preview ?? null}
            previewError={pendingSkillChange.previewError}
            applyError={pendingSkillChange.applyError}
            busy={applyingSkillChange}
            onOpenChange={closeSkillChangeDialog}
            onConfirm={confirmSkillChange}
            onConfirmRelated={confirmSkillChangeWithRelated}
          />
        </Suspense>
      ) : null}
      <PanelGroup
        className={`window ${sidebarCollapsed ? "sidebarCollapsed" : ""} ${forceSidebarResizeHover ? "forceSidebarResizeHover" : ""}`}
        key={sidebarCollapsed ? "window-collapsed" : "window-expanded"}
        orientation="horizontal"
        disabled
      >
        <Panel
          key={sidebarCollapsed ? "sidebar-collapsed" : "sidebar-expanded"}
          className="sidebarPanel"
          collapsible={false}
          defaultSize={sidebarSize}
          groupResizeBehavior="preserve-pixel-size"
          minSize={sidebarSize}
          maxSize={sidebarSize}
          {...({ order: 1 } as Record<string, unknown>)}
        >
          <Sidebar
            view={view === "skillDetail" ? "skills" : view === "ruleDetail" ? "rules" : view}
            setView={navigate}
            onPrefetchView={prefetchView}
            onCancelPrefetchView={cancelPrefetchView}
            sources={data.sources.length ? data.sources : fallbackAgents}
            collapsed={sidebarCollapsed}
            setCollapsed={setSidebarCollapsed}
            agentFilter={agentFilter}
            setAgentFilter={setAgentFilter}
          />
        </Panel>
        <Panel className="mainPanel" minSize="520px" {...({ order: 2 } as Record<string, unknown>)}>
          <Suspense fallback={<div className="content viewLoadingFallback" aria-busy="true" />}>
          {view === "skillDetail" ? (
            <SkillEditorView
              skill={activeSkill}
              skills={data.skills}
              back={() => navigate("skills")}
              skillIndexStatus={skillIndexStatus}
              onOpenSession={openSessionFromLink}
              onOpenSkill={openSkillByName}
              onSaved={(skills) => {
                if (skills) {
                  setData((current) => normalizeReport({ ...current, skills }, { fallback: false }));
                }
              }}
            />
          ) : view === "ruleDetail" ? (
            <RuleEditorView rule={activeRule} back={() => navigate("rules")} />
          ) : view === "skills" ? (
            <SkillsView
              skills={filteredData.skills as SkillRecord[]}
              loadingSkills={loadingDomains.has("skills")}
              checkingUpdates={checkingSkillUpdates}
              applyingUpdates={applyingSkillUpdates}
              updateError={skillUpdateError}
              onRefresh={refreshSkills}
              onSetVisibility={applyVisibility}
              onApplyWrapper={(args) => applySkillChange(SkillChangeCommand.Wrap, args)}
              onApplyUpdates={(names, onApplied) => previewAndApply(SkillChangeCommand.UpdateMany, { names }, { onApplied })}
              onDeleteSkills={(names, onApplied) => previewAndApply(SkillChangeCommand.DeleteMany, { names }, { onApplied })}
              onAddInstalled={applyInstalledSkills}
              openSkill={(skill) => {
                setActiveSkill(skill);
                setView("skillDetail");
              }}
            />
          ) : view === "sessions" ? (
            <SessionsView
              sessions={filteredData.sessions as SessionRecord[]}
              loadTranscript={loadTranscript}
              loadSessionAnalytics={loadSessionAnalytics}
              searchSessions={searchSessions}
              loadSessionSkillLinks={loadSessionSkillLinks}
              loadingSessions={loadingDomains.has("sessions")}
              onRefreshSessions={refreshSessionsFromScan}
              onResumeSession={resumeSessionInTerminal}
              onOpenSkill={openSkillByName}
              activeSessionKey={activeSessionKey}
              skillIndexStatus={skillIndexStatus}
              onSessionProjectsChanged={applySessionProjectsChanged}
            />
          ) : view === "prompts" ? (
            <PromptsView
              prompts={filteredData.prompts as ComponentProps<typeof PromptsView>["prompts"]}
              loadingPrompts={loadingDomains.has("prompts")}
              onRefreshPrompts={async () => { await refreshPrompts(); }}
              onPromptSaved={applyPromptSaved}
              onPromptsDeleted={removePrompts}
            />
            ) : view === "rules" ? (
              <RulesView rows={filteredData.rules} skills={data.skills} loadingRows={loadingDomains.has("rules")} onOpenSkill={openSkillByName} />
            ) : view === "hooks" ? (
              <HooksView rows={filteredData.hooks} loadingRows={loadingDomains.has("hooks")} onDeleteHook={deleteHook} onDeleteHooks={deleteHooks} onSetHookEnabled={setHookEnabled} onReviewHook={reviewHook} />
          ) : view === "mcp" ? (
            <DataListView title="MCP" rows={filteredData.mcp} columns={mcpColumns} loading={loadingDomains.has("mcp")} />
          ) : view === "config" ? (
            <ConfigView />
          ) : view === "settings" ? (
            <SettingsView
              appearance={appearance}
              themePreferences={themePreferences}
              onAppearanceChange={changeAppearance}
              onThemeChange={(mode, theme) => {
                setThemePreferences((current) => ({ ...current, [mode]: theme }));
              }}
            />
          ) : view === "overview" ? (
            <OverviewView
              counts={{
                skills: filteredData.skills.length,
                sessions: filteredData.sessions.length,
                prompts: filteredData.prompts.length,
                rules: filteredData.rules.length,
                hooks: filteredData.hooks.length,
                mcp: filteredData.mcp.length,
              }}
              hookReviewCount={filteredData.hooks.filter((hook) => hook.needs_review === true).length}
              skills={filteredData.skills as SkillRecord[]}
              sessions={filteredData.sessions as SessionRecord[]}
              analyticsRevision={analyticsRevision}
              analyticsRevisionReady={analyticsRevisionReady}
              agentFilter={agentFilter}
              onNavigate={(id: OverviewNavId) => navigate(id)}
              onOpenSession={(session) => {
                setActiveSessionKey(`${session.agent ?? ""}:${session.id ?? ""}`);
                setView("sessions");
              }}
            />
          ) : (
            <PlaceholderView title={activeNav?.label ?? "Overview"} />
          )}
          </Suspense>
        </Panel>
      </PanelGroup>
    </main>
  );
}
