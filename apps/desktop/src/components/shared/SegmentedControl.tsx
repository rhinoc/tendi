import { motion, useReducedMotion } from "motion/react";
import { ToggleGroup } from "radix-ui";
import { createContext, useContext, useId, useState, type ReactNode } from "react";

import { SPRING_LAYOUT } from "../../lib/ease.ts";
import "./SegmentedControl.css";

export type SegmentedControlVariant = "default" | "icon";

export type SegmentedControlProps = Omit<
  ToggleGroup.ToggleGroupSingleProps,
  "type" | "children" | "className"
> & {
  children?: ReactNode;
  className?: string;
  fullWidth?: boolean;
  variant?: SegmentedControlVariant;
};

type SegmentedControlContextValue = {
  activeValue: string | undefined;
  layoutId: string;
  reduceMotion: boolean;
};

const SegmentedControlContext = createContext<SegmentedControlContextValue | null>(null);

function useSegmentedControlContext() {
  const context = useContext(SegmentedControlContext);
  if (!context) throw new Error("SegmentedControlItem must be used inside SegmentedControl");
  return context;
}

export function SegmentedControl({
  children,
  className = "",
  fullWidth = false,
  variant = "default",
  value,
  defaultValue,
  onValueChange,
  ...props
}: SegmentedControlProps) {
  const controlled = value !== undefined;
  const [uncontrolledValue, setUncontrolledValue] = useState(defaultValue);
  const layoutId = useId();
  const reduceMotion = useReducedMotion() ?? false;
  const activeValue = controlled ? value : uncontrolledValue;

  const handleValueChange = (nextValue: string) => {
    if (!controlled) setUncontrolledValue(nextValue);
    onValueChange?.(nextValue);
  };

  const rootProps = {
    ...props,
    type: "single" as const,
    value: controlled ? value : undefined,
    defaultValue: controlled ? undefined : defaultValue,
    onValueChange: handleValueChange,
    className: ["segmentedControl", className].filter(Boolean).join(" "),
    "data-full-width": fullWidth ? "true" : undefined,
    "data-variant": variant,
  };

  return (
    <SegmentedControlContext.Provider value={{ activeValue, layoutId, reduceMotion }}>
      <ToggleGroup.Root {...rootProps}>{children}</ToggleGroup.Root>
    </SegmentedControlContext.Provider>
  );
}

export type SegmentedControlItemProps = ToggleGroup.ToggleGroupItemProps;

export function SegmentedControlItem({ className = "", children, value, ...props }: SegmentedControlItemProps) {
  const { activeValue, layoutId, reduceMotion } = useSegmentedControlContext();
  const active = activeValue === value;

  return (
    <ToggleGroup.Item
      {...props}
      value={value}
      className={["segmentedControlItem", className].filter(Boolean).join(" ")}
    >
      {active ? (
        <motion.span
          layoutId={layoutId}
          layout="position"
          transition={reduceMotion ? { duration: 0 } : SPRING_LAYOUT}
          className="segmentedControlIndicator"
          aria-hidden="true"
        />
      ) : null}
      {children}
    </ToggleGroup.Item>
  );
}
