import { getCurrentWindow } from "@tauri-apps/api/window";
import baseIconSvg from "../../src-tauri/icons/tendi-icon.svg?raw";
import { normalizeColorTheme, type ColorTheme } from "./appearance.ts";
import { logger } from "./logger.ts";
import { invokeCommand, isTauriRuntime, TauriCommand } from "./tauri.ts";

export type AppIcon = ColorTheme;

export const appIconOptions = [
  { value: "sakura-pop", label: "Sakura Pop" },
  { value: "gruvbox", label: "Gruvbox" },
  { value: "dracula", label: "Dracula" },
  { value: "nord", label: "Nord" },
  { value: "catppuccin", label: "Catppuccin" },
  { value: "tokyo-night", label: "Tokyo Night" },
  { value: "vercel", label: "Vercel" },
] as const satisfies ReadonlyArray<{ value: AppIcon; label: string }>;

const APP_ICON_CACHE_KEY = "tendi.app-icon";

type IconPalette = {
  tileTop: string;
  tileMiddle: string;
  tileBottom: string;
  tileAccent: string;
  rimLight: string;
  rimDark: string;
  silverWarm: string;
  silverLight: string;
  silverMid: string;
  silverDark: string;
  foldLight: string;
  foldMid: string;
  foldDark: string;
  edge: string;
  flare: string;
};

const iconPalettes: Record<AppIcon, IconPalette> = {
  "sakura-pop": {
    tileTop: "#46247b", tileMiddle: "#1a0b3b", tileBottom: "#09051c", tileAccent: "#ff2d92",
    rimLight: "#ff71ce", rimDark: "#b967ff", silverWarm: "#fff0fb", silverLight: "#ffd7f1",
    silverMid: "#ffb5dd", silverDark: "#9b4a91", foldLight: "#ff71ce", foldMid: "#ff2d92",
    foldDark: "#c51b70", edge: "#fff0fb", flare: "#fffb96",
  },
  gruvbox: {
    tileTop: "#3c3836", tileMiddle: "#282828", tileBottom: "#1d2021", tileAccent: "#d65d0e",
    rimLight: "#a89984", rimDark: "#504945", silverWarm: "#fbf1c7", silverLight: "#ebdbb2",
    silverMid: "#d5c4a1", silverDark: "#7c6f64", foldLight: "#fe8019", foldMid: "#d65d0e",
    foldDark: "#af3a03", edge: "#f9f5d7", flare: "#fabd2f",
  },
  dracula: {
    tileTop: "#44475a", tileMiddle: "#282a36", tileBottom: "#191a21", tileAccent: "#ff79c6",
    rimLight: "#6272a4", rimDark: "#44475a", silverWarm: "#f8f8f2", silverLight: "#e6e6e6",
    silverMid: "#c8c8d0", silverDark: "#6272a4", foldLight: "#ff79c6", foldMid: "#bd93f9",
    foldDark: "#8f62bf", edge: "#f8f8f2", flare: "#f1fa8c",
  },
  nord: {
    tileTop: "#4c566a", tileMiddle: "#2e3440", tileBottom: "#242933", tileAccent: "#88c0d0",
    rimLight: "#d8dee9", rimDark: "#4c566a", silverWarm: "#eceff4", silverLight: "#e5e9f0",
    silverMid: "#d8dee9", silverDark: "#81a1c1", foldLight: "#8fbcbb", foldMid: "#88c0d0",
    foldDark: "#5e81ac", edge: "#eceff4", flare: "#ebcb8b",
  },
  catppuccin: {
    tileTop: "#313244", tileMiddle: "#1e1e2e", tileBottom: "#11111b", tileAccent: "#cba6f7",
    rimLight: "#6c7086", rimDark: "#45475a", silverWarm: "#cdd6f4", silverLight: "#bac2de",
    silverMid: "#a6adc8", silverDark: "#6c7086", foldLight: "#f5c2e7", foldMid: "#cba6f7",
    foldDark: "#8839ef", edge: "#f5e0e6", flare: "#f9e2af",
  },
  "tokyo-night": {
    tileTop: "#24283b", tileMiddle: "#1a1b26", tileBottom: "#16161e", tileAccent: "#7aa2f7",
    rimLight: "#565f89", rimDark: "#3b4261", silverWarm: "#c0caf5", silverLight: "#a9b1d6",
    silverMid: "#9aa5ce", silverDark: "#565f89", foldLight: "#7dcfff", foldMid: "#bb9af7",
    foldDark: "#7e57c2", edge: "#c0caf5", flare: "#e0af68",
  },
  vercel: {
    tileTop: "#262626", tileMiddle: "#000000", tileBottom: "#000000", tileAccent: "#50a8ff",
    rimLight: "#a3a3a3", rimDark: "#525252", silverWarm: "#ffffff", silverLight: "#ededed",
    silverMid: "#d4d4d4", silverDark: "#737373", foldLight: "#ffffff", foldMid: "#ededed",
    foldDark: "#a3a3a3", edge: "#ffffff", flare: "#50a8ff",
  },
};

