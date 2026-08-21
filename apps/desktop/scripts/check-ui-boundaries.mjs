import { readdir, readFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import ts from "typescript";
import { writeStderr, writeStdout } from "./stdio.mjs";

const PACKAGE_ROOT = fileURLToPath(new URL("../", import.meta.url));
const SOURCE_ROOT = join(PACKAGE_ROOT, "src");
const SHARED_TOOLTIP_FILE = join(SOURCE_ROOT, "components/shared/Tooltip.tsx");
const JSX_EXTENSIONS = new Set([".jsx", ".tsx"]);
const RADIX_TOOLTIP_MODULES = new Set(["radix-ui", "@radix-ui/react-tooltip"]);

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
  return files.sort();
}

function identifierText(node) {
  return ts.isIdentifier(node) ? node.text : undefined;
}

function isIntrinsicTag(node) {
  const name = identifierText(node);
  return name !== undefined && name[0] === name[0].toLowerCase();
}

function checkFile(fileName, sourceText) {
  const extension = fileName.slice(fileName.lastIndexOf("."));
  const sourceFile = ts.createSourceFile(
    fileName,
    sourceText,
    ts.ScriptTarget.Latest,
    true,
    extension === ".jsx" ? ts.ScriptKind.JSX : ts.ScriptKind.TSX,
  );
  const violations = [];
  const report = (node, message) => {
    const position = sourceFile.getLineAndCharacterOfPosition(node.getStart(sourceFile));
    violations.push(`${relative(PACKAGE_ROOT, fileName)}:${position.line + 1}:${position.character + 1} ${message}`);
  };

  const checkIntrinsicAttributes = (tagName, attributes) => {
    if (!isIntrinsicTag(tagName)) return;
    for (const attribute of attributes.properties) {
      if (ts.isJsxAttribute(attribute) && identifierText(attribute.name) === "title") {
        report(attribute, "native title attribute is not allowed; use the shared Tooltip component");
      }
    }
  };

  const checkTooltipImport = (node) => {
    if (!ts.isStringLiteral(node.moduleSpecifier) || !RADIX_TOOLTIP_MODULES.has(node.moduleSpecifier.text)) return;
    if (fileName === SHARED_TOOLTIP_FILE) return;

    const importClause = node.importClause;
    if (!importClause || importClause.isTypeOnly || !importClause.namedBindings) return;
    if (!ts.isNamedImports(importClause.namedBindings)) return;

    for (const specifier of importClause.namedBindings.elements) {
      const importedName = identifierText(specifier.propertyName ?? specifier.name);
      if (specifier.isTypeOnly || importedName !== "Tooltip") continue;
      report(specifier, "radix-ui Tooltip may only be imported by src/components/shared/Tooltip.tsx");
    }
  };

  const visit = (node) => {
    if (ts.isImportDeclaration(node)) checkTooltipImport(node);
    if (ts.isJsxElement(node)) {
      checkIntrinsicAttributes(node.openingElement.tagName, node.openingElement.attributes);
      if (identifierText(node.openingElement.tagName) === "title") {
        report(node.openingElement.tagName, "native <title> element is not allowed in desktop JSX");
      }
    } else if (ts.isJsxSelfClosingElement(node)) {
      checkIntrinsicAttributes(node.tagName, node.attributes);
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
  writeStderr("UI boundary violations found in desktop source:");
  for (const violation of violations) writeStderr(`- ${violation}`);
  process.exitCode = 1;
} else {
  writeStdout("UI boundaries passed: no native title tooltips or out-of-bound Radix Tooltip imports.");
}
