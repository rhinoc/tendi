import { useCallback, useEffect, useMemo, useRef } from "react";
import { TextSelection } from "@tiptap/pm/state";
import { EditorContent, useEditor, type Editor } from "@tiptap/react";

import { splitMarkdownFrontmatter } from "../../lib/file-tree.ts";
import { tiptapExtensions } from "../../lib/tiptap.ts";
import { prosemirrorTextRanges } from "./codemirror-search.ts";

export type TiptapMarkdownPreviewProps = {
  content: string;
  searchQuery?: string;
  searchIndex?: number;
  onSearchMatchCount?: (count: number) => void;
  onSelectionChange?: (text: string) => void;
};

export function TiptapMarkdownPreview({
  content,
  searchQuery = "",
  searchIndex = 0,
  onSearchMatchCount,
  onSelectionChange,
}: TiptapMarkdownPreviewProps) {
  const parts = useMemo(() => splitMarkdownFrontmatter(content), [content]);
  const lastBodyRef = useRef(parts.body);
  const emitSelection = useCallback((editorInstance: Editor | null) => {
    if (!editorInstance) {
      onSelectionChange?.("");
      return;
    }
    const selection = editorInstance.state?.selection;
    if (!selection || selection.empty) {
      onSelectionChange?.("");
      return;
    }
    onSelectionChange?.(editorInstance.state.doc.textBetween(selection.from, selection.to, "\n"));
  }, [onSelectionChange]);
  const editor = useEditor({
    extensions: tiptapExtensions,
    content: parts.body,
    contentType: "markdown",
    editable: false,
    immediatelyRender: false,
    onSelectionUpdate: ({ editor: nextEditor }) => emitSelection(nextEditor),
  });

  useEffect(() => {
    if (!editor || parts.body === lastBodyRef.current) return;
    lastBodyRef.current = parts.body;
    editor.commands.setContent(parts.body, { contentType: "markdown", emitUpdate: false });
  }, [editor, parts]);

  useEffect(() => {
    editor?.setEditable(false);
  }, [editor]);

  useEffect(() => {
    emitSelection(editor);
  }, [editor, emitSelection, parts.body]);

  const searchMatches = useMemo(
    () => editor ? prosemirrorTextRanges(editor.state.doc, searchQuery) : [],
    [content, editor, searchQuery],
  );

  useEffect(() => {
    onSearchMatchCount?.(searchMatches.length);
  }, [onSearchMatchCount, searchMatches.length]);

  useEffect(() => {
    if (!editor || !searchQuery.trim() || searchMatches.length === 0) return;
    const match = searchMatches[Math.min(searchIndex, searchMatches.length - 1)];
    if (!match) return;
    const selection = TextSelection.create(editor.state.doc, match.from, match.to);
    editor.view.dispatch(editor.state.tr.setSelection(selection).scrollIntoView());
  }, [editor, searchIndex, searchMatches, searchQuery]);

  return (
    <div className="tiptapEditorShell tiptapPreviewShell">
      <div className="tiptapEditorHost">
        <EditorContent className="tiptapEditor" editor={editor} />
      </div>
    </div>
  );
}
