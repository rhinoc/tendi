import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const backupViewCss = await readFile(new URL("../src/features/skills/BackupView.css", import.meta.url), "utf8");
const backupView = await readFile(new URL("../src/features/skills/BackupView.tsx", import.meta.url), "utf8");
const accordionCss = await readFile(new URL("../src/components/shared/CollapsibleAccordion.css", import.meta.url), "utf8");
const accordionView = await readFile(new URL("../src/components/shared/CollapsibleAccordion.tsx", import.meta.url), "utf8");

test("Sync summary keeps loading local to settings and sync actions", () => {
  assert.match(backupViewCss, /\.settingsBackupActionStack\s*\{[\s\S]*grid-template-rows:\s*var\(--control-height\)\s+var\(--leading-ui\);/);
  assert.match(backupViewCss, /\.settingsBackupActionStack\s*\{[\s\S]*min-height:\s*calc\(var\(--control-height\)\s*\+\s*3px\s*\+\s*var\(--leading-ui\)\);/);
  assert.match(backupViewCss, /\.settingsBackupPrimaryActions\s*\{[\s\S]*height:\s*var\(--control-height\);/);
  assert.doesNotMatch(backupView, /settingsBackupSummaryActions \$\{loading/);
  assert.doesNotMatch(backupView, /backupDetailsLoading|LoadingInline/);
  assert.match(backupView, /className="backupDetailsSectionLoading" role="status" aria-label="Loading sync contents" aria-busy="true"><LoadingDots size=\{15\} \/><\/div>/);
  assert.match(backupViewCss, /\.backupDetailsSectionLoading\s*\{[\s\S]*min-height: var\(--control-height\);[\s\S]*place-items: center;[\s\S]*color: var\(--muted\);/);
  assert.match(backupView, /configured \|\| loading \? <StatefulButton[\s\S]*state=\{stateFor\(BackupAction\.Backup\)\}/);
  assert.doesNotMatch(backupView, /state=\{stateFor\(BackupAction\.Backup\)\}[^>]*disabled=\{loading\}/);
});

test("Nested sync contents scroll and keep checkbox feedback without row hover", () => {
  assert.match(backupViewCss, /\.backupCategoryAccordion\s*\{[\s\S]*--collapsible-content-overflow: auto;[\s\S]*--collapsible-content-max-height: 280px;/);
  assert.match(backupViewCss, /\.backupDetailsBody\s*\{[\s\S]*overflow-x: hidden;[\s\S]*overflow-y: auto;[\s\S]*overscroll-behavior: contain;/);
  assert.doesNotMatch(accordionCss, /\.collapsibleAccordionHeading:hover\s*\{/);
  assert.match(accordionCss, /\.collapsibleAccordionHeading:has\(:focus-visible\)/);
  assert.match(accordionCss, /\.collapsibleAccordionTrigger:focus-visible\s*\{[\s\S]*outline: none;/);
  assert.match(backupView, /separateExpandedItems=\{false\}/);
  assert.doesNotMatch(backupViewCss, /\.collapsibleAccordion/);
  assert.match(backupViewCss, /\.backupDetailsAccordionSection > :first-child\s*\{[\s\S]*--collapsible-content-padding: 0 16px 4px;/);
  assert.match(backupViewCss, /\.backupDetailsAccordionSection \[data-accordion-item="repository"\],[\s\S]*--collapsible-content-padding: 0 16px 16px;/);
  assert.match(backupViewCss, /\.backupCategoryAccordion \.backupContentItems\s*\{[\s\S]*padding-inline: 12px;/);
  assert.doesNotMatch(backupViewCss, /\.backupContentItemRow:hover\s*\{/);
  assert.match(accordionCss, /--collapsible-heading-min-height: 44px;/);
  assert.match(backupViewCss, /--collapsible-trigger-min-height: 40px;/);
  assert.match(accordionCss, /--collapsible-content-padding: 12px 16px;/);
});

test("Expanded sync item rows use a smaller type scale", () => {
  assert.match(backupViewCss, /\.backupContentItemRow \.dataCellTitle\s*\{[\s\S]*font-size: var\(--text-xs\);/);
  assert.match(backupViewCss, /\.backupContentItemRow \.dataCellSub\s*\{[\s\S]*font-size: var\(--text-xs\);/);
});

test("History rows match sync item rows without dividers", () => {
  assert.match(backupViewCss, /\.settingsBackupVersionList li\s*\{[\s\S]*display: block;/);
  assert.doesNotMatch(backupViewCss, /\.settingsBackupVersionList li\s*\{[^}]*border-top:/);
  assert.match(backupViewCss, /\.backupHistoryItemCard\s*\{[\s\S]*border-radius: 0;/);
  assert.doesNotMatch(backupViewCss, /\.backupHistoryItemCard:hover\s*\{/);
  assert.match(backupViewCss, /\.backupHistoryVersionRow\s*\{[\s\S]*gap: 12px;[\s\S]*min-height: var\(--data-table-table-row-height\);[\s\S]*padding-inline: 8px;/);
  assert.match(backupViewCss, /\.backupHistoryVersionRow \.settingsBackupVersionCopy > span\s*\{[\s\S]*font-size: var\(--text-xs\);/);
  assert.match(backupView, /<GitCommitHorizontal size=\{14\} aria-hidden="true" \/>/);
  assert.match(backupView, /formatRelativeTime\(version\.createdAt \* 1000, now\)/);
  assert.doesNotMatch(backupViewCss, /\.backupHistoryItemCard:hover \.settingsBackupVersionCopy strong/);
});

test("Configured sync keeps Disconnect as an accordion item with an action", () => {
  assert.match(backupView, /id: "disconnect",\s*title: "Disconnect",\s*content: <div className="backupDisconnectContent">[\s\S]*BackupAction\.Disconnect/);
  assert.match(backupViewCss, /\.backupDisconnectContent\s*\{[\s\S]*display: flex;[\s\S]*align-items: center;/);
  assert.doesNotMatch(backupView, /<DialogActionBar[\s\S]*leading=\{/);
  assert.doesNotMatch(backupView, /footer=\{/);
});

test("Nested accordion uses a concentric inner corner radius", () => {
  assert.match(accordionView, /cornerRadius = 28/);
  assert.match(backupView, /const BACKUP_CATEGORY_ACCORDION_RADIUS = 12;/);
  assert.match(backupView, /cornerRadius=\{BACKUP_CATEGORY_ACCORDION_RADIUS\}/);
  assert.match(accordionCss, /--collapsible-content-padding: 12px 16px;/);
});

test("Repository is an accordion item and is the default", () => {
  assert.match(backupViewCss, /\.backupDetailsDialog \{[\s\S]*height:\s*min\(560px, calc\(100vh - 64px\)\);/);
  assert.match(backupViewCss, /\.backupDetailsSection\s*\{[\s\S]*padding-top: 22px;/);
  assert.match(backupView, /<CollapsibleAccordion\s+key=\{detailsOpen \? "open" : "closed"\}\s+defaultValue="repository"/);
  assert.match(backupView, /id: "repository",\s*title: "Repository"/);
  assert.match(accordionView, /data-accordion-item=\{item\.id\}/);
  assert.doesNotMatch(backupViewCss, /\.backupDetailsSection\s*\+\s*\.backupDetailsSection/);
});
