// TEMP visual diagnostics for the DataTable frozen refactor. Not a real test.
import { existsSync, mkdirSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";

const require = createRequire(import.meta.url);
const appDir = join(dirname(fileURLToPath(import.meta.url)), "..");
const PORT = 5194;
const colorScheme = process.env.COLOR_SCHEME === "dark" ? "dark" : "light";
const shotPrefix = `_shot_${colorScheme}`;

function loadPlaywrightCore() {
  try { return require("playwright-core"); }
  catch { return createRequire("/opt/homebrew/lib/node_modules/playwright/package.json")("playwright-core"); }
}
function chromiumExecutablePath() {
  const cached = join(homedir(), "Library/Caches/ms-playwright/chromium-1223/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing");
  if (existsSync(cached)) return cached;
  const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  if (existsSync(chrome)) return chrome;
  return null;
}

function buildReport() {
  const sessions = Array.from({ length: 12 }, (_, i) => ({
    id: `session-${i + 1}`,
    title: `Session number ${i + 1} with a long enough title to overflow`,
    agent: ["cursor", "codex", "claude"][i % 3],
    project: `/Users/dev/project-${i + 1}`,
    started_at: `2026-06-2${i}T09:00:00`,
    updated_at: `2026-06-2${i}T18:00:00`,
    path: `/tmp/session-${i + 1}.jsonl`,
    message_count: 10 + i,
  }));
  const hooks = Array.from({ length: 12 }, (_, i) => ({
    event: `event-${i + 1}`, agent: ["cursor", "codex", "claude"][i % 3], matcher: "*",
    enabled: i % 2 === 0, command: `echo hook-${i + 1}`, path: `/Users/dev/.claude/hooks.json`,
    trust_hash: `hook-trust-${i + 1}`, type: "command",
  }));
  return {
    skills: { skills: [] }, prompts: { prompts: [] }, sessions: { sessions },
    rules: { rules: [] }, hooks: { hooks }, mcp: { servers: [] }, agents: { agents: [] },
    settings: { appearance: "system", terminal: "auto", editor: "vscode", additionalSessionRoots: [], configProfiles: {} },
  };
}

const { chromium } = loadPlaywrightCore();
let server, browser;
try {
  server = await createServer({ root: appDir, appType: "spa", logLevel: "error", server: { host: "127.0.0.1", port: PORT, strictPort: true } });
  await server.listen();
  browser = await chromium.launch({ headless: true, executablePath: chromiumExecutablePath() ?? undefined });
  const page = await browser.newPage({ colorScheme, viewport: { width: 760, height: 560 } });
  await page.addInitScript((report) => {
    const callbacks = new Map();
    let nextCallbackId = 1;
    let sessionScanHandler = null;
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      invoke: async (command, args) => {
        if (command === "plugin:event|listen") {
          sessionScanHandler = callbacks.get(args.handler);
          return 1;
        }
        if (command === "plugin:event|unlisten") return null;
        if (command === "scan") return report;
        if (command === "sessions_list") return report.sessions.sessions;
        if (command === "sessions_scan_start") {
          queueMicrotask(() => {
            sessionScanHandler?.({ id: 1, event: "sessions://scan", payload: { generation: 1, phase: "recent", upserts: [], deleted: [], scanned: 0, complete: true } });
            sessionScanHandler?.({ id: 1, event: "sessions://scan", payload: { generation: 1, phase: "backfill", upserts: [], deleted: [], scanned: report.sessions.sessions.length, complete: true } });
          });
          return 1;
        }
        if (command === "hooks_list") return report.hooks.hooks;
        if (command === "skills_list") return report.skills.skills;
        if (command === "prompts_list") return report.prompts.prompts;
        if (command === "rules_list") return report.rules.rules;
        if (command === "mcp_list") return report.mcp.servers;
        if (command === "agents_list") return report.agents.agents;
        if (command === "settings_get") return report.settings;
        if (command === "settings_save") {
          Object.assign(report.settings, args);
          return report.settings;
        }
        if (command === "terminal_apps_list") return [{ id: "auto", label: "Auto", available: true }];
        if (command === "skills_updates") return [];
        return null;
      },
      transformCallback: (callback) => {
        const id = nextCallbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id) => callbacks.delete(id),
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    };
  }, buildReport());

  await page.goto(`http://127.0.0.1:${PORT}/`);
  await page.getByRole("heading", { name: "Skills" }).waitFor();
  await page.getByRole("button", { name: "Sessions", exact: true }).click();
  await page.getByRole("heading", { name: "Sessions", exact: true }).waitFor();
  await page.locator(".dataRow").first().waitFor();

  const dir = join(appDir, "..", "..", ".codex-screenshots");
  mkdirSync(dir, { recursive: true });
  await page.screenshot({ path: join(dir, `${shotPrefix}_1_initial.png`) });

  // vertical scroll
  await page.evaluate(() => { document.querySelector(".dataTableBodyScroll").scrollTop = 120; });
  await page.waitForTimeout(150);
  await page.screenshot({ path: join(dir, `${shotPrefix}_2_vscroll.png`) });

  // horizontal scroll + hover a row
  await page.evaluate(() => { const s = document.querySelector(".dataTableBodyScroll"); s.scrollTop = 0; s.scrollLeft = 160; });
  await page.waitForTimeout(150);
  await page.locator(".dataRow").nth(2).hover();
  await page.waitForTimeout(150);
  await page.screenshot({ path: join(dir, `${shotPrefix}_3_hscroll_hover.png`) });

  const info = await page.evaluate(() => {
    const s = document.querySelector(".dataTableBodyScroll");
    const rootStyle = getComputedStyle(document.documentElement);
    return {
      colorScheme: matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light",
      scrollW: s.scrollWidth,
      clientW: s.clientWidth,
      scrollH: s.scrollHeight,
      clientH: s.clientHeight,
      text: rootStyle.getPropertyValue("--text").trim(),
      contentBg: rootStyle.getPropertyValue("--content-bg").trim(),
      panelBg: rootStyle.getPropertyValue("--panel-bg").trim(),
    };
  });

  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page.getByRole("heading", { name: "Settings", exact: true }).waitFor();
  await page.screenshot({ path: join(dir, `${shotPrefix}_4_settings.png`) });
  const systemControl = page.locator(".settingsAppearanceItem", { hasText: "System" });
  const lightControl = page.locator(".settingsAppearanceItem", { hasText: "Light" });
  const darkControl = page.locator(".settingsAppearanceItem", { hasText: "Dark" });
  await systemControl.waitFor({ timeout: 5000 });
  await darkControl.click();
  await page.waitForFunction(() => document.documentElement.dataset.theme === "dark");
  await lightControl.click();
  await page.waitForFunction(() => document.documentElement.dataset.theme === "light");
  await systemControl.click();
  await page.waitForFunction(() => {
    const expected = matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
    return document.documentElement.dataset.theme === expected;
  });
  const oppositeColorScheme = colorScheme === "dark" ? "light" : "dark";
  await page.emulateMedia({ colorScheme: oppositeColorScheme });
  await page.waitForFunction((expected) => document.documentElement.dataset.theme === expected, oppositeColorScheme);
  await page.emulateMedia({ colorScheme });
  await page.waitForFunction((expected) => document.documentElement.dataset.theme === expected, colorScheme);
  info.appearance = await page.evaluate(() => ({
    preference: document.documentElement.dataset.appearance,
    resolved: document.documentElement.dataset.theme,
  }));
  console.log(JSON.stringify(info));
} finally {
  await browser?.close().catch(() => {});
  await server?.close().catch(() => {});
}
