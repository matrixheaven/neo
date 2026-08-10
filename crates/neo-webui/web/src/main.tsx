import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app";
import { AppProvider } from "./state/store";
import { applyTheme, loadThemePreference } from "./state/theme";
import "./styles.css";

// Resolve and apply the theme synchronously before the first render so the
// page never flashes the wrong palette.
applyTheme(loadThemePreference());

const container = document.getElementById("root");
if (!container) {
  throw new Error("missing #root");
}

createRoot(container).render(
  <StrictMode>
    <AppProvider>
      <App />
    </AppProvider>
  </StrictMode>,
);
