import { useSyncExternalStore } from "react";

export type SkillDraft = {
  content: string;
  originalContent: string;
  sha256: string;
};

export type SkillEditorState = {
  activePath: string;
  selectedPath: string;
  fileTreeCollapsed: boolean;
  collapsedFolders: Set<string>;
  createdPaths: Set<string>;
  drafts: Record<string, SkillDraft>;
};

export type SkillEditorStateValue<T> = T | ((current: T) => T);

const MAX_CLEAN_DRAFT_CHARS = 32 * 1024 * 1024;
const stateBySkillId = new Map<string, SkillEditorState>();
const listenersBySkillId = new Map<string, Set<() => void>>();

function createInitialState(): SkillEditorState {
  return {
    activePath: "",
    selectedPath: "",
    fileTreeCollapsed: false,
    collapsedFolders: new Set<string>(),
    createdPaths: new Set<string>(),
    drafts: {},
  };
}

export function getSkillEditorState(skillId: string): SkillEditorState {
  const existing = stateBySkillId.get(skillId);
  if (existing) return existing;
  const initial = createInitialState();
  stateBySkillId.set(skillId, initial);
  return initial;
}

export function subscribeSkillEditorState(skillId: string, listener: () => void) {
  const listeners = listenersBySkillId.get(skillId) ?? new Set<() => void>();
  listeners.add(listener);
  listenersBySkillId.set(skillId, listeners);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) listenersBySkillId.delete(skillId);
  };
}

export function updateSkillEditorState(
  skillId: string,
  update: SkillEditorState | ((current: SkillEditorState) => SkillEditorState),
) {
  const current = getSkillEditorState(skillId);
  const next = typeof update === "function" ? update(current) : update;
  if (next === current) return current;
  stateBySkillId.set(skillId, next);
  listenersBySkillId.get(skillId)?.forEach((listener) => listener());
  return next;
}

export function updateSkillEditorField<K extends keyof SkillEditorState>(
  skillId: string,
  field: K,
  value: SkillEditorStateValue<SkillEditorState[K]>,
) {
  return updateSkillEditorState(skillId, (current) => ({
    ...current,
    [field]: typeof value === "function"
      ? (value as (current: SkillEditorState[K]) => SkillEditorState[K])(current[field])
      : value,
  }));
}

export function trimCleanDrafts(drafts: Record<string, SkillDraft>, activePath: string) {
  let cleanChars = 0;
  const evictable: Array<[string, SkillDraft]> = [];
  for (const [path, draft] of Object.entries(drafts)) {
    if (path === activePath || draft.content !== draft.originalContent) continue;
    cleanChars += draft.content.length;
    evictable.push([path, draft]);
  }
  if (cleanChars <= MAX_CLEAN_DRAFT_CHARS) return drafts;

  const next = { ...drafts };
  for (const [path, draft] of evictable) {
    if (cleanChars <= MAX_CLEAN_DRAFT_CHARS) break;
    delete next[path];
    cleanChars -= draft.content.length;
  }
  return next;
}

export function hydrateSkillDraft(
  drafts: Record<string, SkillDraft>,
  path: string,
  content: string,
  sha256: string,
  activePath: string,
) {
  const existing = drafts[path];
  const nextDraft = existing && existing.content !== existing.originalContent
    ? existing
    : { content, originalContent: content, sha256 };
  return trimCleanDrafts({ ...drafts, [path]: nextDraft }, activePath);
}

export function discardDirtyDrafts(drafts: Record<string, SkillDraft>) {
  return Object.fromEntries(
    Object.entries(drafts).filter(([, draft]) => draft.content === draft.originalContent),
  );
}

export function useSkillEditorState(skillId: string) {
  return useSyncExternalStore(
    (listener) => subscribeSkillEditorState(skillId, listener),
    () => getSkillEditorState(skillId),
    () => getSkillEditorState(skillId),
  );
}
