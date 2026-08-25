import { Tooltip } from "./shared/Tooltip.tsx";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useLayoutEffect,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import {
  flexRender,
  getCoreRowModel,
  getGroupedRowModel,
  getSortedRowModel,
  useReactTable,
  type ColumnDef as TanStackColumnDef,
  type GroupingState,
  type Row,
  type RowSelectionState,
  type SortingState,
  type Updater,
} from "@tanstack/react-table";
import { ChevronDown, ListTree } from "lucide-react";
import { Accordion, ContextMenu } from "radix-ui";

import { FreezeColumnResizeHandle, useFreezeColumnResize } from "./shared/freeze-column.tsx";
import { LoadingState } from "./shared/LoadingState.tsx";
import { SelectionActionBar } from "./shared/SelectionActionBar.tsx";
import { SelectionCheckbox } from "./shared/SelectionCheckbox.tsx";
import { useElementSize } from "./shared/useElementSize.ts";
import {
  MARQUEE_DRAG_THRESHOLD,
  clientPointFromContent,
  clientRectFromPoints,
  contentPointFromClient,
  dayGroupKey,
  elementContentRect,
  formatDayGroupLabel,
  marqueeAutoScrollDelta,
  rectFromPoints,
  rectsIntersect,
  suppressNextClick,
} from "../lib/index.ts";
import type { ColumnDef, DataTableProps, SortState } from "./DataTable.types";
import "./DataTable.css";

export type { ColumnDef, DataTableProps, FreezeColumnConfig, SortState } from "./DataTable.types";

type MarqueeRect = { left: number; top: number; width: number; height: number };

type MarqueeDrag = {
  active: boolean;
  start: { x: number; y: number };
  startContent: { x: number; y: number };
  current: { x: number; y: number };
  baseSelected: Set<string>;
  additive: boolean;
  autoScrollFrame: number | null;
  cleanup: (() => void) | null;
};

type DataTableColumnMeta<TRow> = {
  source: ColumnDef<TRow>;
};

type DataTableGroupLayout = {
  bodyTop: number;
  bodyHeight: number;
  expanded: boolean;
};

const EMPTY_GROUP_KEY = "__empty__";
const DATA_TABLE_ROW_HEIGHT = 58;
const DATA_TABLE_VIRTUAL_THRESHOLD = 40;
const DATA_TABLE_VIRTUAL_OVERSCAN = 4;
const DATA_TABLE_GROUP_HEADER_HEIGHT = 34;
const DATA_TABLE_GROUP_GAP = 18;
const DATA_TABLE_INTERACTIVE_SELECTOR = "button, a, input, textarea, select, [role='button'], [role='menuitem'], [role='menuitemcheckbox'], [role='menuitemradio'], [data-no-row-click]";
const DATA_TABLE_MARQUEE_BLOCK_SELECTOR = [
  DATA_TABLE_INTERACTIVE_SELECTOR,
  ".visibility",
  ".sectionHeader",
  "[data-no-drag]",
  "[data-selectable-text]",
].join(", ");

function columnDisplayValue<TRow>(column: ColumnDef<TRow>, row: TRow): ReactNode {
  const raw = column.value ? column.value(row) : row[column.key as keyof TRow];
  if (raw === null || raw === undefined || raw === "") return column.empty ?? "-";
  return raw as ReactNode;
}

function columnLabel<TRow>(column: ColumnDef<TRow>) {
  return column.header ?? column.label ?? "";
}

function columnType<TRow>(column: ColumnDef<TRow>) {
  if (column.type) return column.type;
  if (column.groupByDay) return "date";
  if (column.groupable === true || typeof column.groupBy === "function" || column.groupOrder) return "enum";
  return "text";
}

function groupsByDay<TRow>(column: ColumnDef<TRow>) {
  return columnType(column) === "date" || column.groupByDay;
}

function canGroupColumn<TRow>(column: ColumnDef<TRow>) {
  if (!columnLabel(column) || column.groupable === false) return false;
  const type = columnType(column);
  return type === "date" || type === "enum";
}

function normalizeGroupKey(raw: unknown) {
  const text = `${raw ?? ""}`;
  return text === "" ? EMPTY_GROUP_KEY : text;
}

function defaultGroupLabel(key: string) {
  if (key === EMPTY_GROUP_KEY) return "Unknown";
  return key;
}

function columnGroupKey<TRow>(column: ColumnDef<TRow>) {
  return column.groupKey ?? column.key;
}

function isSortableColumn<TRow>(column: ColumnDef<TRow> | undefined): column is ColumnDef<TRow> {
  if (!column) return false;
  if (column.sortable === false) return false;
  return Boolean(columnLabel(column));
}

function primitiveColumnValue<TRow>(column: ColumnDef<TRow>, row: TRow) {
  if (column.sortValue) return column.sortValue(row);
  const value = row[column.key as keyof TRow];
  return typeof value === "string" || typeof value === "number" ? value : `${value ?? ""}`;
}

function groupColumnValue<TRow>(column: ColumnDef<TRow>, row: TRow) {
  if (typeof column.groupBy === "function") return normalizeGroupKey(column.groupBy(row));
  if (groupsByDay(column)) return normalizeGroupKey(dayGroupKey(row[column.key as keyof TRow]));
  if (column.value) return normalizeGroupKey(column.value(row));
  return normalizeGroupKey(row[column.key as keyof TRow]);
}

function compareColumnValue(left: unknown, right: unknown) {
  if (typeof left === "number" || typeof right === "number") {
    return (Number(left) || 0) - (Number(right) || 0);
  }
  return `${left ?? ""}`.localeCompare(`${right ?? ""}`);
}

function renderTextCell<TRow>(column: ColumnDef<TRow>, row: TRow) {
  const display = columnDisplayValue(column, row);
  const className = column.cell === "number"
    ? "dataCellNumber"
    : column.cell === "title"
      ? "dataCellTitle"
      : "dataCellText";
  const explicitTitle = column.title?.(row);
  const title = explicitTitle
    ?? (typeof display === "string" && display !== (column.empty ?? "-") ? display : undefined);
  return <Tooltip content={title} interactive={Boolean(explicitTitle)} onlyWhenTruncated={!explicitTitle}><span className={className}>{display}</span></Tooltip>;
}

