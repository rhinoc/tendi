import { Tooltip } from "./Tooltip.tsx";
import { lazy, Suspense, useCallback, useEffect, useRef, useState, type ReactNode } from "react";
import { Check, ChevronDown, ChevronUp, Code2, Eye, Save, Search, X } from "lucide-react";

import { isJsonPath, isMarkdownPath, isYamlPath } from "../../lib/index.ts";
import { loadCodeMirrorFileEditor } from "./code-mirror-loader.ts";
import type { CodeMirrorLanguage } from "./CodeMirrorFileEditor.tsx";
import { EditorStatePlaceholder } from "./EditorStatePlaceholder.tsx";
import { LoadingIcon } from "./LoadingIcon.tsx";
import { PlainTextFileEditor } from "./PlainTextFileEditor.tsx";
import { CopyButton } from "./CopyButton.tsx";
import { SearchClearButton } from "./SearchClearButton.tsx";

const CodeMirrorFileEditor = lazy(() => loadCodeMirrorFileEditor().then(({ CodeMirrorFileEditor: component }) => ({ default: component })));
const TokenStatusBar = lazy(() => import("../TokenStatusBar.tsx").then(({ TokenStatusBar: component }) => ({ default: component })));
const TiptapMarkdownPreview = lazy(() => import("./TiptapMarkdownPreview.tsx").then(({ TiptapMarkdownPreview: component }) => ({ default: component })));
const LARGE_PLAIN_TEXT_CHARS = 256 * 1024;

type DeferredTokenStatusBarProps = {
  activePath: string;
  content: string;
  selectionText: string;
  leadingSlot?: ReactNode;
};

function DeferredTokenStatusBar(props: DeferredTokenStatusBarProps) {
  const [ready, setReady] = useState(false);

  useEffect(() => {
    const timer = window.setTimeout(() => setReady(true), 300);
    return () => {
      window.clearTimeout(timer);
    };
  }, []);

  if (!ready) return null;
  return (
    <Suspense fallback={null}>
      <TokenStatusBar {...props} />
    </Suspense>
  );
}

export type DiffStats = {
  added: number;
  removed: number;
};

export type MarkdownFilePaneProps = {
  activePath: string;
  dirty: boolean;
  diffStats: DiffStats;
  content: string;
  originalContent: string;
  onChange: (value: string) => void;
  onSave: () => void;
  language?: CodeMirrorLanguage;
  readOnly?: boolean;
  onNormalize?: (value: string) => void;
  copyablePath?: boolean;
  showDirtyIndicator?: boolean;
  showTokenStatusBar?: boolean;
  saveState?: "idle" | "saving" | "saved" | "error";
  saveError?: string;
};

