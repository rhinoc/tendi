import { StateField, type EditorState } from "@codemirror/state";
import { Decoration, EditorView } from "@codemirror/view";
import type { Node as ProseMirrorNode } from "@tiptap/pm/model";

import { findTextRanges, type TextRange } from "./text-ranges.ts";

export { findTextRanges } from "./text-ranges.ts";
export type { TextRange } from "./text-ranges.ts";

export function buildCodeMirrorSearchDecorations(
  state: EditorState,
  query: string,
  activeIndex: number,
  ranges?: TextRange[],
) {
  const searchRanges = ranges ?? findTextRanges(state.doc.toString(), query);
  if (searchRanges.length === 0) return Decoration.none;
  return Decoration.set(searchRanges.map((range, index) => Decoration.mark({
    class: `cmEditorSearchMatch ${index === activeIndex ? "active" : ""}`,
  }).range(range.from, range.to)), true);
}

export function codeMirrorSearchExtension(query: string, activeIndex: number, ranges?: TextRange[]) {
  return StateField.define({
    create(state) {
      return buildCodeMirrorSearchDecorations(state, query, activeIndex, ranges);
    },
    update(decorations, transaction) {
      return decorations.map(transaction.changes);
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}

export function prosemirrorTextRanges(doc: ProseMirrorNode, query: string): TextRange[] {
  const needle = query.trim().toLowerCase();
  if (!needle) return [];
  const ranges: TextRange[] = [];
  doc.descendants((node, pos) => {
    if (!node.isText || !node.text) return;
    const text = node.text.toLowerCase();
    let index = text.indexOf(needle);
    while (index >= 0) {
      ranges.push({ from: pos + index, to: pos + index + needle.length });
      index = text.indexOf(needle, index + needle.length);
    }
  });
  return ranges;
}
