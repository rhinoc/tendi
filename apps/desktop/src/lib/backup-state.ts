export enum BackupConfigurationState {
  Loading = "loading",
  Configured = "configured",
  Unconfigured = "unconfigured",
}

export function backupConfigurationState(response: { config: unknown } | null): BackupConfigurationState {
  if (response === null) return BackupConfigurationState.Loading;
  return response.config ? BackupConfigurationState.Configured : BackupConfigurationState.Unconfigured;
}
