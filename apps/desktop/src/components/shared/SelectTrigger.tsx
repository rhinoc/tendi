import type { ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { Select } from "radix-ui";

export type SelectTriggerProps = {
  label: string;
  children: ReactNode;
  className?: string;
  showChevron?: boolean;
  chevronSize?: number;
};

export function SelectTrigger({
  label,
  children,
  className = "",
  showChevron = true,
  chevronSize = 14,
}: SelectTriggerProps) {
  return (
    <Select.Trigger className={`selectControlTrigger ${className}`.trim()} aria-label={label}>
      {children}
      {showChevron ? (
        <Select.Icon asChild>
          <ChevronDown size={chevronSize} />
        </Select.Icon>
      ) : null}
    </Select.Trigger>
  );
}
