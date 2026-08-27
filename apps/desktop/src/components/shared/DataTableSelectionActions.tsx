import { Fragment, useLayoutEffect, useRef, useState, type ReactNode } from "react";

import { MoreActionsButton } from "./MoreActionsButton.tsx";
import { RowActionsMenu } from "./RowActionsMenu.tsx";
import { visibleSelectionActionCount } from "../../lib/table-selection-actions.ts";

export type DataTableSelectionActionDefinition = {
  id: string;
  direct: ReactNode;
  menu: ReactNode;
  measure: ReactNode;
  separatorBefore?: boolean;
};

export function renderDataTableSelectionMenu(actions: DataTableSelectionActionDefinition[]) {
  return (
    <>
      {actions.map((action) => (
        <Fragment key={action.id}>
          {action.separatorBefore ? <div className="skillMenuSeparator" role="separator" /> : null}
          {action.menu}
        </Fragment>
      ))}
    </>
  );
}

export function DataTableSelectionActions({
  actions,
  ariaLabel = "More selected actions",
}: {
  actions: DataTableSelectionActionDefinition[];
  ariaLabel?: string;
}) {
  const containerRef = useRef<HTMLDivElement>(null);
  const measureRef = useRef<HTMLDivElement>(null);
  const [visibleCount, setVisibleCount] = useState(actions.length);

  useLayoutEffect(() => {
    const container = containerRef.current;
    const measure = measureRef.current;
    if (!container || !measure) return undefined;
    const updateVisibleCount = () => {
      const availableWidth = container.clientWidth;
      const gap = Number.parseFloat(getComputedStyle(measure).gap) || 4;
      const widths = Array.from(measure.querySelectorAll<HTMLElement>("[data-selection-action-measure]"))
        .map((element) => element.getBoundingClientRect().width);
      const overflowWidth = measure.querySelector<HTMLElement>("[data-selection-actions-overflow-measure]")?.getBoundingClientRect().width ?? 0;
      const nextVisibleCount = visibleSelectionActionCount(availableWidth, widths, overflowWidth, gap);
      setVisibleCount((current) => current === nextVisibleCount ? current : nextVisibleCount);
    };
    if (typeof ResizeObserver === "undefined") {
      updateVisibleCount();
      return undefined;
    }
    const observer = new ResizeObserver(updateVisibleCount);
    observer.observe(container);
    observer.observe(measure);
    updateVisibleCount();
    return () => observer.disconnect();
  }, [actions]);

  if (actions.length === 0) return null;
  const visibleActions = actions.slice(0, visibleCount);
  const overflowActions = actions.slice(visibleCount);
  return (
    <div ref={containerRef} className="selectionActionsContent">
      <div className="selectionActionsVisible">
        {visibleActions.map((action) => <span className="selectionAction" key={action.id}>{action.direct}</span>)}
        {overflowActions.length > 0 ? (
          <RowActionsMenu ariaLabel={ariaLabel}>
            {renderDataTableSelectionMenu(overflowActions)}
          </RowActionsMenu>
        ) : null}
      </div>
      <div ref={measureRef} className="selectionActionsMeasure" aria-hidden="true">
        {actions.map((action) => <button data-selection-action-measure key={action.id}>{action.measure}</button>)}
        <MoreActionsButton data-selection-actions-overflow-measure aria-label={ariaLabel} />
      </div>
    </div>
  );
}
