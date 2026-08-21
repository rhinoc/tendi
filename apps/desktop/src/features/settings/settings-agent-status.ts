import type { BundledSkillStatus, CliInstallStatus } from "../../lib/tauri.ts";

export type CodingAgentsAction = "install" | "repair" | "remove" | null;

export function isCodingAgentsInstalled(
  cliStatus: CliInstallStatus | null,
  bundledSkillStatus: BundledSkillStatus | null,
): boolean {
  if (!cliStatus || bundledSkillStatus?.current !== true) return false;
  return cliStatus.supported === false
    || (cliStatus.state === "installed" && cliStatus.pathConfigured);
}

export function codingAgentsAction(
  cliStatus: CliInstallStatus | null,
  bundledSkillStatus: BundledSkillStatus | null,
): CodingAgentsAction {
  if (!cliStatus || !bundledSkillStatus || cliStatus.state === "conflict") return null;
  if (cliStatus.state === "installed" && !cliStatus.pathConfigured) return "remove";
  if (bundledSkillStatus.installed && !bundledSkillStatus.current) return "repair";
  if (!bundledSkillStatus.current) return "install";
  if (cliStatus.state === "stale") return "repair";
  if (cliStatus.supported && !(cliStatus.state === "installed" && cliStatus.pathConfigured)) return "install";
  if (!cliStatus.supported || (cliStatus.state === "installed" && cliStatus.pathConfigured)) return "remove";
  return null;
}
