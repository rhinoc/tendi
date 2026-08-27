export type SkillActionId =
  | "open-editor"
  | "locations"
  | "update"
  | "reveal"
  | "copy-path"
  | "visibility"
  | "backup"
  | "wrapper"
  | "delete";

export const skillActionIds = ({ selectionCount, backupConfigured }: { selectionCount: number; backupConfigured: boolean }): SkillActionId[] => {
  if (selectionCount === 0) return [];
  const actionIds: SkillActionId[] = selectionCount === 1
    ? ["open-editor", "locations", "update", "reveal", "copy-path"]
    : ["locations", "update"];
  actionIds.push("visibility");
  if (backupConfigured) actionIds.push("backup");
  actionIds.push("wrapper", "delete");
  return actionIds;
};
