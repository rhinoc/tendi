import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";

import { writeStderr, writeStdout } from "./stdio.mjs";

const PACKAGE_ROOT = fileURLToPath(new URL("../", import.meta.url));
const SOURCE_ROOT = join(PACKAGE_ROOT, "src");
const CSS_EXTENSIONS = new Set([".css"]);
const TS_EXTENSIONS = new Set([".ts", ".tsx"]);
const integerPixel = /^\d+px$/;

async function sourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await sourceFiles(path));
      continue;
    }
    const extension = entry.name.slice(entry.name.lastIndexOf("."));
    if (CSS_EXTENSIONS.has(extension) || TS_EXTENSIONS.has(extension)) files.push(path);
  }
  return files.sort();
}

function lineNumber(source, offset) {
  return source.slice(0, offset).split("\n").length;
}

function report(violations, fileName, source, offset, message) {
  violations.push(`${relative(PACKAGE_ROOT, fileName)}:${lineNumber(source, offset)} ${message}`);
}

function checkCss(fileName, source, violations) {
  const declarationPattern = /\b(font-size|line-height)\s*:\s*([^;{}]+)\s*;/g;
  for (const match of source.matchAll(declarationPattern)) {
    const property = match[1];
    const value = match[2].trim();
    if (value.startsWith("var(--")) continue;
    report(violations, fileName, source, match.index, `${property} must use a typography variable, found ${value}`);
  }
}

function checkTypeScript(fileName, source, violations) {
  const declarationPattern = /\b(fontSize|lineHeight)\s*:\s*(["'])(.*?)\2/g;
  for (const match of source.matchAll(declarationPattern)) {
    const property = match[1];
    const value = match[3].trim();
    if (value.startsWith("var(--")) continue;
    report(violations, fileName, source, match.index, `${property} must use a typography variable, found ${value}`);
  }
}

function checkVariables(fileName, source, violations) {
  const tokenPattern = /(--(?:text|leading)-[a-z0-9-]+)\s*:\s*([^;]+);/g;
  for (const match of source.matchAll(tokenPattern)) {
    const token = match[1];
    const value = match[2].trim();
    if (integerPixel.test(value)) continue;
    report(violations, fileName, source, match.index, `${token} must resolve to an integer pixel value, found ${value}`);
  }
}

const files = await sourceFiles(SOURCE_ROOT);
const violations = [];
for (const fileName of files) {
  const source = await readFile(fileName, "utf8");
  if (fileName.endsWith("variables.css")) checkVariables(fileName, source, violations);
  if (fileName.endsWith(".css")) checkCss(fileName, source, violations);
  else checkTypeScript(fileName, source, violations);
}

if (violations.length > 0) {
  writeStderr("Typography token violations found:");
  for (const violation of violations) writeStderr(`- ${violation}`);
  process.exitCode = 1;
} else {
  writeStdout("Typography tokens passed: integer scales and variable-backed font sizes/line heights.");
}
