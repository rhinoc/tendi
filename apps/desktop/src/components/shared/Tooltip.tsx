import {
  useRef,
  useState,
  type ReactElement,
  type ReactNode,
} from "react";
import { Tooltip as RadixTooltip } from "radix-ui";
import "./Tooltip.css";

const TOOLTIP_DELAY = 500;

export interface TooltipProps {
  children: ReactElement;
  content: ReactNode;
  interactive?: boolean;
  onlyWhenTruncated?: boolean;
}

function isTruncated(element: HTMLElement | null) {
  if (!element) return false;
  return [element, ...element.querySelectorAll<HTMLElement>("*")].some((candidate) => (
    candidate.scrollWidth > candidate.clientWidth + 1
    || candidate.scrollHeight > candidate.clientHeight + 1
  ));
}

function TooltipPopup({ content, interactive = false }: Pick<TooltipProps, "content" | "interactive">) {
  return (
    <RadixTooltip.Portal>
      <RadixTooltip.Content
        className={`appTooltip${interactive ? " isInteractive" : ""}`}
        sideOffset={6}
        collisionPadding={8}
      >
        {content}
      </RadixTooltip.Content>
    </RadixTooltip.Portal>
  );
}

function TruncatedTooltip({ children, content, interactive }: Omit<TooltipProps, "onlyWhenTruncated">) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);

  return (
    <RadixTooltip.Root
      open={open}
      onOpenChange={(nextOpen) => setOpen(nextOpen && isTruncated(triggerRef.current))}
    >
      <RadixTooltip.Trigger asChild ref={triggerRef}>{children}</RadixTooltip.Trigger>
      <TooltipPopup content={content} interactive={interactive} />
    </RadixTooltip.Root>
  );
}

export function Tooltip({ children, content, interactive = false, onlyWhenTruncated = false }: TooltipProps) {
  if (content === null || content === undefined || content === false || content === "") {
    return children;
  }

  if (onlyWhenTruncated) {
    return <TruncatedTooltip content={content} interactive={interactive}>{children}</TruncatedTooltip>;
  }

  return (
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <TooltipPopup content={content} interactive={interactive} />
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
