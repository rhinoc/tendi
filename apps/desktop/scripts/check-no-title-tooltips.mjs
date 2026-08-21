import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { writeStderr, writeStdout } from "./stdio.mjs";

const SOURCE_ROOT = fileURLToPath(new URL("../src/", import.meta.url));
const JSX_EXTENSIONS = new Set([".jsx", ".tsx"]);

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
    if (JSX_EXTENSIONS.has(extension)) files.push(path);
  }
  return files;
}

function identifierText(node) {
  return ts.isIdentifier(node) ? node.text : undefined;
}

function isIntrinsicTag(node) {
  const name = identifierText(node);
  return name !== undefined && name[0] === name[0].toLowerCase();
}

function checkFile(fileName, sourceText) {
  const sourceFile = ts.createSourceFile(
    fileName,
    sourceText,
    ts.ScriptTarget.Latest,
    true,
    ts.ScriptKind.TSX,
  );
  const violations = [];
  const report = (node, message) => {
    const position = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
    violations.push(`${relative(process.cwd(), fileName)}:${position.line + 1}:${position.character + 1} ${message}`);
  };
  const checkAttributes = (node, attributes) => {
    if (!isIntrinsicTag(node)) return;
    for (const attribute of attributes.properties) {
      if (ts.isJsxAttribute(attribute) && identifierText(attribute.name) === "title") {
        report(attribute, "native title attribute is not allowed");
      }
    }
  };
  const visit = (node) => {
    if (ts.isJsxElement(node)) {
      checkAttributes(node.openingElement.tagName, node.openingElement.attributes);
      if (identifierText(node.openingElement.tagName) === "title") {
        report(node.openingElement.tagName, "<title> tooltip element is not allowed");
      }
    } else if (ts.isJsxSelfClosingElement(node)) {
      checkAttributes(node.tagName, node.attributes);
    }
    ts.forEachChild(node, visit);
  };

  visit(sourceFile);
  return violations;
}

const violations = [];
for (const fileName of await sourceFiles(SOURCE_ROOT)) {
  violations.push(...checkFile(fileName, await readFile(fileName, "utf8")));
}

if (violations.length > 0) {
  writeStderr("Native title tooltips are not allowed in desktop JSX:");
  for (const violation of violations) writeStderr(`- ${violation}`);
  process.exitCode = 1;
} else {
  writeStdout("No native title tooltips found in desktop JSX.");
}
