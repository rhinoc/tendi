import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { Group as PanelGroup, Panel, usePanelRef } from "react-resizable-panels";
import { Accordion, Checkbox, ContextMenu, Dialog, DropdownMenu, Select } from "radix-ui";
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
import stepfunIcon from "@lobehub/icons-static-svg/icons/stepfun-color.svg";
import traeIcon from "@lobehub/icons-static-svg/icons/trae-color.svg";

import type { ComponentProps } from "react";
import { applyAppearance, applyFontFamily, listenForSystemAppearanceChange, normalizeAppearance, normalizeFontFamily, normalizeThemePreferences, readCachedAppearance, readCachedFontFamily, readCachedThemePreferences, type Appearance, type ColorTheme, type FontFamily, type ThemePreferences } from "./lib/appearance.ts";
import { applyAppIcon, normalizeAppIcon, readCachedAppIcon, type AppIcon } from "./lib/app-icon.ts";
import { COLLAPSED_SIDEBAR_SIZE, SIDEBAR_SIZE, SkillChangeCommand, SkillVisibility, TauriCommand, UPDATE_AVAILABLE_EVENT, agentIdentityKey, applySessionDelta, applyVisibilityState, clearSkillUpdateAvailability, fallbackAgents, hookSourcePath, hookTrustHash, invokeCommand, isConcreteAgent, isTauriRuntime, logger, mergeSessionRows, navItems, normalizeDomainRows, normalizeReport, normalizeSession, normalizeSessionResumeTarget, normalizeTranscriptLocatorPage, normalizeTranscriptPage, normalizeTranscriptSearchResult, promptTitleFromBody, replaceSkillReportPreservingUpdates, ruleAgents, safeInvoke, sameAgent, sessionAppDeepLink, sessionIdentity, sessionLaunchPayload, sessionLogicalIdentity, sessionResumeTargetForAgent, subscribeDaemonEvents } from "./lib/index.ts";
import type { BundledSkillStatus, CliInstallStatus, DaemonEvent, DesktopUpdateState, MissingSessionProjectPolicy, ProjectSummary, SessionIdentityRecord, SessionProjectSummary, SessionResumeOptions, SessionResumeOutcome, SessionResumeTarget, TranscriptLocatorPage, TranscriptPage, TranscriptSearchResult, TranscriptSearchScopes, UpdateCheckResult } from "./lib/index.ts";
import { sortSidebarSources, type OrderedSidebarSource } from "./lib/sidebar-sources.ts";
import { mcpColumns } from "./lib/tableColumns.tsx";
import { PlaceholderView } from "./components/shared/PlaceholderView.tsx";
import { DialogLoadingFallback } from "./components/shared/DialogLoadingFallback.tsx";
import { DialogActionButton } from "./components/shared/DialogActionButton.tsx";
import { DialogShell } from "./components/shared/DialogShell.tsx";
import { DialogStatefulButton } from "./components/shared/DialogStatefulButton.tsx";
import { LoadingState } from "./components/shared/LoadingState.tsx";
import { Sidebar } from "./components/shared/Sidebar.tsx";
import { Toast } from "./components/shared/Toast.tsx";
import type { SessionRecord, SkillLinkRecord } from "./views/SessionsView.tsx";
import type { SkillInstallResult, SkillRecord } from "./views/SkillsView.tsx";
import { ConfigView } from "./views/ConfigView.tsx";
import { DataListView } from "./views/McpView.tsx";
import { HooksView } from "./views/HooksView.tsx";
import { PromptsView } from "./views/PromptsView.tsx";
import { RulesView } from "./views/RulesView.tsx";
import { SessionsView } from "./views/SessionsView.tsx";
import { SkillsView } from "./views/SkillsView.tsx";
import { BackupView } from "./features/skills/BackupView.tsx";
import { RuleEditorView } from "./features/rules/RuleEditorView.tsx";
import { SettingsView } from "./features/settings/SettingsView.tsx";
import { SkillEditorView } from "./features/skills/SkillEditorView.tsx";
import { OverviewView, type OverviewNavId } from "./views/OverviewView.tsx";
import { desktopStore, useDesktopStore, type AgentTargetOption, type SkillIndexStatus, type SkillUpdateReport } from "./store/desktop-store.ts";

type ViewId =
  | "overview"
  | "skills"
  | "backup"
  | "prompts"
  | "sessions"
  | "rules"
  | "hooks"
  | "mcp"
  | "config"
  | "settings"
  | "skillDetail"
  | "ruleDetail";

const loadConfirmSkillChangesDialog = () => import("./features/skills/ConfirmSkillChangesDialog.tsx");
const loadBundledSkillInstallDialog = () => import("./features/skills/BundledSkillInstallDialog.tsx");

const ConfirmSkillChangesDialog = lazy(() => loadConfirmSkillChangesDialog().then(({ ConfirmSkillChangesDialog: component }) => ({ default: component })));
const BundledSkillInstallDialog = lazy(() => loadBundledSkillInstallDialog().then(({ BundledSkillInstallDialog: component }) => ({ default: component })));

function SkillChangeDialogFallback({
  command,
  names,
  onOpenChange,
  onConfirm,
}: {
  command: SkillChangeCommand | null;
  names: string[];
  onOpenChange: (open: boolean) => void;
  onConfirm: () => void;
}) {
  const isUpdate = command === SkillChangeCommand.UpdateMany;
  const isDelete = command === SkillChangeCommand.DeleteMany;
  return (
    <DialogShell
      open
      onOpenChange={onOpenChange}
      descriptionId="skill-changes-loading-description"
      contentProps={{ "data-update-preview": isUpdate }}
    >
      <div className="skillChangeDialogBody">
        <Dialog.Title className="confirmDialogTitle">{isDelete ? "Delete selected skills?" : "Confirm skill changes"}</Dialog.Title>
        {isDelete ? (
          <>
            <p id="skill-changes-loading-description" className="confirmDialogDescription">
              Delete the selected skills from their installed locations.
            </p>
            <div className="skillDeleteNames" data-selectable-text>
              {names.map((name) => <span key={name}>{name}</span>)}
            </div>
          </>
        ) : (
          <>
            <p id="skill-changes-loading-description" className="dialogVisuallyHidden">
              Preparing skill change preview.
            </p>
            <LoadingState className="skillUpdatePreviewLoading" label="Preparing update preview" />
          </>
        )}
      </div>
      {isDelete ? (
        <div className="confirmDialogActions">
          <DialogActionButton variant="secondary" onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
          <DialogStatefulButton state="idle" variant="danger" aria-label="Delete skills" onClick={onConfirm}>
            Delete skills
          </DialogStatefulButton>
        </div>
      ) : null}
    </DialogShell>
  );
}

const SESSION_EVENT_FLUSH_MS = 200;
const AUTO_UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
const AUTO_UPDATE_LAST_CHECK_KEY = "tendi-update-last-check-at";

type SkillPreview = {
  summary?: string;
  plan?: Record<string, unknown>;
  previewId?: string;
  refreshRequired?: boolean;
  skills?: SkillRecord[];
};
type SkillRefreshResult = { skills: SkillRecord[]; updateCheck?: string; updates?: SkillUpdateReport[] };

type SkillUpdateCheckEvent = {
  status: "completed" | "failed";
  skills?: SkillRecord[] | null;
  updates?: SkillUpdateReport[];
  error?: string | null;
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

const DOMAIN_KEYS: DomainKey[] = ["skills", "prompts", "sessions", "rules", "hooks", "mcp"];
const SESSION_LOAD_ERROR = "Could not load sessions. Try again.";
const SESSION_REFRESH_ERROR = "Could not refresh sessions. Try again.";

type OverviewDomainCount = {
  count: number;
  secondaryCount: number;
};

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return `${error}`;
}

