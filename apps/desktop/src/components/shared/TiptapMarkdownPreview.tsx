import { useCallback, useEffect, useMemo, useRef } from "react";
import { TextSelection } from "@tiptap/pm/state";
import { EditorContent, useEditor, type Editor } from "@tiptap/react";

import { splitMarkdownFrontmatter } from "../../lib/file-tree.ts";
import { tiptapExtensions } from "../../lib/tiptap.ts";
import { prosemirrorTextRanges } from "./codemirror-search.ts";

export type TiptapMarkdownPreviewProps = {
  content: string;
  stripFrontmatter?: boolean;
  immediatelyRender?: boolean;
  searchQuery?: string;
  searchIndex?: number;
  onSearchMatchCount?: (count: number) => void;
  onSelectionChange?: (text: string) => void;
};

export function TiptapMarkdownPreview({
  content,
  stripFrontmatter = true,
  immediatelyRender = false,
  searchQuery = "",
  searchIndex = 0,
  onSearchMatchCount,
  onSelectionChange,
}: TiptapMarkdownPreviewProps) {
  const body = useMemo(
    () => stripFrontmatter ? splitMarkdownFrontmatter(content).body : content,
    [content, stripFrontmatter],
  );
  const lastBodyRef = useRef(body);
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
    content: body,
    contentType: "markdown",
    editable: false,
    immediatelyRender,
    onSelectionUpdate: ({ editor: nextEditor }) => emitSelection(nextEditor),
  });

  useEffect(() => {
    if (!editor || body === lastBodyRef.current) return;
    lastBodyRef.current = body;
    editor.commands.setContent(body, { contentType: "markdown", emitUpdate: false });
  }, [body, editor]);

  useEffect(() => {
    editor?.setEditable(false);
  }, [editor]);

  useEffect(() => {
    emitSelection(editor);
  }, [body, editor, emitSelection]);

  const searchMatches = useMemo(
    () => editor ? prosemirrorTextRanges(editor.state.doc, searchQuery) : [],
    [body, editor, searchQuery],
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
