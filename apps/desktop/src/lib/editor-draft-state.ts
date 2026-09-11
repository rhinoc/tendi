import { useSyncExternalStore } from "react";

export type EditorDraft = {
  content: string;
  originalContent: string;
  sha256: string;
};

const emptyDraft = (): EditorDraft => ({ content: "", originalContent: "", sha256: "" });
const draftsByResource = new Map<string, EditorDraft>();
const listenersByResource = new Map<string, Set<() => void>>();

export function getEditorDraft(resourceKey: string): EditorDraft {
  const existing = draftsByResource.get(resourceKey);
  if (existing) return existing;
  const initial = emptyDraft();
  draftsByResource.set(resourceKey, initial);
  return initial;
}

export function subscribeEditorDraft(resourceKey: string, listener: () => void) {
  const listeners = listenersByResource.get(resourceKey) ?? new Set<() => void>();
  listeners.add(listener);
  listenersByResource.set(resourceKey, listeners);
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0) listenersByResource.delete(resourceKey);
  };
}

export function updateEditorDraft(
  resourceKey: string,
  update: EditorDraft | ((current: EditorDraft) => EditorDraft),
) {
  const current = getEditorDraft(resourceKey);
  const next = typeof update === "function" ? update(current) : update;
  if (next === current) return current;
  draftsByResource.set(resourceKey, next);
  listenersByResource.get(resourceKey)?.forEach((listener) => listener());
  return next;
}

export function clearEditorDraft(resourceKey: string) {
  return updateEditorDraft(resourceKey, emptyDraft());
}

export function discardEditorDraft(resourceKey: string) {
  return updateEditorDraft(resourceKey, (current) => ({
    ...current,
    content: current.originalContent,
  }));
}

export function useEditorDraft(resourceKey: string) {
  return useSyncExternalStore(
    (listener) => subscribeEditorDraft(resourceKey, listener),
    () => getEditorDraft(resourceKey),
    () => getEditorDraft(resourceKey),
  );
}
