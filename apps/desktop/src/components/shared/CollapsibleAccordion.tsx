import { motion, useReducedMotion } from "motion/react";
import { useId, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { ChevronDown } from "lucide-react";

import { EASE_OUT } from "../../lib/ease.ts";

import "./CollapsibleAccordion.css";

const CONTENT_OPEN_TRANSITION = {
  type: "spring",
  duration: 0.32,
  bounce: 0.08,
} as const;

const CONTENT_CLOSE_TRANSITION = {
  type: "spring",
  duration: 0.24,
  bounce: 0.06,
} as const;

const CHEVRON_TRANSITION = {
  type: "spring",
  duration: 0.2,
  bounce: 0.05,
} as const;

const ROW_TRANSITION = {
  type: "spring",
  duration: 0.28,
  bounce: 0.06,
} as const;

export type CollapsibleAccordionItem = {
  id: string;
  title: ReactNode;
  content?: ReactNode;
  /**
   * Set this when content is rendered lazily. A collapsed item can have no
   * content in the current render and still needs to remain expandable.
   */
  expandable?: boolean;
  leading?: ReactNode;
  className?: string;
  rowClassName?: string;
  contentClassName?: string;
};

export type CollapsibleAccordionValue = string | string[] | null;

export type CollapsibleAccordionVariant = "default" | "data-table";
export type CollapsibleAccordionDensity = "default" | "compact";
export type CollapsibleAccordionContentPadding = "default" | "none" | "compact" | "inset";
export type CollapsibleAccordionContentOverflow = "hidden" | "visible" | "auto";
export type CollapsibleAccordionSurface = "default" | "transparent";

function classNames(...values: Array<string | undefined>) {
  return values.filter(Boolean).join(" ");
}

function CollapsibleAccordionRow({
  item,
  open,
  startsGroup,
  endsGroup,
  separatedFromPrevious,
  contentId,
  triggerId,
  reduce,
  cornerRadius,
  contentPadding,
  contentOverflow,
  contentMaxHeight,
  surface,
  variant,
  onToggle,
}: {
  item: CollapsibleAccordionItem;
  open: boolean;
  startsGroup: boolean;
  endsGroup: boolean;
  separatedFromPrevious: boolean;
  contentId: string;
  triggerId: string;
  reduce: boolean;
  cornerRadius: number;
  contentPadding: CollapsibleAccordionContentPadding;
  contentOverflow: CollapsibleAccordionContentOverflow;
  contentMaxHeight?: number | string;
  surface: CollapsibleAccordionSurface;
  variant: CollapsibleAccordionVariant;
  onToggle: () => void;
}) {
  const expandable = item.expandable ?? (item.content !== undefined && item.content !== null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [contentHeight, setContentHeight] = useState(0);
  const rowLayout = variant === "data-table"
    ? undefined
    : { marginTop: separatedFromPrevious ? 12 : 0 };

  useLayoutEffect(() => {
    const node = contentRef.current;
    if (!node || !expandable) return;

    const updateHeight = () => setContentHeight(node.offsetHeight);
    updateHeight();
    if (!open || typeof ResizeObserver === "undefined") return;

    const observer = new ResizeObserver(updateHeight);
    observer.observe(node);
    return () => observer.disconnect();
  }, [expandable, open]);

  return (
    <motion.div
      initial={false}
      animate={rowLayout}
      transition={reduce ? { duration: 0 } : ROW_TRANSITION}
      data-state={open ? "open" : "closed"}
      className={classNames("collapsibleAccordionRow", item.rowClassName)}
    >
      <motion.div
        data-state={open ? "open" : "closed"}
        data-accordion-item={item.id}
        initial={false}
        animate={{
          borderTopLeftRadius: startsGroup ? cornerRadius : 0,
          borderTopRightRadius: startsGroup ? cornerRadius : 0,
          borderBottomLeftRadius: endsGroup ? cornerRadius : 0,
          borderBottomRightRadius: endsGroup ? cornerRadius : 0,
        }}
        transition={reduce ? { duration: 0 } : ROW_TRANSITION}
        className={classNames("collapsibleAccordionItem", item.className)}
      >
        <div data-accordion-part="heading" className="collapsibleAccordionHeading">
          {item.leading ? <div className="collapsibleAccordionLeading">{item.leading}</div> : null}
          <button
            id={triggerId}
            type="button"
            data-accordion-part="trigger"
            className="collapsibleAccordionTrigger"
            data-state={open ? "open" : "closed"}
            aria-expanded={expandable ? open : undefined}
            aria-controls={expandable ? contentId : undefined}
            disabled={!expandable}
            onClick={expandable ? onToggle : undefined}
          >
            <div className="collapsibleAccordionTitle">{item.title}</div>
            {expandable ? (
              <motion.span
                aria-hidden="true"
                animate={{ rotate: open ? 180 : 0 }}
                transition={reduce ? { duration: 0 } : CHEVRON_TRANSITION}
                className="collapsibleAccordionChevron"
              >
                <ChevronDown size={16} />
              </motion.span>
            ) : null}
          </button>
        </div>
        {expandable ? (
          <motion.div
            id={contentId}
            role="region"
            aria-labelledby={triggerId}
            aria-hidden={!open}
            inert={!open}
            initial={false}
            animate={{ height: open ? contentHeight : 0 }}
            transition={reduce ? { duration: 0 } : open ? CONTENT_OPEN_TRANSITION : CONTENT_CLOSE_TRANSITION}
            className={classNames("collapsibleAccordionContent", item.contentClassName)}
            data-accordion-part="content"
            data-accordion-content-padding={contentPadding}
            data-accordion-content-overflow={contentOverflow}
            data-accordion-surface={surface}
            style={{ maxHeight: contentMaxHeight }}
          >
            <motion.div
              ref={contentRef}
              initial={false}
              animate={{ opacity: open ? 1 : 0 }}
              transition={reduce ? { duration: 0 } : { duration: 0.18, ease: EASE_OUT }}
              data-accordion-part="content-inner"
              className="collapsibleAccordionContentInner"
            >
              {item.content}
            </motion.div>
          </motion.div>
        ) : null}
      </motion.div>
    </motion.div>
  );
}

export function CollapsibleAccordion({
  items,
  defaultValue = null,
  value,
  onValueChange,
  type = "single",
  className = "",
  cornerRadius = 28,
  separateExpandedItems = true,
  variant = "default",
  density = "default",
  contentPadding = "default",
  contentOverflow = "hidden",
  contentMaxHeight,
  surface = "default",
  reduceMotion = false,
}: {
  items: CollapsibleAccordionItem[];
  defaultValue?: CollapsibleAccordionValue;
  value?: CollapsibleAccordionValue;
  onValueChange?: (value: CollapsibleAccordionValue) => void;
  type?: "single" | "multiple";
  className?: string;
  cornerRadius?: number;
  separateExpandedItems?: boolean;
  variant?: CollapsibleAccordionVariant;
  density?: CollapsibleAccordionDensity;
  contentPadding?: CollapsibleAccordionContentPadding;
  contentOverflow?: CollapsibleAccordionContentOverflow;
  contentMaxHeight?: number | string;
  surface?: CollapsibleAccordionSurface;
  reduceMotion?: boolean;
}) {
  const reduce = (useReducedMotion() ?? false) || reduceMotion;
  const baseId = useId();
  const [internalValue, setInternalValue] = useState<CollapsibleAccordionValue>(defaultValue);
  const currentValue = value === undefined ? internalValue : value;
  const openValues = new Set(
    type === "multiple"
      ? Array.isArray(currentValue) ? currentValue : currentValue ? [currentValue] : []
      : typeof currentValue === "string" ? [currentValue] : [],
  );

  return (
    <div
      className={`collapsibleAccordion ${className}`}
      data-accordion-variant={variant}
      data-accordion-density={density}
      data-accordion-content-padding={contentPadding}
      data-accordion-content-overflow={contentOverflow}
      data-accordion-surface={surface}
    >
      {items.map((item, index) => {
        const open = openValues.has(item.id);
        const previousIsOpen = index > 0 && openValues.has(items[index - 1]!.id);
        const nextIsOpen = index < items.length - 1 && openValues.has(items[index + 1]!.id);
        const startsGroup = open || index === 0 || previousIsOpen;
        const endsGroup = open || index === items.length - 1 || nextIsOpen;
        const separatedFromPrevious = separateExpandedItems && index > 0 && (open || previousIsOpen);
        const contentId = `${baseId}-${item.id}-content`;
        const triggerId = `${baseId}-${item.id}-trigger`;

        const toggle = () => {
          let nextValue: CollapsibleAccordionValue;
          if (type === "multiple") {
            const nextValues = new Set(openValues);
            if (open) nextValues.delete(item.id);
            else nextValues.add(item.id);
            nextValue = [...nextValues];
          } else {
            nextValue = open ? null : item.id;
          }
          if (value === undefined) setInternalValue(nextValue);
          onValueChange?.(nextValue);
        };

        return (
          <CollapsibleAccordionRow
            key={item.id}
            item={item}
            open={open}
            startsGroup={startsGroup}
            endsGroup={endsGroup}
            separatedFromPrevious={separatedFromPrevious}
            contentId={contentId}
            triggerId={triggerId}
            reduce={reduce}
            cornerRadius={cornerRadius}
            contentPadding={contentPadding}
            contentOverflow={contentOverflow}
            contentMaxHeight={contentMaxHeight}
            surface={surface}
            variant={variant}
            onToggle={toggle}
          />
        );
      })}
    </div>
  );
}
