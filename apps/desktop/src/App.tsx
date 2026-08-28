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
import { COLLAPSED_SIDEBAR_SIZE, DOMAIN_KEYS, SIDEBAR_SIZE, SkillChangeCommand, SkillVisibility, TauriCommand, UPDATE_AVAILABLE_EVENT, agentIdentityKey, applySessionDelta, applyVisibilityState, clearSkillUpdateAvailability, dialogCopy, hookSourcePath, hookTrustHash, invokeCommand, isConcreteAgent, isTauriRuntime, logger, mergeSessionRows, navItems, normalizeConfigProfiles, normalizeDomainRows, normalizeMissingSessionProjectPolicy, normalizePrompt, normalizeSession, normalizeSessionResumeTarget, normalizeSessionSkillLink, normalizeTranscriptLocatorPage, normalizeTranscriptPage, normalizeTranscriptSearchResult, promptTitleFromBody, recomputeSources, replaceSkillReportPreservingUpdates, ruleAgents, safeInvoke, sameAgent, sessionAppDeepLink, sessionIdentity, sessionIdentityRecordKey, sessionLaunchPayload, sessionLogicalIdentity, sessionResumeTargetForAgent, skillChangeActionLabel, skillChangeDescription, skillChangeBusyLabel, skillChangeLoadingCopy, skillChangeTitle, subscribeDaemonEvents } from "./lib/index.ts";
import type { BundledSkillStatus, CliInstallStatus, DaemonEvent, DesktopUpdateState, DomainKey, HookRecord, McpRecord, MissingSessionProjectPolicy, NormalizedSkill, ProjectSummary, SessionIdentityRecord, SessionProjectSummary, SessionRecord, SessionResumeOutcome, SessionResumeTarget, SessionSkillLinkRecord, SettingsPayload, TranscriptLocatorPage, TranscriptPage, TranscriptSearchResult, TranscriptSearchScopes, UpdateCheckResult } from "./lib/index.ts";
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
import type { RawSkillRecord, SkillInstallResult, WrapperArgs } from "./lib/index.ts";
import { ConfigView } from "./views/ConfigView.tsx";
import { DataListView } from "./views/McpView.tsx";
import { HooksView } from "./views/HooksView.tsx";
import { PromptsView } from "./views/PromptsView.tsx";
import { RulesView } from "./views/RulesView.tsx";
import { SessionsView } from "./views/SessionsView.tsx";
import { SkillsView } from "./views/SkillsView.tsx";
import { SettingsView } from "./features/settings/SettingsView.tsx";
import { SkillEditorView } from "./features/skills/SkillEditorView.tsx";
import { OverviewView } from "./views/OverviewView.tsx";
import { desktopStore, selectCatalogCountErrors, selectCatalogCountLoadedDomains, selectCatalogCounts, selectHookReviewCount, selectSessionListStatus, selectSkillUpdateCount, useDesktopStore, type AgentTargetOption, type SkillIndexStatus, type SkillUpdateReport } from "./store/desktop-store.ts";

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
  | "skillDetail";

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
  const actionLabel = skillChangeActionLabel(command);
  return (
    <DialogShell
      open
      onOpenChange={onOpenChange}
      descriptionId="skill-changes-loading-description"
      contentProps={{ "data-update-preview": isUpdate }}
    >
      <div className="skillChangeDialogBody">
        <Dialog.Title className="confirmDialogTitle">{skillChangeTitle(command)}</Dialog.Title>
        {isDelete ? (
          <>
            <p id="skill-changes-loading-description" className="confirmDialogDescription">
              {skillChangeDescription(command)}
            </p>
            <div className="skillDeleteNames" data-selectable-text>
              {names.map((name) => <span key={name}>{name}</span>)}
            </div>
          </>
        ) : (
          <>
            <p id="skill-changes-loading-description" className="dialogVisuallyHidden">
              {skillChangeLoadingCopy.description}
            </p>
            <LoadingState className="skillUpdatePreviewLoading" label={skillChangeLoadingCopy.previewLabel} />
          </>
        )}
      </div>
      {isDelete ? (
        <div className="confirmDialogActions">
          <DialogActionButton variant="secondary" onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
          <DialogStatefulButton state="idle" variant="danger" aria-label={actionLabel} onClick={onConfirm}>
            {actionLabel}
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
  skills?: RawSkillRecord[];
};
type SkillRefreshResult = { skills: RawSkillRecord[]; updateCheck?: string; updates?: SkillUpdateReport[] };

type SkillUpdateCheckEvent = {
  status: "completed" | "failed";
  skills: RawSkillRecord[] | null;
  updates: SkillUpdateReport[];
  error: string | null;
};

type SessionScanEvent = {
  generation: number;
  phase: "recent" | "backfill" | "watch" | "error";
  upserts: Array<Record<string, unknown>>;
  deleted: SessionIdentityRecord[];
  scanned: number;
  complete: boolean;
  error: string | null;
};

type SessionScanStartResult = {
  generation: number;
  started: boolean;
};

type PendingSkillChange = {
  command: SkillChangeCommand;
  args: Record<string, unknown>;
  names: string[];
  preview?: SkillPreview | null;
  previewError?: string;
  applyError?: string;
  onApplied?: () => void;
};

type DomainRow = Record<string, unknown>;

const SESSION_LOAD_ERROR = "Could not load sessions. Try again.";
const SESSION_REFRESH_ERROR = "Could not refresh sessions. Try again.";

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
      .map((agent) => agentIdentityKey(agent.name)),
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

function hookDeleteArgs(hook: HookRecord) {
  return {
    agent: hook.agent,
    path: hookSourcePath(hook),
    expectedTrustHash: hookTrustHash(hook),
    event: hook.event,
    matcher: hook.matcher ?? null,
    hookType: hook.hook_type ?? null,
    command: hook.command ?? null,
    url: hook.url ?? null,
    prompt: hook.prompt ?? null,
    filter: hook.filter ?? null,
    statusMessage: hook.status_message ?? null,
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

function isDetailView(value: ViewId): value is "skillDetail" {
  return value === "skillDetail";
}

export function App() {
  const storeState = useDesktopStore((state) => state);
  const { catalogs, skillUpdates, sessions, analytics } = storeState;
  const data = catalogs.data;
  const agentTargets = catalogs.agentTargets;
  const loadingDomains = catalogs.loadingDomains;
  const domainErrors = catalogs.errors;
  const domainRetryRevision = catalogs.retryRevision;
  const checkingSkillUpdates = skillUpdates.checking;
  const skillUpdateError = skillUpdates.error;
  const skillIndexStatus = skillUpdates.indexStatus;
  const analyticsRevision = analytics.revision;
  const analyticsRevisionReady = analytics.ready;
  const analyticsRevisionError = analytics.error;
  const sessionListStatus = selectSessionListStatus(storeState);
  const sessionListError = sessionListStatus === "error" ? SESSION_LOAD_ERROR : "";
  const sessionRefreshError = sessions.refreshError;
  const [view, setView] = useState<ViewId>("overview");
  const deferredView = useDeferredValue(view);
  const contentView = isDetailView(view)
    ? view
    : isDetailView(deferredView) ? view : deferredView;
  const [activeSkillName, setActiveSkillName] = useState<string | null>(null);
  const activeSkill = useMemo(() => {
    if (!activeSkillName) return null;
    return data.skills.find((item) => item.name === activeSkillName) ?? null;
  }, [activeSkillName, data.skills]);
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
  const [terminal, setTerminal] = useState("auto");
  const [editor, setEditor] = useState("vscode");
  const [additionalSessionRoots, setAdditionalSessionRoots] = useState<string[]>([]);
  const [developerMode, setDeveloperMode] = useState(false);
  const [projects, setProjects] = useState<ProjectSummary[]>([]);
  const [sessionProjects, setSessionProjects] = useState<SessionProjectSummary[]>([]);
  const [configProfiles, setConfigProfiles] = useState<Record<string, string>>({});
  const [missingSessionProjectPolicy, setMissingSessionProjectPolicy] = useState<MissingSessionProjectPolicy>("show");
  const [desktopUpdate, setDesktopUpdate] = useState<DesktopUpdateState>({ status: "idle" });
  const [updateNoticeDismissed, setUpdateNoticeDismissed] = useState(false);
  const [sessionResumeTarget, setSessionResumeTarget] = useState<SessionResumeTarget>("auto");
  const [settingsLoading, setSettingsLoading] = useState(true);
  const [settingsLoadError, setSettingsLoadError] = useState("");
  const changeAppIcon = useCallback((nextAppIcon: AppIcon) => {
    setAppIcon(nextAppIcon);
    void applyAppIcon(nextAppIcon);
  }, []);
  const appearanceChangeRevision = useRef(0);
  const domainLoadInFlight = useRef(new Map<DomainKey, Promise<void>>());
  const promptsRefreshInFlight = useRef<Promise<unknown[]> | null>(null);
  const sessionsRefreshInFlight = useRef<Promise<unknown> | null>(null);
  const skillIndexRunInFlight = useRef<Promise<SkillIndexStatus> | null>(null);
  const skillIndexStatusRefreshInFlight = useRef<Promise<SkillIndexStatus | null> | null>(null);
  const sessionResumeTargetRequests = useRef(new Map<string, Promise<"terminal" | "app">>());
  const sessionEventReady = useRef<Promise<void>>(Promise.resolve());
  const sessionEventFlushTimer = useRef<number | undefined>(undefined);
  const sessionScanGeneration = useRef(0);
  const completedSessionScans = useRef(new Set<number>());
  const sessionScanWaiters = useRef(new Map<number, Array<() => void>>());
  const pendingRecentSessions = useRef(new Map<string, SessionRecord>());
  const pendingWatchSessions = useRef(new Map<string, SessionRecord>());
  const pendingDeletedSessions = useRef(new Map<string, SessionIdentityRecord>());
  const updateOperationInFlight = useRef(false);
  const settingsLoadRequest = useRef(0);
  const sidebarPanelRef = usePanelRef();
  const skillListRevision = useRef(0);
  const skillListInFlight = useRef<Promise<RawSkillRecord[] | null> | null>(null);
  const forcedSkillListInFlight = useRef<Promise<RawSkillRecord[] | null> | null>(null);
  const skillRefreshInFlight = useRef<Promise<SkillRefreshResult | null> | null>(null);
  const skillUpdateCheckRevision = useRef(0);
  const skillUpdateCheckInFlight = useRef<Promise<void> | null>(null);
  const skillUpdateCheckActive = useRef(false);
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
      data.sources,
      data.agents,
    ),
    [agentTargets, data.agents, data.sources],
  );
  const installedAgentKeys = useMemo(() => {
    const keys = new Set<string>();
    for (const agent of data.agents) {
      if (agent.installed === true) keys.add(agentIdentityKey(agent.name));
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

  const overviewCounts = useMemo(() => selectCatalogCounts(filteredData), [filteredData]);
  const overviewCountsLoaded = useMemo(
    () => selectCatalogCountLoadedDomains(storeState),
    [catalogs.data, catalogs.loadedDomains],
  );
  const overviewCountErrors = useMemo(
    () => selectCatalogCountErrors(storeState),
    [catalogs.errors],
  );
  const overviewHookReviewCount = selectHookReviewCount(filteredData);
  const overviewSkillUpdateCount = selectSkillUpdateCount(data, agentFilter);

  const setData = desktopStore.actions.updateData;
  const setSkillUpdateError = desktopStore.actions.setSkillUpdateError;
  const setCheckingSkillUpdates = desktopStore.actions.setSkillUpdatesChecking;
  const setSkillIndexStatus = desktopStore.actions.setSkillIndexStatus;
  const setAgentTargets = desktopStore.actions.setAgentTargets;
  const setAnalyticsRevision = desktopStore.actions.setAnalyticsRevision;
  const setAnalyticsRevisionReady = desktopStore.actions.setAnalyticsReady;
  const setAnalyticsRevisionError = desktopStore.actions.setAnalyticsError;
  const setSessionRefreshError = useCallback((message: string) => {
    desktopStore.actions.setSessionRefreshError(message);
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

  const loadSettings = useCallback(async () => {
    const requestId = ++settingsLoadRequest.current;
    const appearanceRevision = appearanceChangeRevision.current;
    setSettingsLoading(true);
    setSettingsLoadError("");
    try {
      const settings = await invokeCommand<SettingsPayload>(TauriCommand.SettingsGet);
      if (requestId !== settingsLoadRequest.current) return;
      if (appearanceRevision === appearanceChangeRevision.current) {
        setAppearance(normalizeAppearance(settings.appearance));
      }
      setThemePreferences(normalizeThemePreferences({ light: settings.lightTheme as ColorTheme, dark: settings.darkTheme as ColorTheme }));
      const normalizedAppIcon = normalizeAppIcon(settings.appIcon);
      setAppIcon(normalizedAppIcon);
      void applyAppIcon(normalizedAppIcon);
      setFontFamily(normalizeFontFamily(settings.fontFamily));
      setTerminal(typeof settings.terminal === "string" && settings.terminal.trim() ? settings.terminal : "auto");
      setEditor(typeof settings.editor === "string" && settings.editor.trim() ? settings.editor.trim() : "vscode");
      setAdditionalSessionRoots(Array.isArray(settings.additionalSessionRoots)
        ? settings.additionalSessionRoots.filter((root): root is string => typeof root === "string")
        : []);
      setDeveloperMode(settings.developerMode === true);
      setSessionResumeTarget(normalizeSessionResumeTarget(settings.sessionResumeTarget));
      setMissingSessionProjectPolicy(normalizeMissingSessionProjectPolicy(settings.missingSessionProjectPolicy));
      setConfigProfiles(normalizeConfigProfiles(settings.configProfiles));
      setSettingsLoadError("");
    } catch (error) {
      if (requestId === settingsLoadRequest.current) setSettingsLoadError(errorMessage(error));
    } finally {
      if (requestId === settingsLoadRequest.current) setSettingsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  const refreshSkillList = useCallback((force = false): Promise<RawSkillRecord[] | null> => {
    if (!force && skillListInFlight.current) return skillListInFlight.current;
    if (force && forcedSkillListInFlight.current) return forcedSkillListInFlight.current;
    if (force && skillListInFlight.current) {
      const currentRequest = skillListInFlight.current;
      const queuedRequest = currentRequest.then(
        () => refreshSkillList(),
        () => refreshSkillList(),
      );
      forcedSkillListInFlight.current = queuedRequest;
      void queuedRequest.then(
        () => {
          if (forcedSkillListInFlight.current === queuedRequest) forcedSkillListInFlight.current = null;
        },
        () => {
          if (forcedSkillListInFlight.current === queuedRequest) forcedSkillListInFlight.current = null;
        },
      );
      return queuedRequest;
    }

    const revision = ++skillListRevision.current;
    const request = (async () => {
      try {
        const skills = await invokeCommand<RawSkillRecord[]>(TauriCommand.SkillsList);
        if (revision !== skillListRevision.current) return null;
        setData((current) => {
          const normalized = normalizeDomainRows(current, "skills", skills);
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
    })();
    skillListInFlight.current = request;
    void request.then(
      () => {
        if (skillListInFlight.current === request) skillListInFlight.current = null;
      },
      () => {
        if (skillListInFlight.current === request) skillListInFlight.current = null;
      },
    );
    return request;
  }, [setDomainError]);

  const refreshSkillListAndUpdates = useCallback((): Promise<SkillRefreshResult | null> => {
    if (skillRefreshInFlight.current) return skillRefreshInFlight.current;
    const revision = ++skillListRevision.current;
    setCheckingSkillUpdates(true);
    const request = (async () => {
      try {
        const result = await invokeCommand<SkillRefreshResult>(TauriCommand.SkillsRefresh);
        if (revision !== skillListRevision.current) return null;
        setData((current) => {
          const next = normalizeDomainRows(current, "skills", result.skills);
          return replaceSkillReportPreservingUpdates(current, next);
        });
        if (result.updates) desktopStore.actions.setSkillUpdateReports(result.updates);
        desktopStore.actions.markDomainLoaded("skills");
        setDomainError("skills", "");
        const updateCheckRunning = result.updateCheck === "started" || result.updateCheck === "already-running";
        skillUpdateCheckActive.current = updateCheckRunning;
        setCheckingSkillUpdates(updateCheckRunning);
        return result;
      } catch (error) {
        logger.error("skills refresh failed", { revision, error });
        if (revision === skillListRevision.current) {
          setDomainError("skills", errorMessage(error));
          setCheckingSkillUpdates(false);
        }
        skillUpdateCheckActive.current = false;
        return null;
      }
    })();
    skillRefreshInFlight.current = request;
    void request.then(
      () => {
        if (skillRefreshInFlight.current === request) skillRefreshInFlight.current = null;
      },
      () => {
        if (skillRefreshInFlight.current === request) skillRefreshInFlight.current = null;
      },
    );
    return request;
  }, [setDomainError]);

  const applySkillRows = useCallback((skills: RawSkillRecord[]) => {
    setData((current) => {
      const next = normalizeDomainRows(current, "skills", skills);
      return replaceSkillReportPreservingUpdates(current, next);
    });
  }, []);

  const applyInstalledSkills = useCallback((result: SkillInstallResult) => {
    applySkillRows(result.skills);
  }, [applySkillRows]);

  const refreshSkillUpdates = useCallback((): Promise<void> => {
    if (skillUpdateCheckInFlight.current || skillUpdateCheckActive.current) return Promise.resolve();
    const revision = ++skillUpdateCheckRevision.current;
    skillUpdateCheckActive.current = true;
    setCheckingSkillUpdates(true);
    setSkillUpdateError("");
    const request = (async () => {
      try {
        await sessionEventReady.current;
        const result = await invokeCommand<{ updateCheck: string }>(TauriCommand.SkillsUpdates, { check: true });
        if (revision !== skillUpdateCheckRevision.current) return;
        if (result.updateCheck !== "started" && result.updateCheck !== "already-running") {
          throw new Error("Invalid skill update check response");
        }
      } catch (error) {
        logger.error("skill updates check failed", { revision, error });
        skillUpdateCheckActive.current = false;
        if (revision === skillUpdateCheckRevision.current) {
          setCheckingSkillUpdates(false);
          setSkillUpdateError(errorMessage(error));
        }
      }
    })();
    skillUpdateCheckInFlight.current = request;
    void request.then(
      () => {
        if (skillUpdateCheckInFlight.current === request) skillUpdateCheckInFlight.current = null;
      },
      () => {
        if (skillUpdateCheckInFlight.current === request) skillUpdateCheckInFlight.current = null;
      },
    );
    return request;
  }, []);

  const ensureSkillUpdates = useCallback(() => {
    const { skillUpdates: current } = desktopStore.getSnapshot();
    if (current.fresh || skillUpdateCheckInFlight.current || skillUpdateCheckActive.current) return Promise.resolve();
    return refreshSkillUpdates();
  }, [refreshSkillUpdates]);

  const refreshSkills = useCallback(async () => {
    setDomainLoading("skills", true);
    setDomainError("skills", "");
    try {
      await refreshSkillListAndUpdates();
    } finally {
      setDomainLoading("skills", false);
    }
  }, [refreshSkillListAndUpdates, setDomainError]);

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
    const id = (prompt as Record<string, unknown>).id;
    if (typeof id !== "string" || !id.trim()) return;
    const normalized = normalizePrompt(prompt as Record<string, unknown>);
    if (!normalized) return;
    setData((current) => ({
      ...current,
      prompts: [normalized, ...current.prompts.filter((item) => item.id !== id)],
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

  const deleteRules = useCallback(async (paths: string[]) => {
    try {
      const rows = await invokeCommand(TauriCommand.RuleFileDeleteMany, { paths });
      if (Array.isArray(rows)) setDomainRows("rules", rows);
      return rows;
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.RuleFileDeleteMany, error });
      return { error: `${error}` };
    }
  }, []);

  const deleteHook = useCallback(async (hook: HookRecord) => {
    try {
      const rows = await invokeCommand(TauriCommand.HookDelete, hookDeleteArgs(hook));
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookDelete, error });
      return { error: `${error}` };
    }
  }, []);

  const deleteHooks = useCallback(async (hooks: HookRecord[]) => {
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

  const setHookEnabled = useCallback(async (hook: HookRecord, enabled: boolean) => {
    try {
      const rows = await invokeCommand(TauriCommand.HookSetEnabled, {
        agent: hook.agent,
        path: hookSourcePath(hook),
        expectedTrustHash: hookTrustHash(hook),
        event: hook.event,
        matcher: hook.matcher ?? null,
        hookType: hook.hook_type ?? null,
        command: hook.command ?? null,
        url: hook.url ?? null,
        prompt: hook.prompt ?? null,
        filter: hook.filter ?? null,
        statusMessage: hook.status_message ?? null,
        enabled,
      });
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookSetEnabled, error });
      return { error: `${error}` };
    }
  }, []);

  const setMcpEnabled = useCallback(async (server: McpRecord, enabled: boolean) => {
    try {
      const rows = await invokeCommand(TauriCommand.McpSetEnabled, {
        agent: server.agent,
        path: server.path,
        expectedTrustHash: server.trust_hash,
        name: server.name,
        enabled,
        serverPath: server.server_path ?? [],
      });
      if (Array.isArray(rows)) setDomainRows("mcp", rows);
      return rows;
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.McpSetEnabled, error });
      return { error: `${error}` };
    }
  }, []);

  const reviewHook = useCallback(async (hook: HookRecord) => {
    try {
      const rows = await invokeCommand(TauriCommand.HookReview, {
        agent: hook.agent,
        path: hookSourcePath(hook),
        expectedTrustHash: hookTrustHash(hook),
        event: hook.event,
        matcher: hook.matcher ?? null,
        hookType: hook.hook_type ?? null,
        command: hook.command ?? null,
        url: hook.url ?? null,
        prompt: hook.prompt ?? null,
        filter: hook.filter ?? null,
        statusMessage: hook.status_message ?? null,
      });
      if (Array.isArray(rows)) setDomainRows("hooks", rows);
      return rows;
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookReview, error });
      return { error: `${error}` };
    }
  }, []);

  const setSessionRows = useCallback((rows: unknown[], { markLoaded = true } = {}) => {
    setData((current) => {
      const nextSessions = mergeSessionRows(current.sessions, rows as Array<Record<string, unknown>>);
      if (nextSessions === current.sessions) return current;
      return recomputeSources({ ...current, sessions: nextSessions });
    });
    if (markLoaded) desktopStore.actions.markDomainLoaded("sessions");
    desktopStore.actions.setDomainError("sessions", "");
    setSessionRefreshError("");
  }, [setSessionRefreshError]);

  const setSessionLoadError = useCallback(() => {
    const hasRows = desktopStore.getSnapshot().catalogs.data.sessions.length > 0;
    desktopStore.actions.setDomainError("sessions", hasRows ? "" : SESSION_LOAD_ERROR);
    setSessionRefreshError(SESSION_REFRESH_ERROR);
  }, [setSessionRefreshError]);

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
      return sessions === current.sessions ? current : recomputeSources({ ...current, sessions });
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
    const upserts = event.upserts.flatMap((row) => {
      const session = normalizeSession(row);
      return session ? [session] : [];
    });
    if (event.phase === "recent") {
      for (const session of upserts) pendingRecentSessions.current.set(sessionLogicalIdentity(session), session);
    } else if (event.phase === "watch") {
      for (const session of upserts) pendingWatchSessions.current.set(sessionLogicalIdentity(session), session);
    }
    for (const identity of event.deleted) {
      const key = sessionIdentityRecordKey(identity);
      if (!key) continue;
      pendingDeletedSessions.current.set(key, identity);
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
        scanned: event.scanned,
        complete: event.complete,
        error: event.error ?? "unknown session scan error",
      });
      if (!event.complete) return;
      setSessionRefreshError(SESSION_REFRESH_ERROR);
      setSessionLoadError();
      finishSessionScanWaiters(event.generation);
    } else if (event.phase === "watch" && event.complete) {
      if (event.error) {
        setSessionRefreshError(SESSION_REFRESH_ERROR);
        setSessionLoadError();
      } else {
        setSessionRefreshError("");
      }
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
        skillUpdateCheckActive.current = false;
        if (payload.status === "completed") {
          if (Array.isArray(payload.skills)) {
            setData((current) => {
              const next = normalizeDomainRows(current, "skills", payload.skills);
              return replaceSkillReportPreservingUpdates(current, next);
            });
            desktopStore.actions.markDomainLoaded("skills");
            desktopStore.actions.setDomainError("skills", "");
          }
          desktopStore.actions.setSkillUpdateReports(payload.updates);
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
  }, [handleSessionScanEvent]);

  useEffect(() => {
    void refreshSkillListAndUpdates();
  }, [refreshSkillListAndUpdates]);

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

  const runSkillIndex = useCallback(() => {
    if (skillIndexRunInFlight.current) return skillIndexRunInFlight.current;
    const request = (skillRefreshInFlight.current ?? Promise.resolve())
      .then(() => invokeCommand<SkillIndexStatus>(TauriCommand.SessionSkillIndexRun, { force: false }));
    skillIndexRunInFlight.current = request;
    void request.then(
      (status) => {
        setSkillIndexStatus(status);
        if (skillIndexRunInFlight.current === request) skillIndexRunInFlight.current = null;
      },
      () => {
        if (skillIndexRunInFlight.current === request) skillIndexRunInFlight.current = null;
      },
    );
    return request;
  }, []);

  const refreshSessionsFromScan = useCallback(() => {
    setSessionRefreshError("");
    desktopStore.actions.setDomainLoading("sessions", true);
    if (!sessionsRefreshInFlight.current) {
      sessionsRefreshInFlight.current = sessionEventReady.current
        .then(() => invokeCommand<SessionScanStartResult>(TauriCommand.SessionsScanStart))
        .then(async ({ generation, started }) => {
          sessionScanGeneration.current = Math.max(sessionScanGeneration.current, generation);
          if (!completedSessionScans.current.has(generation)) {
            await new Promise<void>((resolve) => {
              const waiters = sessionScanWaiters.current.get(generation) ?? [];
              waiters.push(resolve);
              sessionScanWaiters.current.set(generation, waiters);
            });
          }
          await refreshSessionProjects();
          if (started) void runSkillIndex();
          return generation;
        })
        .catch((error) => {
          logger.error("sessions scan start failed", { error });
          setSessionRefreshError(SESSION_REFRESH_ERROR);
          setSessionLoadError();
          return null;
        })
        .finally(() => {
          desktopStore.actions.setDomainLoading("sessions", false);
          sessionsRefreshInFlight.current = null;
        });
    }
    return sessionsRefreshInFlight.current;
  }, [refreshSessionProjects, runSkillIndex, setSessionRefreshError]);

  const refreshSkillIndexStatus = useCallback(async (): Promise<SkillIndexStatus | null> => {
    const existing = skillIndexStatusRefreshInFlight.current;
    if (existing) return existing;
    const request = (async () => {
      const status = await safeInvoke<SkillIndexStatus>(TauriCommand.SessionSkillIndexStatus);
      if (status) setSkillIndexStatus(status);
      return status;
    })();
    skillIndexStatusRefreshInFlight.current = request;
    void request.then(
      () => {
        if (skillIndexStatusRefreshInFlight.current === request) skillIndexStatusRefreshInFlight.current = null;
      },
      () => {
        if (skillIndexStatusRefreshInFlight.current === request) skillIndexStatusRefreshInFlight.current = null;
      },
    );
    return request;
  }, []);

  const readSkillIndexStatus = useCallback(() => refreshSkillIndexStatus(), [refreshSkillIndexStatus]);

  const setDomainLoading = (domain: string, loading: boolean) => {
    if (!isDomainKey(domain)) return;
    desktopStore.actions.setDomainLoading(domain, loading);
  };

  const loadDomainForRetry = useCallback((domain: DomainKey) => {
    desktopStore.actions.markDomainLoaded(domain, false);
    desktopStore.actions.setDomainError(domain, "");
    if (domain === "sessions") {
      setSessionRefreshError("");
    }
    setDomainLoading(domain, true);
    desktopStore.actions.bumpDomainRetryRevision();
  }, [setSessionRefreshError]);

  const retryCatalogCounts = useCallback(() => {
    for (const domain of DOMAIN_KEYS) {
      desktopStore.actions.markDomainLoaded(domain, false);
      desktopStore.actions.setDomainError(domain, "");
      desktopStore.actions.setDomainLoading(domain, true);
    }
    setSessionRefreshError("");
    desktopStore.actions.bumpDomainRetryRevision();
  }, [setSessionRefreshError]);

  useEffect(() => {
    let cancelled = false;
    safeInvoke(TauriCommand.AgentsList).then((agents) => {
      if (!cancelled && Array.isArray(agents)) {
        setData((current) => normalizeDomainRows(current, "agents", agents));
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
      void refreshSkillList(true);
    } catch (error) {
      logger.error("bundled skill install failed", { error });
      setBundledSkillError(`${error}`);
    } finally {
      setBundledSkillBusy(false);
    }
  };

  useEffect(() => {
    const loadOneDomain = async (domain: DomainKey, force = false) => {
      if (!force && desktopStore.getSnapshot().catalogs.loadedDomains.has(domain)) return;
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
            if (view === "overview") {
              await refreshSkillListAndUpdates();
            } else {
              await refreshSkillList();
              void ensureSkillUpdates();
            }
            return;
          }
          if (domain === "sessions") {
            if (desktopStore.getSnapshot().catalogs.data.sessions.length === 0) {
              try {
                const cachedRows = await invokeCommand<unknown[]>(TauriCommand.SessionsList);
                setSessionRows(cachedRows, {
                  markLoaded: view !== "overview",
                });
              } catch (error) {
                logger.error("sessions cached list load failed", { view, error });
                setSessionLoadError();
              }
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
      let cancelled = false;
      const loadOverviewDomain = async (domain: DomainKey) => {
        await loadOneDomain(domain);
      };
      void Promise.all(DOMAIN_KEYS.map(loadOverviewDomain)).catch((error) => {
        if (!cancelled) logger.error("overview inventory load failed", { error });
      });
      return () => {
        cancelled = true;
      };
    }

    const domain = view === "skillDetail" ? "skills" : view;
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
  }, [domainRetryRevision, ensureSkillUpdates, refreshPrompts, refreshSessionsFromScan, refreshSkillList, refreshSkillListAndUpdates, setDomainError, setSessionLoadError, setSessionRows, view]);

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
    setActiveSkillName(null);
  }, [navigateTo]);

  const loadTranscript = useCallback(async (session: SessionRecord, cursor?: string, knownSourceVersion?: string): Promise<TranscriptPage> => {
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
    const page = await invokeCommand(TauriCommand.SessionTranscriptLocator, {
      path: session.path,
      agent: session.agent,
    });
    return normalizeTranscriptLocatorPage(page);
  }, []);

  const searchTranscript = useCallback(async (session: SessionRecord, query: string, scopes: TranscriptSearchScopes): Promise<TranscriptSearchResult | null> => {
    const result = await invokeCommand(TauriCommand.SessionTranscriptSearch, {
      path: session.path,
      agent: session.agent,
      query,
      scopes,
    });
    return normalizeTranscriptSearchResult(result);
  }, []);

  const searchSessions = useCallback(async (query: string, candidates: SessionRecord[]) => {
    try {
      const searchableCandidates = candidates.filter((session) => agentIdentityKey(session.agent) !== "imported");
      const rows = await invokeCommand<unknown[]>(TauriCommand.SessionsSearch, {
        query,
        candidates: searchableCandidates.map((session) => ({
          id: session.id,
          agent: agentIdentityKey(session.agent),
          path: session.path,
        })),
      });
      if (!Array.isArray(rows)) throw new Error("Invalid session search response");
      return rows.flatMap((row) => {
        const session = normalizeSession(row as Record<string, unknown>);
        return session ? [session] : [];
      });
    } catch (error) {
      logger.error("session search failed", { query, candidateCount: candidates.length, error });
      throw error;
    }
  }, []);

  const loadSessionSkillLinks = useCallback(async (session: SessionRecord) => {
    await readSkillIndexStatus();
    const links = await invokeCommand<unknown[]>(TauriCommand.SessionSkillLinks, { sessionId: session.id, agent: session.agent });
    if (!Array.isArray(links)) throw new Error("Invalid session skill links response");
    return links.flatMap((link) => {
      const normalized = normalizeSessionSkillLink(link);
      return normalized ? [normalized] : [];
    });
  }, [readSkillIndexStatus]);

  const openSkillByName = useCallback((skillName: string) => {
    const skill = data.skills.find((item) => item.name === skillName);
    if (skill) setActiveSkillName(skill.name);
    navigateTo(skill ? "skillDetail" : "skills");
  }, [data.skills, navigateTo]);

  const openSessionFromLink = useCallback((link: DomainRow) => {
    const sessionId = typeof link.session_id === "string" ? link.session_id.trim() : "";
    const agent = typeof link.agent === "string" ? link.agent.trim() : "";
    if (!sessionId || !agent) return;
    setActiveSessionKey(`${agent}:${sessionId}`);
    navigateTo("sessions");
  }, [navigateTo]);

  const resolveSessionResumeTarget = useCallback(async (session: SessionRecord): Promise<"terminal" | "app"> => {
    const configuredTarget = sessionResumeTargetForAgent(sessionResumeTarget, session.agent);
    if (configuredTarget !== "auto") return configuredTarget;
    const requestKey = sessionIdentity(session);
    const existingRequest = sessionResumeTargetRequests.current.get(requestKey);
    if (existingRequest) return existingRequest;
    const request = (async () => {
      try {
        const inferredTarget = await invokeCommand<"terminal" | "app">(TauriCommand.SessionResumeTarget, {
          session: sessionLaunchPayload(session),
        });
        return inferredTarget === "app" ? "app" : "terminal";
      } catch (error) {
        logger.warn("session resume target inference failed; using terminal", { error });
        return "terminal";
      }
    })();
    sessionResumeTargetRequests.current.set(requestKey, request);
    return request;
  }, [sessionResumeTarget]);

  const resumeSession = useCallback(async (session: SessionRecord): Promise<SessionResumeOutcome> => {
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
      | { status: "activeWriter"; lockPath: string }
      | { status: "launched"; agent: string; terminal: string; commandLine: string }
    >(TauriCommand.SessionResumeInTerminal, {
      session: sessionLaunchPayload(session),
    });
    if (result.status === "activeWriter") return result;
    return { status: "launched", target: "terminal", terminal: result.terminal };
  }, [resolveSessionResumeTarget]);

  const previewAndApply = useCallback(async (
    command: SkillChangeCommand,
    args: Record<string, unknown>,
    { onApplied }: { onApplied?: () => void } = {},
  ) => {
    const isUpdate = command === SkillChangeCommand.UpdateMany;
    const isDelete = command === SkillChangeCommand.DeleteMany;
    const names = Array.isArray(args.names)
      && args.names.length > 0
      && args.names.every((name): name is string => typeof name === "string" && name.trim().length > 0)
      ? args.names
      : undefined;
    if ((isUpdate || isDelete) && !names) {
      const error = "Skill change requires at least one valid skill name.";
      setSkillUpdateError(error);
      return;
    }
    if (isDelete) {
      setPendingSkillChange({ command, args, names: names!, preview: null, onApplied });
      return;
    }
    if (isUpdate) {
      setSkillUpdateError("");
      setPendingSkillChange({ command, args, names: names!, preview: null, onApplied });
    }
    try {
      const preview = isUpdate
        ? await invokeCommand<SkillPreview>(command, { ...args, dryRun: true })
        : await safeInvoke(command, { ...args, dryRun: true }) as SkillPreview | null;
      if (!preview) return;
      if (isUpdate) {
        setPendingSkillChange((current) => current?.command === command ? { ...current, preview } : current);
      } else {
        setPendingSkillChange({ command, args, names: names!, preview, onApplied });
      }
    } catch (error) {
      logger.error("skill change preview failed", { command, error });
      const message = `${error}`;
      setSkillUpdateError(message);
      if (isUpdate) setPendingSkillChange((current) => current?.command === command ? { ...current, previewError: message } : current);
      if (isDelete) setPendingSkillChange((current) => current?.command === command && current.args === args ? { ...current, previewError: message } : current);
    }
  }, []);

  const applySkillChange = useCallback(async (command: SkillChangeCommand, args: Record<string, unknown>) => {
    const result = await invokeCommand<SkillPreview>(command, { ...args, dryRun: false });
    if (result.skills) {
      setData((current) => {
        const next = normalizeDomainRows(current, "skills", result.skills);
        return replaceSkillReportPreservingUpdates(current, next, typeof args.name === "string" ? [args.name] : []);
      });
    }
    return result;
  }, []);

  const closeSkillChangeDialog = (open: boolean) => {
    if (!open && !applyingSkillChange) setPendingSkillChange(null);
  };

  const confirmSkillChange = async (resolutions: Record<string, string> = {}) => {
    if (!pendingSkillChange || applyingSkillChange) return;
    setApplyingSkillChange(true);
    const { command, args, names, onApplied } = pendingSkillChange;
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
          const next = normalizeDomainRows(current, "skills", result.skills);
          return replaceSkillReportPreservingUpdates(current, next, names);
        });
      } else if (command === SkillChangeCommand.DeleteMany) {
        const namesSet = new Set(names);
        setData((current) => recomputeSources({
          ...current,
          skills: current.skills.filter((skill) => !namesSet.has(skill.name)),
        }));
      } else if (command === SkillChangeCommand.UpdateMany) {
        setData((current) => clearSkillUpdateAvailability(current, names));
      }
      setPendingSkillChange(null);
      onApplied?.();
      if (!result.skills && (command !== SkillChangeCommand.DeleteMany || result?.refreshRequired)) {
        void refreshSkillList(true);
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

  const applyVisibility = useCallback(async (names: string[], visibility: SkillVisibility) => {
    setData((current) => applyVisibilityState(current, names, visibility));
    const result = await safeInvoke<SkillPreview>(SkillChangeCommand.Set, { names, visibility, dryRun: false });
    if (result?.skills) {
      setData((current) => {
        const next = normalizeDomainRows(current, "skills", result.skills);
        return replaceSkillReportPreservingUpdates(current, next);
      });
    }
    if (!result) await refreshSkillList(true);
  }, [refreshSkillList]);

  const applyWrapperSkill = useCallback(
    (args: WrapperArgs) => applySkillChange(SkillChangeCommand.Wrap, args),
    [applySkillChange],
  );
  const applySkillUpdates = useCallback(
    (names: string[], onApplied?: () => void) => previewAndApply(SkillChangeCommand.UpdateMany, { names }, { onApplied }),
    [previewAndApply],
  );
  const deleteSkills = useCallback(
    (names: string[], onApplied?: () => void) => previewAndApply(SkillChangeCommand.DeleteMany, { names }, { onApplied }),
    [previewAndApply],
  );
  const openSkillFromList = useCallback((skill: NormalizedSkill) => {
    setActiveSkillName(skill.name);
    navigateTo("skillDetail");
  }, [navigateTo]);

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
            title={dialogCopy.bundledSkillSetupTitle}
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
            names={pendingSkillChange.names}
            onOpenChange={closeSkillChangeDialog}
            onConfirm={() => { void confirmSkillChange(); }}
          />
        )}>
          <ConfirmSkillChangesDialog
            open
            command={pendingSkillChange.command}
            names={pendingSkillChange.names}
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
            view={view === "skillDetail" ? "skills" : view}
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
              onReadSkillIndexStatus={readSkillIndexStatus}
              skillIndexStatus={skillIndexStatus}
              onOpenSession={openSessionFromLink}
              onOpenSkill={openSkillByName}
              onSaved={(skills) => {
                if (skills) {
                  setData((current) => {
                    const next = normalizeDomainRows(current, "skills", skills);
                    return replaceSkillReportPreservingUpdates(current, next);
                  });
                }
              }}
            />
          ) : (
            <PlaceholderView title="Skill unavailable" />
          )) : contentView === "skills" ? (
            <SkillsView
              skills={filteredData.skills}
              projects={projects}
              installedAgentKeys={installedAgentKeys}
              targetOptions={agentTargets}
              loadingSkills={loadingDomains.has("skills")}
              loadError={domainErrors.skills ?? ""}
              hasRows={data.skills.length > 0}
              checkingUpdates={checkingSkillUpdates}
              updateError={skillUpdateError}
              onRefresh={refreshSkills}
              onSkillsUpdated={applySkillRows}
              onSetVisibility={applyVisibility}
              onApplyWrapper={applyWrapperSkill}
              onApplyUpdates={applySkillUpdates}
              onDeleteSkills={deleteSkills}
              onAddInstalled={applyInstalledSkills}
              openSkill={openSkillFromList}
            />
          ) : contentView === "sessions" ? (
            <SessionsView
              sessions={filteredData.sessions as SessionRecord[]}
              developerMode={developerMode}
              loadTranscript={loadTranscript}
              loadTranscriptLocator={loadTranscriptLocator}
              searchTranscript={searchTranscript}
              searchSessions={searchSessions}
              loadSessionSkillLinks={loadSessionSkillLinks}
              skillIndexStatus={skillIndexStatus}
              loadingSessions={loadingDomains.has("sessions")}
              sessionListError={sessionListError}
              sessionRefreshError={sessionRefreshError}
              onRefreshSessions={refreshSessionsFromScan}
              onResumeSession={resumeSession}
              resolveSessionResumeTarget={resolveSessionResumeTarget}
              sessionResumeTarget={sessionResumeTarget}
              missingSessionProjectPolicy={missingSessionProjectPolicy}
              projects={projects}
              sessionProjects={sessionProjects}
              onOpenSkill={openSkillByName}
              activeSessionKey={activeSessionKey}
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
              <RulesView rows={filteredData.rules} skills={data.skills} projects={projects} loadingRows={loadingDomains.has("rules")} loadError={domainErrors.rules ?? ""} hasRows={data.rules.length > 0} onRetry={() => { void loadDomainForRetry("rules"); }} onOpenSkill={openSkillByName} onDeleteRules={deleteRules} />
            ) : contentView === "hooks" ? (
              <HooksView rows={filteredData.hooks} projects={projects} loadingRows={loadingDomains.has("hooks")} loadError={domainErrors.hooks ?? ""} hasRows={data.hooks.length > 0} onRetry={() => { void loadDomainForRetry("hooks"); }} onDeleteHook={deleteHook} onDeleteHooks={deleteHooks} onSetHookEnabled={setHookEnabled} onReviewHook={reviewHook} />
            ) : contentView === "mcp" ? (
            <DataListView title="MCP" rows={filteredData.mcp} columns={mcpColumns} projects={projects} loading={loadingDomains.has("mcp")} loadError={domainErrors.mcp ?? ""} hasRows={data.mcp.length > 0} onRetry={() => { void loadDomainForRetry("mcp"); }} onSetMcpEnabled={setMcpEnabled} />
          ) : contentView === "config" ? (
            <ConfigView activeProfiles={configProfiles} onActiveProfilesChange={setConfigProfiles} />
          ) : contentView === "settings" ? (
            <SettingsView
              appearance={appearance}
              themePreferences={themePreferences}
              fontFamily={fontFamily}
              terminal={terminal}
              editor={editor}
              additionalSessionRoots={additionalSessionRoots}
              appIcon={appIcon}
              configProfiles={configProfiles}
              developerMode={developerMode}
              sessionResumeTarget={sessionResumeTarget}
              missingSessionProjectPolicy={missingSessionProjectPolicy}
              onAppearanceChange={changeAppearance}
              onFontFamilyChange={setFontFamily}
              onTerminalChange={setTerminal}
              onEditorChange={setEditor}
              onAdditionalSessionRootsChange={setAdditionalSessionRoots}
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
              appSettingsLoading={settingsLoading}
              appSettingsLoadError={settingsLoadError}
              onRetryAppSettings={() => { void loadSettings(); }}
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
              skills={filteredData.skills}
              sessions={filteredData.sessions as SessionRecord[]}
              skillUpdateCount={overviewSkillUpdateCount}
              analyticsRevision={analyticsRevision}
              analyticsRevisionReady={analyticsRevisionReady}
              analyticsRevisionError={analyticsRevisionError}
              onRetryAnalyticsRevision={refreshAnalyticsRevision}
              agentFilter={agentFilter}
              overviewCountsLoaded={overviewCountsLoaded}
              overviewCountErrors={overviewCountErrors}
              onRetryCounts={retryCatalogCounts}
              sessionListStatus={sessionListStatus}
              sessionListError={sessionListError}
              onNavigate={(id: DomainKey) => navigate(id)}
              onOpenSession={(session) => {
                setActiveSessionKey(`${session.agent}:${session.id}`);
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
