import { Tooltip } from "./Tooltip.tsx";
import { useEffect, useMemo, useRef, useState } from "react";
import { AlertCircle, Check, ChevronDown, Monitor, Moon, Play, RefreshCw, Sun } from "lucide-react";
import { DropdownMenu, ToggleGroup } from "radix-ui";

import { TauriCommand, safeInvoke } from "../../lib/index.ts";
import { normalizeAppearance, type Appearance } from "../../lib/appearance.ts";
import { ContentTopDragStrip } from "./ContentTopDragStrip.tsx";
import { PageHeader } from "./PageHeader.tsx";
import "./SettingsView.css";

type TerminalApp = {
  id: string;
  label: string;
  available?: boolean;
};

type AppSettings = {
  appearance: Appearance;
  terminal: string;
  additionalSessionRoots: string[];
  configProfiles: Record<string, string>;
};

type SettingsViewProps = {
  appearance: Appearance;
  onAppearanceChange: (appearance: Appearance) => void;
};

const appearanceOptions = [
  { value: "system", label: "Follow system", icon: Monitor },
  { value: "light", label: "Light", icon: Sun },
  { value: "dark", label: "Dark", icon: Moon },
] as const;

function normalizeSettings(settings: AppSettings): AppSettings {
  return {
    ...settings,
    appearance: normalizeAppearance(settings.appearance),
    additionalSessionRoots: settings.additionalSessionRoots ?? [],
    configProfiles: settings.configProfiles ?? {},
  };
}

