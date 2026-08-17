import { useCallback, useEffect, useMemo, useRef } from "react";

import { findTextRanges } from "./text-ranges.ts";

export type PlainTextFileEditorProps = {
  content: string;
  onChange?: (value: string) => void;
  readOnly?: boolean;
  searchQuery?: string;
  searchIndex?: number;
  onSearchMatchCount?: (count: number) => void;
  onSelectionChange?: (text: string) => void;
};

export function PlainTextFileEditor({
  content,
  onChange,
  readOnly = false,
  searchQuery = "",
  searchIndex = 0,
  onSearchMatchCount,
  onSelectionChange,
}: PlainTextFileEditorProps) {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const searchMatches = useMemo(() => findTextRanges(content, searchQuery), [content, searchQuery]);
  const emitSelection = useCallback(() => {
    const textarea = textareaRef.current;
    if (!textarea || textarea.selectionStart === textarea.selectionEnd) {
      onSelectionChange?.("");
      return;
    }
    onSelectionChange?.(content.slice(textarea.selectionStart, textarea.selectionEnd));
  }, [content, onSelectionChange]);

  useEffect(() => {
    onSearchMatchCount?.(searchMatches.length);
  }, [onSearchMatchCount, searchMatches.length]);

  useEffect(() => {
    const textarea = textareaRef.current;
    if (!textarea || !searchQuery.trim() || searchMatches.length === 0) return;
    const match = searchMatches[Math.min(searchIndex, searchMatches.length - 1)];
    if (!match) return;
    let line = 0;
    for (let index = 0; index < match.from; index += 1) {
      if (content[index] === "\n") line += 1;
    }
    const lineHeight = Number.parseFloat(window.getComputedStyle(textarea).lineHeight) || 20;
    textarea.scrollTop = Math.max(0, line * lineHeight - textarea.clientHeight / 2);
  }, [content, searchIndex, searchMatches, searchQuery]);

  return (
    <textarea
      ref={textareaRef}
      className="plainTextEditor"
      spellCheck={false}
      value={content}
      readOnly={readOnly}
      onChange={readOnly ? undefined : (event) => onChange?.(event.target.value)}
      onKeyUp={emitSelection}
      onMouseUp={emitSelection}
      onSelect={emitSelection}
    />
  );
}
