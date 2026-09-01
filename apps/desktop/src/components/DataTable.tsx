import { Tooltip } from "./shared/Tooltip.tsx";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  useLayoutEffect,
  memo,
  type CSSProperties,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
  type WheelEvent as ReactWheelEvent,
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
import { RowMenuOpenChangeProvider } from "./shared/row-menu-context.tsx";
import { useVirtualViewport } from "./shared/useVirtualViewport.ts";
import { fixedVirtualRange, virtualRangeFor } from "../lib/virtualization.ts";
import {
  MARQUEE_DRAG_THRESHOLD,
  clientPointFromContent,
  clientRectFromPoints,
  compareTimestamps,
  contentPointFromClient,
  dayGroupKey,
  elementContentRect,
  formatDayGroupLabel,
  marqueeAutoScrollDelta,
  rectFromPoints,
  rectsIntersect,
} from "../lib/index.ts";
import { ColumnCellVariant, ColumnDataType, type ColumnDef, type DataTableProps, type SortState } from "./DataTable.types";
import { SortDirection } from "../lib/sort.ts";
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
const DATA_TABLE_GROUP_VIRTUAL_OVERSCAN = 4;
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
  if (column.groupByDay) return ColumnDataType.Date;
  if (column.groupable === true || typeof column.groupBy === "function" || column.groupOrder) return ColumnDataType.Enum;
  return ColumnDataType.Text;
}

function groupsByDay<TRow>(column: ColumnDef<TRow>) {
  return columnType(column) === ColumnDataType.Date || column.groupByDay;
}

function canGroupColumn<TRow>(column: ColumnDef<TRow>) {
  if (!columnLabel(column) || column.groupable === false) return false;
  const type = columnType(column);
  return type === ColumnDataType.Date || type === ColumnDataType.Enum;
}

function normalizeGroupKey(raw: unknown) {
  const text = `${raw ?? ""}`;
  return text === "" ? EMPTY_GROUP_KEY : text;
}

