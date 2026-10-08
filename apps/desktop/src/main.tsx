import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { TooltipProvider } from "./components/shared/Tooltip.tsx";
import { ToastProvider } from "./components/shared/Toast.tsx";
import { applyAppearance, applyFontFamily, readCachedAppearance, readCachedFontFamily, readCachedThemePreferences } from "./lib/appearance.ts";
import { applyAppIcon, readCachedAppIcon } from "./lib/app-icon.ts";
import { logger } from "./lib/logger.ts";
import "./variables.css";
import "./theme-overrides.css";
import "./styles.css";
import "./components/shared/animations.css";

applyAppearance(readCachedAppearance(), readCachedThemePreferences());
applyFontFamily(readCachedFontFamily());
void applyAppIcon(readCachedAppIcon());

const root = document.getElementById("root");
if (!root) throw new Error("Root element #root not found");

window.addEventListener("error", (event) => {
  logger.error("frontend uncaught error", {
    error: event.error ?? event.message,
    filename: event.filename,
    line: event.lineno,
    column: event.colno,
  });
});
window.addEventListener("unhandledrejection", (event) => {
  logger.error("frontend unhandled rejection", { error: event.reason });
});

createRoot(root).render(
  <React.StrictMode>
    <TooltipProvider>
      <ToastProvider>
        <App />
      </ToastProvider>
    </TooltipProvider>
  </React.StrictMode>,
);

logger.info("frontend started");
