import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { Monitor, Moon, Sun } from "lucide-react";
import { Popover } from "radix-ui";
import { compactDateTime, formatUserPath, remoteRepositoryLabel, TauriCommand, invokeCommand, normalizeMissingSessionProjectPolicy, normalizeSessionResumeTarget, safeInvoke, type BundledSkillInstallReport, type BundledSkillStatus, type CliInstallStatus, type DesktopUpdateState, type MissingSessionProjectPolicy, type ProjectSummary, type SessionResumeTarget } from "../../lib/index.ts";
import { normalizeAppearance, normalizeColorTheme, normalizeFontFamily, type Appearance, type ColorTheme, type FontFamily, type ResolvedAppearance, type ThemePreferences } from "../../lib/appearance.ts";
import { appIconOptions, appIconPreviewDataUrl, normalizeAppIcon, type AppIcon } from "../../lib/app-icon.ts";
import { Button } from "../../components/shared/Button.tsx";
import { ContentTopDragStrip } from "../../components/shared/ContentTopDragStrip.tsx";
import { CompactTable, type CompactTableColumn } from "../../components/shared/CompactTable.tsx";
import { LoadErrorState } from "../../components/shared/LoadErrorState.tsx";
import { LoadingIcon } from "../../components/shared/LoadingIcon.tsx";
import { PageHeader } from "../../components/shared/PageHeader.tsx";
import { SegmentedControl, SegmentedControlItem } from "../../components/shared/SegmentedControl.tsx";
import { SelectControl } from "../../components/shared/SelectControl.tsx";
import { StatefulButton } from "../../components/shared/StatefulButton.tsx";
import { Switch } from "../../components/shared/Switch.tsx";
import { SettingsApplicationPicker, type SettingsApplicationOption } from "./SettingsApplicationPicker.tsx";
import { codingAgentsAction, isCodingAgentsInstalled } from "./settings-agent-status.ts";
import "./SettingsView.css";

type TerminalApp = {
  id: string;
  label: string;
  available?: boolean;
};

type ProjectScanScope = {
  path: string;
  excluded?: boolean;
  enabled: boolean;
  lastScannedAt?: string | null;
  projectCount: number;
};

const projectTableColumns: CompactTableColumn<ProjectSummary>[] = [
  { key: "name", header: "Project", width: "160px", cellClassName: "compactTableCell--title", empty: "Project" },
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
        <Popover.Content className="settingsProjectsPopover" side="bottom" align="start" sideOffset={8}>
          <div className="settingsProjectsPopoverHeader">
            <strong>Scanned projects</strong>
            <span>{projects.length}</span>
          </div>
          <CompactTable
            className="settingsProjectsTable"
            ariaLabel="Scanned projects"
            rows={projects}
            columns={projectTableColumns}
            getRowId={(project) => project.id ?? project.rootPath ?? project.name ?? "project"}
            emptyState="No projects found"
          />
        </Popover.Content>
      </Popover.Portal>
    </Popover.Root>
  );
}

type AppSettings = {
  appearance: Appearance;
  lightTheme: ColorTheme;
  darkTheme: ColorTheme;
  appIcon: AppIcon;
  fontFamily: FontFamily;
  terminal: string;
  sessionResumeTarget: SessionResumeTarget;
  missingSessionProjectPolicy: MissingSessionProjectPolicy;
  editor: string;
  developerMode: boolean;
  additionalSessionRoots: string[];
  configProfiles: Record<string, string>;
};

type SettingsViewProps = {
  appearance: Appearance;
  themePreferences: ThemePreferences;
  fontFamily: FontFamily;
  developerMode: boolean;
  onAppearanceChange: (appearance: Appearance) => void;
  onThemeChange: (mode: ResolvedAppearance, theme: ColorTheme) => void;
  onFontFamilyChange: (fontFamily: FontFamily) => void;
  onDeveloperModeChange: (enabled: boolean) => void;
  sessionResumeTarget: SessionResumeTarget;
  missingSessionProjectPolicy: MissingSessionProjectPolicy;
  onSessionResumeTargetChange: (target: SessionResumeTarget) => void;
  onMissingSessionProjectPolicyChange: (policy: MissingSessionProjectPolicy) => void;
  appIcon: AppIcon;
  onAppIconChange: (appIcon: AppIcon) => void;
  projects: ProjectSummary[];
  onProjectsScanned?: (projects: ProjectSummary[]) => void;
  update: DesktopUpdateState;
  onCheckForUpdates: () => void;
  onInstallUpdate: () => void;
};

const appearanceOptions = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
] as const;

