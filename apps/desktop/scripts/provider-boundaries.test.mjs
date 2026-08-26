import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import { join, relative, sep } from "node:path";
import { fileURLToPath } from "node:url";
import test from "node:test";
import ts from "typescript";

const PACKAGE_ROOT = fileURLToPath(new URL("../", import.meta.url));
const SOURCE_ROOT = join(PACKAGE_ROOT, "src");
const PROVIDER_ROOT = join(SOURCE_ROOT, "lib", "agent");
const SOURCE_EXTENSIONS = new Set([".ts", ".tsx"]);
const PROVIDER_NAMES = new Set(["codex", "claude", "cursor"]);

async function sourceFiles(directory) {
  const entries = await readdir(directory, { withFileTypes: true });
  const files = [];
  for (const entry of entries) {
    const fileName = join(directory, entry.name);
    if (entry.isDirectory()) {
      files.push(...await sourceFiles(fileName));
      continue;
    }
    const extension = entry.name.slice(entry.name.lastIndexOf("."));
    if (SOURCE_EXTENSIONS.has(extension)) files.push(fileName);
  }
  return files.sort();
}

function isProviderOwned(fileName) {
  const providerPath = relative(SOURCE_ROOT, fileName);
  const providerRoot = relative(SOURCE_ROOT, PROVIDER_ROOT);
  return providerPath === providerRoot || providerPath.startsWith(`${providerRoot}${sep}`);
}

function isProviderLiteral(node) {
  return (ts.isStringLiteral(node) || ts.isNoSubstitutionTemplateLiteral(node))
    && PROVIDER_NAMES.has(node.text.toLowerCase());
}

function providerLiterals(node, sourceFile) {
  const literals = [];
  const visit = (child) => {
    if (isProviderLiteral(child)) literals.push(child);
    ts.forEachChild(child, visit);
  };
  visit(node);
  return literals.map((literal) => {
    const position = sourceFile.getLineAndCharacterOfPosition(literal.getStart(sourceFile));
    return `${relative(PACKAGE_ROOT, sourceFile.fileName)}:${position.line + 1}:${position.character + 1} ${literal.text}`;
  });
}

async function providerBranchViolations() {
  const violations = [];
  for (const fileName of await sourceFiles(SOURCE_ROOT)) {
    if (isProviderOwned(fileName)) continue;
    const sourceFile = ts.createSourceFile(
      fileName,
      await readFile(fileName, "utf8"),
      ts.ScriptTarget.Latest,
      true,
      fileName.endsWith(".tsx") ? ts.ScriptKind.TSX : ts.ScriptKind.TS,
    );
    const report = (node, kind) => {
      for (const literal of providerLiterals(node, sourceFile)) {
        violations.push(`${literal} in ${kind}; provider branching must stay in src/lib/agent`);
      }
    };
    const visit = (node) => {
      if (ts.isIfStatement(node)) report(node.expression, "if condition");
      if (ts.isConditionalExpression(node)) report(node.condition, "ternary condition");
      if (ts.isCaseClause(node)) report(node.expression, "switch case");
      ts.forEachChild(node, visit);
    };
    visit(sourceFile);
  }
  return violations;
}

test("provider-specific branching stays inside provider-owned files", async () => {
  assert.deepEqual(await providerBranchViolations(), []);
});

test("session deeplink consumers use the provider capability dispatcher", async () => {
  const sessionsView = await readFile(join(SOURCE_ROOT, "views", "SessionsView.tsx"), "utf8");
  const sessions = await readFile(join(SOURCE_ROOT, "lib", "sessions.ts"), "utf8");
  assert.match(sessionsView, /const deeplink = sessionAppDeepLink\(session\);/);
  assert.match(sessions, /agentDefinition\(normalizedAgentKey\(session\.agent\)\)\?\.sessionAppDeepLink\?\.\(session\)/);
});
