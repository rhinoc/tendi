import { StateField } from "@codemirror/state";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";
import type { EditorState } from "@codemirror/state";

import { currentLineDiffMap, type DiffLine } from "./diff.ts";

export class RemovedDiffWidget extends WidgetType {
  lines: DiffLine[];

  constructor(lines: DiffLine[]) {
    super();
    this.lines = lines;
  }

  eq(other: RemovedDiffWidget): boolean {
    return JSON.stringify(this.lines) === JSON.stringify(other.lines);
  }

  toDOM(): HTMLElement {
    const block = document.createElement("div");
    block.className = "cmDiffRemovedBlock";
    for (const line of this.lines) {
      const row = document.createElement("div");
      row.className = "cmDiffRemovedLine";
      const marker = document.createElement("span");
      marker.className = "cmDiffRemovedMarker";
      marker.textContent = "-";
      const text = document.createElement("span");
      text.className = "cmDiffRemovedText";
      for (const segment of line.segments ?? [{ text: line.text || " ", changed: true }]) {
        const span = document.createElement("span");
        if (segment.changed) span.className = "cmDiffRemovedCharChanged";
        span.textContent = segment.text || " ";
        text.appendChild(span);
      }
      row.append(marker, text);
      block.appendChild(row);
    }
    return block;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

export function buildEditableDiffDecorations(state: EditorState, originalContent: string) {
  const documentText = state.doc.toString();
  if (documentText === originalContent) return Decoration.none;

  const { lineStates, removedBefore } = currentLineDiffMap(originalContent, documentText);
  const decorations = [];
  for (const [lineNumber, removedLines] of removedBefore) {
    const position = lineNumber <= state.doc.lines
      ? state.doc.line(lineNumber).from
      : state.doc.length;
    decorations.push(Decoration.widget({
      widget: new RemovedDiffWidget(removedLines),
      block: true,
      side: -1,
    }).range(position));
  }

  for (const [lineNumber, lineState] of lineStates) {
    if (lineNumber > state.doc.lines) continue;
    const line = state.doc.line(lineNumber);
    decorations.push(Decoration.line({
      class: "cmDiffLineAdded",
    }).range(line.from));

    let offset = line.from;
    for (const segment of lineState.segments) {
      const length = segment.text.length;
      if (segment.changed && length > 0) {
        decorations.push(Decoration.mark({ class: "cmDiffAddedCharChanged" }).range(offset, offset + length));
      }
      offset += length;
    }
  }

  return Decoration.set(decorations, true);
}

export function editableDiffExtension(originalContent: string) {
  return StateField.define({
    create(state) {
      return buildEditableDiffDecorations(state, originalContent);
    },
    update(decorations, transaction) {
      return transaction.docChanged
        ? buildEditableDiffDecorations(transaction.state, originalContent)
        : decorations.map(transaction.changes);
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}
