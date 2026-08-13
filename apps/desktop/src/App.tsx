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
import { applyAppearance, listenForSystemAppearanceChange, normalizeAppearance, readCachedAppearance, type Appearance } from "./lib/appearance.ts";
import { COLLAPSED_SIDEBAR_SIZE, SIDEBAR_SIZE, SkillChangeCommand, SkillVisibility, TauriCommand, applySessionDelta, applySkillUpdateReports, applyVisibilityState, clearSkillUpdateAvailability, fallbackAgents, fallbackTranscript, hookSourcePath, hookTrustHash, initialData, mergeSessionRows, mergeSkillListPreservingUpdates, navItems, normalizeReport, normalizeSession, normalizeTranscript, safeInvoke, sameAgent, sessionIdentity, sessionLaunchPayload, sessionLogicalIdentity } from "./lib/index.ts";
import type { SessionAnalyticsDetail, SessionIdentityRecord } from "./lib/index.ts";
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
};
type SkillUpdateReport = { name: string; status: string };

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
  const [view, setView] = useState<ViewId>("skills");
  const [activeSkill, setActiveSkill] = useState<SkillRecord | null>(null);
  const [activeRule, setActiveRule] = useState<DomainRow | null>(null);
  const [activeSessionKey, setActiveSessionKey] = useState("");
  const [skillIndexStatus, setSkillIndexStatus] = useState<SkillIndexStatus | null>(null);
  const [data, setData] = useState<RuntimeData>(() => initialData());
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [agentFilter, setAgentFilter] = useState("All");
  const [pendingSkillChange, setPendingSkillChange] = useState<PendingSkillChange | null>(null);
  const [applyingSkillChange, setApplyingSkillChange] = useState(false);
  const [applyingSkillUpdates, setApplyingSkillUpdates] = useState(false);
  const [skillUpdateError, setSkillUpdateError] = useState("");
  const [checkingSkillUpdates, setCheckingSkillUpdates] = useState(false);
  const [loadingDomains, setLoadingDomains] = useState(() => new Set<string>(["skills"]));
  const [appearance, setAppearance] = useState<Appearance>(() => readCachedAppearance());
  const appearanceChangeRevision = useRef(0);
  const loadedDomains = useRef(new Set<string>());
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
    applyAppearance(appearance);
    if (appearance !== "system") return;
    return listenForSystemAppearanceChange(() => applyAppearance("system"));
  }, [appearance]);

  const changeAppearance = useCallback((nextAppearance: Appearance) => {
    appearanceChangeRevision.current += 1;
    setAppearance(nextAppearance);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const revision = appearanceChangeRevision.current;
    safeInvoke<{ appearance?: unknown }>(TauriCommand.SettingsGet).then((settings) => {
      if (!cancelled && settings && revision === appearanceChangeRevision.current) {
        setAppearance(normalizeAppearance(settings.appearance));
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

  const applyInstalledSkills = (result: SkillInstallResult) => {
    const plan = result.report?.plan;
    const installed = plan?.selected ?? [];
    const materialized = result.report?.results ?? [];
    if (installed.length > 0) {
      const optimisticSkills = installed.map((skill, index) => ({
        name: skill.name,
        description: skill.description,
        dependencies: skill.dependencies ?? [],
        agents: plan?.target ? [plan.target] : [],
        paths: [{
          path: materialized[index]?.target,
          agent: plan?.target,
          scope: "global",
          source: plan?.source,
          source_kind: plan?.source_kind,
        }],
        source_summary: plan?.source ?? "local",
      }));
      setData((current) => {
        const remaining = new Map(optimisticSkills.map((skill) => [skill.name, skill]));
        const merged = current.skills.map((skill) => {
          const installedSkill = remaining.get(skill.name);
          if (!installedSkill) return skill;
          remaining.delete(skill.name);
          const installedPath = installedSkill.paths[0];
          return {
            ...skill,
            ...installedSkill,
            agents: [...new Set([...skill.agents, ...(installedSkill.agents ?? [])])],
            paths: [
              ...skill.paths.filter((path) => !installedPath.path || path.path !== installedPath.path),
              installedPath,
            ],
          };
        });
        return normalizeReport({
          ...current,
          skills: [...merged, ...remaining.values()],
        }, { fallback: false });
      });
    }
    void refreshSkillList();
  };

  const refreshSkillUpdates = async () => {
    const revision = ++skillUpdateCheckRevision.current;
    setCheckingSkillUpdates(true);
    try {
      const updates = await safeInvoke(TauriCommand.SkillsUpdates, { check: true }) as SkillUpdateReport[] | null;
      if (revision !== skillUpdateCheckRevision.current || !updates) return;
      setData((current) => applySkillUpdateReports(current, updates));
    } finally {
      if (revision === skillUpdateCheckRevision.current) setCheckingSkillUpdates(false);
    }
  };

  const refreshSkills = async () => {
    setDomainLoading("skills", true);
    try {
      const skills = await refreshSkillList();
      if (skills) void refreshSkillUpdates();
    } finally {
      setDomainLoading("skills", false);
    }
  };

  const setDomainRows = (domain: DomainKey, rows: unknown[]) => {
    setData((current) => normalizeReport({ ...current, [domain]: rows }, { fallback: false }));
    loadedDomains.current.add(domain);
  };

  const refreshPrompts = useCallback(async () => {
    const rows = await safeInvoke(TauriCommand.PromptsList) as unknown[] | null;
    if (rows) setDomainRows("prompts", rows);
    return rows;
  }, []);

  const deleteHook = useCallback(async (hook: DomainRow) => {
    try {
      const rows = await invoke(TauriCommand.HookDelete, {
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
      console.warn(`tendi command failed: ${TauriCommand.HookDelete}`, error);
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

  const setSessionRows = useCallback((rows: unknown[]) => {
    setData((current) => {
      const nextSessions = mergeSessionRows(current.sessions, rows as Array<Record<string, unknown>>);
      if (nextSessions === current.sessions) return current;
      return { ...current, sessions: nextSessions };
    });
    loadedDomains.current.add("sessions");
  }, []);

  const flushSessionEventBuffer = useCallback(() => {
    const recent = [...pendingRecentSessions.current.values()];
    const watch = [...pendingWatchSessions.current.values()];
    const deleted = [...pendingDeletedSessions.current.values()];
    pendingRecentSessions.current.clear();
    pendingWatchSessions.current.clear();
    pendingDeletedSessions.current.clear();
    sessionEventFlushTimer.current = undefined;
    if (currentView.current !== "sessions") {
      return;
    }
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
    const loadOneDomain = async (domain: DomainKey, cancelled: () => boolean) => {
      if (loadedDomains.current.has(domain)) return;
      setDomainLoading(domain, true);
      try {
        if (domain === "skills") {
          await refreshSkillList();
          return;
        }
        if (domain === "sessions") {
          const cachedRows = await safeInvoke(TauriCommand.SessionsList) as unknown[] | null;
          if (!cancelled() && cachedRows) setSessionRows(cachedRows);
          await refreshSessionsFromScan();
          return;
        }
        if (domain === "prompts") {
          await refreshPrompts();
          return;
        }
        const rows = await safeInvoke(domainListCommands[domain]) as unknown[] | null;
        if (!cancelled() && rows) setDomainRows(domain, rows);
      } finally {
        if (!cancelled()) setDomainLoading(domain, false);
      }
    };

    if (view === "overview") {
      let cancelled = false;
      const domains: DomainKey[] = ["skills", "sessions", "prompts", "rules", "hooks", "mcp"];
      let timer = 0;
      let frame = 0;
      frame = window.requestAnimationFrame(() => {
        timer = window.setTimeout(() => {
          void Promise.all(domains.map((domain) => loadOneDomain(domain, () => cancelled)));
        }, 0);
      });
      return () => {
        cancelled = true;
        window.cancelAnimationFrame(frame);
        window.clearTimeout(timer);
        for (const domain of domains) setDomainLoading(domain, false);
      };
    }

    const domain = view === "skillDetail" ? "skills" : view === "ruleDetail" ? "rules" : view;
    if (!isDomainKey(domain)) return;
    let cancelled = false;
    let timer = 0;
    let frame = 0;
    frame = window.requestAnimationFrame(() => {
      timer = window.setTimeout(() => {
        void loadOneDomain(domain, () => cancelled);
      }, 0);
    });
    return () => {
      cancelled = true;
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

  const loadTranscript = useCallback(async (session: SessionRecord) => {
    if (!session?.path) return fallbackTranscript;
    const items = await safeInvoke(TauriCommand.SessionTranscript, { path: session.path, agent: session.agent });
    return Array.isArray(items) ? normalizeTranscript(items) : fallbackTranscript;
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
    if (isUpdate) {
      setApplyingSkillUpdates(true);
      setSkillUpdateError("");
      setPendingSkillChange({ command, args, preview: null, onApplied });
    }
    try {
      const preview = isUpdate
        ? await invoke<SkillPreview>(command, { ...args, dryRun: true })
        : await safeInvoke(command, { ...args, dryRun: true }) as SkillPreview | null;
      if (!preview) return;
      if (isUpdate) {
        setPendingSkillChange((current) => current?.command === command ? { ...current, preview } : current);
      } else {
        setPendingSkillChange({ command, args, preview, onApplied });
      }
    } catch (error) {
      const message = `${error}`;
      setSkillUpdateError(message);
      if (isUpdate) setPendingSkillChange((current) => current?.command === command ? { ...current, previewError: message } : current);
    } finally {
      if (isUpdate) setApplyingSkillUpdates(false);
    }
  };

  const applySkillChange = async (command: SkillChangeCommand, args: Record<string, unknown>) => {
    const result = await invoke(command, { ...args, dryRun: false });
    await refreshSkillList();
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
      : undefined;
    try {
      await invoke(command, { ...args, previewId, dryRun: false });
      if (command === SkillChangeCommand.DeleteMany) {
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
      void refreshSkillList({ preserveUpdates: false });
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
      await invoke(pendingSkillChange.command, { ...pendingSkillChange.args, names, dryRun: false });
      const deletedNames = new Set(names);
      setData((current) => normalizeReport({
        ...current,
        skills: current.skills.filter((skill) => !deletedNames.has(skill.name)),
      }, { fallback: false }));
      setPendingSkillChange(null);
      pendingSkillChange.onApplied?.();
      void refreshSkillList();
    } catch (error) {
      const message = `${error}`;
      setPendingSkillChange((current) => current ? { ...current, applyError: message } : current);
    } finally {
      setApplyingSkillChange(false);
    }
  };

  const applyVisibility = async (names: string[], visibility: SkillVisibility) => {
    setData((current) => applyVisibilityState(current, names, visibility));
    const result = await safeInvoke(SkillChangeCommand.Set, { names, visibility, dryRun: false });
    if (!result) await refreshSkillList();
  };

  const forceSidebarResizeHover =
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("forceSidebarResizeHover") === "1";
  const sidebarSize = sidebarCollapsed ? COLLAPSED_SIDEBAR_SIZE : SIDEBAR_SIZE;

  return (
    <main className="appShell">
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
              onSaved={() => { void refreshSkillList(); }}
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
              onSessionProjectsChanged={setSessionRows}
            />
          ) : view === "prompts" ? (
            <PromptsView
              prompts={filteredData.prompts as ComponentProps<typeof PromptsView>["prompts"]}
              loadingPrompts={loadingDomains.has("prompts")}
              onRefreshPrompts={async () => { await refreshPrompts(); }}
            />
            ) : view === "rules" ? (
              <RulesView rows={filteredData.rules} skills={data.skills} loadingRows={loadingDomains.has("rules")} onOpenSkill={openSkillByName} />
            ) : view === "hooks" ? (
              <HooksView rows={filteredData.hooks} loadingRows={loadingDomains.has("hooks")} onDeleteHook={deleteHook} onSetHookEnabled={setHookEnabled} />
          ) : view === "mcp" ? (
            <DataListView title="MCP" rows={filteredData.mcp} columns={mcpColumns} loading={loadingDomains.has("mcp")} />
          ) : view === "config" ? (
            <ConfigView />
          ) : view === "settings" ? (
            <SettingsView appearance={appearance} onAppearanceChange={changeAppearance} />
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
              skills={filteredData.skills as SkillRecord[]}
              sessions={filteredData.sessions as SessionRecord[]}
              sessionsLoading={loadingDomains.has("sessions") || !loadedDomains.current.has("sessions")}
              agentFilter={agentFilter}
              checkingUpdates={checkingSkillUpdates}
              applyingUpdates={applyingSkillUpdates}
              updateError={skillUpdateError}
              onNavigate={(id: OverviewNavId) => navigate(id)}
              onOpenSkill={(skill) => {
                setActiveSkill(skill);
                setView("skillDetail");
              }}
              onOpenSession={(session) => {
                setActiveSessionKey(`${session.agent ?? ""}:${session.id ?? ""}`);
                setView("sessions");
              }}
              onCheckUpdates={refreshSkillUpdates}
              onApplyUpdates={(names) => previewAndApply(SkillChangeCommand.UpdateMany, { names })}
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