export function MarkdownFilePane({
  activePath,
  dirty,
  diffStats,
  content,
  originalContent,
  onChange,
  onSave,
  language,
  readOnly = false,
  copyablePath = false,
  showDirtyIndicator = true,
  showTokenStatusBar = true,
  saveState = "idle",
  saveError = "",
}: MarkdownFilePaneProps) {
  const markdown = language === "markdown" || (!language && isMarkdownPath(activePath));
  const activeLanguage = language ?? (
    isYamlPath(activePath) ? "yaml" : isJsonPath(activePath) ? "json" : undefined
  );
  const useCodeMirror = Boolean(activeLanguage) || markdown || content.length >= LARGE_PLAIN_TEXT_CHARS;
  const paneRef = useRef<HTMLElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const [searchIndex, setSearchIndex] = useState(0);
  const [searchCount, setSearchCount] = useState(0);
  const [searchHovered, setSearchHovered] = useState(false);
  const [showPreview, setShowPreview] = useState(false);
  const [showDiff, setShowDiff] = useState(false);
  const [selectionText, setSelectionText] = useState("");
  const showEditorDiff = !readOnly && !showPreview && showDiff;
  const hasDiff = diffStats.added > 0 || diffStats.removed > 0;
  const activeSearchIndex = searchCount > 0 ? Math.min(searchIndex, searchCount - 1) : 0;
  const searchLabel = searchQuery.trim()
    ? searchCount > 0
      ? `${activeSearchIndex + 1}/${searchCount}`
      : "No results"
    : "";

  const focusSearchInput = useCallback(() => {
    setSearchOpen(true);
    window.requestAnimationFrame(() => {
      searchInputRef.current?.focus();
      searchInputRef.current?.select();
    });
  }, []);

  const closeSearch = useCallback(() => {
    setSearchOpen(false);
    setSearchQuery("");
  }, []);

  const moveSearch = useCallback((direction: number) => {
    if (searchCount === 0) return;
    setSearchIndex((current) => (current + direction + searchCount) % searchCount);
  }, [searchCount]);

  const handleSearchMatchCount = useCallback((nextCount: number) => {
    setSearchCount(nextCount);
  }, []);

  useEffect(() => {
    setSearchIndex(0);
  }, [searchQuery]);

  useEffect(() => {
    setSearchIndex(0);
    setSearchCount(0);
  }, [activePath, showPreview]);

  useEffect(() => {
    setShowPreview(false);
    setShowDiff(false);
  }, [activePath]);

  useEffect(() => {
    setSelectionText("");
  }, [activePath, showPreview]);

  useEffect(() => {
    if (searchCount > 0 && searchIndex >= searchCount) setSearchIndex(searchCount - 1);
  }, [searchCount, searchIndex]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (!(event.metaKey || event.ctrlKey) || event.key.toLowerCase() !== "f") return;
      const pane = paneRef.current;
      if (!pane) return;
      const target = event.target instanceof Node ? event.target : null;
      const activeElement = document.activeElement;
      const inPane = target && pane.contains(target);
      const focusedInPane = activeElement && pane.contains(activeElement);
      if (!inPane && !focusedInPane && !searchHovered) return;
      event.preventDefault();
      event.stopPropagation();
      focusSearchInput();
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [focusSearchInput, searchHovered]);

  return (
    <section
      className={`codePane ${searchOpen ? "findOpen" : ""}`}
      ref={paneRef}
      onMouseEnter={() => setSearchHovered(true)}
      onMouseLeave={() => setSearchHovered(false)}
    >
      <div className="codeTabs">
        <div className="codeTabTitle">
          <Tooltip content={activePath} onlyWhenTruncated><span>{activePath}</span></Tooltip>
          {copyablePath && activePath ? (
            <CopyButton
              className="codeTabCopyButton"
              value={activePath}
              copyLabel="Copy path"
              copiedLabel="Path copied"
              iconSize={13}
              stopPropagation
            />
          ) : null}
          {!readOnly && showDirtyIndicator && dirty && <span className="dirty">modified</span>}
          {!readOnly && saveState === "saving" && <span className="editorSaveStatus">saving</span>}
          {!readOnly && saveState === "saved" && <span className="editorSaveStatus saved">saved</span>}
          {!readOnly && saveState === "error" && <span className="editorSaveStatus error" role="alert">{saveError || "save failed"}</span>}
        </div>
        <div className="codeTabActions">
          <button
            className={`editorIconButton ${searchOpen ? "active" : ""}`}
            aria-label="Find in file"
            aria-pressed={searchOpen}
            onClick={focusSearchInput}
          >
            <Search size={13} />
          </button>
          {markdown && (
            <button
              className="editorIconButton"
              aria-label={showPreview ? "Switch to edit" : "Switch to preview"}
              onClick={() => setShowPreview((value: boolean) => !value)}
            >
              {showPreview ? <Code2 size={13} /> : <Eye size={13} />}
            </button>
          )}
          {!readOnly && (
            <>
              <button
                className="editorIconButton"
                aria-label={saveState === "saving" ? "Saving file" : saveState === "saved" ? "File saved" : "Save file"}
                aria-busy={saveState === "saving"}
                onClick={onSave}
                disabled={!dirty || saveState === "saving"}
              >
                {saveState === "saving"
                  ? <LoadingIcon size={13} />
                  : saveState === "saved"
                    ? <Check size={13} strokeWidth={2.6} />
                    : <Save size={13} />}
              </button>
            </>
          )}
        </div>
      </div>
      {searchOpen && (
        <div className="editorFindBar" data-no-drag onMouseDown={(event) => event.stopPropagation()}>
          <Search size={14} />
          <input
            ref={searchInputRef}
            aria-label="Find in file"
            value={searchQuery}
            onChange={(event) => setSearchQuery(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Enter") {
                event.preventDefault();
                moveSearch(event.shiftKey ? -1 : 1);
              } else if (event.key === "Escape") {
                event.preventDefault();
                closeSearch();
              }
            }}
            placeholder="Find in file"
          />
          <SearchClearButton value={searchQuery} onClear={() => setSearchQuery("")} ariaLabel="Clear find query" />
          <span className={`editorFindCount ${searchQuery.trim() && searchCount === 0 ? "empty" : ""}`}>{searchLabel}</span>
          <button className="editorIconButton" aria-label="Previous match" disabled={searchCount === 0} onClick={() => moveSearch(-1)}>
            <ChevronUp size={13} />
          </button>
          <button className="editorIconButton" aria-label="Next match" disabled={searchCount === 0} onClick={() => moveSearch(1)}>
            <ChevronDown size={13} />
          </button>
          <button className="editorIconButton" aria-label="Close find" onClick={closeSearch}>
            <X size={13} />
          </button>
        </div>
      )}
      {markdown && showPreview ? (
        <Suspense fallback={<EditorStatePlaceholder label="Loading preview" />}>
          <TiptapMarkdownPreview
            content={content}
            searchQuery={searchQuery}
            searchIndex={activeSearchIndex}
            onSearchMatchCount={handleSearchMatchCount}
            onSelectionChange={setSelectionText}
          />
        </Suspense>
      ) : !useCodeMirror ? (
        <PlainTextFileEditor
          content={content}
          readOnly={readOnly}
          onChange={onChange}
          searchQuery={searchQuery}
          searchIndex={activeSearchIndex}
          onSearchMatchCount={handleSearchMatchCount}
          onSelectionChange={setSelectionText}
        />
      ) : (
        <Suspense fallback={<EditorStatePlaceholder label="Loading file" />}>
          <CodeMirrorFileEditor
            content={content}
            originalContent={originalContent}
            markdown={markdown}
            language={activeLanguage}
            showDiff={showEditorDiff}
            onChange={onChange}
            readOnly={readOnly}
            searchQuery={searchQuery}
            searchIndex={activeSearchIndex}
            onSearchMatchCount={handleSearchMatchCount}
            onSelectionChange={setSelectionText}
          />
        </Suspense>
      )}
      {showTokenStatusBar ? (
        <DeferredTokenStatusBar
          activePath={activePath}
          content={content}
          selectionText={selectionText}
          leadingSlot={!readOnly && markdown && hasDiff ? (
            <button
              type="button"
              className={`tokenDiffToggle ${showEditorDiff ? "active" : ""}`}
              aria-label={showEditorDiff ? "Hide diff" : "Show diff"}
              aria-pressed={showEditorDiff}
              onClick={() => {
                setShowPreview(false);
                setShowDiff((value) => !value);
              }}
            >
              <span className="tokenDiffValue added">+{diffStats.added}</span>
              <span className="tokenDiffValue removed">-{diffStats.removed}</span>
            </button>
          ) : undefined}
        />
      ) : null}
    </section>
  );
}
