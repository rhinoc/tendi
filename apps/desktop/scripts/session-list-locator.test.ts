import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

import {
  planSessionListLocation,
  shouldShowSessionListLocator,
} from "../src/lib/session-list-locator.ts";

const sessionsView = await readFile(new URL("../src/views/SessionsView.tsx", import.meta.url), "utf8");
const sessionController = await readFile(new URL("../src/controllers/session-controller.ts", import.meta.url), "utf8");
const sessionsCss = await readFile(new URL("../src/views/SessionsView.css", import.meta.url), "utf8");

test("shows the list locator only for an expanded detail whose session is outside the list viewport", () => {
  assert.equal(shouldShowSessionListLocator({ hasActiveSession: true, detailCollapsed: false, activeSessionInListViewport: false }), true);
  assert.equal(shouldShowSessionListLocator({ hasActiveSession: false, detailCollapsed: false, activeSessionInListViewport: false }), false);
  assert.equal(shouldShowSessionListLocator({ hasActiveSession: true, detailCollapsed: true, activeSessionInListViewport: false }), false);
  assert.equal(shouldShowSessionListLocator({ hasActiveSession: true, detailCollapsed: false, activeSessionInListViewport: true }), false);
  assert.equal(shouldShowSessionListLocator({ hasActiveSession: true, detailCollapsed: false, activeSessionInListViewport: null }), false);
});

test("renders the locator at the bottom-right of the session list instead of the detail header", () => {
  const headerActionsStart = sessionsView.indexOf('<div className="threadHeaderActions">');
  const headerActionsEnd = sessionsView.indexOf("<SessionRelationsPopover", headerActionsStart);
  const listBodyStart = sessionsView.indexOf('className="sessionListBody"');
  const listBodyEnd = sessionsView.indexOf('className="sessionPager"', listBodyStart);

  assert.ok(headerActionsStart >= 0 && headerActionsEnd > headerActionsStart);
  assert.ok(listBodyStart >= 0 && listBodyEnd > listBodyStart);
  assert.doesNotMatch(sessionsView.slice(headerActionsStart, headerActionsEnd), /Locate session in list|LocateFixed/);
  assert.match(sessionsView.slice(listBodyStart, listBodyEnd), /className="sessionListLocator"/);
  assert.match(sessionsCss, /\.sessionListLocator\s*\{[\s\S]*position:\s*absolute;[\s\S]*right:[^;]+;[\s\S]*bottom:[^;]+;/);
});

test("keeps the current list context when the target is still in its result set", () => {
  assert.equal(planSessionListLocation({
    targetRowId: "active",
    currentPageRowIds: ["before", "active", "after"],
    currentResultRowIds: ["before", "active", "after", "later"],
    allRowIds: ["before", "active", "after", "later", "excluded"],
  }), "scroll");

  assert.equal(planSessionListLocation({
    targetRowId: "active",
    currentPageRowIds: ["before"],
    currentResultRowIds: ["before", "active", "after"],
    allRowIds: ["before", "active", "after", "excluded"],
  }), "page");
});

test("reveals only a target excluded by the current context and ignores missing targets", () => {
  assert.equal(planSessionListLocation({
    targetRowId: "active",
    currentPageRowIds: ["before"],
    currentResultRowIds: ["before"],
    allRowIds: ["before", "active"],
  }), "reveal");

  assert.equal(planSessionListLocation({
    targetRowId: "missing",
    currentPageRowIds: ["before"],
    currentResultRowIds: ["before"],
    allRowIds: ["before", "active"],
  }), "missing");
});

test("uses logical row identity while keeping source-sensitive identity separate", () => {
  assert.match(sessionsView, /return sessionIdentity\(session\)/);
  assert.doesNotMatch(sessionsView, /function sessionTableRowId/);
  assert.match(sessionController, /return JSON\.stringify\(\[session\.agent, session\.id\]\)/);
  assert.doesNotMatch(sessionController, /JSON\.stringify\(\[session\.agent, session\.id, session\.path\]\)/);
  assert.doesNotMatch(sessionsView, /preserveLocatedSessionPageRef/);
  assert.match(sessionsView, /scrollResetKey=\{`\$\{pageContextKey\}\\u0000\$\{boundedCurrentPage\}`\}/);
});
