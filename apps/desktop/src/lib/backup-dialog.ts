export enum BackupDialogLeadingAction {
  Back = "back",
  Disconnect = "disconnect",
}

export function backupDialogLeadingAction(hasActiveCategory: boolean, configured: boolean): BackupDialogLeadingAction | null {
  if (hasActiveCategory) return BackupDialogLeadingAction.Back;
  return configured ? BackupDialogLeadingAction.Disconnect : null;
}
