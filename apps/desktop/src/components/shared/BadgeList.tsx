import { useLayoutEffect, useRef, useState } from "react";

import { Badge } from "./Badge.tsx";
import { Tooltip } from "./Tooltip.tsx";
import "./BadgeList.css";

export type BadgeListProps = {
  items: string[];
  ariaLabel?: string;
  active?: boolean;
  className?: string;
};

export function BadgeList({ items, ariaLabel = "Selected items", active = true, className = "" }: BadgeListProps) {
  const listRef = useRef<HTMLSpanElement>(null);
  const measureRef = useRef<HTMLSpanElement>(null);
  const [visibleItemCount, setVisibleItemCount] = useState(items.length);
  const itemsKey = items.join("\u0000");

  useLayoutEffect(() => {
    if (!active) {
      setVisibleItemCount(items.length);
      return;
    }
    const list = listRef.current;
    const measure = measureRef.current;
    if (!list || !measure) return;

    const recalculate = () => {
      const availableWidth = list.clientWidth;
      measure.style.width = `${availableWidth}px`;
      const gap = Number.parseFloat(getComputedStyle(list).columnGap) || 0;
      const itemWidths = Array.from(measure.querySelectorAll<HTMLElement>("[data-measure-item]"))
        .map((badge) => badge.getBoundingClientRect().width);
      const overflowWidths = new Map(
        Array.from(measure.querySelectorAll<HTMLElement>("[data-measure-overflow]"))
          .map((badge) => [Number(badge.dataset.measureOverflow), badge.getBoundingClientRect().width]),
      );
      let nextCount = items.length;

      for (let count = items.length; count >= 0; count -= 1) {
        const hiddenCount = items.length - count;
        const itemCount = count + (hiddenCount > 0 ? 1 : 0);
        const itemWidth = itemWidths.slice(0, count).reduce((total, width) => total + width, 0);
        const overflowWidth = hiddenCount > 0 ? overflowWidths.get(hiddenCount) ?? 0 : 0;
        const totalWidth = itemWidth + overflowWidth + Math.max(0, itemCount - 1) * gap;
        if (totalWidth <= availableWidth) {
          nextCount = count;
          break;
        }
      }

      setVisibleItemCount((current) => current === nextCount ? current : nextCount);
    };

    recalculate();
    const observedOwner = list.parentElement ?? list;
    let observedWidth = observedOwner.getBoundingClientRect().width;
    const observer = new ResizeObserver(([entry]) => {
      const nextWidth = entry.contentRect.width;
      if (nextWidth === observedWidth) return;
      observedWidth = nextWidth;
      recalculate();
    });
    observer.observe(observedOwner);
    return () => observer.disconnect();
  }, [active, itemsKey]);

  const hiddenItemCount = items.length - visibleItemCount;
  const hiddenItems = items.slice(visibleItemCount);
  return (
    <span className={["badgeList", className].filter(Boolean).join(" ")} ref={listRef} aria-label={ariaLabel}>
      {items.slice(0, visibleItemCount).map((item, index) => (
        <Badge tone="accent" key={`${item}-${index}`}>{item}</Badge>
      ))}
      {hiddenItemCount > 0 ? (
        <Tooltip content={hiddenItems.join("\n")}>
          <span
            className="badgeListOverflow"
            aria-label={`Hidden items: ${hiddenItems.join(", ")}`}
          >
            <Badge tone="neutral">{hiddenItemCount}+</Badge>
          </span>
        </Tooltip>
      ) : null}
      <span className="badgeListMeasure" ref={measureRef} aria-hidden="true">
        {items.map((item, index) => (
          <Badge tone="accent" data-measure-item={index} key={`measure-item-${item}-${index}`}>
            {item}
          </Badge>
        ))}
        {items.map((_, index) => {
          const hiddenCount = items.length - index;
          return (
            <Badge tone="neutral" data-measure-overflow={hiddenCount} key={`measure-overflow-${hiddenCount}`}>
              {hiddenCount}+
            </Badge>
          );
        })}
      </span>
    </span>
  );
}
