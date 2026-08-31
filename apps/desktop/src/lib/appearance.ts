export enum Appearance {
  System = "system",
  Light = "light",
  Dark = "dark",
}

export enum ColorTheme {
  SakuraPop = "sakura-pop",
  Gruvbox = "gruvbox",
  Dracula = "dracula",
  Nord = "nord",
  Catppuccin = "catppuccin",
  TokyoNight = "tokyo-night",
  Vercel = "vercel",
}

export enum FontFamily {
  Geist = "geist",
  Manrope = "manrope",
  Inter = "inter",
  IbmPlexSans = "ibm-plex-sans",
  InstrumentSans = "instrument-sans",
  PlusJakartaSans = "plus-jakarta-sans",
  BricolageGrotesque = "bricolage-grotesque",
}

export const appearances = [Appearance.System, Appearance.Light, Appearance.Dark] as const;
export const colorThemes = [ColorTheme.SakuraPop, ColorTheme.Gruvbox, ColorTheme.Dracula, ColorTheme.Nord, ColorTheme.Catppuccin, ColorTheme.TokyoNight, ColorTheme.Vercel] as const;
export const fontFamilies = [
  FontFamily.Geist,
  FontFamily.Manrope,
  FontFamily.Inter,
  FontFamily.IbmPlexSans,
  FontFamily.InstrumentSans,
  FontFamily.PlusJakartaSans,
  FontFamily.BricolageGrotesque,
] as const;

export type ResolvedAppearance = Exclude<Appearance, Appearance.System>;
export type ThemePreferences = {
  light: ColorTheme;
  dark: ColorTheme;
};

const APPEARANCE_CACHE_KEY = "tendi.appearance";
const THEME_CACHE_KEY = "tendi.color-themes";
const FONT_FAMILY_CACHE_KEY = "tendi.font-family";
const SYSTEM_DARK_QUERY = "(prefers-color-scheme: dark)";
let themeTransitionFrame: number | null = null;

function suppressThemeTransitions(root: HTMLElement): void {
  root.dataset.themeChanging = "true";
  if (themeTransitionFrame !== null) window.cancelAnimationFrame(themeTransitionFrame);
  themeTransitionFrame = window.requestAnimationFrame(() => {
    themeTransitionFrame = window.requestAnimationFrame(() => {
      delete root.dataset.themeChanging;
      themeTransitionFrame = null;
    });
  });
}

export function normalizeAppearance(value: unknown): Appearance {
  return appearances.includes(value as Appearance) ? value as Appearance : Appearance.System;
}

export function normalizeColorTheme(value: unknown): ColorTheme {
  return colorThemes.includes(value as ColorTheme) ? value as ColorTheme : ColorTheme.Vercel;
}

export function normalizeFontFamily(value: unknown): FontFamily {
  return fontFamilies.includes(value as FontFamily) ? value as FontFamily : FontFamily.Manrope;
}

export function normalizeThemePreferences(value: Partial<ThemePreferences> | null | undefined): ThemePreferences {
  return {
    light: normalizeColorTheme(value?.light),
    dark: normalizeColorTheme(value?.dark),
  };
}

export function readCachedAppearance(): Appearance {
  try {
    return normalizeAppearance(window.localStorage.getItem(APPEARANCE_CACHE_KEY));
  } catch {
    return Appearance.System;
  }
}

export function readCachedThemePreferences(): ThemePreferences {
  try {
    const value = JSON.parse(window.localStorage.getItem(THEME_CACHE_KEY) ?? "null") as Partial<ThemePreferences> | null;
    return normalizeThemePreferences(value);
  } catch {
    return normalizeThemePreferences(null);
  }
}

export function readCachedFontFamily(): FontFamily {
  try {
    return normalizeFontFamily(window.localStorage.getItem(FONT_FAMILY_CACHE_KEY));
  } catch {
    return FontFamily.Manrope;
  }
}

export function applyFontFamily(fontFamily: FontFamily): void {
  const normalized = normalizeFontFamily(fontFamily);
  document.documentElement.dataset.fontFamily = normalized;
  try {
    window.localStorage.setItem(FONT_FAMILY_CACHE_KEY, normalized);
  } catch {
    // The persisted app setting remains the source of truth if browser storage is unavailable.
  }
}

export function resolveAppearance(appearance: Appearance): ResolvedAppearance {
  if (appearance !== Appearance.System) return appearance;
  return window.matchMedia(SYSTEM_DARK_QUERY).matches ? Appearance.Dark : Appearance.Light;
}

export function resolveColorTheme(appearance: Appearance, preferences: ThemePreferences): ColorTheme {
  return preferences[resolveAppearance(appearance)];
}

export function applyAppearance(appearance: Appearance, preferences = readCachedThemePreferences()): void {
  const normalized = normalizeAppearance(appearance);
  const resolved = resolveAppearance(normalized);
  const normalizedPreferences = normalizeThemePreferences(preferences);
  const root = document.documentElement;
  const colorTheme = normalizedPreferences[resolved];
  if (root.dataset.theme !== resolved || root.dataset.colorTheme !== colorTheme) suppressThemeTransitions(root);
  root.dataset.appearance = normalized;
  root.dataset.theme = resolved;
  root.dataset.colorTheme = colorTheme;
  root.style.colorScheme = resolved;
  try {
    window.localStorage.setItem(APPEARANCE_CACHE_KEY, normalized);
    window.localStorage.setItem(THEME_CACHE_KEY, JSON.stringify(normalizedPreferences));
  } catch {
    // The persisted app setting remains the source of truth if browser storage is unavailable.
  }
}

export function listenForSystemAppearanceChange(onChange: () => void): () => void {
  const media = window.matchMedia(SYSTEM_DARK_QUERY);
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}
