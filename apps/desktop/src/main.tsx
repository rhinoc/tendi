import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { TooltipProvider } from "./components/shared/Tooltip.tsx";
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

createRoot(root).render(
  <React.StrictMode>
    <TooltipProvider>
      <App />
    </TooltipProvider>
  </React.StrictMode>,
);

logger.info("frontend started");