function renderCell<TRow>(column: ColumnDef<TRow>, row: TRow): ReactNode {
  if (column.render) return column.render(row);
  const raw = column.value ? column.value(row) : row[column.key as keyof TRow];
  if (raw !== null && raw !== undefined && raw !== "" && typeof raw !== "string" && typeof raw !== "number") {
    return raw as ReactNode;
  }
  return renderTextCell(column, row);
}

function sortStateToTable(sort: SortState | null): SortingState {
  return sort ? [{ id: sort.key, desc: sort.direction === "desc" }] : [];
}

function tableSortingToSortState(sorting: SortingState): SortState | null {
  const item = sorting[0];
  return item ? { key: item.id, direction: item.desc ? "desc" : "asc" } : null;
}

function selectedIdsToRowSelection(ids: string[]): RowSelectionState {
  return Object.fromEntries(ids.map((id) => [id, true]));
}

function applyUpdater<T>(updater: Updater<T>, current: T): T {
  return typeof updater === "function" ? (updater as (value: T) => T)(current) : updater;
}

function sameStringArray(left: readonly string[], right: readonly string[]) {
  return left.length === right.length && left.every((value, index) => value === right[index]);
}

function metaFor<TRow>(column: { columnDef: { meta?: unknown } }) {
  return column.columnDef.meta as DataTableColumnMeta<TRow>;
}

