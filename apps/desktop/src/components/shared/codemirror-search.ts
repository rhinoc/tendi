import { StateField, type EditorState } from "@codemirror/state";
import { Decoration, EditorView } from "@codemirror/view";
import type { Node as ProseMirrorNode } from "@tiptap/pm/model";

import { findTextRanges } from "./text-ranges.ts";
import type { TextRange } from "./text-ranges.ts";

export { findTextRanges } from "./text-ranges.ts";
export type { TextRange } from "./text-ranges.ts";

export function buildCodeMirrorSearchDecorations(
  state: EditorState,
  query: string,
  activeIndex: number,
) {
  const searchRanges = findTextRanges(state.doc.toString(), query);
  if (searchRanges.length === 0) return Decoration.none;
  const activeMatchIndex = Math.min(Math.max(activeIndex, 0), searchRanges.length - 1);
  return Decoration.set(searchRanges.map((range, index) => Decoration.mark({
    class: `cmEditorSearchMatch ${index === activeMatchIndex ? "active" : ""}`,
  }).range(range.from, range.to)), true);
}

export function codeMirrorSearchExtension(query: string, activeIndex: number) {
  return StateField.define({
    create(state) {
      return buildCodeMirrorSearchDecorations(state, query, activeIndex);
    },
    update(decorations, transaction) {
      return transaction.docChanged
        ? buildCodeMirrorSearchDecorations(transaction.state, query, activeIndex)
        : decorations.map(transaction.changes);
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}

export function prosemirrorTextRanges(doc: ProseMirrorNode, query: string): TextRange[] {
  if (!query.trim()) return [];

  const ranges: TextRange[] = [];
  doc.descendants((node, pos) => {
    if (!node.isText || !node.text) return;
    for (const range of findTextRanges(node.text, query)) {
      ranges.push({ from: pos + range.from, to: pos + range.to });
    }
  });
  return ranges;
}
