import { useMemo } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { syntaxHighlighting } from "@codemirror/language";
import { markdown as codeMirrorMarkdown } from "@codemirror/lang-markdown";
import { EditorView } from "@codemirror/view";

import { codeMirrorBaseTheme, codeMirrorHighlightStyle } from "../../lib/codemirror-theme.ts";
import { codeMirrorPromptXmlTagExtension } from "../../lib/prompt-codemirror.ts";

const promptEditorBasicSetup = {
  foldGutter: false,
  highlightActiveLine: true,
  highlightActiveLineGutter: false,
  lineNumbers: false,
  searchKeymap: true,
};

export type PromptBodyEditorProps = {
  value: string;
  onChange: (value: string) => void;
};

export function PromptBodyEditor({ value, onChange }: PromptBodyEditorProps) {
  const extensions = useMemo(() => [
    codeMirrorBaseTheme,
    syntaxHighlighting(codeMirrorHighlightStyle),
    codeMirrorMarkdown(),
    codeMirrorPromptXmlTagExtension(),
    EditorView.lineWrapping,
  ], []);

  return (
    <CodeMirror
      className="codeMirrorEditor promptCodeMirrorEditor"
      value={value}
      height="100%"
      theme="none"
      basicSetup={promptEditorBasicSetup}
      extensions={extensions}
      onChange={onChange}
      placeholder="Write the prompt you want to reuse..."
    />
  );
}
