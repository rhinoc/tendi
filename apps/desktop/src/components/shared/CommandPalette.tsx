import { useEffect, useId, useLayoutEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { ArrowDown, ArrowUp, CornerDownLeft, Search } from "lucide-react";
import type { LucideIcon } from "lucide-react";

import {
  filterCommandPaletteItems,
  type CommandPaletteSearchItem,
} from "../../lib/command-palette.ts";
import { fixedVirtualRange } from "../../lib/virtualization.ts";
import { SegmentedControl, SegmentedControlItem } from "./SegmentedControl.tsx";
import { useVirtualViewport } from "./useVirtualViewport.ts";
import "./CommandPalette.css";

export type CommandPaletteScope = "current" | "all";

export type CommandPaletteItem = CommandPaletteSearchItem & {
  id: string;
  icon?: LucideIcon;
  onSelect: () => void;
};

const COMMAND_PALETTE_ROW_HEIGHT = 72;
export { filterCommandPaletteItems } from "../../lib/command-palette.ts";

export function CommandPalette({
  open,
  scope,
  items,
  onOpenChange,
  onScopeChange,
  emptyMessage = "No matching content.",
}: {
  open: boolean;
  scope: CommandPaletteScope;
  items: readonly CommandPaletteItem[];
  onOpenChange: (open: boolean) => void;
  onScopeChange: (scope: CommandPaletteScope) => void;
  emptyMessage?: string;
}) {
  const [query, setQuery] = useState("");
  const [activeIndex, setActiveIndex] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const listId = useId();
  const filteredItems = useMemo(() => filterCommandPaletteItems(items, query), [items, query]);
  const { scrollOffset, readViewportSize, scheduleScrollSync, syncScrollPosition } = useVirtualViewport(
    { width: 0, height: 420 },
    {
      ref: listRef,
      refreshKey: filteredItems,
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ height }) => height > 0,
      isEqual: (current, next) => current.height === next.height,
    },
  );
  const virtualRange = useMemo(() => {
    const range = fixedVirtualRange(
      filteredItems.length,
      scrollOffset,
      readViewportSize(),
      COMMAND_PALETTE_ROW_HEIGHT,
      5,
    );
    return {
      start: Math.min(range.start, activeIndex),
      end: Math.max(range.end, activeIndex + 1),
    };
  }, [activeIndex, filteredItems.length, readViewportSize, scrollOffset]);

  useEffect(() => {
    if (!open) return;
    setQuery("");
    setActiveIndex(0);
    const frame = requestAnimationFrame(() => inputRef.current?.focus());
    return () => cancelAnimationFrame(frame);
  }, [open]);

  useEffect(() => {
    if (!open) return;
    const root = document.documentElement;
    const previousRootOverflow = root.style.overflow;
    const previousBodyOverflow = document.body.style.overflow;
    root.style.overflow = "hidden";
    document.body.style.overflow = "hidden";
    return () => {
      root.style.overflow = previousRootOverflow;
      document.body.style.overflow = previousBodyOverflow;
    };
  }, [open]);

  useEffect(() => {
    setActiveIndex((current) => Math.min(current, Math.max(0, filteredItems.length - 1)));
  }, [filteredItems.length]);

  useLayoutEffect(() => {
    if (!open || !listRef.current) return;
    listRef.current.scrollTop = 0;
    syncScrollPosition();
  }, [open, query, syncScrollPosition]);

  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.isComposing) return;
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === "p") {
        event.preventDefault();
        event.stopPropagation();
        onScopeChange(event.shiftKey ? "all" : "current");
        onOpenChange(true);
        return;
      }
      if (event.key === "Escape") {
        if (!open) return;
        event.preventDefault();
        onOpenChange(false);
      }
    };
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [onOpenChange, onScopeChange, open]);

  useEffect(() => {
    if (!open) return;
    const list = listRef.current;
    if (!list || !filteredItems[activeIndex]) return;
    const targetTop = activeIndex * COMMAND_PALETTE_ROW_HEIGHT;
    const targetBottom = targetTop + COMMAND_PALETTE_ROW_HEIGHT;
    if (targetTop < list.scrollTop) list.scrollTo({ top: targetTop, behavior: "smooth" });
    else if (targetBottom > list.scrollTop + list.clientHeight) {
      list.scrollTo({ top: targetBottom - list.clientHeight, behavior: "smooth" });
    }
    scheduleScrollSync();
  }, [activeIndex, filteredItems, open, scheduleScrollSync]);

  const selectItem = (item: CommandPaletteItem) => {
    item.onSelect();
    onOpenChange(false);
  };

  const onInputKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setActiveIndex((current) => filteredItems.length ? (current + 1) % filteredItems.length : 0);
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setActiveIndex((current) => filteredItems.length ? (current - 1 + filteredItems.length) % filteredItems.length : 0);
    } else if (event.key === "Enter") {
      event.preventDefault();
      const item = filteredItems[activeIndex];
      if (item) selectItem(item);
    }
  };

  if (!open || typeof document === "undefined") return null;

  return createPortal(
    <div className="commandPaletteRoot">
      <button
        type="button"
        className="commandPaletteOverlay"
        aria-label="Close command palette"
        onClick={() => onOpenChange(false)}
      />
      <div className="commandPaletteLayer">
        <section
          className="commandPalettePanel"
          role="dialog"
          aria-modal="true"
          aria-label="Search content"
          onClick={(event) => event.stopPropagation()}
        >
          <div className="commandPaletteSearchRow">
            <Search size={17} aria-hidden="true" />
            <input
              ref={inputRef}
              value={query}
              onChange={(event) => {
                setQuery(event.target.value);
                setActiveIndex(0);
              }}
              onKeyDown={onInputKeyDown}
              placeholder={scope === "current" ? "Search this page…" : "Search all pages…"}
              aria-label={scope === "current" ? "Search this page" : "Search all pages"}
              role="combobox"
              aria-expanded="true"
              aria-controls={listId}
              aria-activedescendant={filteredItems[activeIndex] ? `${listId}-${activeIndex}` : undefined}
              aria-autocomplete="list"
            />
            <SegmentedControl
              className="commandPaletteScope"
              aria-label="Search scope"
              value={scope}
              onValueChange={(value) => {
                if (value === "current" || value === "all") onScopeChange(value);
              }}
            >
              <SegmentedControlItem value="current">This page</SegmentedControlItem>
              <SegmentedControlItem value="all">All pages</SegmentedControlItem>
            </SegmentedControl>
            <kbd>ESC</kbd>
          </div>
          <div ref={listRef} id={listId} className="commandPaletteResults" role="listbox" aria-label="Search results" onScroll={scheduleScrollSync}>
            {filteredItems.length === 0 ? (
              <div className="commandPaletteEmpty">{emptyMessage}</div>
            ) : (
              <div className="commandPaletteVirtualContent" style={{ height: filteredItems.length * COMMAND_PALETTE_ROW_HEIGHT }}>
                {filteredItems.slice(virtualRange.start, virtualRange.end).map((item, offset) => {
                  const index = virtualRange.start + offset;
                  const Icon = item.icon;
                  const active = index === activeIndex;
                  const showGroup = index === 0 || filteredItems[index - 1]?.group !== item.group;
                  return (
                    <div
                      className="commandPaletteVirtualRow"
                      key={item.id}
                      style={{ height: COMMAND_PALETTE_ROW_HEIGHT, transform: `translateY(${index * COMMAND_PALETTE_ROW_HEIGHT}px)` }}
                    >
                      {showGroup ? <div className="commandPaletteGroupLabel">{item.group}</div> : null}
                      <button
                        type="button"
                        id={`${listId}-${index}`}
                        className={`commandPaletteItem${active ? " active" : ""}`}
                        role="option"
                        aria-selected={active}
                        data-command-id={item.id}
                        onMouseEnter={() => setActiveIndex(index)}
                        onClick={() => selectItem(item)}
                      >
                        <span className="commandPaletteItemIcon" aria-hidden="true">{Icon ? <Icon size={16} /> : null}</span>
                        <span className="commandPaletteItemText">
                          <span className="commandPaletteItemLabel">{item.label}</span>
                          {item.detail ? <span className="commandPaletteItemDetail">{item.detail}</span> : null}
                        </span>
                        {scope === "all" && item.group !== "Pages" ? <span className="commandPaletteItemPage">{item.group}</span> : null}
                      </button>
                    </div>
                  );
                })}
              </div>
            )}
          </div>
          <footer className="commandPaletteFooter">
            <span><ArrowUp size={12} /><ArrowDown size={12} /> Navigate</span>
            <span><CornerDownLeft size={12} /> Open</span>
            <span><kbd>⌘ P</kbd> This page</span>
            <span><kbd>⇧ ⌘ P</kbd> All pages</span>
          </footer>
        </section>
      </div>
    </div>,
    document.body,
  );
}
