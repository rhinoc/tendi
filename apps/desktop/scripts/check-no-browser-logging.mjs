import { readFileSync, readdirSync, statSync } from "node:fs";
import { relative, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const desktopDir = resolve(fileURLToPath(new URL("..", import.meta.url)));
const sourceDir = resolve(desktopDir, "src");
const sourceFiles = [];

function walk(directory) {
  for (const entry of readdirSync(directory)) {
    const path = resolve(directory, entry);
    if (entry === "node_modules" || entry === "dist") continue;
    if (statSync(path).isDirectory()) walk(path);
    else if (/\.(ts|tsx|js|jsx)$/.test(entry)) sourceFiles.push(path);
  }
}

walk(sourceDir);

const violations = sourceFiles.flatMap((path) => {
  const lines = readFileSync(path, "utf8").split("\n");
  return lines.flatMap((line, index) => /\bconsole\s*\./.test(line)
    ? [`${relative(desktopDir, path)}:${index + 1}`]
    : []);
});

if (violations.length > 0) {
  process.stderr.write(`browser logging API usage is forbidden in desktop source:\n${violations.map((value) => `- ${value}`).join("\n")}\n`);
  process.exitCode = 1;
} else {
  process.stdout.write("desktop source contains no browser logging API calls\n");
}
