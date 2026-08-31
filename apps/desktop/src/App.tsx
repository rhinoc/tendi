import { lazy, memo, Suspense, useCallback, useDeferredValue, useEffect, useLayoutEffect, useMemo, useRef, useState, type ComponentProps } from "react";
import { listen } from "@tauri-apps/api/event";
import { Group as PanelGroup, Panel, usePanelRef } from "react-resizable-panels";
import { Dialog } from "radix-ui";

import { Appearance, applyAppearance, applyFontFamily, listenForSystemAppearanceChange, type ColorTheme, type ThemePreferences } from "./lib/appearance.ts";
import { applyAppIcon, type AppIcon } from "./lib/app-icon.ts";
import { AppPage, assertRuntimeEventPayload, AsyncStatus, COLLAPSED_SIDEBAR_SIZE, DOMAIN_KEYS, DesktopUpdateStatus, RuntimeDomainKey, SessionResumeOutcomeStatus, SessionResumeTarget, SIDEBAR_SIZE, SkillChangeCommand, SkillVisibility, TauriCommand, UPDATE_AVAILABLE_EVENT, UpdateCheckStatus, agentIdentityKey, findSkillBySelector, dialogCopy, hookSourcePath, hookTrustHash, isConcreteAgent, isTauriRuntime, logger, navItems, normalizeSessionSkillLink, promptTitleFromBody, sessionAppDeepLink, sessionIdentity, sessionLaunchPayload, sessionSourceExternalKey, sessionResumeTargetForAgent, skillChangeActionLabel, skillChangeDescription, skillChangeLoadingCopy, skillChangeTitle } from "./lib/index.ts";
import type { BundledSkillStatus, DesktopUpdateState, DomainKey, HookRecord, McpRecord, NormalizedSkill, ProjectSummary, SessionRecord, SessionResumeOutcome, UpdateCheckResult } from "./lib/index.ts";
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
import type { RawDomainRow, RawSkillRecord, SkillInstallResult, WrapperArgs } from "./lib/index.ts";
import { ConfigView } from "./views/ConfigView.tsx";
import { DataListView } from "./views/McpView.tsx";
import { HooksView } from "./views/HooksView.tsx";
import { PromptsView } from "./views/PromptsView.tsx";
import { RulesView } from "./views/RulesView.tsx";
import { SessionsView } from "./views/SessionsView.tsx";
import { SkillsView } from "./views/SkillsView.tsx";
import { SettingsView } from "./features/settings/SettingsView.tsx";
import { UpdateNotesDialog } from "./features/settings/UpdateNotesDialog.tsx";
import { SkillEditorView } from "./features/skills/SkillEditorView.tsx";
import { OverviewView } from "./views/OverviewView.tsx";
import { desktopStore, selectSessionListStatus, SessionListStatus, useDesktopStore, type AgentTargetOption, type SkillIndexStatus } from "./store/desktop-store.ts";
import { selectCatalogView } from "./controllers/catalog-controller.ts";
import { selectOverviewCounts, selectOverviewHookReviewCount, selectOverviewSkillUpdateCount } from "./controllers/overview-controller.ts";
import { normalizeSessionRows, sessionSearchCandidateRows } from "./controllers/session-controller.ts";
import {
  applySkillChangeIfAvailable,
  checkForUpdates as checkForUpdatesCommand,
  deleteHook as deleteHookCommand,
  deleteHooks as deleteHooksCommand,
  deleteRules as deleteRulesCommand,
  dismissBundledSkillPrompt as dismissBundledSkillPromptCommand,
  invokeAgentsList,
  invokeAnalyticsRevision,
  invokeDomainList,
  invokeProjectList,
  invokeSessionSkillLinks,
  invokeSessionProjectList,
  inferSessionResumeTarget,
  installBundledSkill as installBundledSkillCommand,
  installCli,
  installUpdate as installUpdateCommand,
  loadSessionTranscript,
  loadSessionTranscriptLocator,
  openUrl,
  readBundledSkillStatus,
  readSkillTargets,
  readSettings,
  readSkillIndexStatus as readSkillIndexStatusCommand,
  runSkillIndex as runSkillIndexCommand,
  searchSessionTranscript,
  syncSkillBackup,
  invokeSessionSearch,
  previewSkillChange,
  previewSkillChangeIfAvailable,
  requestSkillUpdates,
  resumeSessionInTerminal,
  SkillUpdateCheckState,
  reviewHook as reviewHookCommand,
  savePrompt,
  setHookEnabled as setHookEnabledCommand,
  setHooksEnabled as setHooksEnabledCommand,
  setMcpEnabled as setMcpEnabledCommand,
  setMcpEnabledMany as setMcpEnabledManyCommand,
} from "./lib/runtime-gateway.ts";
import { applySkillChangeAndCommit, commitHookCommandResult, commitMcpCommandResult, commitRuleCommandResult, commitSkillChangeResult, commitSkillRows, createSkillCatalogRuntime } from "./lib/runtime-workflows.ts";
import type { SkillChangeArgs, SkillChangeResponse } from "./lib/runtime-gateway.ts";
import { useSessionRuntimeController } from "./controllers/session-runtime-controller.ts";

const MemoSidebar = memo(Sidebar) as typeof Sidebar;
const MemoSkillEditorView = memo(SkillEditorView);
const MemoSkillsView = memo(SkillsView);
const MemoSessionsView = memo(SessionsView);
const MemoPromptsView = memo(PromptsView);
const MemoRulesView = memo(RulesView);
const MemoHooksView = memo(HooksView);
const MemoDataListView = memo(DataListView);
const MemoConfigView = memo(ConfigView);
const MemoSettingsView = memo(SettingsView);

const searchSessionRecords = async (query: string, candidates: readonly SessionRecord[]): Promise<SessionRecord[]> => {
  const rows = await invokeSessionSearch(query, sessionSearchCandidateRows(candidates));
  return normalizeSessionRows(rows);
};

