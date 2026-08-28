import type { SkillChangeCommand } from "./skills.ts";

export function skillChangeDescription(command: SkillChangeCommand | null): string {
  if (command === "skills_delete_many") return "Delete the selected skills from their installed locations.";
  if (command === "skills_update_many") return "Apply available updates for the selected skills.";
  if (command === "skills_set") return "Apply the selected visibility change.";
  return "Apply the selected skill change.";
}

export function skillChangeTitle(command: SkillChangeCommand | null): string {
  return command === "skills_delete_many" ? "Delete selected skills?" : "Confirm skill changes";
}

export function skillChangeActionLabel(command: SkillChangeCommand | null): string {
  if (command === "skills_delete_many") return "Delete skills";
  if (command === "skills_update_many") return "Apply updates";
  if (command === "skills_set") return "Apply visibility";
  if (command === "skills_wrap") return "Create skill";
  return "Apply changes";
}

export function skillChangeBusyLabel(command: SkillChangeCommand | null): string {
  if (command === "skills_delete_many") return "Deleting…";
  if (command === "skills_update_many") return "Updating…";
  if (command === "skills_wrap") return "Creating…";
  return "Applying…";
}

export const skillChangeLoadingCopy = {
  description: "Preparing skill change preview.",
  previewLabel: "Preparing update preview",
} as const;