type SidebarSource = {
  label: string;
  count: number;
};

function sidebarSources(
  targets: AgentTargetOption[],
  discoveredSources: SidebarSource[],
  agents: Record<string, unknown>[],
): SidebarSource[] {
  const installedKeys = new Set(
    agents
      .filter((agent) => agent.installed === true)
      .map((agent) => agentIdentityKey(agent.name ?? agent.kind)),
  );
  const sourceByKey = new Map(discoveredSources.map((source) => [agentIdentityKey(source.label), source]));
  const seen = new Set<string>();
  const result: OrderedSidebarSource[] = [];

  const add = (label: string, count: number, order: number) => {
    const key = agentIdentityKey(label);
    if (!isConcreteAgent(label) || seen.has(key)) return;
    seen.add(key);
    const source = sourceByKey.get(key);
    result.push({
      label,
      count: source?.count ?? count,
      installed: installedKeys.has(key) || (source?.count ?? 0) > 0,
      order,
    });
  };

  targets.forEach((target, index) => add(target.displayName, 0, index));
  discoveredSources.forEach((source, index) => add(source.label, source.count, targets.length + index));

  return sortSidebarSources(result);
}

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

export function App() {
  const storeState = useDesktopStore((state) => state);
  const { catalogs, inventory, skillUpdates, sessions, analytics } = storeState;
  const data = catalogs.data;
  const agentTargets = catalogs.agentTargets;
  const loadingDomains = catalogs.loadingDomains;
  const domainErrors = catalogs.errors;
  const domainRetryRevision = catalogs.retryRevision;
  const overviewCounts = inventory.counts;
  const overviewCountsLoaded = inventory.loaded;
  const overviewCountErrors = inventory.errors;
  const overviewHookReviewCount = inventory.hookReviewCount;
  const overviewSkillUpdateCount = inventory.skillUpdateCount;
  const checkingSkillUpdates = skillUpdates.checking;
  const skillUpdateError = skillUpdates.error;
  const skillIndexStatus = skillUpdates.indexStatus;
  const analyticsRevision = analytics.revision;
  const analyticsRevisionReady = analytics.ready;
  const analyticsRevisionError = analytics.error;
  const sessionListStatus = sessions.listStatus;
  const sessionListError = sessions.listError;
  const sessionRefreshError = sessions.refreshError;
  const [view, setView] = useState<ViewId>("overview");
  const deferredView = useDeferredValue(view);
  const contentView = (
    (deferredView === "skillDetail" && view !== "skillDetail")
    || (deferredView === "ruleDetail" && view !== "ruleDetail")
  ) ? view : deferredView;
  const [activeSkill, setActiveSkill] = useState<SkillRecord | null>(null);
  const [activeRule, setActiveRule] = useState<DomainRow | null>(null);
  const [activeSessionKey, setActiveSessionKey] = useState("");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [agentFilter, setAgentFilter] = useState("All");
  const [pendingSkillChange, setPendingSkillChange] = useState<PendingSkillChange | null>(null);
  const [bundledSkillPrompt, setBundledSkillPrompt] = useState<BundledSkillStatus | null>(null);
  const [bundledSkillBusy, setBundledSkillBusy] = useState(false);
  const [bundledSkillError, setBundledSkillError] = useState("");
  const [applyingSkillChange, setApplyingSkillChange] = useState(false);
  const [appearance, setAppearance] = useState<Appearance>(() => readCachedAppearance());
  const [themePreferences, setThemePreferences] = useState<ThemePreferences>(() => readCachedThemePreferences());
  const [fontFamily, setFontFamily] = useState<FontFamily>(() => readCachedFontFamily());
  const [appIcon, setAppIcon] = useState<AppIcon>(() => readCachedAppIcon());
  const [developerMode, setDeveloperMode] = useState(false);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [sessionProjects, setSessionProjects] = useState<SessionProjectSummary[]>([]);
  const [missingSessionProjectPolicy, setMissingSessionProjectPolicy] = useState<MissingSessionProjectPolicy>("show");
  const [desktopUpdate, setDesktopUpdate] = useState<DesktopUpdateState>({ status: "idle" });
  const [updateNoticeDismissed, setUpdateNoticeDismissed] = useState(false);
  const [sessionResumeTarget, setSessionResumeTarget] = useState<SessionResumeTarget>("auto");
  const changeAppIcon = useCallback((nextAppIcon: AppIcon) => {
    setAppIcon(nextAppIcon);
    void applyAppIcon(nextAppIcon);
  }, []);
  const appearanceChangeRevision = useRef(0);
  const domainLoadInFlight = useRef(new Map<DomainKey, Promise<void>>());
  const promptsRefreshInFlight = useRef<Promise<unknown[]> | null>(null);
  const sessionsRefreshInFlight = useRef<Promise<unknown> | null>(null);
  const sessionEventReady = useRef<Promise<void>>(Promise.resolve());
  const sessionEventFlushTimer = useRef<number | undefined>(undefined);
  const sessionScanGeneration = useRef(0);
  const completedSessionScans = useRef(new Set<number>());
  const sessionScanWaiters = useRef(new Map<number, Array<() => void>>());
  const pendingRecentSessions = useRef(new Map<string, SessionRecord>());
  const pendingWatchSessions = useRef(new Map<string, SessionRecord>());
  const pendingDeletedSessions = useRef(new Map<string, SessionIdentityRecord>());
  const updateOperationInFlight = useRef(false);
  const sidebarPanelRef = usePanelRef();
  const skillListRevision = useRef(0);
  const skillUpdateCheckRevision = useRef(0);
  const refreshSessionProjects = useCallback(async () => {
    const result = await safeInvoke<unknown[]>(TauriCommand.SessionProjectsList);
    if (Array.isArray(result)) setSessionProjects(result as SessionProjectSummary[]);
  }, []);
  const refreshProjects = useCallback(async () => {
    const [projectsResult, sessionProjectsResult] = await Promise.all([
      safeInvoke<unknown[]>(TauriCommand.ProjectsList),
      safeInvoke<unknown[]>(TauriCommand.SessionProjectsList),
    ]);
    if (Array.isArray(projectsResult)) setProjects(projectsResult as ProjectSummary[]);
    if (Array.isArray(sessionProjectsResult)) setSessionProjects(sessionProjectsResult as SessionProjectSummary[]);
  }, []);
  const applyUpdateCheckResult = useCallback((result: UpdateCheckResult, manual: boolean) => {
    if (result.status === "available" && result.version) {
      setDesktopUpdate({
        status: "available",
        version: result.version,
        body: result.body ?? undefined,
      });
      setUpdateNoticeDismissed(false);
      return;
    }
    if (result.status === "up-to-date") {
      setDesktopUpdate({ status: manual ? "up-to-date" : "idle" });
      return;
    }
    if (result.status === "busy" && manual) {
      setDesktopUpdate({ status: "error", error: "Another update operation is already running." });
    }
  }, []);
  const checkForUpdates = useCallback(async (manual = false) => {
    if (!isTauriRuntime() || updateOperationInFlight.current) return;
    updateOperationInFlight.current = true;
    if (manual) setDesktopUpdate({ status: "checking" });
    try {
      try {
        window.localStorage.setItem(AUTO_UPDATE_LAST_CHECK_KEY, `${Date.now()}`);
      } catch (error) {
        logger.warn("automatic desktop update check timestamp write failed", { error });
      }
      const result = await invokeCommand<UpdateCheckResult>(TauriCommand.CheckForUpdates);
      applyUpdateCheckResult(result, manual);
    } catch (error) {
      if (manual) {
        setDesktopUpdate({ status: "error", error: errorMessage(error) });
      } else {
        logger.warn("automatic desktop update check failed", { error });
      }
    } finally {
      updateOperationInFlight.current = false;
    }
  }, [applyUpdateCheckResult]);
  const installUpdate = useCallback(async () => {
    if (desktopUpdate.status !== "available" || updateOperationInFlight.current) return;
    updateOperationInFlight.current = true;
    setDesktopUpdate((current) => ({ ...current, status: "installing" }));
    try {
      const result = await invokeCommand<UpdateCheckResult>(TauriCommand.InstallUpdate);
      applyUpdateCheckResult(result, true);
    } catch (error) {
      setDesktopUpdate({ status: "error", error: errorMessage(error) });
    } finally {
      updateOperationInFlight.current = false;
    }
  }, [applyUpdateCheckResult, desktopUpdate.status]);
  const activeNav = navItems.find((item) => item.id === view);
  useEffect(() => {
    void refreshProjects();
  }, [refreshProjects]);
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let lastCheckedAt = 0;
    try {
      lastCheckedAt = Number(window.localStorage.getItem(AUTO_UPDATE_LAST_CHECK_KEY) ?? 0);
    } catch (error) {
      logger.warn("automatic desktop update check timestamp read failed", { error });
    }
    if (Number.isFinite(lastCheckedAt) && Date.now() - lastCheckedAt < AUTO_UPDATE_CHECK_INTERVAL_MS) return;
    const timer = window.setTimeout(() => { void checkForUpdates(); }, 1000);
    return () => window.clearTimeout(timer);
  }, [checkForUpdates]);
  useEffect(() => {
    if (!isTauriRuntime()) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void listen<UpdateCheckResult>(UPDATE_AVAILABLE_EVENT, (event) => {
      if (!disposed) applyUpdateCheckResult(event.payload, false);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    }).catch((error) => {
      logger.warn("desktop update event subscription failed", { error });
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [applyUpdateCheckResult]);
  const availableSidebarSources = useMemo(
    () => sidebarSources(
      agentTargets,
      data.sources.length || agentTargets.length ? data.sources : fallbackAgents,
      data.agents,
    ),
    [agentTargets, data.agents, data.sources],
  );
  const installedAgentKeys = useMemo(() => {
    const keys = new Set<string>();
    for (const agent of data.agents) {
      if (agent.installed === true) keys.add(agentIdentityKey(agent.name ?? agent.kind));
    }
    for (const source of data.sources) {
      if (source.count > 0) keys.add(agentIdentityKey(source.label));
    }
    return [...keys];
  }, [data.agents, data.sources]);
  useEffect(() => {
    if (agentFilter !== "All" && !availableSidebarSources.some((source) => source.label === agentFilter)) {
      setAgentFilter("All");
    }
  }, [agentFilter, availableSidebarSources]);
  useEffect(() => {
    const syncBackup = () => { void safeInvoke(TauriCommand.SkillsBackupSync); };
    const syncWhenVisible = () => {
      if (document.visibilityState === "visible") syncBackup();
    };
    window.addEventListener("focus", syncBackup);
    document.addEventListener("visibilitychange", syncWhenVisible);
    return () => {
      window.removeEventListener("focus", syncBackup);
      document.removeEventListener("visibilitychange", syncWhenVisible);
    };
  }, []);
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
      rules: data.rules.filter((rule) => ruleAgents(rule).some((agent) => sameAgent(agent, agentFilter))),
      hooks: data.hooks.filter((hook) => sameAgent(hook.agent, agentFilter)),
      mcp: data.mcp.filter((server) => sameAgent(server.agent, agentFilter)),
    };
  }, [agentFilter, data]);

  const setData = desktopStore.actions.updateData;
  const setSkillUpdateError = desktopStore.actions.setSkillUpdateError;
  const setCheckingSkillUpdates = desktopStore.actions.setSkillUpdatesChecking;
  const setSkillIndexStatus = desktopStore.actions.setSkillIndexStatus;
  const setAgentTargets = desktopStore.actions.setAgentTargets;
  const setAnalyticsRevision = desktopStore.actions.setAnalyticsRevision;
  const setAnalyticsRevisionReady = desktopStore.actions.setAnalyticsReady;
  const setAnalyticsRevisionError = desktopStore.actions.setAnalyticsError;
  const setSessionListStatus = useCallback((status: typeof sessions.listStatus) => {
    desktopStore.actions.setSessionListState({ listStatus: status });
  }, []);
  const setSessionListError = useCallback((message: string) => {
    desktopStore.actions.setSessionListState({ listError: message });
  }, []);
  const setSessionRefreshError = useCallback((message: string) => {
    desktopStore.actions.setSessionListState({ refreshError: message });
  }, []);

  const setDomainError = useCallback((domain: DomainKey, message: string) => {
    desktopStore.actions.setDomainError(domain, message);
  }, []);

  useEffect(() => {
    applyAppearance(appearance, themePreferences);
    if (appearance !== "system") return;
    return listenForSystemAppearanceChange(() => applyAppearance("system", themePreferences));
  }, [appearance, themePreferences]);

  useEffect(() => {
    applyFontFamily(fontFamily);
  }, [fontFamily]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "f") return;
      if (view === "sessions") return;
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
  }, [view]);

  useEffect(() => {
    if (view === "skills") void loadConfirmSkillChangesDialog();
  }, [view]);

  const changeAppearance = useCallback((nextAppearance: Appearance) => {
    appearanceChangeRevision.current += 1;
    setAppearance(nextAppearance);
  }, []);

  useEffect(() => {
    let cancelled = false;
    const revision = appearanceChangeRevision.current;
    safeInvoke<{ appearance?: unknown; lightTheme?: unknown; darkTheme?: unknown; appIcon?: unknown; fontFamily?: unknown; developerMode?: unknown; sessionResumeTarget?: unknown }>(TauriCommand.SettingsGet).then((settings) => {
      if (!cancelled && settings && revision === appearanceChangeRevision.current) {
        setAppearance(normalizeAppearance(settings.appearance));
        setThemePreferences(normalizeThemePreferences({ light: settings.lightTheme as ColorTheme, dark: settings.darkTheme as ColorTheme }));
        const normalizedAppIcon = normalizeAppIcon(settings.appIcon);
        setAppIcon(normalizedAppIcon);
        void applyAppIcon(normalizedAppIcon);
        setFontFamily(normalizeFontFamily(settings.fontFamily));
        setDeveloperMode(settings.developerMode === true);
        setSessionResumeTarget(normalizeSessionResumeTarget(settings.sessionResumeTarget));
      }
    });
    return () => { cancelled = true; };
  }, []);

  const refreshSkillList = async () => {
    const revision = ++skillListRevision.current;
    try {
      const skills = await invokeCommand<SkillRecord[]>(TauriCommand.SkillsList);
      if (revision !== skillListRevision.current) return null;
      setData((current) => {
        const normalized = normalizeReport({ ...current, skills });
        return replaceSkillReportPreservingUpdates(current, normalized);
      });
      desktopStore.actions.markDomainLoaded("skills");
      setDomainError("skills", "");
      return skills;
    } catch (error) {
      logger.error("skills list refresh failed", { revision, error });
      if (revision === skillListRevision.current) setDomainError("skills", errorMessage(error));
      return null;
    }
  };

  const refreshSkillListAndUpdates = async () => {
    const revision = ++skillListRevision.current;
    setCheckingSkillUpdates(true);
    try {
      const result = await invokeCommand<SkillRefreshResult>(TauriCommand.SkillsRefresh);
      if (revision !== skillListRevision.current) return null;
      setData((current) => {
        const next = normalizeReport({ ...current, skills: result.skills });
        return replaceSkillReportPreservingUpdates(current, next);
      });
      if (result.updates) desktopStore.actions.setSkillUpdateReports(result.updates, agentFilter);
      desktopStore.actions.markDomainLoaded("skills");
      setDomainError("skills", "");
      setCheckingSkillUpdates(result.updateCheck === "started" || result.updateCheck === "already-running");
      return result;
    } catch (error) {
      logger.error("skills refresh failed", { revision, error });
      if (revision === skillListRevision.current) {
        setDomainError("skills", errorMessage(error));
        setCheckingSkillUpdates(false);
      }
      return null;
    }
  };

  const applyInstalledSkills = (result: SkillInstallResult) => {
    if (!result.skills) return;
    setData((current) => {
      const next = normalizeReport({ ...current, skills: result.skills });
      return replaceSkillReportPreservingUpdates(current, next);
    });
  };

  const refreshSkillUpdates = useCallback(async () => {
    const revision = ++skillUpdateCheckRevision.current;
    setCheckingSkillUpdates(true);
    setSkillUpdateError("");
    try {
      await sessionEventReady.current;
      const result = await invokeCommand<{ updateCheck?: string; updates?: SkillUpdateReport[] }>(TauriCommand.SkillsUpdates, { check: true });
      if (revision !== skillUpdateCheckRevision.current) return;
      if (result.updateCheck !== "started" && result.updateCheck !== "already-running") {
        desktopStore.actions.setSkillUpdateReports(result.updates ?? [], agentFilter);
        setCheckingSkillUpdates(false);
      }
    } catch (error) {
      logger.error("skill updates check failed", { revision, error });
      if (revision === skillUpdateCheckRevision.current) {
        setCheckingSkillUpdates(false);
        setSkillUpdateError(errorMessage(error));
      }
    }
  }, [agentFilter]);

  const refreshSkills = async () => {
    setDomainLoading("skills", true);
    setDomainError("skills", "");
    try {
      await refreshSkillListAndUpdates();
    } finally {
      setDomainLoading("skills", false);
    }
  };

  const setDomainRows = (domain: DomainKey, rows: unknown[]) => {
    setData((current) => normalizeDomainRows(current, domain, rows));
    desktopStore.actions.markDomainLoaded(domain);
    setDomainError(domain, "");
  };

  const refreshPrompts = useCallback(async () => {
    setDomainLoading("prompts", true);
    setDomainError("prompts", "");
    try {
      if (!promptsRefreshInFlight.current) {
        promptsRefreshInFlight.current = (async () => {
          const rows = await invokeCommand<unknown[]>(TauriCommand.PromptsList);
          if (!Array.isArray(rows)) throw new Error("Invalid prompts response");
          setDomainRows("prompts", rows);
          return rows;
        })().finally(() => {
          promptsRefreshInFlight.current = null;
        });
      }
      return await promptsRefreshInFlight.current;
    } catch (error) {
      logger.error("prompts refresh failed", { error });
      setDomainError("prompts", errorMessage(error));
      return null;
    } finally {
      setDomainLoading("prompts", false);
    }
  }, []);

  const applyPromptSaved = useCallback((prompt: unknown) => {
    if (!prompt || typeof prompt !== "object") return;
    const id = `${(prompt as Record<string, unknown>).id ?? ""}`;
    setData((current) => normalizeReport({
      ...current,
      prompts: [prompt, ...current.prompts.filter((item) => item.id !== id)],
    }));
    desktopStore.actions.markDomainLoaded("prompts");
  }, []);

  const savePromptFromSession = useCallback(async (body: string) => {
    if (!body.trim()) return false;
    const prompt = await safeInvoke(TauriCommand.PromptSave, {
      id: null,
      title: promptTitleFromBody(body),
      tags: [],
      body,
    });
    if (!prompt) return false;
    applyPromptSaved(prompt);
    return true;
  }, [applyPromptSaved]);

  const removePrompts = useCallback((ids: string[]) => {
    const deleted = new Set(ids);
    setData((current) => ({
      ...current,
      prompts: current.prompts.filter((prompt) => !deleted.has(prompt.id)),
    }));
  }, []);

  const deleteHook = useCallback(async (hook: DomainRow) => {
    try {
      const rows = await invokeCommand(TauriCommand.HookDelete, hookDeleteArgs(hook));
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookDelete, error });
      return { error: `${error}` };
    }
  }, []);

  const deleteHooks = useCallback(async (hooks: DomainRow[]) => {
    try {
      const rows = await invokeCommand(TauriCommand.HookDeleteMany, {
        requests: hooks.map(hookDeleteArgs),
      });
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookDeleteMany, error });
      return { error: `${error}` };
    }
  }, []);

  const setHookEnabled = useCallback(async (hook: DomainRow, enabled: boolean) => {
    try {
      const rows = await invokeCommand(TauriCommand.HookSetEnabled, {
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
      logger.warn("tendi command failed", { command: TauriCommand.HookSetEnabled, error });
      return { error: `${error}` };
    }
  }, []);

  const reviewHook = useCallback(async (hook: DomainRow) => {
    try {
      const rows = await invokeCommand(TauriCommand.HookReview, {
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
      logger.warn("tendi command failed", { command: TauriCommand.HookReview, error });
      return { error: `${error}` };
    }
  }, []);

  const setSessionRows = useCallback((rows: unknown[], { markLoaded = true, markReady = true } = {}) => {
    if (markReady) {
      setSessionListStatus("loaded");
      setSessionListError("");
    }
    setData((current) => {
      const nextSessions = mergeSessionRows(current.sessions, rows as Array<Record<string, unknown>>);
      if (nextSessions === current.sessions) return current;
      return { ...current, sessions: nextSessions };
    });
    if (markLoaded) desktopStore.actions.markDomainLoaded("sessions");
    if (rows.length > 0) desktopStore.actions.setSessionListState({ rowsAvailable: true });
    if (markReady) desktopStore.actions.setSessionListState({ listLoaded: true });
  }, []);

  const setSessionLoadError = useCallback(() => {
    if (desktopStore.getSnapshot().sessions.rowsAvailable) {
      setSessionListStatus("loaded");
      setSessionListError("");
      return;
    }
    setSessionListStatus("error");
    setSessionListError(SESSION_LOAD_ERROR);
  }, []);

  const flushSessionEventBuffer = useCallback(() => {
    const recent = [...pendingRecentSessions.current.values()];
    const watch = [...pendingWatchSessions.current.values()];
    const deleted = [...pendingDeletedSessions.current.values()];
    pendingRecentSessions.current.clear();
    pendingWatchSessions.current.clear();
    pendingDeletedSessions.current.clear();
    sessionEventFlushTimer.current = undefined;
    if (recent.length === 0 && watch.length === 0 && deleted.length === 0) return;
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
  }, []);

  const scheduleSessionEventFlush = useCallback(() => {
    if (sessionEventFlushTimer.current !== undefined) return;
    sessionEventFlushTimer.current = window.setTimeout(flushSessionEventBuffer, SESSION_EVENT_FLUSH_MS);
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
    if (event.phase === "recent") {
      for (const session of upserts) pendingRecentSessions.current.set(sessionLogicalIdentity(session), session);
    } else if (event.phase === "watch") {
      for (const session of upserts) pendingWatchSessions.current.set(sessionLogicalIdentity(session), session);
    }
    for (const identity of event.deleted ?? []) {
      pendingDeletedSessions.current.set(
        sessionIdentity({ id: `${identity.id ?? ""}`, agent: `${identity.agent ?? ""}`, path: `${identity.path ?? ""}` }),
        identity,
      );
    }
    scheduleSessionEventFlush();
    if (event.phase === "recent" && event.complete) {
      setSessionRefreshError("");
      if (sessionEventFlushTimer.current !== undefined) window.clearTimeout(sessionEventFlushTimer.current);
      flushSessionEventBuffer();
    }
    if (event.phase === "backfill" && event.complete) {
      void (async () => {
        try {
          const rows = await invokeCommand<unknown[]>(TauriCommand.SessionsList);
          if (!Array.isArray(rows)) throw new Error("Invalid sessions response");
          setSessionRows(rows);
          const status = await invokeCommand<SkillIndexStatus>(TauriCommand.SessionSkillIndexRun, { force: false });
          setSkillIndexStatus(status);
          setSessionRefreshError("");
        } catch (error) {
          logger.error("sessions backfill finalization failed", {
            generation: event.generation,
            phase: event.phase,
            error,
          });
          setSessionRefreshError(SESSION_REFRESH_ERROR);
          setSessionLoadError();
        } finally {
          finishSessionScanWaiters(event.generation);
        }
      })();
    } else if (event.phase === "error") {
      logger.error("sessions scan failed", {
        generation: event.generation,
        phase: event.phase,
        scanned: event.scanned ?? 0,
        complete: event.complete ?? false,
        error: event.error ?? "unknown session scan error",
      });
      if (!event.complete) return;
      setSessionRefreshError(SESSION_REFRESH_ERROR);
      setSessionLoadError();
      finishSessionScanWaiters(event.generation);
    } else if (event.phase === "watch" && event.complete) {
      setSessionRefreshError("");
    }
  }, [finishSessionScanWaiters, flushSessionEventBuffer, scheduleSessionEventFlush, setSessionLoadError, setSessionRows]);

  useEffect(() => {
    let disposed = false;
    let unsubscribe: (() => void) | null = null;
    const setup = subscribeDaemonEvents((event: DaemonEvent) => {
      if (disposed) return;
      if (event.event === "sessions://scan") {
        handleSessionScanEvent(event.payload as SessionScanEvent);
      } else if (event.event === "analytics://revision") {
        const revision = Number((event.payload as { revision?: unknown }).revision);
        if (Number.isFinite(revision)) {
          setAnalyticsRevision(revision);
          setAnalyticsRevisionReady(true);
          setAnalyticsRevisionError("");
        }
      } else if (event.event === "skills://updates") {
        const payload = event.payload as SkillUpdateCheckEvent;
        if (payload.status === "completed") {
          if (Array.isArray(payload.skills)) {
            setData((current) => {
              const next = normalizeReport({ ...current, skills: payload.skills });
              return replaceSkillReportPreservingUpdates(current, next);
            });
            desktopStore.actions.markDomainLoaded("skills");
            desktopStore.actions.setDomainError("skills", "");
          }
          desktopStore.actions.setSkillUpdateReports(payload.updates ?? [], agentFilter);
          setSkillUpdateError("");
        } else {
          setSkillUpdateError(payload.error || "Update check failed");
        }
        setCheckingSkillUpdates(false);
      }
    });
    sessionEventReady.current = setup.then((cleanup) => {
      if (disposed) cleanup();
      else unsubscribe = cleanup;
    }).catch((error) => {
      if (!disposed) logger.warn("daemon event subscription failed", { error });
    });
    return () => {
      disposed = true;
      unsubscribe?.();
      if (sessionEventFlushTimer.current !== undefined) {
        window.clearTimeout(sessionEventFlushTimer.current);
        sessionEventFlushTimer.current = undefined;
      }
    };
  }, [agentFilter, handleSessionScanEvent]);

  useEffect(() => {
    void refreshSkillUpdates();
  }, [refreshSkillUpdates]);

  const refreshAnalyticsRevision = useCallback(async () => {
    setAnalyticsRevisionError("");
    setAnalyticsRevisionReady(false);
    try {
      const revision = await invokeCommand<number>(TauriCommand.AnalyticsRevision);
      setAnalyticsRevision(revision);
      setAnalyticsRevisionReady(true);
    } catch (error) {
      logger.error("analytics revision load failed", { error });
      setAnalyticsRevisionError(errorMessage(error));
    }
  }, []);

  useEffect(() => {
    void refreshAnalyticsRevision();
  }, [refreshAnalyticsRevision]);

  const refreshSessionsFromScan = useCallback((showLoading = true) => {
    setSessionRefreshError("");
    if (showLoading) {
      setSessionListStatus("loading");
      setSessionListError("");
    }
    if (!sessionsRefreshInFlight.current) {
      sessionsRefreshInFlight.current = sessionEventReady.current
        .then(() => invokeCommand<number>(TauriCommand.SessionsScanStart))
        .then(async (generation) => {
          sessionScanGeneration.current = Math.max(sessionScanGeneration.current, generation);
          if (!completedSessionScans.current.has(generation)) {
            await new Promise<void>((resolve) => {
              const waiters = sessionScanWaiters.current.get(generation) ?? [];
              waiters.push(resolve);
              sessionScanWaiters.current.set(generation, waiters);
            });
          }
          await refreshSessionProjects();
          return generation;
        })
        .catch((error) => {
          logger.error("sessions scan start failed", { error });
          setSessionRefreshError(SESSION_REFRESH_ERROR);
          setSessionLoadError();
          return null;
        })
        .finally(() => {
          sessionsRefreshInFlight.current = null;
        });
    }
    return sessionsRefreshInFlight.current;
  }, [refreshSessionProjects]);

  const refreshSkillIndexStatus = useCallback(async () => {
    const status = await safeInvoke(TauriCommand.SessionSkillIndexStatus);
    if (status) setSkillIndexStatus(status);
    return status;
  }, []);

  const setDomainLoading = (domain: string, loading: boolean) => {
    if (!isDomainKey(domain)) return;
    desktopStore.actions.setDomainLoading(domain, loading);
  };

  const loadDomainForRetry = useCallback((domain: DomainKey) => {
    desktopStore.actions.markDomainLoaded(domain, false);
    if (domain === "sessions") {
      desktopStore.actions.setSessionListState({ listLoaded: false });
      setSessionListStatus("loading");
      setSessionListError("");
      setSessionRefreshError("");
    }
    setDomainLoading(domain, true);
    desktopStore.actions.bumpDomainRetryRevision();
  }, [setSessionListError, setSessionListStatus, setSessionRefreshError]);

  const loadOverviewCount = useCallback(async (domain: DomainKey) => {
    desktopStore.actions.setInventoryError(domain, false);
    try {
      const result = await invokeCommand<OverviewDomainCount>(TauriCommand.OverviewCount, {
        domain,
        agent: agentFilter === "All" ? null : agentFilter,
      });
      if (!Number.isFinite(result.count) || !Number.isFinite(result.secondaryCount)) {
        throw new Error(`Invalid ${domain} count response`);
      }
      desktopStore.actions.setInventoryCount(domain, result.count, result.secondaryCount);
    } catch (error) {
      logger.warn("tendi overview count failed", { domain, error });
      desktopStore.actions.setInventoryError(domain, true);
    }
  }, [agentFilter]);

  useEffect(() => {
    let cancelled = false;
    safeInvoke(TauriCommand.AgentsList).then((agents) => {
      if (!cancelled && Array.isArray(agents)) {
        setData((current) => normalizeReport({ ...current, agents }));
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    safeInvoke<AgentTargetOption[]>(TauriCommand.SkillsTargets).then((targets) => {
      if (!cancelled && Array.isArray(targets)) setAgentTargets(targets);
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
      await invokeCommand(TauriCommand.BundledSkillPromptDismiss);
      setBundledSkillPrompt(null);
    } catch (error) {
      logger.error("bundled skill prompt dismiss failed", { error });
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
      const cliStatus = await invokeCommand<CliInstallStatus>(TauriCommand.CliInstall);
      if (cliStatus.state !== "installed" || !cliStatus.pathConfigured) {
        throw new Error(cliStatus.detail || "The Tendi CLI is not available on PATH.");
      }
      await invokeCommand(TauriCommand.BundledSkillInstall);
      setBundledSkillPrompt(null);
      void refreshSkillList();
    } catch (error) {
      logger.error("bundled skill install failed", { error });
      setBundledSkillError(`${error}`);
    } finally {
      setBundledSkillBusy(false);
    }
  };

  useEffect(() => {
    const loadOneDomain = async (domain: DomainKey) => {
      if (desktopStore.getSnapshot().catalogs.loadedDomains.has(domain)) return;
      const existing = domainLoadInFlight.current.get(domain);
      if (existing) {
        await existing;
        if (domain === "sessions" && view !== "overview" && !desktopStore.getSnapshot().catalogs.loadedDomains.has(domain)) {
          await refreshSessionsFromScan();
        }
        return;
      }
      const request = (async () => {
        setDomainLoading(domain, true);
        setDomainError(domain, "");
        try {
          if (domain === "skills") {
            await refreshSkillListAndUpdates();
            return;
          }
          if (domain === "sessions") {
            if (!desktopStore.getSnapshot().sessions.listLoaded) {
              setSessionListStatus("loading");
              setSessionListError("");
              try {
                const cachedRows = await invokeCommand<unknown[]>(TauriCommand.SessionsList);
                setSessionRows(cachedRows, {
                  markLoaded: view !== "overview",
                  markReady: view !== "overview" || cachedRows.length > 0,
                });
              } catch (error) {
                logger.error("sessions cached list load failed", { view, error });
                setSessionRefreshError(SESSION_REFRESH_ERROR);
                setSessionLoadError();
              }
            }
            if (view === "overview") {
              void refreshSessionsFromScan(false);
              return;
            }
            await refreshSessionsFromScan();
            return;
          }
          if (domain === "prompts") {
            await refreshPrompts();
            return;
          }
          const rows = await invokeCommand<unknown[]>(domainListCommands[domain]);
          if (!Array.isArray(rows)) throw new Error(`Invalid ${domain} response`);
          setDomainRows(domain, rows);
    } catch (error) {
      logger.error("domain load failed", { domain, error });
      setDomainError(domain, errorMessage(error));
          if (domain === "sessions") {
            setSessionRefreshError(SESSION_REFRESH_ERROR);
            setSessionLoadError();
          }
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
      desktopStore.actions.resetInventory(agentFilter);
      void (async () => {
        await loadOneDomain("sessions");
        for (const domain of DOMAIN_KEYS) {
          await loadOverviewCount(domain);
        }
      })();
      return () => undefined;
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
    };
  }, [agentFilter, domainRetryRevision, loadOverviewCount, refreshPrompts, refreshSessionsFromScan, setDomainError, setSessionLoadError, setSessionRows, view]);

  useEffect(() => {
    if (view !== "overview" || sessionListStatus === "loading") return;
    const count = data.sessions.filter((session) => (
      agentFilter === "All" || sameAgent(session.agent, agentFilter)
    )).length;
    desktopStore.actions.setInventoryCount("sessions", count);
  }, [agentFilter, data.sessions, sessionListStatus, view]);

  useEffect(() => {
    desktopStore.actions.refreshSkillUpdateCount(agentFilter);
  }, [agentFilter, data.skills]);

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

  const navigateTo = useCallback((next: ViewId) => {
    setView(next);
  }, []);

  const navigate = useCallback((next: ViewId) => {
    navigateTo(next);
    setActiveSkill(null);
    setActiveRule(null);
  }, [navigateTo]);

  const loadTranscript = useCallback(async (session: SessionRecord, cursor?: string, knownSourceVersion?: string): Promise<TranscriptPage> => {
    if (!session?.path) return { items: [], locatorItems: [], warnings: [], done: true, sourceVersion: "", restartRequired: false, unchanged: false };
    const page = await invokeCommand(TauriCommand.SessionTranscript, {
      path: session.path,
      agent: session.agent,
      cursor,
      limit: 160,
      knownSourceVersion,
    });
    return normalizeTranscriptPage(page);
  }, []);

  const loadTranscriptLocator = useCallback(async (session: SessionRecord): Promise<TranscriptLocatorPage> => {
    if (!session?.path) return { locatorItems: [], warnings: [], sourceVersion: "" };
    const page = await invokeCommand(TauriCommand.SessionTranscriptLocator, {
      path: session.path,
      agent: session.agent,
    });
    return normalizeTranscriptLocatorPage(page);
  }, []);

  const searchTranscript = useCallback(async (session: SessionRecord, query: string, scopes: TranscriptSearchScopes): Promise<TranscriptSearchResult | null> => {
    if (!session?.path) return null;
    const result = await invokeCommand(TauriCommand.SessionTranscriptSearch, {
      path: session.path,
      agent: session.agent,
      query,
      scopes,
    });
    return normalizeTranscriptSearchResult(result);
  }, []);

  const searchSessions = useCallback(async (query: string) => {
    try {
      const rows = await invokeCommand<unknown[]>(TauriCommand.SessionsSearch, { query });
      if (!Array.isArray(rows)) throw new Error("Invalid session search response");
      return rows.map((row, index) => normalizeSession(row as Record<string, unknown>, index)) as SessionRecord[];
    } catch (error) {
      logger.error("session search failed", { query, error });
      throw error;
    }
  }, []);

  const loadSessionSkillLinks = useCallback(async (session: SessionRecord) => {
    if (!session?.id) return [];
    const links = await invokeCommand<unknown[]>(TauriCommand.SessionSkillLinks, { sessionId: session.id, agent: session.agent });
    if (!Array.isArray(links)) throw new Error("Invalid session skill links response");
    return links as SkillLinkRecord[];
  }, []);

  const openSkillByName = useCallback((skillName: string) => {
    const skill = data.skills.find((item) => item.name === skillName) as SkillRecord | undefined;
    if (skill) setActiveSkill(skill);
    navigateTo(skill ? "skillDetail" : "skills");
  }, [data.skills, navigateTo]);

  const openSessionFromLink = useCallback((link: DomainRow) => {
    setActiveSessionKey(`${link.agent ?? ""}:${link.session_id ?? link.sessionId ?? ""}`);
    navigateTo("sessions");
  }, [navigateTo]);

  const resolveSessionResumeTarget = useCallback(async (session: SessionRecord): Promise<"terminal" | "app"> => {
    const configuredTarget = sessionResumeTargetForAgent(sessionResumeTarget, session.agent);
    if (configuredTarget !== "auto") return configuredTarget;
    try {
      const inferredTarget = await invokeCommand<"terminal" | "app">(TauriCommand.SessionResumeTarget, {
        session: sessionLaunchPayload(session),
      });
      return inferredTarget === "app" ? "app" : "terminal";
    } catch (error) {
      logger.warn("session resume target inference failed; using terminal", { error });
      return "terminal";
    }
  }, [sessionResumeTarget]);

  const resumeSession = useCallback(async (session: SessionRecord, options: SessionResumeOptions = {}): Promise<SessionResumeOutcome> => {
    if (await resolveSessionResumeTarget(session) === "app") {
      const appUrl = sessionAppDeepLink(session);
      if (appUrl) {
        try {
          await invokeCommand(TauriCommand.OpenUrl, { url: appUrl });
          return { status: "launched", target: "app" };
        } catch (error) {
          logger.warn("session app resume unavailable; using terminal", { agent: session.agent, error });
        }
      }
    }
    const result = await invokeCommand<
      | { status: "activeWriter"; lockPath: string; pids: number[] }
      | { status: "launched"; agent: string; terminal: string; commandLine: string }
    >(TauriCommand.SessionResumeInTerminal, {
      session: sessionLaunchPayload(session),
      forceActiveWriter: options.forceActiveWriter ?? false,
    });
    if (result.status === "activeWriter") return result;
    return { status: "launched", target: "terminal", terminal: result.terminal };
  }, [resolveSessionResumeTarget]);

  const previewAndApply = async (
    command: SkillChangeCommand,
    args: Record<string, unknown>,
    { onApplied }: { onApplied?: () => void } = {},
  ) => {
    const isUpdate = command === SkillChangeCommand.UpdateMany;
    const isDelete = command === SkillChangeCommand.DeleteMany;
    if (isDelete) {
      setPendingSkillChange({ command, args, preview: null, onApplied });
      return;
    }
    if (isUpdate) {
      setSkillUpdateError("");
      setPendingSkillChange({ command, args, preview: null, onApplied });
    }
    try {
      const preview = isUpdate
        ? await invokeCommand<SkillPreview>(command, { ...args, dryRun: true })
        : await safeInvoke(command, { ...args, dryRun: true }) as SkillPreview | null;
      if (!preview) return;
      if (isUpdate) {
        setPendingSkillChange((current) => current?.command === command ? { ...current, preview } : current);
      } else {
        setPendingSkillChange({ command, args, preview, onApplied });
      }
    } catch (error) {
      logger.error("skill change preview failed", { command, error });
      const message = `${error}`;
      setSkillUpdateError(message);
      if (isUpdate) setPendingSkillChange((current) => current?.command === command ? { ...current, previewError: message } : current);
      if (isDelete) setPendingSkillChange((current) => current?.command === command && current.args === args ? { ...current, previewError: message } : current);
    }
  };

  const applySkillChange = async (command: SkillChangeCommand, args: Record<string, unknown>) => {
    const result = await invokeCommand<SkillPreview>(command, { ...args, dryRun: false });
    if (result.skills) {
      setData((current) => {
        const next = normalizeReport({ ...current, skills: result.skills });
        return replaceSkillReportPreservingUpdates(current, next, typeof args.name === "string" ? [args.name] : []);
      });
    }
    return result;
  };

  const closeSkillChangeDialog = (open: boolean) => {
    if (!open && !applyingSkillChange) setPendingSkillChange(null);
  };

  const confirmSkillChange = async (resolutions: Record<string, string> = {}) => {
    if (!pendingSkillChange || applyingSkillChange) return;
    setApplyingSkillChange(true);
    const { command, args, onApplied } = pendingSkillChange;
    setPendingSkillChange((current) => current ? { ...current, applyError: undefined } : current);
    if (command === SkillChangeCommand.UpdateMany) setSkillUpdateError("");
    const previewId = command === SkillChangeCommand.UpdateMany
      ? pendingSkillChange.preview?.previewId
      : undefined;
    try {
      const result = await invokeCommand<SkillPreview>(command, {
        ...args,
        dryRun: false,
        ...(command === SkillChangeCommand.UpdateMany ? { previewId, resolutions } : {}),
      });
      if (result.skills) {
        setData((current) => {
          const next = normalizeReport({ ...current, skills: result.skills });
          return replaceSkillReportPreservingUpdates(current, next, (args.names as unknown[] | undefined)?.map((name) => `${name}`) ?? []);
        });
      } else if (command === SkillChangeCommand.DeleteMany) {
        const names = new Set((args.names as unknown[] | undefined)?.map((name) => `${name}`) ?? []);
        setData((current) => normalizeReport({
          ...current,
          skills: current.skills.filter((skill) => !names.has(skill.name)),
        }));
      } else if (command === SkillChangeCommand.UpdateMany) {
        const names = (args.names as unknown[] | undefined)?.map((name) => `${name}`) ?? [];
        setData((current) => clearSkillUpdateAvailability(current, names));
      }
      if (command === SkillChangeCommand.UpdateMany) {
        const names = (args.names as unknown[] | undefined)?.map((name) => `${name}`) ?? [];
        desktopStore.actions.clearSkillUpdateReports(names, agentFilter);
      }
      setPendingSkillChange(null);
      onApplied?.();
      if (!result.skills && (command !== SkillChangeCommand.DeleteMany || result?.refreshRequired)) {
        void refreshSkillList();
      }
    } catch (error) {
      logger.error("skill change apply failed", { command, error });
      const message = `${error}`;
      setPendingSkillChange((current) => current?.command === command ? { ...current, applyError: message } : current);
      if (command === SkillChangeCommand.UpdateMany) setSkillUpdateError(message);
    } finally {
      setApplyingSkillChange(false);
    }
  };

  const applyVisibility = async (names: string[], visibility: SkillVisibility) => {
    setData((current) => applyVisibilityState(current, names, visibility));
    const result = await safeInvoke<SkillPreview>(SkillChangeCommand.Set, { names, visibility, dryRun: false });
    if (result?.skills) {
      setData((current) => {
        const next = normalizeReport({ ...current, skills: result.skills });
        return replaceSkillReportPreservingUpdates(current, next);
      });
    }
    if (!result) await refreshSkillList();
  };

  const forceSidebarResizeHover =
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("forceSidebarResizeHover") === "1";
  const sidebarSize = sidebarCollapsed ? COLLAPSED_SIDEBAR_SIZE : SIDEBAR_SIZE;

  const updateToast = desktopUpdate.status === "available" && desktopUpdate.version && !updateNoticeDismissed ? (
    <Toast
      tone="success"
      message={`Tendi ${desktopUpdate.version} is available.`}
      action={{ label: "Install", onClick: () => { void installUpdate(); } }}
      onDismiss={() => setUpdateNoticeDismissed(true)}
    />
  ) : desktopUpdate.status === "installing" ? (
    <Toast tone="info" message="Installing Tendi update…" />
  ) : desktopUpdate.status === "error" && desktopUpdate.error ? (
    <Toast tone="error" message={`Update failed: ${desktopUpdate.error}`} onDismiss={() => setDesktopUpdate({ status: "idle" })} />
  ) : null;

  useLayoutEffect(() => {
    sidebarPanelRef.current?.resize(Number.parseFloat(sidebarSize));
  }, [sidebarPanelRef, sidebarSize]);

  return (
    <main className="appShell">
      {updateToast}
      {bundledSkillPrompt ? (
        <Suspense fallback={(
          <DialogLoadingFallback
            title="Set up Tendi for coding agents?"
            label="Loading setup dialog"
            descriptionId="bundled-skill-loading-description"
            onOpenChange={(open) => { if (!open && !bundledSkillBusy) void dismissBundledSkillPrompt(); }}
          />
        )}>
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
        <Suspense fallback={(
          <SkillChangeDialogFallback
            command={pendingSkillChange.command}
            names={Array.isArray(pendingSkillChange.args.names) ? pendingSkillChange.args.names.map((name) => `${name}`) : []}
            onOpenChange={closeSkillChangeDialog}
            onConfirm={() => { void confirmSkillChange(); }}
          />
        )}>
          <ConfirmSkillChangesDialog
            open
            command={pendingSkillChange.command}
            names={Array.isArray(pendingSkillChange.args.names) ? pendingSkillChange.args.names.map((name) => `${name}`) : []}
            preview={pendingSkillChange.preview ?? null}
            previewError={pendingSkillChange.previewError}
            applyError={pendingSkillChange.applyError}
            busy={applyingSkillChange}
            onOpenChange={closeSkillChangeDialog}
            onConfirm={(resolutions) => { void confirmSkillChange(resolutions); }}
          />
        </Suspense>
      ) : null}
      <PanelGroup
        className={`window ${sidebarCollapsed ? "sidebarCollapsed" : ""} ${forceSidebarResizeHover ? "forceSidebarResizeHover" : ""}`}
        orientation="horizontal"
        disabled
      >
        <Panel
          className="sidebarPanel"
          collapsible={false}
          defaultSize={SIDEBAR_SIZE}
          groupResizeBehavior="preserve-pixel-size"
          minSize={COLLAPSED_SIDEBAR_SIZE}
          maxSize={SIDEBAR_SIZE}
          panelRef={sidebarPanelRef}
          {...({ order: 1 } as Record<string, unknown>)}
        >
          <Sidebar
            view={view === "skillDetail" ? "skills" : view === "ruleDetail" ? "rules" : view}
            setView={navigate}
            sources={availableSidebarSources}
            collapsed={sidebarCollapsed}
            setCollapsed={setSidebarCollapsed}
            agentFilter={agentFilter}
            setAgentFilter={setAgentFilter}
          />
        </Panel>
        <Panel className="mainPanel" minSize="520px" {...({ order: 2 } as Record<string, unknown>)}>
          {contentView === "skillDetail" ? (activeSkill ? (
            <SkillEditorView
              skill={activeSkill}
              skills={data.skills}
              back={() => navigate("skills")}
              skillIndexStatus={skillIndexStatus}
              onOpenSession={openSessionFromLink}
              onOpenSkill={openSkillByName}
              onSaved={(skills) => {
                if (skills) {
                  setData((current) => {
                    const next = normalizeReport({ ...current, skills });
                    return replaceSkillReportPreservingUpdates(current, next);
                  });
                }
              }}
            />
          ) : (
            <PlaceholderView title="Skill unavailable" />
          )) : contentView === "ruleDetail" ? (
            <RuleEditorView rule={activeRule} back={() => navigate("rules")} />
          ) : contentView === "skills" ? (
            <SkillsView
              skills={filteredData.skills as SkillRecord[]}
              projects={projects}
              installedAgentKeys={installedAgentKeys}
              loadingSkills={loadingDomains.has("skills")}
              loadError={domainErrors.skills ?? ""}
              hasRows={data.skills.length > 0}
              checkingUpdates={checkingSkillUpdates}
              updateError={skillUpdateError}
              onRefresh={refreshSkills}
              onSetVisibility={applyVisibility}
              onApplyWrapper={(args) => applySkillChange(SkillChangeCommand.Wrap, args)}
              onApplyUpdates={(names, onApplied) => previewAndApply(SkillChangeCommand.UpdateMany, { names }, { onApplied })}
              onDeleteSkills={(names, onApplied) => previewAndApply(SkillChangeCommand.DeleteMany, { names }, { onApplied })}
              onAddInstalled={applyInstalledSkills}
              openSkill={(skill) => {
                setActiveSkill(skill);
                navigateTo("skillDetail");
              }}
            />
          ) : contentView === "backup" ? (
            <BackupView />
          ) : contentView === "sessions" ? (
            <SessionsView
              sessions={filteredData.sessions as SessionRecord[]}
              developerMode={developerMode}
              loadTranscript={loadTranscript}
              loadTranscriptLocator={loadTranscriptLocator}
              searchTranscript={searchTranscript}
              searchSessions={searchSessions}
              loadSessionSkillLinks={loadSessionSkillLinks}
              loadingSessions={loadingDomains.has("sessions")}
              sessionListError={sessionListError}
              sessionRefreshError={sessionRefreshError}
              onRefreshSessions={refreshSessionsFromScan}
              onResumeSession={resumeSession}
              sessionResumeTarget={sessionResumeTarget}
              missingSessionProjectPolicy={missingSessionProjectPolicy}
              projects={projects}
              sessionProjects={sessionProjects}
              onOpenSkill={openSkillByName}
              activeSessionKey={activeSessionKey}
              skillIndexStatus={skillIndexStatus}
              onSavePrompt={savePromptFromSession}
            />
          ) : contentView === "prompts" ? (
            <PromptsView
              prompts={filteredData.prompts as ComponentProps<typeof PromptsView>["prompts"]}
              loadingPrompts={loadingDomains.has("prompts")}
              loadError={domainErrors.prompts ?? ""}
              hasRows={data.prompts.length > 0}
              onRefreshPrompts={() => loadDomainForRetry("prompts")}
              onPromptSaved={applyPromptSaved}
              onPromptsDeleted={removePrompts}
            />
          ) : contentView === "rules" ? (
              <RulesView rows={filteredData.rules} skills={data.skills} projects={projects} loadingRows={loadingDomains.has("rules")} loadError={domainErrors.rules ?? ""} hasRows={data.rules.length > 0} onRetry={() => { void loadDomainForRetry("rules"); }} onOpenSkill={openSkillByName} />
            ) : contentView === "hooks" ? (
              <HooksView rows={filteredData.hooks} projects={projects} loadingRows={loadingDomains.has("hooks")} loadError={domainErrors.hooks ?? ""} hasRows={data.hooks.length > 0} onRetry={() => { void loadDomainForRetry("hooks"); }} onDeleteHook={deleteHook} onDeleteHooks={deleteHooks} onSetHookEnabled={setHookEnabled} onReviewHook={reviewHook} />
            ) : contentView === "mcp" ? (
            <DataListView title="MCP" rows={filteredData.mcp} columns={mcpColumns} projects={projects} loading={loadingDomains.has("mcp")} loadError={domainErrors.mcp ?? ""} hasRows={data.mcp.length > 0} onRetry={() => { void loadDomainForRetry("mcp"); }} />
          ) : contentView === "config" ? (
            <ConfigView />
          ) : contentView === "settings" ? (
            <SettingsView
              appearance={appearance}
              themePreferences={themePreferences}
              fontFamily={fontFamily}
              appIcon={appIcon}
              developerMode={developerMode}
              sessionResumeTarget={sessionResumeTarget}
              missingSessionProjectPolicy={missingSessionProjectPolicy}
              onAppearanceChange={changeAppearance}
              onFontFamilyChange={setFontFamily}
              onDeveloperModeChange={setDeveloperMode}
              onSessionResumeTargetChange={setSessionResumeTarget}
              onMissingSessionProjectPolicyChange={setMissingSessionProjectPolicy}
              onAppIconChange={changeAppIcon}
              projects={projects}
              onProjectsScanned={(nextProjects) => {
                setProjects(nextProjects);
                void refreshSessionProjects();
                loadDomainForRetry("skills");
                loadDomainForRetry("rules");
                loadDomainForRetry("mcp");
              }}
              update={desktopUpdate}
              onCheckForUpdates={() => { void checkForUpdates(true); }}
              onInstallUpdate={() => { void installUpdate(); }}
              onThemeChange={(mode, theme) => {
                setThemePreferences((current) => ({ ...current, [mode]: theme }));
              }}
            />
          ) : contentView === "overview" ? (
            <OverviewView
              counts={{
                ...overviewCounts,
              }}
              hookReviewCount={overviewHookReviewCount}
              skills={filteredData.skills as SkillRecord[]}
              sessions={filteredData.sessions as SessionRecord[]}
              skillUpdateCount={overviewSkillUpdateCount}
              analyticsRevision={analyticsRevision}
              analyticsRevisionReady={analyticsRevisionReady}
              analyticsRevisionError={analyticsRevisionError}
              onRetryAnalyticsRevision={refreshAnalyticsRevision}
              agentFilter={agentFilter}
              overviewCountsLoaded={overviewCountsLoaded}
              overviewCountErrors={overviewCountErrors}
              sessionListStatus={sessionListStatus}
              sessionListError={sessionListError}
              onNavigate={(id: OverviewNavId) => navigate(id)}
              onOpenSession={(session) => {
                setActiveSessionKey(`${session.agent ?? ""}:${session.id ?? ""}`);
                navigate("sessions");
              }}
            />
          ) : (
            <PlaceholderView title={activeNav?.label ?? "Overview"} />
          )}
        </Panel>
      </PanelGroup>
    </main>
  );
}
