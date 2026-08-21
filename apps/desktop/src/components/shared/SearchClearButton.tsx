import { X } from "lucide-react";

import "./SearchClearButton.css";

export type SearchClearButtonProps = {
  value: string;
  onClear: () => void;
  ariaLabel?: string;
  className?: string;
};

export function SearchClearButton({ value, onClear, ariaLabel = "Clear search", className = "" }: SearchClearButtonProps) {
  const empty = value.length === 0;
  return (
    <button
      type="button"
      className={`searchClearButton${empty ? " isEmpty" : ""}${className ? ` ${className}` : ""}`}
      aria-label={ariaLabel}
      disabled={empty}
      tabIndex={empty ? -1 : undefined}
      onMouseDown={(event) => event.preventDefault()}
      onClick={onClear}
    >
      <X size={14} aria-hidden="true" />
    </button>
  );
}
