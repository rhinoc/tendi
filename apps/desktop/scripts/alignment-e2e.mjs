// Alignment E2E — verifies the three unified-table requirements across every
// data tab, plus navigation and header contracts for all nine pages.
//
// Requirements under test (per tab):
//   req1  Row selection checkbox is hidden by default and revealed on row hover.
//   req2  Row checkbox sits on the same vertical line as the bottom-bar checkbox.
//   req3  The tab header (page title + any list/section/column header) shares a
//         vertical line with the row's first text column (NOT the checkbox).
//   req4  The bottom action bar uses uniform padding (top = bottom = left = right),
//         its height equals top padding + bottom padding + checkbox height, the
//         checkbox sits that same distance from the bar's top/left/bottom edges and
//         from the selected-count label, and outer right/bottom inset match the page
//         (left inset follows the selection-rail gutter).
//   req5  Vertical scroll may overscroll; horizontal scroll must not. The table
//         column header is a sticky element inside the single scroller and stays
//         pinned to the top while the body scrolls.
//   req6  Row separator lines start on the same vertical line as the row text.
//   req7  Table header bottom rule starts on the text rail (not full bleed); grouped
//         section headers show an item count matching their rows (Skills).
//   req8  Active-row hover/selection highlights inset 1px from the row top so adjacent
//         active rows keep a visible gap; frozen columns hide their own separator
//         shadows so the sticky segment reads as one continuous highlight.
//   bugN  Frozen-column regression checks cover scroll bleed, marquee selection,
//         sticky headers, active-row seams and frozen/scroll row height sync.
//
// The app talks to a mocked `__TAURI_INTERNALS__.invoke`, so the test is fast,
// deterministic and self-contained (no cargo / real data). Run with:
//   npm run e2e:align        (from apps/desktop)
//
// The run collects EVERY failure across EVERY tab before throwing, so a single
// run reports the full picture instead of stopping at the first mismatch.

