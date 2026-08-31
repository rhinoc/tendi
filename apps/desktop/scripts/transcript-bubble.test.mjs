import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptDir = dirname(fileURLToPath(import.meta.url));
const css = await readFile(join(scriptDir, "../src/views/SessionsView.css"), "utf8");
const globalCss = await readFile(join(scriptDir, "../src/styles.css"), "utf8");
const confirmDialogCss = await readFile(join(scriptDir, "../src/components/shared/confirm-dialog.css"), "utf8");

function cssRule(value, selector) {
  const match = value.match(new RegExp(`${selector} \\{([\\s\\S]*?)\\n\\}`));
  assert.ok(match, `missing CSS rule: ${selector}`);
  return match[1];
}

const chatMessage = cssRule(css, "\\.chatMessage");
const bubble = cssRule(css, "\\.bubble");
const bubbleParagraph = cssRule(css, "\\.bubble p");
const bubbleFooter = cssRule(css, "\\.bubbleFooter");
const messageActionButton = cssRule(css, "\\.messageActionButton");
const globalParagraph = cssRule(globalCss, "(?:^|\\n)p");
const confirmDialogDescription = cssRule(confirmDialogCss, "\\.confirmDialogDescription");

assert.match(globalParagraph, /text-wrap: wrap;/);
assert.doesNotMatch(globalParagraph, /text-wrap: pretty;/);
assert.match(confirmDialogDescription, /text-wrap: wrap;/);
assert.doesNotMatch(confirmDialogDescription, /text-wrap: pretty;/);

assert.match(chatMessage, /width: fit-content;/);
assert.match(chatMessage, /max-width: min\(80%, 600px\);/);
assert.match(bubble, /width: 100%;/);
assert.match(bubble, /max-width: 100%;/);
assert.match(bubble, /overflow-wrap: anywhere;/);
assert.match(bubble, /word-break: break-all;/);
assert.match(bubbleParagraph, /white-space: normal;/);
assert.doesNotMatch(bubbleParagraph, /white-space: pre-wrap;/);
assert.match(bubbleParagraph, /text-wrap: wrap;/);
assert.doesNotMatch(bubbleParagraph, /text-wrap: (?:balance|pretty);/);
assert.match(bubbleParagraph, /overflow-wrap: anywhere;/);
assert.match(bubbleParagraph, /word-break: break-all;/);
assert.match(bubbleFooter, /width: 100%;/);
assert.match(bubbleFooter, /padding-inline: calc\(\(var\(--control-size-icon\) - 13px\) \/ 2\) 0;/);
assert.match(messageActionButton, /padding: 0;/);
