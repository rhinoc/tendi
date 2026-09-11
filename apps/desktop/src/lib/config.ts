export type ConfigDisplayNameSource = {
  label?: string | null;
};

export function configDisplayName(config: ConfigDisplayNameSource | null | undefined): string {
  return config?.label || "Config file";
}
