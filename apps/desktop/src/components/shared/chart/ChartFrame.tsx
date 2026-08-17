import type { ReactNode } from "react";

import "./ChartFrame.css";

export type ChartFrameProps = {
  children: ReactNode;
  legend?: ReactNode;
  emptyState?: ReactNode;
  ariaLabel?: string;
  ariaLabelledBy?: string;
  className?: string;
};

export function ChartFrame({
  children,
  legend = null,
  emptyState = null,
  ariaLabel,
  ariaLabelledBy,
  className = "",
}: ChartFrameProps) {
  return (
    <section
      className={["chartFrame", className].filter(Boolean).join(" ")}
      aria-label={ariaLabel}
      aria-labelledby={ariaLabelledBy}
    >
      {legend}
      {emptyState ?? children}
    </section>
  );
}
