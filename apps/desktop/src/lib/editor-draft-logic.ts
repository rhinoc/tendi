export type EditorDraft = {
  content: string;
  originalContent: string;
  sha256: string;
};

export function emptyEditorDraft(): EditorDraft {
  return { content: "", originalContent: "", sha256: "" };
}

export function isEditorDraftDirty(draft: EditorDraft): boolean {
  return draft.content !== draft.originalContent;
}

export function cleanEditorDraft(content: string, sha256: string): EditorDraft {
  return { content, originalContent: content, sha256 };
}

export function hydrateEditorDraft(
  current: EditorDraft | undefined,
  content: string,
  sha256: string,
): EditorDraft {
  return current && isEditorDraftDirty(current)
    ? current
    : cleanEditorDraft(content, sha256);
}

export function discardEditorDraftChanges(draft: EditorDraft): EditorDraft {
  return { ...draft, content: draft.originalContent };
}

export function retainActiveAndDirtyDrafts<T extends EditorDraft>(
  drafts: Record<string, T>,
  activeResource: string,
): Record<string, T> {
  return Object.fromEntries(
    Object.entries(drafts).filter(([resource, draft]) => (
      resource === activeResource || isEditorDraftDirty(draft)
    )),
  );
}

export function discardCleanDrafts<T extends EditorDraft>(drafts: Record<string, T>): Record<string, T> {
  return Object.fromEntries(
    Object.entries(drafts).filter(([, draft]) => isEditorDraftDirty(draft)),
  );
}

export type EditorSnapshot = { content: string; sha256: string };

export type ExternalDraftChange =
  | { kind: "unchanged"; draft: EditorDraft }
  | { kind: "updated"; draft: EditorDraft }
  | { kind: "conflict"; draft: EditorDraft; snapshot: EditorSnapshot };

export function reconcileExternalEditorDraft(
  draft: EditorDraft,
  snapshot: EditorSnapshot,
): ExternalDraftChange {
  if (snapshot.sha256 === draft.sha256) return { kind: "unchanged", draft };
  if (isEditorDraftDirty(draft)) return { kind: "conflict", draft, snapshot };
  return { kind: "updated", draft: cleanEditorDraft(snapshot.content, snapshot.sha256) };
}
