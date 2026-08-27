import type { ChangeEventHandler, KeyboardEventHandler, ReactNode } from "react";
import { Search } from "lucide-react";

import { SearchClearButton } from "./SearchClearButton.tsx";
import "./SearchField.css";

export type SearchFieldProps = {
  value: string;
  onChange: ChangeEventHandler<HTMLInputElement>;
  placeholder?: string;
  "aria-label"?: string;
  className?: string;
  pageSearch?: boolean;
  onKeyDown?: KeyboardEventHandler<HTMLInputElement>;
  onClear: () => void;
  clearLabel?: string;
  endContent?: ReactNode;
};

export function SearchField({
  value,
  onChange,
  placeholder,
  "aria-label": ariaLabel,
  className = "",
  pageSearch = false,
  onKeyDown,
  onClear,
  clearLabel,
  endContent,
}: SearchFieldProps) {
  return (
    <div className={className ? `searchBox ${className}` : "searchBox"} data-page-search={pageSearch ? "true" : undefined}>
      <Search size={18} />
      <input aria-label={ariaLabel} placeholder={placeholder} value={value} onChange={onChange} onKeyDown={onKeyDown} />
      <SearchClearButton value={value} onClear={onClear} ariaLabel={clearLabel} />
      {endContent ? <span className="searchBoxEndContent">{endContent}</span> : null}
    </div>
  );
}
