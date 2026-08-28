export type BackupConfigurationState = "loading" | "configured" | "unconfigured";

export function backupConfigurationState(response: { config: unknown } | null): BackupConfigurationState {
  if (response === null) return "loading";
  return response.config ? "configured" : "unconfigured";
}
