import type { SkillUpdateReport } from "../lib/skill-updates.ts";

type SkillUpdateEventPayload = {
  status: string;
  updates: SkillUpdateReport[];
  error: string | null;
};

export function applySkillUpdateEvent(
  payload: SkillUpdateEventPayload,
  options: {
    setSkillUpdateReports: (updates: SkillUpdateReport[]) => void;
    setSkillUpdateError: (message: string) => void;
    setCheckingSkillUpdates: (checking: boolean) => void;
    setSkillUpdateCheckActive: (active: boolean) => void;
  },
): void {
  if (payload.status === "completed") {
    // An update check only produces reports; it does not mutate the skill
    // projection. Do not replace or refresh the list here: doing so makes a
    // background check reorder the visible table later.
    options.setSkillUpdateReports(payload.updates);
    options.setSkillUpdateError("");
  } else {
    options.setSkillUpdateError(payload.error || "Update check failed");
  }
  // The daemon owns the asynchronous check after the RPC returns `started`.
  // Release the client-side guard only at its terminal event so the next
  // check can start after completed or failed, but never while it is running.
  options.setSkillUpdateCheckActive(false);
  options.setCheckingSkillUpdates(false);
}
