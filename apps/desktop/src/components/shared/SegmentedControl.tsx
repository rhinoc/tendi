import type { ReactNode } from "react";
import { ToggleGroup } from "radix-ui";

import "./SegmentedControl.css";

export type SegmentedControlVariant = "default" | "accent";

export type SegmentedControlProps = Omit<
  ToggleGroup.ToggleGroupSingleProps,
  "type" | "children" | "className"
> & {
  children?: ReactNode;
  className?: string;
  fullWidth?: boolean;
  variant?: SegmentedControlVariant;
};

export function SegmentedControl({
  children,
  className = "",
  fullWidth = false,
  variant = "default",
  ...props
}: SegmentedControlProps) {
  return (
    <ToggleGroup.Root
      {...props}
      type="single"
      className={["segmentedControl", className].filter(Boolean).join(" ")}
      data-full-width={fullWidth ? "true" : undefined}
      data-variant={variant}
    >
      {children}
    </ToggleGroup.Root>
  );
}

export type SegmentedControlItemProps = ToggleGroup.ToggleGroupItemProps;

export function SegmentedControlItem({ className = "", ...props }: SegmentedControlItemProps) {
  return (
    <ToggleGroup.Item
      {...props}
      className={["segmentedControlItem", className].filter(Boolean).join(" ")}
    />
  );
}
