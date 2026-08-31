import { SkillOperationStatus } from "./skill-status.ts";
import type { AvailableSkill, SkillAddPlan } from "./skills.ts";

export function shouldShowSkillQuickSelect(availableCount: number, existingCount: number) {
  return availableCount > 0 && existingCount > 0;
}

function normalizeSkillOperationStatus(status: unknown): SkillOperationStatus | undefined {
  if (status === SkillOperationStatus.Planned || status === SkillOperationStatus.Ready || status === SkillOperationStatus.AlreadyExists || status === SkillOperationStatus.Replace || status === SkillOperationStatus.AlreadyInstalled) {
    return status as SkillOperationStatus;
  }
  return undefined;
}

export function normalizeSkillAddPlan(plan: SkillAddPlan | null | undefined): SkillAddPlan | null {
  if (!plan) return null;
  if (
    !Array.isArray(plan.available)
    || !Array.isArray(plan.selected)
    || !Array.isArray(plan.operations)
    || typeof plan.source !== "string"
    || !plan.source.trim()
    || typeof plan.source_kind !== "string"
    || !plan.source_kind.trim()
    || typeof plan.target !== "string"
    || !plan.target.trim()
  ) return null;
  const normalizeAvailable = (skills: AvailableSkill[]) => skills.flatMap((skill) => (
    typeof skill.name === "string"
      && skill.name.trim()
      && typeof skill.relative_path === "string"
      && Array.isArray(skill.dependencies)
      ? [{
        ...skill,
        name: skill.name.trim(),
        relative_path: skill.relative_path.trim(),
        dependencies: skill.dependencies.filter((dependency): dependency is string => typeof dependency === "string"),
      }]
      : []
  ));
  const available = normalizeAvailable(plan.available);
  const selected = normalizeAvailable(plan.selected);
  const operations = plan.operations.flatMap((operation) => {
    const status = normalizeSkillOperationStatus(operation.status);
    return typeof operation.name === "string" && operation.name.trim() && status
      ? [{ ...operation, name: operation.name.trim(), status }]
      : [];
  });
  return {
    ...plan,
    source: plan.source.trim(),
    source_kind: plan.source_kind.trim(),
    target: plan.target.trim(),
    available,
    selected,
    operations,
  };
}

export enum SkillSourcePageKind {
  Recommended = "recommended",
  Matches = "matches",
}

export type SkillSourcePageSnapshot<T> = {
  kind: SkillSourcePageKind;
  source: string;
  marketplaceResults: T[];
};

export function captureSkillSourcePage<T>(source: string, marketplaceResults: T[]): SkillSourcePageSnapshot<T> {
  return {
    kind: source.trim() ? SkillSourcePageKind.Matches : SkillSourcePageKind.Recommended,
    source,
    marketplaceResults,
  };
}

export function restoreSkillSourcePage<T>(page: SkillSourcePageSnapshot<T>) {
  const source = page.kind === SkillSourcePageKind.Recommended ? "" : page.source;
  return {
    source,
    marketplaceQuery: source,
    marketplaceResults: page.marketplaceResults,
  };
}

export function skillSourceErrorMessage(error: unknown) {
  const message = error instanceof Error ? error.message : String(error);
  if (/git clone failed/i.test(message) && /repository not found/i.test(message)) {
    return "Repository not found.";
  }
  return message.replace(/^DaemonCommandError:\s*/i, "");
}

export function isSkillSourceActionReady(source: string, isDirectSource: boolean, target: string) {
  return Boolean(source.trim()) && (!isDirectSource || Boolean(target.trim()));
}

export function resolveSkillInstallTarget<T extends { id: string }>(target: string, options: T[]) {
  return options.some((option) => option.id === target) ? target : options[0]?.id ?? "";
}
