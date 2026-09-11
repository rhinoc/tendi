"use client";
// beui.dev/components/motion/expanding-arrow-button

import {
  motion,
  useReducedMotion,
  type HTMLMotionProps,
} from "motion/react";
import {
  forwardRef,
  type FocusEvent,
  type MouseEvent,
  type ReactNode,
  useState,
} from "react";

import { EASE_OUT, SPRING_LAYOUT, SPRING_PRESS } from "../../lib/ease.ts";
import { useHoverCapable } from "../../lib/hooks/use-hover-capable.ts";
import "./expanding-arrow-button.css";

export interface ExpandingArrowButtonProps extends Omit<
  HTMLMotionProps<"button">,
  "children"
> {
  children: ReactNode;
  accentClassName?: string;
  labelClassName?: string;
  compact?: boolean;
  expanded?: boolean;
  expandOnFocus?: boolean;
}

const ARROW_OPACITY = [1, 0.78, 0.54, 0.32, 0.16] as const;

function cn(...values: Array<string | undefined | false>) {
  return values.filter(Boolean).join(" ");
}

function DottedChevron({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 20 28"
      fill="none"
      aria-hidden="true"
      className={className}
    >
      <circle cx="4" cy="4" r="2" fill="currentColor" />
      <circle cx="10" cy="9" r="2" fill="currentColor" />
      <circle cx="16" cy="14" r="2" fill="currentColor" />
      <circle cx="10" cy="19" r="2" fill="currentColor" />
      <circle cx="4" cy="24" r="2" fill="currentColor" />
    </svg>
  );
}

export const ExpandingArrowButton = forwardRef<
  HTMLButtonElement,
  ExpandingArrowButtonProps
>(function ExpandingArrowButton(
  {
    children,
    className,
    accentClassName,
    labelClassName,
    compact = false,
    expanded = false,
    expandOnFocus = true,
    disabled,
    onMouseEnter,
    onMouseLeave,
    onFocus,
    onBlur,
    ...rest
  },
  ref,
) {
  const reduce = useReducedMotion();
  const canHover = useHoverCapable();
  const [hovered, setHovered] = useState(false);
  const [focused, setFocused] = useState(false);
  const active = expanded || (!disabled && ((canHover && hovered) || (expandOnFocus && focused)));
  const layoutTransition = reduce ? { duration: 0 } : SPRING_LAYOUT;

  const handleMouseEnter = (event: MouseEvent<HTMLButtonElement>) => {
    setHovered(true);
    onMouseEnter?.(event);
  };

  const handleMouseLeave = (event: MouseEvent<HTMLButtonElement>) => {
    setHovered(false);
    onMouseLeave?.(event);
  };

  const handleFocus = (event: FocusEvent<HTMLButtonElement>) => {
    setFocused(true);
    onFocus?.(event);
  };

  const handleBlur = (event: FocusEvent<HTMLButtonElement>) => {
    setFocused(false);
    onBlur?.(event);
  };

  return (
    <motion.button
      ref={ref}
      type="button"
      disabled={disabled}
      onMouseEnter={handleMouseEnter}
      onMouseLeave={handleMouseLeave}
      onFocus={handleFocus}
      onBlur={handleBlur}
      whileTap={reduce || disabled ? undefined : { scale: 0.97 }}
      transition={SPRING_PRESS}
      className={cn(
        "expandingArrowButton",
        compact ? "expandingArrowButtonCompact" : undefined,
        className,
      )}
      {...rest}
    >
      <motion.span
        layout="size"
        aria-hidden="true"
        transition={layoutTransition}
        style={{
          width: active ? "calc(100% - 6px)" : compact ? 26 : 52,
          borderRadius: compact ? 6 : 16,
        }}
        className={cn("expandingArrowButtonAccent", accentClassName)}
      >
        <motion.span
          animate={{ opacity: active ? 0 : 1 }}
          transition={{ duration: reduce ? 0 : 0.1, ease: EASE_OUT }}
          className="expandingArrowButtonChevron"
        >
          <DottedChevron className={cn("expandingArrowButtonChevronIcon", compact ? "compact" : undefined)} />
        </motion.span>

        <span className="expandingArrowButtonTrail">
          {ARROW_OPACITY.map((opacity, index) => (
            <motion.span
              key={opacity}
              animate={{
                opacity: active ? 1 : 0,
                transform:
                  active && !reduce ? "translateX(0px)" : "translateX(-6px)",
              }}
              transition={{
                duration: reduce ? 0 : 0.18,
                delay: active && !reduce ? 0.04 + index * 0.025 : 0,
                ease: EASE_OUT,
              }}
              style={{ color: `color-mix(in srgb, var(--accent) ${opacity * 100}%, transparent)` }}
              className="expandingArrowButtonTrailItem"
            >
              <DottedChevron className={cn("expandingArrowButtonChevronIcon", compact ? "compact" : undefined)} />
            </motion.span>
          ))}
        </span>
      </motion.span>

      <motion.span
        animate={{
          opacity: active ? 0 : 1,
          transform:
            active && !reduce ? "translateX(6px)" : "translateX(0px)",
        }}
        transition={{ duration: reduce ? 0 : 0.12, ease: EASE_OUT }}
        className={cn("expandingArrowButtonLabel", labelClassName)}
      >
        {children}
      </motion.span>
    </motion.button>
  );
});
