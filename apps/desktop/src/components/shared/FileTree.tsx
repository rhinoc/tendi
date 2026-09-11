import {
  AnimatePresence,
  motion,
  useReducedMotion,
} from "motion/react";
import { ChevronRight, File, Folder, FolderOpen } from "lucide-react";
import {
  useEffect,
  useRef,
  useState,
  type KeyboardEvent,
  type MouseEvent,
  type ReactElement,
  type ReactNode,
} from "react";
import { ContextMenu } from "radix-ui";

import { EASE_OUT, SPRING_LAYOUT, SPRING_SWAP } from "../../lib/ease.ts";
import { LoadingState } from "./LoadingState.tsx";
import "./FileTree.css";

const ROW_ENTER = { duration: 0.22, ease: EASE_OUT } as const;

export type FileTreeItem = {
  id: string;
  label: string;
  kind: "file" | "folder";
  depth: number;
  parentId?: string | null;
  position?: number;
  setSize?: number;
  expanded?: boolean;
  disabled?: boolean;
  icon?: ReactNode;
  className?: string;
};

export type FileTreeProps = {
  items: readonly FileTreeItem[];
  selectedId?: string | null;
  collapsed?: boolean;
  onCollapsedChange?: (collapsed: boolean) => void;
  onItemActivate?: (item: FileTreeItem) => void;
  onItemToggle?: (item: FileTreeItem) => void;
  onItemRename?: (item: FileTreeItem) => void;
  canRename?: (item: FileTreeItem) => boolean;
  onItemContextMenu?: (item: FileTreeItem) => void;
  onTreeKeyDown?: (event: KeyboardEvent<HTMLElement>, item: FileTreeItem) => void;
  renderItem?: (item: FileTreeItem, row: ReactElement) => ReactNode;
  renderTrailing?: (item: FileTreeItem) => ReactNode;
  renamingId?: string;
  renameValue?: string;
  onRenameValueChange?: (value: string) => void;
  onRenameCommit?: () => void | Promise<void>;
  onRenameCancel?: () => void;
  actions?: ReactNode;
  rootContextMenu?: ReactNode;
  loading?: boolean;
  loadingLabel?: string;
  title?: ReactNode;
  ariaLabel?: string;
  showHeader?: boolean;
  className?: string;
};

function DefaultIcon({ item, reduce }: { item: FileTreeItem; reduce: boolean }) {
  if (item.kind === "file") return <File size={16} />;
  if (reduce) return item.expanded ? <FolderOpen size={16} /> : <Folder size={16} />;
  return (
    <AnimatePresence initial={false} mode="popLayout">
      <motion.span
        key={item.expanded ? "open" : "closed"}
        initial={{ opacity: 0, scale: 0.75, rotate: item.expanded ? -8 : 8 }}
        animate={{ opacity: 1, scale: 1, rotate: 0 }}
        exit={{ opacity: 0, scale: 0.75, rotate: item.expanded ? 8 : -8 }}
        transition={SPRING_SWAP}
        className="fileTreeIconSwap"
      >
        {item.expanded ? <FolderOpen size={16} /> : <Folder size={16} />}
      </motion.span>
    </AnimatePresence>
  );
}

