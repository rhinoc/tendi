import { ChevronRight } from "lucide-react";
import { useId, type HTMLAttributes, type ReactNode } from "react";

import "./Disclosure.css";

export type DisclosureContentPadding = "default" | "compact" | "inset";
export type DisclosureSummarySize = "default" | "comfortable";

export type DisclosureProps = Omit<HTMLAttributes<HTMLDivElement>, "children" | "className"> & {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  summary: ReactNode;
  children?: ReactNode;
  className?: string;
  summaryRowClassName?: string;
  summaryClassName?: string;
  detailsClassName?: string;
  detailsId?: string;
  summaryAriaLabel?: string;
  summaryActions?: ReactNode;
  contentPadding?: DisclosureContentPadding;
  summarySize?: DisclosureSummarySize;
};

function joinClassNames(...classNames: Array<string | undefined>): string {
  return classNames.filter(Boolean).join(" ");
}

export function Disclosure({
  open,
  onOpenChange,
  summary,
  children,
  className,
  summaryRowClassName,
  summaryClassName,
  detailsClassName,
  detailsId,
  summaryAriaLabel,
  summaryActions,
  contentPadding = "default",
  summarySize = "default",
  ...rootProps
}: DisclosureProps) {
  const generatedId = useId().replace(/[^a-zA-Z0-9_-]/g, "-");
  const resolvedDetailsId = detailsId ?? `disclosure-details-${generatedId}`;

  return (
    <div
      {...rootProps}
      className={joinClassNames("disclosure", className)}
      data-content-padding={contentPadding}
      data-summary-size={summarySize}
      data-state={open ? "open" : "closed"}
    >
      <div className={joinClassNames("disclosureSummaryRow", summaryRowClassName)}>
        <button
          type="button"
          className={joinClassNames("disclosureSummary", summaryClassName)}
          aria-controls={resolvedDetailsId}
          aria-expanded={open}
          aria-label={summaryAriaLabel}
          onClick={() => onOpenChange(!open)}
        >
          {summary}
          <ChevronRight className="disclosureChevron" size={14} aria-hidden="true" />
        </button>
        {summaryActions}
      </div>
      {open ? (
        <div id={resolvedDetailsId} className={joinClassNames("disclosureDetails", detailsClassName)}>
          {children}
        </div>
      ) : null}
    </div>
  );
}
