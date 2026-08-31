export function shouldShowSessionListLocator({
  hasActiveSession,
  detailCollapsed,
  activeSessionInListViewport,
}: {
  hasActiveSession: boolean;
  detailCollapsed: boolean;
  activeSessionInListViewport: boolean | null;
}) {
  return hasActiveSession
    && !detailCollapsed
    && activeSessionInListViewport === false;
}

export enum SessionListLocationPlan {
  Scroll = "scroll",
  Page = "page",
  Reveal = "reveal",
  Missing = "missing",
}

export function planSessionListLocation({
  targetRowId,
  currentPageRowIds,
  currentResultRowIds,
  allRowIds,
}: {
  targetRowId: string;
  currentPageRowIds: readonly string[];
  currentResultRowIds: readonly string[];
  allRowIds: readonly string[];
}): SessionListLocationPlan {
  if (currentPageRowIds.includes(targetRowId)) return SessionListLocationPlan.Scroll;
  if (currentResultRowIds.includes(targetRowId)) return SessionListLocationPlan.Page;
  if (allRowIds.includes(targetRowId)) return SessionListLocationPlan.Reveal;
  return SessionListLocationPlan.Missing;
}
