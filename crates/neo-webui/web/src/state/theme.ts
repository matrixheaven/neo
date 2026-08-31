/**
 * Theme preference and persistent appearance. The resolved interface theme is
 * "light" | "dark" (mirrored onto document.documentElement[data-theme]); the
 * config-backed appearance may request "system" (follow OS) at runtime.
 * Appearance (font sizes, code theme, line numbers, word wrap) is applied via
 * CSS variables and the `data-code-theme` attribute.
 */

import type { WebUiAppearance } from "../protocol";

export type Theme = "light" | "dark";

export const THEME_STORAGE_KEY = "neo-webui.theme";
export const APPEARANCE_STORAGE_KEY = "neo-webui.appearance";

export function defaultAppearance(): WebUiAppearance {
  return {
    theme: "system",
    ui_font_size: 14,
    code_font_size: 12,
    code_theme: "auto",
    show_line_numbers: true,
    word_wrap: true,
  };
}

function systemPrefersLight(): boolean {
  if (typeof window.matchMedia !== "function") return false;
  return window.matchMedia("(prefers-color-scheme: light)").matches;
}

/** Resolve a config theme ("system" | "light" | "dark") to a concrete theme. */
export function resolveTheme(theme: string): Theme {
  if (theme === "light") return "light";
  if (theme === "dark") return "dark";
  return systemPrefersLight() ? "light" : "dark";
}

/** Legacy preference loader (browser local), kept for first-paint before the
 * bootstrap snapshot arrives. The config-backed appearance wins afterwards. */
export function loadThemePreference(): Theme {
  try {
    const raw = window.localStorage.getItem(THEME_STORAGE_KEY);
    if (raw === "light" || raw === "dark") return raw;
  } catch {
    // Storage may be unavailable; fall through to the system preference.
  }
  return systemPrefersLight() ? "light" : "dark";
}

export function saveThemePreference(theme: Theme): void {
  try {
    window.localStorage.setItem(THEME_STORAGE_KEY, theme);
  } catch {
    // Local preference only; storage may be unavailable.
  }
}

/** Apply the interface theme to the document. Components never branch on
 * theme; only the data-theme attribute switches the raw palette. */
export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
}

/** Apply full appearance (theme + font sizes + code theme + display toggles).
 * Called from the config-backed bootstrap and settings save. */
export function applyAppearance(appearance: WebUiAppearance): Theme {
  const theme = resolveTheme(appearance.theme);
  applyTheme(theme);
  document.documentElement.dataset.codeTheme = appearance.code_theme;
  document.documentElement.style.setProperty(
    "--font-size-body",
    `${appearance.ui_font_size}px`,
  );
  document.documentElement.style.setProperty(
    "--font-size-code",
    `${appearance.code_font_size}px`,
  );
  document.documentElement.dataset.lineNumbers = appearance.show_line_numbers
    ? "on"
    : "off";
  document.documentElement.dataset.wordWrap = appearance.word_wrap ? "on" : "off";
  return theme;
}

/** Subscribe to OS theme changes when the appearance requests "system". */
export function watchSystemTheme(
  appearance: WebUiAppearance,
  onChange: (theme: Theme) => void,
): () => void {
  if (appearance.theme !== "system" || typeof window.matchMedia !== "function") {
    return () => {};
  }
  const query = window.matchMedia("(prefers-color-scheme: light)");
  const handle = () => onChange(resolveTheme(appearance.theme));
  query.addEventListener("change", handle);
  return () => query.removeEventListener("change", handle);
}
