import type { RuntimeData } from "../lib/data.ts";
import {
  SkillVisibility,
  normalizeSkill,
  isReadOnlySkillSource,
  isSkillSelectable,
  skillTargets,
  type NormalizedSkill,
  type RawSkillRecord,
  type AvailableSkill,
  type SkillInstallResult,
  type SkillOperation,
} from "../lib/skills.ts";
import { SkillOperationStatus, SkillUpdateAvailability } from "../lib/skill-status.ts";
import { skillSection } from "../lib/skills.ts";
import type { SkillUpdateReport } from "../lib/skill-updates.ts";

export enum SkillInstallFilter {
  All = "all",
  New = "new",
  Existing = "existing",
}

export function mergeSkillRows(previous: readonly RawSkillRecord[], next: readonly RawSkillRecord[]): RawSkillRecord[] {
  const merged = [...previous];
  const indexes = new Map<string, number>();
  merged.forEach((row, index) => {
    if (typeof row.id === "string" && row.id) indexes.set(`id:${row.id}`, index);
  });
  for (const row of next) {
    const idKey = typeof row.id === "string" && row.id ? `id:${row.id}` : "";
    const index = idKey ? indexes.get(idKey) : undefined;
    if (index === undefined) {
      if (idKey) indexes.set(idKey, merged.length);
      merged.push(row);
    } else {
      merged[index] = row;
    }
  }
  return merged;
}

function sameValue(left: unknown, right: unknown): boolean {
  return JSON.stringify(left) === JSON.stringify(right);
}

function preserveSkillUpdate(previous: NormalizedSkill | undefined, next: NormalizedSkill): NormalizedSkill {
  if (!previous || previous.updateAvailability !== SkillUpdateAvailability.UpdateAvailable || next.updateAvailability === SkillUpdateAvailability.UpdateAvailable) return next;
  return {
    ...next,
    updateAvailability: previous.updateAvailability,
    statusTone: next.statusTone === "muted" ? next.statusTone : "warn",
  };
}

function reconcileSkills(current: readonly NormalizedSkill[], incoming: readonly NormalizedSkill[]): NormalizedSkill[] {
  const byId = new Map(current.map((skill) => [skill.id, skill]));
  const next = incoming.map((skill) => {
    const candidate = preserveSkillUpdate(byId.get(skill.id), skill);
    const previous = byId.get(skill.id);
    return previous && sameValue(previous, candidate) ? previous : candidate;
  });
  if (next.length === current.length && next.every((skill, index) => skill === current[index])) return current as NormalizedSkill[];
  return next;
}

function normalizedSkills(rows: readonly RawSkillRecord[]): NormalizedSkill[] {
  return rows.flatMap((row) => normalizeSkill(row) ?? []);
}

/** Replace the skill projection while retaining update badges and row references. */
export function applySkillSnapshot(data: RuntimeData, rows: readonly RawSkillRecord[]): RuntimeData {
  const skills = reconcileSkills(data.skills, normalizedSkills(rows));
  return skills === data.skills ? data : { ...data, skills };
}

/** Apply install/move/update results with one id index and one pass. */
export function applySkillPatch(
  data: RuntimeData,
  rows: readonly RawSkillRecord[],
  deleted: readonly string[] = [],
): RuntimeData {
  const deletedSet = new Set(deleted);
  const current = data.skills.filter((skill) => !deletedSet.has(skill.id));
  const byId = new Map(current.map((skill) => [skill.id, skill]));
  const patched = normalizedSkills(rows);
  const patchedById = new Map(patched.map((skill) => [skill.id, skill]));
  const emitted = new Set<string>();
  const next = current.map((skill) => {
    const update = patchedById.get(skill.id);
    if (!update) return skill;
    emitted.add(update.id);
    const preserved = preserveSkillUpdate(skill, update);
    return sameValue(skill, preserved) ? skill : preserved;
  });
  for (const skill of patched) {
    if (emitted.has(skill.id) || byId.has(skill.id)) continue;
    next.push(skill);
  }
  const reconciled = reconcileSkills([], next);
  return reconciled.length === data.skills.length && reconciled.every((skill, index) => skill === data.skills[index])
    ? data
    : { ...data, skills: reconciled };
}

export function applySkillUpdateReports(data: RuntimeData, updates: readonly SkillUpdateReport[]): RuntimeData {
  if (updates.length === 0) return data;
  const byId = new Map(updates.flatMap((update) => update.id ? [[update.id, update] as const] : []));
  let changed = false;
  const skills = data.skills.map((skill) => {
    const update = byId.get(skill.id);
    if (!update) return skill;
    const next = {
      ...skill,
      updateAvailability: update.status,
      statusTone: update.status === SkillUpdateAvailability.UpdateAvailable ? "warn" : skill.statusTone,
    };
    if (sameValue(skill, next)) return skill;
    changed = true;
    return next;
  });
  return changed ? { ...data, skills } : data;
}

