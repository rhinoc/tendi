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

export type NormalizedSkillPath = NonNullable<SkillLike["paths"]>[number] & {
  path: string;
  root: string;
  scope: string;
  agent: string;
  install_target: string;
  source_kind: string;
};

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
  installTargets: string[];
  updateStatus: string;
  ctime?: string;
  mtime?: string;
  paths: NormalizedSkillPath[];
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
  const summary = `${skill.source ?? ""}`.toLowerCase();
  const pathKinds = (skill.paths ?? []).map((path) => `${path.source_kind ?? ""}`.toLowerCase());
  const pathValues = (skill.paths ?? []).flatMap((path) => [path.path, path.root, path.source]).map((path) => `${path ?? ""}`.toLowerCase());
  return (
    !isPluginSkillSource(skill) && (
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

export function normalizeSkillVisibility(value: unknown): SkillVisibility | undefined {
  const normalized = `${value ?? ""}`.trim().toLowerCase();
  if (normalized === "auto") return SkillVisibility.Auto;
  if (normalized === "manual") return SkillVisibility.Manual;
  if (normalized === "off") return SkillVisibility.Off;
  if (normalized === "mixed") return SkillVisibility.Mixed;
  return undefined;
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
  if (kind === "local") return "Local";
  return "";
}

export function normalizeSkill(skill: Record<string, unknown>): NormalizedSkill | undefined {
  const name = typeof skill.name === "string" && skill.name.trim() ? skill.name.trim() : undefined;
  if (
    !name
    || !Array.isArray(skill.agents)
    || skill.agents.length === 0
    || !Array.isArray(skill.tags)
    || !Array.isArray(skill.dependencies)
    || !Array.isArray(skill.dependents)
    || !Array.isArray(skill.paths)
    || !Array.isArray(skill.install_targets)
    || skill.install_targets.length === 0
    || typeof skill.visibility !== "string"
    || typeof skill.source_summary !== "string"
    || typeof skill.update_status !== "string"
    || typeof skill.is_system !== "boolean"
  ) return undefined;
  const rawAgents = skill.agents;
  const agents = rawAgents
    .filter((agent): agent is string => typeof agent === "string" && agent.trim().length > 0)
    .map((agent) => friendlyAgent(agent));
  if (agents.length !== rawAgents.length) return undefined;
  const uniqueAgents = [...new Set(agents)];
  const tags = skill.tags.filter((tag): tag is string => typeof tag === "string");
  const dependencies = skill.dependencies.filter((dependency): dependency is string => typeof dependency === "string");
  const dependents = skill.dependents.filter((dependent): dependent is string => typeof dependent === "string");
  if (
    tags.length !== skill.tags.length
    || dependencies.length !== skill.dependencies.length
    || dependents.length !== skill.dependents.length
  ) return undefined;
  const visibility = normalizeSkillVisibility(skill.visibility);
  if (!visibility) return undefined;
  const isSystem = skill.is_system;
  const source = skill.source_summary;
  const paths = skill.paths.flatMap<NormalizedSkillPath>((value) => {
    if (!value || typeof value !== "object" || Array.isArray(value)) return [];
    const path = value as Record<string, unknown>;
    const requiredPathFields = ["path", "root", "scope", "agent", "install_target", "source_kind"];
    if (requiredPathFields.some((field) => typeof path[field] !== "string" || !(path[field] as string).trim())) return [];
    if ((path.source_kind as string).trim().toLowerCase() === "unknown") return [];
    return [{
      ...path,
      path: path.path as string,
      root: path.root as string,
      scope: path.scope as string,
      agent: path.agent as string,
      install_target: path.install_target as string,
      source_kind: path.source_kind as string,
    } as NormalizedSkillPath];
  });
  if (paths.length !== skill.paths.length || paths.length === 0) return undefined;
  const installTargets = skill.install_targets.filter((target): target is string => typeof target === "string" && target.trim().length > 0);
  if (installTargets.length !== skill.install_targets.length) return undefined;
  const tone = statusTone({ ...(skill as SkillLike), source, visibility, isSystem } as SkillLike & { visibility?: SkillVisibility; isSystem?: boolean });
  return {
    id: name,
    section: skillSection({ ...(skill as SkillLike & { visibility?: SkillVisibility | string; statusTone?: string; isSystem?: boolean }), source, paths, visibility, statusTone: tone, isSystem }),
    name,
    description: typeof skill.description === "string" ? skill.description : "",
    tags,
    dependencies,
    dependents,
    isWrapper: tags.includes("wrapper"),
    agents: uniqueAgents,
    visibility,
    isSystem,
    statusTone: tone,
    source,
    installTargets,
    updateStatus: skill.update_status,
    ctime: typeof skill.ctime === "string" ? skill.ctime : undefined,
    mtime: typeof skill.mtime === "string" ? skill.mtime : undefined,
    paths,
    meta: skill.update_status,
  };
}

export function primarySkillPath(skill: SkillLike): string | null {
  return skill.paths?.find((path) => path.path)?.path ?? null;
}

export function skillTargets(skill: SkillLike & { name?: string }) {
  return (skill.paths ?? [])
    .filter((path): path is NonNullable<SkillLike["paths"]>[number] & { path: string; install_target: string; agent: string } => (
      typeof path.path === "string" && path.path.length > 0
      && typeof path.install_target === "string" && path.install_target.length > 0
      && typeof path.agent === "string" && path.agent.length > 0
    ))
    .map((path) => ({
      id: path.install_target,
      agent: path.agent,
      label: targetLabel(path),
      path: path.path,
    }));
}

export function targetLabel(target: NonNullable<SkillLike["paths"]>[number]): string {
  const agent = target.install_target?.split(":")[0] ?? "";
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
        meta: update.status,
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
      meta: prior.meta,
      statusTone: skill.statusTone === "muted" ? skill.statusTone : "warn",
    };
  });
}

export function replaceSkillReportPreservingUpdates<T extends { skills: NormalizedSkill[] }>(
  previous: T,
  next: T,
  clearNames: string[] = [],
): T {
  return clearSkillUpdateAvailability(
    {
      ...next,
      skills: mergeSkillListPreservingUpdates(previous.skills, next.skills),
    },
    clearNames,
  );
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
