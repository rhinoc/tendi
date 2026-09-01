import type { ReactNode } from "react";

import type { SortDirection } from "../lib/sort.ts";

export enum ColumnCellVariant {
  Text = "text",
  Title = "title",
  Number = "number",
}

export enum ColumnDataType {
  Text = "text",
  Date = "date",
  Enum = "enum",
}
export type { SortDirection } from "../lib/sort.ts";

export type SortState = {
  key: string;
  direction: SortDirection;
};

export type FreezeColumnConfig = {
  defaultWidth: number;
  min: number;
  max: number;
};

/** Header and body are separate CSS grids synced via scrollLeft — width is required. */
export type ColumnDef<TRow> = {
  key: string;
  width: string;
  header?: string;
  label?: string;
  render?: (row: TRow) => ReactNode;
  value?: (row: TRow) => ReactNode;
  title?: (row: TRow) => string | undefined;
  empty?: string;
  cell?: ColumnCellVariant;
  sortValue?: (row: TRow) => string | number;
  sortable?: boolean;
  type?: ColumnDataType;
  groupable?: boolean;
  groupBy?: (row: TRow) => string;
  groupByDay?: boolean;
  groupLabel?: (key: string) => ReactNode;
  groupOrder?: string[];
  groupKey?: string;
  sticky?: boolean;
};

export type RowContextMenuContext<TRow> = {
  selectedRows: TRow[];
  selected: boolean;
};

export type DataTableProps<TRow> = {
  rows: TRow[];
  columns: ColumnDef<TRow>[];
  getRowId: (row: TRow) => string;
  getRowLabel?: (row: TRow) => string | undefined;
  selectable?: boolean | ((row: TRow) => boolean);
  selectedIds?: string[];
  onSelectionChange?: (ids: string[]) => void;
  enableMarquee?: boolean;
  defaultGroupBy?: string | null;
  groupBy?: string | null;
  onGroupByChange?: (groupBy: string | null) => void;
  groupOrder?: string[];
  groupLabel?: (key: string) => ReactNode;
  defaultSort?: SortState | null;
  sort?: SortState | null;
  onSortChange?: (sort: SortState) => void;
  manualSorting?: boolean;
  rowHeight?: number;
  enableVirtualization?: boolean;
  scrollResetKey?: string;
  scrollToRowId?: string;
  onScrollToRowComplete?: (rowId: string) => void;
  freezeColumn?: FreezeColumnConfig;
  onRowClick?: (row: TRow) => void;
  rowProps?: (row: TRow) => { className?: string };
  rowContextMenu?: (row: TRow, ctx: RowContextMenuContext<TRow>) => ReactNode | null;
  bottomBar?: (selectedRows: TRow[], ctx: { clear: () => void }) => ReactNode;
  bottomBarActionsClassName?: string;
  bottomBarCheckboxLabel?: string;
  selectionLabel?: string;
  loading?: boolean;
  loadingLabel?: string;
  emptyState?: ReactNode;
};
