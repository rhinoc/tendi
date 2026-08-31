import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const hooksCss = await readFile(new URL("../src/views/HooksView.css", import.meta.url), "utf8");
const sessionsView = await readFile(new URL("../src/views/SessionsView.tsx", import.meta.url), "utf8");

test("Hooks source loading and preview keep the same outer height", () => {
  assert.match(hooksCss, /\.hookSourceLoading\s*\{[\s\S]*min-height:\s*142px;/);
  assert.match(hooksCss, /\.hookSourcePreview\s*\{[\s\S]*min-height:\s*142px;/);
  assert.match(hooksCss, /\.hookSourcePreview\s*>\s*\.hookSourceLoading\s*\{[\s\S]*min-height:\s*140px;/);
});

test("Skills popover waits for loaded links before opening", () => {
  assert.match(sessionsView, /const pendingOpenRef = useRef\(false\);/);
  assert.match(sessionsView, /if \(open && !loaded\) \{/);
  assert.match(sessionsView, /<Popover\.Root open=\{open\} onOpenChange=\{handleOpenChange\}>/);
  assert.match(sessionsView, /loading \|\| links\.length > 0 \? " hasChart"/);
});
