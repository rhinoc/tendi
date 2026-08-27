export type TableSelectionActionId =
  | "open-editor"
  | "copy"
  | "copy-path"
  | "reveal"
  | "edit"
  | "enable"
  | "disable"
  | "delete";

export function visibleSelectionActionCount(
  availableWidth: number,
  actionWidths: readonly number[],
  overflowWidth: number,
  gap = 4,
) {
  let visibleCount = actionWidths.length;
  while (visibleCount > 0) {
    const hiddenCount = actionWidths.length - visibleCount;
    const actionsWidth = actionWidths.slice(0, visibleCount).reduce((sum, width) => sum + width, 0)
      + Math.max(0, visibleCount - 1) * gap;
    const totalWidth = actionsWidth + (hiddenCount > 0 ? gap + overflowWidth : 0);
    if (totalWidth <= availableWidth + 1) break;
    visibleCount -= 1;
  }
  return visibleCount;
}

function actionIdsForCount(
  selectionCount: number,
  single: readonly TableSelectionActionId[],
  multiple: readonly TableSelectionActionId[],
) {
  if (selectionCount === 0) return [];
  return [...(selectionCount === 1 ? single : multiple)];
}

export function hookSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(
    selectionCount,
    ["open-editor", "reveal", "copy-path", "enable", "disable", "delete"],
    ["enable", "disable", "delete"],
  );
}

export function mcpSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(
    selectionCount,
    ["enable", "disable", "open-editor", "reveal", "copy-path"],
    ["enable", "disable"],
  );
}

export function promptSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(selectionCount, ["copy", "edit", "delete"], ["delete"]);
}

export function ruleSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(selectionCount, ["open-editor", "reveal", "copy-path", "delete"], ["delete"]);
}

export function configSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(selectionCount, ["open-editor", "reveal", "copy-path", "delete"], ["delete"]);
}
