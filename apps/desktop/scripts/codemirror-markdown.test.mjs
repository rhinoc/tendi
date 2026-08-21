import assert from "node:assert/strict";
import { test } from "node:test";

import { syntaxTree } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { highlightTree } from "@lezer/highlight";
import { Schema } from "@tiptap/pm/model";

import {
  codeMirrorMarkdown,
  codeMirrorMarkdownLanguage,
} from "../src/lib/codemirror-markdown.ts";
import { codeMirrorSearchExtension, prosemirrorTextRanges } from "../src/components/shared/codemirror-search.ts";
import { findTextRanges } from "../src/components/shared/text-ranges.ts";
import { buildEditableDiffDecorations } from "../src/lib/codemirror-diff.ts";
import { codeMirrorHighlightStyle } from "../src/lib/codemirror-theme.ts";

const classToSyntaxVariable = new Map(
  codeMirrorHighlightStyle.module.rules.flatMap((rule) => {
    const match = /^\.([^ ]+) \{color: var\((--syntax-[^)]+)\)/.exec(rule);
    return match ? [[match[1], match[2]]] : [];
  }),
);

async function fencedCodeSyntaxVariables(language, source) {
  const languageDescription = codeMirrorMarkdownLanguage(language);
  assert.ok(languageDescription, `${language} language is registered`);
  await languageDescription.load();
  const document = `\`\`\`${language}\n${source}\n\`\`\``;
  const codeFrom = document.indexOf("\n") + 1;
  const codeTo = document.lastIndexOf("\n");
  const state = EditorState.create({
    doc: document,
    extensions: [codeMirrorMarkdown()],
  });
  const variables = new Set();
  highlightTree(syntaxTree(state), codeMirrorHighlightStyle, (from, to, classes) => {
    if (from < codeFrom || to > codeTo) return;
    for (const className of classes.split(" ")) {
      const variable = classToSyntaxVariable.get(className);
      if (variable) variables.add(variable);
    }
  });
  return variables;
}

test("Markdown fenced code uses nested language syntax tokens", async () => {
  const json = await fencedCodeSyntaxVariables("json", '{"name": "tendi", "enabled": true, "count": 3}');
  assert.ok(json.has("--syntax-property"));
  assert.ok(json.has("--syntax-string"));
  assert.ok(json.has("--syntax-atom"));
  assert.ok(json.has("--syntax-number"));

  const typescript = await fencedCodeSyntaxVariables("typescript", 'const label: string = "tendi";');
  assert.ok(typescript.has("--syntax-keyword"));
  assert.ok(typescript.has("--syntax-string"));

  const shell = await fencedCodeSyntaxVariables("bash", 'if test -f "$path"; then echo "ready"; fi');
  assert.ok(shell.has("--syntax-keyword"));
  assert.ok(shell.has("--syntax-string"));
});

test("CodeMirror search decorations follow the current document after replacement", () => {
  const initialContent = "x".repeat(339);
  const nextContent = `${"x".repeat(437)}needle`;
  const searchField = codeMirrorSearchExtension("needle", 0);
  const state = EditorState.create({
    doc: initialContent,
    extensions: [searchField],
  });
  const transaction = state.update({
    changes: { from: 0, to: initialContent.length, insert: nextContent },
  });

  assert.doesNotThrow(() => void transaction.state);
  const ranges = [];
  transaction.state.field(searchField).between(0, transaction.state.doc.length, (from, to) => {
    ranges.push({ from, to });
  });
  assert.deepEqual(ranges, [{ from: 437, to: 443 }]);
});

test("Search ranges preserve source positions when case folding expands text", () => {
  assert.deepEqual(findTextRanges("İabc", "abc"), [{ from: 1, to: 4 }]);
  assert.deepEqual(findTextRanges("😀Alpha", "alpha"), [{ from: 2, to: 7 }]);
  assert.deepEqual(findTextRanges("a.b", "."), [{ from: 1, to: 2 }]);
});

test("ProseMirror search ranges preserve text-node positions", () => {
  const schema = new Schema({
    nodes: {
      doc: { content: "text*" },
      text: { group: "inline" },
    },
  });
  const doc = schema.node("doc", null, [schema.text("İabc")]);
  assert.deepEqual(prosemirrorTextRanges(doc, "abc"), [{ from: 1, to: 4 }]);
});

function decorationRanges(decorations, length) {
  const ranges = [];
  decorations.between(0, length, (from, to) => {
    ranges.push({ from, to });
  });
  return ranges;
}

test("Editable diff decorations do not extend past an empty added line", () => {
  const state = EditorState.create({ doc: "a\n" });
  const ranges = decorationRanges(buildEditableDiffDecorations(state, "a"), state.doc.length);
  assert.ok(ranges.every(({ from, to }) => from >= 0 && from <= to && to <= state.doc.length));
});