export function clearSkillUpdateAvailability(data: RuntimeData, selectors: readonly string[]): RuntimeData {
  if (selectors.length === 0) return data;
  const selected = new Set(selectors);
  let changed = false;
  const skills = data.skills.map((skill) => {
    if (!selected.has(skill.id) || skill.updateAvailability !== SkillUpdateAvailability.UpdateAvailable) return skill;
    changed = true;
    return {
      ...skill,
      updateAvailability: SkillUpdateAvailability.UpToDate,
      statusTone: skill.statusTone === "warn" ? "ok" : skill.statusTone,
    };
  });
  return changed ? { ...data, skills } : data;
}

export function applySkillVisibility(data: RuntimeData, selectors: readonly string[], visibility: SkillVisibility): RuntimeData {
  if (selectors.length === 0) return data;
  const selected = new Set(selectors);
  let changed = false;
  const skills = data.skills.map((skill) => {
    if (!selected.has(skill.id)) return skill;
    const nextTone = skill.statusTone === "muted" ? "muted" : visibility === SkillVisibility.Manual ? "warn" : "ok";
    const next = {
      ...skill,
      visibility,
      statusTone: nextTone,
      section: skillSection({ ...skill, visibility, statusTone: nextTone }),
    };
    if (sameValue(skill, next)) return skill;
    changed = true;
    return next;
  });
  return changed ? { ...data, skills } : data;
}

export function isNewSkillOperationStatus(status: SkillOperationStatus | undefined): boolean {
  return !status || status === SkillOperationStatus.Planned || status === SkillOperationStatus.Ready;
}

export function isExistingSkillOperationStatus(status: SkillOperationStatus | undefined): boolean {
  return status === SkillOperationStatus.AlreadyInstalled
    || status === SkillOperationStatus.AlreadyExists
    || status === SkillOperationStatus.Replace;
}

export function isSelectableOperationStatus(status: SkillOperationStatus | undefined): boolean {
  return isNewSkillOperationStatus(status) || isExistingSkillOperationStatus(status);
}

export function isSkillInstallDependencyLocked(
  name: string,
  selectedRootNames: ReadonlySet<string>,
  dependencyReasonsByName: ReadonlyMap<string, readonly string[]>,
): boolean {
  return (dependencyReasonsByName.get(name)?.length ?? 0) > 0 && !selectedRootNames.has(name);
}

export function canToggleSkillInstallRoot(
  name: string,
  selectedNames: ReadonlySet<string>,
  selectedRootNames: ReadonlySet<string>,
  operationByName: ReadonlyMap<string, SkillOperation>,
  dependencyReasonsByName: ReadonlyMap<string, readonly string[]>,
): boolean {
  if (!isSelectableOperationStatus(operationByName.get(name)?.status)) return false;
  return !(selectedNames.has(name) && isSkillInstallDependencyLocked(name, selectedRootNames, dependencyReasonsByName));
}

export function skillInstallRootsForPreset<T extends { name: string }>(
  preset: SkillInstallFilter,
  selectableSkills: readonly T[],
  newSkills: readonly T[],
  existingSkills: readonly T[],
  selectableNames: ReadonlySet<string>,
): string[] {
  const skills = preset === SkillInstallFilter.All
    ? selectableSkills
    : preset === SkillInstallFilter.New
      ? newSkills
      : existingSkills;
  return skills.filter((skill) => selectableNames.has(skill.name)).map((skill) => skill.name);
}

export function canInstallSkillSelection(input: {
  resolvedTarget: string;
  source: string;
  hasPlan: boolean;
  selected: readonly string[];
  selectedHasExisting: boolean;
  replaceExisting: boolean;
  busy: boolean;
}): boolean {
  return Boolean(
    input.resolvedTarget
    && input.source.trim()
    && input.hasPlan
    && input.selected.length > 0
    && (!input.selectedHasExisting || input.replaceExisting)
    && !input.busy,
  );
}

export function installedSkillName(result: SkillInstallResult): string | null {
  const rows = result.updated ?? result.skills ?? [];
  const installedNames = new Set(rows.flatMap((skill) => typeof skill.name === "string" ? [skill.name] : []));
  return result.report.plan.selected
    .map((skill) => skill.name)
    .find((name) => installedNames.has(name)) ?? null;
}

