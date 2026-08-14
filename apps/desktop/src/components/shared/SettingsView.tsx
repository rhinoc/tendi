import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { AlertCircle, Check, Info, Monitor, Moon, Sun } from "lucide-react";
import { ToggleGroup } from "radix-ui";

import { TauriCommand, safeInvoke, type BundledSkillInstallReport, type BundledSkillStatus, type CliInstallStatus } from "../../lib/index.ts";
import { normalizeAppearance, normalizeColorTheme, type Appearance, type ColorTheme, type ResolvedAppearance, type ThemePreferences } from "../../lib/appearance.ts";
import { ContentTopDragStrip } from "./ContentTopDragStrip.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import { LoadingInline } from "./LoadingInline.tsx";
import { PageHeader } from "./PageHeader.tsx";
import { SelectControl } from "./SelectControl.tsx";
import { SettingsApplicationPicker, type SettingsApplicationOption } from "./SettingsApplicationPicker.tsx";
import "./SettingsView.css";

type TerminalApp = {
  id: string;
  label: string;
  available?: boolean;
};

type AppSettings = {
  appearance: Appearance;
  lightTheme: ColorTheme;
  darkTheme: ColorTheme;
  terminal: string;
  editor: string;
  additionalSessionRoots: string[];
  configProfiles: Record<string, string>;
};

type SettingsViewProps = {
  appearance: Appearance;
  themePreferences: ThemePreferences;
  onAppearanceChange: (appearance: Appearance) => void;
  onThemeChange: (mode: ResolvedAppearance, theme: ColorTheme) => void;
};

const appearanceOptions = [
  { value: "system", label: "System", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
] as const;

const themeOptions = [
  { value: "gruvbox", label: "Gruvbox" },
  { value: "dracula", label: "Dracula" },
  { value: "nord", label: "Nord" },
  { value: "catppuccin", label: "Catppuccin" },
  { value: "tokyo-night", label: "Tokyo Night" },
] as const;

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
      />
    </div>
  );
}

function normalizeSettings(settings: AppSettings): AppSettings {
  return {
    ...settings,
    appearance: normalizeAppearance(settings.appearance),
    lightTheme: normalizeColorTheme(settings.lightTheme),
    darkTheme: normalizeColorTheme(settings.darkTheme),
    editor: settings.editor?.trim() || "vscode",
    additionalSessionRoots: settings.additionalSessionRoots ?? [],
    configProfiles: settings.configProfiles ?? {},
  };
}

function SettingsSection({ title, description, children }: { title: string; description?: ReactNode; children: ReactNode }) {
  return (
    <section className="settingsSection">
      <div className="settingsSectionText">
        <h2>{title}</h2>
        {description}
      </div>
      <div className="settingsControlGroup">{children}</div>
    </section>
  );
}

