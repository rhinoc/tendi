import { useCallback, useRef, useState } from "react";

import { selectionDeleteErrorLabel } from "../../lib/action-labels.ts";
import type { CatalogMutationResponse } from "../../lib/runtime-gateway.ts";

type RuleDeleteCandidate = {
  rule: { path: string };
};

type UseRuleOperationsOptions = {
  onDeleteRules?: (paths: string[]) => Promise<CatalogMutationResponse>;
  clearPendingDelete: () => void;
  clearSelection: () => void;
};

function operationError(value: CatalogMutationResponse | undefined): string | null {
  if (!value || typeof value !== "object") return null;
  const error = "error" in value ? value.error : undefined;
  return typeof error === "string" ? error : null;
}

export function useRuleOperations({
  onDeleteRules,
  clearPendingDelete,
  clearSelection,
}: UseRuleOperationsOptions) {
  const [deleting, setDeleting] = useState(false);
  const [deleteError, setDeleteError] = useState("");
  const deletingRef = useRef(false);

  const filterDeletableRules = useCallback(<T extends RuleDeleteCandidate>(items: readonly T[]): T[] => (
    items.filter((item) => Boolean(item.rule.path))
  ), []);

  const confirmDeleteRules = useCallback(async (targets: readonly RuleDeleteCandidate[]): Promise<boolean> => {
    const deletableTargets = filterDeletableRules(targets);
    if (deletableTargets.length === 0 || deletingRef.current) return false;
    deletingRef.current = true;
    setDeleting(true);
    setDeleteError("");
    try {
      const result = await onDeleteRules?.(deletableTargets.map((item) => item.rule.path));
      const error = operationError(result);
      if (error) {
        setDeleteError(error);
        return false;
      }
      if (!result) {
        setDeleteError(selectionDeleteErrorLabel("rule", deletableTargets.length));
        return false;
      }
      clearPendingDelete();
      clearSelection();
      return true;
    } catch (error) {
      setDeleteError(`${error}`);
      return false;
    } finally {
      deletingRef.current = false;
      setDeleting(false);
    }
  }, [clearPendingDelete, clearSelection, filterDeletableRules, onDeleteRules]);

  const clearDeleteError = useCallback(() => setDeleteError(""), []);

  return {
    deleting,
    deleteError,
    clearDeleteError,
    filterDeletableRules,
    confirmDeleteRules,
  };
}
