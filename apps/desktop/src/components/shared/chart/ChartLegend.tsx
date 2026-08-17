import type { ReactNode } from "react";

import "./ChartLegend.css";

export type ChartLegendItem = {
  key: string;
  label: ReactNode;
  swatchClassName?: string;
};

export type ChartLegendProps = {
  items: readonly ChartLegendItem[];
  ariaLabel?: string;
  className?: string;
};

export function ChartLegend({ items, ariaLabel, className = "" }: ChartLegendProps) {
  const isEmpty = items.length === 0;
  return (
    <div
      className={["chartLegend", isEmpty ? "chartLegendEmpty" : "", className].filter(Boolean).join(" ")}
      aria-label={isEmpty ? undefined : ariaLabel}
      aria-hidden={isEmpty || undefined}
    >
      {items.map((item) => (
        <span className="chartLegendItem" key={item.key}>
          <span className={`chartLegendSwatch ${item.swatchClassName ?? ""}`.trim()} aria-hidden="true" />
          <span>{item.label}</span>
        </span>
      ))}
    </div>
  );
}
