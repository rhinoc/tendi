export enum SkillActionId {
  OpenEditor = "open-editor",
  Locations = "locations",
  Update = "update",
  Reveal = "reveal",
  CopyPath = "copy-path",
  Visibility = "visibility",
  Wrapper = "wrapper",
  Delete = "delete",
}

export const skillActionIds = ({ selectionCount }: { selectionCount: number }): SkillActionId[] => {
  if (selectionCount === 0) return [];
  const actionIds: SkillActionId[] = selectionCount === 1
    ? [SkillActionId.OpenEditor, SkillActionId.Locations, SkillActionId.Update, SkillActionId.Reveal, SkillActionId.CopyPath]
    : [SkillActionId.Locations, SkillActionId.Update];
  actionIds.push(SkillActionId.Visibility);
  actionIds.push(SkillActionId.Wrapper, SkillActionId.Delete);
  return actionIds;
};
