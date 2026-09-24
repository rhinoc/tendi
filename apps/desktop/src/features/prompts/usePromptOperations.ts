import { useCallback, useRef, useState } from "react";

import { promptActionLabels } from "../../lib/action-labels.ts";
import { normalizePromptTags, type PromptDraft } from "../../lib/prompt-model.ts";

function operationErrorMessage(error: unknown, fallback: string): string {
  if (error instanceof Error && error.message) return error.message;
  if (typeof error === "string" && error) return error;
  return fallback;
}

export function usePromptOperations({
  onSavePrompt,
  onDeletePrompts,
}: {
  onSavePrompt: (draft: PromptDraft) => Promise<boolean>;
  onDeletePrompts: (ids: string[]) => Promise<boolean>;
}) {
  const [saving, setSaving] = useState(false);
  const [saveError, setSaveError] = useState("");
  const [deletingPromptIds, setDeletingPromptIds] = useState<string[]>([]);
  const savingRef = useRef(false);
  const deletingPromptIdsRef = useRef(new Set<string>());
  const filterDeletablePromptIds = useCallback((ids: readonly string[]) => (
    ids.filter((id) => !deletingPromptIdsRef.current.has(id))
  ), []);

  const savePrompt = useCallback(async (draft: PromptDraft): Promise<boolean> => {
    if (savingRef.current) return false;
    savingRef.current = true;
    setSaving(true);
    setSaveError("");
    try {
      const saved = await onSavePrompt({ ...draft, tags: normalizePromptTags(draft.tags) });
      if (!saved) setSaveError(promptActionLabels.saveFailed);
      return saved;
    } catch (error) {
      setSaveError(operationErrorMessage(error, promptActionLabels.saveFailed));
      return false;
    } finally {
      savingRef.current = false;
      setSaving(false);
    }
  }, [onSavePrompt]);

  const deletePrompts = useCallback(async (ids: string[]): Promise<string[] | null> => {
    const pendingIds = [...new Set(filterDeletablePromptIds(ids))];
    if (pendingIds.length === 0) return [];
    for (const id of pendingIds) deletingPromptIdsRef.current.add(id);
    setDeletingPromptIds([...deletingPromptIdsRef.current]);
    try {
      const deleted = await onDeletePrompts(pendingIds);
      return deleted ? pendingIds : null;
    } finally {
      for (const id of pendingIds) deletingPromptIdsRef.current.delete(id);
      setDeletingPromptIds([...deletingPromptIdsRef.current]);
    }
  }, [filterDeletablePromptIds, onDeletePrompts]);

  const clearSaveError = useCallback(() => setSaveError(""), []);

  return {
    saving,
    saveError,
    savePrompt,
    clearSaveError,
    deletingPromptIds,
    filterDeletablePromptIds,
    deletePrompts,
  };
}
