import type { ChangeEventHandler } from "react";
import { Search } from "lucide-react";

export type SearchFieldProps = {
  value: string;
  onChange: ChangeEventHandler<HTMLInputElement>;
  placeholder?: string;
  "aria-label"?: string;
  className?: string;
};

export function SearchField({ value, onChange, placeholder, "aria-label": ariaLabel, className = "" }: SearchFieldProps) {
  return (
    <div className={className ? `searchBox ${className}` : "searchBox"}>
      <Search size={15} />
      <input aria-label={ariaLabel} placeholder={placeholder} value={value} onChange={onChange} />
    </div>
  );
}
