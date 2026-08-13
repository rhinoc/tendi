import { titleValue } from "./strings.ts";
import { friendlyAgent } from "./agents.ts";
import {
  isWebSource,
  openSource,
  parseRemoteSource,
  pathLooksLikePluginCache,
  skillSourceDetails,
  sourceIconDetails,
  sourceOpenUrl,
  sourceLocalPath,
  type SkillLike,
  type SourceDetails,
} from "./sources.ts";
import { TauriCommand, safeInvoke } from "./tauri.ts";

export enum SkillVisibility {
  Auto = "Auto",
  Manual = "Manual",
  Off = "Off",
  Mixed = "Mixed",
}

export const editableSkillVisibilities = [
  SkillVisibility.Auto,
  SkillVisibility.Manual,
  SkillVisibility.Off,
] as const;

export const allSkillVisibilities = [
  SkillVisibility.Auto,
  SkillVisibility.Manual,
  SkillVisibility.Off,
  SkillVisibility.Mixed,
] as const;

export type NormalizedSkill = {
  id: string;
  section: string;
  name: string;
  description: string;
  tags: string[];
  dependencies: string[];
  dependents: string[];
  isWrapper: boolean;
  agents: string[];
  visibility: SkillVisibility;
  isSystem: boolean;
  statusTone: string;
  source: string;
  installTargets?: string[];
  updateStatus: string;
  paths: NonNullable<SkillLike["paths"]>;
  meta?: string;
  [key: string]: unknown;
};

export function isPluginSkillSource(skill: SkillLike): boolean {
  const paths = skill.paths ?? [];
  return paths.some((path) => (
    `${path.scope ?? ""}`.toLowerCase() === "plugin" ||
    pathLooksLikePluginCache(path.path) ||
    pathLooksLikePluginCache(path.root) ||
    pathLooksLikePluginCache(path.source) ||
    pathLooksLikePluginCache(path.install_target)
  ));
}

export function isSystemSkillSource(skill: SkillLike, source: SourceDetails = skillSourceDetails(skill)): boolean {
  const kind = `${source.kind ?? ""}`.toLowerCase();
  const value = `${source.value ?? ""}`.toLowerCase();
  const summary = `${skill.source_summary ?? skill.source ?? ""}`.toLowerCase();
  const pathKinds = (skill.paths ?? []).map((path) => `${path.source_kind ?? ""}`.toLowerCase());
  const pathValues = (skill.paths ?? []).flatMap((path) => [path.path, path.root, path.source]).map((path) => `${path ?? ""}`.toLowerCase());
  return (
    !isPluginSkillSource(skill) && (
      skill.is_system === true ||
      skill.isSystem === true ||
      skill.source === "system" ||
      summary === "system" ||
      kind.includes("system") ||
      value === "system" ||
      pathKinds.some((pathKind) => pathKind.includes("system")) ||
      pathValues.some((pathValue) => pathValue.includes("/.system/") || pathValue.includes("/cache/"))
    )
  );
}

export function isReadOnlySkillSource(skill: SkillLike): boolean {
  return isSystemSkillSource(skill) || isPluginSkillSource(skill);
}

export function normalizeSkillVisibility(value: unknown): SkillVisibility {
  const normalized = titleValue(value ?? SkillVisibility.Auto);
  if (normalized === SkillVisibility.Manual) return SkillVisibility.Manual;
  if (normalized === SkillVisibility.Off) return SkillVisibility.Off;
  if (normalized === SkillVisibility.Mixed) return SkillVisibility.Mixed;
  return SkillVisibility.Auto;
}

export function statusTone(skill: { statusTone?: string; visibility?: SkillVisibility | string; isSystem?: boolean } & SkillLike): string {
  if (isReadOnlySkillSource(skill)) return "muted";
  if (skill.statusTone) return skill.statusTone;
  if (skill.visibility === SkillVisibility.Mixed) return "warn";
  if (skill.visibility === SkillVisibility.Manual) return "warn";
  return "ok";
}

export function isSkillVisibilityEditable(skill: { visibility?: SkillVisibility | string }): boolean {
  return skill.visibility !== SkillVisibility.Mixed;
}

export function isSkillRowSelectable(skill: { visibility?: SkillVisibility | string }): boolean {
  return isSkillVisibilityEditable(skill);
}

