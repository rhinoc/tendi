export type BackupDialogLeadingAction = "back" | "disconnect" | null;

export function backupDialogLeadingAction(hasActiveCategory: boolean, configured: boolean): BackupDialogLeadingAction {
  if (hasActiveCategory) return "back";
  return configured ? "disconnect" : null;
}
