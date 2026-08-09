import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./app";
import { AppProvider } from "./state/store";
import "./styles.css";

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
