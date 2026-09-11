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
    ? [SkillActionId.Visibility, SkillActionId.Update, SkillActionId.OpenEditor, SkillActionId.Reveal, SkillActionId.CopyPath, SkillActionId.Wrapper, SkillActionId.Locations]
    : [SkillActionId.Visibility, SkillActionId.Update, SkillActionId.Wrapper, SkillActionId.Locations];
  actionIds.push(SkillActionId.Delete);
  return actionIds;
};
