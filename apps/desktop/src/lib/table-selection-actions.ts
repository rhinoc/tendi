export enum TableSelectionActionId {
  OpenEditor = "open-editor",
  Copy = "copy",
  CopyPath = "copy-path",
  Reveal = "reveal",
  Edit = "edit",
  Enable = "enable",
  Disable = "disable",
  Delete = "delete",
}

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
    [TableSelectionActionId.OpenEditor, TableSelectionActionId.Reveal, TableSelectionActionId.CopyPath, TableSelectionActionId.Enable, TableSelectionActionId.Disable, TableSelectionActionId.Delete],
    [TableSelectionActionId.Enable, TableSelectionActionId.Disable, TableSelectionActionId.Delete],
  );
}

export function mcpSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(
    selectionCount,
    [TableSelectionActionId.Enable, TableSelectionActionId.Disable, TableSelectionActionId.OpenEditor, TableSelectionActionId.Reveal, TableSelectionActionId.CopyPath],
    [TableSelectionActionId.Enable, TableSelectionActionId.Disable],
  );
}

export function promptSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(selectionCount, [TableSelectionActionId.Copy, TableSelectionActionId.Edit, TableSelectionActionId.Delete], [TableSelectionActionId.Delete]);
}

export function ruleSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(selectionCount, [TableSelectionActionId.OpenEditor, TableSelectionActionId.Reveal, TableSelectionActionId.CopyPath, TableSelectionActionId.Delete], [TableSelectionActionId.Delete]);
}

export function configSelectionActionIds(selectionCount: number) {
  return actionIdsForCount(selectionCount, [TableSelectionActionId.OpenEditor, TableSelectionActionId.Reveal, TableSelectionActionId.CopyPath, TableSelectionActionId.Delete], [TableSelectionActionId.Delete]);
}
