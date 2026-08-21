import { StateField } from "@codemirror/state";
import type { EditorState, Range } from "@codemirror/state";
import { Decoration, EditorView, WidgetType } from "@codemirror/view";

import { diffPreview, type DiffSegment } from "./diff.ts";

type ConflictMarkerKind = "start" | "base" | "separator" | "end";
type ConflictSection = "local" | "base" | "incoming";
type ConflictResolutionSide = "local" | "incoming" | "both";

type ConflictBlock = {
  startLine: number;
  endLine: number;
  localStart: number;
  localEnd: number;
  incomingStart: number;
  incomingEnd: number;
};

function conflictMarkerKind(line: string): ConflictMarkerKind | null {
  const normalized = line.endsWith("\r") ? line.slice(0, -1) : line;
  if (normalized.startsWith("<<<<<<< ")) return "start";
  if (normalized.startsWith("||||||| ")) return "base";
  if (normalized === "=======") return "separator";
  if (normalized.startsWith(">>>>>>> ")) return "end";
  return null;
}

function sectionLineClass(section: ConflictSection) {
  return section === "local"
    ? "cmConflictLocalLine"
    : section === "base"
      ? "cmConflictBaseLine"
      : "cmConflictIncomingLine";
}

function markerLineClass(kind: ConflictMarkerKind) {
  if (kind === "start") return "cmConflictLocalMarkerLine";
  if (kind === "base") return "cmConflictBaseMarkerLine";
  if (kind === "end") return "cmConflictIncomingMarkerLine";
  return "cmConflictSeparatorLine";
}

function markerTextClass(kind: ConflictMarkerKind) {
  if (kind === "start") return "cmConflictLocalMarkerText";
  if (kind === "base") return "cmConflictBaseMarkerText";
  if (kind === "end") return "cmConflictIncomingMarkerText";
  return "cmConflictSeparatorText";
}

function conflictBlocks(content: string): ConflictBlock[] {
  const lines = content.split("\n");
  const blocks: ConflictBlock[] = [];
  for (let startLine = 0; startLine < lines.length; startLine += 1) {
    if (conflictMarkerKind(lines[startLine]) !== "start") continue;
    let baseLine = -1;
    let separatorLine = -1;
    let endLine = -1;
    for (let line = startLine + 1; line < lines.length; line += 1) {
      const kind = conflictMarkerKind(lines[line]);
      if (kind === "base" && baseLine === -1) baseLine = line;
      if (kind === "separator") {
        separatorLine = line;
        break;
      }
    }
    if (separatorLine === -1) continue;
    for (let line = separatorLine + 1; line < lines.length; line += 1) {
      if (conflictMarkerKind(lines[line]) === "end") {
        endLine = line;
        break;
      }
    }
    if (endLine === -1) continue;
    blocks.push({
      startLine,
      endLine,
      localStart: startLine + 1,
      localEnd: baseLine >= 0 ? baseLine : separatorLine,
      incomingStart: separatorLine + 1,
      incomingEnd: endLine,
    });
    startLine = endLine;
  }
  return blocks;
}

function resolveConflictBlock(content: string, index: number, side: ConflictResolutionSide) {
  const lines = content.split("\n");
  const block = conflictBlocks(content)[index];
  if (!block) return content;
  const replacement = side === "local"
    ? lines.slice(block.localStart, block.localEnd)
    : side === "incoming"
      ? lines.slice(block.incomingStart, block.incomingEnd)
      : [
        ...lines.slice(block.localStart, block.localEnd),
        ...lines.slice(block.incomingStart, block.incomingEnd),
      ];
  lines.splice(block.startLine, block.endLine - block.startLine + 1, ...replacement);
  return lines.join("\n");
}

function conflictCharDiff(before: string, after: string) {
  const diffLines = diffPreview(before, after);
  const beforeChanged = new Map<number, DiffSegment[]>();
  const afterChanged = new Map<number, DiffSegment[]>();
  let beforeLine = 0;
  let afterLine = 0;
  for (const line of diffLines) {
    if (line.kind === "removed") {
      beforeChanged.set(beforeLine, line.segments ?? [{ text: line.text, changed: true }]);
      beforeLine += 1;
      continue;
    }
    if (line.kind === "added") {
      afterChanged.set(afterLine, line.segments ?? [{ text: line.text, changed: true }]);
      afterLine += 1;
      continue;
    }
    beforeLine += 1;
    afterLine += 1;
  }
  return { beforeChanged, afterChanged };
}

function addChangedCharDecorations(
  decorations: Range<Decoration>[],
  state: EditorState,
  documentStartLine: number,
  changedLines: Map<number, DiffSegment[]>,
  className: string,
) {
  for (const [lineIndex, segments] of changedLines) {
    const lineNumber = documentStartLine + lineIndex + 1;
    if (lineNumber > state.doc.lines) continue;
    const line = state.doc.line(lineNumber);
    let offset = line.from;
    for (const segment of segments) {
      const length = segment.text.length;
      if (segment.changed && length > 0) {
        decorations.push(Decoration.mark({ class: className }).range(offset, offset + length));
      }
      offset += length;
    }
  }
}

