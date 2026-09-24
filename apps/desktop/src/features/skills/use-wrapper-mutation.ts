import { useCallback, useRef, useState } from "react";

import type { WrapperArgs } from "../../lib/skills.ts";
import type { SkillChangeResponse } from "../../lib/runtime-gateway.ts";

export type WrapperMutationOutcome =
  | { status: "succeeded"; result: SkillChangeResponse }
  | { status: "failed" }
  | { status: "busy" };

type UseWrapperMutationOptions = {
  onApplyWrapper: (args: WrapperArgs) => Promise<SkillChangeResponse>;
};

function commandErrorMessage(error: unknown) {
  if (typeof error === "string" && error.trim()) return error;
  if (error instanceof Error && error.message.trim()) return error.message;
  return "Could not create the wrapper skill.";
}

export function useWrapperMutation({ onApplyWrapper }: UseWrapperMutationOptions) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const inFlightRef = useRef(false);

  const clearError = useCallback(() => {
    setError("");
  }, []);

  const apply = useCallback(async (args: WrapperArgs): Promise<WrapperMutationOutcome> => {
    if (inFlightRef.current) return { status: "busy" };

    inFlightRef.current = true;
    setBusy(true);
    setError("");
    try {
      const result = await onApplyWrapper(args);
      return { status: "succeeded", result };
    } catch (applyError) {
      setError(commandErrorMessage(applyError));
      return { status: "failed" };
    } finally {
      inFlightRef.current = false;
      setBusy(false);
    }
  }, [onApplyWrapper]);

  return { apply, busy, clearError, error };
}
