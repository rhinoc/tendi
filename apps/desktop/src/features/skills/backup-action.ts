export type BackupStatusRecord = {
  skillPath: string;
  state: "backed-up" | "pending" | "needs-attention" | "excluded" | "not-backed-up" | "unmanaged" | string;
  reason?: string | null;
};

type SkillWithPaths = {
  paths: Array<{ path?: string | null }>;
};

export function backupStatusForSkill(skill: SkillWithPaths, statuses: Map<string, BackupStatusRecord>): BackupStatusRecord {
  const entries = skill.paths
    .map((path) => path.path ? statuses.get(path.path) : undefined)
    .filter((status): status is BackupStatusRecord => Boolean(status));
  if (entries.length === 0) return { skillPath: "", state: "unmanaged" };
  if (entries.some((status) => status.state === "needs-attention")) return entries.find((status) => status.state === "needs-attention")!;
  if (entries.some((status) => status.state === "pending")) return entries.find((status) => status.state === "pending")!;
  if (entries.some((status) => status.state === "unmanaged")) return entries.find((status) => status.state === "unmanaged")!;
  if (entries.every((status) => status.state === "backed-up")) return entries[0];
  if (entries.every((status) => status.state === "excluded")) return entries[0];
  if (entries.some((status) => status.state === "not-backed-up")) return entries.find((status) => status.state === "not-backed-up")!;
  return entries[0];
}

export function backupSkillsForSelection<T>(skills: T[], getStatus: (skill: T) => BackupStatusRecord, configured: boolean): T[] {
  if (!configured) return [];
  return skills.filter((skill) => {
    const status = getStatus(skill);
    return status.state === "unmanaged" && Boolean(status.skillPath);
  });
}
