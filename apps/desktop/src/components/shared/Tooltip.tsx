import {
  useRef,
  useState,
  type ComponentPropsWithoutRef,
  type ReactElement,
  type ReactNode,
} from "react";
import { Tooltip as RadixTooltip } from "radix-ui";
import "./Tooltip.css";

const TOOLTIP_DELAY = 500;
type TooltipContentProps = ComponentPropsWithoutRef<typeof RadixTooltip.Content>;

export interface TooltipProps {
  children: ReactElement;
  content: ReactNode;
  interactive?: boolean;
  onlyWhenTruncated?: boolean;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  side?: TooltipContentProps["side"];
  align?: TooltipContentProps["align"];
  sideOffset?: TooltipContentProps["sideOffset"];
  collisionPadding?: TooltipContentProps["collisionPadding"];
  className?: string;
  unstyled?: boolean;
}

function isTruncated(element: HTMLElement | null) {
  if (!element) return false;
  return [element, ...element.querySelectorAll<HTMLElement>("*")].some((candidate) => {
    const style = window.getComputedStyle(candidate);
    const clipsHorizontally = style.overflowX === "hidden" || style.overflowX === "clip";
    const clipsVertically = style.overflowY === "hidden" || style.overflowY === "clip";
    return (
      (clipsHorizontally && candidate.scrollWidth > candidate.clientWidth + 1)
      || (clipsVertically && candidate.scrollHeight > candidate.clientHeight + 1)
    );
  });
}

type TooltipPopupProps = Omit<TooltipProps, "children" | "onlyWhenTruncated" | "open" | "onOpenChange">;

function TooltipPopup({
  content,
  interactive = false,
  side,
  align,
  sideOffset = 6,
  collisionPadding = 8,
  className = "",
  unstyled = false,
}: TooltipPopupProps) {
  const popupClassName = [
    unstyled ? "" : "appTooltip",
    interactive && !unstyled ? "isInteractive" : "",
    className,
  ].filter(Boolean).join(" ");

  return (
    <RadixTooltip.Portal>
      <RadixTooltip.Content
        className={popupClassName}
        side={side}
        align={align}
        sideOffset={sideOffset}
        collisionPadding={collisionPadding}
      >
        {content}
      </RadixTooltip.Content>
    </RadixTooltip.Portal>
  );
}

function TruncatedTooltip({
  children,
  content,
  interactive,
  open,
  onOpenChange,
  ...popupProps
}: Omit<TooltipProps, "onlyWhenTruncated">) {
  const [internalOpen, setInternalOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const isControlled = open !== undefined;

  return (
    <RadixTooltip.Root
      open={isControlled ? open : internalOpen}
      onOpenChange={(nextOpen) => {
        const allowedOpen = nextOpen && isTruncated(triggerRef.current);
        if (!isControlled) setInternalOpen(allowedOpen);
        onOpenChange?.(allowedOpen);
      }}
    >
      <RadixTooltip.Trigger asChild ref={triggerRef}>{children}</RadixTooltip.Trigger>
      <TooltipPopup content={content} interactive={interactive} {...popupProps} />
    </RadixTooltip.Root>
  );
}

export function Tooltip({
  children,
  content,
  interactive = true,
  onlyWhenTruncated = false,
  open,
  onOpenChange,
  ...popupProps
}: TooltipProps) {
  if (content === null || content === undefined || content === false || content === "") {
    return children;
  }

  if (onlyWhenTruncated) {
    return (
      <TruncatedTooltip
        content={content}
        interactive={interactive}
        open={open}
        onOpenChange={onOpenChange}
        {...popupProps}
      >
        {children}
      </TruncatedTooltip>
    );
  }

  return (
    <RadixTooltip.Root open={open} onOpenChange={onOpenChange}>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <TooltipPopup content={content} interactive={interactive} {...popupProps} />
    </RadixTooltip.Root>
  );
}

export interface TooltipProviderProps {
  children: ReactNode;
}

export function TooltipProvider({ children }: TooltipProviderProps) {
  return (
    <RadixTooltip.Provider delayDuration={TOOLTIP_DELAY} skipDelayDuration={150}>
      {children}
    </RadixTooltip.Provider>
  );
}
