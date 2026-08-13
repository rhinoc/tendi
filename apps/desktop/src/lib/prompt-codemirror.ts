import { StateField, type EditorState } from "@codemirror/state";
import { Decoration as CodeMirrorDecoration, EditorView } from "@codemirror/view";

import { assignPromptXmlTagColors, promptXmlMatchesInText, promptXmlTagClass } from "./prompt-xml.ts";

export function buildCodeMirrorPromptXmlDecorations(state: EditorState) {
  const matches = promptXmlMatchesInText(state.doc.toString());
  if (matches.length === 0) return CodeMirrorDecoration.none;

  assignPromptXmlTagColors(matches);
  return CodeMirrorDecoration.set(matches.map((match) => CodeMirrorDecoration.mark({
    class: `cmPromptXmlTag ${promptXmlTagClass(match.colorIndex!)}`,
  }).range(match.from, match.to)), true);
}

export function codeMirrorPromptXmlTagExtension() {
  return StateField.define({
    create(state) {
      return buildCodeMirrorPromptXmlDecorations(state);
    },
    update(decorations, transaction) {
      return transaction.docChanged
        ? buildCodeMirrorPromptXmlDecorations(transaction.state)
        : decorations.map(transaction.changes);
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}
