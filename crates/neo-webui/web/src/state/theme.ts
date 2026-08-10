/**
 * Theme preference ("light" | "dark"). The initial value follows
 * prefers-color-scheme (dark when no system preference is detectable); an
 * explicit user toggle is persisted to localStorage. Allowed storage keys:
 * sidebar width, theme preference — nothing else.
 */

export type Theme = "light" | "dark";

export const THEME_STORAGE_KEY = "neo-webui.theme";

/** Stored preference wins; otherwise the system preference; dark default. */
export function loadThemePreference(): Theme {
  try {
    const raw = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (raw === "light" || raw === "dark") return raw;
  } catch {
    // Storage may be unavailable; fall through to the system preference.
  }
  if (typeof window.matchMedia === "function") {
    return window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }
  return "dark";
}

export function saveThemePreference(theme: Theme): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Local preference only; storage may be unavailable.
  }
}

/** Components never branch on theme; only the document attribute switches. */
export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
}
