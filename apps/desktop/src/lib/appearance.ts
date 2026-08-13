export const appearances = ["system", "light", "dark"] as const;

export type Appearance = (typeof appearances)[number];
export type ResolvedAppearance = Exclude<Appearance, "system">;

const APPEARANCE_CACHE_KEY = "tendi.appearance";
const SYSTEM_DARK_QUERY = "(prefers-color-scheme: dark)";

export function normalizeAppearance(value: unknown): Appearance {
  return appearances.includes(value as Appearance) ? value as Appearance : "system";
}

export function readCachedAppearance(): Appearance {
  try {
    return normalizeAppearance(window.localStorage.getItem(APPEARANCE_CACHE_KEY));
  } catch {
    return "system";
  }
}

export function resolveAppearance(appearance: Appearance): ResolvedAppearance {
  if (appearance !== "system") return appearance;
  return window.matchMedia(SYSTEM_DARK_QUERY).matches ? "dark" : "light";
}

export function applyAppearance(appearance: Appearance): void {
  const normalized = normalizeAppearance(appearance);
  const resolved = resolveAppearance(normalized);
  document.documentElement.dataset.appearance = normalized;
  document.documentElement.dataset.theme = resolved;
  document.documentElement.style.colorScheme = resolved;
  try {
    window.localStorage.setItem(APPEARANCE_CACHE_KEY, normalized);
  } catch {
    // The persisted app setting remains the source of truth if browser storage is unavailable.
  }
}

export function listenForSystemAppearanceChange(onChange: () => void): () => void {
  const media = window.matchMedia(SYSTEM_DARK_QUERY);
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}