let latestApplyRequest = 0;

export function normalizeAppIcon(value: unknown): AppIcon {
  return normalizeColorTheme(value);
}

export function readCachedAppIcon(): AppIcon {
  try {
    return normalizeAppIcon(window.localStorage.getItem(APP_ICON_CACHE_KEY));
  } catch {
    return "sakura-pop";
  }
}

export function appIconSvg(value: unknown, options: { compact?: boolean } = {}): string {
  const palette = iconPalettes[normalizeAppIcon(value)];
  const replacements: Record<string, string> = {
    "#3C3836": palette.tileTop,
    "#282828": palette.tileMiddle,
    "#1D2021": palette.tileBottom,
    "#D65D0E": palette.tileAccent,
    "#A89984": palette.rimLight,
    "#504945": palette.rimDark,
    "#FBF1C7": palette.silverWarm,
    "#EBDBB2": palette.silverLight,
    "#D5C4A1": palette.silverMid,
    "#7C6F64": palette.silverDark,
    "#FE8019": palette.foldLight,
    "#AF3A03": palette.foldDark,
    "#F9F5D7": palette.edge,
    "#FABD2F": palette.flare,
  };
  const svg = Object.entries(replacements).reduce((current, [source, target]) => current.replaceAll(source, target), baseIconSvg);
  if (!options.compact) return svg;
  return svg
    .replace(' filter="url(#tile-shadow)"', "")
    .replace(' filter="url(#mark-shadow)"', "")
    .replace(' filter="url(#flare-blur)"', "");
}

export function appIconDataUrl(value: unknown, options?: { compact?: boolean }): string {
  return `data:image/svg+xml;charset=utf-8,${encodeURIComponent(appIconSvg(value, options))}`;
}

export function appIconPreviewDataUrl(value: unknown): string {
  return appIconDataUrl(value, { compact: true });
}

export async function applyAppIcon(value: unknown): Promise<void> {
  const appIcon = normalizeAppIcon(value);
  try {
    window.localStorage.setItem(APP_ICON_CACHE_KEY, appIcon);
  } catch {
    // The persisted app setting remains the source of truth if browser storage is unavailable.
  }
  if (!isTauriRuntime()) return;

  const requestId = ++latestApplyRequest;
  try {
    const svg = appIconSvg(appIcon);
    if (requestId !== latestApplyRequest) return;
    try {
      await invokeCommand(TauriCommand.AppIconSet, { icon: svg });
    } catch (error) {
      logger.warn("native app icon update failed", { appIcon, error });
    }
    try {
      await getCurrentWindow().setIcon(appIconDataUrl(appIcon));
    } catch (error) {
      logger.warn("window icon update failed", { appIcon, error });
    }
  } catch (error) {
    logger.warn("app icon update failed", { appIcon, error });
  }
}