export function isSkillSelectable(skill: SkillLike & { visibility?: SkillVisibility | string }): boolean {
  return !isReadOnlySkillSource(skill) && skill.visibility !== SkillVisibility.Mixed;
}

export function skillSection(skill: SkillLike & { visibility?: SkillVisibility | string; statusTone?: string; isSystem?: boolean }): string {
  const source = skillSourceDetails(skill);
  const kind = `${source.kind ?? ""}`.toLowerCase();
  const value = `${source.value ?? ""}`.toLowerCase();
  if (isPluginSkillSource(skill)) return "Plugin";
  if (isSystemSkillSource(skill, source)) return "System";
  if (
    kind &&
    kind !== "local" &&
    kind !== "unknown" &&
    !kind.includes("system")
  ) return "Remote";
  if (isWebSource(value) || parseRemoteSource(value)) return "Remote";
  return "Local";
}

export function normalizeSkill(skill: Record<string, unknown>, index: number): NormalizedSkill {
  const rawAgents = (skill.agents as string[] | undefined)?.length
    ? (skill.agents as string[])
    : ((skill.paths as SkillLike["paths"]) ?? []).map((path) => path.agent);
  const agents = rawAgents
    .filter(Boolean)
    .map((agent) => friendlyAgent(agent));
  const uniqueAgents = [...new Set(agents.length ? agents : (skill.agents as string[] | undefined) ?? ["Codex"])];
  const tags = (skill.tags as string[] | undefined) ?? [];
  const dependencies = (skill.dependencies as string[] | undefined) ?? [];
  const dependents = (skill.dependents as string[] | undefined) ?? [];
  const visibility = normalizeSkillVisibility(skill.visibility);
  const isSystem = skill.is_system === true || skill.isSystem === true;
  const tone = statusTone({ ...(skill as SkillLike), visibility, isSystem } as SkillLike & { visibility?: SkillVisibility; isSystem?: boolean });
  return {
    id: `${skill.id ?? skill.name ?? `skill-${index}`}`,
    section: skillSection({ ...(skill as SkillLike & { visibility?: SkillVisibility | string; statusTone?: string; isSystem?: boolean }), visibility, statusTone: tone, isSystem }),
    name: `${skill.name ?? `skill-${index}`}`,
    description: `${skill.description ?? skill.summary ?? "No description"}`,
    tags,
    dependencies,
    dependents,
    isWrapper: tags.includes("wrapper"),
    agents: uniqueAgents,
    visibility,
    isSystem,
    statusTone: tone,
    source: `${skill.source_summary ?? skill.source ?? "local"}`,
    installTargets: (skill.install_targets as string[] | undefined) ?? (skill.installTargets as string[] | undefined) ?? [],
    updateStatus: `${skill.update_status ?? skill.updateStatus ?? "local"}`,
    paths: (skill.paths as SkillLike["paths"]) ?? [],
    meta: `${skill.update_status ?? skill.updateStatus ?? ""}`,
  };
}

export function primarySkillPath(skill: SkillLike): string | null {
  return skill.paths?.find((path) => path.path)?.path ?? null;
}

export function skillTargets(skill: SkillLike & { name?: string }) {
  return (skill.paths ?? [])
    .filter((path) => path.path)
    .map((path, index) => ({
      id: `${path.install_target ?? path.path}-${index}`,
      agent: path.install_target?.split(":")[0] ?? path.agent ?? "target",
      label: targetLabel(path),
      path: path.path ?? "",
    }));
}

export function targetLabel(target: NonNullable<SkillLike["paths"]>[number]): string {
  const agent = target.install_target?.split(":")[0] ?? target.agent ?? "target";
  const scope = target.scope ? ` ${target.scope}` : "";
  return `${targetAgentLabel(agent)}${scope}`;
}

export function targetAgentLabel(agent: unknown): string {
  const key = `${agent ?? ""}`.toLowerCase().replace(/[^a-z0-9]+/g, "");
  if (key === "shared") return "Shared";
  return friendlyAgent(agent);
}

export function localSkillSourcePath(skill: SkillLike, source: SourceDetails = skillSourceDetails(skill)): string | null {
  const value = `${source.value ?? ""}`.trim();
  const normalizedValue = value.toLowerCase();
  return (
    primarySkillPath(skill) ??
    (value && normalizedValue !== "local" && normalizedValue !== "unknown" ? sourceLocalPath(value) : null)
  );
}