export function suggestedWrapperName(selectedSkills: readonly { name: string }[]): string {
  const names = selectedSkills.map((skill) => skill.name).filter(Boolean);
  if (names.length === 0) return "wrapper";
  const prefixes = names
    .map((name) => name.split(/[-_]/)[0])
    .filter((prefix) => prefix.length > 1);
  return prefixes.length > 0 && prefixes.every((prefix) => prefix === prefixes[0]) ? prefixes[0] : "wrapper";
}

function childSkillSummary(selectedSkills: readonly { name: string }[]): string {
  const names = selectedSkills.map((skill) => skill.name).filter(Boolean);
  if (names.length === 0) return "the selected child skills";
  const visibleNames = names.slice(0, 4);
  const suffix = names.length > visibleNames.length ? `, and ${names.length - visibleNames.length} more` : "";
  return `${visibleNames.join(", ")}${suffix}`;
}

export function suggestedWrapperDescription(name: string, selectedSkills: readonly { name: string }[] = []): string {
  const domain = name.trim().replace(/[-_]+/g, " ");
  return domain ? `Use when the request is about ${domain} and matches one of these child skills: ${childSkillSummary(selectedSkills)}.` : "";
}

export function selectMovableSkills<T extends NormalizedSkill>(skills: readonly T[]): T[] {
  return skills.filter((skill) => !isReadOnlySkillSource(skill) && skillTargets(skill).length > 0);
}

export function selectSkillInstallTargetOptions<T extends { id: string; supportsGlobal: boolean }>(
  targetOptions: readonly T[],
  installedAgentKeys: readonly string[],
  agentIdentityKey: (agent: unknown) => string,
  isVisibleAgent: (agent: unknown) => boolean,
): T[] {
  const installed = new Set(installedAgentKeys);
  return targetOptions
    .map((option, index) => ({
      option,
      index,
      installed: installed.has(agentIdentityKey(option.id)),
    }))
    .filter(({ option }) => option.supportsGlobal && option.id !== "universal" && isVisibleAgent(option.id))
    .sort((left, right) => {
      if (left.option.id === "shared") return -1;
      if (right.option.id === "shared") return 1;
      if (left.installed !== right.installed) return left.installed ? -1 : 1;
      return left.index - right.index;
    })
    .map(({ option }) => option);
}

export function skillSelectionTargets(skills: readonly NormalizedSkill[]): {
  updateNames: string[];
  deletableNames: string[];
} {
  return {
    updateNames: skills
      .filter((skill) => skill.updateAvailability === SkillUpdateAvailability.UpdateAvailable)
      .map((skill) => skill.id),
    deletableNames: skills.filter(isSkillSelectable).map((skill) => skill.id),
  };
}

export function selectedExistingSkillOperation(
  operation: SkillOperation | undefined,
  selected: boolean,
): SkillOperation | undefined {
  if (!operation || !selected || operation.status !== SkillOperationStatus.AlreadyExists) return operation;
  return {
    ...operation,
    status: SkillOperationStatus.Replace,
    message: operation.message?.startsWith("target already exists:")
      ? operation.message.replace("target already exists:", "will replace existing target:")
      : operation.message,
  };
}

export function expandSkillDependencies(
  roots: readonly string[],
  dependencyByName: ReadonlyMap<string, readonly string[]>,
  selectableNames: ReadonlySet<string>,
): string[] {
  const expanded = new Set<string>();
  const pending = roots.filter((name) => selectableNames.has(name));
  while (pending.length > 0) {
    const name = pending.pop();
    if (!name || expanded.has(name) || !selectableNames.has(name)) continue;
    expanded.add(name);
    for (const dependency of dependencyByName.get(name) ?? []) {
      if (!expanded.has(dependency)) pending.push(dependency);
    }
  }
  return [...expanded].sort((left, right) => left.localeCompare(right));
}

export function partitionSkillOperations<T extends { name: string }>(
  skills: readonly T[],
  operationByName: ReadonlyMap<string, { status?: string }>,
): { newSkills: T[]; existingSkills: T[] } {
  const newSkills: T[] = [];
  const existingSkills: T[] = [];
  for (const skill of skills) {
    const status = operationByName.get(skill.name)?.status;
    if (status === "already-installed" || status === "already-exists" || status === "replace") existingSkills.push(skill);
    else newSkills.push(skill);
  }
  return { newSkills, existingSkills };
}

export type SkillInstallViewModel = {
  selectableSkills: AvailableSkill[];
  selectableNameSet: Set<string>;
  selected: string[];
  selectedSet: Set<string>;
  selectedRootSet: Set<string>;
  selectedHasExisting: boolean;
  operationByName: Map<string, SkillOperation>;
  dependencyReasonsByName: Map<string, string[]>;
  newSkills: AvailableSkill[];
  existingSkills: AvailableSkill[];
  visibleAvailableSkills: AvailableSkill[];
  searchMatches: string[];
  searchMatchSet: Set<string>;
};

