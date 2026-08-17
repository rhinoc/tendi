import { useMemo } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { syntaxHighlighting } from "@codemirror/language";
import { EditorView } from "@codemirror/view";

import { codeMirrorJson } from "../../lib/codemirror-json.ts";
import { codeMirrorBaseTheme, codeMirrorHighlightStyle } from "../../lib/codemirror-theme.ts";

const hookPreviewBasicSetup = {
  foldGutter: false,
  highlightActiveLine: false,
  highlightActiveLineGutter: false,
  lineNumbers: false,
  searchKeymap: true,
  syntaxHighlighting: false,
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