export function skillSourceAction(
  skill: { name: string; section?: string } & SkillLike,
  source: SourceDetails = skillSourceDetails(skill),
) {
  const sourceIcon = sourceIconDetails(source);
  const sourceUrl = sourceOpenUrl(source.value, source.kind, source.relativePath);
  if (sourceUrl) {
    return {
      icon: sourceIcon.icon,
      ariaLabel: `Open ${sourceIcon.label} source for ${skill.name}`,
      title: `Open ${sourceIcon.label} source`,
      onClick: () => openSource(source.value, source.kind, source.relativePath),
    };
  }
  if (skill.section === "Local") {
    const path = localSkillSourcePath(skill, source);
    if (path) {
      return {
        icon: sourceIcon.icon,
        ariaLabel: `Reveal ${sourceIcon.label} source for ${skill.name} in Finder`,
        title: `Reveal ${sourceIcon.label} source in Finder`,
        onClick: () => safeInvoke(TauriCommand.RevealInFinder, { path }),
      };
    }
  }
  return null;
}

export enum SkillChangeCommand {
  Set = "skills_set",
  UpdateMany = "skills_update_many",
  DeleteMany = "skills_delete_many",
  Wrap = "skills_wrap",
}

export function skillChangeActionLabel(command: SkillChangeCommand): string {
  if (command === SkillChangeCommand.DeleteMany) return "Delete";
  if (command === SkillChangeCommand.UpdateMany) return "Update";
  if (command === SkillChangeCommand.Wrap) return "Create";
  return "Apply";
}

export function applySkillUpdateReports<T extends { skills: NormalizedSkill[] }>(
  report: T,
  updates: Array<{ name: string; status: string }> | null | undefined,
): T {
  if (!Array.isArray(updates)) return report;
  const byName = new Map(updates.map((update) => [update.name, update]));
  return {
    ...report,
    skills: report.skills.map((skill) => {
      const update = byName.get(skill.name);
      if (!update) return skill;
      return {
        ...skill,
        updateStatus: update.status,
        meta: update.status === "update-available" ? "update" : update.status,
        statusTone: update.status === "update-available" ? "warn" : skill.statusTone,
      };
    }),
  };
}

/** skills_list does not carry update-check results; keep prior update-available badges across refreshes. */
export function mergeSkillListPreservingUpdates(
  previous: NormalizedSkill[],
  next: NormalizedSkill[],
): NormalizedSkill[] {
  const previousByName = new Map(previous.map((skill) => [skill.name, skill]));
  return next.map((skill) => {
    const prior = previousByName.get(skill.name);
    if (!prior || prior.updateStatus !== "update-available") return skill;
    if (skill.updateStatus === "update-available") return skill;
    return {
      ...skill,
      updateStatus: prior.updateStatus,
      meta: prior.meta || "update",
      statusTone: skill.statusTone === "muted" ? skill.statusTone : "warn",
    };
  });
}

export function clearSkillUpdateAvailability<T extends { skills: NormalizedSkill[] }>(
  report: T,
  names: string[],
): T {
  const selected = new Set(names);
  return {
    ...report,
    skills: report.skills.map((skill) => {
      if (!selected.has(skill.name) || skill.updateStatus !== "update-available") return skill;
      return {
        ...skill,
        updateStatus: "up-to-date",
        meta: "up-to-date",
        statusTone: skill.statusTone === "warn" ? "ok" : skill.statusTone,
      };
    }),
  };
}

export function applyVisibilityState<T extends { skills: NormalizedSkill[] }>(
  report: T,
  names: string[],
  visibility: SkillVisibility,
): T {
  const selected = new Set(names);
  return {
    ...report,
    skills: report.skills.map((skill) => {
      if (!selected.has(skill.name)) return skill;
      const nextTone = skill.statusTone === "muted" ? "muted" : visibility === SkillVisibility.Manual ? "warn" : "ok";
      return {
        ...skill,
        visibility,
        statusTone: nextTone,
        section: skillSection({ ...skill, visibility, statusTone: nextTone }),
      };
    }),
  };
}