const conflictActions: { side: ConflictResolutionSide; label: string; className: string }[] = [
  { side: "local", label: "Accept Local", className: "cmConflictActionLocal" },
  { side: "incoming", label: "Accept Remote", className: "cmConflictActionRemote" },
  { side: "both", label: "Accept Both", className: "cmConflictActionBoth" },
];

class ConflictActionWidget extends WidgetType {
  constructor(
    private readonly content: string,
    private readonly index: number,
    private readonly onResolve: (content: string) => void,
  ) {
    super();
  }

  eq(other: ConflictActionWidget): boolean {
    return this.content === other.content && this.index === other.index;
  }

  toDOM(): HTMLElement {
    const group = document.createElement("span");
    group.className = "cmConflictActionGroup";
    group.setAttribute("aria-label", `Conflict ${this.index + 1} actions`);
    for (const action of conflictActions) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = `cmConflictActionButton ${action.className}`;
      button.textContent = action.label;
      button.addEventListener("mousedown", (event) => event.preventDefault());
      button.addEventListener("click", (event) => {
        event.preventDefault();
        event.stopPropagation();
        this.onResolve(resolveConflictBlock(this.content, this.index, action.side));
      });
      group.appendChild(button);
    }
    return group;
  }

  ignoreEvent(): boolean {
    return false;
  }
}

function buildConflictDecorations(
  state: EditorState,
  onResolve?: (content: string) => void,
) {
  const blocks = conflictBlocks(state.doc.toString());
  const documentLines = state.doc.toString().split("\n");
  const blockByStartLine = new Map(blocks.map((block, index) => [block.startLine + 1, index]));
  const decorations: Range<Decoration>[] = [];
  for (const block of blocks) {
    const baseStart = block.localEnd + 1;
    const baseEnd = block.incomingStart - 1;
    const base = documentLines.slice(baseStart, baseEnd).join("\n");
    const local = documentLines.slice(block.localStart, block.localEnd).join("\n");
    const incoming = documentLines.slice(block.incomingStart, block.incomingEnd).join("\n");
    const localDiff = conflictCharDiff(base, local);
    const incomingDiff = conflictCharDiff(base, incoming);
    addChangedCharDecorations(decorations, state, block.localStart, localDiff.afterChanged, "cmConflictLocalCharChanged");
    addChangedCharDecorations(decorations, state, block.incomingStart, incomingDiff.afterChanged, "cmConflictIncomingCharChanged");
    addChangedCharDecorations(decorations, state, baseStart, localDiff.beforeChanged, "cmConflictBaseCharChanged");
    addChangedCharDecorations(decorations, state, baseStart, incomingDiff.beforeChanged, "cmConflictBaseCharChanged");
  }
  let section: ConflictSection | null = null;
  for (let lineNumber = 1; lineNumber <= state.doc.lines; lineNumber += 1) {
    const line = state.doc.line(lineNumber);
    const marker = conflictMarkerKind(line.text);
    if (marker) {
      const conflictIndex = blockByStartLine.get(lineNumber);
      if (marker === "start" && conflictIndex !== undefined && onResolve) {
        decorations.push(Decoration.widget({
          widget: new ConflictActionWidget(state.doc.toString(), conflictIndex, onResolve),
          side: -1,
        }).range(line.from));
      }
      decorations.push(Decoration.line({ class: `cmConflictMarkerLine ${markerLineClass(marker)}` }).range(line.from));
      if (line.from < line.to) {
        decorations.push(Decoration.mark({ class: `cmConflictMarkerText ${markerTextClass(marker)}` }).range(line.from, line.to));
      }
      if (marker === "start") section = "local";
      else if (marker === "base") section = "base";
      else if (marker === "separator") section = "incoming";
      else section = null;
      continue;
    }
    if (section) {
      decorations.push(Decoration.line({ class: sectionLineClass(section) }).range(line.from));
      if (line.from < line.to) {
        const textClass = section === "local"
          ? "cmConflictLocalText"
          : section === "base"
            ? "cmConflictBaseText"
            : "cmConflictIncomingText";
        decorations.push(Decoration.mark({ class: textClass }).range(line.from, line.to));
      }
    }
  }
  return Decoration.set(decorations, true);
}

export function conflictMarkersExtension(onResolve?: (content: string) => void) {
  return StateField.define({
    create: (state) => buildConflictDecorations(state, onResolve),
    update(decorations, transaction) {
      return transaction.docChanged
        ? buildConflictDecorations(transaction.state, onResolve)
        : decorations.map(transaction.changes);
    },
    provide: (field) => EditorView.decorations.from(field),
  });
}
