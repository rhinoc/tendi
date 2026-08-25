import type { CSSProperties, ReactNode } from "react";

import { Tooltip } from "./Tooltip.tsx";
import "./CompactTable.css";

export type CompactTableColumn<TRow> = {
  key: string;
  header: ReactNode;
  width: string;
  value?: (row: TRow) => ReactNode;
  title?: (row: TRow) => ReactNode;
  cellClassName?: string;
  empty?: ReactNode;
};

export type CompactTableProps<TRow> = {
  rows: readonly TRow[];
  columns: readonly CompactTableColumn<TRow>[];
  getRowId?: (row: TRow, index: number) => string;
  ariaLabel: string;
  emptyState?: ReactNode;
  className?: string;
};

function valueForColumn<TRow>(column: CompactTableColumn<TRow>, row: TRow) {
  if (column.value) return column.value(row);
  return (row as Record<string, unknown>)[column.key] as ReactNode;
}

function displayValue<TRow>(column: CompactTableColumn<TRow>, row: TRow) {
  const value = valueForColumn(column, row);
  return value === null || value === undefined || value === ""
    ? (column.empty ?? "-")
    : value;
}

export function CompactTable<TRow>({
  rows,
  columns,
  getRowId = (_row, index) => `${index}`,
  ariaLabel,
  emptyState = "No items",
  className = "",
}: CompactTableProps<TRow>) {
  return (
    <div className={["compactTableFrame", className].filter(Boolean).join(" ")}>
      <table className="compactTable" aria-label={ariaLabel}>
        <colgroup>
          {columns.map((column) => <col key={column.key} style={{ width: column.width } satisfies CSSProperties} />)}
        </colgroup>
        <thead>
          <tr>
            {columns.map((column) => <th scope="col" key={column.key}>{column.header}</th>)}
          </tr>
        </thead>
        <tbody>
          {rows.length === 0 ? (
            <tr>
              <td className="compactTableEmpty" colSpan={columns.length}>{emptyState}</td>
            </tr>
          ) : rows.map((row, index) => (
            <tr key={getRowId(row, index)}>
              {columns.map((column) => {
                const display = displayValue(column, row);
                const title = column.title?.(row)
                  ?? (typeof display === "string" && display !== (column.empty ?? "-") ? display : undefined);
                const cellClassName = [
                  "compactTableCell",
                  column.cellClassName,
                ].filter(Boolean).join(" ");
                return (
                  <td className={cellClassName} key={column.key}>
                    <Tooltip content={title} onlyWhenTruncated>
                      <span className="compactTableCellText">{display}</span>
                    </Tooltip>
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