export function SettingsView({ appearance, themePreferences, onAppearanceChange, onThemeChange }: SettingsViewProps) {
  const [settings, setSettings] = useState<AppSettings>({ appearance, lightTheme: themePreferences.light, darkTheme: themePreferences.dark, terminal: "auto", editor: "vscode", additionalSessionRoots: [], configProfiles: {} });
  const [terminalInput, setTerminalInput] = useState("auto");
  const [editorInput, setEditorInput] = useState("vscode");
  const [additionalSessionRootsInput, setAdditionalSessionRootsInput] = useState("");
  const [terminalApps, setTerminalApps] = useState<TerminalApp[]>([]);
  const [terminalError, setTerminalError] = useState("");
  const [editorError, setEditorError] = useState("");
  const [sessionRootsError, setSessionRootsError] = useState("");
  const [appearanceError, setAppearanceError] = useState("");
  const [themeError, setThemeError] = useState("");
  const [cliStatus, setCliStatus] = useState<CliInstallStatus | null>(null);
  const [cliBusy, setCliBusy] = useState<"install" | "remove" | null>(null);
  const [cliError, setCliError] = useState("");
  const [bundledSkillStatus, setBundledSkillStatus] = useState<BundledSkillStatus | null>(null);
  const [bundledSkillBusy, setBundledSkillBusy] = useState(false);
  const [bundledSkillError, setBundledSkillError] = useState("");
  const [updateState, setUpdateState] = useState<"idle" | "loading" | "success" | "error">("idle");
  const appearanceSaveRequestRef = useRef(0);
  const themeSaveRequestRef = useRef(0);
  const terminalOptions: SettingsApplicationOption[] = useMemo(() => {
    const items = terminalApps.length ? terminalApps : [{ id: "auto", label: "Auto", available: true }];
    return items.map((app) => ({
      value: app.label,
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

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      safeInvoke(TauriCommand.SettingsGet),
      safeInvoke(TauriCommand.TerminalAppsList),
      safeInvoke<CliInstallStatus>(TauriCommand.CliStatus),
      safeInvoke<BundledSkillStatus>(TauriCommand.BundledSkillStatus),
    ]).then(([nextSettings, apps, nextCliStatus, nextBundledSkillStatus]) => {
      if (cancelled) return;
      if (nextSettings) {
        const normalizedSettings = normalizeSettings(nextSettings as AppSettings);
        setSettings(normalizedSettings);
        onAppearanceChange(normalizedSettings.appearance);
        onThemeChange("light", normalizedSettings.lightTheme);
        onThemeChange("dark", normalizedSettings.darkTheme);
        setTerminalInput(normalizedSettings.terminal);
        setEditorInput(normalizedSettings.editor);
        setAdditionalSessionRootsInput(normalizedSettings.additionalSessionRoots.join("\n"));
      }
      if (Array.isArray(apps)) setTerminalApps(apps);
      if (nextCliStatus) setCliStatus(nextCliStatus);
      if (nextBundledSkillStatus) setBundledSkillStatus(nextBundledSkillStatus);
    });
    return () => { cancelled = true; };
  }, [onAppearanceChange]);

  const applySavedSettings = (value: AppSettings) => {
    const savedSettings = normalizeSettings(value);
    setSettings(savedSettings);
    setTerminalInput(savedSettings.terminal);
    setEditorInput(savedSettings.editor);
    setAdditionalSessionRootsInput(savedSettings.additionalSessionRoots.join("\n"));
    return savedSettings;
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

  const testTerminal = async (terminal: string) => {
    return Boolean(await safeInvoke<string>(TauriCommand.TerminalAppTest, { terminal }));
  };

  const testEditor = async (editor: string) => {
    return Boolean(await safeInvoke<boolean>(TauriCommand.EditorAppTest, { editor }));
  };

  const checkForUpdates = async () => {
    setUpdateState("loading");
    const result = await safeInvoke<{ status?: string }>(TauriCommand.CheckForUpdates);
    setUpdateState(result?.status === "up-to-date" ? "success" : result ? "success" : "error");
  };

  const changeCliRegistration = async (action: "install" | "remove") => {
    if (cliBusy) return;
    setCliBusy(action);
    setCliError("");
    try {
      const nextStatus = await invoke<CliInstallStatus>(
        action === "install" ? TauriCommand.CliInstall : TauriCommand.CliRemove,
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

  const cliInstalled = cliStatus?.state === "installed";
  const cliNeedsRepair = cliStatus?.state === "stale";
  const cliConflict = cliStatus?.state === "conflict";
  const cliSupported = cliStatus?.supported ?? false;
  const cliHealthy = cliInstalled && cliStatus.pathConfigured;
  const bundledSkillHealthy = bundledSkillStatus?.current === true;
  const bundledSkillConflict = bundledSkillStatus?.installed === true && !bundledSkillHealthy;
  const canInstallBundledSkill = bundledSkillStatus !== null
    && !bundledSkillStatus.installed
    && !bundledSkillStatus.current
    && !cliConflict;
  const canRegisterCliSeparately = bundledSkillStatus !== null
    && !canInstallBundledSkill
    && !cliHealthy
    && cliSupported
    && !cliConflict;
  const setupReady = cliHealthy && bundledSkillHealthy;
  const setupConflict = cliConflict || bundledSkillConflict;

  const installBundledSkill = async () => {
    if (bundledSkillBusy || cliBusy) return;
    setBundledSkillBusy(true);
    setBundledSkillError("");
    try {
      let nextCliStatus = cliStatus;
      if (!cliHealthy && cliSupported) {
        nextCliStatus = await invoke<CliInstallStatus>(TauriCommand.CliInstall);
        setCliStatus(nextCliStatus);
        if (nextCliStatus.state !== "installed" || !nextCliStatus.pathConfigured) {
          throw new Error(nextCliStatus.detail || "The Tendi CLI is not available on PATH.");
        }
      }
      const report = await invoke<BundledSkillInstallReport>(TauriCommand.BundledSkillInstall);
      setBundledSkillStatus(report.status);
    } catch (error) {
      setBundledSkillError(`${error}`);
      const nextStatus = await safeInvoke<BundledSkillStatus>(TauriCommand.BundledSkillStatus);
      if (nextStatus) setBundledSkillStatus(nextStatus);
    } finally {
      setBundledSkillBusy(false);
    }
  };

  return (
    <section className="content dataPage settingsPage">
      <ContentTopDragStrip />
      <PageHeader title="Settings" />
      <div className="settingsShell">
        <div className="settingsGroup">
          <SettingsSection title="Appearance">
              <ToggleGroup.Root
                className="settingsAppearanceControl"
                type="single"
                value={appearance}
                onValueChange={(value) => {
                  if (value) saveAppearance(value as Appearance);
                }}
                aria-label="Application appearance"
              >
                {appearanceOptions.map((option) => {
                  const Icon = option.icon;
                  return (
                    <ToggleGroup.Item
                      className="settingsAppearanceItem"
                      value={option.value}
                      aria-label={option.label}
                      key={option.value}
                    >
                      <Icon size={14} />
                      <span>{option.label}</span>
                    </ToggleGroup.Item>
                  );
                })}
              </ToggleGroup.Root>
              {appearanceError ? <span className="settingsError" role="alert">{appearanceError}</span> : null}
          </SettingsSection>
          <SettingsSection title="Theme">
            <div className="settingsThemeControls">
              {themeModes.map((mode) => (
                <ThemeSelect
                  key={mode.value}
                  mode={mode}
                  value={settings[mode.key]}
                  onChange={(value) => { void saveTheme(mode.value, value); }}
                />
              ))}
            </div>
            {themeError ? <span className="settingsError" role="alert">{themeError}</span> : null}
          </SettingsSection>
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
          <SettingsSection title="Coding agents">
              <div className="settingsAgentRow">
                {!cliStatus || !bundledSkillStatus ? <LoadingIcon size={14} /> : setupReady ? <Check className="isSuccess" size={14} /> : setupConflict ? <AlertCircle className="isAttention" size={14} /> : <Info size={14} />}
                <div className="settingsAgentStatus">
                  <strong>{!cliStatus || !bundledSkillStatus ? "Checking…" : setupReady ? "Installed" : setupConflict ? "Needs attention" : "Not installed"}</strong>
                  <div className="settingsCliActions">
                    {canInstallBundledSkill ? (
                      <button
                        className="settingsUpdateButton"
                        type="button"
                      disabled={bundledSkillBusy || cliBusy !== null}
                      aria-busy={bundledSkillBusy}
                      aria-label={cliHealthy || !cliSupported ? "Install Tendi skill" : "Install CLI and Tendi skill"}
                      onClick={() => { void installBundledSkill(); }}
                    >
                      {bundledSkillBusy ? <LoadingInline label="Installing…" /> : "Install"}
                      </button>
                    ) : null}
                    {canRegisterCliSeparately ? (
                      <button
                        className="settingsUpdateButton"
                        type="button"
                        disabled={cliBusy !== null}
                        aria-busy={cliBusy === "install"}
                        onClick={() => { void changeCliRegistration("install"); }}
                      >
                        {cliBusy === "install" ? <LoadingInline label="Registering…" /> : cliNeedsRepair ? "Repair CLI" : "Install CLI"}
                      </button>
                    ) : null}
                    {cliInstalled || cliNeedsRepair ? (
                      <button
                        className="settingsUpdateButton"
                        type="button"
                        disabled={cliBusy !== null}
                        aria-busy={cliBusy === "remove"}
                        onClick={() => { void changeCliRegistration("remove"); }}
                      >
                        {cliBusy === "remove" ? <LoadingInline label="Removing…" /> : "Remove"}
                      </button>
                    ) : null}
                  </div>
                  {cliError ? <span className="settingsError" role="alert">{cliError}</span> : null}
                  {bundledSkillError ? <span className="settingsError" role="alert">{bundledSkillError}</span> : null}
                </div>
              </div>
          </SettingsSection>
          <SettingsSection title="Updates">
              <button
                className={`settingsUpdateButton${updateState === "success" ? " isSuccess" : updateState === "error" ? " isError" : ""}`}
                type="button"
                onClick={checkForUpdates}
                disabled={updateState === "loading"}
                aria-busy={updateState === "loading"}
              >
                {updateState === "loading" ? <LoadingInline label="Checking for updates…" /> : updateState === "success" ? "You're up to date" : updateState === "error" ? "Check failed — try again" : "Check for Updates…"}
              </button>
          </SettingsSection>
        </div>
      </div>
    </section>
  );
}