import { existsSync } from "node:fs";
import { createRequire } from "node:module";
import { homedir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "vite";
import { writeStderr, writeStdout } from "./stdio.mjs";

const require = createRequire(import.meta.url);
const appDir = join(dirname(fileURLToPath(import.meta.url)), "..");

// Pixel tolerance for "same vertical line" checks. Sub-pixel rounding from the
// browser layout engine makes a hard 0px assertion flaky; 1.5px keeps the
// intent (visually identical) while tolerating rounding.
const TOLERANCE = 1.5;
const PORT = 5193;

function loadPlaywrightCore() {
  try {
    return require("playwright-core");
  } catch {
    return createRequire("/opt/homebrew/lib/node_modules/playwright/package.json")("playwright-core");
  }
}

function chromiumExecutablePath() {
  const cached = join(
    homedir(),
    "Library/Caches/ms-playwright/chromium-1223/chrome-mac-arm64/Google Chrome for Testing.app/Contents/MacOS/Google Chrome for Testing",
  );
  if (existsSync(cached)) return cached;
  const chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
  if (existsSync(chrome)) return chrome;
  return null;
}

// ----- fabricated report (matches the shapes each normalizer/column reads) ---
function buildReport() {
  const skills = [
    { id: "alpha-skill", name: "alpha-skill", description: "First local skill.", agents: ["Codex", "Cursor"], visibility: "Manual", source: "github", install_targets: ["shared"], update_status: "update-available" },
    { id: "beta-skill", name: "beta-skill", description: "Second local skill.", agents: ["Codex"], visibility: "Auto", source: "local", install_targets: ["shared"], update_status: "local" },
    { id: "system-skill", name: "system-skill", description: "Managed system skill.", agents: ["Codex"], visibility: "Auto", is_system: true, source: "system", install_targets: ["codex"], update_status: "local" },
  ];
  const prompts = Array.from({ length: 12 }, (_, i) => ({
    id: `prompt-${i + 1}`,
    title: `Prompt number ${i + 1}`,
    tags: ["smoke"],
    body: `Body for prompt ${i + 1}.`,
    updated_at: `2026-06-2${i}T10:00:00`,
  }));
  const sessions = Array.from({ length: 12 }, (_, i) => ({
    id: `session-${i + 1}`,
    title: `Session number ${i + 1}`,
    agent: ["cursor", "codex"][i % 2],
    project: `/Users/dev/project-${i + 1}`,
    started_at: `2026-06-2${i}T09:00:00`,
    updated_at: `2026-06-2${i}T18:00:00`,
    path: `/tmp/session-${i + 1}.jsonl`,
    message_count: 10 + i,
    turn_count: 3 + i,
  }));
  const rules = Array.from({ length: 12 }, (_, i) => ({
    path: `/Users/dev/.cursor/rules/rule-${i + 1}.mdc`,
    agent: ["cursor", "codex", "claude"][i % 3],
    kind: ["Always", "Auto", "Manual"][i % 3],
    scope: ["Project", "Global"][i % 2],
    order: i + 1,
    sha256: `rule-sha-${i + 1}`,
  }));
  const hooks = Array.from({ length: 12 }, (_, i) => ({
    event: `event-${i + 1}`,
    agent: ["cursor", "codex", "claude"][i % 3],
    matcher: "*",
    enabled: i % 2 === 0,
    command: `echo hook-${i + 1}`,
    path: `/Users/dev/.claude/hooks.json`,
    trust_hash: `hook-trust-${i + 1}`,
    type: "command",
  }));
  const mcp = Array.from({ length: 12 }, (_, i) => ({
    agent: i < 2 ? "cursor" : ["cursor", "codex", "claude"][i % 3],
    name: i < 2 ? "cursor-app-control" : `mcp-server-${i + 1}`,
    scope: i < 2 ? `project-${i + 1}` : "global",
    transport: i < 2 ? "cursor-plugin" : ["stdio", "http"][i % 2],
    status: i < 2 ? "configured" : ["running", "stopped"][i % 2],
    path: i < 2
      ? `/Users/dev/.cursor/projects/project-${i + 1}/mcps/cursor-app-control/SERVER_METADATA.json`
      : `/Users/dev/.cursor/mcp-${i + 1}.json`,
  }));
  return {
    skills: { skills },
    prompts: { prompts },
    sessions: { sessions },
    rules: { rules },
    hooks: { hooks },
    mcp: { servers: mcp },
    agents: {
      agents: [
        { kind: "claude", name: "Claude Code", installed: true },
        { kind: "ghost", name: "Ghost Agent", installed: false },
      ],
    },
  };
}

const tabs = [
  { id: "skills", nav: "Skills", heading: "Skills", compact: false, selectable: true, listHeader: "section", tableHeader: false, frozen: true },
  { id: "prompts", nav: "Prompts", heading: "Prompts", compact: false, selectable: true, listHeader: "table", tableHeader: true, frozen: false },
  { id: "sessions", nav: "Sessions", heading: "Sessions", compact: true, selectable: false, listHeader: "table", tableHeader: true, frozen: true },
  { id: "rules", nav: "Rules", heading: "Rules", compact: true, selectable: true, listHeader: "table", tableHeader: true, frozen: true },
  { id: "hooks", nav: "Hooks", heading: "Hooks", compact: true, selectable: true, listHeader: "table", tableHeader: true, frozen: true },
  { id: "mcp", nav: "MCP", heading: "MCP", compact: false, selectable: true, listHeader: "table", tableHeader: true, frozen: true },
];

const failures = [];
const runtimePageErrors = [];
function check(tab, requirement, condition, detail) {
  const status = condition ? "ok " : "FAIL";
  writeStdout(`  [${status}] ${tab} ${requirement}: ${detail}`);
  if (!condition) failures.push(`${tab} ${requirement}: ${detail}`);
}

async function runPageHeaderChecks(page, tab, heading, expectedCompact) {
  const metrics = await page.locator(".pageHeader").first().evaluate((node) => {
    const style = getComputedStyle(node);
    const rootStyle = getComputedStyle(document.documentElement);
    const toPx = (value) => Number.parseFloat(value) || 0;
    return {
      title: node.querySelector("h1")?.textContent?.trim() ?? "",
      titleTop: node.querySelector("h1")?.getBoundingClientRect().top ?? 0,
      compact: node.classList.contains("compact"),
      height: node.getBoundingClientRect().height,
      paddingTop: toPx(style.paddingTop),
      paddingBottom: toPx(style.paddingBottom),
      marginBottom: toPx(style.marginBottom),
      pageTopInset: toPx(rootStyle.getPropertyValue("--page-top-inset")),
      pageHeaderBottomInset: toPx(rootStyle.getPropertyValue("--page-header-bottom-inset")),
      parentPaddingTop: toPx(getComputedStyle(node.parentElement ?? node).paddingTop),
    };
  });
  check(tab, "header-title", metrics.title === heading, `title ${JSON.stringify(metrics.title)}`);
  check(tab, "header-mode", metrics.compact === expectedCompact, `compact ${metrics.compact}, expected ${expectedCompact}`);
  check(tab, "header-visible", metrics.height > 0, `height ${metrics.height}px`);
  check(
    tab,
    "header-title-top",
    metrics.pageTopInset > 0 && Math.abs(metrics.titleTop - metrics.pageTopInset) <= TOLERANCE,
    `title top ${metrics.titleTop}px vs ${metrics.pageTopInset}px`,
  );
  const bottomSpaceMatches = expectedCompact
    ? Math.abs(metrics.paddingBottom - metrics.pageHeaderBottomInset) <= TOLERANCE && metrics.marginBottom <= TOLERANCE
    : metrics.paddingBottom <= TOLERANCE && Math.abs(metrics.marginBottom - metrics.pageHeaderBottomInset) <= TOLERANCE;
  check(
    tab,
    "header-bottom-space",
    metrics.pageHeaderBottomInset > 0 && bottomSpaceMatches,
    `${expectedCompact ? "padding-bottom" : "margin-bottom"} ${expectedCompact ? metrics.paddingBottom : metrics.marginBottom}px vs ${metrics.pageHeaderBottomInset}px`,
  );
  if (expectedCompact) {
    check(
      tab,
      "header-top-space",
      metrics.pageTopInset > 0 && Math.abs(metrics.paddingTop - metrics.pageTopInset) <= TOLERANCE,
      `padding-top ${metrics.paddingTop}px vs ${metrics.pageTopInset}px`,
    );
  } else {
    check(
      tab,
      "header-parent-top-space",
      metrics.pageTopInset > 0 && Math.abs(metrics.parentPaddingTop - metrics.pageTopInset) <= TOLERANCE,
      `parent padding-top ${metrics.parentPaddingTop}px vs ${metrics.pageTopInset}px`,
    );
  }
}

async function runReq8NonFrozenFixture(page) {
  await page.evaluate(() => {
    if (document.getElementById("align-req8-fixture")) return;
    const fixture = document.createElement("div");
    fixture.id = "align-req8-fixture";
    fixture.className = "dataTableShell";
    fixture.style.cssText = [
      "position:fixed",
      "top:0",
      "left:0",
      "width:860px",
      "opacity:0.02",
      "z-index:99999",
      "pointer-events:auto",
      "--data-grid-columns:var(--data-table-selection-col) minmax(220px, 1fr) minmax(160px, 1fr)",
    ].join(";");
    fixture.innerHTML = `
      <div class="dataTableBody listSurface">
        <div class="dataTableRowSlot">
          <div class="dataRow rowFrame rowSelected" data-row-selectable="true" data-row-id="req8-a">
            <span class="rowSelectionPlaceholder" aria-hidden="true"></span>
            <div class="dataCell"><span class="dataCellTitle">Req8 row A</span></div>
            <div class="dataCell"><span class="dataCellText">Column B</span></div>
          </div>
        </div>
        <div class="dataTableRowSlot">
          <div class="dataRow rowFrame" data-row-selectable="true" data-row-id="req8-b">
            <span class="rowSelectionPlaceholder" aria-hidden="true"></span>
            <div class="dataCell"><span class="dataCellTitle">Req8 row B</span></div>
            <div class="dataCell"><span class="dataCellText">Column B</span></div>
          </div>
        </div>
      </div>
    `;
    document.body.appendChild(fixture);
  });

  await page.locator("#align-req8-fixture .dataRow.rowFrame").nth(1).hover();
  await page.waitForFunction(() => {
    const hovered = document.querySelector("#align-req8-fixture .dataRow.rowFrame:hover");
    return hovered && getComputedStyle(hovered, "::after").backgroundColor !== "rgba(0, 0, 0, 0)"
      && getComputedStyle(hovered, "::after").backgroundColor !== "transparent";
  }, { timeout: 2000 }).catch(() => {});

  const fixtureMetrics = await page.evaluate(() => {
    const isTransparent = (bg) => !bg || bg === "transparent" || bg === "rgba(0, 0, 0, 0)";
    const fixture = document.getElementById("align-req8-fixture");
    const selected = fixture?.querySelector(".dataRow.rowSelected");
    const hovered = fixture?.querySelector(".dataRow.rowFrame:hover");
    if (!fixture || !selected || !hovered) return { missing: true };
    const afterTop = (row) => parseFloat(getComputedStyle(row, "::after").top);
    return {
      missing: false,
      hasFrozenShell: fixture.classList.contains("dataTableShell--frozen"),
      hasFrozenCell: Boolean(hovered.querySelector(".dataCell[data-frozen]")),
      selectedAfterTop: afterTop(selected),
      hoveredAfterTop: afterTop(hovered),
      hoveredBeforeTransparent: isTransparent(getComputedStyle(hovered, "::before").backgroundColor),
      hoveredAfterBg: getComputedStyle(hovered, "::after").backgroundColor,
    };
  });

  check("fixture", "req8-nonfrozen-shell", fixtureMetrics.missing !== true && fixtureMetrics.hasFrozenShell === false, `fixture frozen shell ${fixtureMetrics.hasFrozenShell}`);
  check("fixture", "req8-nonfrozen-no-frozen-cell", fixtureMetrics.missing !== true && fixtureMetrics.hasFrozenCell === false, `fixture frozen cell ${fixtureMetrics.hasFrozenCell}`);
  check(
    "fixture",
    "req8-nonfrozen-hover-inset",
    fixtureMetrics.missing !== true && Math.abs(fixtureMetrics.hoveredAfterTop - 1) <= TOLERANCE,
    `hovered ::after top ${fixtureMetrics.hoveredAfterTop}px should be 1px`,
  );
  check(
    "fixture",
    "req8-nonfrozen-selected-inset",
    fixtureMetrics.missing !== true && Math.abs(fixtureMetrics.selectedAfterTop - 1) <= TOLERANCE,
    `selected ::after top ${fixtureMetrics.selectedAfterTop}px should be 1px`,
  );
  check(
    "fixture",
    "req8-nonfrozen-adjacent-gap",
    fixtureMetrics.missing !== true && fixtureMetrics.hoveredBeforeTransparent === true,
    `separator above hovered row transparent=${fixtureMetrics.hoveredBeforeTransparent}`,
  );

  await page.mouse.move(0, 0);
  await page.evaluate(() => document.getElementById("align-req8-fixture")?.remove());
}

async function runReq8TabChecks(page, tab) {
  await page.keyboard.press("Escape").catch(() => {});
  await page.waitForFunction(
    () => !document.querySelector('[role="dialog"][data-state="open"]'),
    { timeout: 3000 },
  ).catch(() => {});

  const rows = page.locator(".dataRow.rowFrame");
  const rowCount = await rows.count();
  if (rowCount < 2) {
    check(tab.id, "req8-rows", false, `need at least 2 rows, found ${rowCount}`);
    return;
  }

  if (!tab.selectable) {
    await page.evaluate(() => {
      document.querySelectorAll(".dataRow.rowSelected").forEach((row) => row.classList.remove("rowSelected"));
      const row = document.querySelector(".dataRow.rowFrame");
      if (!row?.dataset.rowId) return;
      document.querySelectorAll(`[data-row-id="${CSS.escape(row.dataset.rowId)}"]`).forEach((peer) => peer.classList.add("rowSelected"));
    });
  } else {
    await page.evaluate(() => {
      if (document.querySelector(".dataRow.rowSelected")) return;
      const row = document.querySelector('.dataRow[data-row-selectable="true"]');
      if (!row?.dataset.rowId) return;
      document.querySelectorAll(`[data-row-id="${CSS.escape(row.dataset.rowId)}"]`).forEach((peer) => peer.classList.add("rowSelected"));
    });
  }

  await rows.nth(1).hover();
  await page.waitForFunction(() => {
    const hovered = document.querySelector(".dataRow.rowFrame:hover");
    if (!hovered) return false;
    const bg = getComputedStyle(hovered, "::after").backgroundColor;
    return bg !== "transparent" && bg !== "rgba(0, 0, 0, 0)";
  }, { timeout: 2000 }).catch(() => {});

  const metrics = await page.evaluate(() => {
    const isTransparent = (bg) => !bg || bg === "transparent" || bg === "rgba(0, 0, 0, 0)";
    const hasInsetStrip = (shadow) => Boolean(shadow && shadow !== "none" && shadow.includes("inset"));
    const measureRow = (row) => {
      if (!row) return null;
      const frozenCell = row.querySelector(".dataCell[data-frozen]");
      return {
        afterTop: parseFloat(getComputedStyle(row, "::after").top),
        beforeTransparent: isTransparent(getComputedStyle(row, "::before").backgroundColor),
        frozenCell: frozenCell
          ? {
            hasInsetStrip: hasInsetStrip(getComputedStyle(frozenCell).boxShadow),
            gapHasInsetStrip: hasInsetStrip(getComputedStyle(frozenCell, "::after").boxShadow),
          }
          : null,
      };
    };
    return {
      frozenShell: Boolean(document.querySelector(".dataTableShell--frozen")),
      selected: measureRow(document.querySelector(".dataRow.rowSelected")),
      hovered: measureRow(document.querySelector(".dataRow.rowFrame:hover")),
    };
  });

  check(
    tab.id,
    "req8-hover-inset",
    metrics.hovered && Math.abs(metrics.hovered.afterTop - 1) <= TOLERANCE,
    `hovered ::after top ${metrics.hovered?.afterTop}px should be 1px`,
  );
  check(
    tab.id,
    "req8-selected-inset",
    metrics.selected && Math.abs(metrics.selected.afterTop - 1) <= TOLERANCE,
    `selected ::after top ${metrics.selected?.afterTop}px should be 1px`,
  );
  check(
    tab.id,
    "req8-adjacent-gap",
    metrics.hovered && metrics.hovered.beforeTransparent === true,
    `separator above hovered row transparent=${metrics.hovered?.beforeTransparent}`,
  );

  if (tab.frozen) {
    check(tab.id, "req8-frozen-shell", metrics.frozenShell === true, `frozen shell ${metrics.frozenShell}`);
    check(
      tab.id,
      "req8-frozen-hover-no-strip",
      metrics.hovered?.frozenCell?.hasInsetStrip === false,
      `hovered frozen cell inset strip ${metrics.hovered?.frozenCell?.hasInsetStrip}`,
    );
    check(
      tab.id,
      "req8-frozen-gap-no-strip",
      metrics.hovered?.frozenCell?.gapHasInsetStrip === false,
      `hovered frozen gap inset strip ${metrics.hovered?.frozenCell?.gapHasInsetStrip}`,
    );
    check(
      tab.id,
      "req8-frozen-selected-no-strip",
      metrics.selected?.frozenCell?.hasInsetStrip === false,
      `selected frozen cell inset strip ${metrics.selected?.frozenCell?.hasInsetStrip}`,
    );
  } else {
    check(tab.id, "req8-no-frozen-shell", metrics.frozenShell === false, `frozen shell ${metrics.frozenShell}`);
    check(
      tab.id,
      "req8-no-frozen-cell",
      metrics.hovered?.frozenCell == null,
      `hovered frozen cell ${metrics.hovered?.frozenCell != null}`,
    );
  }

  await page.mouse.move(0, 0);
}

async function runSessionLocatorChecks(page) {
  await page.locator(".dataRow--frozenPane .sessionTitleText").first().click();
  const locatorRows = page.locator(".sessionLocatorRow");
  await locatorRows.first().waitFor();
  await page.waitForFunction(
    () => document.querySelectorAll('.sessionLocatorRow[aria-current="true"]').length > 0,
    { timeout: 2000 },
  ).catch(() => {});
  const locatorCount = await locatorRows.count();
  const currentLocatorCount = await page.locator('.sessionLocatorRow[aria-current="true"]').count();
  check(
    "sessions",
    "session-locator-items",
    locatorCount === 5 && currentLocatorCount > 0,
    `${locatorCount} locator items, ${currentLocatorCount} current`,
  );
  await locatorRows.nth(2).hover();
  const locatorPreview = page.locator(".sessionLocatorPreview");
  const locatorPreviewText = (await locatorPreview.allTextContents()).join(" ");
  check(
    "sessions",
    "session-locator-preview",
    locatorPreviewText.includes("Locator prompt 3")
      && locatorPreviewText.includes("Locator response 3"),
    `preview text: ${locatorPreviewText}`,
  );
  const locatorPreviewLayout = await locatorPreview.evaluate((preview) => {
    const rect = preview.getBoundingClientRect();
    let ancestor = preview.parentElement;
    let clippingAncestor = "";
    while (ancestor) {
      const style = getComputedStyle(ancestor);
      const ancestorRect = ancestor.getBoundingClientRect();
      const clipsX = style.overflowX !== "visible"
        && (rect.left < ancestorRect.left || rect.right > ancestorRect.right);
      const clipsY = style.overflowY !== "visible"
        && (rect.top < ancestorRect.top || rect.bottom > ancestorRect.bottom);
      if (clipsX || clipsY) {
        clippingAncestor = ancestor.className || ancestor.tagName;
        break;
      }
      ancestor = ancestor.parentElement;
    }
    return {
      width: rect.width,
      inViewport: rect.left >= 0
        && rect.top >= 0
        && rect.right <= window.innerWidth
        && rect.bottom <= window.innerHeight,
      clippingAncestor,
    };
  });
  check(
    "sessions",
    "session-locator-preview-unclipped",
    locatorPreviewLayout.width >= 200
      && locatorPreviewLayout.inViewport
      && locatorPreviewLayout.clippingAncestor === "",
    `width ${locatorPreviewLayout.width}px, in viewport ${locatorPreviewLayout.inViewport}, clipping ancestor ${locatorPreviewLayout.clippingAncestor || "none"}`,
  );
  await locatorRows.nth(4).click();
  await page.waitForTimeout(40);
  check(
    "sessions",
    "session-locator-jump",
    await page.locator('[data-transcript-key="user-8"].transcriptTarget').count() === 1,
    "fifth locator jumps to user-8",
  );
  const firstLocatorBox = await locatorRows.first().boundingBox();
  const fourthLocatorBox = await locatorRows.nth(3).boundingBox();
  if (firstLocatorBox && fourthLocatorBox) {
    await page.mouse.move(firstLocatorBox.x + 3, firstLocatorBox.y + firstLocatorBox.height / 2);
    await page.mouse.down();
    await page.mouse.move(fourthLocatorBox.x + 3, fourthLocatorBox.y + fourthLocatorBox.height / 2, { steps: 4 });
    await page.mouse.up();
  }
  check(
    "sessions",
    "session-locator-scrub",
    firstLocatorBox != null
      && fourthLocatorBox != null
      && await page.locator('[data-transcript-key="user-6"].transcriptTarget').count() === 1,
    "dragging from the first to fourth locator jumps to user-6",
  );
  const collapseButton = page.getByRole("button", { name: "Collapse session detail", exact: true });
  if (await collapseButton.count()) {
    await collapseButton.click({ force: true });
    await page.waitForFunction(
      () => document.querySelector(".transcriptPanelHost")?.classList.contains("collapsed") === true,
      { timeout: 2000 },
    ).catch(() => {});
  }
}

async function runFrozenBugSmokeChecks(page, tab) {
  if (!tab.frozen) return;

  const overflowFixture = await page.evaluate(() => {
    const shell = document.querySelector(".dataTableShell--frozen");
    const scrollColumns = shell?.querySelectorAll(".dataTableHeader--scroll .dataHeaderCell").length ?? 0;
    if (!shell || scrollColumns === 0) return { applied: false, original: "" };
    const original = shell.style.getPropertyValue("--data-scroll-grid-columns");
    shell.style.setProperty("--data-scroll-grid-columns", `repeat(${scrollColumns}, minmax(500px, 500px))`);
    return { applied: true, original };
  });
  check(
    tab.id,
    "bug2-horizontal-overflow-fixture",
    overflowFixture.applied === true,
    overflowFixture.applied ? "wide scroll columns applied" : "missing frozen table scroll columns",
  );

  const b2Metrics = await page.evaluate(() => {
    const round = (value) => Math.round(value * 100) / 100;
    const scroller = document.querySelector(".dataTableBodyScroll");
    const frozenPane = document.querySelector(".dataTableFrozenPane");
    const scrollCell = document.querySelector(".dataRow--scrollPane .dataCell");
    if (!scroller || !frozenPane || !scrollCell) return { missing: true };
    const maxScroll = Math.max(0, scroller.scrollWidth - scroller.clientWidth);
    const previousLeft = scroller.scrollLeft;
    scroller.scrollLeft = scroller.scrollWidth;
    const frozenRect = frozenPane.getBoundingClientRect();
    const cellRect = scrollCell.getBoundingClientRect();
    const probeX = Math.min(frozenRect.right - 4, Math.max(frozenRect.left + 4, cellRect.left + 2));
    const probeY = Math.min(frozenRect.bottom - 4, Math.max(frozenRect.top + 48, cellRect.top + 10));
    const hit = document.elementFromPoint(probeX, probeY);
    const hitInScrollCell = Boolean(hit?.closest?.(".dataRow--scrollPane"));
    const overlapsFrozen = cellRect.left < frozenRect.right && cellRect.right > frozenRect.left;
    const actualLeft = scroller.scrollLeft;
    scroller.scrollLeft = previousLeft;
    return {
      missing: false,
      maxScroll: round(maxScroll),
      scrollLeft: round(actualLeft),
      overlapsFrozen,
      hitInScrollCell,
      frozenRight: round(frozenRect.right),
      scrollCellLeft: round(cellRect.left),
    };
  });
  check(
    tab.id,
    "bug2-no-scroll-cell-over-frozen",
    b2Metrics.missing !== true
      && b2Metrics.maxScroll > TOLERANCE
      && b2Metrics.scrollLeft > 0
      && (!b2Metrics.overlapsFrozen || b2Metrics.hitInScrollCell === false),
    `maxScroll ${b2Metrics.maxScroll}, scrollLeft ${b2Metrics.scrollLeft}, scroll cell left ${b2Metrics.scrollCellLeft}, frozen right ${b2Metrics.frozenRight}, hit scroll cell ${b2Metrics.hitInScrollCell}`,
  );

  if (tab.selectable) {
    const b4Metrics = await page.evaluate(() => {
      const row = document.querySelector('.dataRow--frozenPane[data-row-selectable="true"]');
      const scrollRow = row?.dataset.rowId
        ? document.querySelector(`.dataRow--scrollPane[data-row-id="${CSS.escape(row.dataset.rowId)}"]`)
        : document.querySelector('.dataRow--scrollPane[data-row-selectable="true"]');
      if (!row || !scrollRow) return { missing: true };
      const frozenRect = row.getBoundingClientRect();
      const scrollRect = scrollRow.getBoundingClientRect();
      return {
        missing: false,
        startX: frozenRect.right - 12,
        startY: frozenRect.top + Math.min(24, Math.max(8, frozenRect.height / 2)),
        endX: scrollRect.left + Math.min(120, Math.max(16, scrollRect.width / 2)),
        endY: scrollRect.top + Math.min(44, Math.max(18, scrollRect.height - 8)),
        frozenLeft: frozenRect.left,
        frozenRight: frozenRect.right,
      };
    });
    if (b4Metrics.missing === true) {
      check(tab.id, "bug4-marquee-starts-in-frozen", false, "missing selectable frozen/scroll row");
    } else {
      await page.mouse.move(b4Metrics.startX, b4Metrics.startY);
      await page.mouse.down();
      await page.mouse.move(b4Metrics.endX, b4Metrics.endY, { steps: 8 });
      await page.waitForTimeout(80);
      const marqueeMetrics = await page.evaluate(() => {
        const marquee = document.querySelector(".dataTableMarquee");
        const selected = document.querySelectorAll(".dataRow.rowSelected").length;
        if (!marquee) return { missing: true, selected };
        const rect = marquee.getBoundingClientRect();
        return { missing: false, left: rect.left, right: rect.right, selected };
      });
      await page.mouse.up();
      check(
        tab.id,
        "bug4-marquee-starts-in-frozen",
        marqueeMetrics.missing !== true
          && marqueeMetrics.left < b4Metrics.frozenRight - TOLERANCE
          && marqueeMetrics.selected > 0,
        `marquee left ${marqueeMetrics.left}, frozen right ${b4Metrics.frozenRight}, selected row nodes ${marqueeMetrics.selected}`,
      );
    }
  }

  const b6Geometry = await page.evaluate(() => {
    const frozenHeader = document.querySelector(".dataTableHeader--frozen");
    const scrollHeader = document.querySelector(".dataTableHeader--scroll");
    if (!frozenHeader || !scrollHeader) return { missing: true };
    const headers = [
      { name: "frozen", element: frozenHeader },
      { name: "scroll", element: scrollHeader },
    ];
    return {
      missing: false,
      dpr: window.devicePixelRatio || 1,
      points: headers.map(({ name, element }) => {
        const rect = element.getBoundingClientRect();
        const after = getComputedStyle(element, "::after");
        const afterLeft = parseFloat(after.left);
        return {
          name,
          height: parseFloat(after.height),
          bg: after.backgroundColor,
          x: rect.left + (Number.isNaN(afterLeft) ? 0 : afterLeft) + 4,
          y: Math.round(rect.bottom) - 1,
          bgX: rect.left + 4,
          bgY: Math.round(rect.bottom) - 3,
        };
      }),
    };
  });
  let b6Metrics = b6Geometry;
  if (b6Geometry.missing !== true) {
    const screenshot = await page.screenshot({ type: "png" });
    b6Metrics = await page.evaluate(async ({ geometry, b64 }) => {
      const image = new Image();
      image.src = `data:image/png;base64,${b64}`;
      await image.decode();
      const canvas = document.createElement("canvas");
      canvas.width = image.naturalWidth;
      canvas.height = image.naturalHeight;
      const context = canvas.getContext("2d");
      context.drawImage(image, 0, 0);
      const pixel = (x, y) => {
        const scale = geometry.dpr || 1;
        const data = context.getImageData(Math.round(x * scale), Math.round(y * scale), 1, 1).data;
        return [data[0], data[1], data[2], data[3]];
      };
      const delta = (left, right) => Math.max(
        Math.abs(left[0] - right[0]),
        Math.abs(left[1] - right[1]),
        Math.abs(left[2] - right[2]),
      );
      return {
        missing: false,
        points: geometry.points.map((point) => {
          const line = pixel(point.x, point.y);
          const background = pixel(point.bgX, point.bgY);
          return {
            ...point,
            line,
            background,
            delta: delta(line, background),
          };
        }),
      };
    }, { geometry: b6Geometry, b64: screenshot.toString("base64") });
  }
  check(
    tab.id,
    "bug6-frozen-header-rule-visible",
    b6Metrics.missing !== true
      && b6Metrics.points.every((point) => (
        point.height >= 1
        && point.bg !== "transparent"
        && point.bg !== "rgba(0, 0, 0, 0)"
        && point.delta >= 8
      )),
    b6Metrics.missing === true
      ? "missing frozen/scroll header"
      : b6Metrics.points.map((point) => `${point.name} height ${point.height}px bg ${point.bg} pixel delta ${point.delta}`).join(", "),
  );

  if (tab.id === "sessions") {
    const separatorGeometry = await page.evaluate(() => {
      // Skip the row immediately below the synthetic selected row: its leading
      // separator is intentionally hidden to avoid a doubled active-row edge.
      const frozenRow = document.querySelectorAll(".dataRow--frozenPane")[2];
      const scrollRow = frozenRow?.dataset.rowId
        ? document.querySelector(`.dataRow--scrollPane[data-row-id="${CSS.escape(frozenRow.dataset.rowId)}"]`)
        : null;
      if (!frozenRow || !scrollRow) return { missing: true };
      const frozenRect = frozenRow.getBoundingClientRect();
      const scrollRect = scrollRow.getBoundingClientRect();
      const frozenBeforeLeft = parseFloat(getComputedStyle(frozenRow, "::before").left);
      const scrollBeforeLeft = parseFloat(getComputedStyle(scrollRow, "::before").left);
      return {
        missing: false,
        dpr: window.devicePixelRatio || 1,
        rowId: frozenRow.dataset.rowId,
        y: Math.round(frozenRect.top),
        frozenX: frozenRect.left + (Number.isNaN(frozenBeforeLeft) ? 0 : frozenBeforeLeft) + 80,
        scrollX: scrollRect.left + (Number.isNaN(scrollBeforeLeft) ? 0 : scrollBeforeLeft) + 80,
      };
    });
    let separatorMetrics = separatorGeometry;
    if (separatorGeometry.missing !== true) {
      const screenshot = await page.screenshot({ type: "png" });
      separatorMetrics = await page.evaluate(async ({ geometry, b64 }) => {
        const image = new Image();
        image.src = `data:image/png;base64,${b64}`;
        await image.decode();
        const canvas = document.createElement("canvas");
        canvas.width = image.naturalWidth;
        canvas.height = image.naturalHeight;
        const context = canvas.getContext("2d");
        context.drawImage(image, 0, 0);
        const pixel = (x, y) => {
          const scale = geometry.dpr || 1;
          return [...context.getImageData(Math.round(x * scale), Math.round(y * scale), 1, 1).data];
        };
        const channelDelta = (left, right) => Math.max(
          Math.abs(left[0] - right[0]),
          Math.abs(left[1] - right[1]),
          Math.abs(left[2] - right[2]),
        );
        const bestLinePixel = (x) => {
          const background = pixel(x, geometry.y + 3);
          return [-1, 0, 1, 2]
            .map((offset) => {
              const value = pixel(x, geometry.y + offset);
              return { value, contrast: channelDelta(value, background) };
            })
            .reduce((best, item) => (item.contrast > best.contrast ? item : best));
        };
        const frozen = bestLinePixel(geometry.frozenX);
        const scroll = bestLinePixel(geometry.scrollX);
        return {
          missing: false,
          rowId: geometry.rowId,
          frozen: frozen.value,
          scroll: scroll.value,
          frozenContrast: frozen.contrast,
          scrollContrast: scroll.contrast,
          delta: channelDelta(frozen.value, scroll.value),
        };
      }, { geometry: separatorGeometry, b64: screenshot.toString("base64") });
    }
    check(
      tab.id,
      "bug6-frozen-row-separator-not-doubled",
      separatorMetrics.missing !== true
        && separatorMetrics.frozenContrast >= 5
        && separatorMetrics.scrollContrast >= 8
        && separatorMetrics.frozenContrast <= separatorMetrics.scrollContrast + 2,
      separatorMetrics.missing === true
        ? "missing paired frozen/scroll row"
        : `row ${separatorMetrics.rowId}, frozen ${separatorMetrics.frozen.join(",")} contrast ${separatorMetrics.frozenContrast}, scroll ${separatorMetrics.scroll.join(",")} contrast ${separatorMetrics.scrollContrast}, delta ${separatorMetrics.delta}`,
    );
  }

  const b7Metrics = await page.evaluate(async () => {
    const row = document.querySelector(".dataRow--frozenPane");
    const paired = row?.dataset.rowId
      ? document.querySelector(`.dataRow--scrollPane[data-row-id="${CSS.escape(row.dataset.rowId)}"]`)
      : document.querySelector(".dataRow--scrollPane");
    if (!row || !paired) return { missing: true };
    row.classList.add("rowHover");
    paired.classList.add("rowHover");
    await new Promise((resolve) => window.setTimeout(resolve, 150));
    const frozenAfter = getComputedStyle(row, "::after");
    const scrollAfter = getComputedStyle(paired, "::after");
    const result = {
      missing: false,
      frozenTopRight: frozenAfter.borderTopRightRadius,
      frozenBottomRight: frozenAfter.borderBottomRightRadius,
      scrollTopLeft: scrollAfter.borderTopLeftRadius,
      scrollBottomLeft: scrollAfter.borderBottomLeftRadius,
      frozenBg: frozenAfter.backgroundColor,
      scrollBg: scrollAfter.backgroundColor,
    };
    row.classList.remove("rowHover");
    paired.classList.remove("rowHover");
    return result;
  });
  check(
    tab.id,
    "bug7-hover-seam-no-inner-radius",
    b7Metrics.missing !== true
      && parseFloat(b7Metrics.frozenTopRight) === 0
      && parseFloat(b7Metrics.frozenBottomRight) === 0
      && parseFloat(b7Metrics.scrollTopLeft) === 0
      && parseFloat(b7Metrics.scrollBottomLeft) === 0
      && b7Metrics.frozenBg === b7Metrics.scrollBg,
    `radii frozen right ${b7Metrics.frozenTopRight}/${b7Metrics.frozenBottomRight}, scroll left ${b7Metrics.scrollTopLeft}/${b7Metrics.scrollBottomLeft}, bg ${b7Metrics.frozenBg} vs ${b7Metrics.scrollBg}`,
  );

  const b8Metrics = await page.evaluate(() => {
    const row = document.querySelector(".dataRow--frozenPane");
    const cell = row?.querySelector(".dataCell[data-frozen]");
    if (!row || !cell) return { missing: true };
    row.classList.add("rowHover");
    const afterTop = parseFloat(getComputedStyle(row, "::after").top);
    const result = {
      missing: false,
      afterTop,
      cellBg: getComputedStyle(cell).backgroundColor,
      overlayBg: getComputedStyle(row, "::after").backgroundColor,
    };
    row.classList.remove("rowHover");
    return result;
  });
  check(
    tab.id,
    "bug8-frozen-active-bg-inset",
    b8Metrics.missing !== true
      && Math.abs(b8Metrics.afterTop - 1) <= TOLERANCE
      && (b8Metrics.cellBg === "transparent" || b8Metrics.cellBg === "rgba(0, 0, 0, 0)"),
    `after top ${b8Metrics.afterTop}, cell bg ${b8Metrics.cellBg}, overlay bg ${b8Metrics.overlayBg}`,
  );

  const b10Metrics = await page.evaluate(() => {
    const scroller = document.querySelector(".dataTableBodyScroll");
    const frozenHeader = document.querySelector(".dataTableHeader--frozen");
    if (!scroller || !frozenHeader) return { missing: true };
    const maxScroll = Math.max(0, scroller.scrollWidth - scroller.clientWidth);
    const previousLeft = scroller.scrollLeft;
    scroller.scrollLeft = scroller.scrollWidth;
    const rect = frozenHeader.getBoundingClientRect();
    const hit = document.elementFromPoint(rect.left + Math.min(80, rect.width / 2), rect.top + rect.height / 2);
    const result = {
      missing: false,
      maxScroll: Math.round(maxScroll * 100) / 100,
      scrollLeft: scroller.scrollLeft,
      hitScrollHeader: Boolean(hit?.closest?.(".dataTableHeader--scroll")),
      hitFrozenHeader: Boolean(hit?.closest?.(".dataTableHeader--frozen")),
      hitClass: `${hit?.className ?? ""}`,
    };
    scroller.scrollLeft = previousLeft;
    return result;
  });
  check(
    tab.id,
    "bug10-scroll-header-not-over-frozen",
    b10Metrics.missing !== true
      && b10Metrics.maxScroll > TOLERANCE
      && b10Metrics.scrollLeft > 0
      && b10Metrics.hitScrollHeader === false
      && b10Metrics.hitFrozenHeader === true,
    `maxScroll ${b10Metrics.maxScroll}, scrollLeft ${b10Metrics.scrollLeft}, hit frozen ${b10Metrics.hitFrozenHeader}, hit scroll ${b10Metrics.hitScrollHeader}, hit ${b10Metrics.hitClass}`,
  );

  const b11Metrics = await page.evaluate(() => {
    const scroller = document.querySelector(".dataTableBodyScroll");
    const frozenHeader = document.querySelector(".dataTableHeader--frozen");
    if (!scroller || !frozenHeader) return { missing: true };
    const previousTop = scroller.scrollTop;
    const topBefore = frozenHeader.getBoundingClientRect().top;
    scroller.scrollTop = Math.min(220, scroller.scrollHeight - scroller.clientHeight);
    const topAfter = frozenHeader.getBoundingClientRect().top;
    const rect = frozenHeader.getBoundingClientRect();
    const hit = document.elementFromPoint(rect.left + Math.min(80, rect.width / 2), rect.top + rect.height / 2);
    const result = {
      missing: false,
      didScroll: scroller.scrollTop > 0,
      topBefore,
      topAfter,
      hitFrozenHeader: Boolean(hit?.closest?.(".dataTableHeader--frozen")),
      hitFrozenRow: Boolean(hit?.closest?.(".dataRow--frozenPane")),
    };
    scroller.scrollTop = previousTop;
    return result;
  });
  check(
    tab.id,
    "bug11-frozen-header-pinned-over-body",
    b11Metrics.missing !== true
      && b11Metrics.didScroll === true
      && Math.abs(b11Metrics.topBefore - b11Metrics.topAfter) <= TOLERANCE
      && b11Metrics.hitFrozenHeader === true
      && b11Metrics.hitFrozenRow === false,
    `top ${b11Metrics.topBefore} vs ${b11Metrics.topAfter}, hit header ${b11Metrics.hitFrozenHeader}, hit row ${b11Metrics.hitFrozenRow}`,
  );

  const b12Metrics = await page.evaluate(() => {
    const round = (value) => Math.round(value * 100) / 100;
    const frozenRows = [...document.querySelectorAll(".dataRow--frozenPane")].slice(0, 10);
    const pairs = frozenRows.map((frozenRow) => {
      const id = frozenRow.dataset.rowId ?? "";
      const scrollRow = document.querySelector(`.dataRow--scrollPane[data-row-id="${CSS.escape(id)}"]`);
      if (!scrollRow) return null;
      const frozenRect = frozenRow.getBoundingClientRect();
      const scrollRect = scrollRow.getBoundingClientRect();
      return {
        id,
        topDelta: round(Math.abs(frozenRect.top - scrollRect.top)),
        heightDelta: round(Math.abs(frozenRect.height - scrollRect.height)),
        frozenHeight: round(frozenRect.height),
        scrollHeight: round(scrollRect.height),
      };
    }).filter(Boolean);
    const worstTop = pairs.reduce((worst, pair) => (pair.topDelta > worst.topDelta ? pair : worst), { topDelta: 0, id: "" });
    const worstHeight = pairs.reduce((worst, pair) => (pair.heightDelta > worst.heightDelta ? pair : worst), { heightDelta: 0, id: "" });
    return {
      missing: pairs.length === 0,
      count: pairs.length,
      maxTopDelta: worstTop.topDelta,
      maxHeightDelta: worstHeight.heightDelta,
      topRowId: worstTop.id,
      heightRowId: worstHeight.id,
      heightSample: worstHeight.frozenHeight == null
        ? ""
        : `${worstHeight.frozenHeight}/${worstHeight.scrollHeight}`,
    };
  });
  check(
    tab.id,
    "bug12-frozen-scroll-row-sync",
    b12Metrics.missing !== true
      && b12Metrics.maxTopDelta <= TOLERANCE
      && b12Metrics.maxHeightDelta <= TOLERANCE,
    `pairs ${b12Metrics.count}, max top delta ${b12Metrics.maxTopDelta} (${b12Metrics.topRowId}), max height delta ${b12Metrics.maxHeightDelta} (${b12Metrics.heightRowId}) heights ${b12Metrics.heightSample}`,
  );

  if (tab.id === "sessions") {
    const b12ButtonMetrics = await page.evaluate(() => {
      const round = (value) => Math.round(value * 100) / 100;
      const button = document.querySelector(".copyableSessionId.inSessionTable .copyableSessionIdButton");
      const row = button?.closest(".dataRow--frozenPane");
      const paired = row?.dataset.rowId
        ? document.querySelector(`.dataRow--scrollPane[data-row-id="${CSS.escape(row.dataset.rowId)}"]`)
        : null;
      if (!button || !row || !paired) return { missing: true };
      const buttonRect = button.getBoundingClientRect();
      const rowRect = row.getBoundingClientRect();
      const pairedRect = paired.getBoundingClientRect();
      return {
        missing: false,
        buttonHeight: round(buttonRect.height),
        buttonWidth: round(buttonRect.width),
        frozenHeight: round(rowRect.height),
        scrollHeight: round(pairedRect.height),
      };
    });
    check(
      tab.id,
      "bug12-session-copy-button-compact",
      b12ButtonMetrics.missing !== true
        && b12ButtonMetrics.buttonHeight <= 16 + TOLERANCE
        && b12ButtonMetrics.buttonWidth <= 16 + TOLERANCE
        && Math.abs(b12ButtonMetrics.frozenHeight - b12ButtonMetrics.scrollHeight) <= TOLERANCE,
      `button ${b12ButtonMetrics.buttonWidth}x${b12ButtonMetrics.buttonHeight}, row heights ${b12ButtonMetrics.frozenHeight}/${b12ButtonMetrics.scrollHeight}`,
    );

    const copyButton = page.locator(".dataRow--frozenPane .copyableSessionIdButton").first();
    if (await copyButton.count()) {
      const sessionId = copyButton.locator("xpath=..");
      const sessionTitle = copyButton.locator("xpath=ancestor::*[contains(concat(' ', normalize-space(@class), ' '), ' dataRow--frozenPane ')]").locator(".sessionTitleText");

      await sessionTitle.hover();
      await page.waitForTimeout(180);
      const rowHoverOpacity = await copyButton.evaluate((node) => getComputedStyle(node).opacity);

      await sessionId.locator("code").hover();
      await page.waitForTimeout(180);
      const idHoverOpacity = await copyButton.evaluate((node) => getComputedStyle(node).opacity);

      check(
        tab.id,
        "bug14-session-copy-button-hover-scope",
        rowHoverOpacity === "0" && idHoverOpacity === "1",
        `title hover opacity ${rowHoverOpacity}, session ID hover opacity ${idHoverOpacity}`,
      );
    } else {
      check(tab.id, "bug14-session-copy-button-hover-scope", false, "copy button missing");
    }
  }

  const groupButton = page.locator(".dataTableHeader .dataHeaderGroupButton").first();
  if (await groupButton.count()) {
    const clearSelectionButton = page.getByRole("button", { name: "Clear selection", exact: true });
    if (await clearSelectionButton.count()) await clearSelectionButton.click();
    await groupButton.click();
    await page.waitForFunction(
      () => Boolean(
        document.querySelector(".dataTableFrozenPane .dataGroup .dataRow--frozenPane")
        && document.querySelector(".dataTableScrollPane .dataGroup .dataRow--scrollPane"),
      ),
      { timeout: 2000 },
    ).catch(() => {});
    const groupedMetrics = await page.evaluate(() => {
      const round = (value) => Math.round(value * 100) / 100;
      const frozenGroup = document.querySelector(".dataTableFrozenPane .dataGroup");
      const scrollGroup = document.querySelector(".dataTableScrollPane .dataGroup");
      const frozenHeading = frozenGroup?.querySelector(".sectionHeading");
      const scrollSpacer = scrollGroup?.querySelector(".dataSplitGroupSpacer");
      const frozenRow = frozenGroup?.querySelector(".dataRow--frozenPane");
      const scrollRow = frozenRow?.dataset.rowId
        ? document.querySelector(`.dataTableScrollPane .dataRow--scrollPane[data-row-id="${CSS.escape(frozenRow.dataset.rowId)}"]`)
        : null;
      if (!frozenGroup || !scrollGroup || !frozenHeading || !scrollSpacer || !frozenRow || !scrollRow) {
        return { missing: true };
      }
      const frozenHeadingRect = frozenHeading.getBoundingClientRect();
      const scrollSpacerRect = scrollSpacer.getBoundingClientRect();
      const frozenRowRect = frozenRow.getBoundingClientRect();
      const scrollRowRect = scrollRow.getBoundingClientRect();
      const isVisible = (backgroundColor) => backgroundColor !== "transparent" && backgroundColor !== "rgba(0, 0, 0, 0)";
      return {
        missing: false,
        rowId: frozenRow.dataset.rowId,
        headingHeight: round(frozenHeadingRect.height),
        spacerHeight: round(scrollSpacerRect.height),
        rowTopDelta: round(Math.abs(frozenRowRect.top - scrollRowRect.top)),
        rowHeightDelta: round(Math.abs(frozenRowRect.height - scrollRowRect.height)),
        frozenRowTop: round(frozenRowRect.top),
        scrollRowTop: round(scrollRowRect.top),
        frozenSeparatorVisible: isVisible(getComputedStyle(frozenRow, "::before").backgroundColor),
        scrollSeparatorVisible: isVisible(getComputedStyle(scrollRow, "::before").backgroundColor),
      };
    });
    check(
      tab.id,
      "bug13-grouped-frozen-scroll-row-sync",
      groupedMetrics.missing !== true
        && groupedMetrics.headingHeight > 0
        && Math.abs(groupedMetrics.headingHeight - groupedMetrics.spacerHeight) <= TOLERANCE
        && groupedMetrics.rowTopDelta <= TOLERANCE
        && groupedMetrics.rowHeightDelta <= TOLERANCE,
      groupedMetrics.missing === true
        ? "missing grouped frozen/scroll rows"
        : `heading ${groupedMetrics.headingHeight}, spacer ${groupedMetrics.spacerHeight}, row ${groupedMetrics.rowId} top ${groupedMetrics.frozenRowTop}/${groupedMetrics.scrollRowTop} delta ${groupedMetrics.rowTopDelta}, height delta ${groupedMetrics.rowHeightDelta}`,
    );
    check(
      tab.id,
      "bug18-grouped-row-separators-visible",
      groupedMetrics.missing !== true
        && groupedMetrics.frozenSeparatorVisible === true
        && groupedMetrics.scrollSeparatorVisible === true,
      groupedMetrics.missing === true
        ? "missing grouped frozen/scroll rows"
        : `frozen separator ${groupedMetrics.frozenSeparatorVisible}, scroll separator ${groupedMetrics.scrollSeparatorVisible}`,
    );
    const activeGroupButton = page.locator(".dataTableHeader .dataHeaderGroupButton.activeGroup").first();
    if (await activeGroupButton.count()) await activeGroupButton.click({ force: true });
    await page.waitForTimeout(40);
  } else {
    check(tab.id, "bug13-grouped-frozen-scroll-row-sync", true, "no groupable header");
  }
  await page.evaluate(({ original }) => {
    const shell = document.querySelector(".dataTableShell--frozen");
    if (!shell) return;
    if (original) shell.style.setProperty("--data-scroll-grid-columns", original);
    else shell.style.removeProperty("--data-scroll-grid-columns");
  }, overflowFixture);
}

const { chromium } = loadPlaywrightCore();
let server;
let browser;

async function cleanup() {
  await browser?.close().catch(() => {});
  await server?.close().catch(() => {});
}

for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, async () => {
    await cleanup();
    process.kill(process.pid, signal);
  });
}

