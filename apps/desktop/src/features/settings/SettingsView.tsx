import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { CheckCircle2, Monitor, Moon, Sun, Trash2 } from "lucide-react";
import { DropdownMenu, Popover } from "radix-ui";
import { actionLabels, AsyncStatus, CliInstallState, compactDateTime, DesktopUpdateStatus, formatUserPath, logExportLabels, MissingSessionProjectPolicy, normalizeSettings, remoteRepositoryLabel, TauriCommand, normalizeMissingSessionProjectPolicy, normalizeSessionResumeTarget, safeInvoke, SessionResumeTarget, type BundledSkillStatus, type CliInstallStatus, type DesktopUpdateState, type ProjectSummary, type RawSkillRecord, type SettingsPayload, type SettingsState, type SkillInstallResult } from "../../lib/index.ts";
import { Appearance, ColorTheme, FontFamily, type ResolvedAppearance, type ThemePreferences } from "../../lib/appearance.ts";
import { appIconOptions, appIconPreviewDataUrl, type AppIcon } from "../../lib/app-icon.ts";
import { Button } from "../../components/shared/Button.tsx";
import { Badge } from "../../components/shared/Badge.tsx";
import { ContentTopDragStrip } from "../../components/shared/ContentTopDragStrip.tsx";
import { CompactTable, type CompactTableColumn } from "../../components/shared/CompactTable.tsx";
import { DeleteConfirmationDialog } from "../../components/shared/DeleteConfirmationDialog.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { PageHeader } from "../../components/shared/PageHeader.tsx";
import { SegmentedControl, SegmentedControlItem } from "../../components/shared/SegmentedControl.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton } from "../../components/shared/StatefulButton.tsx";
import { Toast } from "../../components/shared/Toast.tsx";
import { Switch } from "../../components/shared/Switch.tsx";
import { RowActionsMenu } from "../../components/shared/RowActionsMenu.tsx";
import { SettingsApplicationPicker, type SettingsApplicationOption } from "./SettingsApplicationPicker.tsx";
import { SettingsSection } from "./SettingsSection.tsx";
import { BackupSettings } from "../skills/BackupView.tsx";
import { AddSkillDialog } from "../../views/SkillsView.tsx";
import {
  exportLogs as exportLogsCommand,
  installCli as installCliCommand,
  readBundledSkillStatus,
  readCliStatus,
  readProjectScanScopes,
  readTerminalApps,
  removeCli as removeCliCommand,
  saveProjectScanScopes as saveProjectScanScopesCommand,
  saveSettings,
  scanProjects as scanProjectsCommand,
  testEditorApp,
  testTerminalApp,
} from "../../lib/runtime-gateway.ts";
import "./SettingsView.css";

type TerminalApp = {
  id: string;
  label: string;
  available?: boolean;
};

const projectTableColumns: CompactTableColumn<ProjectSummary>[] = [
  { key: "name", header: "Project", width: "160px", cellClassName: "compactTableCell--title", empty: "" },
  {
    key: "rootPath",
    header: "Path",
    width: "minmax(180px, 1fr)",
    cellClassName: "compactTableCell--muted",
    value: (project) => formatUserPath(project.rootPath),
    title: (project) => project.rootPath || undefined,
    empty: "-",
  },
  {
    key: "remoteUrl",
    header: "Remote",
    width: "minmax(140px, 0.8fr)",
    cellClassName: "compactTableCell--muted",
    value: (project) => remoteRepositoryLabel(project.remoteUrl) || undefined,
    title: (project) => project.remoteUrl || undefined,
    empty: "-",
  },
  {
    key: "lastScannedAt",
    header: "Last scanned",
    width: "minmax(140px, 0.7fr)",
    cellClassName: "compactTableCell--muted",
    value: (project) => compactDateTime(project.lastScannedAt, { year: true }) || undefined,
    title: (project) => project.lastScannedAt || undefined,
    empty: "-",
  },
];

