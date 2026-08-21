export const appearances = ["system", "light", "dark"] as const;
export const colorThemes = ["sakura-pop", "gruvbox", "dracula", "nord", "catppuccin", "tokyo-night", "vercel"] as const;
export const fontFamilies = [
  "geist",
  "manrope",
  "inter",
  "ibm-plex-sans",
  "instrument-sans",
  "plus-jakarta-sans",
  "bricolage-grotesque",
] as const;

export type Appearance = (typeof appearances)[number];
export type ResolvedAppearance = Exclude<Appearance, "system">;
export type ColorTheme = (typeof colorThemes)[number];
export type FontFamily = (typeof fontFamilies)[number];
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
  return appearances.includes(value as Appearance) ? value as Appearance : "system";
}

export function normalizeColorTheme(value: unknown): ColorTheme {
  return colorThemes.includes(value as ColorTheme) ? value as ColorTheme : "sakura-pop";
}

export function normalizeFontFamily(value: unknown): FontFamily {
  return fontFamilies.includes(value as FontFamily) ? value as FontFamily : "manrope";
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
    return "system";
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
    return "manrope";
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
  if (appearance !== "system") return appearance;
  return window.matchMedia(SYSTEM_DARK_QUERY).matches ? "dark" : "light";
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