export type SkillListViewModel = {
  visibleSkills: NormalizedSkill[];
  selectedSkills: NormalizedSkill[];
};

type SkillListViewCacheEntry = {
  query: string;
  selectedIds: readonly string[];
  view: SkillListViewModel;
};

const skillListViewCache = new WeakMap<readonly NormalizedSkill[], SkillListViewCacheEntry[]>();

export function selectSkillListView(
  skills: readonly NormalizedSkill[],
  query: string,
  selectedIds: readonly string[],
): SkillListViewModel {
  const normalizedQuery = query.trim().toLowerCase();
  const cached = skillListViewCache.get(skills)?.find((entry) => entry.query === normalizedQuery && entry.selectedIds === selectedIds);
  if (cached) return cached.view;
  const visibleSkills = normalizedQuery
    ? skills.filter((skill) => `${skill.name} ${skill.description}`.toLowerCase().includes(normalizedQuery))
    : skills as NormalizedSkill[];
  const selected = new Set(selectedIds);
  const view = {
    visibleSkills,
    selectedSkills: skills.filter((skill) => selected.has(skill.id)),
  };
  const entries = skillListViewCache.get(skills) ?? [];
  entries.push({ query: normalizedQuery, selectedIds, view });
  skillListViewCache.set(skills, entries);
  return view;
}

function skillSearchText(skill: AvailableSkill): string {
  return [skill.name, skill.description, skill.relative_path, ...skill.dependencies].filter(Boolean).join(" ").toLowerCase();
}

/** Build the add-skill operation view once. The view only consumes this result. */
export function buildSkillInstallViewModel(
  available: readonly AvailableSkill[],
  operations: readonly SkillOperation[],
  selectedRoots: readonly string[],
  skillFilter: SkillInstallFilter,
  search: string,
  replaceExisting = false,
): SkillInstallViewModel {
  const dependencyByName = new Map(available.map((skill) => [skill.name, skill.dependencies]));
  const rawOperationByName = new Map(operations.map((operation) => [operation.name, operation]));
  const selectableSkills = available.filter((skill) => {
    const status = rawOperationByName.get(skill.name)?.status;
    return status === undefined || status === SkillOperationStatus.Planned || status === SkillOperationStatus.Ready
      || status === SkillOperationStatus.AlreadyExists || status === SkillOperationStatus.AlreadyInstalled || status === SkillOperationStatus.Replace;
  });
  const selectableNameSet = new Set(selectableSkills.map((skill) => skill.name));
  const selected = expandSkillDependencies(selectedRoots, dependencyByName, selectableNameSet);
  const selectedSet = new Set(selected);
  const selectedRootSet = new Set(selectedRoots);
  const selectedHasExisting = selected.some((name) => {
    const status = rawOperationByName.get(name)?.status;
    return status === SkillOperationStatus.AlreadyExists || status === SkillOperationStatus.AlreadyInstalled || status === SkillOperationStatus.Replace;
  });
  const operationByName = new Map(operations.map((operation) => [
    operation.name,
    selectedExistingSkillOperation(operation, selectedSet.has(operation.name) && replaceExisting) ?? operation,
  ]));
  const dependencyReasonsByName = new Map<string, string[]>();
  for (const root of selectedRootSet) {
    for (const dependency of expandSkillDependencies([root], dependencyByName, selectableNameSet)) {
      if (dependency !== root) dependencyReasonsByName.set(dependency, [...(dependencyReasonsByName.get(dependency) ?? []), root]);
    }
  }
  for (const [name, reasons] of dependencyReasonsByName) dependencyReasonsByName.set(name, [...new Set(reasons)].sort());
  const { newSkills, existingSkills } = partitionSkillOperations(available, rawOperationByName);
  const statusFiltered = skillFilter === SkillInstallFilter.All ? [...available] : skillFilter === SkillInstallFilter.New ? newSkills : existingSkills;
  const normalizedSearch = search.trim().toLowerCase();
  const visibleAvailableSkills = normalizedSearch ? statusFiltered.filter((skill) => skillSearchText(skill).includes(normalizedSearch)) : statusFiltered;
  const searchMatches = normalizedSearch ? visibleAvailableSkills.map((skill) => skill.name) : [];
  return {
    selectableSkills,
    selectableNameSet,
    selected,
    selectedSet,
    selectedRootSet,
    selectedHasExisting,
    operationByName,
    dependencyReasonsByName,
    newSkills,
    existingSkills,
    visibleAvailableSkills,
    searchMatches,
    searchMatchSet: new Set(searchMatches),
  };
}