function defaultGroupLabel(key: string) {
  return key === EMPTY_GROUP_KEY ? "" : key;
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

function compareColumnValue<TRow>(column: ColumnDef<TRow>, left: unknown, right: unknown) {
  if (columnType(column) === ColumnDataType.Date) return compareTimestamps(left, right);
  if (typeof left === "number" || typeof right === "number") {
    return (Number(left) || 0) - (Number(right) || 0);
  }
  return `${left ?? ""}`.localeCompare(`${right ?? ""}`);
}

function renderTextCell<TRow>(column: ColumnDef<TRow>, row: TRow) {
  const display = columnDisplayValue(column, row);
  const className = column.cell === ColumnCellVariant.Number
    ? "dataCellNumber"
    : column.cell === ColumnCellVariant.Title
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

type DataTableCellContext<TRow> = {
  column: { columnDef: { meta?: unknown } };
  row: { original: TRow };
};

type DataTableCell<TRow> = ReturnType<Row<TRow>["getVisibleCells"]>[number];

enum DataTablePane {
  Full = "full",
  Frozen = "frozen",
  Scroll = "scroll",
}

type DataTableRowProps<TRow extends Record<string, unknown>> = {
  row: Row<TRow>;
  pane: DataTablePane;
  className: string;
  rowSelectable: boolean;
  selectable: boolean;
  getRowLabel?: (row: TRow) => string | undefined;
  selectedRows: TRow[];
  rowContextMenu?: DataTableProps<TRow>["rowContextMenu"];
  onRowClick?: (row: TRow) => void;
  onHoveredRowChange?: (rowId: string | null) => void;
  onRowMenuOpenChange: (rowId: string, open: boolean) => void;
  renderDataCell: (cell: DataTableCell<TRow>) => ReactNode;
};

function DataTableRowComponent<TRow extends Record<string, unknown>>({
  row,
  pane,
  className,
  rowSelectable,
  selectable,
  getRowLabel,
  selectedRows,
  rowContextMenu,
  onRowClick,
  onHoveredRowChange,
  onRowMenuOpenChange,
  renderDataCell,
}: DataTableRowProps<TRow>) {
  const id = row.id;
  const cells = row.getVisibleCells().filter((cell) => (
    pane === DataTablePane.Full || (pane === DataTablePane.Frozen ? cell.column.getIsPinned() : !cell.column.getIsPinned())
  ));
  const rowLabel = getRowLabel?.(row.original);
  const contextMenuContent = rowContextMenu?.(row.original, { selectedRows, selected: row.getIsSelected() }) ?? null;
  const body = (
    <div
      className={className}
      data-row-id={id}
      data-row-selectable={rowSelectable ? "true" : "false"}
      onMouseEnter={onHoveredRowChange ? () => onHoveredRowChange(id) : undefined}
      onMouseLeave={onHoveredRowChange ? () => onHoveredRowChange(null) : undefined}
      onClick={onRowClick ? (event) => {
        const target = event.target;
        if (target instanceof Element && target.closest(DATA_TABLE_INTERACTIVE_SELECTOR)) return;
        onRowClick(row.original);
      } : undefined}
    >
      <RowMenuOpenChangeProvider onOpenChange={(open) => onRowMenuOpenChange(id, open)}>
        {pane !== DataTablePane.Scroll ? (
          selectable && rowSelectable ? (
            <SelectionCheckbox
              checked={row.getIsSelected()}
              label={rowLabel ? `Select ${rowLabel}` : "Select row"}
              className="rowSelection"
              onChange={() => row.toggleSelected()}
            />
          ) : (
            <span className="rowSelectionPlaceholder" aria-hidden="true" />
          )
        ) : null}
        {cells.map(renderDataCell)}
      </RowMenuOpenChangeProvider>
    </div>
  );

  return (
    <div className="dataTableRowSlot">
      {contextMenuContent == null ? body : (
        <ContextMenu.Root
          onOpenChange={(open) => onRowMenuOpenChange(id, open)}
        >
          <ContextMenu.Trigger asChild>{body}</ContextMenu.Trigger>
          <ContextMenu.Portal>
            <ContextMenu.Content className="skillMenuContent" alignOffset={6} data-no-drag>
              {contextMenuContent}
            </ContextMenu.Content>
          </ContextMenu.Portal>
        </ContextMenu.Root>
      )}
    </div>
  );
}

const DataTableRow = memo(DataTableRowComponent, (previous, next) => (
  previous.row === next.row
  && previous.pane === next.pane
  && previous.className === next.className
  && previous.rowSelectable === next.rowSelectable
  && previous.selectable === next.selectable
  && previous.getRowLabel === next.getRowLabel
  && previous.selectedRows === next.selectedRows
  && previous.rowContextMenu === next.rowContextMenu
  && previous.onRowClick === next.onRowClick
  && previous.onHoveredRowChange === next.onHoveredRowChange
  && previous.onRowMenuOpenChange === next.onRowMenuOpenChange
  && previous.renderDataCell === next.renderDataCell
)) as typeof DataTableRowComponent;

function renderDataTableCell<TRow extends Record<string, unknown>>({ column, row }: DataTableCellContext<TRow>): ReactNode {
  const source = metaFor<TRow>(column).source;
  return renderCell(source, row.original);
}

function sortStateToTable(sort: SortState | null): SortingState {
  return sort ? [{ id: sort.key, desc: sort.direction === SortDirection.Desc }] : [];
}

function tableSortingToSortState(sorting: SortingState): SortState | null {
  const item = sorting[0];
  return item ? { key: item.id, direction: item.desc ? SortDirection.Desc : SortDirection.Asc } : null;
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

function virtualSeekGap(viewportTop: number, viewportHeight: number, renderedTop: number, renderedBottom: number) {
  const viewportBottom = viewportTop + viewportHeight;
  if (viewportTop < renderedTop) {
    return {
      top: viewportTop,
      height: Math.min(viewportBottom, renderedTop) - viewportTop,
    };
  }
  if (viewportBottom > renderedBottom) {
    return {
      top: Math.max(viewportTop, renderedBottom),
      height: viewportBottom - Math.max(viewportTop, renderedBottom),
    };
  }
  return null;
}

function groupVirtualRange(
  layout: DataTableGroupLayout,
  count: number,
  scrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  overscan: number,
  datasetEpoch: string,
  stableKey: string,
) {
  const bodyBottom = layout.bodyTop + layout.bodyHeight;
  const bufferedTop = Math.max(0, scrollTop - overscan * rowHeight);
  const bufferedBottom = scrollTop + viewportHeight + overscan * rowHeight;
  if (bufferedBottom > layout.bodyTop && bufferedTop < bodyBottom) {
    return virtualRangeFor({
      datasetEpoch,
      stableKey,
      count,
      estimate: rowHeight,
      scrollOffset: Math.max(0, scrollTop - layout.bodyTop),
      viewportSize: viewportHeight,
      overscan,
    });
  }
  if (scrollTop >= bodyBottom) return { start: count, end: count };
  return { start: 0, end: 0 };
}

function groupVirtualSeekGap(
  layouts: readonly DataTableGroupLayout[],
  groups: ReadonlyArray<{ subRows: readonly unknown[] }>,
  renderedScrollTop: number,
  targetScrollTop: number,
  viewportHeight: number,
  rowHeight: number,
  overscan: number,
  datasetEpoch: string,
  stableKey: string,
) {
  const viewportBottom = targetScrollTop + viewportHeight;
  const covered: Array<{ top: number; bottom: number }> = [];
  const headerTops = layouts.map((layout) => layout.bodyTop - DATA_TABLE_GROUP_HEADER_HEIGHT);
  layouts.forEach((layout, index) => {
    const group = groups[index];
    if (!group) return;
    const headerTop = layout.bodyTop - DATA_TABLE_GROUP_HEADER_HEIGHT;
    const nextHeaderTop = headerTops[index + 1] ?? Number.POSITIVE_INFINITY;
    if (targetScrollTop >= headerTop && targetScrollTop < nextHeaderTop) {
      covered.push({ top: targetScrollTop, bottom: Math.min(viewportBottom, targetScrollTop + DATA_TABLE_GROUP_HEADER_HEIGHT) });
    } else if (headerTop < viewportBottom && layout.bodyTop > targetScrollTop) {
      covered.push({ top: Math.max(targetScrollTop, headerTop), bottom: Math.min(viewportBottom, layout.bodyTop) });
    }
    if (!layout.expanded) return;
    const rendered = groupVirtualRange(layout, group.subRows.length, renderedScrollTop, viewportHeight, rowHeight, overscan, datasetEpoch, `${stableKey}:${index}`);
    if (rendered.start >= rendered.end) return;
    const rowsTop = layout.bodyTop + rendered.start * rowHeight;
    const rowsBottom = layout.bodyTop + rendered.end * rowHeight;
    if (rowsTop < viewportBottom && rowsBottom > targetScrollTop) {
      covered.push({ top: Math.max(targetScrollTop, rowsTop), bottom: Math.min(viewportBottom, rowsBottom) });
    }
  });

  covered.sort((left, right) => left.top - right.top);
  const merged = covered.reduce<Array<{ top: number; bottom: number }>>((ranges, current) => {
    const previous = ranges[ranges.length - 1];
    if (previous && current.top <= previous.bottom) {
      previous.bottom = Math.max(previous.bottom, current.bottom);
    } else {
      ranges.push({ ...current });
    }
    return ranges;
  }, []);

  let cursor = targetScrollTop;
  let largestGap: { top: number; height: number } | null = null;
  for (const range of [...merged, { top: viewportBottom, bottom: viewportBottom }]) {
    if (range.top > cursor) {
      const gap = { top: cursor, height: range.top - cursor };
      if (!largestGap || gap.height > largestGap.height) largestGap = gap;
    }
    cursor = Math.max(cursor, range.bottom);
  }
  return largestGap;
}

export function DataTable<TRow extends Record<string, unknown>>({
  rows,
  columns,
  getRowId,
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
  enableVirtualization = true,
  scrollResetKey,
  scrollToRowId,
  onScrollToRowComplete,
  freezeColumn,
  onRowClick,
  rowProps,
  rowContextMenu,
  bottomBar,
  bottomBarActionsClassName,
  bottomBarCheckboxLabel = "Select rows",
  selectionLabel = "selected",
  loading = false,
  loadingLabel = "Loading",
  emptyState = "No items",
}: DataTableProps<TRow>) {
  const shellRef = useRef<HTMLDivElement>(null);
  const scrollAnchorRef = useRef<{ id: string; offsetTop: number } | null>(null);
  const previousScrollResetKeyRef = useRef(scrollResetKey);
  const scrollHeaderTrackRef = useRef<HTMLDivElement>(null);
  const virtualSeekRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const {
    scrollOffset: scrollTop,
    readViewportSize,
    syncScrollPosition,
    scheduleScrollSync,
  } = useVirtualViewport<HTMLDivElement>(
    { width: 0, height: 720 },
    {
      ref: scrollRef,
      refreshKey: rows,
      readSize: (element) => ({ width: element.clientWidth, height: element.clientHeight }),
      isValidSize: ({ height }) => height > 0,
      isEqual: (current, next) => current.height === next.height,
    },
  );

  useLayoutEffect(() => {
    const scroll = scrollRef.current;
    if (!scroll) return undefined;
    const resetToTop = () => {
      scroll.scrollTop = 0;
      virtualSeekRef.current?.classList.remove("dataTableVirtualSeek--visible");
    };
    const resetRequested = previousScrollResetKeyRef.current !== scrollResetKey;
    previousScrollResetKeyRef.current = scrollResetKey;
    const anchor = scrollAnchorRef.current;
    if (resetRequested) {
      scrollAnchorRef.current = null;
      resetToTop();
    } else if (anchor) {
      const viewport = scroll.getBoundingClientRect();
      const row = [...scroll.querySelectorAll<HTMLElement>("[data-row-id]")]
        .find((candidate) => candidate.dataset.rowId === anchor.id && candidate.getBoundingClientRect().bottom > viewport.top);
      if (row) {
        const currentOffsetTop = row.getBoundingClientRect().top - viewport.top;
        scroll.scrollTop += currentOffsetTop - anchor.offsetTop;
      } else {
        resetToTop();
      }
      scrollAnchorRef.current = null;
    } else if (scroll.scrollTop > 0) {
      resetToTop();
    }
    syncScrollPosition();
    return () => {
      const currentScroll = scrollRef.current;
      if (!currentScroll) return;
      const viewport = currentScroll.getBoundingClientRect();
      const row = [...currentScroll.querySelectorAll<HTMLElement>("[data-row-id]")]
        .find((candidate) => candidate.getBoundingClientRect().bottom > viewport.top);
      const rowId = row?.dataset.rowId;
      if (row && rowId) {
        scrollAnchorRef.current = {
          id: rowId,
          offsetTop: row.getBoundingClientRect().top - viewport.top,
        };
      }
    };
  }, [rows, scrollResetKey, syncScrollPosition]);
  const marqueeDragRef = useRef<MarqueeDrag | null>(null);
  const suppressClickRef = useRef(false);
  const [marquee, setMarquee] = useState<MarqueeRect | null>(null);
  const [hoveredRowId, setHoveredRowId] = useState<string | null>(null);
  const [openRowMenuId, setOpenRowMenuId] = useState<string | null>(null);
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
      return (key: string) => (key === EMPTY_GROUP_KEY ? "" : formatDayGroupLabel(key));
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
      cell: renderDataTableCell,
      enableSorting: isSortableColumn(column),
      enableGrouping: canGroupColumn(column),
      getGroupingValue: (row) => groupColumnValue(column, row),
      sortingFn: (rowA, rowB) => compareColumnValue(
        column,
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
  const virtualizedRows = enableVirtualization && !grouping[0] && modelRows.length > DATA_TABLE_VIRTUAL_THRESHOLD;
  const virtualDatasetEpoch = `${scrollResetKey ?? "default"}:${JSON.stringify(grouping)}:${JSON.stringify(sorting)}:${modelRows.length}`;
  const virtualStableKey = `${modelRows[0]?.id ?? ""}:${modelRows.at(-1)?.id ?? ""}`;
  const viewportHeight = readViewportSize();
  const visibleRows = useMemo(() => {
    if (!virtualizedRows) {
      return { rows: modelRows, top: 0, bottom: 0, start: 0, end: modelRows.length };
    }
    const { start, end } = virtualRangeFor({
      datasetEpoch: virtualDatasetEpoch,
      stableKey: virtualStableKey,
      count: modelRows.length,
      estimate: rowHeight,
      scrollOffset: scrollTop,
      viewportSize: viewportHeight,
      overscan: DATA_TABLE_VIRTUAL_OVERSCAN,
    });
    return {
      rows: modelRows.slice(start, end),
      top: start * rowHeight,
      bottom: Math.max(0, (modelRows.length - end) * rowHeight),
      start,
      end,
    };
  }, [modelRows, rowHeight, scrollTop, viewportHeight, virtualDatasetEpoch, virtualStableKey, virtualizedRows]);
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
        const rowId = node.dataset.rowId;
        if (rowId && rectsIntersect(contentRect, elementContentRect(node, scroll, bounds))) hits.add(rowId);
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
              className={`dataHeaderLabelButton dataHeaderSortLabelButton ${sortDirection ? "activeSort" : ""} ${sortDirection === SortDirection.Asc ? "ascending" : ""}`}
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
    openRowMenuId === row.id && "menuActive",
    extraClassName,
  ].filter(Boolean).join(" ");

  const notifyRowMenuOpenChange = useCallback((rowId: string, open: boolean) => {
    setOpenRowMenuId((current) => open ? rowId : current === rowId ? null : current);
  }, []);
  const handleRowHover = useCallback((rowId: string | null) => {
    setHoveredRowId(rowId);
  }, []);

  const renderDataCell = useCallback((cell: DataTableCell<TRow>) => {
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
  }, []);

  const renderDataRow = (row: Row<TRow>) => {
    const id = row.id;
    const rowSelectable = row.getCanSelect();
    return (
      <DataTableRow
        key={id}
        row={row}
        pane={DataTablePane.Full}
        className={rowClassName(row, rowProps?.(row.original)?.className)}
        rowSelectable={rowSelectable}
        selectable={Boolean(selectable)}
        getRowLabel={getRowLabel}
        selectedRows={selectedRows}
        rowContextMenu={rowContextMenu}
        onRowClick={onRowClick}
        onRowMenuOpenChange={notifyRowMenuOpenChange}
        renderDataCell={renderDataCell}
      />
    );
  };

  const renderSplitDataRow = (row: Row<TRow>, pane: Exclude<DataTablePane, DataTablePane.Full>) => {
    const id = row.id;
    const rowSelectable = row.getCanSelect();
    return (
      <DataTableRow
        key={`${pane}-${id}`}
        row={row}
        pane={pane}
        className={`${rowClassName(row, rowProps?.(row.original)?.className)} dataRow--${pane}Pane`}
        rowSelectable={rowSelectable}
        selectable={Boolean(selectable)}
        getRowLabel={getRowLabel}
        selectedRows={selectedRows}
        rowContextMenu={rowContextMenu}
        onRowClick={onRowClick}
        onHoveredRowChange={freezeColumn ? handleRowHover : undefined}
        onRowMenuOpenChange={notifyRowMenuOpenChange}
        renderDataCell={renderDataCell}
      />
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
  const virtualizedGroups = enableVirtualization && Boolean(grouping[0]) && groupedLeafCount > DATA_TABLE_VIRTUAL_THRESHOLD;
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
      syncScrollPosition();
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
    syncScrollPosition();
  }, [expandedGroupSet, freezeColumn, groupLayouts, groupRows, grouping, modelRows, onScrollToRowComplete, rowHeight, scrollRef, scrollToRowId, scrollTop, setExpandedGroups, showColumnHeader, syncScrollPosition]);

  const renderGroupedRows = (
    group: Row<TRow>,
    layout: DataTableGroupLayout,
    renderRow: (row: Row<TRow>) => ReactNode,
  ) => {
    if (!layout.expanded) return null;
    if (!virtualizedGroups) return group.subRows.map(renderRow);

    const count = group.subRows.length;
    const { start, end } = groupVirtualRange(
      layout,
      count,
      scrollTop,
      viewportHeight,
      rowHeight,
      DATA_TABLE_GROUP_VIRTUAL_OVERSCAN,
      virtualDatasetEpoch,
      `${virtualStableKey}:${group.id}`,
    );

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
            <Accordion.Content>{renderGroupedRows(group, groupLayouts[index], (row) => renderSplitDataRow(row, DataTablePane.Frozen))}</Accordion.Content>
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
            {renderGroupedRows(group, groupLayouts[index], (row) => renderSplitDataRow(row, DataTablePane.Scroll))}
          </div>
        );
      })}
    </div>
  );

  const syncScrollHeader = useCallback((left: number) => {
    const track = scrollHeaderTrackRef.current;
    if (track) track.style.transform = `translateX(${-left}px)`;
  }, []);

  const positionVirtualSeek = useCallback((top: number, height: number) => {
    const scroll = scrollRef.current;
    const seek = virtualSeekRef.current;
    if (!scroll || !seek) return;
    seek.style.top = `${top}px`;
    seek.style.left = `${scroll.scrollLeft}px`;
    seek.style.width = `${scroll.clientWidth}px`;
    seek.style.height = `${height}px`;
  }, [scrollRef]);

  const setVirtualSeekVisible = useCallback((visible: boolean, top?: number, height?: number) => {
    if (visible) {
      const scroll = scrollRef.current;
      positionVirtualSeek(top ?? scroll?.scrollTop ?? 0, height ?? scroll?.clientHeight ?? 0);
    }
    virtualSeekRef.current?.classList.toggle("dataTableVirtualSeek--visible", visible);
  }, [positionVirtualSeek, scrollRef]);

  const groupedWindowMissedAt = useCallback((targetTop: number, viewportHeight: number) => {
    if (!virtualizedGroups) return false;
    return groupRows.some((group, index) => {
      const layout = groupLayouts[index];
      if (!layout?.expanded) return false;
      const target = groupVirtualRange(layout, group.subRows.length, targetTop, viewportHeight, rowHeight, 0, virtualDatasetEpoch, `${virtualStableKey}:${index}`);
      if (target.start === target.end) return false;
      const rendered = groupVirtualRange(layout, group.subRows.length, scrollTop, viewportHeight, rowHeight, DATA_TABLE_GROUP_VIRTUAL_OVERSCAN, virtualDatasetEpoch, `${virtualStableKey}:${index}`);
      return rendered.start > target.start || rendered.end < target.end;
    });
  }, [groupLayouts, groupRows, rowHeight, scrollTop, virtualDatasetEpoch, virtualStableKey, virtualizedGroups, viewportHeight]);

  const handleBodyScroll = useCallback(() => {
    const scroll = scrollRef.current;
    if (!scroll) return;
    const viewportTop = scroll.scrollTop;
    if (freezeColumn) syncScrollHeader(scroll.scrollLeft);
    if (!virtualizedRows && !virtualizedGroups) {
      syncScrollPosition();
      return;
    }
    const headerHeight = showColumnHeader && !freezeColumn ? DATA_TABLE_GROUP_HEADER_HEIGHT : 0;
    const renderedTop = headerHeight + visibleRows.top;
    const renderedBottom = renderedTop + visibleRows.rows.length * rowHeight;
    const seekGap = virtualizedRows
      ? virtualSeekGap(viewportTop, scroll.clientHeight, renderedTop, renderedBottom)
      : null;
    const groupedWindowMissed = groupedWindowMissedAt(viewportTop, scroll.clientHeight);
    const groupedSeekGap = groupedWindowMissed
      ? groupVirtualSeekGap(groupLayouts, groupRows, scrollTop, viewportTop, scroll.clientHeight, rowHeight, DATA_TABLE_GROUP_VIRTUAL_OVERSCAN, virtualDatasetEpoch, virtualStableKey)
      : null;
    if (seekGap) setVirtualSeekVisible(true, seekGap.top, seekGap.height);
    else if (groupedSeekGap) setVirtualSeekVisible(true, groupedSeekGap.top, groupedSeekGap.height);
    else setVirtualSeekVisible(false);
    scheduleScrollSync();
  }, [freezeColumn, groupLayouts, groupRows, groupedWindowMissedAt, rowHeight, scheduleScrollSync, scrollRef, scrollTop, setVirtualSeekVisible, showColumnHeader, syncScrollHeader, syncScrollPosition, viewportHeight, virtualDatasetEpoch, virtualStableKey, virtualizedGroups, virtualizedRows, visibleRows]);

  const handleBodyWheel = useCallback((event: ReactWheelEvent<HTMLDivElement>) => {
    if (event.deltaY === 0 || (!virtualizedRows && !virtualizedGroups)) return;
    const scroll = scrollRef.current;
    if (!scroll) return;
    const maxScrollTop = Math.max(0, scroll.scrollHeight - scroll.clientHeight);
    const nextScrollTop = Math.min(maxScrollTop, Math.max(0, scroll.scrollTop + event.deltaY));
    if (Math.abs(scroll.scrollTop - nextScrollTop) <= 1) return;
    const flatWindowMissed = virtualizedRows && (() => {
      const nextRange = fixedVirtualRange(
        modelRows.length,
        nextScrollTop,
        scroll.clientHeight,
        rowHeight,
        DATA_TABLE_VIRTUAL_OVERSCAN,
      );
      return visibleRows.start !== nextRange.start || visibleRows.end !== nextRange.end;
    })();
    const groupedWindowMissed = groupedWindowMissedAt(nextScrollTop, scroll.clientHeight);
    if (flatWindowMissed || groupedWindowMissed) {
      scheduleScrollSync();
    }
  }, [groupedWindowMissedAt, modelRows.length, rowHeight, scheduleScrollSync, scrollRef, virtualizedGroups, virtualizedRows, visibleRows]);

  useLayoutEffect(() => {
    setVirtualSeekVisible(false);
  }, [groupLayouts, modelRows, scrollTop, setVirtualSeekVisible, virtualizedGroups, virtualizedRows, visibleRows]);

  useLayoutEffect(() => {
    if (!freezeColumn) return;
    const scroll = scrollRef.current;
    if (scroll) syncScrollHeader(scroll.scrollLeft);
  });

  const renderFrozenTable = () => (
    <>
      {showColumnHeader ? (
        <div className="dataTableSplitHeader">
          <div className="dataTableFrozenHeaderPane">
            <div className="dataTableFrozenSurface">{frozenHeaderRow}</div>
          </div>
          <div className="dataTableScrollHeaderPane">
            <div ref={scrollHeaderTrackRef} className="dataTableScrollHeaderTrack" style={{ transform: "translateX(0px)" }}>
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
        onWheelCapture={handleBodyWheel}
        onMouseDownCapture={enableMarquee ? beginMarquee : undefined}
        onClickCapture={enableMarquee ? suppressClickAfterMarquee : undefined}
      >
        <div ref={virtualSeekRef} className="dataTableVirtualSeek" aria-hidden="true" />
        <div className="dataTableSplitSurface">
          <div className="dataTableFrozenPane">
            <div className="dataTableBody">
              {grouping[0]
                ? renderFrozenGroups()
                : renderVisibleRows((row) => renderSplitDataRow(row, DataTablePane.Frozen))}
            </div>
          </div>
          <div className="dataTableScrollPane">
            <div className="dataTableBody">
              {grouping[0]
                ? renderScrollGroups()
                : renderVisibleRows((row) => renderSplitDataRow(row, DataTablePane.Scroll))}
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
              onWheelCapture={handleBodyWheel}
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
              <div ref={virtualSeekRef} className="dataTableVirtualSeek" aria-hidden="true" />
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
          actionsClassName={bottomBarActionsClassName}
        >
          {bottomBar(selectedRows, { clear: clearSelection })}
        </SelectionActionBar>
      ) : null}
    </div>
  );
}
