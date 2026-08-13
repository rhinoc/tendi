import React from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App.tsx";
import { TooltipProvider } from "./components/shared/Tooltip.tsx";
import { applyAppearance, readCachedAppearance } from "./lib/appearance.ts";
import "./variables.css";
import "./styles.css";
import "./components/shared/animations.css";

applyAppearance(readCachedAppearance());

const root = document.getElementById("root");
if (!root) throw new Error("Root element #root not found");

createRoot(root).render(
  <React.StrictMode>
    <TooltipProvider>
      <App />
    </TooltipProvider>
  </React.StrictMode>,
);
