import { forwardRef, type ComponentPropsWithoutRef, type ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import { Select } from "radix-ui";

type RadixSelectTriggerProps = ComponentPropsWithoutRef<typeof Select.Trigger>;

export type SelectTriggerProps = Omit<RadixSelectTriggerProps, "children"> & {
  label: string;
  children: ReactNode;
  showChevron?: boolean;
  chevronSize?: number;
};

export const SelectTrigger = forwardRef<HTMLButtonElement, SelectTriggerProps>(function SelectTrigger({
  label,
  children,
  className = "",
  showChevron = true,
  chevronSize = 14,
  "aria-label": ariaLabel,
  ...triggerProps
}, ref) {
  return (
    <Select.Trigger
      {...triggerProps}
      ref={ref}
      className={`selectControlTrigger ${className}`.trim()}
      aria-label={ariaLabel ?? label}
    >
      {children}
      {showChevron ? (
        <Select.Icon asChild>
          <ChevronDown size={chevronSize} />
        </Select.Icon>
      ) : null}
    </Select.Trigger>
  );
});