function ProjectsPopover({ projects }: { projects: ProjectSummary[] }) {
  return (
    <Popover.Root>
      <Popover.Trigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className="settingsProjectCountTrigger"
          aria-label={`Show ${projects.length} scanned projects`}
        >
          {projects.length} projects found
        </Button>
      </Popover.Trigger>
      <Popover.Portal>
        <Popover.Content
          className="settingsProjectsPopover"
          side="bottom"
          align="start"
          sideOffset={8}
          collisionPadding={8}
        >
          <div className="settingsProjectsPopoverHeader">
            <strong>Scanned projects</strong>
            <span>{projects.length}</span>
          </div>
          <CompactTable
            className="settingsProjectsTable"
            ariaLabel="Scanned projects"
            rows={projects}
            columns={projectTableColumns}
            getRowId={(project) => project.id}
            emptyState="No projects found"
          />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

type SettingsViewProps = {
  appearance: Appearance;
  themePreferences: ThemePreferences;
  fontFamily: FontFamily;
  terminal: string;
  editor: string;
  additionalSessionRoots: string[];
  developerMode: boolean;
  onAppearanceChange: (appearance: Appearance) => void;
  onThemeChange: (mode: ResolvedAppearance, theme: ColorTheme) => void;
  onFontFamilyChange: (fontFamily: FontFamily) => void;
  onTerminalChange: (terminal: string) => void;
  onEditorChange: (editor: string) => void;
  onAdditionalSessionRootsChange: (roots: string[]) => void;
  onDeveloperModeChange: (enabled: boolean) => void;
  sessionResumeTarget: SessionResumeTarget;
  missingSessionProjectPolicy: MissingSessionProjectPolicy;
  onSessionResumeTargetChange: (target: SessionResumeTarget) => void;
  onMissingSessionProjectPolicyChange: (policy: MissingSessionProjectPolicy) => void;
  appIcon: AppIcon;
  onAppIconChange: (appIcon: AppIcon) => void;
  configProfiles: Record<string, string>;
  projects: ProjectSummary[];
  onProjectsScanned?: (projects: ProjectSummary[]) => void;
  appSettingsLoading: boolean;
  appSettingsLoadError: string;
  onRetryAppSettings: () => void;
  update: DesktopUpdateState;
  onCheckForUpdates: () => void;
  onInstallUpdate: () => void;
  onViewUpdateNotes: () => void;
  onSkillsUpdated?: (skills: RawSkillRecord[], options?: { patch?: boolean; deleted?: string[] }) => void;
  installedAgentKeys: string[];
  targetOptions: Array<{
    id: string;
    displayName: string;
    supportsGlobal: boolean;
    globalPath?: string;
  }>;
};

const appearanceOptions = [
  { value: Appearance.System, label: "System", icon: Monitor },
  { value: Appearance.Light, label: "Light", icon: Sun },
  { value: Appearance.Dark, label: "Dark", icon: Moon },
] as const;

const themeOptions = [
  { value: ColorTheme.SakuraPop, label: "Sakura Pop" },
  { value: ColorTheme.Gruvbox, label: "Gruvbox" },
  { value: ColorTheme.Dracula, label: "Dracula" },
  { value: ColorTheme.Nord, label: "Nord" },
  { value: ColorTheme.Catppuccin, label: "Catppuccin" },
  { value: ColorTheme.TokyoNight, label: "Tokyo Night" },
  { value: ColorTheme.Vercel, label: "Vercel" },
] as const;

const fontOptions = [
  { value: FontFamily.Geist, label: "Geist" },
  { value: FontFamily.Manrope, label: "Manrope" },
  { value: FontFamily.Inter, label: "Inter" },
  { value: FontFamily.IbmPlexSans, label: "IBM Plex Sans" },
  { value: FontFamily.InstrumentSans, label: "Instrument Sans" },
  { value: FontFamily.PlusJakartaSans, label: "Plus Jakarta Sans" },
  { value: FontFamily.BricolageGrotesque, label: "Bricolage Grotesque" },
] as const satisfies ReadonlyArray<{ value: FontFamily; label: string }>;

const themePreviewColors: Record<ResolvedAppearance, Record<ColorTheme, { foreground: string; background: string }>> = {
  [Appearance.Light]: {
    [ColorTheme.SakuraPop]: { foreground: "#4b2347", background: "#fff4fb" },
    [ColorTheme.Gruvbox]: { foreground: "#282828", background: "#fbf1c7" },
    [ColorTheme.Dracula]: { foreground: "#282a36", background: "#f8f8f2" },
    [ColorTheme.Nord]: { foreground: "#2e3440", background: "#eceff4" },
    [ColorTheme.Catppuccin]: { foreground: "#4c4f69", background: "#eff1f5" },
    [ColorTheme.TokyoNight]: { foreground: "#3760bf", background: "#e1e2e7" },
    [ColorTheme.Vercel]: { foreground: "#171717", background: "#ffffff" },
  },
  [Appearance.Dark]: {
    [ColorTheme.SakuraPop]: { foreground: "#f8e8f5", background: "#211331" },
    [ColorTheme.Gruvbox]: { foreground: "#ebdbb2", background: "#282828" },
    [ColorTheme.Dracula]: { foreground: "#f8f8f2", background: "#282a36" },
    [ColorTheme.Nord]: { foreground: "#eceff4", background: "#2e3440" },
    [ColorTheme.Catppuccin]: { foreground: "#cdd6f4", background: "#1e1e2e" },
    [ColorTheme.TokyoNight]: { foreground: "#c0caf5", background: "#1a1b26" },
    [ColorTheme.Vercel]: { foreground: "#ededed", background: "#000000" },
  },
};

const themeModes = [
  { value: Appearance.Light, label: "Light theme", key: "lightTheme" },
  { value: Appearance.Dark, label: "Dark theme", key: "darkTheme" },
] as const satisfies ReadonlyArray<{ value: ResolvedAppearance; label: string; key: "lightTheme" | "darkTheme" }>;

type ThemeMode = (typeof themeModes)[number];

type SettingsInputKey = "terminal" | "editor" | "additionalSessionRoots" | "projectScanScopes";
type SettingsInputRevisions = Record<SettingsInputKey, number>;

function ThemeSelect({ mode, value, onChange }: { mode: ThemeMode; value: ColorTheme; onChange: (value: ColorTheme) => void }) {
  return (
    <div className="settingsThemePicker">
      <span className="settingsThemePickerLabel">{mode.label}</span>
      <SelectControl
        className="settingsThemeSelect"
        contentClassName="settingsThemeSelectContent"
        label={mode.label}
        value={value}
        onValueChange={(nextValue) => onChange(nextValue as ColorTheme)}
        options={[...themeOptions]}
        showOptionTooltip={false}
        renderOption={(option) => {
          const colors = themePreviewColors[mode.value][option.value as ColorTheme];
          return (
            <span className="settingsThemeOption">
              <span
                className="settingsThemeSwatch"
                style={{ background: `linear-gradient(135deg, ${colors.foreground} 0 50%, ${colors.background} 50% 100%)` }}
                aria-hidden="true"
              />
              <span>{option.label}</span>
            </span>
          );
        }}
      />
    </div>
  );
}

function AppIconSelect({ value, onChange }: { value: AppIcon; onChange: (value: AppIcon) => void }) {
  return (
    <SelectControl
      className="settingsAppIconSelect"
      contentClassName="settingsAppIconSelectContent"
      label="Application icon"
      value={value}
      onValueChange={(nextValue) => onChange(nextValue as AppIcon)}
      options={[...appIconOptions]}
      showOptionTooltip={false}
      renderValue={(option) => option ? (
        <span className="settingsAppIconOption">
          <img className="settingsAppIconPreview" src={appIconPreviewDataUrl(option.value)} alt="" aria-hidden="true" />
          <span>{option.label}</span>
        </span>
      ) : null}
      renderOption={(option) => (
        <span className="settingsAppIconOption">
          <img className="settingsAppIconPreview" src={appIconPreviewDataUrl(option.value)} alt="" aria-hidden="true" />
          <span>{option.label}</span>
        </span>
      )}
    />
  );
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return `${error}`;
}

function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settingsGroup">
      <h2 className="settingsGroupTitle">{title}</h2>
      <div className="settingsGroupItems">{children}</div>
    </section>
  );
}

