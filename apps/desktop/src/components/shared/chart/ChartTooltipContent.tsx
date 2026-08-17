import type { ReactNode } from "react";

import "./ChartTooltipContent.css";

export type ChartTooltipContentClassNames = Partial<{
  root: string;
  header: string;
  details: string;
  detail: string;
  swatch: string;
  label: string;
  value: string;
}>;

export type ChartTooltipDetail = {
  key: string;
  label: ReactNode;
  value: ReactNode;
  swatchClassName?: string;
};

export type ChartTooltipContentProps = {
  title: ReactNode;
  value?: ReactNode;
  details?: readonly ChartTooltipDetail[];
  footer?: ReactNode;
  className?: string;
  classNames?: ChartTooltipContentClassNames;
};

export function ChartTooltipContent({
  title,
  value,
  details = [],
  footer = null,
  className = "",
  classNames = {},
}: ChartTooltipContentProps) {
  const classes = (base: string, custom?: string) => [base, custom].filter(Boolean).join(" ");
  return (
    <div className={classes(classes("chartTooltipContent", classNames.root), className)}>
      <div className={classes("chartTooltipHeader", classNames.header)}>
        <strong>{title}</strong>
        {value !== undefined ? <span>{value}</span> : null}
      </div>
      {details.length ? (
        <div className={classes("chartTooltipDetails", classNames.details)}>
          {details.map((detail) => (
            <div className={classes("chartTooltipDetail", classNames.detail)} key={detail.key}>
              <span className={classes(`chartTooltipDetailSwatch ${detail.swatchClassName ?? ""}`.trim(), classNames.swatch)} aria-hidden="true" />
              <span className={classes("chartTooltipDetailLabel", classNames.label)}>{detail.label}</span>
              <strong className={classes("chartTooltipDetailValue", classNames.value)}>{detail.value}</strong>
            </div>
          ))}
        </div>
      ) : null}
      {footer}
    </div>
  );
}
