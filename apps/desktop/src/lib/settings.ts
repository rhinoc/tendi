import { normalizeAppearance, normalizeColorTheme, normalizeFontFamily, type Appearance, type ColorTheme, type FontFamily } from "./appearance.ts";
import { normalizeAppIcon, type AppIcon } from "./app-icon.ts";
import { normalizeMissingSessionProjectPolicy, type MissingSessionProjectPolicy } from "./projects.ts";
import { normalizeSessionResumeTarget, type SessionResumeTarget } from "./sessions.ts";

export type SettingsPayload = {
  appearance: Appearance;
  lightTheme: ColorTheme;
  darkTheme: ColorTheme;
  appIcon: AppIcon;
  fontFamily: FontFamily;
  terminal: string;
  editor: string;
  sessionResumeTarget: SessionResumeTarget;
  missingSessionProjectPolicy: MissingSessionProjectPolicy;
  developerMode: boolean;
  additionalSessionRoots: string[];
  configProfiles: Record<string, string>;
};

export type SettingsState = Omit<SettingsPayload, "configProfiles">;

export function normalizeConfigProfiles(value: unknown): Record<string, string> {
  if (!value || typeof value !== "object" || Array.isArray(value)) return {};
  return Object.fromEntries(
    Object.entries(value).filter((entry): entry is [string, string] => typeof entry[1] === "string"),
  );
}

export function normalizeSettings(settings: Partial<SettingsPayload>): SettingsState {
  return {
    appearance: normalizeAppearance(settings.appearance),
    lightTheme: normalizeColorTheme(settings.lightTheme),
    darkTheme: normalizeColorTheme(settings.darkTheme),
    appIcon: normalizeAppIcon(settings.appIcon),
    fontFamily: normalizeFontFamily(settings.fontFamily),
    terminal: typeof settings.terminal === "string" && settings.terminal.trim() ? settings.terminal : "auto",
    editor: typeof settings.editor === "string" && settings.editor.trim() ? settings.editor.trim() : "vscode",
    sessionResumeTarget: normalizeSessionResumeTarget(settings.sessionResumeTarget),
    missingSessionProjectPolicy: normalizeMissingSessionProjectPolicy(settings.missingSessionProjectPolicy),
    developerMode: settings.developerMode === true,
    additionalSessionRoots: Array.isArray(settings.additionalSessionRoots)
      ? settings.additionalSessionRoots.filter((root): root is string => typeof root === "string")
      : [],
  };
}