try {
  server = await createServer({
    root: appDir,
    appType: "spa",
    logLevel: "error",
    server: { host: "127.0.0.1", port: PORT, strictPort: true },
  });
  await server.listen();

  browser = await chromium.launch({
    headless: true,
    executablePath: chromiumExecutablePath() ?? undefined,
  });

  const page = await browser.newPage({ viewport: { width: 1440, height: 1024 } });
  page.on("pageerror", (error) => {
    const detail = error instanceof Error ? error.stack ?? error.message : String(error);
    runtimePageErrors.push(detail);
    writeStderr("browser page error:", detail);
  });
  page.on("console", (message) => {
    if (message.type() === "error") writeStderr("browser console error:", message.text());
  });

  await page.addInitScript((report) => {
    const callbacks = new Map();
    const daemonEventQueue = [];
    const daemonEventWaiters = [];
    let nextCallbackId = 1;
    let sessionScanHandler = null;
    let sessionsListReleased = false;
    let skillUpdateAttempts = 0;
    const unhandledCommands = [];
    const emitDaemonEvent = (event) => {
      const resolve = daemonEventWaiters.shift();
      if (resolve) resolve(event);
      else daemonEventQueue.push(event);
    };
    const invokeDomainCommand = async (command, args) => {
      if (command === "log_event") return null;
      if (command === "plugin:event|listen") {
        sessionScanHandler = callbacks.get(args.handler);
        return 1;
      }
      if (command === "plugin:event|unlisten") return null;
      if (command === "scan") return report;
      if (command === "settings_get") {
        return {
          appearance: "dark",
          lightTheme: "gruvbox",
          darkTheme: "gruvbox",
          terminal: "auto",
          editor: "vscode",
          developerMode: false,
          additionalSessionRoots: [],
          configProfiles: {},
        };
      }
      if (command === "analytics_revision") return 1;
      if (command === "bundled_skill_status") {
        return {
          name: "tendi",
          target: "codex",
          installed: true,
          current: true,
          promptHandled: true,
          shouldPrompt: false,
        };
      }
      if (command === "session_skill_index_status") {
        return { state: "idle", indexed: 0, total: 0, failed: 0 };
      }
      if (command === "session_skill_index_run") return { started: true };
      if (command === "session_skill_links") return [];
      if (command === "overview_count") {
        const counts = {
          skills: report.skills.skills.length,
          prompts: report.prompts.prompts.length,
          sessions: report.sessions.sessions.length,
          rules: report.rules.rules.length,
          hooks: report.hooks.hooks.length,
          mcp: report.mcp.servers.length,
        };
        return {
          count: counts[args?.domain] ?? 0,
          secondaryCount: args?.domain === "skills" || args?.domain === "hooks" ? 1 : 0,
        };
      }
      if (command === "analytics_overview") {
        const usage = {
          inputTokens: 10,
          cachedInputTokens: 2,
          cacheWriteInputTokens: 1,
          outputTokens: 8,
          reasoningOutputTokens: 0,
          totalTokens: 19,
        };
        const runs = { started: 1, completed: 1, unclosed: 0, totalMs: 100, maxMs: 100 };
        return {
          revision: 1,
          generatedAt: "2026-06-29T19:00:00Z",
          daysRequested: args?.days ?? 30,
          rankDays: args?.rankDays ?? 30,
          coverage: {
            first: "2026-06-29",
            last: "2026-06-29",
            totalSessions: 1,
            analyzedSessions: 1,
            indexingSessions: 0,
          },
          capabilities: [{
            agent: "codex",
            tokenUsage: true,
            reasoningTokens: false,
            explicitRuns: true,
            rateLimitHistory: false,
          }],
          summary: {
            usage,
            responses: 1,
            sessions: 1,
            runs,
            aborted: 0,
            abortedRate: 0,
            compacted: 0,
            compactedSessions: 0,
          },
          days: [{
            date: "2026-06-29",
            usage,
            responses: 1,
            sessions: 1,
            sessionsByAgent: { codex: 1 },
            runs,
            aborted: 0,
            compacted: 0,
            models: [{ model: "mock-model", totalTokens: 19 }],
            tools: [],
            skills: [],
            rateLimits: {},
          }],
          tools: [],
          skills: [],
          warnings: [],
        };
      }
      if (command === "agent_configs_list") {
        return [{
          agent: "codex",
          label: "Codex",
          path: "/tmp/tendi-codex.json",
          format: "json",
          exists: true,
        }];
      }
      if (command === "agent_config_watch") {
        return { path: args?.path ?? "/tmp/tendi-codex.json" };
      }
      if (command === "agent_config_read") {
        return {
          path: args?.path ?? "/tmp/tendi-codex.json",
          content: "{}",
          sha256: "config-sha",
          exists: true,
        };
      }
      if (command === "rule_file_read") {
        return { content: "# Mock rule\n", sha256: args?.path ? `rule-${args.path}` : "rule-sha" };
      }
      if (command === "hook_source_read") {
        return { content: "echo mock-hook\n", source_line: 1, path: args?.path ?? "" };
      }
      if (command === "terminal_apps_list") return [{ id: "auto", label: "Auto", available: true }];
      if (command === "cli_status") {
        return {
          state: "installed",
          supported: true,
          commandPath: "/usr/local/bin/tendi",
          bundledPath: null,
          pathConfigured: true,
          currentTarget: null,
          detail: "",
        };
      }
      if (command === "skills_refresh") return { skills: report.skills.skills, updateCheck: "completed" };
      if (command === "skills_list") return report.skills.skills;
      if (command === "skills_backup_status") return { config: null, statuses: [], versions: [] };
      if (command === "skills_backup_sync") return null;
      if (command === "projects_list") return [
        { id: "project-1", name: "project-1", rootPath: "/Users/dev/.cursor/projects/project-1" },
        { id: "project-2", name: "project-2", rootPath: "/Users/dev/.cursor/projects/project-2" },
      ];
      if (command === "session_projects_list") return [];
      if (command === "project_scan_scopes_list") return [];
      if (command === "app_icon_set") return null;
      if (command === "plugin:window|set_icon") return null;
      if (command === "check_for_updates") return { status: "up-to-date" };
      if (command === "skills_targets") {
        return [
          { id: "claude", displayName: "Claude", supportsGlobal: true },
          { id: "codex", displayName: "Codex", supportsGlobal: true },
          { id: "cursor", displayName: "Cursor", supportsGlobal: true },
        ];
      }
      if (command === "prompts_list") return report.prompts.prompts;
      if (command === "sessions_list") {
        if (sessionsListReleased) return report.sessions.sessions;
        return new Promise((resolve) => {
          window.__releaseSessionsList = () => {
            sessionsListReleased = true;
            resolve(report.sessions.sessions);
          };
        });
      }
      if (command === "sessions_scan_start") {
        queueMicrotask(() => {
          const recent = { id: 1, event: "sessions://scan", payload: { generation: 1, phase: "recent", upserts: [], deleted: [], scanned: 0, complete: true } };
          const backfill = { id: 1, event: "sessions://scan", payload: { generation: 1, phase: "backfill", upserts: [], deleted: [], scanned: report.sessions.sessions.length, complete: true } };
          emitDaemonEvent(recent);
          emitDaemonEvent(backfill);
          sessionScanHandler?.(recent);
          sessionScanHandler?.(backfill);
        });
        return 1;
      }
      if (command === "session_transcript") {
        return {
          items: Array.from({ length: 5 }, (_, index) => ([
            {
              type: "user",
              body: `Locator prompt ${index + 1}`,
              time: `10:0${index}`,
            },
            {
              type: "assistant",
              body: `Locator response ${index + 1}`,
              time: `10:0${index}`,
            },
          ])).flat(),
          locatorItems: Array.from({ length: 5 }, (_, index) => ({
            index: index * 2,
            label: `Locator prompt ${index + 1}`,
            response: `Locator response ${index + 1}`,
          })),
          warnings: [],
          nextCursor: null,
          done: true,
        };
      }
      if (command === "sessions_search") {
        return report.sessions.sessions.map((session, index) => ({
          ...session,
          search_score: report.sessions.sessions.length - index,
          search_snippet: "Matched ⟦session⟧ text",
        }));
      }
      if (command === "rules_list") return report.rules.rules;
      if (command === "hooks_list") return report.hooks.hooks;
      if (command === "mcp_list") return report.mcp.servers;
      if (command === "agents_list") return report.agents.agents;
      if (command === "skills_updates") return [];
      if (command === "skills_update_many") {
        if (args?.dryRun) {
          return {
            plan: {
              git_updates: [{
                files: [{ path: "skills/alpha-skill/SKILL.md", before: "old", after: "new" }],
              }],
            },
          };
        }
        skillUpdateAttempts += 1;
        if (skillUpdateAttempts === 1) throw new Error("mock update failed");
        report.skills.skills = report.skills.skills.map((skill) => (
          skill.name === "alpha-skill" ? { ...skill, update_status: "checkable" } : skill
        ));
        return { applied: true };
      }
      unhandledCommands.push(command);
      throw new Error(`Unhandled alignment e2e mock command: ${command}`);
    };
    window.__TAURI_EVENT_PLUGIN_INTERNALS__ = { unregisterListener: () => {} };
    window.__TAURI_INTERNALS__ = {
      invoke: async (command, args) => {
        if (command === "daemon_invoke") {
          const request = args?.request ?? {};
          return { ok: true, result: await invokeDomainCommand(request.command, request.args) };
        }
        if (command === "daemon_subscribe_events") return 1;
        if (command === "daemon_next_event") {
          if (daemonEventQueue.length > 0) return daemonEventQueue.shift();
          return new Promise((resolve) => daemonEventWaiters.push(resolve));
        }
        if (command === "daemon_unsubscribe_events") return null;
        return invokeDomainCommand(command, args);
      },
      transformCallback: (callback) => {
        const id = nextCallbackId++;
        callbacks.set(id, callback);
        return id;
      },
      unregisterCallback: (id) => callbacks.delete(id),
      metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
    };
    window.__alignmentMockDiagnostics = () => ({ unhandledCommands: [...unhandledCommands] });
  }, buildReport());

  await page.goto(`http://127.0.0.1:${PORT}/`, { waitUntil: "domcontentloaded", timeout: 10000 });
  await page.getByRole("button", { name: "Skills", exact: true }).waitFor({ state: "visible", timeout: 5000 });
  await page.getByRole("heading", { name: "Overview", exact: true }).waitFor();
  await page.locator(".overviewPage").waitFor();
  await runPageHeaderChecks(page, "overview", "Overview", false);
  const navLabels = await page.locator(".navItem").evaluateAll((buttons) => buttons.map((button) => button.textContent?.trim() ?? ""));
  check(
    "navigation",
    "all-pages-listed",
    JSON.stringify(navLabels) === JSON.stringify(["Overview", "Skills", "Backup", "Sessions", "Rules", "MCP", "Hooks", "Prompts", "Config", "Settings"]),
    navLabels.join(", "),
  );
  await page.getByRole("button", { name: "Skills", exact: true }).click();
  await page.getByRole("heading", { name: "Skills" }).waitFor();
  await page.waitForFunction(
    () => getComputedStyle(document.querySelector('.navItem[aria-label="Skills"]')).fontWeight === "550",
    { timeout: 500 },
  );
  const navState = await page.evaluate(() => {
    const buttons = [...document.querySelectorAll(".navItem")];
    const active = document.querySelector('.navItem[aria-label="Skills"]');
    return {
      active: active?.classList.contains("active") === true && active?.getAttribute("aria-current") === "page",
      fontWeights: [...new Set(buttons.map((button) => getComputedStyle(button).fontWeight))],
      activeFontWeight: active ? getComputedStyle(active).fontWeight : "",
    };
  });
  check("navigation", "active-state-after-click", navState.active, "Skills should be the active navigation item");
  check("navigation", "active-tab-weight", navState.activeFontWeight === "550", `active weight: ${navState.activeFontWeight}`);
  check("navigation", "inactive-tab-weight", navState.fontWeights.includes("400"), `weights: ${navState.fontWeights.join(", ")}`);

  writeStdout("\n== installed agent filter ==");
  await page.getByRole("combobox", { name: "Agent filter" }).click();
  await page.waitForFunction(
    () => [...document.querySelectorAll('[role="option"]')].some((option) => option.textContent?.includes("Claude")),
    { timeout: 2000 },
  ).catch(() => {});
  const agentOptions = await page.getByRole("option").allTextContents();
  check("sidebar", "installed-agent-filter", agentOptions.some((label) => label.includes("Claude")), `options: ${agentOptions.join(", ")}`);
  check("sidebar", "uninstalled-agent-filter", !agentOptions.some((label) => label.includes("Ghost Agent")), `options: ${agentOptions.join(", ")}`);
  await page.keyboard.press("Escape");

  writeStdout("\n== req8 non-frozen fixture ==");
  await runReq8NonFrozenFixture(page);

  for (const tab of tabs) {
    writeStdout(`\n== ${tab.heading} ==`);
    if (tab.id !== "skills") {
      // Dismiss any overlay (e.g. a row click that opened an editor dialog)
      // left over from the previous tab before navigating.
      await page.keyboard.press("Escape").catch(() => {});
      await page.getByRole("button", { name: tab.nav, exact: true }).click();
    }
    await page.getByRole("heading", { name: tab.heading, exact: true }).waitFor();
    await runPageHeaderChecks(page, tab.id, tab.heading, tab.compact);
    if (tab.id === "sessions") {
      const initialDetailPanels = await page.locator(".transcriptPanelHost").count();
      check(
        tab.id,
        "bug-session-detail-no-fallback-flash",
        initialDetailPanels === 0,
        `${initialDetailPanels} detail panels before sessions load`,
      );
      await page.evaluate(() => window.__releaseSessionsList?.());
    }
    await page.locator(".dataRow").first().waitFor();

    if (tab.id === "sessions") {
      const searchInput = page.getByPlaceholder("Search sessions");
      await searchInput.fill("session");
      await page.locator(".sessionSearchSnippet").first().waitFor();
      const workspaceSortButton = page.locator('.dataHeaderCell[data-column="project"] [aria-label="Sort by Project"]');
      const workspaceGroupButton = page.locator('.dataHeaderCell[data-column="project"] [aria-label="Group by Project"]');
      const workspaceSortEnabled = await workspaceSortButton.count() === 1;
      if (workspaceSortEnabled) {
        await workspaceSortButton.click();
        await page.waitForTimeout(40);
      }
      const workspaceGroupInactive = await workspaceGroupButton.getAttribute("aria-pressed") === "false";
      const workspaceValues = await page.locator('.dataRow--scrollPane .dataCell[data-column="project"]').allTextContents();
      const expectedWorkspaceValues = [...workspaceValues].sort((left, right) => left.localeCompare(right));
      check(
        tab.id,
        "bug15-header-sort-and-group-actions",
        workspaceSortEnabled
          && workspaceGroupInactive
          && JSON.stringify(workspaceValues) === JSON.stringify(expectedWorkspaceValues),
        `sort button ${workspaceSortEnabled ? "enabled" : "missing"}, group inactive ${workspaceGroupInactive}, workspaces ${workspaceValues.join(", ")}`,
      );
      const headerControlSpacing = await page.locator('.dataHeaderCell[data-column="project"]').evaluate((header) => {
        const round = (value) => Math.round(value * 100) / 100;
        const label = header.querySelector(".dataHeaderSortLabelButton > span:first-child");
        const sortIcon = header.querySelector(".dataHeaderSortIcon");
        const sortButton = header.querySelector(".dataHeaderSortLabelButton");
        const groupButton = header.querySelector(".dataHeaderGroupButton");
        if (!label || !sortIcon || !sortButton || !groupButton) return { missing: true };
        const labelRect = label.getBoundingClientRect();
        const sortIconRect = sortIcon.getBoundingClientRect();
        const sortButtonRect = sortButton.getBoundingClientRect();
        const groupButtonRect = groupButton.getBoundingClientRect();
        return {
          missing: false,
          sortGap: round(sortIconRect.left - labelRect.right),
          groupGap: round(groupButtonRect.left - sortButtonRect.right),
          groupWidth: round(groupButtonRect.width),
        };
      });
      check(
        tab.id,
        "bug16-header-control-spacing",
        headerControlSpacing.missing !== true
          && Math.abs(headerControlSpacing.sortGap - headerControlSpacing.groupGap) <= TOLERANCE
          && headerControlSpacing.groupWidth === 16,
        headerControlSpacing.missing === true
          ? "missing header controls"
          : `sort gap ${headerControlSpacing.sortGap}px, group gap ${headerControlSpacing.groupGap}px, group width ${headerControlSpacing.groupWidth}px`,
      );
      await workspaceSortButton.hover();
      const groupOpacityOnHover = await workspaceGroupButton.evaluate((button) => getComputedStyle(button).opacity);
      await page.locator(".sessionListBody").hover({ position: { x: 4, y: 4 } });
      const groupOpacityAtRest = await workspaceGroupButton.evaluate((button) => getComputedStyle(button).opacity);
      check(
        tab.id,
        "bug17-group-control-visibility",
        groupOpacityAtRest === "0" && groupOpacityOnHover === "1",
        `rest opacity ${groupOpacityAtRest}, hover opacity ${groupOpacityOnHover}`,
      );
      await searchInput.fill("");
      await page.locator(".copyableSessionId.inSessionTable").first().waitFor();
    }

    if (tab.id === "skills") {
      await page.getByRole("button", { name: "View update for alpha-skill" }).click();
      const updateDialog = page.locator(".confirmDialogPanel");
      await updateDialog.getByRole("button", { name: "Apply updates" }).click();
      const updateFailure = updateDialog.getByRole("alert");
      await updateFailure.waitFor();
      check(
        tab.id,
        "bug-update-failure-keeps-dialog-open",
        await updateDialog.isVisible() && (await updateFailure.textContent())?.includes("mock update failed"),
        `dialog visible ${await updateDialog.isVisible()}, error ${await updateFailure.textContent()}`,
      );
      await updateDialog.getByRole("button", { name: "Apply updates" }).click();
      await updateDialog.waitFor({ state: "hidden" });
      check(
        tab.id,
        "bug-update-success-clears-availability",
        await page.getByRole("button", { name: "View update for alpha-skill" }).count() === 0,
        "update badge should disappear after the retry succeeds",
      );

      const originGroupButton = page.locator('.dataHeaderCell[data-column="origin"] .dataHeaderGroupButton');
      const visibilityGroupButton = page.locator('.dataHeaderCell[data-column="visibility"] .dataHeaderGroupButton');
      await visibilityGroupButton.click();
      await page.getByRole("button", { name: "Refresh skills and check updates" }).click();
      await page.waitForTimeout(40);
      const visibilityGroupingPersisted = await visibilityGroupButton.getAttribute("aria-pressed");
      check(
        tab.id,
        "bug-grouping-persists-after-refresh",
        visibilityGroupingPersisted === "true",
        `visibility grouping aria-pressed ${visibilityGroupingPersisted}`,
      );
      if (visibilityGroupingPersisted === "true") {
        await originGroupButton.click();
        await page.waitForTimeout(40);
      }
    }

    // --- req3: header(s) align with the first row text column --------------
    // Measure the actual rendered TEXT (leftmost glyph) of the title, the list
    // header label and the row's first text cell — not the container box — so
    // padding inside e.g. a sort button cannot hide a misalignment.
    const layout = await page.evaluate((listHeader) => {
      const round = (value) => Math.round(value * 100) / 100;
      // Left edge of the first visible glyph inside `el`. Walks to the first
      // non-empty TEXT NODE and measures it directly, so element boxes (e.g. a
      // width:100% sort button with inner padding) cannot mask where the text
      // actually starts.
      const textLeft = (el) => {
        if (!el) return null;
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        let node;
        while ((node = walker.nextNode())) {
          if (!node.textContent.trim()) continue;
          const range = document.createRange();
          range.selectNodeContents(node);
          for (const rect of range.getClientRects()) {
            if (rect.width > 0 && rect.height > 0) return round(rect.left);
          }
        }
        return round(el.getBoundingClientRect().left);
      };
      const h1 = document.querySelector(".pageHeader h1");
      const firstCell = document.querySelector(".dataRow .dataCell");
      let listHeaderEl = null;
      if (listHeader === "section") {
        listHeaderEl = document.querySelector(".dataGroup .sectionHeaderLabel");
      } else if (listHeader === "table") {
        listHeaderEl = document.querySelector(".dataTableHeader .dataHeaderCell");
      }
      return {
        h1Left: textLeft(h1),
        firstCellLeft: textLeft(firstCell),
        listHeaderLeft: listHeaderEl ? textLeft(listHeaderEl) : null,
      };
    }, tab.listHeader);

    check(
      tab.id,
      "req3-title",
      layout.h1Left !== null && layout.firstCellLeft !== null && Math.abs(layout.h1Left - layout.firstCellLeft) <= TOLERANCE,
      `page title text left ${layout.h1Left} vs first text left ${layout.firstCellLeft}`,
    );
    if (tab.listHeader !== "none") {
      check(
        tab.id,
        "req3-listhead",
        layout.listHeaderLeft !== null && Math.abs(layout.listHeaderLeft - layout.firstCellLeft) <= TOLERANCE,
        `list header text left ${layout.listHeaderLeft} vs first text left ${layout.firstCellLeft}`,
      );
    }

    // Named headers and first-row cells must share each column's grid track.
    // This catches scrollbar/right-inset space being applied to only header or
    // body, which shifts all columns after a flexible first column.
    const columnAlignment = await page.evaluate(() => {
      const round = (value) => Math.round(value * 100) / 100;
      return [...document.querySelectorAll(".dataHeaderCell[data-column]")]
        .map((header) => {
          const column = header.dataset.column;
          const cell = column
            ? document.querySelector(`.dataRow .dataCell[data-column="${CSS.escape(column)}"]`)
            : null;
          return {
            column,
            named: Boolean(header.textContent?.trim()),
            headerLeft: round(header.getBoundingClientRect().left),
            cellLeft: cell ? round(cell.getBoundingClientRect().left) : null,
          };
        })
        .filter(({ named, cellLeft }) => named && cellLeft !== null);
    });
    const misalignedColumns = columnAlignment.filter(({ headerLeft, cellLeft }) => Math.abs(headerLeft - cellLeft) > TOLERANCE);
    check(
      tab.id,
      "req3-column-heads",
      columnAlignment.length > 0 && misalignedColumns.length === 0,
      columnAlignment.length === 0
        ? "no named header/cell pairs"
        : (misalignedColumns.length === 0 ? columnAlignment : misalignedColumns)
          .map(({ column, headerLeft, cellLeft }) => `${column}:${headerLeft}/${cellLeft}`)
          .join(", "),
    );

    // --- req6: row separator starts on the text rail ------------------------
    const separatorAlign = await page.evaluate(() => {
      const round = (value) => Math.round(value * 100) / 100;
      const textLeft = (el) => {
        if (!el) return null;
        const walker = document.createTreeWalker(el, NodeFilter.SHOW_TEXT);
        let node;
        while ((node = walker.nextNode())) {
          if (!node.textContent.trim()) continue;
          const range = document.createRange();
          range.selectNodeContents(node);
          for (const rect of range.getClientRects()) {
            if (rect.width > 0 && rect.height > 0) return round(rect.left);
          }
        }
        return round(el.getBoundingClientRect().left);
      };
      const rows = [...document.querySelectorAll(".dataRow.rowFrame")];
      const target = rows.find((row) => {
        const bg = getComputedStyle(row, "::before").backgroundColor;
        return bg !== "rgba(0, 0, 0, 0)" && bg !== "transparent";
      }) ?? rows[1] ?? rows[0];
      if (!target) return { missing: true };
      const rowRect = target.getBoundingClientRect();
      const beforeLeft = parseFloat(getComputedStyle(target, "::before").left);
      const firstCell = target.querySelector(".dataCell");
      return {
        separatorLeft: round(rowRect.left + (Number.isNaN(beforeLeft) ? 0 : beforeLeft)),
        textLeft: textLeft(firstCell),
      };
    });
    check(
      tab.id,
      "req6-separator",
      separatorAlign.missing !== true
        && separatorAlign.textLeft !== null
        && Math.abs(separatorAlign.separatorLeft - separatorAlign.textLeft) <= TOLERANCE,
      `separator left ${separatorAlign.separatorLeft} vs text left ${separatorAlign.textLeft}`,
    );

    // --- req7: header rule inset + group counts -----------------------------
    if (tab.tableHeader) {
      const headerSep = await page.evaluate(() => {
        const round = (value) => Math.round(value * 100) / 100;
        // The header underline is the sticky header's own `::after` rule.
        const header = document.querySelector(".dataTableHeader--frozen") ?? document.querySelector(".dataTableHeader");
        const row = document.querySelector(".dataRow.rowFrame");
        if (!header || !row) return { missing: true };
        const headerRect = header.getBoundingClientRect();
        const afterLeft = parseFloat(getComputedStyle(header, "::after").left);
        const rowRect = row.getBoundingClientRect();
        const beforeLeft = parseFloat(getComputedStyle(row, "::before").left);
        return {
          headerSepLeft: round(headerRect.left + (Number.isNaN(afterLeft) ? 0 : afterLeft)),
          rowSepLeft: round(rowRect.left + (Number.isNaN(beforeLeft) ? 0 : beforeLeft)),
          ruleLeft: round(headerRect.left),
        };
      });
      check(
        tab.id,
        "req7-header-sep",
        headerSep.missing !== true
          && Math.abs(headerSep.headerSepLeft - headerSep.rowSepLeft) <= TOLERANCE,
        `header rule left ${headerSep.headerSepLeft} vs row rule left ${headerSep.rowSepLeft}`,
      );
      check(
        tab.id,
        "req7-header-sep-inset",
        headerSep.missing !== true
          && headerSep.headerSepLeft - headerSep.ruleLeft > TOLERANCE,
        `header rule left ${headerSep.headerSepLeft} vs rule left ${headerSep.ruleLeft}`,
      );
      check(
        tab.id,
        "req7-header-sep-border",
        headerSep.missing !== true,
        "header rule element should exist",
      );
    }

    if (tab.id === "skills") {
      const groupCounts = await page.evaluate(() => [...document.querySelectorAll(".dataTableFrozenPane .dataGroup")].map((group) => {
        const label = group.querySelector(".sectionHeaderLabel")?.textContent?.trim() ?? "";
        const countText = group.querySelector(".sectionHeaderCount")?.textContent?.trim() ?? "";
        const count = Number.parseInt(countText, 10);
        const rows = group.querySelectorAll(".dataRow").length;
        return { label, count, rows, hasCount: countText.length > 0 };
      }));
      const countsOk = groupCounts.length > 0 && groupCounts.every((group) => group.hasCount && group.count === group.rows);
      check(
        tab.id,
        "req7-group-count",
        countsOk,
        groupCounts.map((group) => `${group.label}:${group.count}/${group.rows}`).join(", ") || "no groups",
      );
    }

    // --- req5: scroll overscroll + pinned table header ----------------------
    const scrollMetrics = await page.evaluate(({ tableHeader, clampHeight }) => {
      const round = (value) => Math.round(value * 100) / 100;
      const scroller = document.querySelector(".dataTableBodyScroll");
      if (!scroller) return { missing: true };
      const style = getComputedStyle(scroller);
      if (clampHeight) scroller.style.maxHeight = `${clampHeight}px`;
      const header = document.querySelector(".dataTableHeader--scroll") ?? document.querySelector(".dataTableHeader");
      const metrics = {
        overscrollX: style.overscrollBehaviorX,
        overscrollY: style.overscrollBehaviorY,
        scrollable: scroller.scrollHeight > scroller.clientHeight + 1,
      };
      if (tableHeader && header) {
        metrics.headerInsideScroller = scroller.contains(header);
        metrics.headerPosition = getComputedStyle(header).position;
        if (metrics.scrollable) {
          const topBefore = round(header.getBoundingClientRect().top);
          const previousTop = scroller.scrollTop;
          scroller.scrollTop = Math.min(160, scroller.scrollHeight - scroller.clientHeight);
          metrics.headerTopBefore = topBefore;
          metrics.headerTopAfter = round(header.getBoundingClientRect().top);
          metrics.didScroll = scroller.scrollTop > 0;
          scroller.scrollTop = previousTop;
        }
      }
      return metrics;
    }, { tableHeader: tab.tableHeader, clampHeight: 240 });

    check(
      tab.id,
      "req5-scroll",
      !scrollMetrics.missing,
      "data table body scroller should exist",
    );
    if (!scrollMetrics.missing) {
      check(
        tab.id,
        "req5-overscroll-x",
        scrollMetrics.overscrollX === "none",
        `horizontal overscroll ${scrollMetrics.overscrollX}`,
      );
      check(
        tab.id,
        "req5-overscroll-y",
        scrollMetrics.overscrollY === "auto",
        `vertical overscroll ${scrollMetrics.overscrollY}`,
      );
      if (tab.tableHeader) {
        check(
          tab.id,
          "req5-header-sticky",
          tab.frozen
            ? scrollMetrics.headerInsideScroller === false && scrollMetrics.headerPosition === "relative"
            : scrollMetrics.headerInsideScroller === true && scrollMetrics.headerPosition === "sticky",
          tab.frozen
            ? `split header position ${scrollMetrics.headerPosition} (outside scroller: ${scrollMetrics.headerInsideScroller === false})`
            : `table header position ${scrollMetrics.headerPosition} (inside scroller: ${scrollMetrics.headerInsideScroller})`,
        );
        if (scrollMetrics.scrollable) {
          check(
            tab.id,
            "req5-header-pin",
            scrollMetrics.didScroll
              && scrollMetrics.headerTopBefore !== undefined
              && Math.abs(scrollMetrics.headerTopBefore - scrollMetrics.headerTopAfter) <= TOLERANCE,
            `header top ${scrollMetrics.headerTopBefore} vs ${scrollMetrics.headerTopAfter} after scroll`,
          );
        } else {
          check(tab.id, "req5-header-pin", false, "table body should scroll for header pin check");
        }
      }
    }

    if (!tab.selectable) {
      await runReq8TabChecks(page, tab);
      await runFrozenBugSmokeChecks(page, tab);
      if (tab.id === "sessions") await runSessionLocatorChecks(page);
      continue;
    }

    // --- req1: checkbox hidden by default, revealed on hover ---------------
    const rowId = await page.evaluate(() =>
      document.querySelector('.dataRow[data-row-selectable="true"]')?.dataset.rowId ?? null,
    );
    check(tab.id, "req1-rowexists", Boolean(rowId), `selectable row id ${rowId}`);
    if (!rowId) continue;
    const rowLocator = () => page.locator('.dataRow[data-row-selectable="true"]').first();
    const row = rowLocator();

    const hiddenOpacity = await row.locator(".rowSelection").evaluate((node) => getComputedStyle(node).opacity);
    check(tab.id, "req1-hidden", hiddenOpacity === "0", `default checkbox opacity ${hiddenOpacity}`);

    await rowLocator().hover({ force: true });
    let revealed = "0";
    try {
      await page.waitForFunction(
        () => {
          const node = document.querySelector('.dataRow[data-row-selectable="true"] .rowSelection');
          return node && getComputedStyle(node).opacity === "1";
        },
        { timeout: 2000 },
      );
      revealed = "1";
    } catch {
      revealed = await rowLocator().locator(".rowSelection").evaluate((node) => getComputedStyle(node).opacity);
    }
    check(tab.id, "req1-reveal", revealed === "1", `hovered checkbox opacity ${revealed}`);

    // --- req2: row checkbox aligns with bottom-bar checkbox ----------------
    await rowLocator().locator(".rowSelection").click({ force: true });
    await page.locator(".actionBar.bottomBar").first().waitFor();
    if (tab.id === "mcp") {
      const identityMetrics = await page.evaluate(() => ({
        rowIds: Array.from(document.querySelectorAll(".dataTableFrozenPane .dataRow[data-row-id]"), (node) => node.dataset.rowId),
        selectedRows: document.querySelectorAll(".dataTableFrozenPane .dataRow.rowSelected").length,
        scopes: Array.from(document.querySelectorAll(".dataTableScrollPane [data-column='scope']"), (node) => node.textContent).slice(0, 2),
      }));
      check(
        tab.id,
        "bug-mcp-row-identity",
        new Set(identityMetrics.rowIds).size === identityMetrics.rowIds.length
          && identityMetrics.selectedRows === 1
          && new Set(identityMetrics.scopes).size === 2,
        `${new Set(identityMetrics.rowIds).size}/${identityMetrics.rowIds.length} unique row IDs; ${identityMetrics.selectedRows} selected; scopes ${identityMetrics.scopes.join(", ")}`,
      );
    }
    const align = await page.evaluate(() => {
      const round = (value) => Math.round(value * 100) / 100;
      const rowCheckbox = document.querySelector('.dataRow.rowSelected .rowSelection');
      const barCheckbox = document.querySelector('.actionBar.bottomBar .selectionCheckbox');
      const rowEl = document.querySelector('.dataRow.rowSelected');
      const cbRect = rowCheckbox?.getBoundingClientRect();
      const rowRect = rowEl?.getBoundingClientRect();
      return {
        rowLeft: cbRect ? round(cbRect.left) : null,
        barLeft: barCheckbox ? round(barCheckbox.getBoundingClientRect().left) : null,
        checkboxCenterY: cbRect ? round(cbRect.top + cbRect.height / 2) : null,
        rowCenterY: rowRect ? round(rowRect.top + rowRect.height / 2) : null,
      };
    });
    check(
      tab.id,
      "req2-align",
      align.rowLeft !== null && align.barLeft !== null && Math.abs(align.rowLeft - align.barLeft) <= TOLERANCE,
      `row checkbox left ${align.rowLeft} vs bar checkbox left ${align.barLeft}`,
    );
    check(
      tab.id,
      "req2-vcenter",
      align.checkboxCenterY !== null && align.rowCenterY !== null && Math.abs(align.checkboxCenterY - align.rowCenterY) <= TOLERANCE,
      `row checkbox center-y ${align.checkboxCenterY} vs row center-y ${align.rowCenterY}`,
    );

    // --- req4: action bar padding + height ------------------------------------
    // The outer bar owns the page inset; the visible surface owns the bar padding.
    const barMetrics = await page.locator(".actionBar.bottomBar .actionBarSurface").first().evaluate((node) => {
      const style = getComputedStyle(node);
      const checkbox = node.querySelector(".selectionCheckbox");
      const box = node.getBoundingClientRect();
      const checkboxBox = checkbox?.getBoundingClientRect();
      return {
        padTop: parseFloat(style.paddingTop),
        padRight: parseFloat(style.paddingRight),
        padBottom: parseFloat(style.paddingBottom),
        padLeft: parseFloat(style.paddingLeft),
        borderTop: parseFloat(style.borderTopWidth),
        borderBottom: parseFloat(style.borderBottomWidth),
        surfaceRadius: parseFloat(style.borderTopLeftRadius),
        checkboxRadius: checkbox ? parseFloat(getComputedStyle(checkbox).borderTopLeftRadius) : 0,
        height: box.height,
        checkboxHeight: checkboxBox?.height ?? 0,
      };
    });
    const padValues = [barMetrics.padTop, barMetrics.padRight, barMetrics.padBottom, barMetrics.padLeft];
    const padSpread = Math.max(...padValues) - Math.min(...padValues);
    check(
      tab.id,
      "req4-pad",
      padSpread <= TOLERANCE,
      `top ${barMetrics.padTop}px right ${barMetrics.padRight}px bottom ${barMetrics.padBottom}px left ${barMetrics.padLeft}px`,
    );
    const expectedHeight =
      barMetrics.padTop + barMetrics.padBottom + barMetrics.checkboxHeight
      + barMetrics.borderTop + barMetrics.borderBottom;
    check(
      tab.id,
      "req4-height",
      Math.abs(barMetrics.height - expectedHeight) <= TOLERANCE,
      `bar height ${barMetrics.height}px vs pad+checkbox ${expectedHeight}px (checkbox ${barMetrics.checkboxHeight}px)`,
    );
    check(
      tab.id,
      "req4-corner-center",
      Math.abs(barMetrics.surfaceRadius - (barMetrics.checkboxRadius + barMetrics.padTop)) <= TOLERANCE,
      `surface radius ${barMetrics.surfaceRadius}px vs checkbox radius + pad ${barMetrics.checkboxRadius + barMetrics.padTop}px`,
    );
    const actionButtonShadows = await page.locator(".actionBar.bottomBar .actionBarActions > button").evaluateAll((buttons) => (
      buttons.map((button) => getComputedStyle(button).boxShadow)
    ));
    check(
      tab.id,
      "req4-action-button-shadow",
      actionButtonShadows.every((shadow) => shadow === "none"),
      `idle action button shadows ${actionButtonShadows.join(", ") || "none"}`,
    );

    const insetMetrics = await page.evaluate(() => {
      const round = (value) => Math.round(value * 100) / 100;
      const readVarPx = (name) => {
        const probe = document.createElement("div");
        probe.style.position = "absolute";
        probe.style.visibility = "hidden";
        probe.style.width = `var(${name})`;
        document.body.appendChild(probe);
        const px = parseFloat(getComputedStyle(probe).width);
        probe.remove();
        return px;
      };
      const bar = document.querySelector(".actionBar.bottomBar");
      const container = bar?.closest(".content, .sessionListBody");
      if (!container || !bar) return { missing: true };
      const containerRect = container.getBoundingClientRect();
      const barRect = bar.getBoundingClientRect();
      return {
        insetLeft: round(barRect.left - containerRect.left),
        insetRight: round(containerRect.right - barRect.right),
        insetBottom: round(containerRect.bottom - barRect.bottom),
        expectedLeft: readVarPx("--data-table-bottom-bar-outer-inset-left"),
        expectedRight: readVarPx("--data-table-bottom-bar-outer-inset-right"),
        expectedBottom: readVarPx("--data-table-bottom-bar-outer-inset-bottom"),
      };
    });
    check(
      tab.id,
      "req4-inset-left",
      insetMetrics.missing !== true
        && Math.abs(insetMetrics.insetLeft - insetMetrics.expectedLeft) <= TOLERANCE,
      `bar left inset ${insetMetrics.insetLeft}px vs ${insetMetrics.expectedLeft}px`,
    );
    check(
      tab.id,
      "req4-inset-right",
      insetMetrics.missing !== true
        && Math.abs(insetMetrics.insetRight - insetMetrics.expectedRight) <= TOLERANCE,
      `bar right inset ${insetMetrics.insetRight}px vs ${insetMetrics.expectedRight}px`,
    );
    check(
      tab.id,
      "req4-inset-bottom",
      insetMetrics.missing !== true
        && Math.abs(insetMetrics.insetBottom - insetMetrics.expectedBottom) <= TOLERANCE,
      `bar bottom inset ${insetMetrics.insetBottom}px vs ${insetMetrics.expectedBottom}px`,
    );

    const checkboxMetrics = await page.evaluate(() => {
      const round = (value) => Math.round(value * 100) / 100;
      const bar = document.querySelector(".actionBar.bottomBar .actionBarSurface");
      const checkbox = bar?.querySelector(".selectionCheckbox");
      const label = bar?.querySelector(".actionBarSelectionSummary > span");
      if (!bar || !checkbox || !label) return { missing: true };
      const barStyle = getComputedStyle(bar);
      const barRect = bar.getBoundingClientRect();
      const checkboxRect = checkbox.getBoundingClientRect();
      const labelRect = label.getBoundingClientRect();
      const borderTop = parseFloat(barStyle.borderTopWidth);
      const borderLeft = parseFloat(barStyle.borderLeftWidth);
      const borderBottom = parseFloat(barStyle.borderBottomWidth);
      const pad = parseFloat(barStyle.paddingTop);
      return {
        pad,
        top: round(checkboxRect.top - barRect.top - borderTop),
        left: round(checkboxRect.left - barRect.left - borderLeft),
        bottom: round(barRect.bottom - checkboxRect.bottom - borderBottom),
        labelGap: round(labelRect.left - checkboxRect.right),
      };
    });
    check(
      tab.id,
      "req4-checkbox-inset",
      checkboxMetrics.missing !== true
        && Math.abs(checkboxMetrics.top - checkboxMetrics.pad) <= TOLERANCE
        && Math.abs(checkboxMetrics.left - checkboxMetrics.pad) <= TOLERANCE
        && Math.abs(checkboxMetrics.bottom - checkboxMetrics.pad) <= TOLERANCE,
      `top ${checkboxMetrics.top}px left ${checkboxMetrics.left}px bottom ${checkboxMetrics.bottom}px vs pad ${checkboxMetrics.pad}px`,
    );
    check(
      tab.id,
      "req4-checkbox-label-gap",
      checkboxMetrics.missing !== true
        && Math.abs(checkboxMetrics.labelGap - checkboxMetrics.pad) <= TOLERANCE,
      `checkbox-to-label gap ${checkboxMetrics.labelGap}px vs pad ${checkboxMetrics.pad}px`,
    );

    await runReq8TabChecks(page, tab);
    await runFrozenBugSmokeChecks(page, tab);
    if (tab.id === "sessions") await runSessionLocatorChecks(page);

    // The next tab mounts a fresh view component, so selection does not carry
    // over; no explicit reset needed (overlays are dismissed via Escape above).
  }

  for (const pageSpec of [
    { id: "backup", nav: "Backup", heading: "Backup", compact: false, ready: ".backupPage" },
    { id: "config", nav: "Config", heading: "Config", compact: true, ready: ".configListPane" },
    { id: "settings", nav: "Settings", heading: "Settings", compact: false, ready: ".settingsShell" },
  ]) {
    writeStdout(`\n== ${pageSpec.heading} navigation smoke ==`);
    await page.keyboard.press("Escape").catch(() => {});
    await page.getByRole("button", { name: pageSpec.nav, exact: true }).click();
    await page.getByRole("heading", { name: pageSpec.heading, exact: true }).waitFor();
    await page.locator(pageSpec.ready).waitFor();
    await runPageHeaderChecks(page, pageSpec.id, pageSpec.heading, pageSpec.compact);
  }

  const mockDiagnostics = await page.evaluate(() => window.__alignmentMockDiagnostics?.() ?? { unhandledCommands: [] });
  for (const command of new Set(mockDiagnostics.unhandledCommands)) {
    check("runtime", "mock-command-covered", false, `unhandled Tauri command ${command}`);
  }
  for (const error of runtimePageErrors) {
    check("runtime", "pageerror", false, error);
  }

  writeStdout("\n----------------------------------------");
  if (failures.length) {
    writeStdout(`alignment e2e: ${failures.length} failure(s)`);
    for (const failure of failures) writeStdout(`  - ${failure}`);
    process.exitCode = 1;
  } else {
    writeStdout("alignment e2e ok — all tabs pass req1/req2/req3/req4/req5/req6/req7/req8");
  }
} finally {
  await cleanup();
}