export function FileTree({
  items,
  selectedId = null,
  collapsed = false,
  onCollapsedChange,
  onItemActivate,
  onItemToggle,
  onItemRename,
  canRename,
  onItemContextMenu,
  onTreeKeyDown,
  renderItem,
  renderTrailing,
  renamingId,
  renameValue = "",
  onRenameValueChange,
  onRenameCommit,
  onRenameCancel,
  actions,
  rootContextMenu,
  loading = false,
  loadingLabel = "Loading files",
  title = "Files",
  ariaLabel = "Files",
  showHeader = true,
  className = "",
}: FileTreeProps) {
  const reduce = useReducedMotion() ?? false;
  const [focusedId, setFocusedId] = useState<string | null>(selectedId ?? items[0]?.id ?? null);
  const rowRefs = useRef(new Map<string, HTMLButtonElement>());
  const renameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (focusedId && items.some((item) => item.id === focusedId)) return;
    setFocusedId(items[0]?.id ?? null);
  }, [focusedId, items]);

  useEffect(() => {
    if (!renamingId) return;
    requestAnimationFrame(() => {
      renameInputRef.current?.focus();
      renameInputRef.current?.select();
    });
  }, [renamingId]);

  const focusRow = (id: string) => {
    setFocusedId(id);
    const row = rowRefs.current.get(id);
    if (row) row.focus();
    else requestAnimationFrame(() => rowRefs.current.get(id)?.focus());
  };

  const activate = (item: FileTreeItem) => {
    if (!item.disabled) onItemActivate?.(item);
  };

  const toggle = (item: FileTreeItem) => {
    if (item.disabled || item.kind !== "folder") return;
    (onItemToggle ?? onItemActivate)?.(item);
  };

  const handleClick = (event: MouseEvent<HTMLButtonElement>, item: FileTreeItem) => {
    if (event.detail > 1 || item.disabled) return;
    activate(item);
  };

  const handleDoubleClick = (event: MouseEvent<HTMLButtonElement>, item: FileTreeItem) => {
    event.preventDefault();
    if (item.disabled) return;
    const renameAllowed = Boolean(onItemRename) && (canRename?.(item) ?? true);
    if (renameAllowed) onItemRename?.(item);
    else activate(item);
  };

  const handleKeyDown = (event: KeyboardEvent<HTMLButtonElement>, item: FileTreeItem) => {
    onTreeKeyDown?.(event, item);
    if (event.defaultPrevented) return;
    const index = items.findIndex((candidate) => candidate.id === item.id);
    const previous = items[index - 1];
    const next = items[index + 1];

    if (event.key === "ArrowDown" && next) {
      event.preventDefault();
      focusRow(next.id);
    } else if (event.key === "ArrowUp" && previous) {
      event.preventDefault();
      focusRow(previous.id);
    } else if (event.key === "Home" && items[0]) {
      event.preventDefault();
      focusRow(items[0].id);
    } else if (event.key === "End" && items.at(-1)) {
      event.preventDefault();
      focusRow(items.at(-1)!.id);
    } else if (event.key === "ArrowRight" && item.kind === "folder") {
      event.preventDefault();
      if (!item.expanded) toggle(item);
      else if (next?.parentId === item.id) focusRow(next.id);
    } else if (event.key === "ArrowLeft") {
      event.preventDefault();
      if (item.kind === "folder" && item.expanded) toggle(item);
      else if (item.parentId) focusRow(item.parentId);
    } else if (event.key === "Enter" || event.key === " ") {
      event.preventDefault();
      if (item.kind === "folder") toggle(item);
      else activate(item);
    }
  };

  const rows = loading ? (
    <LoadingState className="fileTreeLoading" label={loadingLabel} />
  ) : (
    <div
      className="fileTreeRows"
      role="tree"
      aria-label={ariaLabel}
    >
      {items.map((item, index) => {
        const isSelected = selectedId === item.id;
        const isRenaming = renamingId === item.id;
        const rowStyle = { paddingLeft: `${8 + item.depth * 18}px` };
        const icon = (
          <span className="treeFileIcon" aria-hidden="true">
            {item.icon ?? <DefaultIcon item={item} reduce={reduce} />}
          </span>
        );
        const rowClassName = [
          "fileItem",
          item.kind === "folder" ? "folderItem" : "",
          isSelected ? "selected" : "",
          item.disabled ? "disabled" : "",
          item.className ?? "",
        ].filter(Boolean).join(" ");
        const row = isRenaming ? (
          <div className={`${rowClassName} renaming`} style={rowStyle}>
            <span className="treeChevron empty" aria-hidden="true" />
            {icon}
            <input
              ref={renameInputRef}
              aria-label={`Rename ${item.label}`}
              value={renameValue}
              onBlur={() => { void onRenameCommit?.(); }}
              onChange={(event) => onRenameValueChange?.(event.target.value)}
              onKeyDown={(event) => {
                event.stopPropagation();
                if (event.key === "Enter") {
                  event.preventDefault();
                  void onRenameCommit?.();
                } else if (event.key === "Escape") {
                  event.preventDefault();
                  onRenameCancel?.();
                }
              }}
            />
          </div>
        ) : (
          <button
            ref={(node) => {
              if (node) rowRefs.current.set(item.id, node);
              else rowRefs.current.delete(item.id);
            }}
            type="button"
            role="treeitem"
            aria-level={item.depth + 1}
            aria-posinset={item.position ?? index + 1}
            aria-setsize={item.setSize ?? items.length}
            aria-selected={isSelected}
            aria-expanded={item.kind === "folder" ? item.expanded : undefined}
            aria-disabled={item.disabled || undefined}
            tabIndex={focusedId === item.id ? 0 : -1}
            onFocus={() => setFocusedId(item.id)}
            onKeyDown={(event) => handleKeyDown(event, item)}
            onClick={(event) => handleClick(event, item)}
            onContextMenu={() => {
              onItemContextMenu?.(item);
            }}
            onDoubleClick={(event) => handleDoubleClick(event, item)}
            className={rowClassName}
            style={rowStyle}
          >
            <motion.span
              aria-hidden="true"
              onClick={(event) => {
                if (item.kind !== "folder") return;
                event.preventDefault();
                event.stopPropagation();
                toggle(item);
              }}
              onDoubleClick={(event) => {
                event.preventDefault();
                event.stopPropagation();
              }}
              animate={{ rotate: item.kind === "folder" && item.expanded ? 90 : 0 }}
              transition={reduce ? { duration: 0 } : SPRING_SWAP}
              className={`treeChevron ${item.kind === "file" ? "empty" : ""}`}
            >
              <ChevronRight size={14} />
            </motion.span>
            {icon}
            <span className="fileItemName">{item.label}</span>
            {renderTrailing?.(item)}
          </button>
        );
        const renderedRow = renderItem?.(item, row) ?? row;
        return (
          <motion.div
            layout={reduce ? false : "position"}
            key={item.id}
            initial={reduce ? false : { opacity: 0, y: -6 }}
            animate={{
              opacity: item.disabled ? 0.42 : 1,
              y: 0,
              transition: reduce
                ? { duration: 0 }
                : { ...ROW_ENTER, delay: Math.min((item.position ?? index + 1) * 0.025, 0.1) },
            }}
            transition={reduce ? { duration: 0 } : SPRING_LAYOUT}
            className="fileTreeMotionRow"
          >
            {renderedRow}
          </motion.div>
        );
      })}
    </div>
  );

  return (
    <aside className={["fileTree", collapsed ? "collapsed" : "", className].filter(Boolean).join(" ")}>
      <ContextMenu.Root>
        <ContextMenu.Trigger asChild>
          <div className="fileTreeBody">
            {showHeader ? (
              <div className="fileTreeHeader">
                <button
                  type="button"
                  className="fileTreeToggle"
                  aria-label={collapsed ? "Expand files" : "Collapse files"}
                  onClick={() => onCollapsedChange?.(!collapsed)}
                >
                  <ChevronRight className={collapsed ? "" : "fileTreeHeaderChevronExpanded"} size={14} />
                  {!collapsed && <span>{title}</span>}
                </button>
                {!collapsed && actions ? <div className="fileTreeActions">{actions}</div> : null}
              </div>
            ) : null}
            {!collapsed ? rows : null}
          </div>
        </ContextMenu.Trigger>
        {!collapsed ? rootContextMenu : null}
      </ContextMenu.Root>
    </aside>
  );
}
