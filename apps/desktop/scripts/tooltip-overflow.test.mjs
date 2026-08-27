import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const packageRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const tooltipSource = await readFile(join(packageRoot, "src/components/shared/Tooltip.tsx"), "utf8");
const tooltipCss = await readFile(join(packageRoot, "src/components/shared/Tooltip.css"), "utf8");
assert.match(tooltipSource, /content,\n\s+interactive = true,/);
assert.match(tooltipCss, /max-height:\s*min\(360px/);
assert.match(tooltipCss, /overflow-y:\s*auto/);
assert.match(tooltipCss, /\.appTooltip\.isInteractive\s*\{\s*pointer-events:\s*auto;/s);
