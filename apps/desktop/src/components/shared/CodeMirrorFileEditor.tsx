import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import type { Extension } from "@codemirror/state";
import { EditorView, type ViewUpdate } from "@codemirror/view";

import { codeMirrorSearchExtension } from "./codemirror-search.ts";
import { EditorStatePlaceholder } from "./EditorStatePlaceholder.tsx";
import { findTextRanges } from "./text-ranges.ts";

const codeMirrorBasicSetup = {
  foldGutter: false,
  highlightActiveLine: true,
  highlightActiveLineGutter: true,
  searchKeymap: true,
  syntaxHighlighting: false,
};

export type CodeMirrorLanguage = "json" | "markdown" | "toml" | "yaml";

async function loadCodeMirrorModules(activeLanguage: CodeMirrorLanguage | undefined, showDiff: boolean) {
  const { codeMirrorBaseTheme, codeMirrorHighlightStyle, syntaxHighlighting } = await import("../../lib/codemirror-theme.ts");
  let languageExtension: Extension | undefined;
  let promptXmlTagExtension: Extension | undefined;
  if (activeLanguage === "markdown") {
    const [{ markdown }, { codeMirrorPromptXmlTagExtension }] = await Promise.all([
      import("@codemirror/lang-markdown"),
      import("../../lib/prompt-codemirror.ts"),
    ]);
    languageExtension = markdown();
    promptXmlTagExtension = codeMirrorPromptXmlTagExtension();
  } else if (activeLanguage === "json") {
    languageExtension = (await import("../../lib/codemirror-json.ts")).codeMirrorJson();
  } else if (activeLanguage === "toml") {
    const [{ StreamLanguage }, { toml }] = await Promise.all([
      import("@codemirror/language"),
      import("@codemirror/legacy-modes/mode/toml"),
    ]);
    languageExtension = StreamLanguage.define(toml);
  } else if (activeLanguage === "yaml") {
    const [{ StreamLanguage }, { yaml }] = await Promise.all([
      import("@codemirror/language"),
      import("@codemirror/legacy-modes/mode/yaml"),
    ]);
    languageExtension = StreamLanguage.define(yaml);
  }
  const diffExtensionFactory = showDiff
    ? (await import("../../lib/codemirror-diff.ts")).editableDiffExtension
    : undefined;
  return {
    activeLanguage,
    baseTheme: codeMirrorBaseTheme,
    diffExtensionFactory,
    highlightStyle: codeMirrorHighlightStyle,
    languageExtension,
    promptXmlTagExtension,
    showDiff,
    syntaxHighlighting,
  };
}

export type CodeMirrorFileEditorProps = {
  content: string;
  originalContent?: string;
  markdown?: boolean;
  language?: CodeMirrorLanguage;
  showDiff?: boolean;
  onChange?: (value: string) => void;
  readOnly?: boolean;
  searchQuery?: string;
  searchIndex?: number;
  onSearchMatchCount?: (count: number) => void;
  onSelectionChange?: (text: string) => void;
};

export function CodeMirrorFileEditor({
  content,
  originalContent,
  markdown,
  language,
  showDiff = false,
  onChange,
  readOnly = false,
  searchQuery = "",
  searchIndex = 0,
  onSearchMatchCount,
  onSelectionChange,
}: CodeMirrorFileEditorProps) {
  const editorViewRef = useRef<EditorView | null>(null);
  const [loadedModules, setLoadedModules] = useState<Awaited<ReturnType<typeof loadCodeMirrorModules>> | null>(null);
  const onChangeRef = useRef(onChange);
  const onSelectionChangeRef = useRef(onSelectionChange);
  onChangeRef.current = onChange;
  onSelectionChangeRef.current = onSelectionChange;
  const searchMatches = useMemo(() => findTextRanges(content, searchQuery), [content, searchQuery]);
  const emitSelection = useCallback((view: EditorView) => {
    const selected = view.state.selection.ranges
      .filter((range) => !range.empty)
      .map((range) => view.state.sliceDoc(range.from, range.to))
      .join("\n");
    onSelectionChangeRef.current?.(selected);
  }, []);
  const handleChange = useCallback((value: string) => {
    onChangeRef.current?.(value);
  }, []);
  const handleCreateEditor = useCallback((view: EditorView) => {
    editorViewRef.current = view;
    emitSelection(view);
  }, [emitSelection]);
  const handleUpdate = useCallback((update: ViewUpdate) => {
    if (update.selectionSet || update.docChanged) emitSelection(update.view);
  }, [emitSelection]);
  const activeLanguage = language ?? (markdown ? "markdown" : undefined);
  useEffect(() => {
    let cancelled = false;
    editorViewRef.current = null;
    setLoadedModules(null);
    void loadCodeMirrorModules(activeLanguage, showDiff).then((next) => {
      if (!cancelled) setLoadedModules(next);
    });
    return () => {
      cancelled = true;
    };
  }, [activeLanguage, showDiff]);
  const shellExtensions = useMemo<Extension[]>(() => [
    EditorView.lineWrapping,
    EditorView.editable.of(!readOnly),
  ], [readOnly]);
  const extensions = useMemo(() => {
    if (!loadedModules || loadedModules.activeLanguage !== activeLanguage || loadedModules.showDiff !== showDiff) return shellExtensions;
    const next: Extension[] = [
      loadedModules.baseTheme,
      ...shellExtensions,
    ];
    if (loadedModules.languageExtension) next.push(loadedModules.languageExtension);
    next.push(loadedModules.syntaxHighlighting(loadedModules.highlightStyle));
    if (loadedModules.promptXmlTagExtension) next.push(loadedModules.promptXmlTagExtension);
    if (loadedModules.diffExtensionFactory) next.push(loadedModules.diffExtensionFactory(originalContent ?? ""));
    if (searchQuery.trim()) {
      next.push(codeMirrorSearchExtension(
        searchQuery,
        Math.min(searchIndex, searchMatches.length - 1),
        searchMatches,
      ));
    }
    return next;
  }, [activeLanguage, loadedModules, originalContent, searchIndex, searchMatches, searchQuery, shellExtensions, showDiff]);

  useEffect(() => {
    onSearchMatchCount?.(searchMatches.length);
  }, [onSearchMatchCount, searchMatches.length]);

  useEffect(() => {
    const view = editorViewRef.current;
    if (!view || !searchQuery.trim() || searchMatches.length === 0) return;
    const match = searchMatches[Math.min(searchIndex, searchMatches.length - 1)];
    if (!match) return;
    view.dispatch({
      effects: EditorView.scrollIntoView(match.from, { y: "center" }),
    });
  }, [searchIndex, searchMatches, searchQuery]);

  if (!loadedModules || loadedModules.activeLanguage !== activeLanguage || loadedModules.showDiff !== showDiff) {
    return <EditorStatePlaceholder label="Loading file" />;
  }

  return (
    <CodeMirror
      className="codeMirrorEditor"
      value={content}
      height="100%"
      theme="none"
      basicSetup={codeMirrorBasicSetup}
      extensions={extensions}
      onChange={readOnly ? undefined : handleChange}
      onCreateEditor={handleCreateEditor}
      onUpdate={handleUpdate}
    />
  );
}
