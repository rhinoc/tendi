import { useCallback, useState } from "react";

import { selectionDeleteErrorLabel } from "../../lib/action-labels.ts";
import {
  hookDeleteIdentity,
  hookEnableDisabledReason,
  hookReviewDisabledReason,
  hookSelectionTargets,
  type HookItem,
  type HookRecord,
} from "../../lib/hooks.ts";
import type { CatalogMutationResponse } from "../../lib/runtime-gateway.ts";

type HookOperationOptions = {
  rows: HookRecord[];
  onDeleteHook?: (hook: HookRecord) => Promise<CatalogMutationResponse>;
  onDeleteHooks?: (hooks: HookRecord[]) => Promise<CatalogMutationResponse>;
  onSetHookEnabled?: (hook: HookRecord, enabled: boolean) => Promise<CatalogMutationResponse>;
  onSetHooksEnabled?: (hooks: HookRecord[], enabled: boolean) => Promise<CatalogMutationResponse>;
  onReviewHook?: (hook: HookRecord) => Promise<CatalogMutationResponse>;
  clearSelection: () => void;
};

function operationError(result: CatalogMutationResponse | undefined): string | undefined {
  if (!result || !("error" in result)) return undefined;
  return typeof result.error === "string" ? result.error : undefined;
}

export function useHookOperations({
  rows,
  onDeleteHook,
  onDeleteHooks,
  onSetHookEnabled,
  onSetHooksEnabled,
  onReviewHook,
  clearSelection,
}: HookOperationOptions) {
  const [deletingKey, setDeletingKey] = useState("");
  const [updatingEnabledKeys, setUpdatingEnabledKeys] = useState<Set<string>>(() => new Set());
  const [reviewingKey, setReviewingKey] = useState("");
  const [error, setError] = useState("");
  const clearError = useCallback(() => setError(""), []);

  const setHookEnabled = useCallback(async (item: HookItem, enabled: boolean) => {
    if (!item?.hook || updatingEnabledKeys.size > 0 || hookEnableDisabledReason(item.hook)) return;
    setUpdatingEnabledKeys(new Set([item.key]));
    setError("");
    try {
      const result = await onSetHookEnabled?.(item.hook, enabled);
      const updateError = operationError(result);
      if (updateError) setError(updateError);
      else if (!result) setError("Could not update hook.");
    } catch (reason) {
      setError(`${reason}`);
    } finally {
      setUpdatingEnabledKeys(new Set());
    }
  }, [onSetHookEnabled, updatingEnabledKeys.size]);

  const setSelectedHooksEnabled = useCallback(async (items: HookItem[], enabled: boolean) => {
    if (updatingEnabledKeys.size > 0) return;
    const targets = items.filter((item) => (
      !hookEnableDisabledReason(item.hook) && Boolean(item.hook.enabled) !== enabled
    ));
    if (targets.length === 0) return;

    setUpdatingEnabledKeys(new Set(targets.map((item) => item.key)));
    setError("");
    try {
      if (targets.length > 1 && onSetHooksEnabled) {
        const result = await onSetHooksEnabled(targets.map((item) => item.hook), enabled);
        const updateError = operationError(result);
        if (updateError) setError(updateError);
        else if (!result) setError("Could not update selected hooks.");
        else clearSelection();
        return;
      }

      let firstError = "";
      for (const item of targets) {
        const identity = hookDeleteIdentity(item.hook);
        const hook = rows.find((row) => hookDeleteIdentity(row) === identity);
        if (!hook) {
          firstError ||= "Could not find selected hook.";
          continue;
        }
        try {
          const result = await onSetHookEnabled?.(hook, enabled);
          const updateError = operationError(result);
          if (updateError) firstError ||= updateError;
          else if (!result) firstError ||= "Could not update selected hooks.";
        } catch (reason) {
          firstError ||= `${reason}`;
        }
      }
      if (firstError) setError(firstError);
      else clearSelection();
    } catch (reason) {
      setError(`${reason}`);
    } finally {
      setUpdatingEnabledKeys(new Set());
    }
  }, [clearSelection, onSetHookEnabled, onSetHooksEnabled, rows, updatingEnabledKeys.size]);

  const reviewHook = useCallback(async (item: HookItem) => {
    if (!item?.hook || reviewingKey || hookReviewDisabledReason(item.hook)) return;
    setReviewingKey(item.key);
    setError("");
    try {
      const result = await onReviewHook?.(item.hook);
      const reviewError = operationError(result);
      if (reviewError) setError(reviewError);
      else if (!result) setError("Could not review hook.");
    } catch (reason) {
      setError(`${reason}`);
    } finally {
      setReviewingKey("");
    }
  }, [onReviewHook, reviewingKey]);

  const deleteHooks = useCallback(async (targets: HookItem[]) => {
    const deletableTargets = hookSelectionTargets(targets).deletable;
    if (deletableTargets.length === 0 || deletingKey) return;
    setError("");
    if (deletableTargets.length > 1 && onDeleteHooks) {
      setDeletingKey("batch");
      try {
        const result = await onDeleteHooks(deletableTargets.map((item) => item.hook));
        const deleteError = operationError(result);
        if (deleteError) setError(deleteError);
        else if (!result) setError(selectionDeleteErrorLabel("hook", deletableTargets.length));
      } catch (reason) {
        setError(`${reason}`);
      } finally {
        setDeletingKey("");
        clearSelection();
      }
      return;
    }
    try {
      for (const item of deletableTargets) {
        const identity = hookDeleteIdentity(item.hook);
        if (!identity) continue;
        const hook = rows.find((row) => hookDeleteIdentity(row) === identity);
        if (!hook) continue;
        setDeletingKey(identity);
        const result = await onDeleteHook?.(hook);
        const deleteError = operationError(result);
        if (deleteError) {
          setError(deleteError);
          break;
        }
        if (!result) {
          setError(selectionDeleteErrorLabel("hook", 1));
          break;
        }
      }
    } catch (reason) {
      setError(`${reason}`);
    } finally {
      setDeletingKey("");
      clearSelection();
    }
  }, [clearSelection, deletingKey, onDeleteHook, onDeleteHooks, rows]);

  return {
    deletingKey,
    updatingEnabledKeys,
    reviewingKey,
    error,
    clearError,
    setHookEnabled,
    setSelectedHooksEnabled,
    reviewHook,
    deleteHooks,
  };
}
