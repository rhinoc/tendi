import { useCallback, useState } from "react";

import {
  isMcpMutationDelta,
  mcpEnableBlockReason,
  mcpOperationError,
  mcpRowKey,
  supportsMcpProbe,
  type McpRecord,
} from "../../lib/mcp.ts";
import type { McpMutationResponse } from "../../lib/runtime-gateway.ts";

type McpOperationOptions = {
  onSetMcpEnabled?: (row: McpRecord, enabled: boolean) => Promise<McpMutationResponse>;
  onSetMcpEnabledMany?: (rows: McpRecord[], enabled: boolean) => Promise<McpMutationResponse>;
  onProbeMcp?: (row: McpRecord) => Promise<McpMutationResponse>;
  removeSelection: (id: string) => void;
  clearSelection: () => void;
};

export function useMcpOperations({
  onSetMcpEnabled,
  onSetMcpEnabledMany,
  onProbeMcp,
  removeSelection,
  clearSelection,
}: McpOperationOptions) {
  const [updatingKeys, setUpdatingKeys] = useState<Set<string>>(() => new Set());
  const [probingKey, setProbingKey] = useState("");
  const [error, setError] = useState("");
  const operationBusy = updatingKeys.size > 0 || Boolean(probingKey);
  const clearError = useCallback(() => setError(""), []);

  const setMcpEnabled = useCallback(async (row: McpRecord, enabled: boolean) => {
    if (operationBusy || mcpEnableBlockReason(row)) return;
    const key = mcpRowKey(row);
    setUpdatingKeys(new Set([key]));
    setError("");
    try {
      const result = await onSetMcpEnabled?.(row, enabled);
      const resultError = mcpOperationError(result);
      if (resultError) setError(resultError);
      else if (!isMcpMutationDelta(result)) setError("Could not update MCP server.");
      else removeSelection(key);
    } catch (reason) {
      setError(`${reason}`);
    } finally {
      setUpdatingKeys(new Set());
    }
  }, [onSetMcpEnabled, operationBusy, removeSelection]);

  const setSelectedMcpEnabled = useCallback(async (targets: McpRecord[], enabled: boolean) => {
    if (operationBusy || targets.length === 0) return;
    setUpdatingKeys(new Set(targets.map(mcpRowKey)));
    setError("");
    try {
      if (targets.length > 1 && onSetMcpEnabledMany) {
        const result = await onSetMcpEnabledMany(targets, enabled);
        const resultError = mcpOperationError(result);
        if (resultError) setError(resultError);
        else if (!isMcpMutationDelta(result)) setError("Could not update MCP servers.");
        else clearSelection();
        return;
      }
      for (const row of targets) {
        const result = await onSetMcpEnabled?.(row, enabled);
        const resultError = mcpOperationError(result);
        if (resultError) {
          setError(resultError);
          return;
        }
        if (!isMcpMutationDelta(result)) {
          setError("Could not update MCP servers.");
          return;
        }
      }
      clearSelection();
    } catch (reason) {
      setError(`${reason}`);
    } finally {
      setUpdatingKeys(new Set());
    }
  }, [clearSelection, onSetMcpEnabled, onSetMcpEnabledMany, operationBusy]);

  const probeMcp = useCallback(async (row: McpRecord) => {
    if (operationBusy || !onProbeMcp || !supportsMcpProbe(row.transport)) return;
    const key = mcpRowKey(row);
    setProbingKey(key);
    setError("");
    try {
      const result = await onProbeMcp(row);
      const resultError = mcpOperationError(result);
      if (resultError) setError(resultError);
      else if (!isMcpMutationDelta(result)) setError("Could not check MCP connection.");
    } catch (reason) {
      setError(`${reason}`);
    } finally {
      setProbingKey("");
    }
  }, [onProbeMcp, operationBusy]);

  return {
    updatingKeys,
    probingKey,
    operationBusy,
    error,
    clearError,
    setMcpEnabled,
    setSelectedMcpEnabled,
    probeMcp,
  };
}