const themeOptions = [
  { value: "sakura-pop", label: "Sakura Pop" },
  { value: "gruvbox", label: "Gruvbox" },
  { value: "dracula", label: "Dracula" },
  { value: "nord", label: "Nord" },
  { value: "catppuccin", label: "Catppuccin" },
  { value: "tokyo-night", label: "Tokyo Night" },
  { value: "vercel", label: "Vercel" },
] as const;

const fontOptions = [
  { value: "geist", label: "Geist" },
  { value: "manrope", label: "Manrope" },
  { value: "inter", label: "Inter" },
  { value: "ibm-plex-sans", label: "IBM Plex Sans" },
  { value: "instrument-sans", label: "Instrument Sans" },
  { value: "plus-jakarta-sans", label: "Plus Jakarta Sans" },
  { value: "bricolage-grotesque", label: "Bricolage Grotesque" },
] as const satisfies ReadonlyArray<{ value: FontFamily; label: string }>;

const themePreviewColors: Record<ResolvedAppearance, Record<ColorTheme, { foreground: string; background: string }>> = {
  light: {
    "sakura-pop": { foreground: "#4b2347", background: "#fff4fb" },
    gruvbox: { foreground: "#282828", background: "#fbf1c7" },
    dracula: { foreground: "#282a36", background: "#f8f8f2" },
    nord: { foreground: "#2e3440", background: "#eceff4" },
    catppuccin: { foreground: "#4c4f69", background: "#eff1f5" },
    "tokyo-night": { foreground: "#3760bf", background: "#e1e2e7" },
    vercel: { foreground: "#171717", background: "#ffffff" },
  },
  dark: {
    "sakura-pop": { foreground: "#f8e8f5", background: "#211331" },
    gruvbox: { foreground: "#ebdbb2", background: "#282828" },
    dracula: { foreground: "#f8f8f2", background: "#282a36" },
    nord: { foreground: "#eceff4", background: "#2e3440" },
    catppuccin: { foreground: "#cdd6f4", background: "#1e1e2e" },
    "tokyo-night": { foreground: "#c0caf5", background: "#1a1b26" },
    vercel: { foreground: "#ededed", background: "#000000" },
  },
};

const themeModes = [
  { value: "light", label: "Light theme", key: "lightTheme" },
  { value: "dark", label: "Dark theme", key: "darkTheme" },
] as const satisfies ReadonlyArray<{ value: ResolvedAppearance; label: string; key: "lightTheme" | "darkTheme" }>;

type ThemeMode = (typeof themeModes)[number];

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

function normalizeSettings(settings: AppSettings): AppSettings {
  return {
    ...settings,
    appearance: normalizeAppearance(settings.appearance),
    lightTheme: normalizeColorTheme(settings.lightTheme),
    darkTheme: normalizeColorTheme(settings.darkTheme),
    appIcon: normalizeAppIcon(settings.appIcon),
    fontFamily: normalizeFontFamily(settings.fontFamily),
    editor: settings.editor?.trim() || "vscode",
    sessionResumeTarget: normalizeSessionResumeTarget(settings.sessionResumeTarget),
    missingSessionProjectPolicy: normalizeMissingSessionProjectPolicy(settings.missingSessionProjectPolicy),
    developerMode: settings.developerMode === true,
    additionalSessionRoots: settings.additionalSessionRoots ?? [],
    configProfiles: settings.configProfiles ?? {},
  };
}

function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return `${error}`;
}

async function readSetting<T>(command: TauriCommand): Promise<{ value?: T; error?: string }> {
  try {
    return { value: await invokeCommand<T>(command) };
  } catch (error) {
    return { error: errorMessage(error) };
  }
}

function SettingsSection({ title, children, className = "" }: { title: string; children: ReactNode; className?: string }) {
  return (
    <section className={`settingsSection ${className}`.trim()}>
      <div className="settingsSectionText">
        <h2>{title}</h2>
      </div>
      <div className="settingsControlGroup">{children}</div>
    </section>
  );
}

function SettingsGroup({ title, children }: { title: string; children: ReactNode }) {
  return (
    <section className="settingsGroup">
      <h2 className="settingsGroupTitle">{title}</h2>
      <div className="settingsGroupItems">{children}</div>
    </section>
  );
}