export function DataTable<TRow extends Record<string, unknown>>({
  rows,
  columns,
  getRowId = (row) => `${(row as { id?: string }).id ?? ""}`,
  getRowLabel = (row) => {
    const record = row as { selectionLabel?: string; name?: string; title?: string };
    return record.selectionLabel ?? record.name ?? record.title;
  },
  selectable = false,
  selectedIds,
  onSelectionChange,
  enableMarquee = false,
  defaultGroupBy = null,
  groupBy: controlledGroupBy,
  onGroupByChange,
  groupOrder: groupOrderOverride,
  groupLabel = defaultGroupLabel,
  defaultSort = null,
  sort: controlledSort,
  onSortChange,
  manualSorting = false,
  rowHeight = DATA_TABLE_ROW_HEIGHT,
  scrollToRowId,
  onScrollToRowComplete,
  freezeColumn,
  onRowClick,
  rowProps,
  rowContextMenu,
  bottomBar,
  bottomBarCheckboxLabel = "Select rows",
  selectionLabel = "selected",
  loading = false,
  loadingLabel = "Loading",
  emptyState = "No items",
}: DataTableProps<TRow>) {
  const shellRef = useRef<HTMLDivElement>(null);
  const scrollAnchorRef = useRef<{ id: string; offsetTop: number } | null>(null);
  const { ref: scrollRef, size: scrollSize } = useElementSize<HTMLDivElement>(
    { width: 0, height: 720 },
    {
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ height }) => height > 0,
      isEqual: (current, next) => current.height === next.height,
    },
  );

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll) return undefined;
    const anchor = scrollAnchorRef.current;
    if (anchor) {
      const viewport = scroll.getBoundingClientRect();
      const row = [...scroll.querySelectorAll<HTMLElement>("[data-row-id]")]
        .find((candidate) => candidate.dataset.rowId === anchor.id && candidate.getBoundingClientRect().bottom > viewport.top);
      if (row) {
        const currentOffsetTop = row.getBoundingClientRect().top - viewport.top;
        scroll.scrollTop += currentOffsetTop - anchor.offsetTop;
      }
      scrollAnchorRef.current = null;
    }
    return () => {
      const currentScroll = scrollRef.current;
      if (!currentScroll) return;
      const viewport = currentScroll.getBoundingClientRect();
      const row = [...currentScroll.querySelectorAll<HTMLElement>("[data-row-id]")]
        .find((candidate) => candidate.getBoundingClientRect().bottom > viewport.top);
      if (row) {
        scrollAnchorRef.current = {
          id: row.dataset.rowId ?? "",
          offsetTop: row.getBoundingClientRect().top - viewport.top,
        };
      }
    };
  }, [rows]);
  const marqueeDragRef = useRef<MarqueeDrag | null>(null);
  const suppressClickRef = useRef(false);
  const [marquee, setMarquee] = useState<MarqueeRect | null>(null);
  const [hoveredRowId, setHoveredRowId] = useState<string | null>(null);
  const [menuRowId, setMenuRowId] = useState<string | null>(null);
  const [scrollLeft, setScrollLeft] = useState(0);
  const [scrollTop, setScrollTop] = useState(0);
  const scrollUpdateFrameRef = useRef<number | null>(null);
  const [expandedGroups, setExpandedGroups] = useState<string[]>([]);
  const previousGroupStateRef = useRef<{ groupingKey: string; groupKeys: string[] } | null>(null);
  const groupingControlled = controlledGroupBy !== undefined;

  const defaultGroupColumn = useMemo(() => {
    if (!defaultGroupBy) return null;
    return columns.find((column) => columnGroupKey(column) === defaultGroupBy || column.key === defaultGroupBy) ?? null;
  }, [columns, defaultGroupBy]);
  const defaultGroupingKey = defaultGroupColumn?.key ?? null;
  const controlledGroupColumn = useMemo(() => {
    if (!controlledGroupBy) return null;
    return columns.find((column) => columnGroupKey(column) === controlledGroupBy || column.key === controlledGroupBy) ?? null;
  }, [columns, controlledGroupBy]);
  const [internalGrouping, setInternalGrouping] = useState<GroupingState>(defaultGroupingKey ? [defaultGroupingKey] : []);
  const controlledGroupingKey = controlledGroupColumn?.key ?? null;
  const grouping = useMemo(
    () => groupingControlled
      ? (controlledGroupingKey ? [controlledGroupingKey] : [])
      : internalGrouping,
    [controlledGroupingKey, groupingControlled, internalGrouping],
  );

  useEffect(() => {
    if (groupingControlled) return;
    setInternalGrouping((current) => {
      const currentKey = current[0];
      if (currentKey && columns.some((column) => column.key === currentKey)) return current;
      return defaultGroupingKey ? [defaultGroupingKey] : [];
    });
  }, [columns, defaultGroupingKey, groupingControlled]);

  const [internalSelected, setInternalSelected] = useState<string[]>([]);
  const selected = selectedIds ?? internalSelected;
  const rowSelection = useMemo(() => selectedIdsToRowSelection(selected), [selected]);
  const setSelected = useCallback(
    (next: string[] | ((current: string[]) => string[])) => {
      const value = typeof next === "function" ? next(selected) : next;
      if (onSelectionChange) onSelectionChange(value);
      if (selectedIds === undefined) setInternalSelected(value);
    },
    [onSelectionChange, selected, selectedIds],
  );

  const [internalSort, setInternalSort] = useState<SortState | null>(defaultSort);
  const sort = controlledSort ?? internalSort;
  const sorting = useMemo(() => sortStateToTable(sort), [sort]);

  const isSelectable = useCallback(
    (row: TRow) => (typeof selectable === "function" ? selectable(row) : Boolean(selectable)),
    [selectable],
  );

  const activeGroupColumn = useMemo(() => {
    const key = grouping[0];
    if (!key) return null;
    return columns.find((column) => column.key === key) ?? null;
  }, [columns, grouping]);

  const effectiveGroupLabel = useMemo(() => {
    if (!activeGroupColumn) return groupLabel;
    if (typeof activeGroupColumn.groupLabel === "function") return activeGroupColumn.groupLabel;
    if (groupsByDay(activeGroupColumn)) {
      return (key: string) => (key === EMPTY_GROUP_KEY ? "Unknown" : formatDayGroupLabel(key));
    }
    return groupLabel;
  }, [activeGroupColumn, groupLabel]);

  const effectiveGroupOrder = useMemo(() => {
    if (!activeGroupColumn) return groupOrderOverride;
    return activeGroupColumn.groupOrder ?? groupOrderOverride;
  }, [activeGroupColumn, groupOrderOverride]);

  const dataColumns = useMemo<TanStackColumnDef<TRow, unknown>[]>(() => (
    columns.map((column) => ({
      id: column.key,
      header: columnLabel(column),
      accessorFn: (row) => primitiveColumnValue(column, row),
      cell: ({ row }) => renderCell(column, row.original),
      enableSorting: isSortableColumn(column),
      enableGrouping: canGroupColumn(column),
      getGroupingValue: (row) => groupColumnValue(column, row),
      sortingFn: (rowA, rowB) => compareColumnValue(
        primitiveColumnValue(column, rowA.original),
        primitiveColumnValue(column, rowB.original),
      ),
      size: parseInt(column.width, 10) || undefined,
      minSize: 0,
      meta: { source: column } satisfies DataTableColumnMeta<TRow>,
    }))
  ), [columns]);

  const frozenColumnKey = useMemo(() => {
    if (!freezeColumn) return null;
    return columns.find((column) => column.sticky)?.key ?? columns[0]?.key ?? null;
  }, [columns, freezeColumn]);

  const freeze = useFreezeColumnResize(freezeColumn ?? { defaultWidth: 280, min: 200, max: 520 });

  const table = useReactTable({
    data: rows,
    columns: dataColumns,
    getRowId,
    getCoreRowModel: getCoreRowModel(),
    getSortedRowModel: getSortedRowModel(),
    getGroupedRowModel: getGroupedRowModel(),
    enableRowSelection: (row) => isSelectable(row.original),
    enableMultiRowSelection: true,
    enableColumnPinning: true,
    enableSortingRemoval: false,
    autoResetPageIndex: false,
    manualSorting,
    groupedColumnMode: false,
    state: {
      sorting,
      rowSelection,
      grouping,
      columnPinning: frozenColumnKey ? { left: [frozenColumnKey] } : { left: [] },
      columnSizing: freezeColumn && frozenColumnKey ? { [frozenColumnKey]: freeze.width } : {},
    },
    onSortingChange: (updater) => {
      const next = tableSortingToSortState(applyUpdater(updater, sorting));
      if (!next) return;
      if (onSortChange) onSortChange(next);
      if (controlledSort === undefined) setInternalSort(next);
    },
    onRowSelectionChange: (updater) => {
      const next = applyUpdater(updater, rowSelection);
      setSelected(Object.entries(next).filter(([, value]) => value).map(([id]) => id));
    },
    onGroupingChange: (updater) => {
      const next = applyUpdater(updater, grouping);
      const normalized = next[0] ? [next[0]] : [];
      if (groupingControlled) {
        onGroupByChange?.(normalized[0] ?? null);
      } else {
        setInternalGrouping(normalized);
      }
    },
  });

  const showColumnHeader = columns.some((column) => columnLabel(column));
  const leafRows = useMemo(
    () => table.getRowModel().flatRows.filter((row) => !row.getIsGrouped()),
    [table, rows, sorting, grouping, rowSelection],
  );
  const modelRows = table.getRowModel().rows;
  const virtualizedRows = !grouping[0] && modelRows.length > DATA_TABLE_VIRTUAL_THRESHOLD;
  const visibleRows = useMemo(() => {
    if (!virtualizedRows) {
      return { rows: modelRows, top: 0, bottom: 0 };
    }
    const start = Math.max(0, Math.floor(scrollTop / rowHeight) - DATA_TABLE_VIRTUAL_OVERSCAN);
    const end = Math.min(
      modelRows.length,
      Math.ceil((scrollTop + scrollSize.height) / rowHeight) + DATA_TABLE_VIRTUAL_OVERSCAN,
    );
    return {
      rows: modelRows.slice(start, end),
      top: start * rowHeight,
      bottom: Math.max(0, (modelRows.length - end) * rowHeight),
    };
  }, [modelRows, rowHeight, scrollSize.height, scrollTop, virtualizedRows]);
  const selectableRows = useMemo(() => leafRows.filter((row) => row.getCanSelect()), [leafRows]);
  const selectableIds = useMemo(() => selectableRows.map((row) => row.id), [selectableRows]);
  const selectedRows = useMemo(
    () => table.getSelectedRowModel().flatRows
      .filter((row) => !row.getIsGrouped())
      .map((row) => row.original),
    [table, rowSelection, rows, sorting, grouping],
  );
  const selectedVisibleCount = selectableRows.filter((row) => row.getIsSelected()).length;
  const allSelected = selectableRows.length > 0 && selectedVisibleCount === selectableRows.length;
  const mixedSelected = selectedVisibleCount > 0 && !allSelected;
  const selectionActive = selectedRows.length > 0;

  const toggleAllVisible = useCallback(() => {
    const visible = new Set(selectableIds);
    setSelected((current) => {
      if (allSelected) return current.filter((id) => !visible.has(id));
      return [...new Set([...current, ...selectableIds])];
    });
  }, [allSelected, selectableIds, setSelected]);
  const clearSelection = useCallback(() => setSelected([]), [setSelected]);

  const toggleGroup = useCallback((column: ColumnDef<TRow>) => {
    if (!canGroupColumn(column)) return;
    const next = grouping[0] === column.key ? [] : [column.key];
    if (groupingControlled) {
      onGroupByChange?.(next[0] ?? null);
    } else {
      setInternalGrouping(next);
    }
  }, [grouping, groupingControlled, onGroupByChange]);

  const applyMarqueeSelection = useCallback(
    (contentRect: MarqueeRect, base: Set<string>, additive: boolean) => {
      const scroll = scrollRef.current;
      if (!scroll) return;
      const bounds = scroll.getBoundingClientRect();
      const hits = new Set<string>();
      for (const node of (shellRef.current ?? scroll).querySelectorAll<HTMLElement>("[data-row-id]")) {
        if (node.dataset.rowSelectable === "false") continue;
        if (rectsIntersect(contentRect, elementContentRect(node, scroll, bounds))) hits.add(node.dataset.rowId ?? "");
      }
      const next = additive ? new Set(base) : new Set();
      for (const id of hits) next.add(id);
      setSelected(selectableRows.filter((row) => next.has(row.id)).map((row) => row.id));
    },
    [selectableRows, setSelected],
  );
  const updateMarquee = useCallback(
    (point: { x: number; y: number }) => {
      const drag = marqueeDragRef.current;
      const scroll = scrollRef.current;
      if (!drag || !scroll) return;
      const bounds = scroll.getBoundingClientRect();
      const visualBounds = freezeColumn
        ? (shellRef.current?.getBoundingClientRect() ?? bounds)
        : bounds;
      const currentContent = contentPointFromClient(point, scroll, bounds);
      const contentRect = rectFromPoints(drag.startContent, currentContent);
      const startClient = clientPointFromContent(drag.startContent, scroll, bounds);
      setMarquee(clientRectFromPoints(startClient, point, visualBounds));
      applyMarqueeSelection(contentRect, drag.baseSelected, drag.additive);
    },
    [applyMarqueeSelection, freezeColumn],
  );
  const runAutoScroll = useCallback(() => {
    const drag = marqueeDragRef.current;
    const scroll = scrollRef.current;
    if (!drag || !drag.active || !drag.current || !scroll) return;
    const bounds = scroll.getBoundingClientRect();
    const delta = marqueeAutoScrollDelta(drag.current, bounds);
    if (delta.x === 0 && delta.y === 0) {
      drag.autoScrollFrame = null;
      return;
    }
    const prevLeft = scroll.scrollLeft;
    const prevTop = scroll.scrollTop;
    scroll.scrollLeft += delta.x;
    scroll.scrollTop += delta.y;
    if (scroll.scrollLeft !== prevLeft || scroll.scrollTop !== prevTop) {
      updateMarquee(drag.current);
      drag.autoScrollFrame = window.requestAnimationFrame(runAutoScroll);
      return;
    }
    drag.autoScrollFrame = null;
  }, [updateMarquee]);
  const scheduleAutoScroll = useCallback(() => {
    const drag = marqueeDragRef.current;
    if (!drag?.active || drag.autoScrollFrame) return;
    drag.autoScrollFrame = window.requestAnimationFrame(runAutoScroll);
  }, [runAutoScroll]);
  const moveMarquee = useCallback(
    (event: globalThis.MouseEvent) => {
      const drag = marqueeDragRef.current;
      if (!drag) return;
      const deltaX = event.clientX - drag.start.x;
      const deltaY = event.clientY - drag.start.y;
      if (!drag.active && Math.hypot(deltaX, deltaY) < MARQUEE_DRAG_THRESHOLD) return;
      drag.active = true;
      drag.current = { x: event.clientX, y: event.clientY };
      suppressClickRef.current = true;
      event.preventDefault();
      updateMarquee(drag.current);
      scheduleAutoScroll();
    },
    [scheduleAutoScroll, updateMarquee],
  );
  const finishMarquee = useCallback(
    (event: globalThis.MouseEvent) => {
      const drag = marqueeDragRef.current;
      if (!drag) return;
      if (drag.active) {
        event.preventDefault();
        drag.current = { x: event.clientX, y: event.clientY };
        updateMarquee(drag.current);
      }
      drag.cleanup?.();
      marqueeDragRef.current = null;
      setMarquee(null);
    },
    [updateMarquee],
  );
  const cancelMarquee = useCallback(() => {
    marqueeDragRef.current?.cleanup?.();
    marqueeDragRef.current = null;
    setMarquee(null);
  }, []);
  const beginMarquee = useCallback(
    (event: ReactMouseEvent) => {
      if (!enableMarquee || event.button !== 0) return;
      const target = event.target;
      if (!(target instanceof Element)) return;
      if (target.closest(DATA_TABLE_MARQUEE_BLOCK_SELECTOR)) return;
      const scroll = scrollRef.current;
      if (!scroll) return;
      event.preventDefault();
      const bounds = scroll.getBoundingClientRect();
      const startPoint = { x: event.clientX, y: event.clientY };
      const drag: MarqueeDrag = {
        active: false,
        start: startPoint,
        startContent: contentPointFromClient(startPoint, scroll, bounds),
        current: startPoint,
        baseSelected: new Set(selected),
        additive: event.metaKey || event.ctrlKey || event.shiftKey,
        autoScrollFrame: null,
        cleanup: null,
      };
      drag.cleanup = () => {
        window.removeEventListener("mousemove", moveMarquee, true);
        window.removeEventListener("mouseup", finishMarquee, true);
        window.removeEventListener("blur", cancelMarquee, true);
        if (drag.autoScrollFrame) {
          window.cancelAnimationFrame(drag.autoScrollFrame);
          drag.autoScrollFrame = null;
        }
      };
      marqueeDragRef.current = drag;
      window.addEventListener("mousemove", moveMarquee, true);
      window.addEventListener("mouseup", finishMarquee, true);
      window.addEventListener("blur", cancelMarquee, true);
    },
    [cancelMarquee, enableMarquee, finishMarquee, moveMarquee, selected],
  );
  const suppressClickAfterMarquee = useCallback((event: ReactMouseEvent) => {
    if (!suppressClickRef.current) return;
    suppressClickRef.current = false;
    event.preventDefault();
    event.stopPropagation();
  }, []);
  useEffect(() => () => marqueeDragRef.current?.cleanup?.(), []);

  useEffect(() => () => {
    if (scrollUpdateFrameRef.current !== null) window.cancelAnimationFrame(scrollUpdateFrameRef.current);
  }, []);

  const nonFrozenColumns = useMemo(
    () => (freezeColumn && frozenColumnKey ? columns.filter((column) => column.key !== frozenColumnKey) : columns),
    [columns, freezeColumn, frozenColumnKey],
  );

  const gridColumns = useMemo(() => (
    [
      "var(--data-table-selection-col)",
      ...columns.map((column) => (
        freezeColumn && column.key === frozenColumnKey
          ? "var(--data-freeze-column-width)"
          : column.width
      )),
    ].join(" ")
  ), [columns, freezeColumn, frozenColumnKey]);
  const scrollGridColumns = useMemo(
    () => nonFrozenColumns.map((column) => column.width).join(" ") || "minmax(0, 1fr)",
    [nonFrozenColumns],
  );

  const tableStyle: CSSProperties & {
    "--data-grid-columns"?: string;
    "--data-scroll-grid-columns"?: string;
    "--data-freeze-column-width"?: string;
    "--data-table-table-row-height"?: string;
  } = {
    "--data-grid-columns": gridColumns,
    "--data-scroll-grid-columns": scrollGridColumns,
    ...(freezeColumn ? { "--data-freeze-column-width": `${freeze.width}px` } : {}),
    "--data-table-table-row-height": `${rowHeight}px`,
  };

  const renderHeaderCell = (header: ReturnType<typeof table.getHeaderGroups>[number]["headers"][number]) => {
    const column = metaFor<TRow>(header.column).source;
    const label = columnLabel(column);
    const groupable = canGroupColumn(column);
    const sortable = header.column.getCanSort();
    const grouped = grouping[0] === column.key;
    const sortDirection = header.column.getIsSorted();

    return (
      <div
        className="dataHeaderCell"
        data-column={column.key}
        {...(header.column.getIsPinned() ? { "data-frozen": "" } : {})}
        key={header.id}
      >
        <div className={`dataHeaderCellInner ${grouped ? "isGrouped" : ""}`}>
          {sortable ? (
            <button
              type="button"
              className={`dataHeaderLabelButton dataHeaderSortLabelButton ${sortDirection ? "activeSort" : ""} ${sortDirection === "asc" ? "ascending" : ""}`}
              aria-label={`Sort by ${label}`}
              onClick={header.column.getToggleSortingHandler()}
            >
              <span>{flexRender(header.column.columnDef.header, header.getContext())}</span>
              <span className="dataHeaderSortIcon" aria-hidden="true">
                <ChevronDown size={13} />
              </span>
            </button>
          ) : (
            <span className="dataHeaderLabel">{flexRender(header.column.columnDef.header, header.getContext())}</span>
          )}
          {groupable ? (
            <button
              type="button"
              className={`dataHeaderGroupButton ${grouped ? "activeGroup" : ""}`}
              aria-label={grouped ? `Ungroup by ${label}` : `Group by ${label}`}
              aria-pressed={grouped}
              onClick={() => toggleGroup(column)}
            >
              <ListTree size={13} strokeWidth={1.75} aria-hidden="true" />
            </button>
          ) : null}
        </div>
      </div>
    );
  };

  const headerGroup = table.getHeaderGroups()[0];
  const frozenHeaders = headerGroup?.headers.filter((header) => header.column.getIsPinned()) ?? [];
  const scrollHeaders = freezeColumn
    ? (headerGroup?.headers.filter((header) => !header.column.getIsPinned()) ?? [])
    : (headerGroup?.headers ?? []);

  const rowClassName = (row: Row<TRow>, extraClassName: string | undefined) => [
    "dataRow",
    "rowFrame",
    row.getIsSelected() && "rowSelected",
    selectionActive && "selectionActive",
    hoveredRowId === row.id && "rowHover",
    menuRowId === row.id && "menuActive",
    extraClassName,
  ].filter(Boolean).join(" ");

  const renderSelectionCell = (row: Row<TRow>) => {
    const id = row.id;
    const rowSelectable = row.getCanSelect();
    return selectable && rowSelectable ? (
      <SelectionCheckbox
        checked={row.getIsSelected()}
        label={`Select ${getRowLabel(row.original) ?? id}`}
        className="rowSelection"
        onChange={() => row.toggleSelected()}
      />
    ) : (
      <span className="rowSelectionPlaceholder" aria-hidden="true" />
    );
  };

  const renderDataCell = (cell: Row<TRow>["getVisibleCells"] extends () => (infer TCell)[] ? TCell : never) => {
    const column = metaFor<TRow>(cell.column).source;
    return (
      <div
        className="dataCell"
        data-column={column.key}
        {...(cell.column.getIsPinned() ? { "data-frozen": "" } : {})}
        key={cell.id}
      >
        {flexRender(cell.column.columnDef.cell, cell.getContext())}
      </div>
    );
  };

  const withRowContextMenu = (row: Row<TRow>, segment: ReactNode, key: string) => {
    const contextMenuContent = rowContextMenu?.(row.original, { selectedRows, selected: row.getIsSelected() }) ?? null;
    if (contextMenuContent == null) return segment;
    return (
      <ContextMenu.Root
        key={key}
        onOpenChange={(open) => {
          setMenuRowId((current) => (open ? row.id : current === row.id ? null : current));
          if (!open) suppressNextClick();
        }}
      >
        <ContextMenu.Trigger asChild>{segment}</ContextMenu.Trigger>
        <ContextMenu.Portal>
          <ContextMenu.Content className="skillMenuContent" alignOffset={6} data-no-drag>
            {contextMenuContent}
          </ContextMenu.Content>
        </ContextMenu.Portal>
      </ContextMenu.Root>
    );
  };

  const renderDataRow = (row: Row<TRow>) => {
    const id = row.id;
    const rowSelectable = row.getCanSelect();
    const extraClassName = rowProps?.(row.original)?.className;
    const body = (
      <div
        className={rowClassName(row, extraClassName)}
        data-row-id={id}
        data-row-selectable={rowSelectable ? "true" : "false"}
        onMouseEnter={freezeColumn ? () => setHoveredRowId(id) : undefined}
        onMouseLeave={freezeColumn ? () => setHoveredRowId((current) => (current === id ? null : current)) : undefined}
        onClick={onRowClick ? (event) => {
          const target = event.target;
          if (target instanceof Element && target.closest(DATA_TABLE_INTERACTIVE_SELECTOR)) return;
          onRowClick(row.original);
        } : undefined}
      >
        {renderSelectionCell(row)}
        {row.getVisibleCells().map(renderDataCell)}
      </div>
    );

    return (
      <div className="dataTableRowSlot" key={id}>
        {withRowContextMenu(row, body, `full-${id}`)}
      </div>
    );
  };

  const renderSplitDataRow = (row: Row<TRow>, pane: "frozen" | "scroll") => {
    const id = row.id;
    const rowSelectable = row.getCanSelect();
    const extraClassName = rowProps?.(row.original)?.className;
    const cells = row.getVisibleCells().filter((cell) => (
      pane === "frozen" ? cell.column.getIsPinned() : !cell.column.getIsPinned()
    ));
    const body = (
      <div
        className={`${rowClassName(row, extraClassName)} dataRow--${pane}Pane`}
        data-row-id={id}
        data-row-selectable={rowSelectable ? "true" : "false"}
        onMouseEnter={freezeColumn ? () => setHoveredRowId(id) : undefined}
        onMouseLeave={freezeColumn ? () => setHoveredRowId((current) => (current === id ? null : current)) : undefined}
        onClick={onRowClick ? (event) => {
          const target = event.target;
          if (target instanceof Element && target.closest(DATA_TABLE_INTERACTIVE_SELECTOR)) return;
          onRowClick(row.original);
        } : undefined}
      >
        {pane === "frozen" ? renderSelectionCell(row) : null}
        {cells.map(renderDataCell)}
      </div>
    );

    return (
      <div className="dataTableRowSlot" key={`${pane}-${id}`}>
        {withRowContextMenu(row, body, `${pane}-${id}`)}
      </div>
    );
  };

  const renderVisibleRows = (renderRow: (row: Row<TRow>) => ReactNode) => (
    <>
      {visibleRows.top > 0 ? <div className="dataTableVirtualSpacer" style={{ height: visibleRows.top }} aria-hidden="true" /> : null}
      {visibleRows.rows.map(renderRow)}
      {visibleRows.bottom > 0 ? <div className="dataTableVirtualSpacer" style={{ height: visibleRows.bottom }} aria-hidden="true" /> : null}
    </>
  );

  const groupRows = useMemo(() => {
    const topRows = table.getRowModel().rows;
    if (!grouping[0]) return [];
    const groups = topRows.filter((row) => row.getIsGrouped());
    if (!effectiveGroupOrder) return groups;
    const order = new Map(effectiveGroupOrder.map((key, index) => [normalizeGroupKey(key), index]));
    return [...groups].sort((left, right) => (
      (order.get(left.groupingValue as string) ?? Number.MAX_SAFE_INTEGER)
      - (order.get(right.groupingValue as string) ?? Number.MAX_SAFE_INTEGER)
    ));
  }, [effectiveGroupOrder, grouping, table, rows, sorting]);

  const expandedGroupSet = useMemo(() => new Set(expandedGroups), [expandedGroups]);
  const groupedLeafCount = useMemo(
    () => groupRows.reduce((total, group) => total + group.subRows.length, 0),
    [groupRows],
  );
  const virtualizedGroups = Boolean(grouping[0]) && groupedLeafCount > DATA_TABLE_VIRTUAL_THRESHOLD;
  const groupLayouts = useMemo(() => {
    let top = showColumnHeader && !freezeColumn ? DATA_TABLE_GROUP_HEADER_HEIGHT : 0;
    return groupRows.map((group, index) => {
      const key = `${group.groupingValue}`;
      const expanded = expandedGroupSet.has(key);
      const bodyHeight = expanded ? group.subRows.length * rowHeight : 0;
      const layout = { bodyTop: top + DATA_TABLE_GROUP_HEADER_HEIGHT, bodyHeight, expanded };
      top += DATA_TABLE_GROUP_HEADER_HEIGHT + bodyHeight;
      if (index < groupRows.length - 1) top += DATA_TABLE_GROUP_GAP;
      return layout;
    });
  }, [expandedGroupSet, freezeColumn, groupRows, rowHeight, showColumnHeader]);

  useLayoutEffect(() => {
    if (!scrollToRowId) return;
    const scroll = scrollRef.current;
    if (!scroll) return;
    const viewportHeight = scroll.clientHeight || 720;
    const row = [...scroll.querySelectorAll<HTMLElement>("[data-row-id]")]
      .find((candidate) => candidate.dataset.rowId === scrollToRowId);
    if (row) {
      const viewport = scroll.getBoundingClientRect();
      const bounds = row.getBoundingClientRect();
      const desiredTop = scroll.scrollTop
        + bounds.top
        - viewport.top
        - (viewportHeight - bounds.height) / 2;
      const maxScrollTop = Math.max(0, scroll.scrollHeight - viewportHeight);
      scroll.scrollTo({
        top: Math.min(maxScrollTop, Math.max(0, desiredTop)),
        behavior: "smooth",
      });
      onScrollToRowComplete?.(scrollToRowId);
      return;
    }

    if (!grouping[0]) {
      const targetIndex = modelRows.findIndex((candidate) => candidate.id === scrollToRowId);
      if (targetIndex < 0) return;
      const headerHeight = showColumnHeader && !freezeColumn
        ? scroll.querySelector<HTMLElement>(".dataTableHeader")?.offsetHeight ?? 0
        : 0;
      const targetTop = headerHeight
        + targetIndex * rowHeight
        - (viewportHeight - rowHeight) / 2;
      const maxScrollTop = Math.max(0, scroll.scrollHeight - viewportHeight);
      const nextScrollTop = Math.min(maxScrollTop, Math.max(0, targetTop));
      if (Math.abs(scroll.scrollTop - nextScrollTop) > 1) scroll.scrollTop = nextScrollTop;
      setScrollTop((current) => current === nextScrollTop ? current : nextScrollTop);
      return;
    }

    const targetGroupIndex = groupRows.findIndex((group) => group.subRows.some((candidate) => candidate.id === scrollToRowId));
    if (targetGroupIndex < 0) return;
    const group = groupRows[targetGroupIndex];
    const groupKey = `${group.groupingValue}`;
    if (!expandedGroupSet.has(groupKey)) {
      setExpandedGroups((current) => current.includes(groupKey) ? current : [...current, groupKey]);
      return;
    }
    const targetIndex = group.subRows.findIndex((candidate) => candidate.id === scrollToRowId);
    if (targetIndex < 0) return;
    const layout = groupLayouts[targetGroupIndex];
    if (!layout) return;
    const targetTop = layout.bodyTop
      + targetIndex * rowHeight
      - (viewportHeight - rowHeight) / 2;
    const maxScrollTop = Math.max(0, scroll.scrollHeight - viewportHeight);
    const nextScrollTop = Math.min(maxScrollTop, Math.max(0, targetTop));
    if (Math.abs(scroll.scrollTop - nextScrollTop) > 1) scroll.scrollTop = nextScrollTop;
    setScrollTop((current) => current === nextScrollTop ? current : nextScrollTop);
  }, [expandedGroupSet, freezeColumn, groupLayouts, groupRows, grouping, modelRows, onScrollToRowComplete, rowHeight, scrollRef, scrollToRowId, setExpandedGroups, showColumnHeader]);

  const renderGroupedRows = (
    group: Row<TRow>,
    layout: DataTableGroupLayout,
    renderRow: (row: Row<TRow>) => ReactNode,
  ) => {
    if (!layout.expanded) return null;
    if (!virtualizedGroups) return group.subRows.map(renderRow);

    const count = group.subRows.length;
    const bodyBottom = layout.bodyTop + layout.bodyHeight;
    const viewportBottom = scrollTop + scrollSize.height;
    let start = 0;
    let end = 0;
    if (viewportBottom > layout.bodyTop && scrollTop < bodyBottom) {
      start = Math.max(0, Math.floor((scrollTop - layout.bodyTop) / rowHeight) - DATA_TABLE_VIRTUAL_OVERSCAN);
      end = Math.min(
        count,
        Math.ceil((viewportBottom - layout.bodyTop) / rowHeight) + DATA_TABLE_VIRTUAL_OVERSCAN,
      );
    } else if (scrollTop >= bodyBottom) {
      start = count;
      end = count;
    }

    return (
      <>
        {start > 0 ? <div className="dataTableVirtualSpacer" style={{ height: start * rowHeight }} aria-hidden="true" /> : null}
        {group.subRows.slice(start, end).map(renderRow)}
        {end < count ? <div className="dataTableVirtualSpacer" style={{ height: (count - end) * rowHeight }} aria-hidden="true" /> : null}
      </>
    );
  };

  useEffect(() => {
    const groupingKey = grouping[0] ?? "";
    if (!grouping[0]) {
      setExpandedGroups((current) => current.length === 0 ? current : []);
      previousGroupStateRef.current = null;
      return;
    }
    const nextGroupKeys = groupRows.map((group) => `${group.groupingValue}`);
    const previous = previousGroupStateRef.current;
    previousGroupStateRef.current = { groupingKey, groupKeys: nextGroupKeys };
    if (!previous || previous.groupingKey !== groupingKey) {
      setExpandedGroups((current) => sameStringArray(current, nextGroupKeys) ? current : nextGroupKeys);
      return;
    }
    const previousGroupKeys = new Set(previous.groupKeys);
    const nextGroupKeySet = new Set(nextGroupKeys);
    setExpandedGroups((current) => {
      const next = [
        ...current.filter((key) => nextGroupKeySet.has(key)),
        ...nextGroupKeys.filter((key) => !previousGroupKeys.has(key)),
      ];
      return sameStringArray(current, next) ? current : next;
    });
  }, [groupRows, grouping]);

  const headerRow = showColumnHeader ? (
    <div className="dataTableHeader" role="row">
      <span className="dataHeaderSelection" aria-hidden="true" />
      {table.getHeaderGroups()[0]?.headers.map(renderHeaderCell)}
    </div>
  ) : null;
  const frozenHeaderRow = showColumnHeader ? (
    <div className="dataTableHeader dataTableHeader--frozen" role="row">
      <span className="dataHeaderSelection" aria-hidden="true" />
      {frozenHeaders.map(renderHeaderCell)}
    </div>
  ) : null;
  const scrollHeaderRow = showColumnHeader ? (
    <div className="dataTableHeader dataTableHeader--scroll" role="row">
      {scrollHeaders.map(renderHeaderCell)}
    </div>
  ) : null;

  const isEmpty = rows.length === 0;
  const renderEmptyState = (belowHeader = false) => isEmpty ? (
    <div className={`dataTableEmpty${belowHeader ? " dataTableEmpty--belowHeader" : ""}`}>
      {loading ? <LoadingState label={loadingLabel} /> : emptyState}
    </div>
  ) : null;

  const renderFrozenGroups = () => (
    <Accordion.Root
      key={grouping[0]}
      className="dataTableGroups"
      type="multiple"
      value={expandedGroups}
      onValueChange={setExpandedGroups}
    >
      {groupRows.map((group, index) => {
        const key = `${group.groupingValue}`;
        return (
          <Accordion.Item className="dataGroup" value={key} key={group.id}>
            <Accordion.Header asChild>
              <div className="sectionHeading">
                <Accordion.Trigger className="sectionHeader">
                  <span className="sectionHeaderLabel">{effectiveGroupLabel(key)}</span>
                  <span className="sectionHeaderCount">{group.subRows.length}</span>
                  <ChevronDown className="accordionChevron" size={14} />
                </Accordion.Trigger>
              </div>
            </Accordion.Header>
            <Accordion.Content>{renderGroupedRows(group, groupLayouts[index], (row) => renderSplitDataRow(row, "frozen"))}</Accordion.Content>
          </Accordion.Item>
        );
      })}
    </Accordion.Root>
  );

  const renderScrollGroups = () => (
    <div className="dataTableGroups">
      {groupRows.map((group, index) => {
        return (
          <div className="dataGroup" key={group.id}>
            <div className="sectionHeading dataSplitGroupSpacer" aria-hidden="true" />
            {renderGroupedRows(group, groupLayouts[index], (row) => renderSplitDataRow(row, "scroll"))}
          </div>
        );
      })}
    </div>
  );

  const handleBodyScroll = useCallback(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    if (freezeColumn) setScrollLeft(scroll.scrollLeft);
    if (!virtualizedRows && !virtualizedGroups) return;
    if (scrollUpdateFrameRef.current !== null) return;
    scrollUpdateFrameRef.current = window.requestAnimationFrame(() => {
      scrollUpdateFrameRef.current = null;
      setScrollTop(scrollRef.current?.scrollTop ?? 0);
    });
  }, [freezeColumn, virtualizedGroups, virtualizedRows]);

  const renderFrozenTable = () => (
    <>
      {showColumnHeader ? (
        <div className="dataTableSplitHeader">
          <div className="dataTableFrozenHeaderPane">
            <div className="dataTableFrozenSurface">{frozenHeaderRow}</div>
          </div>
          <div className="dataTableScrollHeaderPane">
            <div className="dataTableScrollHeaderTrack" style={{ transform: `translateX(${-scrollLeft}px)` }}>
              {scrollHeaderRow}
            </div>
          </div>
        </div>
      ) : null}
      <div
        className="tableScroll dataTableScroll dataTableBodyScroll dataTableSplitScroll pageScrollArea pageScrollInsetRight"
        ref={scrollRef}
        style={{ "--data-table-sticky-header-offset": "0px" } as CSSProperties}
        onScroll={handleBodyScroll}
        onMouseDownCapture={enableMarquee ? beginMarquee : undefined}
        onClickCapture={enableMarquee ? suppressClickAfterMarquee : undefined}
      >
        <div className="dataTableSplitSurface">
          <div className="dataTableFrozenPane">
            <div className="dataTableBody">
              {grouping[0]
                ? renderFrozenGroups()
                : renderVisibleRows((row) => renderSplitDataRow(row, "frozen"))}
            </div>
          </div>
          <div className="dataTableScrollPane">
            <div className="dataTableBody">
              {grouping[0]
                ? renderScrollGroups()
                : renderVisibleRows((row) => renderSplitDataRow(row, "scroll"))}
            </div>
          </div>
        </div>
        {renderEmptyState()}
      </div>
    </>
  );

  return (
    <div className="dataTableFrame">
      <div
        ref={shellRef}
        className={`dataTableShell${freezeColumn ? " dataTableShell--frozen" : ""}`}
        style={tableStyle}
      >
        {freezeColumn ? <FreezeColumnResizeHandle label="Resize first column" resize={freeze} /> : null}
        {freezeColumn && marquee ? (
          <div
            className="dataTableMarquee"
            style={{ left: `${marquee.left}px`, top: `${marquee.top}px`, width: `${marquee.width}px`, height: `${marquee.height}px` }}
          />
        ) : null}
        {freezeColumn ? renderFrozenTable() : (
          <>
            <div
              className="tableScroll dataTableScroll dataTableBodyScroll pageScrollArea pageScrollInsetRight"
              ref={scrollRef}
              style={{ "--data-table-sticky-header-offset": "0px" } as CSSProperties}
              onScroll={handleBodyScroll}
              onMouseDownCapture={enableMarquee ? beginMarquee : undefined}
              onClickCapture={enableMarquee ? suppressClickAfterMarquee : undefined}
            >
              {marquee && (
                <div
                  className="dataTableMarquee"
                  style={{ left: `${marquee.left}px`, top: `${marquee.top}px`, width: `${marquee.width}px`, height: `${marquee.height}px` }}
                />
              )}

              <div className="dataTableScrollSurface">
                {headerRow}
                <div className="dataTableBody">
                  {grouping[0] ? (
                    <Accordion.Root
                      key={grouping[0]}
                      className="dataTableGroups"
                      type="multiple"
                      value={expandedGroups}
                      onValueChange={setExpandedGroups}
                    >
                      {groupRows.map((group, index) => {
                        const key = `${group.groupingValue}`;
                        return (
                          <Accordion.Item className="dataGroup" value={key} key={group.id}>
                            <Accordion.Header asChild>
                              <div className="sectionHeading">
                                <Accordion.Trigger className="sectionHeader">
                                  <span className="sectionHeaderLabel">{effectiveGroupLabel(key)}</span>
                                  <span className="sectionHeaderCount">{group.subRows.length}</span>
                                  <ChevronDown className="accordionChevron" size={14} />
                                </Accordion.Trigger>
                              </div>
                            </Accordion.Header>
                            <Accordion.Content>{renderGroupedRows(group, groupLayouts[index], renderDataRow)}</Accordion.Content>
                          </Accordion.Item>
                        );
                      })}
                    </Accordion.Root>
                  ) : (
                    renderVisibleRows(renderDataRow)
                  )}

                </div>
              </div>
              {renderEmptyState(showColumnHeader)}
            </div>
          </>
        )}
      </div>

      {bottomBar && selectedRows.length > 0 ? (
        <SelectionActionBar
          selectedCount={selectedRows.length}
          allSelected={allSelected}
          mixed={mixedSelected}
          onToggleAll={toggleAllVisible}
          onClear={clearSelection}
          checkboxLabel={bottomBarCheckboxLabel}
          label={selectionLabel}
        >
          {bottomBar(selectedRows, { clear: clearSelection })}
        </SelectionActionBar>
      ) : null}
    </div>
  );
}
