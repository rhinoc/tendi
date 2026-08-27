import type { SessionResumeTarget } from "./sessions.ts";

export type SessionResumeState = "idle" | "loading" | "success" | "error";

export function sessionResumeTargetForMenu(
  target: SessionResumeTarget,
  inferredTarget?: Exclude<SessionResumeTarget, "auto">,
): SessionResumeTarget {
  return target === "auto" ? inferredTarget ?? "auto" : target;
}

export function sessionResumeLabel(state: SessionResumeState, target: SessionResumeTarget): string {
  const targetSuffix = target === "app" || target === "terminal" ? ` in ${target}` : "";
  if (state === "loading") return `Opening session${targetSuffix}`;
  if (state === "success") return `Session opened${targetSuffix}`;
  if (state === "error") return `Could not open session${targetSuffix}`;
  return targetSuffix ? `Resume${targetSuffix}` : "Resume";
}

export function sessionResumeErrorMessage(): string {
  return `${sessionResumeLabel("error", "auto")}. Try again.`;
}
