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
  onlyWhenTruncated?: boolean;
}

function isTruncated(element: HTMLElement | null) {
  if (!element) return false;
  return [element, ...element.querySelectorAll<HTMLElement>("*")].some((candidate) => (
    candidate.scrollWidth > candidate.clientWidth + 1
    || candidate.scrollHeight > candidate.clientHeight + 1
  ));
}

function TooltipPopup({ content }: { content: ReactNode }) {
  return (
    <RadixTooltip.Portal>
      <RadixTooltip.Content
        className="appTooltip"
        sideOffset={6}
        collisionPadding={8}
      >
        {content}
        <RadixTooltip.Arrow asChild width={8} height={4}>
          <svg className="appTooltipArrow" viewBox="0 0 30 10" aria-hidden="true">
            <polygon points="0,-2 30,-2 15,10" />
            <polyline points="0,-2 15,10 30,-2" />
          </svg>
        </RadixTooltip.Arrow>
      </RadixTooltip.Content>
    </RadixTooltip.Portal>
  );
}

function TruncatedTooltip({ children, content }: Omit<TooltipProps, "onlyWhenTruncated">) {
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLButtonElement>(null);

  return (
    <RadixTooltip.Root
      open={open}
      onOpenChange={(nextOpen) => setOpen(nextOpen && isTruncated(triggerRef.current))}
    >
      <RadixTooltip.Trigger asChild ref={triggerRef}>{children}</RadixTooltip.Trigger>
      <TooltipPopup content={content} />
    </RadixTooltip.Root>
  );
}

export function Tooltip({ children, content, onlyWhenTruncated = false }: TooltipProps) {
  if (content === null || content === undefined || content === false || content === "") {
    return children;
  }

  if (onlyWhenTruncated) {
    return <TruncatedTooltip content={content}>{children}</TruncatedTooltip>;
  }

  return (
    <RadixTooltip.Root>
      <RadixTooltip.Trigger asChild>{children}</RadixTooltip.Trigger>
      <TooltipPopup content={content} />
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