export function SettingsView({ appearance, themePreferences, fontFamily, developerMode, sessionResumeTarget, missingSessionProjectPolicy, appIcon, projects, onAppearanceChange, onThemeChange, onFontFamilyChange, onDeveloperModeChange, onSessionResumeTargetChange, onMissingSessionProjectPolicyChange, onAppIconChange, onProjectsScanned, update, onCheckForUpdates, onInstallUpdate }: SettingsViewProps) {
  const [settings, setSettings] = useState<AppSettings>({ appearance, lightTheme: themePreferences.light, darkTheme: themePreferences.dark, appIcon, fontFamily, terminal: "auto", sessionResumeTarget, missingSessionProjectPolicy, editor: "vscode", developerMode, additionalSessionRoots: [], configProfiles: {} });
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
  const [projectScanState, setProjectScanState] = useState<"idle" | "loading" | "success" | "error">("idle");
  const [projectScanSummary, setProjectScanSummary] = useState("");
  const [appearanceError, setAppearanceError] = useState("");
  const [themeError, setThemeError] = useState("");
  const [appIconError, setAppIconError] = useState("");
  const [fontFamilyError, setFontFamilyError] = useState("");
  const [developerModeError, setDeveloperModeError] = useState("");
  const [cliStatus, setCliStatus] = useState<CliInstallStatus | null>(null);
  const [cliBusy, setCliBusy] = useState<"install" | null>(null);
  const [cliError, setCliError] = useState("");
  const [bundledSkillStatus, setBundledSkillStatus] = useState<BundledSkillStatus | null>(null);
  const [bundledSkillBusy, setBundledSkillBusy] = useState(false);
  const [bundledSkillError, setBundledSkillError] = useState("");
  const [settingsLoading, setSettingsLoading] = useState(true);
  const [settingsLoadError, setSettingsLoadError] = useState("");
  const [logExportState, setLogExportState] = useState<"idle" | "loading" | "success" | "error">("idle");
  const [logExportError, setLogExportError] = useState("");
  const appearanceSaveRequestRef = useRef(0);
  const themeSaveRequestRef = useRef(0);
  const appIconSaveRequestRef = useRef(0);
  const fontFamilySaveRequestRef = useRef(0);
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
    setSettingsLoading(true);
    setSettingsLoadError("");
    const [nextSettings, apps, nextCliStatus, nextBundledSkillStatus, nextProjectScopes] = await Promise.all([
      readSetting<AppSettings>(TauriCommand.SettingsGet),
      readSetting<TerminalApp[]>(TauriCommand.TerminalAppsList),
      readSetting<CliInstallStatus>(TauriCommand.CliStatus),
      readSetting<BundledSkillStatus>(TauriCommand.BundledSkillStatus),
      readSetting<ProjectScanScope[]>(TauriCommand.ProjectScanScopesList),
    ]);
    const errors = [nextSettings, apps, nextCliStatus, nextBundledSkillStatus, nextProjectScopes]
      .map((result) => result.error)
      .filter((message): message is string => Boolean(message));
    if (nextSettings.value) {
      const normalizedSettings = normalizeSettings(nextSettings.value);
      setSettings(normalizedSettings);
      onAppearanceChange(normalizedSettings.appearance);
      onThemeChangeRef.current("light", normalizedSettings.lightTheme);
      onThemeChangeRef.current("dark", normalizedSettings.darkTheme);
      onAppIconChange(normalizedSettings.appIcon);
      onFontFamilyChange(normalizedSettings.fontFamily);
      onDeveloperModeChange(normalizedSettings.developerMode);
      onSessionResumeTargetChange(normalizedSettings.sessionResumeTarget);
      onMissingSessionProjectPolicyChange(normalizedSettings.missingSessionProjectPolicy);
      setTerminalInput(normalizedSettings.terminal);
      setEditorInput(normalizedSettings.editor);
      setAdditionalSessionRootsInput(normalizedSettings.additionalSessionRoots.join("\n"));
    }
    if (Array.isArray(apps.value)) setTerminalApps(apps.value);
    if (nextCliStatus.value) setCliStatus(nextCliStatus.value);
    if (nextBundledSkillStatus.value) setBundledSkillStatus(nextBundledSkillStatus.value);
    if (Array.isArray(nextProjectScopes.value)) {
      setProjectScanScopesInput(nextProjectScopes.value
        .filter((scope) => scope.enabled)
        .map((scope) => `${scope.excluded ? "!" : ""}${scope.path}`)
        .join("\n"));
    }
    setSettingsLoadError(errors.join("; "));
    setSettingsLoading(false);
  }, [onAppearanceChange, onAppIconChange, onDeveloperModeChange, onFontFamilyChange, onMissingSessionProjectPolicyChange, onSessionResumeTargetChange]);

  useEffect(() => {
    void loadSettings();
  }, [loadSettings]);

  const applySavedSettings = (value: AppSettings) => {
    const savedSettings = normalizeSettings(value);
    setSettings(savedSettings);
    setTerminalInput(savedSettings.terminal);
    setEditorInput(savedSettings.editor);
    setAdditionalSessionRootsInput(savedSettings.additionalSessionRoots.join("\n"));
    onSessionResumeTargetChange(savedSettings.sessionResumeTarget);
    onMissingSessionProjectPolicyChange(savedSettings.missingSessionProjectPolicy);
    onAppIconChange(savedSettings.appIcon);
    return savedSettings;
  };

  const saveAppIcon = async (nextAppIcon: AppIcon) => {
    const previousAppIcon = settings.appIcon;
    const requestId = appIconSaveRequestRef.current + 1;
    appIconSaveRequestRef.current = requestId;
    setAppIconError("");
    setSettings((current) => ({ ...current, appIcon: nextAppIcon }));
    onAppIconChange(nextAppIcon);
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      appIcon: nextAppIcon,
    });
    if (appIconSaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      const savedSettings = applySavedSettings(nextSettings as AppSettings);
      onAppIconChange(savedSettings.appIcon);
    } else {
      setSettings((current) => ({ ...current, appIcon: previousAppIcon }));
      onAppIconChange(previousAppIcon);
      setAppIconError("Save failed");
    }
  };

  const saveAppearance = async (nextAppearance: Appearance) => {
    const previousAppearance = settings.appearance;
    const requestId = appearanceSaveRequestRef.current + 1;
    appearanceSaveRequestRef.current = requestId;
    setAppearanceError("");
    setSettings((current) => ({ ...current, appearance: nextAppearance }));
    onAppearanceChange(nextAppearance);
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      appearance: nextAppearance,
    });
    if (appearanceSaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      const savedSettings = applySavedSettings(nextSettings as AppSettings);
      onAppearanceChange(savedSettings.appearance);
    } else {
      setSettings((current) => ({ ...current, appearance: previousAppearance }));
      onAppearanceChange(previousAppearance);
      setAppearanceError("Save failed");
    }
  };

  const saveTheme = async (mode: ResolvedAppearance, nextTheme: ColorTheme) => {
    const key = mode === "light" ? "lightTheme" : "darkTheme";
    const previousTheme = settings[key];
    const requestId = themeSaveRequestRef.current + 1;
    themeSaveRequestRef.current = requestId;
    setThemeError("");
    setSettings((current) => ({ ...current, [key]: nextTheme }));
    onThemeChange(mode, nextTheme);
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      [key]: nextTheme,
    });
    if (themeSaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      const savedSettings = applySavedSettings(nextSettings as AppSettings);
      onThemeChange("light", savedSettings.lightTheme);
      onThemeChange("dark", savedSettings.darkTheme);
    } else {
      setSettings((current) => ({ ...current, [key]: previousTheme }));
      onThemeChange(mode, previousTheme);
      setThemeError("Save failed");
    }
  };

  const saveFontFamily = async (nextFontFamily: FontFamily) => {
    const previousFontFamily = settings.fontFamily;
    const requestId = fontFamilySaveRequestRef.current + 1;
    fontFamilySaveRequestRef.current = requestId;
    setFontFamilyError("");
    setSettings((current) => ({ ...current, fontFamily: nextFontFamily }));
    onFontFamilyChange(nextFontFamily);
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      fontFamily: nextFontFamily,
    });
    if (fontFamilySaveRequestRef.current !== requestId) return;
    if (nextSettings) {
      const savedSettings = applySavedSettings(nextSettings as AppSettings);
      onFontFamilyChange(savedSettings.fontFamily);
    } else {
      setSettings((current) => ({ ...current, fontFamily: previousFontFamily }));
      onFontFamilyChange(previousFontFamily);
      setFontFamilyError("Save failed");
    }
  };

  const saveTerminal = async (terminal: string) => {
    const normalized = terminal.trim() || "auto";
    setTerminalInput(normalized);
    if (normalized === settings.terminal) return;
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      terminal: normalized,
    });
    if (nextSettings) {
      applySavedSettings(nextSettings as AppSettings);
      setTerminalError("");
    } else {
      setTerminalError("Save failed");
    }
  };

  const saveSessionResumeTarget = async (target: string) => {
    const normalized = normalizeSessionResumeTarget(target);
    if (normalized === settings.sessionResumeTarget) return;
    const previous = settings.sessionResumeTarget;
    setSettings((current) => ({ ...current, sessionResumeTarget: normalized }));
    onSessionResumeTargetChange(normalized);
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      sessionResumeTarget: normalized,
    });
    if (nextSettings) {
      applySavedSettings(nextSettings as AppSettings);
      setSessionResumeError("");
    } else {
      setSettings((current) => ({ ...current, sessionResumeTarget: previous }));
      onSessionResumeTargetChange(previous);
      setSessionResumeError("Save failed");
    }
  };

  const saveMissingSessionProjectPolicy = async (policy: string) => {
    const normalized = normalizeMissingSessionProjectPolicy(policy);
    if (normalized === settings.missingSessionProjectPolicy) return;
    const previous = settings.missingSessionProjectPolicy;
    setSettings((current) => ({ ...current, missingSessionProjectPolicy: normalized }));
    onMissingSessionProjectPolicyChange(normalized);
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      missingSessionProjectPolicy: normalized,
    });
    if (nextSettings) {
      applySavedSettings(nextSettings as AppSettings);
      setMissingSessionProjectError("");
    } else {
      setSettings((current) => ({ ...current, missingSessionProjectPolicy: previous }));
      onMissingSessionProjectPolicyChange(previous);
      setMissingSessionProjectError("Save failed");
    }
  };

  const saveEditor = async (editor: string) => {
    const normalized = editor.trim() || "vscode";
    setEditorInput(normalized);
    if (normalized === settings.editor) return;
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      editor: normalized,
    });
    if (nextSettings) {
      applySavedSettings(nextSettings as AppSettings);
      setEditorError("");
    } else {
      setEditorError("Save failed");
    }
  };

  const saveDeveloperMode = async (nextDeveloperMode: boolean) => {
    const previousDeveloperMode = settings.developerMode;
    if (nextDeveloperMode === previousDeveloperMode) return;
    setDeveloperModeError("");
    setSettings((current) => ({ ...current, developerMode: nextDeveloperMode }));
    onDeveloperModeChange(nextDeveloperMode);
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      developerMode: nextDeveloperMode,
    });
    if (nextSettings) {
      const savedSettings = applySavedSettings(nextSettings as AppSettings);
      onDeveloperModeChange(savedSettings.developerMode);
    } else {
      setSettings((current) => ({ ...current, developerMode: previousDeveloperMode }));
      onDeveloperModeChange(previousDeveloperMode);
      setDeveloperModeError("Save failed");
    }
  };

  const saveAdditionalSessionRoots = async (value: string) => {
    const roots = value
      .split("\n")
      .map((root) => root.trim())
      .filter(Boolean);
    setAdditionalSessionRootsInput(roots.join("\n"));
    if (roots.join("\n") === settings.additionalSessionRoots.join("\n")) return;
    const nextSettings = await safeInvoke(TauriCommand.SettingsSave, {
      ...settings,
      additionalSessionRoots: roots,
    });
    if (nextSettings) {
      applySavedSettings(nextSettings as AppSettings);
      setSessionRootsError("");
    } else {
      setSessionRootsError("Save failed: use absolute paths");
    }
  };

  const saveProjectScanScopes = async (value: string) => {
    const paths = value
      .split("\n")
      .map((path) => path.trim())
      .filter(Boolean);
    setProjectScanScopesInput(paths.join("\n"));
    const result = await safeInvoke<ProjectScanScope[]>(TauriCommand.ProjectScanScopesSave, { paths });
    if (result) {
      setProjectScanScopesError("");
      setProjectScanSummary(`${result.length} scan scope${result.length === 1 ? "" : "s"} saved`);
    } else {
      setProjectScanScopesError("Save failed: use absolute paths");
    }
  };

  const scanProjects = async () => {
    if (projectScanState === "loading") return;
    setProjectScanState("loading");
    setProjectScanSummary("");
    const result = await safeInvoke<{ projects?: unknown[] }>(TauriCommand.ProjectsScan);
    if (result) {
      if (Array.isArray(result.projects)) onProjectsScanned?.(result.projects as ProjectSummary[]);
      setProjectScanState("success");
      setProjectScanSummary(`${result.projects?.length ?? 0} projects found`);
    } else {
      setProjectScanState("error");
      setProjectScanSummary("Scan failed");
    }
  };

  const testTerminal = async (terminal: string) => {
    return Boolean(await safeInvoke<string>(TauriCommand.TerminalAppTest, { terminal }));
  };

  const testEditor = async (editor: string) => {
    return Boolean(await safeInvoke<boolean>(TauriCommand.EditorAppTest, { editor }));
  };

  const exportLogs = async () => {
    if (logExportState === "loading") return;
    setLogExportState("loading");
    setLogExportError("");
    try {
      const exportPath = await invokeCommand<string>(TauriCommand.LogsExport);
      await invokeCommand(TauriCommand.RevealInFinder, { path: exportPath });
      setLogExportState("success");
    } catch (error) {
      setLogExportState("error");
      setLogExportError(errorMessage(error));
    }
  };

  const changeCliRegistration = async () => {
    if (cliBusy) return;
    setCliBusy("install");
    setCliError("");
    try {
      const nextStatus = await invokeCommand<CliInstallStatus>(
        TauriCommand.CliInstall,
      );
      setCliStatus(nextStatus);
    } catch (error) {
      setCliError(`${error}`);
      const nextStatus = await safeInvoke<CliInstallStatus>(TauriCommand.CliStatus);
      if (nextStatus) setCliStatus(nextStatus);
    } finally {
      setCliBusy(null);
    }
  };

  const removeCodingAgents = async () => {
    if (bundledSkillBusy || cliBusy) return;
    setBundledSkillBusy(true);
    setBundledSkillError("");
    setCliError("");
    try {
      const nextSkillStatus = await invokeCommand<BundledSkillStatus>(TauriCommand.BundledSkillRemove);
      setBundledSkillStatus(nextSkillStatus);
      if (cliStatus?.supported && cliStatus.state === "installed") {
        const nextCliStatus = await invokeCommand<CliInstallStatus>(TauriCommand.CliRemove);
        setCliStatus(nextCliStatus);
      }
    } catch (error) {
      setBundledSkillError(`${error}`);
      const [nextSkillStatus, nextCliStatus] = await Promise.all([
        safeInvoke<BundledSkillStatus>(TauriCommand.BundledSkillStatus),
        safeInvoke<CliInstallStatus>(TauriCommand.CliStatus),
      ]);
      if (nextSkillStatus) setBundledSkillStatus(nextSkillStatus);
      if (nextCliStatus) setCliStatus(nextCliStatus);
    } finally {
      setBundledSkillBusy(false);
    }
  };

  const cliInstalled = cliStatus?.state === "installed";
  const cliNeedsRepair = cliStatus?.state === "stale";
  const cliConflict = cliStatus?.state === "conflict";
  const cliPathNeedsAttention = cliInstalled && !cliStatus.pathConfigured;
  const cliHealthy = cliInstalled && cliStatus.pathConfigured;
  const bundledSkillHealthy = bundledSkillStatus?.current === true;
  const bundledSkillConflict = bundledSkillStatus?.installed === true && !bundledSkillHealthy;
  const codingAgentsActionValue = codingAgentsAction(cliStatus, bundledSkillStatus);
  const bundledSkillNeedsRepair = bundledSkillStatus?.installed === true && !bundledSkillStatus.current;
  const canInstallBundledSkill = (codingAgentsActionValue === "install" || codingAgentsActionValue === "repair")
    && bundledSkillStatus?.current !== true
    && !cliConflict;
  const canRegisterCliSeparately = (codingAgentsActionValue === "install" || codingAgentsActionValue === "repair")
    && !canInstallBundledSkill;
  const setupReady = isCodingAgentsInstalled(cliStatus, bundledSkillStatus);
  const setupNeedsAttention = cliConflict || cliPathNeedsAttention || bundledSkillConflict;
  const setupStatusLabel = settingsLoading
    ? "Checking…"
    : settingsLoadError && (!cliStatus || !bundledSkillStatus)
      ? "Unable to load status"
      : !cliStatus || !bundledSkillStatus
        ? "Status unavailable"
        : setupReady
          ? "Installed"
          : setupNeedsAttention
            ? "Needs attention"
            : "";

  const installBundledSkill = async () => {
    if (bundledSkillBusy || cliBusy) return;
    setBundledSkillBusy(true);
    setBundledSkillError("");
    try {
      let nextCliStatus = cliStatus;
      if (!cliHealthy && cliStatus?.supported) {
        nextCliStatus = await invokeCommand<CliInstallStatus>(TauriCommand.CliInstall);
        setCliStatus(nextCliStatus);
        if (nextCliStatus.state !== "installed" || !nextCliStatus.pathConfigured) {
          throw new Error(nextCliStatus.detail || "The Tendi CLI is not available on PATH.");
        }
      }
      const report = await invokeCommand<BundledSkillInstallReport>(
        TauriCommand.BundledSkillInstall,
        bundledSkillNeedsRepair ? { overwrite: true } : undefined,
      );
      setBundledSkillStatus(report.status);
    } catch (error) {
      setBundledSkillError(`${error}`);
      const [nextSkillStatus, nextCliStatus] = await Promise.all([
        safeInvoke<BundledSkillStatus>(TauriCommand.BundledSkillStatus),
        safeInvoke<CliInstallStatus>(TauriCommand.CliStatus),
      ]);
      if (nextSkillStatus) setBundledSkillStatus(nextSkillStatus);
      if (nextCliStatus) setCliStatus(nextCliStatus);
    } finally {
      setBundledSkillBusy(false);
    }
  };

  return (
    <section className="content dataPage settingsPage">
      <ContentTopDragStrip />
      <PageHeader title="Settings">
        <StatefulButton
          size="sm"
          state={update.status === "checking" || update.status === "installing" ? "loading" : update.status === "up-to-date" ? "success" : update.status === "error" ? "error" : "idle"}
          width={160}
          minWidth={160}
          onClick={update.status === "available" ? onInstallUpdate : onCheckForUpdates}
          aria-label={update.status === "checking" ? "Checking for updates" : update.status === "installing" ? "Installing update" : update.status === "available" && update.version ? `Install update ${update.version}` : update.status === "up-to-date" ? "You're up to date" : update.status === "error" ? "Check for updates again" : "Check for updates"}
          loadingContent={<LoadingIcon size={16} />}
          successContent="You're up to date"
          errorContent="Check failed — try again"
        >
          {update.status === "available" && update.version ? `Install ${update.version}` : "Check for Updates"}
        </StatefulButton>
      </PageHeader>
      {settingsLoadError ? <LoadErrorState message={settingsLoadError} onRetry={() => { void loadSettings(); }} /> : null}
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
              {appearanceError ? <span className="settingsError" role="alert">{appearanceError}</span> : null}
          </SettingsSection>
          <SettingsSection title="Theme" className="settingsThemeSection">
            <div className="settingsThemeControls">
              {themeModes.map((mode) => (
                <ThemeSelect
                  key={mode.value}
                  mode={mode}
                  value={settings[mode.key]}
                  onChange={(value) => { void saveTheme(mode.value, value); }}
                />
              ))}
              <div className="settingsThemePicker">
                <span className="settingsThemePickerLabel">App icon</span>
                <AppIconSelect value={settings.appIcon} onChange={(value) => { void saveAppIcon(value); }} />
                {appIconError ? <span className="settingsError" role="alert">{appIconError}</span> : null}
              </div>
            </div>
            {themeError ? <span className="settingsError" role="alert">{themeError}</span> : null}
          </SettingsSection>
          <SettingsSection title="Font">
            <SelectControl
              className="settingsFontSelect"
              contentClassName="settingsSelectContent"
              label="Application font"
              value={settings.fontFamily}
              onValueChange={(value) => { void saveFontFamily(value as FontFamily); }}
              options={[...fontOptions]}
              showOptionTooltip={false}
              renderOption={(option) => <span style={{ fontFamily: `"${option.label}"` }}>{option.label}</span>}
            />
            {fontFamilyError ? <span className="settingsError" role="alert">{fontFamilyError}</span> : null}
          </SettingsSection>
          </SettingsGroup>
          <SettingsGroup title="Workspace">
          <SettingsSection title="Terminal">
            <SettingsApplicationPicker
              id="settings-terminal"
              ariaLabel="Terminal application"
              menuAriaLabel="Choose terminal application"
              placeholder="Terminal application name"
              value={terminalInput}
              savedValue={settings.terminal}
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
                setTerminalError("");
              }}
              onSave={saveTerminal}
              onCancel={() => setTerminalError("")}
              onTest={testTerminal}
            />
          </SettingsSection>
          <SettingsSection title="Session resume">
            <SegmentedControl
              className="settingsSessionResumeControl"
              value={settings.sessionResumeTarget}
              onValueChange={(value) => { void saveSessionResumeTarget(value); }}
              aria-label="Prefer opening resumed sessions in"
            >
              <SegmentedControlItem value="auto">Auto</SegmentedControlItem>
              <SegmentedControlItem value="terminal">Terminal</SegmentedControlItem>
              <SegmentedControlItem value="app">App</SegmentedControlItem>
            </SegmentedControl>
            {sessionResumeError ? <span className="settingsError" role="alert">{sessionResumeError}</span> : null}
          </SettingsSection>
          <SettingsSection title="Missing session projects">
            <SegmentedControl
              className="settingsSessionResumeControl"
              value={settings.missingSessionProjectPolicy}
              onValueChange={(value) => { void saveMissingSessionProjectPolicy(value); }}
              aria-label="Handle sessions whose project path no longer exists"
            >
              <SegmentedControlItem value="show">Show</SegmentedControlItem>
              <SegmentedControlItem value="hide">Hide</SegmentedControlItem>
              <SegmentedControlItem value="merge-by-name">Merge by name</SegmentedControlItem>
            </SegmentedControl>
            {missingSessionProjectError ? <span className="settingsError" role="alert">{missingSessionProjectError}</span> : null}
          </SettingsSection>
          <SettingsSection title="Editor">
            <SettingsApplicationPicker
              id="settings-editor"
              ariaLabel="Editor application"
              menuAriaLabel="Choose editor application"
              placeholder="Editor command"
              value={editorInput}
              savedValue={settings.editor}
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
                setEditorError("");
              }}
              onSave={saveEditor}
              onCancel={() => setEditorError("")}
              onTest={testEditor}
            />
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
                  setAdditionalSessionRootsInput(event.target.value);
                  setSessionRootsError("");
                }}
                onBlur={() => saveAdditionalSessionRoots(additionalSessionRootsInput)}
                onKeyDown={(event) => {
                  if ((event.metaKey || event.ctrlKey) && event.key === "Enter") event.currentTarget.blur();
                  if (event.key === "Escape") {
                    setAdditionalSessionRootsInput(settings.additionalSessionRoots.join("\n"));
                    setSessionRootsError("");
                    event.currentTarget.blur();
                  }
                }}
              />
              {sessionRootsError ? <span className="settingsError" role="alert">{sessionRootsError}</span> : null}
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
                aria-label={projectScanState === "loading" ? "Scanning projects" : "Scan projects"}
                loadingContent={<LoadingIcon size={14} />}
                successContent="Scan now"
                errorContent="Scan now"
                onClick={() => { void scanProjects(); }}
              >
                Scan now
              </StatefulButton>
              {projects.length > 0 && projectScanState !== "error" ? <ProjectsPopover projects={projects} /> : null}
              {projectScanSummary && (projects.length === 0 || projectScanState === "error") ? <span className="settingsScanSummary" role={projectScanState === "error" ? "alert" : "status"}>{projectScanSummary}</span> : null}
            </div>
            {projectScanScopesError ? <span className="settingsError" role="alert">{projectScanScopesError}</span> : null}
          </SettingsSection>
          </SettingsGroup>
          <SettingsGroup title="Developer">
          <SettingsSection title="Coding agents">
              <div className="settingsAgentRow">
                <div className="settingsAgentStatus">
                  {setupStatusLabel ? <strong>{setupStatusLabel}</strong> : null}
                  <div className="settingsCliActions">
                    {canInstallBundledSkill ? (
                      <StatefulButton
                        size="sm"
                        state={bundledSkillBusy ? "loading" : "idle"}
                        width={112}
                        minWidth={112}
                        disabled={cliBusy !== null}
                        aria-label={bundledSkillNeedsRepair ? "Repair" : "Install"}
                        onClick={() => { void installBundledSkill(); }}
                        loadingContent={<LoadingIcon size={14} />}
                      >
                        {bundledSkillNeedsRepair ? "Repair" : "Install"}
                      </StatefulButton>
                    ) : null}
                    {canRegisterCliSeparately ? (
                      <StatefulButton
                        size="sm"
                        state={cliBusy === "install" ? "loading" : "idle"}
                        width={112}
                        minWidth={112}
                        disabled={cliBusy !== null}
                        aria-label={cliBusy === "install" ? (cliNeedsRepair ? "Repairing" : "Installing") : cliNeedsRepair ? "Repair" : "Install"}
                        onClick={() => { void changeCliRegistration(); }}
                        loadingContent={<LoadingIcon size={14} />}
                      >
                        {cliNeedsRepair ? "Repair" : "Install"}
                      </StatefulButton>
                    ) : null}
                    {codingAgentsActionValue === "remove" ? (
                      <StatefulButton
                        size="sm"
                        state={bundledSkillBusy ? "loading" : "idle"}
                        width={112}
                        minWidth={112}
                        disabled={cliBusy !== null}
                        aria-label={bundledSkillBusy ? "Removing" : "Remove"}
                        onClick={() => { void removeCodingAgents(); }}
                        loadingContent={<LoadingIcon size={14} />}
                      >
                        Remove
                      </StatefulButton>
                    ) : null}
                  </div>
                  {cliError ? <span className="settingsError" role="alert">{cliError}</span> : null}
                  {bundledSkillError ? <span className="settingsError" role="alert">{bundledSkillError}</span> : null}
                </div>
              </div>
          </SettingsSection>
          <SettingsSection title="Developer mode">
            <div className="settingsCheckboxRow">
              <Switch
                checked={settings.developerMode}
                label="Enable developer mode"
                onCheckedChange={(checked) => { void saveDeveloperMode(checked); }}
              />
            </div>
            {developerModeError ? <span className="settingsError" role="alert">{developerModeError}</span> : null}
          </SettingsSection>
          <SettingsSection title="Logs">
            <StatefulButton
              size="sm"
              state={logExportState}
              width={144}
              minWidth={144}
              onClick={() => { void exportLogs(); }}
              aria-label={logExportState === "loading" ? "Exporting logs" : logExportState === "success" ? "Logs exported" : logExportState === "error" ? "Export logs again" : "Export logs"}
              loadingContent={<LoadingIcon size={16} />}
              successContent="Exported"
              errorContent="Export failed"
            >
              Export Logs
            </StatefulButton>
            {logExportError ? <span className="settingsError" role="alert">{logExportError}</span> : null}
          </SettingsSection>
          </SettingsGroup>
        </div>
      </div>
    </section>
  );
}
