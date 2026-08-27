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
