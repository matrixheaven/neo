/**
 * Theme toggle: the initial theme follows prefers-color-scheme (dark when no
 * system preference), the top-bar button flips it immediately via
 * document.documentElement[data-theme], and only the explicit toggle persists
 * the preference to localStorage.
 */

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import React from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { App } from "../../src/app";
import { AppProvider } from "../../src/state/store";
import { THEME_STORAGE_KEY } from "../../src/state/theme";
import { FakeWebSocket, mockFetch, resetHarness } from "./harness";

function stubSystemColorScheme(scheme: "light" | "dark"): void {
  vi.stubGlobal(
    "matchMedia",
    vi.fn((query: string) => ({
      matches: query === "(prefers-color-scheme: light)" ? scheme === "light" : scheme === "dark",
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      onchange: null,
      dispatchEvent: () => false,
    })),
  );
}

function renderApp() {
  return render(
    React.createElement(
      AppProvider,
      null,
      React.createElement(App),
    ),
  );
}

describe("theme toggle", () => {
  beforeEach(() => {
    resetHarness();
    vi.stubGlobal("fetch", vi.fn(mockFetch));
    vi.stubGlobal("WebSocket", FakeWebSocket);
  });

  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
    delete document.documentElement.dataset.theme;
  });

  it("theme_toggle_persists_and_defaults_to_system", async () => {
    // System prefers light and no stored preference: the app starts light
    // (Moon icon offers the switch to dark) and writes nothing to storage.
    stubSystemColorScheme("light");
    const first = renderApp();
    await screen.findByLabelText("会话列表");
    const toggle = screen.getByRole("button", { name: "切换主题" });
    expect(first.container.querySelector("svg.lucide-moon")).not.toBeNull();
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();

    // The toggle flips the document attribute immediately and persists.
    fireEvent.click(toggle);
    expect(document.documentElement.dataset.theme).toBe("dark");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("dark");
    fireEvent.click(toggle);
    expect(document.documentElement.dataset.theme).toBe("light");
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe("light");
    cleanup();

    // System prefers dark and no stored preference: the app starts dark.
    window.localStorage.clear();
    stubSystemColorScheme("dark");
    const second = renderApp();
    await screen.findByLabelText("会话列表");
    expect(second.container.querySelector("svg.lucide-sun")).not.toBeNull();
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();
  });
});
