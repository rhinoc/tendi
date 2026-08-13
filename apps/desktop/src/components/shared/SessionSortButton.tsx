import { ChevronDown } from "lucide-react";

import type { SortState } from "../DataTable.types";

export type SessionSortColumn = {
  key: string;
  label?: string;
};

export type SessionSortButtonProps = {
  column: SessionSortColumn;
  sort: SortState;
  onSort: (key: string) => void;
};

export function SessionSortButton({ column, sort, onSort }: SessionSortButtonProps) {
  const active = sort.key === column.key;
  return (
    <button
      className={`sessionSortButton ${active ? "active" : ""} ${active && sort.direction === "asc" ? "ascending" : ""}`}
      onClick={() => onSort(column.key)}
    >
      <span>{column.label}</span>
      <ChevronDown size={13} />
    </button>
  );
}