export function SettingsView({ appearance, onAppearanceChange }: SettingsViewProps) {
  const [settings, setSettings] = useState<AppSettings>({ appearance, terminal: "auto", additionalSessionRoots: [], configProfiles: {} });
  const [terminalInput, setTerminalInput] = useState("auto");
  const [additionalSessionRootsInput, setAdditionalSessionRootsInput] = useState("");
  const [terminalApps, setTerminalApps] = useState<TerminalApp[]>([]);
  const [terminalMenuOpen, setTerminalMenuOpen] = useState(false);
  const [terminalError, setTerminalError] = useState("");
  const [sessionRootsError, setSessionRootsError] = useState("");
  const [appearanceError, setAppearanceError] = useState("");
  const [terminalTestState, setTerminalTestState] = useState<"idle" | "loading" | "success" | "error">("idle");
  const [updateState, setUpdateState] = useState<"idle" | "loading" | "success" | "error">("idle");
  const terminalTestRequestRef = useRef(0);
  const appearanceSaveRequestRef = useRef(0);
  const terminalOptions = useMemo(() => {
    const items = terminalApps.length ? terminalApps : [{ id: "auto", label: "Auto", available: true }];
    return items.map((app) => ({
      value: app.label,
      label: app.available ? app.label : `${app.label} (not found)`,
    }));
  }, [terminalApps]);

  useEffect(() => {
    let cancelled = false;
    Promise.all([
      safeInvoke(TauriCommand.SettingsGet),
      safeInvoke(TauriCommand.TerminalAppsList),
    ]).then(([nextSettings, apps]) => {
      if (cancelled) return;
      if (nextSettings) {
        const normalizedSettings = normalizeSettings(nextSettings as AppSettings);
        setSettings(normalizedSettings);
        onAppearanceChange(normalizedSettings.appearance);
        setTerminalInput(normalizedSettings.terminal);
        setAdditionalSessionRootsInput(normalizedSettings.additionalSessionRoots.join("\n"));
      }
      if (Array.isArray(apps)) setTerminalApps(apps);
    });
    return () => { cancelled = true; };
  }, [onAppearanceChange]);

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
      const savedSettings = normalizeSettings(nextSettings as AppSettings);
      setSettings(savedSettings);
      onAppearanceChange(savedSettings.appearance);
    } else {
      setSettings((current) => ({ ...current, appearance: previousAppearance }));
      onAppearanceChange(previousAppearance);
      setAppearanceError("Save failed");
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
      const savedSettings = nextSettings as AppSettings;
      setSettings(savedSettings);
      setTerminalInput(savedSettings.terminal);
      setAdditionalSessionRootsInput(savedSettings.additionalSessionRoots.join("\n"));
      setTerminalError("");
    } else {
      setTerminalError("Save failed");
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
      const savedSettings = nextSettings as AppSettings;
      setSettings(savedSettings);
      setTerminalInput(savedSettings.terminal);
      setAdditionalSessionRootsInput(savedSettings.additionalSessionRoots.join("\n"));
      setSessionRootsError("");
    } else {
      setSessionRootsError("Save failed: use absolute paths");
    }
  };

  const chooseTerminal = (terminal: string) => {
    setTerminalInput(terminal);
    setTerminalMenuOpen(false);
    setTerminalError("");
    terminalTestRequestRef.current += 1;
    setTerminalTestState("idle");
    saveTerminal(terminal);
  };

  const testTerminal = async () => {
    const terminal = terminalInput.trim() || "auto";
    const requestId = terminalTestRequestRef.current + 1;
    terminalTestRequestRef.current = requestId;
    setTerminalTestState("loading");
    const openedApp = await safeInvoke<string>(TauriCommand.TerminalAppTest, { terminal });
    if (terminalTestRequestRef.current !== requestId) return;
    setTerminalTestState(openedApp ? "success" : "error");
  };

  const checkForUpdates = async () => {
    setUpdateState("loading");
    const result = await safeInvoke<{ status?: string }>(TauriCommand.CheckForUpdates);
    setUpdateState(result?.status === "up-to-date" ? "success" : result ? "success" : "error");
  };

  return (
    <section className="content dataPage settingsPage">
      <ContentTopDragStrip />
      <PageHeader title="Settings" />
      <div className="settingsShell">
        <div className="settingsGroup">
          <section className="settingsSection">
            <div className="settingsSectionText">
              <h2>Appearance</h2>
            </div>
            <div className="settingsControlGroup">
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
            </div>
          </section>
          <section className="settingsSection">
            <div className="settingsSectionText">
              <h2>Terminal</h2>
            </div>
            <div className="settingsControlGroup">
              <div className="settingsTerminalRow">
                <div className="settingsTerminalInput">
                  <input
                    id="settings-terminal"
                    className="settingsSelect"
                    aria-label="Terminal application"
                    placeholder="Terminal application name"
                    value={terminalInput}
                    onChange={(event) => {
                      setTerminalInput(event.target.value);
                      setTerminalError("");
                      terminalTestRequestRef.current += 1;
                      setTerminalTestState("idle");
                    }}
                    onBlur={() => {
                      saveTerminal(terminalInput);
                    }}
                    onKeyDown={(event) => {
                      if (event.key === "Enter") event.currentTarget.blur();
                      if (event.key === "Escape") {
                        setTerminalInput(settings.terminal);
                        setTerminalError("");
                        terminalTestRequestRef.current += 1;
                        setTerminalTestState("idle");
                      }
                    }}
                  />
                  <DropdownMenu.Root open={terminalMenuOpen} onOpenChange={setTerminalMenuOpen}>
                    <DropdownMenu.Trigger asChild>
                      <button className="iconButton settingsTerminalMenuButton" aria-label="Choose terminal application">
                        <ChevronDown size={14} />
                      </button>
                    </DropdownMenu.Trigger>
                    <DropdownMenu.Portal>
                      <DropdownMenu.Content className="skillMenuContent settingsSelectContent" align="end" sideOffset={6}>
                        {terminalOptions.map((option) => (
                          <DropdownMenu.Item
                            className="skillMenuItem"
                            key={option.value}
                            onSelect={() => chooseTerminal(option.value)}
                          >
                            {option.label}
                          </DropdownMenu.Item>
                        ))}
                      </DropdownMenu.Content>
                    </DropdownMenu.Portal>
                  </DropdownMenu.Root>
                </div>
                <Tooltip content={terminalTestState === "loading"
                    ? "Opening terminal application"
                    : terminalTestState === "success"
                      ? "Terminal application opened"
                      : terminalTestState === "error"
                        ? "Could not open terminal application"
                        : "Test opening terminal application"}><button
                  className={`iconButton settingsTerminalTestButton${terminalTestState === "success" ? " isSuccess" : terminalTestState === "error" ? " isError" : ""}`}
                  aria-label={terminalTestState === "loading"
                    ? "Opening terminal application"
                    : terminalTestState === "success"
                      ? "Terminal application opened"
                      : terminalTestState === "error"
                        ? "Could not open terminal application"
                        : "Test terminal application"}
                  aria-busy={terminalTestState === "loading"}
                  disabled={terminalTestState === "loading" || !terminalInput.trim()}
                  onClick={testTerminal}
                >
                  {terminalTestState === "loading"
                    ? <RefreshCw className="loadingSpinner" size={14} />
                    : terminalTestState === "success"
                      ? <Check size={14} />
                      : terminalTestState === "error"
                        ? <AlertCircle size={14} />
                        : <Play size={14} />}
                </button></Tooltip>
              </div>
              {terminalError ? <span className="settingsError" role="alert">{terminalError}</span> : null}
            </div>
          </section>
          <section className="settingsSection">
            <div className="settingsSectionText">
              <h2>Additional sessions</h2>
            </div>
            <div className="settingsControlGroup">
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
            </div>
          </section>
          <section className="settingsSection">
            <div className="settingsSectionText">
              <h2>Updates</h2>
            </div>
            <div className="settingsControlGroup">
              <button
                className={`settingsUpdateButton${updateState === "success" ? " isSuccess" : updateState === "error" ? " isError" : ""}`}
                type="button"
                onClick={checkForUpdates}
                disabled={updateState === "loading"}
                aria-busy={updateState === "loading"}
              >
                {updateState === "loading" ? <RefreshCw className="loadingSpinner" size={14} /> : null}
                {updateState === "loading" ? "Checking for updates…" : updateState === "success" ? "You're up to date" : updateState === "error" ? "Check failed — try again" : "Check for Updates…"}
              </button>
            </div>
          </section>
        </div>
      </div>
    </section>
  );
}