export function SettingsView({ appearance, themePreferences, fontFamily, terminal, editor, additionalSessionRoots, developerMode, sessionResumeTarget, missingSessionProjectPolicy, appIcon, configProfiles, projects, onAppearanceChange, onThemeChange, onFontFamilyChange, onTerminalChange, onEditorChange, onAdditionalSessionRootsChange, onDeveloperModeChange, onSessionResumeTargetChange, onMissingSessionProjectPolicyChange, onAppIconChange, onProjectsScanned, appSettingsLoading, appSettingsLoadError, onRetryAppSettings, update, onCheckForUpdates, onInstallUpdate, onViewUpdateNotes, onSkillsUpdated, installedAgentKeys, targetOptions }: SettingsViewProps) {
  const [terminalInput, setTerminalInput] = useState("auto");
  const [editorInput, setEditorInput] = useState("vscode");
  const [additionalSessionRootsInput, setAdditionalSessionRootsInput] = useState("");
  const [projectScanScopesInput, setProjectScanScopesInput] = useState("");
  const [terminalApps, setTerminalApps] = useState<TerminalApp[]>([]);
  const [terminalError, setTerminalError] = useState("");
  const [sessionResumeError, setSessionResumeError] = useState("");
  const [missingSessionProjectError, setMissingSessionProjectError] = useState("");
  const [editorError, setEditorError] = useState("");
  const [sessionRootsError, setSessionRootsError] = useState("");
  const [projectScanScopesError, setProjectScanScopesError] = useState("");
  const [projectScanState, setProjectScanState] = useState<AsyncStatus>(AsyncStatus.Idle);
  const [projectScanSummary, setProjectScanSummary] = useState("");
  const [appearanceError, setAppearanceError] = useState("");
  const [themeError, setThemeError] = useState("");
  const [appIconError, setAppIconError] = useState("");
  const [fontFamilyError, setFontFamilyError] = useState("");
  const [developerModeError, setDeveloperModeError] = useState("");
  const [cliStatus, setCliStatus] = useState<CliInstallStatus | null>(null);
  enum CliAction {
    Install = "install",
    Remove = "remove",
  }
  const [cliBusy, setCliBusy] = useState<CliAction | null>(null);
  const [cliError, setCliError] = useState("");
  const [bundledSkillStatus, setBundledSkillStatus] = useState<BundledSkillStatus | null>(null);
  const [bundledSkillError, setBundledSkillError] = useState("");
  const [bundledSkillInstallOpen, setBundledSkillInstallOpen] = useState(false);
  const [confirmRemoveCli, setConfirmRemoveCli] = useState(false);
  const [settingsLoading, setSettingsLoading] = useState(true);
  const [settingsLoadError, setSettingsLoadError] = useState("");
  const [logExportState, setLogExportState] = useState<AsyncStatus>(AsyncStatus.Idle);
  const [logExportError, setLogExportError] = useState("");
  const appearanceSaveRequestRef = useRef(0);
  const themeSaveRequestRef = useRef(0);
  const appIconSaveRequestRef = useRef(0);
  const fontFamilySaveRequestRef = useRef(0);
  const inputRevisionRef = useRef<SettingsInputRevisions>({
    terminal: 0,
    editor: 0,
    additionalSessionRoots: 0,
    projectScanScopes: 0,
  });
  const onThemeChangeRef = useRef(onThemeChange);
  onThemeChangeRef.current = onThemeChange;
  const terminalOptions: SettingsApplicationOption[] = useMemo(() => {
    const items = terminalApps.length ? terminalApps : [{ id: "auto", label: "Auto", available: true }];
    return items.map((app) => ({
      value: app.id,
      label: app.label,
      available: app.available,
    }));
  }, [terminalApps]);
  const editorOptions: SettingsApplicationOption[] = [
    { value: "vscode", label: "VS Code" },
    { value: "zed", label: "Zed" },
    { value: "cursor", label: "Cursor" },
    { value: "coteditor", label: "CotEditor" },
  ];

  const loadSettings = useCallback(async () => {
    const inputRevisions = { ...inputRevisionRef.current };
    setSettingsLoading(true);
    setSettingsLoadError("");
    const [apps, nextCliStatus, nextBundledSkillStatus, nextProjectScopes] = await Promise.all([
      readTerminalApps(),
      readCliStatus(),
      readBundledSkillStatus(),
      readProjectScanScopes(),
    ]);
    const errors = [
      apps ? "" : "Unable to read terminal applications",
      nextCliStatus ? "" : "Unable to read CLI status",
      nextProjectScopes ? "" : "Unable to read project scan scopes",
    ].filter(Boolean);
    if (apps) setTerminalApps(apps);
    if (nextCliStatus) setCliStatus(nextCliStatus);
    if (nextBundledSkillStatus) setBundledSkillStatus(nextBundledSkillStatus);
    if (nextProjectScopes) {
      if (inputRevisionRef.current.projectScanScopes === inputRevisions.projectScanScopes) {
        setProjectScanScopesInput(nextProjectScopes
          .filter((scope) => scope.enabled)
          .map((scope) => `${scope.excluded ? "!" : ""}${scope.path}`)
          .join("\n"));
      }
    }
    setSettingsLoadError(errors.join("; "));
    setSettingsLoading(false);
  }, []);

  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  useEffect(() => {
    if (inputRevisionRef.current.terminal === 0) setTerminalInput(terminal);
    if (inputRevisionRef.current.editor === 0) setEditorInput(editor);
    if (inputRevisionRef.current.additionalSessionRoots === 0) {
      setAdditionalSessionRootsInput(additionalSessionRoots.join("\n"));
    }
  }, [additionalSessionRoots, editor, terminal]);

  const applySavedSettings = (value: SettingsState, syncedInputs: Partial<SettingsInputRevisions> = {}) => {
    const savedSettings = value;
    if (syncedInputs.terminal === inputRevisionRef.current.terminal) setTerminalInput(savedSettings.terminal);
    if (syncedInputs.editor === inputRevisionRef.current.editor) setEditorInput(savedSettings.editor);
    if (syncedInputs.additionalSessionRoots === inputRevisionRef.current.additionalSessionRoots) {
      setAdditionalSessionRootsInput(savedSettings.additionalSessionRoots.join("\n"));
    }
    onTerminalChange(savedSettings.terminal);
    onEditorChange(savedSettings.editor);
    onAdditionalSessionRootsChange(savedSettings.additionalSessionRoots);
    onAppearanceChange(savedSettings.appearance);
    onThemeChangeRef.current(Appearance.Light, savedSettings.lightTheme);
    onThemeChangeRef.current(Appearance.Dark, savedSettings.darkTheme);
    onFontFamilyChange(savedSettings.fontFamily);
    onDeveloperModeChange(savedSettings.developerMode);
    onSessionResumeTargetChange(savedSettings.sessionResumeTarget);
    onMissingSessionProjectPolicyChange(savedSettings.missingSessionProjectPolicy);
    onAppIconChange(savedSettings.appIcon);
    return savedSettings;
  };

  const buildSettingsPayload = (overrides: Partial<SettingsState> = {}): SettingsPayload => ({
    ...normalizeSettings({
      appearance,
      lightTheme: themePreferences.light,
      darkTheme: themePreferences.dark,
      appIcon,
      fontFamily,
      terminal,
      editor,
      sessionResumeTarget,
      missingSessionProjectPolicy,
      developerMode,
      additionalSessionRoots,
      ...overrides,
    }),
    configProfiles,
  });

  const saveAppIcon = async (nextAppIcon: AppIcon) => {
    const previousAppIcon = appIcon;
    const requestId = appIconSaveRequestRef.current + 1;
    appIconSaveRequestRef.current = requestId;
    setAppIconError("");
    onAppIconChange(nextAppIcon);
    const nextSettings = await saveSettings(buildSettingsPayload({ appIcon: nextAppIcon }));
    if (appIconSaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      applySavedSettings(nextSettings);
    } else {
      onAppIconChange(previousAppIcon);
      setAppIconError(actionLabels.saveFailed);
    }
  };

  const saveAppearance = async (nextAppearance: Appearance) => {
    const previousAppearance = appearance;
    const requestId = appearanceSaveRequestRef.current + 1;
    appearanceSaveRequestRef.current = requestId;
    setAppearanceError("");
    onAppearanceChange(nextAppearance);
    const nextSettings = await saveSettings(buildSettingsPayload({ appearance: nextAppearance }));
    if (appearanceSaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      applySavedSettings(nextSettings);
    } else {
      onAppearanceChange(previousAppearance);
      setAppearanceError(actionLabels.saveFailed);
    }
  };

  const saveTheme = async (mode: ResolvedAppearance, nextTheme: ColorTheme) => {
    const key = mode === Appearance.Light ? "lightTheme" : "darkTheme";
    const previousTheme = themePreferences[mode];
    const requestId = themeSaveRequestRef.current + 1;
    themeSaveRequestRef.current = requestId;
    setThemeError("");
    onThemeChange(mode, nextTheme);
    const nextSettings = await saveSettings(buildSettingsPayload({ [key]: nextTheme }));
    if (themeSaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      applySavedSettings(nextSettings);
    } else {
      onThemeChange(mode, previousTheme);
      setThemeError(actionLabels.saveFailed);
    }
  };

  const saveFontFamily = async (nextFontFamily: FontFamily) => {
    const previousFontFamily = fontFamily;
    const requestId = fontFamilySaveRequestRef.current + 1;
    fontFamilySaveRequestRef.current = requestId;
    setFontFamilyError("");
    onFontFamilyChange(nextFontFamily);
    const nextSettings = await saveSettings(buildSettingsPayload({ fontFamily: nextFontFamily }));
    if (fontFamilySaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      applySavedSettings(nextSettings);
    } else {
      onFontFamilyChange(previousFontFamily);
      setFontFamilyError(actionLabels.saveFailed);
    }
  };

  const saveTerminal = async (terminal: string) => {
    const inputRevision = inputRevisionRef.current.terminal;
    const normalized = terminal.trim() || "auto";
    setTerminalInput(normalized);
    if (normalized === terminal) return;
    const nextSettings = await saveSettings(buildSettingsPayload({ terminal: normalized }));
    if (nextSettings) {
      applySavedSettings(nextSettings, { terminal: inputRevision });
      setTerminalError("");
    } else {
      setTerminalError(actionLabels.saveFailed);
    }
  };

  const saveSessionResumeTarget = async (target: string) => {
    const normalized = normalizeSessionResumeTarget(target);
    if (normalized === sessionResumeTarget) return;
    const previous = sessionResumeTarget;
    onSessionResumeTargetChange(normalized);
    const nextSettings = await saveSettings(buildSettingsPayload({ sessionResumeTarget: normalized }));
    if (nextSettings) {
      applySavedSettings(nextSettings);
      setSessionResumeError("");
    } else {
      onSessionResumeTargetChange(previous);
      setSessionResumeError(actionLabels.saveFailed);
    }
  };

  const saveMissingSessionProjectPolicy = async (policy: string) => {
    const normalized = normalizeMissingSessionProjectPolicy(policy);
    if (normalized === missingSessionProjectPolicy) return;
    const previous = missingSessionProjectPolicy;
    onMissingSessionProjectPolicyChange(normalized);
    const nextSettings = await saveSettings(buildSettingsPayload({ missingSessionProjectPolicy: normalized }));
    if (nextSettings) {
      applySavedSettings(nextSettings);
      setMissingSessionProjectError("");
    } else {
      onMissingSessionProjectPolicyChange(previous);
      setMissingSessionProjectError(actionLabels.saveFailed);
    }
  };

  const saveEditor = async (editor: string) => {
    const inputRevision = inputRevisionRef.current.editor;
    const normalized = editor.trim() || "vscode";
    setEditorInput(normalized);
    if (normalized === editor) return;
    const nextSettings = await saveSettings(buildSettingsPayload({ editor: normalized }));
    if (nextSettings) {
      applySavedSettings(nextSettings, { editor: inputRevision });
      setEditorError("");
    } else {
      setEditorError(actionLabels.saveFailed);
    }
  };

  const saveDeveloperMode = async (nextDeveloperMode: boolean) => {
    const previousDeveloperMode = developerMode;
    if (nextDeveloperMode === previousDeveloperMode) return;
    setDeveloperModeError("");
    onDeveloperModeChange(nextDeveloperMode);
    const nextSettings = await saveSettings(buildSettingsPayload({ developerMode: nextDeveloperMode }));
    if (nextSettings) {
      applySavedSettings(nextSettings);
    } else {
      onDeveloperModeChange(previousDeveloperMode);
      setDeveloperModeError(actionLabels.saveFailed);
    }
  };

  const saveAdditionalSessionRoots = async (value: string) => {
    const inputRevision = inputRevisionRef.current.additionalSessionRoots;
    const roots = value
      .split("\n")
      .map((root) => root.trim())
      .filter(Boolean);
    setAdditionalSessionRootsInput(roots.join("\n"));
    if (roots.join("\n") === additionalSessionRoots.join("\n")) return;
    const nextSettings = await saveSettings(buildSettingsPayload({ additionalSessionRoots: roots }));
    if (nextSettings) {
      applySavedSettings(nextSettings, { additionalSessionRoots: inputRevision });
      setSessionRootsError("");
    } else {
      setSessionRootsError(`${actionLabels.saveFailed}: use absolute paths`);
    }
  };

  const saveProjectScanScopes = async (value: string) => {
    const paths = value
      .split("\n")
      .map((path) => path.trim())
      .filter(Boolean);
    setProjectScanScopesInput(paths.join("\n"));
    const result = await saveProjectScanScopesCommand(paths);
    if (result) {
      setProjectScanScopesError("");
      setProjectScanSummary(`${result.length} scan scope${result.length === 1 ? "" : "s"} saved`);
    } else {
      setProjectScanScopesError(`${actionLabels.saveFailed}: use absolute paths`);
    }
  };

  const scanProjects = async () => {
    if (projectScanState === AsyncStatus.Loading) return;
    setProjectScanState(AsyncStatus.Loading);
    setProjectScanSummary("");
    const result = await scanProjectsCommand();
    if (result) {
      if (Array.isArray(result.projects)) onProjectsScanned?.(result.projects);
      setProjectScanState(AsyncStatus.Success);
      setProjectScanSummary(`${result.projects?.length ?? 0} projects found`);
    } else {
      setProjectScanState(AsyncStatus.Error);
      setProjectScanSummary("Scan failed");
    }
  };

  const testTerminal = async (terminal: string) => {
    return testTerminalApp(terminal);
  };

  const testEditor = async (editor: string) => {
    return testEditorApp(editor);
  };

  const exportLogs = async () => {
    if (logExportState === AsyncStatus.Loading) return;
    setLogExportState(AsyncStatus.Loading);
    setLogExportError("");
    try {
      const exportPath = await exportLogsCommand();
      await safeInvoke(TauriCommand.RevealInFinder, { path: exportPath });
      setLogExportState(AsyncStatus.Success);
    } catch (error) {
      setLogExportState(AsyncStatus.Error);
      setLogExportError(errorMessage(error));
    }
  };

  const changeCliRegistration = async () => {
    if (cliBusy) return;
    setCliBusy(CliAction.Install);
    setCliError("");
    try {
      const nextStatus = await installCliCommand();
      setCliStatus(nextStatus);
    } catch (error) {
      setCliError(`${error}`);
      const nextStatus = await readCliStatus();
      if (nextStatus) setCliStatus(nextStatus);
    } finally {
      setCliBusy(null);
    }
  };

  const removeCli = async () => {
    if (cliBusy) return;
    setCliBusy(CliAction.Remove);
    setCliError("");
    try {
      const nextStatus = await removeCliCommand();
      setCliStatus(nextStatus);
    } catch (error) {
      setCliError(`${error}`);
      const nextStatus = await readCliStatus();
      if (nextStatus) setCliStatus(nextStatus);
    } finally {
      setCliBusy(null);
      setConfirmRemoveCli(false);
    }
  };

  const requestRemoveCli = () => {
    if (cliBusy) return;
    setConfirmRemoveCli(true);
  };

  const cliInstalled = cliStatus?.state === CliInstallState.Installed;
  const cliNeedsRepair = cliStatus?.state === CliInstallState.Stale;
  const cliConflict = cliStatus?.state === CliInstallState.Conflict;
  const cliPathNeedsAttention = cliInstalled && !cliStatus.pathConfigured;
  const cliHealthy = cliInstalled && cliStatus.pathConfigured;
  const bundledSkillHealthy = bundledSkillStatus?.current === true;
  const bundledSkillConflict = bundledSkillStatus?.installed === true && !bundledSkillHealthy;
  const combinedSettingsLoadError = [appSettingsLoadError, settingsLoadError].filter(Boolean).join("; ");
  const retrySettings = () => {
    if (appSettingsLoadError) onRetryAppSettings();
    if (settingsLoadError) void loadSettings();
  };
  const installBundledSkill = (result: SkillInstallResult) => {
    onSkillsUpdated?.(result.updated ?? result.skills ?? [], { patch: true });
    setBundledSkillError("");
    setBundledSkillInstallOpen(false);
    void readBundledSkillStatus().then((status) => {
      if (status) setBundledSkillStatus(status);
    });
  };

  return (
    <section className="content dataPage settingsPage">
      <DeleteConfirmationDialog
        open={confirmRemoveCli}
        items={["Tendi CLI"]}
        itemLabel="CLI integration"
        title="Uninstall Tendi CLI integration?"
        description="The Tendi CLI command will be removed from your shell PATH."
        confirmLabel="Uninstall"
        loadingLabel="Uninstalling"
        busy={cliBusy !== null}
        onOpenChange={setConfirmRemoveCli}
        onConfirm={() => { void removeCli(); }}
      />
      <AddSkillDialog
        open={bundledSkillInstallOpen}
        onOpenChange={setBundledSkillInstallOpen}
        onClose={() => setBundledSkillInstallOpen(false)}
        onPreviewError={setBundledSkillError}
        onInstalled={installBundledSkill}
        onRequestWrapper={() => undefined}
        installedAgentKeys={installedAgentKeys}
        targetOptions={targetOptions}
        initialSource="tendi://bundled"
        sourceLocked
        title="Install Tendi skill"
      />
      <ContentTopDragStrip />
      <PageHeader title="Settings">
        <div className="settingsUpdateActions">
            {update.status === DesktopUpdateStatus.Available && update.version && update.body?.trim() ? (
            <Button size="sm" onClick={onViewUpdateNotes}>Release notes</Button>
          ) : null}
          <StatefulButton
            size="sm"
            state={update.status === DesktopUpdateStatus.Checking || update.status === DesktopUpdateStatus.Installing ? AsyncStatus.Loading : update.status === DesktopUpdateStatus.UpToDate ? AsyncStatus.Success : update.status === DesktopUpdateStatus.Error ? AsyncStatus.Error : AsyncStatus.Idle}
            width={160}
            minWidth={160}
            onClick={update.status === DesktopUpdateStatus.Available ? onInstallUpdate : onCheckForUpdates}
            aria-label={update.status === DesktopUpdateStatus.Checking ? "Checking for updates" : update.status === DesktopUpdateStatus.Installing ? "Installing update" : update.status === DesktopUpdateStatus.Available && update.version ? `Install update ${update.version}` : update.status === DesktopUpdateStatus.UpToDate ? "You're up to date" : update.status === DesktopUpdateStatus.Error ? "Check for updates again" : "Check for updates"}
            loadingContent={<LoadingIcon size={16} />}
            successContent="You're up to date"
            errorContent="Check failed — try again"
          >
            {update.status === DesktopUpdateStatus.Available && update.version ? `Install ${update.version}` : actionLabels.checkForUpdates}
          </StatefulButton>
        </div>
      </PageHeader>
      {combinedSettingsLoadError ? <LoadErrorState message={combinedSettingsLoadError} onRetry={retrySettings} /> : null}
      <div className="settingsShell">
        <div className="settingsGroups">
          <SettingsGroup title="Appearance">
          <SettingsSection title="Mode">
              <SegmentedControl
                className="settingsAppearanceControl"
                value={appearance}
                onValueChange={(value) => {
                  if (value) saveAppearance(value as Appearance);
                }}
                aria-label="Application appearance"
              >
                {appearanceOptions.map((option) => {
                  const Icon = option.icon;
                  return (
                    <SegmentedControlItem
                      value={option.value}
                      aria-label={option.label}
                      key={option.value}
                    >
                      <Icon size={14} />
                      <span>{option.label}</span>
                    </SegmentedControlItem>
                  );
                })}
              </SegmentedControl>
              {appearanceError ? <Toast tone="error" message={appearanceError} /> : null}
          </SettingsSection>
          <SettingsSection title="Theme" className="settingsThemeSection">
            <div className="settingsThemeControls">
              {themeModes.map((mode) => (
                <ThemeSelect
                  key={mode.value}
                  mode={mode}
                  value={themePreferences[mode.value]}
                  onChange={(value) => { void saveTheme(mode.value, value); }}
                />
              ))}
              <div className="settingsThemePicker">
                <span className="settingsThemePickerLabel">App icon</span>
                <AppIconSelect value={appIcon} onChange={(value) => { void saveAppIcon(value); }} />
                {appIconError ? <Toast tone="error" message={appIconError} /> : null}
              </div>
            </div>
            {themeError ? <Toast tone="error" message={themeError} /> : null}
          </SettingsSection>
          <SettingsSection title="Font">
            <SelectControl
              className="settingsFontSelect"
              contentClassName="settingsSelectContent"
              label="Application font"
              value={fontFamily}
              onValueChange={(value) => { void saveFontFamily(value as FontFamily); }}
              options={[...fontOptions]}
              showOptionTooltip={false}
              renderOption={(option) => <span style={{ fontFamily: `"${option.label}"` }}>{option.label}</span>}
            />
            {fontFamilyError ? <Toast tone="error" message={fontFamilyError} /> : null}
          </SettingsSection>
          </SettingsGroup>
          <SettingsGroup title="General">
          <div className="settingsApplicationPair">
          <SettingsSection title="Terminal">
            <SettingsApplicationPicker
              id="settings-terminal"
              ariaLabel="Terminal application"
              menuAriaLabel="Choose terminal application"
              placeholder="Terminal application name"
              value={terminalInput}
              savedValue={terminal}
              options={terminalOptions}
              error={terminalError}
              labels={{
                opening: "Opening terminal application",
                opened: "Terminal application opened",
                failed: "Could not open terminal application",
                test: "Test terminal application",
              }}
              onChange={(value) => {
                setTerminalInput(value);
                inputRevisionRef.current.terminal += 1;
                setTerminalError("");
              }}
              onSave={saveTerminal}
              onCancel={() => setTerminalError("")}
              onTest={testTerminal}
            />
          </SettingsSection>
          <SettingsSection title="Editor">
            <SettingsApplicationPicker
              id="settings-editor"
              ariaLabel="Editor application"
              menuAriaLabel="Choose editor application"
              placeholder="Editor command"
              value={editorInput}
              savedValue={editor}
              options={editorOptions}
              error={editorError}
              labels={{
                opening: "Testing editor command",
                opened: "Editor command available",
                failed: "Could not find editor command",
                test: "Test editor command",
              }}
              onChange={(value) => {
                setEditorInput(value);
                inputRevisionRef.current.editor += 1;
                setEditorError("");
              }}
              onSave={saveEditor}
              onCancel={() => setEditorError("")}
              onTest={testEditor}
            />
          </SettingsSection>
          </div>
          <SettingsSection title="Session resume">
            <SegmentedControl
              className="settingsSessionResumeControl"
              value={sessionResumeTarget}
              onValueChange={(value) => { void saveSessionResumeTarget(value); }}
              aria-label="Prefer opening resumed sessions in"
            >
              <SegmentedControlItem value={SessionResumeTarget.Auto}>Auto</SegmentedControlItem>
              <SegmentedControlItem value={SessionResumeTarget.Terminal}>Terminal</SegmentedControlItem>
              <SegmentedControlItem value={SessionResumeTarget.App}>App</SegmentedControlItem>
            </SegmentedControl>
            {sessionResumeError ? <Toast tone="error" message={sessionResumeError} /> : null}
          </SettingsSection>
          <SettingsSection title="Missing session projects">
            <SegmentedControl
              className="settingsSessionResumeControl"
              value={missingSessionProjectPolicy}
              onValueChange={(value) => { void saveMissingSessionProjectPolicy(value); }}
              aria-label="Handle sessions whose project path no longer exists"
            >
              <SegmentedControlItem value={MissingSessionProjectPolicy.Show}>Show</SegmentedControlItem>
              <SegmentedControlItem value={MissingSessionProjectPolicy.Hide}>Hide</SegmentedControlItem>
              <SegmentedControlItem value={MissingSessionProjectPolicy.MergeByName}>Merge by name</SegmentedControlItem>
            </SegmentedControl>
            {missingSessionProjectError ? <Toast tone="error" message={missingSessionProjectError} /> : null}
          </SettingsSection>
          <SettingsSection title="Additional sessions">
              <textarea
                id="settings-additional-sessions"
                className="settingsTextarea"
                aria-label="Additional session directories"
                placeholder="~/path/to/sessions"
                spellCheck={false}
                value={additionalSessionRootsInput}
                onChange={(event) => {
                  inputRevisionRef.current.additionalSessionRoots += 1;
                  setAdditionalSessionRootsInput(event.target.value);
                  setSessionRootsError("");
                }}
                onBlur={() => saveAdditionalSessionRoots(additionalSessionRootsInput)}
                onKeyDown={(event) => {
                  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.blur();
                  if (event.key === "Escape") {
                    setAdditionalSessionRootsInput(additionalSessionRoots.join("\n"));
                    setSessionRootsError("");
                    event.currentTarget.blur();
                  }
                }}
              />
              {sessionRootsError ? <Toast tone="error" message={sessionRootsError} /> : null}
          </SettingsSection>
          <SettingsSection title="Scan scopes">
            <textarea
              id="settings-project-scan-scopes"
              className="settingsTextarea"
              aria-label="Project scan scopes"
              placeholder="~/dev\n!~/dev/**/archive"
              spellCheck={false}
              value={projectScanScopesInput}
                onChange={(event) => {
                  inputRevisionRef.current.projectScanScopes += 1;
                  setProjectScanScopesInput(event.target.value);
                setProjectScanScopesError("");
              }}
              onBlur={() => { void saveProjectScanScopes(projectScanScopesInput); }}
              onKeyDown={(event) => {
                if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.blur();
                if (event.key === "Escape") {
                  setProjectScanScopesError("");
                  event.currentTarget.blur();
                }
              }}
            />
            <div className="settingsProjectScanActions">
              <StatefulButton
                state={projectScanState}
                width={112}
                minWidth={112}
                aria-label={projectScanState === AsyncStatus.Loading ? "Scanning projects" : "Scan projects"}
                loadingContent={<LoadingIcon size={14} />}
                successContent="Scan now"
                errorContent="Scan now"
                onClick={() => { void scanProjects(); }}
              >
                Scan now
              </StatefulButton>
              {projects.length > 0 && projectScanState !== AsyncStatus.Error ? <ProjectsPopover projects={projects} /> : null}
              {projectScanSummary && projects.length === 0 && projectScanState !== AsyncStatus.Error ? <span className="settingsScanSummary" role="status">{projectScanSummary}</span> : null}
              {projectScanSummary && projectScanState === AsyncStatus.Error ? <Toast tone="error" message={projectScanSummary} /> : null}
            </div>
            {projectScanScopesError ? <Toast tone="error" message={projectScanScopesError} /> : null}
          </SettingsSection>
          </SettingsGroup>
          <SettingsGroup title="Developer">
            <SettingsSection title="Sync" className="settingsBackupSection">
              <BackupSettings onSkillsRestored={onSkillsUpdated} />
            </SettingsSection>
          <SettingsSection title="Coding helpers">
            <div className="settingsAgentRows">
              <div className="settingsAgentRow">
                <div className="settingsAgentStatus">
                  <strong>Tendi CLI</strong>
                  <div className="settingsAgentControls">
                    <div className="settingsAgentState">
                      {(appSettingsLoading || settingsLoading) && !cliStatus ? <span>Checking…</span> : cliHealthy ? (
                        <Badge className="settingsAgentInstalled" tone="success">
                          <CheckCircle2 size={14} aria-hidden="true" />
                          <span>Installed</span>
                        </Badge>
                      ) : cliConflict ? <span>Needs attention</span> : cliStatus?.supported === false ? <span>Unsupported</span> : cliStatus ? (
                        cliPathNeedsAttention || cliNeedsRepair ? <span>{cliPathNeedsAttention ? "Needs attention" : "Needs repair"}</span> : null
                      ) : <span>Status unavailable</span>}
                    </div>
                    <div className="settingsCliActions">
                      {cliHealthy ? (
                        <RowActionsMenu ariaLabel="Tendi CLI actions">
                          <DropdownMenu.Item
                            className="skillMenuItem danger"
                            disabled={Boolean(cliBusy)}
                            onSelect={requestRemoveCli}
                          >
                            <Trash2 size={14} aria-hidden="true" />
                            Uninstall
                          </DropdownMenu.Item>
                        </RowActionsMenu>
                      ) : cliStatus?.supported && !cliConflict ? (
                        <StatefulButton
                          size="sm"
                          className="settingsAgentAction"
                          state={cliBusy === CliAction.Install ? AsyncStatus.Loading : AsyncStatus.Idle}
                          width={112}
                          minWidth={112}
                          disabled={cliBusy !== null}
                          aria-label={cliBusy === CliAction.Install ? (cliNeedsRepair || cliPathNeedsAttention ? "Repairing CLI" : "Installing CLI") : cliNeedsRepair || cliPathNeedsAttention ? "Repair CLI" : "Install CLI"}
                          onClick={() => { void changeCliRegistration(); }}
                          loadingContent={<LoadingIcon size={14} />}
                        >
                          {cliNeedsRepair || cliPathNeedsAttention ? "Repair CLI" : "Install CLI"}
                        </StatefulButton>
                      ) : null}
                    </div>
                  </div>
                </div>
              </div>
              <div className="settingsAgentRow">
                <div className="settingsAgentStatus">
                  <strong>Tendi skill</strong>
                  <div className="settingsAgentControls">
                    <div className="settingsAgentState">
                      {(appSettingsLoading || settingsLoading) && !bundledSkillStatus ? <span>Checking…</span> : bundledSkillHealthy ? (
                        <Badge className="settingsAgentInstalled" tone="success">
                          <CheckCircle2 size={14} aria-hidden="true" />
                          <span>Installed</span>
                        </Badge>
                      ) : bundledSkillConflict ? <span>Needs attention</span> : bundledSkillStatus ? null : <span>Status unavailable</span>}
                    </div>
                    <div className="settingsCliActions">
                      <Button className="settingsAgentAction" size="sm" onClick={() => { setBundledSkillError(""); setBundledSkillInstallOpen(true); }}>
                        Install skill
                      </Button>
                    </div>
                  </div>
                </div>
              </div>
              {cliError ? <Toast tone="error" message={cliError} /> : null}
              {bundledSkillError ? <Toast tone="error" message={bundledSkillError} /> : null}
            </div>
          </SettingsSection>
          <SettingsSection title="Developer mode">
            <div className="settingsCheckboxRow">
              <Switch
                checked={developerMode}
                label="Enable developer mode"
                onCheckedChange={(checked) => { void saveDeveloperMode(checked); }}
              />
            </div>
            {developerModeError ? <Toast tone="error" message={developerModeError} /> : null}
          </SettingsSection>
          <SettingsSection title="Logs">
            <StatefulButton
              size="sm"
              state={logExportState}
              width={144}
              minWidth={144}
              onClick={() => { void exportLogs(); }}
              aria-label={logExportState === AsyncStatus.Loading ? logExportLabels.loading : logExportState === AsyncStatus.Success ? logExportLabels.success : logExportState === AsyncStatus.Error ? logExportLabels.retry : logExportLabels.idle}
              loadingContent={<LoadingIcon size={16} />}
              successContent={logExportLabels.success}
              errorContent={logExportLabels.error}
            >
              {logExportLabels.idle}
            </StatefulButton>
            {logExportError ? <Toast tone="error" message={logExportError} /> : null}
          </SettingsSection>
          </SettingsGroup>
        </div>
      </div>
    </section>
  );
}
