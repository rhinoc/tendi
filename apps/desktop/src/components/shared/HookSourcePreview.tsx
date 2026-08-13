import { useMemo } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { syntaxHighlighting } from "@codemirror/language";
import { json as codeMirrorJson } from "@codemirror/lang-json";
import { EditorView } from "@codemirror/view";

import { codeMirrorBaseTheme, codeMirrorHighlightStyle } from "../../lib/codemirror-theme.ts";

const hookPreviewBasicSetup = {
  foldGutter: false,
  highlightActiveLine: false,
  highlightActiveLineGutter: false,
  lineNumbers: false,
  searchKeymap: true,
};

export function HookSourcePreview({ content }: { content: string }) {
  const extensions = useMemo(() => [
    codeMirrorBaseTheme,
    syntaxHighlighting(codeMirrorHighlightStyle),
    codeMirrorJson(),
    EditorView.lineWrapping,
    EditorView.editable.of(false),
  ], []);

  return (
    <CodeMirror
      className="codeMirrorEditor hookConfigCodeMirror"
      value={content}
      height="100%"
      theme="none"
      basicSetup={hookPreviewBasicSetup}
      extensions={extensions}
      editable={false}
      readOnly
    />
  );
}