type ViewId = AppPage;

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
            <p id="skill-changes-loading-description" className="confirmDialogDescription">
              {skillChangeDescription(command)}
            </p>
            <LoadingState className="skillUpdatePreviewLoading" label={skillChangeLoadingCopy.previewLabel} />
          </>
        )}
      </div>
      <div className="confirmDialogActions">
        <DialogActionButton variant="secondary" onClick={() => onOpenChange(false)}>Cancel</DialogActionButton>
        <DialogStatefulButton
          state={AsyncStatus.Idle}
          variant={isDelete ? "danger" : "primary"}
          aria-label={actionLabel}
          onClick={onConfirm}
          disabled={!isDelete}
        >
          {actionLabel}
        </DialogStatefulButton>
      </div>
    </DialogShell>
  );
}

const AUTO_UPDATE_CHECK_INTERVAL_MS = 24 * 60 * 60 * 1000;
const AUTO_UPDATE_LAST_CHECK_KEY = "tendi-update-last-check-at";

type SkillPreview = SkillChangeResponse;

type PendingSkillChange = {
  command: SkillChangeCommand;
  args: SkillChangeArgs;
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

function hookSetEnabledArgs(hook: HookRecord, enabled: boolean) {
  return {
    ...hookDeleteArgs(hook),
    enabled,
  };
}

function mcpSetEnabledArgs(server: McpRecord, enabled: boolean) {
  return {
    agent: server.agent,
    path: server.path,
    expectedTrustHash: server.trust_hash,
    name: server.name,
    enabled,
    serverPath: server.server_path ?? [],
  };
}

function isDomainKey(value: string): value is DomainKey {
  return (DOMAIN_KEYS as readonly string[]).includes(value);
}

function domainForView(value: ViewId): DomainKey | null {
  switch (value) {
    case AppPage.Skills:
      return RuntimeDomainKey.Skills;
    case AppPage.Prompts:
      return RuntimeDomainKey.Prompts;
    case AppPage.Sessions:
      return RuntimeDomainKey.Sessions;
    case AppPage.Rules:
      return RuntimeDomainKey.Rules;
    case AppPage.Hooks:
      return RuntimeDomainKey.Hooks;
    case AppPage.Mcp:
      return RuntimeDomainKey.Mcp;
    case AppPage.SkillDetail:
      return RuntimeDomainKey.Skills;
    default:
      return null;
  }
}

function viewForDomain(domain: DomainKey): ViewId {
  switch (domain) {
    case RuntimeDomainKey.Skills:
      return AppPage.Skills;
    case RuntimeDomainKey.Prompts:
      return AppPage.Prompts;
    case RuntimeDomainKey.Sessions:
      return AppPage.Sessions;
    case RuntimeDomainKey.Rules:
      return AppPage.Rules;
    case RuntimeDomainKey.Hooks:
      return AppPage.Hooks;
    case RuntimeDomainKey.Mcp:
      return AppPage.Mcp;
  }
}

function isDetailView(value: ViewId): value is typeof AppPage.SkillDetail {
  return value === AppPage.SkillDetail;
}

export function App() {
  const catalogs = useDesktopStore((state) => state.catalogs);
  const skillUpdates = useDesktopStore((state) => state.skillUpdates);
  const sessionRefreshError = useDesktopStore((state) => state.sessions.refreshError);
  const workspace = useDesktopStore((state) => state.workspace);
  const appSettings = useDesktopStore((state) => state.settings);
  const data = catalogs.data;
  const projects = workspace.projects;
  const sessionProjects = workspace.sessionProjects;
  const {
    appearance,
    themePreferences,
    fontFamily,
    appIcon,
    terminal,
    editor,
    additionalSessionRoots,
    developerMode,
    configProfiles,
    missingSessionProjectPolicy,
    sessionResumeTarget,
  } = appSettings.values;
  const agentTargets = catalogs.agentTargets;
  const loadingDomains = catalogs.loadingDomains;
  const domainErrors = catalogs.errors;
  const domainRetryRevision = catalogs.retryRevision;
  const checkingSkillUpdates = skillUpdates.checking;
  const skillUpdateError = skillUpdates.error;
  const skillIndexStatus = skillUpdates.indexStatus;
  const sessionListStatus = useDesktopStore(selectSessionListStatus);
  const sessionListError = sessionListStatus === SessionListStatus.Error ? SESSION_LOAD_ERROR : "";
  const [view, setView] = useState<ViewId>(AppPage.Overview);
  const deferredView = useDeferredValue(view);
  const contentView = isDetailView(view)
    ? view
    : isDetailView(deferredView) ? view : deferredView;
  const [activeSkillId, setActiveSkillId] = useState<string | null>(null);
  const activeSkill = useMemo(() => {
    if (!activeSkillId) return null;
    return findSkillBySelector(data.skills, activeSkillId) ?? null;
  }, [activeSkillId, data.skills]);
  const [activeSessionKey, setActiveSessionKey] = useState("");
  const [sidebarCollapsed, setSidebarCollapsed] = useState(false);
  const [agentFilter, setAgentFilter] = useState("All");
  const [pendingSkillChange, setPendingSkillChange] = useState<PendingSkillChange | null>(null);
  const [bundledSkillPrompt, setBundledSkillPrompt] = useState<BundledSkillStatus | null>(null);
  const [bundledSkillBusy, setBundledSkillBusy] = useState(false);
  const [bundledSkillError, setBundledSkillError] = useState("");
  const [applyingSkillChange, setApplyingSkillChange] = useState(false);
  const [desktopUpdate, setDesktopUpdate] = useState<DesktopUpdateState>({ status: DesktopUpdateStatus.Idle });
  const [updateNoticeDismissed, setUpdateNoticeDismissed] = useState(false);
  const [updateNotesOpen, setUpdateNotesOpen] = useState(false);
  const changeAppIcon = useCallback((nextAppIcon: AppIcon) => {
    desktopStore.actions.patchSettings({ appIcon: nextAppIcon });
    void applyAppIcon(nextAppIcon);
  }, []);
  const appearanceChangeRevision = useRef(0);
  const domainLoadInFlight = useRef(new Map<DomainKey, Promise<void>>());
  const promptsRefreshInFlight = useRef<Promise<RawDomainRow[]> | null>(null);
  const skillIndexRunInFlight = useRef<Promise<SkillIndexStatus> | null>(null);
  const skillIndexStatusRefreshInFlight = useRef<Promise<SkillIndexStatus | null> | null>(null);
  const sessionResumeTargetRequests = useRef(new Map<string, Promise<Exclude<SessionResumeTarget, SessionResumeTarget.Auto>>>());
  const updateOperationInFlight = useRef(false);
  const settingsLoadRequest = useRef(0);
  const sidebarPanelRef = usePanelRef();
  const skillUpdateCheckRevision = useRef(0);
  const skillUpdateCheckInFlight = useRef<Promise<void> | null>(null);
  const skillUpdateCheckActive = useRef(false);
  const refreshSessionProjects = useCallback(async () => {
    const result = await invokeSessionProjectList();
    if (result) desktopStore.actions.setSessionProjects(result);
  }, []);
  const refreshProjects = useCallback(async () => {
    const [projectsResult, sessionProjectsResult] = await Promise.all([
      invokeProjectList(),
      invokeSessionProjectList(),
    ]);
    if (projectsResult) desktopStore.actions.setProjects(projectsResult);
    if (sessionProjectsResult) desktopStore.actions.setSessionProjects(sessionProjectsResult);
  }, []);
  const applyUpdateCheckResult = useCallback((result: UpdateCheckResult, manual: boolean) => {
    if (result.status === UpdateCheckStatus.Available && result.version) {
      setDesktopUpdate({
        status: DesktopUpdateStatus.Available,
        version: result.version,
        body: result.body ?? undefined,
      });
      setUpdateNoticeDismissed(false);
      return;
    }
    if (result.status === UpdateCheckStatus.UpToDate) {
      setDesktopUpdate({ status: manual ? DesktopUpdateStatus.UpToDate : DesktopUpdateStatus.Idle });
      return;
    }
    if (result.status === UpdateCheckStatus.Busy && manual) {
      setDesktopUpdate({ status: DesktopUpdateStatus.Error, error: "Another update operation is already running." });
    }
  }, []);
  const checkForUpdates = useCallback(async (manual = false) => {
    if (!isTauriRuntime() || updateOperationInFlight.current) return;
    updateOperationInFlight.current = true;
    if (manual) setDesktopUpdate({ status: DesktopUpdateStatus.Checking });
    try {
      try {
        window.localStorage.setItem(AUTO_UPDATE_LAST_CHECK_KEY, `${Date.now()}`);
      } catch (error) {
        logger.warn("automatic desktop update check timestamp write failed", { error });
      }
      const result = await checkForUpdatesCommand();
      applyUpdateCheckResult(result, manual);
    } catch (error) {
      if (manual) {
        setDesktopUpdate({ status: DesktopUpdateStatus.Error, error: errorMessage(error) });
      } else {
        logger.warn("automatic desktop update check failed", { error });
      }
    } finally {
      updateOperationInFlight.current = false;
    }
  }, [applyUpdateCheckResult]);
  const installUpdate = useCallback(async () => {
    if (desktopUpdate.status !== DesktopUpdateStatus.Available || updateOperationInFlight.current) return;
    updateOperationInFlight.current = true;
    setDesktopUpdate((current) => ({ ...current, status: DesktopUpdateStatus.Installing }));
    try {
      const result = await installUpdateCommand();
      applyUpdateCheckResult(result, true);
    } catch (error) {
      setDesktopUpdate({ status: DesktopUpdateStatus.Error, error: errorMessage(error) });
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
      try {
        assertRuntimeEventPayload(UPDATE_AVAILABLE_EVENT, event.payload);
        if (!disposed) applyUpdateCheckResult(event.payload, false);
      } catch (error) {
        logger.warn("desktop update event payload validation failed", { error });
      }
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
      catalogs.indexes.sources,
      data.agents,
    ),
    [agentTargets, catalogs.indexes.sources, data.agents],
  );
  const installedAgentKeys = catalogs.indexes.installedAgentKeys;
  useEffect(() => {
    if (agentFilter !== "All" && !availableSidebarSources.some((source) => source.label === agentFilter)) {
      setAgentFilter("All");
    }
  }, [agentFilter, availableSidebarSources]);
  useEffect(() => {
    const syncBackup = () => { void syncSkillBackup(); };
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
  const filteredData = useMemo(
    () => selectCatalogView(data, agentFilter, catalogs.indexes.sources),
    [agentFilter, catalogs.indexes.sources, data],
  );

  const overviewCounts = useMemo(() => selectOverviewCounts(filteredData), [filteredData]);
  const overviewCountsLoaded = catalogs.indexes.loadedDomains;
  const overviewCountErrors = catalogs.indexes.errorDomains;
  const overviewHookReviewCount = selectOverviewHookReviewCount(filteredData);
  const overviewSkillUpdateCount = selectOverviewSkillUpdateCount(data, agentFilter);

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
    if (appearance !== Appearance.System) return;
    return listenForSystemAppearanceChange(() => applyAppearance(Appearance.System, themePreferences));
  }, [appearance, themePreferences]);

  useEffect(() => {
    applyFontFamily(fontFamily);
  }, [fontFamily]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "f") return;
      if (view === AppPage.Sessions) return;
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
    if (view === AppPage.Skills) void loadConfirmSkillChangesDialog();
  }, [view]);

  const changeAppearance = useCallback((nextAppearance: Appearance) => {
    appearanceChangeRevision.current += 1;
    desktopStore.actions.patchSettings({ appearance: nextAppearance });
  }, []);

  const loadSettings = useCallback(async () => {
    const requestId = ++settingsLoadRequest.current;
    const appearanceRevision = appearanceChangeRevision.current;
    desktopStore.actions.setSettingsLoading(true);
    desktopStore.actions.setSettingsError("");
    try {
      const settings = await readSettings();
      if (requestId !== settingsLoadRequest.current) return;
      if (appearanceRevision === appearanceChangeRevision.current) {
        desktopStore.actions.patchSettings({ appearance: settings.appearance });
      }
      const themePreferencesForState = {
        light: settings.lightTheme as ColorTheme,
        dark: settings.darkTheme as ColorTheme,
      } satisfies ThemePreferences;
      void applyAppIcon(settings.appIcon);
      desktopStore.actions.patchSettings({
        themePreferences: themePreferencesForState,
        appIcon: settings.appIcon,
        fontFamily: settings.fontFamily,
        terminal: settings.terminal,
        editor: settings.editor,
        additionalSessionRoots: settings.additionalSessionRoots,
        developerMode: settings.developerMode,
        sessionResumeTarget: settings.sessionResumeTarget,
        missingSessionProjectPolicy: settings.missingSessionProjectPolicy,
        configProfiles: settings.configProfiles,
      });
    } catch (error) {
      if (requestId === settingsLoadRequest.current) desktopStore.actions.setSettingsError(errorMessage(error));
    } finally {
      if (requestId === settingsLoadRequest.current) desktopStore.actions.setSettingsLoading(false);
    }
  }, []);

  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  const skillRuntime = useMemo(() => createSkillCatalogRuntime({
    store: desktopStore,
    setError: (message) => setDomainError(RuntimeDomainKey.Skills, message),
    setChecking: setCheckingSkillUpdates,
    setUpdateCheckActive: (active) => { skillUpdateCheckActive.current = active; },
    onError: (message, error) => logger.error(message, { error }),
  }), [setCheckingSkillUpdates, setDomainError]);
  const {
    refreshList: refreshSkillList,
    refreshListAndUpdates: refreshSkillListAndUpdates,
  } = skillRuntime;

  const runSkillIndex = useCallback(() => {
    if (skillIndexRunInFlight.current) return skillIndexRunInFlight.current;
    const request = skillRuntime.whenIdle()
      .then(() => runSkillIndexCommand(false));
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
  }, [skillRuntime]);

  const {
    refreshSessionsFromScan,
    resyncSessionSnapshot,
    whenEventsReady,
  } = useSessionRuntimeController({
    refreshSessionProjects,
    runSkillIndex,
    setSessionRefreshError,
    setAnalyticsRevision,
    setAnalyticsRevisionReady,
    setAnalyticsRevisionError,
    setSkillUpdateError,
    setCheckingSkillUpdates,
  });

  const applySkillRows = useCallback((skills: RawSkillRecord[], options?: { patch?: boolean; deleted?: string[] }) => {
    commitSkillRows(desktopStore, skills, options);
  }, []);

  const applyInstalledSkills = useCallback((result: SkillInstallResult) => {
    applySkillRows(result.updated ?? result.skills ?? [], { patch: true });
  }, [applySkillRows]);

  const refreshSkillUpdates = useCallback((): Promise<void> => {
    if (skillUpdateCheckInFlight.current || skillUpdateCheckActive.current) return Promise.resolve();
    const revision = ++skillUpdateCheckRevision.current;
    skillUpdateCheckActive.current = true;
    setCheckingSkillUpdates(true);
    setSkillUpdateError("");
    const request = (async () => {
      try {
        await whenEventsReady();
        const result = await requestSkillUpdates();
        if (revision !== skillUpdateCheckRevision.current) return;
        if (result.updateCheck !== SkillUpdateCheckState.Started && result.updateCheck !== SkillUpdateCheckState.AlreadyRunning) {
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
  }, [whenEventsReady]);

  const ensureSkillUpdates = useCallback(() => {
    const { skillUpdates: current } = desktopStore.getSnapshot();
    if (current.fresh || skillUpdateCheckInFlight.current || skillUpdateCheckActive.current) return Promise.resolve();
    return refreshSkillUpdates();
  }, [refreshSkillUpdates]);

  const refreshSkills = useCallback(async () => {
    setDomainLoading(RuntimeDomainKey.Skills, true);
    setDomainError(RuntimeDomainKey.Skills, "");
    try {
      await refreshSkillListAndUpdates();
    } finally {
    setDomainLoading(RuntimeDomainKey.Skills, false);
    }
  }, [refreshSkillListAndUpdates, setDomainError]);

  const refreshPrompts = useCallback(async () => {
    setDomainLoading(RuntimeDomainKey.Prompts, true);
    setDomainError(RuntimeDomainKey.Prompts, "");
    try {
      if (!promptsRefreshInFlight.current) {
        promptsRefreshInFlight.current = (async () => {
          const rows = await invokeDomainList(RuntimeDomainKey.Prompts);
          desktopStore.actions.commitDomainSnapshot(RuntimeDomainKey.Prompts, rows);
          return rows;
        })().finally(() => {
          promptsRefreshInFlight.current = null;
        });
      }
      return await promptsRefreshInFlight.current;
    } catch (error) {
      logger.error("prompts refresh failed", { error });
      setDomainError(RuntimeDomainKey.Prompts, errorMessage(error));
      return null;
    } finally {
      setDomainLoading(RuntimeDomainKey.Prompts, false);
    }
  }, []);

  const applyPromptSaved = desktopStore.actions.applyPromptRecord;

  const savePromptFromSession = useCallback(async (body: string) => {
    if (!body.trim()) return false;
    const prompt = await savePrompt({
      id: null,
      title: promptTitleFromBody(body),
      tags: [],
      body,
    });
    if (!prompt) return false;
    desktopStore.actions.applyPromptRecord(prompt, body);
    return true;
  }, []);

  const removePrompts = useCallback((ids: string[]) => {
    desktopStore.actions.removePrompts(ids);
  }, []);

  const deleteRules = useCallback(async (paths: string[]) => {
    try {
      const result = await deleteRulesCommand(paths);
      return commitRuleCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.RuleFileDeleteMany, error });
      return { error: `${error}` };
    }
  }, []);

  const patchRuleSha256 = useCallback((path: string, sha256: string) => {
    desktopStore.actions.patchRuleSha256(path, sha256);
  }, []);

  const deleteHook = useCallback(async (hook: HookRecord) => {
    try {
      const result = await deleteHookCommand(hookDeleteArgs(hook));
      return commitHookCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookDelete, error });
      return { error: `${error}` };
    }
  }, []);

  const deleteHooks = useCallback(async (hooks: HookRecord[]) => {
    try {
      const result = await deleteHooksCommand(hooks.map(hookDeleteArgs));
      return commitHookCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookDeleteMany, error });
      return { error: `${error}` };
    }
  }, []);

  const setHookEnabled = useCallback(async (hook: HookRecord, enabled: boolean) => {
    try {
      const result = await setHookEnabledCommand(hookSetEnabledArgs(hook, enabled));
      return commitHookCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookSetEnabled, error });
      return { error: `${error}` };
    }
  }, []);

  const setHooksEnabled = useCallback(async (hooks: HookRecord[], enabled: boolean) => {
    try {
      const result = await setHooksEnabledCommand(hooks.map((hook) => hookSetEnabledArgs(hook, enabled)));
      return commitHookCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookSetEnabledMany, error });
      return { error: `${error}` };
    }
  }, []);

  const setMcpEnabled = useCallback(async (server: McpRecord, enabled: boolean) => {
    try {
      const result = await setMcpEnabledCommand(mcpSetEnabledArgs(server, enabled));
      return commitMcpCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.McpSetEnabled, error });
      return { error: `${error}` };
    }
  }, []);

  const setMcpEnabledMany = useCallback(async (servers: McpRecord[], enabled: boolean) => {
    try {
      const result = await setMcpEnabledManyCommand(servers.map((server) => mcpSetEnabledArgs(server, enabled)));
      return commitMcpCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.McpSetEnabledMany, error });
      return { error: `${error}` };
    }
  }, []);

  const reviewHook = useCallback(async (hook: HookRecord) => {
    try {
      const result = await reviewHookCommand({
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
      return commitHookCommandResult(desktopStore, result);
    } catch (error) {
      logger.warn("tendi command failed", { command: TauriCommand.HookReview, error });
      return { error: `${error}` };
    }
  }, []);

  useEffect(() => {
    void refreshSkillListAndUpdates();
  }, [refreshSkillListAndUpdates]);

  const refreshAnalyticsRevision = useCallback(async () => {
    setAnalyticsRevisionError("");
    setAnalyticsRevisionReady(false);
    try {
      const revision = await invokeAnalyticsRevision();
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

  const refreshSkillIndexStatus = useCallback(async (): Promise<SkillIndexStatus | null> => {
    const existing = skillIndexStatusRefreshInFlight.current;
    if (existing) return existing;
    const request = (async () => {
      const status = await readSkillIndexStatusCommand();
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
    if (domain === RuntimeDomainKey.Sessions) {
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
    invokeAgentsList().then((agents) => {
      if (!cancelled && Array.isArray(agents)) {
        desktopStore.actions.commitDomainSnapshot(RuntimeDomainKey.Agents, agents);
      }
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    readSkillTargets().then((targets) => {
      if (!cancelled && Array.isArray(targets)) setAgentTargets(targets);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    let cancelled = false;
    readBundledSkillStatus().then((status) => {
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
      await dismissBundledSkillPromptCommand();
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
      const cliStatus = await installCli();
      if (cliStatus.state !== "installed" || !cliStatus.pathConfigured) {
        throw new Error(cliStatus.detail || "The Tendi CLI is not available on PATH.");
      }
      const report = await installBundledSkillCommand();
      setBundledSkillPrompt(null);
      if (Array.isArray(report.updated)) {
        applySkillRows(report.updated, { patch: true });
      } else {
        void refreshSkillList(true);
      }
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
        if (domain === RuntimeDomainKey.Sessions && view !== AppPage.Overview && !desktopStore.getSnapshot().catalogs.loadedDomains.has(domain)) {
          await refreshSessionsFromScan();
        }
        return;
      }
      const request = (async () => {
        setDomainLoading(domain, true);
        setDomainError(domain, "");
        try {
          if (domain === RuntimeDomainKey.Skills) {
            if (view === AppPage.Overview) {
              await refreshSkillListAndUpdates();
            } else {
              await refreshSkillList();
              void ensureSkillUpdates();
            }
            return;
          }
          if (domain === RuntimeDomainKey.Sessions) {
            if (desktopStore.getSnapshot().catalogs.data.sessions.length === 0) {
              await resyncSessionSnapshot();
            }
            await refreshSessionsFromScan();
            return;
          }
          if (domain === RuntimeDomainKey.Prompts) {
            await refreshPrompts();
            return;
          }
          const rows = await invokeDomainList(domain);
          desktopStore.actions.commitDomainSnapshot(domain, rows);
        } catch (error) {
          logger.error("domain load failed", { domain, error });
          setDomainError(domain, errorMessage(error));
          if (domain === RuntimeDomainKey.Sessions) {
            setSessionRefreshError(SESSION_REFRESH_ERROR);
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

    if (view === AppPage.Overview) {
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

    const domain = domainForView(view);
    if (!domain) return;
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
  }, [domainRetryRevision, ensureSkillUpdates, refreshPrompts, refreshSessionsFromScan, refreshSkillList, refreshSkillListAndUpdates, resyncSessionSnapshot, setDomainError, view]);

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
    setActiveSkillId(null);
  }, [navigateTo]);
  const navigateToDomain = useCallback((domain: DomainKey) => {
    navigate(viewForDomain(domain));
  }, [navigate]);
  const openOverviewSession = useCallback((session: SessionRecord) => {
    setActiveSessionKey(sessionSourceExternalKey(session));
    navigate(AppPage.Sessions);
  }, [navigate]);
  const backToSkills = useCallback(() => navigate(AppPage.Skills), [navigate]);
  const saveSkillEditorRows = useCallback((skills?: Parameters<NonNullable<ComponentProps<typeof SkillEditorView>["onSaved"]>>[0]) => {
    if (!skills) return;
    desktopStore.actions.patchSkills(skills);
  }, []);
  const refreshPromptsForView = useCallback(() => {
    void loadDomainForRetry(RuntimeDomainKey.Prompts);
  }, [loadDomainForRetry]);
  const retryRulesForView = useCallback(() => {
    void loadDomainForRetry(RuntimeDomainKey.Rules);
  }, [loadDomainForRetry]);
  const retryHooksForView = useCallback(() => {
    void loadDomainForRetry(RuntimeDomainKey.Hooks);
  }, [loadDomainForRetry]);
  const retryMcpForView = useCallback(() => {
    void loadDomainForRetry(RuntimeDomainKey.Mcp);
  }, [loadDomainForRetry]);
  const handleProjectsScanned = useCallback((nextProjects: ProjectSummary[]) => {
    desktopStore.actions.setProjects(nextProjects);
    void refreshSessionProjects();
    retryRulesForView();
    retryMcpForView();
    void loadDomainForRetry(RuntimeDomainKey.Skills);
  }, [loadDomainForRetry, refreshSessionProjects, retryMcpForView, retryRulesForView]);
  const retryAppSettings = useCallback(() => {
    void loadSettings();
  }, [loadSettings]);
  const checkForUpdatesManually = useCallback(() => {
    void checkForUpdates(true);
  }, [checkForUpdates]);
  const installUpdateManually = useCallback(() => {
    void installUpdate();
  }, [installUpdate]);
  const openUpdateNotes = useCallback(() => setUpdateNotesOpen(true), []);
  const handleThemeChange = useCallback((
    mode: Parameters<NonNullable<ComponentProps<typeof SettingsView>["onThemeChange"]>>[0],
    theme: Parameters<NonNullable<ComponentProps<typeof SettingsView>["onThemeChange"]>>[1],
  ) => {
    const current = desktopStore.getSnapshot().settings.values.themePreferences;
    desktopStore.actions.patchSettings({ themePreferences: { ...current, [mode]: theme } });
  }, []);

  const loadSessionSkillLinks = useCallback(async (session: SessionRecord) => {
    await readSkillIndexStatus();
    const links = await invokeSessionSkillLinks(session.id, session.agent);
    return links.flatMap((link) => {
      const normalized = normalizeSessionSkillLink(link);
      return normalized ? [normalized] : [];
    });
  }, [readSkillIndexStatus]);

  const openSkillByName = useCallback((skillName: string) => {
    const skill = findSkillBySelector(data.skills, skillName);
    if (skill) setActiveSkillId(skill.id);
    navigateTo(skill ? AppPage.SkillDetail : AppPage.Skills);
  }, [data.skills, navigateTo]);

  const openSessionFromLink = useCallback((link: DomainRow) => {
    const sessionId = typeof link.session_id === "string" ? link.session_id.trim() : "";
    const agent = typeof link.agent === "string" ? link.agent.trim() : "";
    if (!sessionId || !agent) return;
    const path = typeof link.session_path === "string" ? link.session_path.trim() : "";
    setActiveSessionKey(path
      ? sessionSourceExternalKey({ agent, id: sessionId, path })
      : `${agent}:${sessionId}`);
    navigateTo(AppPage.Sessions);
  }, [navigateTo]);

  const resolveSessionResumeTarget = useCallback(async (session: SessionRecord): Promise<Exclude<SessionResumeTarget, SessionResumeTarget.Auto>> => {
    const configuredTarget = sessionResumeTargetForAgent(sessionResumeTarget, session.agent);
    if (configuredTarget !== SessionResumeTarget.Auto) return configuredTarget;
    const requestKey = sessionIdentity(session);
    const existingRequest = sessionResumeTargetRequests.current.get(requestKey);
    if (existingRequest) return existingRequest;
    const request = (async () => {
      try {
        const inferredTarget = await inferSessionResumeTarget(sessionLaunchPayload(session));
        return inferredTarget === SessionResumeTarget.App ? SessionResumeTarget.App : SessionResumeTarget.Terminal;
      } catch (error) {
        logger.warn("session resume target inference failed; using terminal", { error });
        return SessionResumeTarget.Terminal;
      }
    })();
    sessionResumeTargetRequests.current.set(requestKey, request);
    return request;
  }, [sessionResumeTarget]);

  const resumeSession = useCallback(async (
    session: SessionRecord,
    requestedTarget?: Exclude<SessionResumeTarget, SessionResumeTarget.Auto>,
  ): Promise<SessionResumeOutcome> => {
    const target = requestedTarget ?? await resolveSessionResumeTarget(session);
    if (target === SessionResumeTarget.App) {
      const appUrl = sessionAppDeepLink(session);
      if (appUrl) {
        try {
          await openUrl(appUrl);
          return { status: SessionResumeOutcomeStatus.Launched, target: SessionResumeTarget.App };
        } catch (error) {
          if (requestedTarget) throw error;
          logger.warn("session app resume unavailable; using terminal", { agent: session.agent, error });
        }
      }
      if (requestedTarget && !appUrl) throw new Error(`${session.agent} sessions cannot be resumed in app`);
    }
    const result = await resumeSessionInTerminal(sessionLaunchPayload(session));
    if (result.status === SessionResumeOutcomeStatus.ActiveWriter) return result;
    return { status: SessionResumeOutcomeStatus.Launched, target: SessionResumeTarget.Terminal, terminal: result.terminal };
  }, [resolveSessionResumeTarget]);

  const previewAndApply = useCallback(async (
    command: SkillChangeCommand,
    args: SkillChangeArgs,
    { onApplied }: { onApplied?: () => void } = {},
  ) => {
    const isUpdate = command === SkillChangeCommand.UpdateMany;
    const isDelete = command === SkillChangeCommand.DeleteMany;
    const names = Array.isArray(args.skillIds)
      && args.skillIds.length > 0
      && args.skillIds.every((skillId): skillId is string => typeof skillId === "string" && skillId.trim().length > 0)
      ? args.skillIds
      : undefined;
    if ((isUpdate || isDelete) && !names) {
      const error = "Skill change requires at least one valid skill ID.";
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
        ? await previewSkillChange(command, args)
        : await previewSkillChangeIfAvailable(command, args);
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
      const result = await applySkillChangeAndCommit(desktopStore, command, {
        ...args,
        ...(command === SkillChangeCommand.UpdateMany ? { previewId, resolutions } : {}),
      });
      const nextSkills = result.updated ?? result.skills;
      if (nextSkills) {
        if (command === SkillChangeCommand.UpdateMany) {
          desktopStore.actions.clearSkillUpdates(names);
        }
      } else if (command === SkillChangeCommand.DeleteMany) {
        desktopStore.actions.patchSkills([], names);
      } else if (command === SkillChangeCommand.UpdateMany) {
        desktopStore.actions.clearSkillUpdates(names);
      }
      setPendingSkillChange(null);
      onApplied?.();
      if (!nextSkills && (command !== SkillChangeCommand.DeleteMany || result?.refreshRequired)) {
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
    desktopStore.actions.setSkillVisibility(names, visibility);
    const result = await applySkillChangeIfAvailable(SkillChangeCommand.Set, { skillIds: names, visibility });
    if (result) commitSkillChangeResult(desktopStore, result);
    if (!result) await refreshSkillList(true);
  }, [refreshSkillList]);

  const applyWrapperSkill = useCallback(
    (args: WrapperArgs) => applySkillChangeAndCommit(desktopStore, SkillChangeCommand.Wrap, args),
    [],
  );
  const applySkillUpdates = useCallback(
    (names: string[], onApplied?: () => void) => previewAndApply(SkillChangeCommand.UpdateMany, { skillIds: names }, { onApplied }),
    [previewAndApply],
  );
  const deleteSkills = useCallback(
    (names: string[], onApplied?: () => void) => previewAndApply(SkillChangeCommand.DeleteMany, { skillIds: names }, { onApplied }),
    [previewAndApply],
  );
  const openSkillFromList = useCallback((skill: NormalizedSkill) => {
    setActiveSkillId(skill.id);
    navigateTo(AppPage.SkillDetail);
  }, [navigateTo]);

  const forceSidebarResizeHover =
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("forceSidebarResizeHover") === "1";
  const sidebarSize = sidebarCollapsed ? COLLAPSED_SIDEBAR_SIZE : SIDEBAR_SIZE;

  const updateToast = desktopUpdate.status === DesktopUpdateStatus.Available && desktopUpdate.version && !updateNoticeDismissed ? (
    <Toast
      tone="success"
      message={`Tendi ${desktopUpdate.version} is available.`}
      action={{
        label: desktopUpdate.body?.trim() ? "View notes" : "Install",
        onClick: () => {
          if (desktopUpdate.body?.trim()) setUpdateNotesOpen(true);
          else void installUpdate();
        },
      }}
      onDismiss={() => setUpdateNoticeDismissed(true)}
    />
  ) : desktopUpdate.status === DesktopUpdateStatus.Installing ? (
    <Toast tone="info" message="Installing Tendi update…" />
  ) : desktopUpdate.status === DesktopUpdateStatus.Error && desktopUpdate.error ? (
    <Toast tone="error" message={`Update failed: ${desktopUpdate.error}`} onDismiss={() => setDesktopUpdate({ status: DesktopUpdateStatus.Idle })} />
  ) : null;

  useLayoutEffect(() => {
    sidebarPanelRef.current?.resize(Number.parseFloat(sidebarSize));
  }, [sidebarPanelRef, sidebarSize]);

  return (
    <main className="appShell">
      {updateToast}
      {desktopUpdate.status === DesktopUpdateStatus.Available && desktopUpdate.version ? (
        <UpdateNotesDialog
          open={updateNotesOpen}
          version={desktopUpdate.version}
          body={desktopUpdate.body}
          onOpenChange={setUpdateNotesOpen}
          onInstall={() => { void installUpdate(); }}
        />
      ) : null}
      {bundledSkillPrompt ? (
        <Suspense fallback={(
          <DialogLoadingFallback
            title={dialogCopy.bundledSkillSetupTitle}
            label="Loading setup dialog"
            descriptionId="bundled-skill-loading-description"
            onOpenChange={(open) => { if (!open && !bundledSkillBusy) void dismissBundledSkillPrompt(); }}
            description={(
              <>This registers the <code>tendi</code> command on your shell PATH, then installs the
                Tendi skill into <span>{bundledSkillPrompt.target}</span>. Coding agents can search local sessions and
                manage skills; no session data is uploaded.</>
            )}
            showLoading={false}
            actions={(
              <>
                <DialogActionButton variant="secondary" disabled={bundledSkillBusy} onClick={() => { void dismissBundledSkillPrompt(); }}>Skip</DialogActionButton>
                <DialogStatefulButton
                  className="dialogStatefulButtonWide"
                  state={bundledSkillBusy ? AsyncStatus.Loading : AsyncStatus.Idle}
                  loadingLabel="Setting up"
                  variant="primary"
                  aria-label="Set up"
                  disabled={!bundledSkillBusy}
                  onClick={() => { void installBundledSkill(); }}
                >
                  Set up
                </DialogStatefulButton>
              </>
            )}
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
          <MemoSidebar
            view={view === AppPage.SkillDetail ? AppPage.Skills : view}
            setView={navigate}
            sources={availableSidebarSources}
            collapsed={sidebarCollapsed}
            setCollapsed={setSidebarCollapsed}
            agentFilter={agentFilter}
            setAgentFilter={setAgentFilter}
          />
        </Panel>
        <Panel className="mainPanel" minSize="520px" {...({ order: 2 } as Record<string, unknown>)}>
          {contentView === AppPage.SkillDetail ? (activeSkill ? (
            <MemoSkillEditorView
              skill={activeSkill}
              skills={data.skills}
              back={backToSkills}
              onReadSkillIndexStatus={readSkillIndexStatus}
              skillIndexStatus={skillIndexStatus}
              onOpenSession={openSessionFromLink}
              onOpenSkill={openSkillByName}
              onSaved={saveSkillEditorRows}
            />
          ) : (
            <PlaceholderView title="Skill unavailable" />
          )) : contentView === AppPage.Skills ? (
            <MemoSkillsView
              skills={filteredData.skills}
              projects={projects}
              installedAgentKeys={installedAgentKeys}
              targetOptions={agentTargets}
              loadingSkills={loadingDomains.has(RuntimeDomainKey.Skills)}
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
          ) : contentView === AppPage.Sessions ? (
            <MemoSessionsView
              sessions={filteredData.sessions as SessionRecord[]}
              developerMode={developerMode}
              loadTranscript={loadSessionTranscript}
              loadTranscriptLocator={loadSessionTranscriptLocator}
              searchTranscript={searchSessionTranscript}
              searchSessions={searchSessionRecords}
              loadSessionSkillLinks={loadSessionSkillLinks}
              skillIndexStatus={skillIndexStatus}
              loadingSessions={loadingDomains.has(RuntimeDomainKey.Sessions)}
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
          ) : contentView === AppPage.Prompts ? (
            <MemoPromptsView
              prompts={filteredData.prompts as ComponentProps<typeof PromptsView>["prompts"]}
              loadingPrompts={loadingDomains.has(RuntimeDomainKey.Prompts)}
              loadError={domainErrors.prompts ?? ""}
              hasRows={data.prompts.length > 0}
              onRefreshPrompts={refreshPromptsForView}
              onPromptSaved={applyPromptSaved}
              onPromptsDeleted={removePrompts}
            />
          ) : contentView === AppPage.Rules ? (
              <MemoRulesView rows={filteredData.rules} skills={data.skills} projects={projects} loadingRows={loadingDomains.has(RuntimeDomainKey.Rules)} loadError={domainErrors.rules ?? ""} hasRows={data.rules.length > 0} onRetry={retryRulesForView} onOpenSkill={openSkillByName} onDeleteRules={deleteRules} onRuleSaved={patchRuleSha256} />
            ) : contentView === AppPage.Hooks ? (
              <MemoHooksView rows={filteredData.hooks} projects={projects} loadingRows={loadingDomains.has(RuntimeDomainKey.Hooks)} loadError={domainErrors.hooks ?? ""} hasRows={data.hooks.length > 0} onRetry={retryHooksForView} onDeleteHook={deleteHook} onDeleteHooks={deleteHooks} onSetHookEnabled={setHookEnabled} onSetHooksEnabled={setHooksEnabled} onReviewHook={reviewHook} />
            ) : contentView === AppPage.Mcp ? (
            <MemoDataListView title="MCP" rows={filteredData.mcp} columns={mcpColumns} projects={projects} loading={loadingDomains.has(RuntimeDomainKey.Mcp)} loadError={domainErrors.mcp ?? ""} hasRows={data.mcp.length > 0} onRetry={retryMcpForView} onSetMcpEnabled={setMcpEnabled} onSetMcpEnabledMany={setMcpEnabledMany} />
          ) : contentView === AppPage.Config ? (
            <MemoConfigView activeProfiles={configProfiles} onActiveProfilesChange={(profiles) => desktopStore.actions.patchSettings({ configProfiles: profiles })} />
          ) : contentView === AppPage.Settings ? (
            <MemoSettingsView
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
              onFontFamilyChange={(value) => desktopStore.actions.patchSettings({ fontFamily: value })}
              onTerminalChange={(value) => desktopStore.actions.patchSettings({ terminal: value })}
              onEditorChange={(value) => desktopStore.actions.patchSettings({ editor: value })}
              onAdditionalSessionRootsChange={(value) => desktopStore.actions.patchSettings({ additionalSessionRoots: value })}
              onDeveloperModeChange={(value) => desktopStore.actions.patchSettings({ developerMode: value })}
              onSessionResumeTargetChange={(value) => desktopStore.actions.patchSettings({ sessionResumeTarget: value })}
              onMissingSessionProjectPolicyChange={(value) => desktopStore.actions.patchSettings({ missingSessionProjectPolicy: value })}
              onAppIconChange={changeAppIcon}
              projects={projects}
              onProjectsScanned={handleProjectsScanned}
              appSettingsLoading={appSettings.loading}
              appSettingsLoadError={appSettings.error}
              onRetryAppSettings={retryAppSettings}
              update={desktopUpdate}
              onCheckForUpdates={checkForUpdatesManually}
              onInstallUpdate={installUpdateManually}
              onViewUpdateNotes={openUpdateNotes}
              onSkillsUpdated={applySkillRows}
              installedAgentKeys={installedAgentKeys}
              targetOptions={agentTargets}
              onThemeChange={handleThemeChange}
            />
          ) : contentView === AppPage.Overview ? (
            <OverviewView
              counts={overviewCounts}
              hookReviewCount={overviewHookReviewCount}
              sessions={filteredData.sessions as SessionRecord[]}
              skillUpdateCount={overviewSkillUpdateCount}
              onRetryAnalyticsRevision={refreshAnalyticsRevision}
              agentFilter={agentFilter}
              overviewCountsLoaded={overviewCountsLoaded}
              overviewCountErrors={overviewCountErrors}
              onRetryCounts={retryCatalogCounts}
              sessionListStatus={sessionListStatus}
              sessionListError={sessionListError}
              onNavigate={navigateToDomain}
              onOpenSession={openOverviewSession}
            />
          ) : (
            <PlaceholderView title={activeNav?.label ?? "Overview"} />
          )}
        </Panel>
      </PanelGroup>
    </main>
  );
}
