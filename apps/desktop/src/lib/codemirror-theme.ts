import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";
import { tags as highlightTags } from "@lezer/highlight";

/** Gruvbox syntax colors, switched by the shared appearance tokens. */
export const codeMirrorHighlightStyle = HighlightStyle.define([
  { tag: highlightTags.heading, color: "var(--syntax-link)", fontWeight: "700" },
  { tag: highlightTags.strong, color: "var(--syntax-text)", fontWeight: "700" },
  { tag: highlightTags.emphasis, color: "var(--syntax-text)", fontStyle: "italic" },
  { tag: highlightTags.link, color: "var(--syntax-link)", textDecoration: "underline" },
  { tag: highlightTags.url, color: "var(--syntax-link)" },
  { tag: highlightTags.monospace, color: "var(--syntax-mono)" },
  { tag: highlightTags.angleBracket, color: "var(--syntax-bracket)" },
  { tag: highlightTags.tagName, color: "var(--syntax-tag)" },
  { tag: highlightTags.attributeName, color: "var(--syntax-number)" },
  { tag: highlightTags.attributeValue, color: "var(--syntax-string)" },
  { tag: highlightTags.keyword, color: "var(--syntax-keyword)", fontWeight: "600" },
  { tag: highlightTags.atom, color: "var(--syntax-atom)" },
  { tag: highlightTags.string, color: "var(--syntax-string)" },
  { tag: highlightTags.comment, color: "var(--syntax-comment)", fontStyle: "italic" },
  { tag: highlightTags.number, color: "var(--syntax-number)" },
  { tag: highlightTags.bool, color: "var(--syntax-atom)" },
  { tag: highlightTags.null, color: "var(--syntax-atom)" },
  { tag: highlightTags.propertyName, color: "var(--syntax-property)" },
  { tag: highlightTags.variableName, color: "var(--syntax-variable)" },
  { tag: highlightTags.className, color: "var(--syntax-variable)" },
  { tag: highlightTags.typeName, color: "var(--syntax-variable)" },
  { tag: highlightTags.operator, color: "var(--syntax-keyword)" },
  { tag: highlightTags.meta, color: "var(--syntax-comment)" },
  { tag: highlightTags.punctuation, color: "var(--syntax-muted)" },
  { tag: highlightTags.separator, color: "var(--syntax-muted)" },
  { tag: highlightTags.squareBracket, color: "var(--syntax-bracket)" },
  { tag: highlightTags.brace, color: "var(--syntax-bracket)" },
]);

export const codeMirrorBaseTheme = EditorView.theme({
  "&": {
    height: "100%",
    backgroundColor: "var(--editor-bg)",
    color: "var(--syntax-text)",
    fontSize: "var(--text-ui)",
  },
  ".cm-scroller": {
    fontFamily: "var(--font-mono)",
    lineHeight: "var(--leading-body)",
  },
  ".cm-content": {
    minHeight: "100%",
    padding: "18px 20px 48px",
    caretColor: "var(--accent)",
  },
  ".cm-cursor, .cm-dropCursor": {
    borderLeftColor: "var(--accent)",
  },
  ".cm-line": {
    padding: "0 8px",
  },
  ".cm-gutters": {
    backgroundColor: "var(--editor-bg)",
    color: "var(--disabled)",
    borderRight: "1px solid var(--line)",
  },
  ".cm-activeLine": {
    backgroundColor: "var(--hover-fill-strong)",
  },
  ".cm-activeLineGutter": {
    backgroundColor: "var(--active-fill)",
    color: "var(--muted)",
  },
  ".cm-selectionBackground": {
    backgroundColor: "var(--selection-fill)",
  },
  "&.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground": {
    backgroundColor: "var(--selection-fill-strong)",
  },
  ".cm-content ::selection, .cm-line::selection, .cm-line ::selection": {
    backgroundColor: "var(--selection-fill-strong)",
  },
  "&.cm-focused": {
    outline: "none",
  },
  ".cmDiffLineAdded": {
    backgroundColor: "var(--green-soft)",
  },
  ".cmDiffAddedCharChanged": {
    borderRadius: "3px",
    backgroundColor: "var(--green-soft-strong)",
  },
  ".cmConflictLocalLine": {
    backgroundColor: "var(--danger-soft)",
    boxShadow: "inset 3px 0 var(--danger)",
  },
  ".cmConflictBaseLine": {
    backgroundColor: "var(--surface-3)",
    boxShadow: "inset 3px 0 var(--muted)",
  },
  ".cmConflictIncomingLine": {
    backgroundColor: "var(--green-soft)",
    boxShadow: "inset 3px 0 var(--green)",
  },
  ".cmConflictLocalText": {
    color: "var(--danger-text) !important",
  },
  ".cmConflictBaseText": {
    color: "var(--muted) !important",
  },
  ".cmConflictIncomingText": {
    color: "var(--green) !important",
  },
  ".cmConflictLocalCharChanged": {
    borderRadius: "3px",
    backgroundColor: "var(--danger-soft-strong)",
  },
  ".cmConflictBaseCharChanged": {
    borderRadius: "3px",
    backgroundColor: "var(--line-strong)",
  },
  ".cmConflictIncomingCharChanged": {
    borderRadius: "3px",
    backgroundColor: "var(--green-soft-strong)",
  },
  ".cmConflictMarkerLine": {
    fontWeight: "650",
  },
  ".cmConflictLocalMarkerLine": {
    backgroundColor: "var(--danger-soft-strong)",
    color: "var(--danger-text)",
  },
  ".cmConflictBaseMarkerLine": {
    backgroundColor: "var(--surface-3)",
    color: "var(--muted)",
  },
  ".cmConflictSeparatorLine": {
    backgroundColor: "var(--accent-soft-strong)",
    color: "var(--accent)",
  },
  ".cmConflictIncomingMarkerLine": {
    backgroundColor: "var(--green-soft-strong)",
    color: "var(--green)",
  },
  ".cmConflictMarkerText": {
    fontWeight: "700",
  },
  ".cmConflictLocalMarkerText": {
    color: "var(--danger-text) !important",
  },
  ".cmConflictBaseMarkerText": {
    color: "var(--muted) !important",
  },
  ".cmConflictSeparatorText": {
    color: "var(--accent) !important",
  },
  ".cmConflictIncomingMarkerText": {
    color: "var(--green) !important",
  },
  ".cmConflictActionGroup": {
    display: "inline-flex",
    alignItems: "center",
    gap: "4px",
    marginRight: "10px",
    verticalAlign: "middle",
  },
  ".cmConflictActionButton": {
    padding: "2px 6px",
    border: "1px solid var(--line-strong)",
    borderRadius: "var(--radius-sm)",
    backgroundColor: "var(--surface-2)",
    color: "var(--text)",
    cursor: "pointer",
    fontFamily: "var(--font-sans)",
    fontSize: "var(--text-sm)",
    lineHeight: "var(--leading-body)",
  },
  ".cmConflictActionButton:hover": {
    backgroundColor: "var(--hover-fill-strong)",
  },
  ".cmConflictActionLocal": {
    borderColor: "var(--danger-soft-strong)",
    backgroundColor: "var(--danger-soft)",
    color: "var(--danger-text)",
  },
  ".cmConflictActionRemote": {
    borderColor: "var(--green-soft-strong)",
    backgroundColor: "var(--green-soft)",
    color: "var(--green)",
  },
  ".cmConflictActionBoth": {
    borderColor: "var(--accent-soft-strong)",
    backgroundColor: "var(--accent-soft)",
    color: "var(--accent)",
  },
}, { dark: false });

export { syntaxHighlighting };
