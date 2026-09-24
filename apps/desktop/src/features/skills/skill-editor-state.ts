import { useSyncExternalStore } from "react";
import {
  discardCleanDrafts,
  hydrateEditorDraft,
  retainActiveAndDirtyDrafts,
  type EditorDraft,
} from "../../lib/editor-draft-logic.ts";

export type SkillDraft = EditorDraft;

export type SkillEditorState = {
  activePath: string;
  selectedPath: string;
  fileTreeCollapsed: boolean;
  collapsedFolders: Set<string>;
  createdPaths: Set<string>;
  drafts: Record<string, SkillDraft>;
};

export type SkillEditorStateValue<T> = T | ((current: T) => T);

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

export { discardCleanDrafts, retainActiveAndDirtyDrafts };

export function setSkillEditorActivePath(skillId: string, activePath: string) {
  return updateSkillEditorState(skillId, (current) => ({
    ...current,
    activePath,
    drafts: retainActiveAndDirtyDrafts(current.drafts, activePath),
  }));
}

export function hydrateSkillDraft(
  drafts: Record<string, SkillDraft>,
  path: string,
  content: string,
  sha256: string,
  activePath: string,
) {
  const nextDraft = hydrateEditorDraft(drafts[path], content, sha256);
  return retainActiveAndDirtyDrafts({ ...drafts, [path]: nextDraft }, activePath);
}

export function useSkillEditorState(skillId: string) {
  return useSyncExternalStore(
    (listener) => subscribeSkillEditorState(skillId, listener),
    () => getSkillEditorState(skillId),
    () => getSkillEditorState(skillId),
  );
}
